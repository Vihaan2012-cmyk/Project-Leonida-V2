//! The state behind every GameInput object: devices, their reading histories, callbacks and the queue a
//! dispatcher drains. Nothing here touches COM or a controller; both are fed in from outside, so tests drive
//! it with synthetic devices.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Instant;

use super::abi::{
    self, BatteryState, GAMEINPUT_E_DEVICE_DISCONNECTED, GAMEINPUT_E_READING_NOT_FOUND,
    GAMEINPUT_E_REFERENCE_READING_TOO_OLD, GamepadState, KeyState, MouseState, RumbleParams, kind,
    status,
};
use super::analog::{AnalogMove, WheelPulses};
use super::com;
use super::config::{InputConfig, MouseButton, VirtualPadMode};
use super::curve::MouseLook;
use super::device::{self, Class, InfoBlock};
use super::mapping::{self, LastUsed};

pub type Job = Box<dyn FnOnce() + Send>;

pub type ReadingFn = unsafe extern "win64" fn(u64, *mut c_void, *mut c_void, bool);
pub type DeviceFn = unsafe extern "win64" fn(u64, *mut c_void, *mut c_void, u64, u32, u32);
pub type GuideFn = unsafe extern "win64" fn(u64, *mut c_void, *mut c_void, u64, bool);

const HISTORY_MAX: usize = 512;
const HISTORY_AGE_US: u64 = 500_000;

/// One reading of one device. Readings are immutable; the title can hold one long after it left the history.
#[derive(Debug, Clone, Default)]
pub struct Reading {
    pub device: usize,
    /// Position in the order the hub took readings, across devices.
    pub order: u64,
    /// The device's own sequence number.
    pub sequence: u64,
    /// Microseconds.
    pub timestamp: u64,
    pub kinds: u32,
    pub pad: GamepadState,
    pub keys: Vec<KeyState>,
    pub mouse: MouseState,
}

impl Reading {
    fn same_input(&self, other: &Reading) -> bool {
        self.pad == other.pad && self.keys == other.keys && self.mouse == other.mouse
    }
}

/// What a controller is, as its backend knows it.
#[derive(Debug, Clone)]
pub struct PadIdentity {
    pub uuid: [u8; 16],
    pub name: String,
    pub vendor: u16,
    pub product: u16,
}

/// Input the window owner forwards from the game window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawInput {
    Key {
        virtual_key: u8,
        scan_code: u32,
        down: bool,
    },
    /// Relative movement in counts, positive down and right.
    MouseMove {
        dx: i32,
        dy: i32,
    },
    MouseButton {
        button: MouseButton,
        down: bool,
    },
    MouseWheel {
        dx: i32,
        dy: i32,
    },
    /// The game window gained or lost focus. Losing it releases every key and button.
    Focus(bool),
}

enum Func {
    Reading(ReadingFn),
    Device {
        kinds: u32,
        filter: u32,
        func: DeviceFn,
    },
    Guide(GuideFn),
    Keyboard,
}

struct Callback {
    token: u64,
    device: Option<usize>,
    kinds: u32,
    context: usize,
    func: Func,
}

struct Device {
    class: Class,
    source: Option<u64>,
    uuid: [u8; 16],
    name: String,
    info: Box<InfoBlock>,
    status: u32,
    /// The newest input, kept while unplugged so a reconnect starts from it.
    template: Reading,
    history: VecDeque<Arc<Reading>>,
    /// The newest reading that left the history.
    dropped: u64,
    sequence: u64,
    battery: BatteryState,
    rumble: RumbleParams,
    rumble_dirty: bool,
    com: usize,
}

struct Dispatcher {
    event: usize,
}

struct Inner {
    devices: Vec<Device>,
    callbacks: Vec<Callback>,
    next_token: u64,
    next_order: u64,
    config: InputConfig,
    focus: bool,
    keys: [bool; 256],
    pressed: Vec<(u8, u32)>,
    mouse_buttons: u32,
    mouse_position: (i64, i64),
    mouse_wheel: (i64, i64),
    look: MouseLook,
    walk: AnalogMove,
    wheel_pulses: WheelPulses,
    last_used: LastUsed,
    dispatcher: Option<Dispatcher>,
    queue: VecDeque<Job>,
    pending_async: Vec<Job>,
}

/// Index of the keyboard, mouse and virtual pad devices.
pub const KEYBOARD: usize = 0;
pub const MOUSE: usize = 1;
pub const VIRTUAL_PAD: usize = 2;

pub struct Hub {
    me: Weak<Hub>,
    inner: Mutex<Inner>,
    clock: Box<dyn Fn() -> u64 + Send + Sync>,
}

fn code_point(virtual_key: u8) -> u32 {
    match virtual_key {
        b'0'..=b'9' => u32::from(virtual_key),
        b'A'..=b'Z' => u32::from(virtual_key.to_ascii_lowercase()),
        0x20 => 0x20,
        _ => 0,
    }
}

impl Hub {
    pub fn new(config: InputConfig) -> Arc<Hub> {
        let epoch = Instant::now();
        Hub::with_clock(
            config,
            Box::new(move || epoch.elapsed().as_micros() as u64 + 1_000_000),
        )
    }

    pub fn with_clock(config: InputConfig, clock: Box<dyn Fn() -> u64 + Send + Sync>) -> Arc<Hub> {
        let mut devices = Vec::new();
        for (class, name, tag) in [
            (Class::Keyboard, "Keyboard", 1u8),
            (Class::Mouse, "Mouse", 2),
            (Class::VirtualPad, "Keyboard and mouse pad (Leonida)", 3),
        ] {
            let uuid = [
                0x4C, 0x45, 0x4F, 0x4E, 0x49, 0x44, 0x41, tag, 0, 0, 0, 0, 0, 0, 0, 0,
            ];
            devices.push(new_device(class, name, None, uuid, 0, 0, name));
        }
        let hub = Arc::new_cyclic(|me| Hub {
            me: me.clone(),
            inner: Mutex::new(Inner {
                devices,
                callbacks: Vec::new(),
                next_token: 1,
                next_order: 1,
                config,
                focus: true,
                keys: [false; 256],
                pressed: Vec::new(),
                mouse_buttons: 0,
                mouse_position: (0, 0),
                mouse_wheel: (0, 0),
                look: MouseLook::default(),
                walk: AnalogMove::default(),
                wheel_pulses: WheelPulses::default(),
                last_used: LastUsed::None,
                dispatcher: None,
                queue: VecDeque::new(),
                pending_async: Vec::new(),
            }),
            clock,
        });
        let mut jobs = Vec::new();
        {
            let mut inner = hub.lock();
            for d in [KEYBOARD, MOUSE] {
                jobs.extend(hub.set_status(&mut inner, d, status::ONLINE));
            }
            jobs.extend(hub.refresh_virtual_status(&mut inner));
        }
        hub.deliver(jobs);
        hub
    }

    /// In merged mode the controllers feed the keyboard and mouse pad and the title never sees them itself.
    fn is_hidden(inner: &Inner, d: usize) -> bool {
        inner.config.virtual_pad == VirtualPadMode::Merged
            && inner.devices.get(d).is_some_and(|x| x.class == Class::Pad)
    }

    /// Which device the player touched last.
    pub fn last_used(&self) -> LastUsed {
        self.lock().last_used
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn now(&self) -> u64 {
        (self.clock)()
    }

    pub fn config(&self) -> InputConfig {
        self.lock().config.clone()
    }

    pub fn set_config(&self, config: InputConfig) {
        let jobs = {
            let mut inner = self.lock();
            inner.config = config;
            let mut jobs = self.refresh_virtual_status(&mut inner);
            jobs.extend(self.update_virtual_pad(&mut inner));
            jobs
        };
        self.deliver(jobs);
    }

    fn arc(&self) -> Arc<Hub> {
        self.me
            .upgrade()
            .expect("the hub is alive while it is used")
    }

    /// Runs jobs now, or queues them for the dispatcher once the title made one.
    fn deliver(&self, jobs: Vec<Job>) {
        if jobs.is_empty() {
            return;
        }
        let mut inner = self.lock();
        if let Some(d) = &inner.dispatcher {
            let event = d.event;
            inner.queue.extend(jobs);
            drop(inner);
            com::event_set(event);
            return;
        }
        drop(inner);
        for job in jobs {
            job();
        }
    }

    // ---- devices ---------------------------------------------------------------------------------------

    pub fn device_count(&self) -> usize {
        self.lock().devices.len()
    }

    pub fn device_status(&self, d: usize) -> u32 {
        self.lock()
            .devices
            .get(d)
            .map_or(status::NONE, |x| x.status)
    }

    pub fn device_class(&self, d: usize) -> Option<Class> {
        self.lock().devices.get(d).map(|x| x.class)
    }

    pub fn device_name(&self, d: usize) -> Option<String> {
        self.lock().devices.get(d).map(|x| x.name.clone())
    }

    /// The device's info block. The pointer stays valid for the life of the process: devices are never removed.
    pub fn device_info(&self, d: usize) -> Option<*const abi::DeviceInfo> {
        self.lock()
            .devices
            .get(d)
            .map(|x| &x.info.info as *const abi::DeviceInfo)
    }

    pub fn device_com(&self, d: usize) -> Option<usize> {
        let mut inner = self.lock();
        let device = inner.devices.get_mut(d)?;
        if device.com == 0 {
            device.com = com::new_device_object(self.arc(), d) as usize;
        }
        Some(device.com)
    }

    /// The device a COM pointer stands for.
    pub fn device_of_com(&self, ptr: usize) -> Option<usize> {
        self.lock()
            .devices
            .iter()
            .position(|d| d.com == ptr && ptr != 0)
    }

    pub fn find_device(&self, id: &[u8; 32]) -> Option<usize> {
        self.lock()
            .devices
            .iter()
            .position(|d| &d.info.info.device_id == id)
    }

    pub fn battery(&self, d: usize) -> BatteryState {
        self.lock()
            .devices
            .get(d)
            .map(|x| x.battery)
            .unwrap_or_default()
    }

    fn set_status(&self, inner: &mut Inner, d: usize, new: u32) -> Vec<Job> {
        let old = inner.devices[d].status;
        if old == new {
            return Vec::new();
        }
        inner.devices[d].status = new;
        let mut jobs = Vec::new();
        if new & status::CONNECTED == 0 {
            let device = &mut inner.devices[d];
            device.dropped = device
                .history
                .back()
                .map_or(device.dropped, |r| r.order.max(device.dropped));
            device.history.clear();
        }
        let timestamp = self.now();
        let supported = inner.devices[d].class.supported_input();
        let hidden = Self::is_hidden(inner, d);
        for cb in &inner.callbacks {
            if hidden {
                break;
            }
            let Func::Device {
                kinds,
                filter,
                func,
            } = cb.func
            else {
                continue;
            };
            if kinds & supported == 0 || cb.device.is_some_and(|x| x != d) {
                continue;
            }
            if (old ^ new) & filter == 0 {
                continue;
            }
            jobs.push(self.device_job(cb.token, cb.context, func, d, timestamp, new, old));
        }
        if new & status::CONNECTED != 0 && old & status::CONNECTED == 0 {
            jobs.extend(self.push_reading(inner, d, true, |_| {}));
        }
        jobs
    }

    #[allow(clippy::too_many_arguments)]
    fn device_job(
        &self,
        token: u64,
        context: usize,
        func: DeviceFn,
        d: usize,
        timestamp: u64,
        current: u32,
        previous: u32,
    ) -> Job {
        let hub = self.arc();
        Box::new(move || {
            if let Some(device) = hub.device_com(d) {
                // SAFETY: the title registered `func` for exactly this call shape; the device object lives for
                // the whole process.
                unsafe {
                    func(
                        token,
                        context as *mut c_void,
                        device as *mut c_void,
                        timestamp,
                        current,
                        previous,
                    )
                };
            }
        })
    }

    fn refresh_virtual_status(&self, inner: &mut Inner) -> Vec<Job> {
        let physical = inner
            .devices
            .iter()
            .any(|d| d.class == Class::Pad && d.status & status::CONNECTED != 0);
        let wanted = match inner.config.virtual_pad {
            VirtualPadMode::Off => status::NONE,
            VirtualPadMode::Always | VirtualPadMode::Merged => status::ONLINE,
            VirtualPadMode::Auto if physical => status::NONE,
            VirtualPadMode::Auto => status::ONLINE,
        };
        self.set_status(inner, VIRTUAL_PAD, wanted)
    }

    // ---- readings --------------------------------------------------------------------------------------

    /// Takes a reading of device `d` if the input changed (or `force`), after `change` edits a copy of the
    /// device's last input.
    fn push_reading(
        &self,
        inner: &mut Inner,
        d: usize,
        force: bool,
        change: impl FnOnce(&mut Reading),
    ) -> Vec<Job> {
        let hidden = Self::is_hidden(inner, d);
        let mut next = inner.devices[d].template.clone();
        change(&mut next);
        if !force && next.same_input(&inner.devices[d].template) {
            return Vec::new();
        }
        let timestamp = self.now();
        let order = inner.next_order;
        inner.next_order += 1;
        let device = &mut inner.devices[d];
        device.sequence += 1;
        next.device = d;
        next.order = order;
        next.sequence = device.sequence;
        next.timestamp = timestamp;
        next.kinds = device.class.supported_input();
        device.template = next.clone();
        if device.status & status::CONNECTED == 0 {
            return Vec::new();
        }
        let reading = Arc::new(next);
        device.history.push_back(reading.clone());
        Self::prune(device, timestamp);
        let kinds = reading.kinds;
        let mut jobs = Vec::new();
        if hidden {
            return jobs;
        }
        for cb in &inner.callbacks {
            let Func::Reading(func) = cb.func else {
                continue;
            };
            if cb.kinds & kinds == 0 || cb.device.is_some_and(|x| x != d) {
                continue;
            }
            let hub = self.arc();
            let reading = reading.clone();
            let (token, context) = (cb.token, cb.context);
            jobs.push(Box::new(move || {
                let object = com::new_reading_object(hub, reading);
                // SAFETY: `func` was registered for this call shape; the reading object is ours until released.
                unsafe {
                    func(token, context as *mut c_void, object, false);
                    com::release(object);
                }
            }) as Job);
        }
        jobs
    }

    fn prune(device: &mut Device, now: u64) {
        while device.history.len() > HISTORY_MAX
            || (device.history.len() >= 2 && device.history[1].timestamp + HISTORY_AGE_US < now)
        {
            if let Some(old) = device.history.pop_front() {
                device.dropped = device.dropped.max(old.order);
            }
        }
    }

    fn prune_all(&self, inner: &mut Inner) {
        let now = self.now();
        for device in &mut inner.devices {
            Self::prune(device, now);
        }
    }

    /// Candidate devices for a query: the one named, or every connected one that supports a kind asked for.
    fn searched(inner: &Inner, kinds: u32, device: Option<usize>) -> Result<Vec<usize>, i32> {
        match device {
            Some(d) => {
                let dev = inner.devices.get(d).ok_or(GAMEINPUT_E_READING_NOT_FOUND)?;
                if dev.class.supported_input() & kinds == 0 || Self::is_hidden(inner, d) {
                    return Err(GAMEINPUT_E_READING_NOT_FOUND);
                }
                if dev.status & status::CONNECTED == 0 {
                    return Err(GAMEINPUT_E_DEVICE_DISCONNECTED);
                }
                Ok(vec![d])
            }
            None => Ok(inner
                .devices
                .iter()
                .enumerate()
                .filter(|(i, x)| {
                    x.status & status::CONNECTED != 0
                        && x.class.supported_input() & kinds != 0
                        && !Self::is_hidden(inner, *i)
                })
                .map(|(i, _)| i)
                .collect()),
        }
    }

    pub fn current_reading(&self, kinds: u32, device: Option<usize>) -> Result<Arc<Reading>, i32> {
        let mut inner = self.lock();
        self.prune_all(&mut inner);
        let searched = Self::searched(&inner, kinds, device)?;
        searched
            .iter()
            .filter_map(|&i| inner.devices[i].history.back())
            .max_by_key(|r| r.order)
            .cloned()
            .ok_or(GAMEINPUT_E_READING_NOT_FOUND)
    }

    pub fn next_reading(
        &self,
        reference: &Reading,
        kinds: u32,
        device: Option<usize>,
    ) -> Result<Arc<Reading>, i32> {
        self.step(reference, kinds, device, true)
    }

    pub fn previous_reading(
        &self,
        reference: &Reading,
        kinds: u32,
        device: Option<usize>,
    ) -> Result<Arc<Reading>, i32> {
        self.step(reference, kinds, device, false)
    }

    fn step(
        &self,
        reference: &Reading,
        kinds: u32,
        device: Option<usize>,
        forward: bool,
    ) -> Result<Arc<Reading>, i32> {
        let mut inner = self.lock();
        self.prune_all(&mut inner);
        let searched = Self::searched(&inner, kinds, device)?;
        let mut horizon = searched
            .iter()
            .map(|&i| inner.devices[i].dropped)
            .max()
            .unwrap_or(0);
        if device.is_none() {
            horizon = horizon.max(inner.devices[reference.device].dropped);
        }
        let candidates = searched
            .iter()
            .flat_map(|&i| inner.devices[i].history.iter())
            .filter(|r| r.kinds & kinds != 0);
        if forward {
            if horizon > reference.order {
                return Err(GAMEINPUT_E_REFERENCE_READING_TOO_OLD);
            }
            candidates
                .filter(|r| r.order > reference.order)
                .min_by_key(|r| r.order)
                .cloned()
                .ok_or(GAMEINPUT_E_READING_NOT_FOUND)
        } else {
            if horizon >= reference.order {
                return Err(GAMEINPUT_E_REFERENCE_READING_TOO_OLD);
            }
            candidates
                .filter(|r| r.order < reference.order)
                .max_by_key(|r| r.order)
                .cloned()
                .ok_or(GAMEINPUT_E_READING_NOT_FOUND)
        }
    }

    // ---- controllers -----------------------------------------------------------------------------------

    /// A controller appeared. `source` is the backend's id for it; the same source coming back gets the same
    /// device. Returns the device index.
    pub fn connect_pad(&self, source: u64, identity: &PadIdentity) -> usize {
        let (d, jobs) = {
            let mut inner = self.lock();
            let d = match inner.devices.iter().position(|x| x.source == Some(source)) {
                Some(d) => d,
                None => {
                    let twin = inner
                        .devices
                        .iter()
                        .filter(|x| x.class == Class::Pad && x.uuid == identity.uuid)
                        .count();
                    let tag = format!("t:{twin}:{}", identity.name);
                    inner.devices.push(new_device(
                        Class::Pad,
                        &identity.name,
                        Some(source),
                        identity.uuid,
                        identity.vendor,
                        identity.product,
                        &tag,
                    ));
                    inner.devices.len() - 1
                }
            };
            let mut jobs = self.set_status(&mut inner, d, status::ONLINE);
            jobs.extend(self.refresh_virtual_status(&mut inner));
            jobs.extend(self.update_virtual_pad(&mut inner));
            (d, jobs)
        };
        self.deliver(jobs);
        d
    }

    pub fn disconnect_pad(&self, source: u64) {
        let jobs = {
            let mut inner = self.lock();
            let Some(d) = inner.devices.iter().position(|x| x.source == Some(source)) else {
                return;
            };
            let mut jobs = self.set_status(&mut inner, d, status::NONE);
            inner.devices[d].rumble = RumbleParams::default();
            inner.devices[d].rumble_dirty = false;
            jobs.extend(self.refresh_virtual_status(&mut inner));
            jobs.extend(self.update_virtual_pad(&mut inner));
            jobs
        };
        self.deliver(jobs);
    }

    pub fn pad_state(&self, source: u64, state: GamepadState) {
        let jobs = {
            let mut inner = self.lock();
            let Some(d) = inner.devices.iter().position(|x| x.source == Some(source)) else {
                return;
            };
            let mut jobs = self.push_reading(&mut inner, d, false, |r| r.pad = state);
            if Self::is_hidden(&inner, d) {
                if mapping::is_activity(&state) {
                    inner.last_used = LastUsed::Pad;
                }
                jobs.extend(self.update_virtual_pad(&mut inner));
            }
            jobs
        };
        self.deliver(jobs);
    }

    pub fn pad_battery(&self, source: u64, battery: BatteryState) {
        let mut inner = self.lock();
        if let Some(d) = inner.devices.iter_mut().find(|x| x.source == Some(source)) {
            d.battery = battery;
        }
    }

    pub fn guide_button(&self, source: u64, pressed: bool) {
        let jobs = {
            let inner = self.lock();
            let Some(mut d) = inner.devices.iter().position(|x| x.source == Some(source)) else {
                return;
            };
            if Self::is_hidden(&inner, d) {
                d = VIRTUAL_PAD;
            }
            let timestamp = self.now();
            let mut jobs: Vec<Job> = Vec::new();
            for cb in &inner.callbacks {
                let Func::Guide(func) = cb.func else {
                    continue;
                };
                if cb.device.is_some_and(|x| x != d) {
                    continue;
                }
                let hub = self.arc();
                let (token, context) = (cb.token, cb.context);
                jobs.push(Box::new(move || {
                    if let Some(device) = hub.device_com(d) {
                        // SAFETY: `func` was registered for this call shape.
                        unsafe {
                            func(
                                token,
                                context as *mut c_void,
                                device as *mut c_void,
                                timestamp,
                                pressed,
                            )
                        };
                    }
                }));
            }
            jobs
        };
        self.deliver(jobs);
    }

    /// The title's rumble request for a device. Levels are clamped to 0..1 and scaled by the config.
    pub fn set_rumble(&self, d: usize, params: RumbleParams) {
        let mut inner = self.lock();
        let scale = if inner.config.rumble {
            inner.config.rumble_scale
        } else {
            0.0
        };
        let level = |v: f32| (v.clamp(0.0, 1.0) * scale).clamp(0.0, 1.0);
        if let Some(device) = inner.devices.get_mut(d) {
            let scaled = RumbleParams {
                low_frequency: level(params.low_frequency),
                high_frequency: level(params.high_frequency),
                left_trigger: level(params.left_trigger),
                right_trigger: level(params.right_trigger),
            };
            if scaled != device.rumble {
                device.rumble = scaled;
                device.rumble_dirty = true;
            }
        }
    }

    pub fn rumble(&self, d: usize) -> RumbleParams {
        self.lock()
            .devices
            .get(d)
            .map(|x| x.rumble)
            .unwrap_or_default()
    }

    /// Rumble levels that changed since the last call, by backend source.
    pub fn take_rumble(&self) -> Vec<(u64, RumbleParams)> {
        let mut inner = self.lock();
        let mut out = Vec::new();
        if inner.config.virtual_pad == VirtualPadMode::Merged
            && inner.devices[VIRTUAL_PAD].rumble_dirty
        {
            let params = inner.devices[VIRTUAL_PAD].rumble;
            inner.devices[VIRTUAL_PAD].rumble_dirty = false;
            for d in inner
                .devices
                .iter_mut()
                .filter(|d| d.class == Class::Pad && d.status & status::CONNECTED != 0)
            {
                d.rumble = params;
                d.rumble_dirty = false;
                out.extend(d.source.map(|s| (s, params)));
            }
        }
        out.extend(
            inner
                .devices
                .iter_mut()
                .filter(|d| d.rumble_dirty)
                .filter_map(|d| {
                    d.rumble_dirty = false;
                    Some((d.source?, d.rumble))
                }),
        );
        out
    }

    // ---- keyboard and mouse ----------------------------------------------------------------------------

    pub fn input(&self, event: RawInput) {
        let jobs = {
            let mut inner = self.lock();
            let mut jobs = Vec::new();
            match event {
                RawInput::Focus(focus) => {
                    inner.focus = focus;
                    if !focus {
                        jobs.extend(self.release_all(&mut inner));
                    }
                }
                RawInput::MouseMove { dx, dy } if inner.focus => {
                    inner.mouse_position.0 += i64::from(dx);
                    inner.mouse_position.1 += i64::from(dy);
                    let now = self.now();
                    if !super::direct::push(dx, dy) {
                        inner.look.push(now, i64::from(dx), i64::from(dy));
                    }
                    if dx.abs() + dy.abs() >= 2 {
                        inner.last_used = LastUsed::KeyboardMouse;
                    }
                    jobs.extend(self.publish_mouse(&mut inner));
                }
                RawInput::MouseMove { .. } => {}
                _ if !inner.focus => {}
                RawInput::Key {
                    virtual_key,
                    scan_code,
                    down,
                } => {
                    let held = inner.keys[virtual_key as usize];
                    if down != held {
                        inner.keys[virtual_key as usize] = down;
                        if down {
                            inner.last_used = LastUsed::KeyboardMouse;
                        }
                        Self::sync_modifiers(&mut inner.keys, virtual_key);
                        if down {
                            inner.pressed.push((virtual_key, scan_code));
                        } else {
                            inner.pressed.retain(|(k, _)| *k != virtual_key);
                        }
                        jobs.extend(self.publish_keys(&mut inner));
                        jobs.extend(self.update_virtual_pad(&mut inner));
                    }
                }
                RawInput::MouseButton { button, down } => {
                    if down {
                        inner.last_used = LastUsed::KeyboardMouse;
                        inner.mouse_buttons |= button.bit();
                    } else {
                        inner.mouse_buttons &= !button.bit();
                    }
                    jobs.extend(self.publish_mouse(&mut inner));
                    jobs.extend(self.update_virtual_pad(&mut inner));
                }
                RawInput::MouseWheel { dx, dy } => {
                    inner.mouse_wheel.0 += i64::from(dx);
                    inner.mouse_wheel.1 += i64::from(dy);
                    if dy != 0 {
                        let notches = (dy.unsigned_abs() / 120).max(1);
                        inner.wheel_pulses.push(usize::from(dy < 0), notches);
                        inner.last_used = LastUsed::KeyboardMouse;
                    }
                    jobs.extend(self.publish_mouse(&mut inner));
                }
            }
            jobs
        };
        self.deliver(jobs);
    }

    fn release_all(&self, inner: &mut Inner) -> Vec<Job> {
        inner.keys = [false; 256];
        inner.pressed.clear();
        inner.mouse_buttons = 0;
        inner.look.reset();
        inner.walk.reset();
        inner.wheel_pulses.reset();
        let mut jobs = self.publish_keys(inner);
        jobs.extend(self.publish_mouse(inner));
        jobs.extend(self.update_virtual_pad(inner));
        jobs
    }

    fn publish_keys(&self, inner: &mut Inner) -> Vec<Job> {
        let keys: Vec<KeyState> = inner
            .pressed
            .iter()
            .map(|&(virtual_key, scan_code)| KeyState {
                scan_code,
                code_point: code_point(virtual_key),
                virtual_key,
                is_dead_key: false,
            })
            .collect();
        self.push_reading(inner, KEYBOARD, false, |r| r.keys = keys)
    }

    fn publish_mouse(&self, inner: &mut Inner) -> Vec<Job> {
        let mouse = MouseState {
            buttons: inner.mouse_buttons,
            position_x: inner.mouse_position.0,
            position_y: inner.mouse_position.1,
            wheel_x: inner.mouse_wheel.0,
            wheel_y: inner.mouse_wheel.1,
        };
        self.push_reading(inner, MOUSE, false, |r| r.mouse = mouse)
    }

    /// A generic Shift, Control or Alt is down while either side is.
    fn sync_modifiers(keys: &mut [bool; 256], changed: u8) {
        for (generic, left, right) in [(0x10, 0xA0, 0xA1), (0x11, 0xA2, 0xA3), (0x12, 0xA4, 0xA5)] {
            if changed == left || changed == right {
                keys[generic as usize] = keys[left as usize] || keys[right as usize];
            }
        }
    }

    fn update_virtual_pad(&self, inner: &mut Inner) -> Vec<Job> {
        let now = self.now();
        let target = mapping::move_target(&inner.config, &inner.keys);
        let movement = inner.config.movement.clone();
        let left = inner.walk.step(now, target, &movement);
        let look = Self::look_settings(&inner.config);
        let right = inner.look.step(now, &look);
        let wheel = inner.wheel_pulses.step(now);
        let kbm = mapping::virtual_pad(
            &inner.config,
            &inner.keys,
            inner.mouse_buttons,
            wheel,
            left,
            right,
        );
        let state = if inner.config.virtual_pad == VirtualPadMode::Merged {
            let pads: Vec<GamepadState> = inner
                .devices
                .iter()
                .filter(|d| d.class == Class::Pad && d.status & status::CONNECTED != 0)
                .map(|d| d.template.pad)
                .collect();
            mapping::merge(&kbm, &pads, inner.last_used)
        } else {
            kbm
        };
        self.push_reading(inner, VIRTUAL_PAD, false, |r| r.pad = state)
    }

    fn look_settings(config: &InputConfig) -> super::curve::LookSettings {
        let mut look = config.look;
        look.sensitivity = look.sensitivity.map(|v| v * config.mouse_sensitivity);
        look
    }

    /// Advances the keyboard and mouse sticks to now, publishes what changed, and runs asynchronous
    /// enumerations. Call it at a steady rate (every 2 ms).
    pub fn tick(&self) {
        let jobs = {
            let mut inner = self.lock();
            let mut jobs = self.update_virtual_pad(&mut inner);
            jobs.append(&mut inner.pending_async);
            jobs
        };
        self.deliver(jobs);
    }

    // ---- callbacks -------------------------------------------------------------------------------------

    fn add_callback(&self, inner: &mut Inner, callback: impl FnOnce(u64) -> Callback) -> u64 {
        let token = inner.next_token;
        inner.next_token += 1;
        inner.callbacks.push(callback(token));
        token
    }

    pub fn register_reading_callback(
        &self,
        device: Option<usize>,
        kinds: u32,
        context: usize,
        func: ReadingFn,
    ) -> u64 {
        let mut inner = self.lock();
        self.add_callback(&mut inner, |token| Callback {
            token,
            device,
            kinds,
            context,
            func: Func::Reading(func),
        })
    }

    pub fn register_guide_callback(
        &self,
        device: Option<usize>,
        context: usize,
        func: GuideFn,
    ) -> u64 {
        let mut inner = self.lock();
        self.add_callback(&mut inner, |token| Callback {
            token,
            device,
            kinds: kind::GAMEPAD,
            context,
            func: Func::Guide(func),
        })
    }

    /// Accepted and never fired: nothing here changes the keyboard layout.
    pub fn register_keyboard_layout_callback(&self, device: Option<usize>, context: usize) -> u64 {
        let mut inner = self.lock();
        self.add_callback(&mut inner, |token| Callback {
            token,
            device,
            kinds: kind::KEYBOARD,
            context,
            func: Func::Keyboard,
        })
    }

    /// Registers a device callback. Devices that already match are reported with a previous status of none:
    /// before this returns when `blocking`, from the next tick otherwise.
    pub fn register_device_callback(
        &self,
        device: Option<usize>,
        kinds: u32,
        status_filter: u32,
        enumeration: u32,
        context: usize,
        func: DeviceFn,
    ) -> u64 {
        let (token, jobs) = {
            let mut inner = self.lock();
            let token = self.add_callback(&mut inner, |token| Callback {
                token,
                device,
                kinds,
                context,
                func: Func::Device {
                    kinds,
                    filter: status_filter,
                    func,
                },
            });
            let mut jobs = Vec::new();
            if enumeration == abi::ENUMERATE_ASYNC || enumeration == abi::ENUMERATE_BLOCKING {
                let timestamp = self.now();
                for (d, dev) in inner.devices.iter().enumerate() {
                    if dev.class.supported_input() & kinds != 0
                        && !Self::is_hidden(&inner, d)
                        && device.is_none_or(|x| x == d)
                        && dev.status & status_filter != 0
                    {
                        jobs.push(self.device_job(
                            token,
                            context,
                            func,
                            d,
                            timestamp,
                            dev.status,
                            status::NONE,
                        ));
                    }
                }
            }
            if enumeration == abi::ENUMERATE_ASYNC {
                inner.pending_async.extend(jobs);
                jobs = Vec::new();
            }
            (token, jobs)
        };
        for job in jobs {
            job();
        }
        token
    }

    /// Removes a callback, so it is not called again. Returns whether the token was registered.
    pub fn remove_callback(&self, token: u64) -> bool {
        let mut inner = self.lock();
        let before = inner.callbacks.len();
        inner.callbacks.retain(|c| c.token != token);
        inner.callbacks.len() != before
    }

    // ---- dispatcher ------------------------------------------------------------------------------------

    /// Makes callbacks queue for `Dispatch` instead of running on the polling thread. Returns the wait event.
    pub fn create_dispatcher(&self) -> usize {
        let mut inner = self.lock();
        if let Some(d) = &inner.dispatcher {
            return d.event;
        }
        let event = com::event_new();
        inner.dispatcher = Some(Dispatcher { event });
        event
    }

    pub fn dispatcher_event(&self) -> Option<usize> {
        self.lock().dispatcher.as_ref().map(|d| d.event)
    }

    /// Runs queued callbacks on the calling thread, oldest first: at least one, then no further one once
    /// `quota_us` has passed, and only those queued when the call started. Returns whether any remain.
    pub fn dispatch(&self, quota_us: u64) -> bool {
        let start = Instant::now();
        let count = self.lock().queue.len();
        for i in 0..count {
            if i > 0 && start.elapsed().as_micros() as u64 >= quota_us {
                break;
            }
            let job = self.lock().queue.pop_front();
            match job {
                Some(job) => job(),
                None => break,
            }
        }
        let inner = self.lock();
        let more = !inner.queue.is_empty();
        if let Some(d) = &inner.dispatcher {
            if more {
                com::event_set(d.event);
            } else {
                com::event_reset(d.event);
            }
        }
        more
    }
}

fn new_device(
    class: Class,
    name: &str,
    source: Option<u64>,
    uuid: [u8; 16],
    vendor: u16,
    product: u16,
    tag: &str,
) -> Device {
    let id = device::device_id(uuid, tag);
    Device {
        class,
        source,
        uuid,
        name: name.to_string(),
        info: device::build(class, name, vendor, product, id),
        status: status::NONE,
        template: Reading::default(),
        history: VecDeque::new(),
        dropped: 0,
        sequence: 0,
        battery: BatteryState::default(),
        rumble: RumbleParams::default(),
        rumble_dirty: false,
        com: 0,
    }
}
