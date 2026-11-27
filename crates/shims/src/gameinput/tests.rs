//! Drives the COM objects through their vtable slots, the way the title calls them, with synthetic devices.

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use super::abi::*;
use super::com;
use super::config::{InputConfig, MouseButton, VirtualPadMode, vk};
use super::hub::{Hub, PadIdentity, RawInput};

type P = *mut c_void;

unsafe fn slot<F: Copy>(object: P, index: usize) -> F {
    // SAFETY: every object starts with a vtable pointer; the slot holds a function of the stated type.
    unsafe {
        let vtable = *(object as *const *const usize);
        std::mem::transmute_copy(&*vtable.add(index))
    }
}

struct Clock(Arc<AtomicU64>);

fn hub_with(config: InputConfig) -> (Arc<Hub>, Clock) {
    let time = Arc::new(AtomicU64::new(1_000_000));
    let t = time.clone();
    let hub = Hub::with_clock(config, Box::new(move || t.load(Ordering::Relaxed)));
    (hub, Clock(time))
}

fn no_virtual() -> InputConfig {
    InputConfig {
        virtual_pad: VirtualPadMode::Off,
        ..InputConfig::default()
    }
}

impl Clock {
    fn advance_ms(&self, ms: u64) {
        self.0.fetch_add(ms * 1000, Ordering::Relaxed);
    }
}

fn pad(name: &str, seed: u8) -> PadIdentity {
    PadIdentity {
        uuid: [seed; 16],
        name: name.to_string(),
        vendor: 0x045E,
        product: 0x0B12,
    }
}

struct Gi(P);

impl Gi {
    fn new(hub: &Arc<Hub>) -> Gi {
        Gi(com::new_game_input(hub.clone()))
    }

    fn timestamp(&self) -> u64 {
        // SAFETY: slot 3 of IGameInput.
        unsafe { slot::<unsafe extern "win64" fn(P) -> u64>(self.0, 3)(self.0) }
    }

    fn reading_call(&self, index: usize, reference: P, kinds: u32, device: P) -> (i32, Reading) {
        let mut out: P = std::ptr::null_mut();
        // SAFETY: slots 4 to 6 of IGameInput have these shapes.
        let hr = unsafe {
            if index == 4 {
                slot::<unsafe extern "win64" fn(P, u32, P, *mut P) -> i32>(self.0, 4)(
                    self.0, kinds, device, &mut out,
                )
            } else {
                slot::<unsafe extern "win64" fn(P, P, u32, P, *mut P) -> i32>(self.0, index)(
                    self.0, reference, kinds, device, &mut out,
                )
            }
        };
        (hr, Reading(out))
    }

    fn current(&self, kinds: u32, device: P) -> (i32, Reading) {
        self.reading_call(4, std::ptr::null_mut(), kinds, device)
    }

    fn next(&self, reference: &Reading, kinds: u32, device: P) -> (i32, Reading) {
        self.reading_call(5, reference.0, kinds, device)
    }

    fn previous(&self, reference: &Reading, kinds: u32, device: P) -> (i32, Reading) {
        self.reading_call(6, reference.0, kinds, device)
    }

    fn stop(&self, token: u64) {
        // SAFETY: slot 12.
        unsafe { slot::<unsafe extern "win64" fn(P, u64)>(self.0, 12)(self.0, token) }
    }

    fn register_reading(&self, device: P, kinds: u32, context: P, f: super::hub::ReadingFn) -> u64 {
        let mut token = 0;
        // SAFETY: slot 8.
        let hr = unsafe {
            slot::<
                unsafe extern "win64" fn(P, P, u32, f32, P, super::hub::ReadingFn, *mut u64) -> i32,
            >(self.0, 8)(self.0, device, kinds, 0.0, context, f, &mut token)
        };
        assert_eq!(hr, S_OK);
        token
    }

    fn register_device(
        &self,
        device: P,
        kinds: u32,
        filter: u32,
        enumeration: u32,
        context: P,
        f: super::hub::DeviceFn,
    ) -> u64 {
        let mut token = 0;
        // SAFETY: slot 9.
        let hr = unsafe {
            slot::<
                unsafe extern "win64" fn(
                    P,
                    P,
                    u32,
                    u32,
                    u32,
                    P,
                    super::hub::DeviceFn,
                    *mut u64,
                ) -> i32,
            >(self.0, 9)(
                self.0,
                device,
                kinds,
                filter,
                enumeration,
                context,
                f,
                &mut token,
            )
        };
        assert_eq!(hr, S_OK);
        token
    }

    fn find_device(&self, id: &[u8; 32]) -> (i32, Device) {
        let mut out: P = std::ptr::null_mut();
        // SAFETY: slot 16.
        let hr = unsafe {
            slot::<unsafe extern "win64" fn(P, *const [u8; 32], *mut P) -> i32>(self.0, 16)(
                self.0, id, &mut out,
            )
        };
        (hr, Device(out))
    }
}

struct Reading(P);

impl Drop for Reading {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: a reading object we own one reference of.
            unsafe { com::release(self.0) };
        }
    }
}

impl Reading {
    fn gamepad(&self) -> Option<GamepadState> {
        let mut s = GamepadState::default();
        // SAFETY: slot 22.
        let ok = unsafe {
            slot::<unsafe extern "win64" fn(P, *mut GamepadState) -> bool>(self.0, 22)(
                self.0, &mut s,
            )
        };
        ok.then_some(s)
    }

    fn ui(&self) -> Option<u32> {
        let mut s = 0;
        // SAFETY: slot 24.
        let ok = unsafe {
            slot::<unsafe extern "win64" fn(P, *mut u32) -> bool>(self.0, 24)(self.0, &mut s)
        };
        ok.then_some(s)
    }

    fn sequence(&self, kinds: u32) -> u64 {
        // SAFETY: slot 4.
        unsafe { slot::<unsafe extern "win64" fn(P, u32) -> u64>(self.0, 4)(self.0, kinds) }
    }

    fn timestamp(&self) -> u64 {
        // SAFETY: slot 5.
        unsafe { slot::<unsafe extern "win64" fn(P) -> u64>(self.0, 5)(self.0) }
    }

    fn kind(&self) -> u32 {
        // SAFETY: slot 3.
        unsafe { slot::<unsafe extern "win64" fn(P) -> u32>(self.0, 3)(self.0) }
    }

    fn device(&self) -> Device {
        let mut out: P = std::ptr::null_mut();
        // SAFETY: slot 6.
        unsafe { slot::<unsafe extern "win64" fn(P, *mut P)>(self.0, 6)(self.0, &mut out) };
        Device(out)
    }

    fn axes(&self) -> Vec<f32> {
        let mut out = [0f32; 8];
        // SAFETY: slots 8 and 9.
        unsafe {
            let n = slot::<unsafe extern "win64" fn(P) -> u32>(self.0, 8)(self.0);
            let got = slot::<unsafe extern "win64" fn(P, u32, *mut f32) -> u32>(self.0, 9)(
                self.0,
                8,
                out.as_mut_ptr(),
            );
            assert_eq!(n, got);
            out[..got as usize].to_vec()
        }
    }

    fn buttons(&self) -> Vec<bool> {
        let mut out = [false; 16];
        // SAFETY: slots 10 and 11.
        unsafe {
            let n = slot::<unsafe extern "win64" fn(P) -> u32>(self.0, 10)(self.0);
            let got = slot::<unsafe extern "win64" fn(P, u32, *mut bool) -> u32>(self.0, 11)(
                self.0,
                16,
                out.as_mut_ptr(),
            );
            assert_eq!(n, got);
            out[..got as usize].to_vec()
        }
    }

    fn keys(&self) -> Vec<KeyState> {
        let mut out = [KeyState::default(); 8];
        // SAFETY: slots 14 and 15.
        unsafe {
            let n = slot::<unsafe extern "win64" fn(P) -> u32>(self.0, 14)(self.0);
            let got = slot::<unsafe extern "win64" fn(P, u32, *mut KeyState) -> u32>(self.0, 15)(
                self.0,
                8,
                out.as_mut_ptr(),
            );
            assert_eq!(n, got);
            out[..got as usize].to_vec()
        }
    }

    fn mouse(&self) -> Option<MouseState> {
        let mut s = MouseState::default();
        // SAFETY: slot 16.
        let ok = unsafe {
            slot::<unsafe extern "win64" fn(P, *mut MouseState) -> bool>(self.0, 16)(self.0, &mut s)
        };
        ok.then_some(s)
    }
}

struct Device(P);

impl Device {
    fn info(&self) -> &'static DeviceInfo {
        // SAFETY: slot 3 returns a pointer valid for the life of the process.
        unsafe { &*slot::<unsafe extern "win64" fn(P) -> *const DeviceInfo>(self.0, 3)(self.0) }
    }

    fn status(&self) -> u32 {
        // SAFETY: slot 4.
        unsafe { slot::<unsafe extern "win64" fn(P) -> u32>(self.0, 4)(self.0) }
    }

    fn rumble(&self, params: &RumbleParams) {
        // SAFETY: slot 10.
        unsafe {
            slot::<unsafe extern "win64" fn(P, *const RumbleParams)>(self.0, 10)(self.0, params)
        }
    }

    fn create_ffb(&self) -> i32 {
        let mut out: P = std::ptr::null_mut();
        // SAFETY: slot 6.
        unsafe {
            slot::<unsafe extern "win64" fn(P, u32, *const c_void, *mut P) -> i32>(self.0, 6)(
                self.0,
                0,
                std::ptr::null(),
                &mut out,
            )
        }
    }
}

fn state(buttons: u32, lx: f32) -> GamepadState {
    GamepadState {
        buttons,
        left_x: lx,
        ..GamepadState::default()
    }
}

#[test]
fn shim_table_serves_name_and_ordinal() {
    let shims = super::shims();
    let by_name = shims.iter().find(|s| s.name == "GameInputCreate").unwrap();
    let by_ordinal = shims.iter().find(|s| s.name == "#1").unwrap();
    assert_eq!(by_name.address, by_ordinal.address);
    assert!(by_name.dll.eq_ignore_ascii_case("gameinput.dll"));
}

#[test]
fn timestamp_counts_microseconds() {
    let (hub, clock) = hub_with(no_virtual());
    let gi = Gi::new(&hub);
    let a = gi.timestamp();
    clock.advance_ms(3);
    assert_eq!(gi.timestamp() - a, 3000);
}

#[test]
fn no_pad_means_no_gamepad_reading() {
    let (hub, _) = hub_with(no_virtual());
    let gi = Gi::new(&hub);
    let (hr, r) = gi.current(kind::GAMEPAD, std::ptr::null_mut());
    assert_eq!(hr, GAMEINPUT_E_READING_NOT_FOUND);
    assert!(r.0.is_null());
}

#[test]
fn gamepad_reading_carries_buttons_sticks_and_ui_state() {
    let (hub, _) = hub_with(no_virtual());
    let gi = Gi::new(&hub);
    hub.connect_pad(7, &pad("Xbox Wireless Controller", 1));
    hub.pad_state(
        7,
        GamepadState {
            buttons: buttons::A | buttons::DPAD_UP,
            left_trigger: 0.25,
            right_trigger: 1.0,
            left_x: -0.5,
            left_y: 0.75,
            right_x: 0.0,
            right_y: -1.0,
        },
    );
    let (hr, r) = gi.current(kind::GAMEPAD, std::ptr::null_mut());
    assert_eq!(hr, S_OK);
    let s = r.gamepad().unwrap();
    assert_eq!(s.buttons, buttons::A | buttons::DPAD_UP);
    assert_eq!((s.left_trigger, s.right_trigger), (0.25, 1.0));
    assert_eq!((s.left_x, s.left_y, s.right_y), (-0.5, 0.75, -1.0));
    let ui_state = r.ui().unwrap();
    assert_eq!(
        ui_state,
        ui::ACCEPT | ui::UP | ui::PAGE_DOWN | ui::SCROLL_DOWN
    );
    assert_eq!(r.axes(), vec![0.25, 0.875, 0.5, 0.0, 0.25, 1.0]);
    let b = r.buttons();
    assert_eq!(b.len(), buttons::COUNT);
    assert!(b[2] && b[6] && !b[0]);
    assert!(r.mouse().is_none());
    assert_eq!(r.kind() & kind::GAMEPAD, kind::GAMEPAD);
}

#[test]
fn reading_names_its_device_and_the_device_describes_itself() {
    let (hub, _) = hub_with(no_virtual());
    let gi = Gi::new(&hub);
    hub.connect_pad(7, &pad("Pad", 1));
    let (_, r) = gi.current(kind::GAMEPAD, std::ptr::null_mut());
    let device = r.device();
    let info = device.info();
    assert_eq!(info.device_family, FAMILY_XBOX_ONE);
    assert_eq!(info.vendor_id, 0x045E);
    assert_eq!(info.product_id, 0x0B12);
    assert_eq!(info.supported_input & kind::GAMEPAD, kind::GAMEPAD);
    assert_eq!(info.supported_rumble_motors, 0xF);
    assert_eq!(info.controller_axis_count, 6);
    assert_eq!(info.controller_button_count, 14);
    assert_eq!(info.info_size, 320);
    // SAFETY: the info block's own tables.
    unsafe {
        assert_eq!((*info.gamepad_info).labels[2], label::XBOX_A);
        assert_eq!(
            (*info.controller_axis_info.add(4)).label,
            label::XBOX_LEFT_TRIGGER
        );
        let name = &*info.display_name;
        let bytes = std::slice::from_raw_parts(name.data, name.size_in_bytes as usize);
        assert_eq!(bytes, b"Pad\0");
    }
    assert_eq!(device.status(), status::ONLINE);
    assert_eq!(device.create_ffb(), E_INVALIDARG);
    let same = r.device();
    assert_eq!(same.0, device.0);
}

#[test]
fn history_steps_through_a_tap_between_two_frames() {
    let (hub, clock) = hub_with(no_virtual());
    let gi = Gi::new(&hub);
    hub.connect_pad(1, &pad("Pad", 1));
    let none = std::ptr::null_mut();
    let (_, first) = gi.current(kind::GAMEPAD, none);
    clock.advance_ms(2);
    hub.pad_state(1, state(buttons::A, 0.0));
    clock.advance_ms(2);
    hub.pad_state(1, state(0, 0.0));
    let (hr, pressed) = gi.next(&first, kind::GAMEPAD, none);
    assert_eq!(hr, S_OK);
    assert_eq!(pressed.gamepad().unwrap().buttons, buttons::A);
    let (hr, released) = gi.next(&pressed, kind::GAMEPAD, none);
    assert_eq!(hr, S_OK);
    assert_eq!(released.gamepad().unwrap().buttons, 0);
    let (hr, out) = gi.next(&released, kind::GAMEPAD, none);
    assert_eq!(hr, GAMEINPUT_E_READING_NOT_FOUND);
    assert!(out.0.is_null());
    let (hr, back) = gi.previous(&released, kind::GAMEPAD, none);
    assert_eq!(hr, S_OK);
    assert_eq!(
        back.sequence(kind::GAMEPAD),
        pressed.sequence(kind::GAMEPAD)
    );
    assert!(pressed.timestamp() < released.timestamp());
    let (hr, _) = gi.previous(&first, kind::GAMEPAD, none);
    assert_eq!(hr, GAMEINPUT_E_READING_NOT_FOUND);
}

#[test]
fn identical_state_makes_no_new_reading() {
    let (hub, _) = hub_with(no_virtual());
    let gi = Gi::new(&hub);
    hub.connect_pad(1, &pad("Pad", 1));
    hub.pad_state(1, state(buttons::B, 0.0));
    let (_, a) = gi.current(kind::GAMEPAD, std::ptr::null_mut());
    hub.pad_state(1, state(buttons::B, 0.0));
    let (_, b) = gi.current(kind::GAMEPAD, std::ptr::null_mut());
    assert_eq!(a.sequence(kind::GAMEPAD), b.sequence(kind::GAMEPAD));
}

#[test]
fn readings_leave_the_history_after_half_a_second() {
    let (hub, clock) = hub_with(no_virtual());
    let gi = Gi::new(&hub);
    hub.connect_pad(1, &pad("Pad", 1));
    let none = std::ptr::null_mut();
    let (_, old) = gi.current(kind::GAMEPAD, none);
    for i in 0..4 {
        clock.advance_ms(300);
        hub.pad_state(1, state(buttons::A, i as f32 * 0.1));
    }
    let (hr, _) = gi.next(&old, kind::GAMEPAD, none);
    assert_eq!(hr, GAMEINPUT_E_REFERENCE_READING_TOO_OLD);
    let (hr, current) = gi.current(kind::GAMEPAD, none);
    assert_eq!(hr, S_OK);
    assert_eq!(current.gamepad().unwrap().left_x, 3.0f32 * 0.1);
    assert_eq!(old.gamepad().unwrap().buttons, 0);
}

#[test]
fn history_is_capped_at_512_readings() {
    let (hub, _) = hub_with(no_virtual());
    let gi = Gi::new(&hub);
    hub.connect_pad(1, &pad("Pad", 1));
    let none = std::ptr::null_mut();
    let (_, first) = gi.current(kind::GAMEPAD, none);
    for i in 0..600 {
        hub.pad_state(1, state(0, (i + 1) as f32 / 1000.0));
    }
    let (hr, _) = gi.next(&first, kind::GAMEPAD, none);
    assert_eq!(hr, GAMEINPUT_E_REFERENCE_READING_TOO_OLD);
}

#[test]
fn a_device_filter_limits_the_search_to_that_pad() {
    let (hub, _) = hub_with(no_virtual());
    let gi = Gi::new(&hub);
    let a = hub.connect_pad(1, &pad("A", 1));
    let b = hub.connect_pad(2, &pad("B", 2));
    let device_a = Device(hub.device_com(a).unwrap() as P);
    let none = std::ptr::null_mut();
    let (_, start) = gi.current(kind::GAMEPAD, device_a.0);
    let (_, newest_before) = gi.current(kind::GAMEPAD, none);
    hub.pad_state(2, state(buttons::X, 0.0));
    hub.pad_state(1, state(buttons::Y, 0.0));
    let (hr, next) = gi.next(&start, kind::GAMEPAD, device_a.0);
    assert_eq!(hr, S_OK);
    assert_eq!(next.gamepad().unwrap().buttons, buttons::Y);
    let (hr, any) = gi.next(&newest_before, kind::GAMEPAD, none);
    assert_eq!(hr, S_OK);
    assert_eq!(any.gamepad().unwrap().buttons, buttons::X);
    let (_, newest) = gi.current(kind::GAMEPAD, none);
    assert_eq!(newest.gamepad().unwrap().buttons, buttons::Y);
    let _ = b;
}

#[test]
fn an_unplugged_device_reports_disconnected_and_keeps_its_object() {
    let (hub, _) = hub_with(no_virtual());
    let gi = Gi::new(&hub);
    let d = hub.connect_pad(1, &pad("Pad", 1));
    let device = Device(hub.device_com(d).unwrap() as P);
    hub.disconnect_pad(1);
    assert_eq!(device.status(), status::NONE);
    let (hr, _) = gi.current(kind::GAMEPAD, device.0);
    assert_eq!(hr, GAMEINPUT_E_DEVICE_DISCONNECTED);
    let (hr, _) = gi.current(kind::GAMEPAD, std::ptr::null_mut());
    assert_eq!(hr, GAMEINPUT_E_READING_NOT_FOUND);
    let again = hub.connect_pad(1, &pad("Pad", 1));
    assert_eq!(again, d);
    assert_eq!(device.status(), status::ONLINE);
    let (hr, _) = gi.current(kind::GAMEPAD, device.0);
    assert_eq!(hr, S_OK);
}

#[test]
fn a_reading_from_before_an_unplug_leads_to_the_snapshot_after_replug() {
    let (hub, _) = hub_with(no_virtual());
    let gi = Gi::new(&hub);
    hub.connect_pad(1, &pad("Pad", 1));
    let none = std::ptr::null_mut();
    hub.pad_state(1, state(buttons::A, 0.0));
    let (_, before) = gi.current(kind::GAMEPAD, none);
    hub.pad_state(1, state(buttons::B, 0.0));
    let (_, last) = gi.current(kind::GAMEPAD, none);
    hub.disconnect_pad(1);
    hub.connect_pad(1, &pad("Pad", 1));
    let (hr, snapshot) = gi.next(&last, kind::GAMEPAD, none);
    assert_eq!(hr, S_OK);
    assert!(snapshot.sequence(kind::GAMEPAD) > last.sequence(kind::GAMEPAD));
    let (hr, _) = gi.next(&before, kind::GAMEPAD, none);
    assert_eq!(hr, GAMEINPUT_E_REFERENCE_READING_TOO_OLD);
}

#[test]
fn twins_get_distinct_ids_and_are_found_by_id() {
    let (hub, _) = hub_with(no_virtual());
    let gi = Gi::new(&hub);
    let a = hub.connect_pad(1, &pad("Pad", 5));
    let b = hub.connect_pad(2, &pad("Pad", 5));
    assert_ne!(a, b);
    let ids: Vec<_> = [a, b]
        .iter()
        .map(|&d| Device(hub.device_com(d).unwrap() as P).info().device_id)
        .collect();
    assert_ne!(ids[0], ids[1]);
    assert_eq!(ids[0][..16], ids[1][..16]);
    let (hr, found) = gi.find_device(&ids[1]);
    assert_eq!(hr, S_OK);
    assert_eq!(found.0 as usize, hub.device_com(b).unwrap());
    let (hr, missing) = gi.find_device(&[9; 32]);
    assert_eq!(hr, GAMEINPUT_E_DEVICE_NOT_FOUND);
    assert!(missing.0.is_null());
}

#[derive(Default)]
struct Log(Mutex<Vec<String>>);

impl Log {
    fn of(context: P) -> &'static Log {
        // SAFETY: the tests pass a leaked `Log`.
        unsafe { &*(context as *const Log) }
    }

    fn leak() -> (&'static Log, P) {
        let log: &'static Log = Box::leak(Box::default());
        (log, log as *const Log as P)
    }

    fn lines(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

unsafe extern "win64" fn on_reading(_token: u64, context: P, reading: P, _overrun: bool) {
    let wrapper = Reading(reading);
    let buttons = wrapper.gamepad().map(|s| s.buttons).unwrap_or(0);
    std::mem::forget(wrapper);
    Log::of(context)
        .0
        .lock()
        .unwrap()
        .push(format!("reading {buttons}"));
}

unsafe extern "win64" fn on_device(
    _token: u64,
    context: P,
    device: P,
    _ts: u64,
    current: u32,
    previous: u32,
) {
    let name_ok = !Device(device).info().display_name.is_null();
    Log::of(context)
        .0
        .lock()
        .unwrap()
        .push(format!("device {current:#x} {previous:#x} {name_ok}"));
}

unsafe extern "win64" fn on_guide(_token: u64, context: P, _device: P, _ts: u64, pressed: bool) {
    Log::of(context)
        .0
        .lock()
        .unwrap()
        .push(format!("guide {pressed}"));
}

#[test]
fn reading_callbacks_hear_changes_until_stopped() {
    let (hub, _) = hub_with(no_virtual());
    let gi = Gi::new(&hub);
    hub.connect_pad(1, &pad("Pad", 1));
    let (log, context) = Log::leak();
    let token = gi.register_reading(std::ptr::null_mut(), kind::GAMEPAD, context, on_reading);
    hub.pad_state(1, state(buttons::A, 0.0));
    hub.pad_state(1, state(buttons::A, 0.0));
    hub.pad_state(1, state(buttons::B, 0.0));
    assert_eq!(log.lines(), vec!["reading 4", "reading 8"]);
    gi.stop(token);
    hub.pad_state(1, state(0, 0.0));
    assert_eq!(log.lines().len(), 2);
}

#[test]
fn a_reading_callback_hears_only_devices_of_its_kind() {
    let (hub, _) = hub_with(no_virtual());
    let gi = Gi::new(&hub);
    hub.connect_pad(1, &pad("Pad", 1));
    let (log, context) = Log::leak();
    gi.register_reading(std::ptr::null_mut(), kind::KEYBOARD, context, on_reading);
    hub.pad_state(1, state(buttons::A, 0.0));
    assert!(log.lines().is_empty());
    hub.input(RawInput::Key {
        virtual_key: b'K',
        scan_code: 37,
        down: true,
    });
    assert_eq!(log.lines().len(), 1);
}

#[test]
fn blocking_enumeration_reports_present_devices_before_returning() {
    let (hub, _) = hub_with(no_virtual());
    let gi = Gi::new(&hub);
    hub.connect_pad(1, &pad("Pad", 1));
    let (log, context) = Log::leak();
    gi.register_device(
        std::ptr::null_mut(),
        kind::GAMEPAD,
        status::CONNECTED,
        ENUMERATE_BLOCKING,
        context,
        on_device,
    );
    assert_eq!(log.lines(), vec!["device 0x7 0x0 true"]);
}

#[test]
fn async_enumeration_waits_for_the_next_tick() {
    let (hub, _) = hub_with(no_virtual());
    let gi = Gi::new(&hub);
    hub.connect_pad(1, &pad("Pad", 1));
    let (log, context) = Log::leak();
    gi.register_device(
        std::ptr::null_mut(),
        kind::GAMEPAD,
        status::CONNECTED,
        ENUMERATE_ASYNC,
        context,
        on_device,
    );
    assert!(log.lines().is_empty());
    hub.tick();
    assert_eq!(log.lines().len(), 1);
}

#[test]
fn device_callbacks_hear_connect_and_disconnect() {
    let (hub, _) = hub_with(no_virtual());
    let gi = Gi::new(&hub);
    let (log, context) = Log::leak();
    gi.register_device(
        std::ptr::null_mut(),
        kind::GAMEPAD,
        status::CONNECTED,
        0,
        context,
        on_device,
    );
    hub.connect_pad(1, &pad("Pad", 1));
    hub.disconnect_pad(1);
    assert_eq!(
        log.lines(),
        vec!["device 0x7 0x0 true", "device 0x0 0x7 true"]
    );
}

#[test]
fn guide_button_reaches_guide_callbacks() {
    let (hub, _) = hub_with(no_virtual());
    let gi = Gi::new(&hub);
    hub.connect_pad(1, &pad("Pad", 1));
    let (log, context) = Log::leak();
    let mut token = 0u64;
    // SAFETY: slot 10 of IGameInput.
    let hr =
        unsafe {
            slot::<unsafe extern "win64" fn(P, P, P, super::hub::GuideFn, *mut u64) -> i32>(
                gi.0, 10,
            )(gi.0, std::ptr::null_mut(), context, on_guide, &mut token)
        };
    assert_eq!(hr, S_OK);
    hub.guide_button(1, true);
    hub.guide_button(1, false);
    assert_eq!(log.lines(), vec!["guide true", "guide false"]);
}

#[test]
fn a_dispatcher_holds_callbacks_until_dispatch() {
    let (hub, _) = hub_with(no_virtual());
    let gi = Gi::new(&hub);
    hub.connect_pad(1, &pad("Pad", 1));
    let (log, context) = Log::leak();
    gi.register_reading(std::ptr::null_mut(), kind::GAMEPAD, context, on_reading);
    let mut dispatcher: P = std::ptr::null_mut();
    // SAFETY: slot 14 of IGameInput, then slot 3 of IGameInputDispatcher.
    unsafe {
        let hr =
            slot::<unsafe extern "win64" fn(P, *mut P) -> i32>(gi.0, 14)(gi.0, &mut dispatcher);
        assert_eq!(hr, S_OK);
        for buttons in [buttons::A, buttons::B, buttons::X] {
            hub.pad_state(1, state(buttons, 0.0));
        }
        assert!(log.lines().is_empty());
        let dispatch = slot::<unsafe extern "win64" fn(P, u64) -> bool>(dispatcher, 3);
        assert!(dispatch(dispatcher, 0));
        assert_eq!(log.lines(), vec!["reading 4"]);
        assert!(!dispatch(dispatcher, u64::MAX));
        assert_eq!(log.lines(), vec!["reading 4", "reading 8", "reading 16"]);
        assert!(!dispatch(dispatcher, 0));
        com::release(dispatcher);
    }
}

#[test]
fn rumble_requests_are_clamped_scaled_and_switchable() {
    let (hub, _) = hub_with(no_virtual());
    let d = hub.connect_pad(1, &pad("Pad", 1));
    let device = Device(hub.device_com(d).unwrap() as P);
    device.rumble(&RumbleParams {
        low_frequency: 2.0,
        high_frequency: 0.5,
        left_trigger: -1.0,
        right_trigger: 0.25,
    });
    let out = hub.take_rumble();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].0, 1);
    assert_eq!(
        out[0].1,
        RumbleParams {
            low_frequency: 1.0,
            high_frequency: 0.5,
            left_trigger: 0.0,
            right_trigger: 0.25
        }
    );
    assert!(hub.take_rumble().is_empty());
    hub.set_config(InputConfig {
        rumble_scale: 0.5,
        ..no_virtual()
    });
    device.rumble(&RumbleParams {
        low_frequency: 1.0,
        ..RumbleParams::default()
    });
    assert_eq!(hub.take_rumble()[0].1.low_frequency, 0.5);
    hub.set_config(InputConfig {
        rumble: false,
        ..no_virtual()
    });
    device.rumble(&RumbleParams {
        low_frequency: 1.0,
        ..RumbleParams::default()
    });
    assert_eq!(hub.take_rumble()[0].1, RumbleParams::default());
    // SAFETY: a null parameter stops rumble.
    unsafe {
        slot::<unsafe extern "win64" fn(P, *const RumbleParams)>(device.0, 10)(
            device.0,
            std::ptr::null(),
        )
    };
}

#[test]
fn keyboard_readings_list_pressed_keys_in_press_order() {
    let (hub, _) = hub_with(no_virtual());
    let gi = Gi::new(&hub);
    for (vk, scan) in [(b'W', 17), (b'A', 30)] {
        hub.input(RawInput::Key {
            virtual_key: vk,
            scan_code: scan,
            down: true,
        });
    }
    let (hr, r) = gi.current(kind::KEYBOARD, std::ptr::null_mut());
    assert_eq!(hr, S_OK);
    let keys = r.keys();
    assert_eq!(
        keys.iter().map(|k| k.virtual_key).collect::<Vec<_>>(),
        vec![b'W', b'A']
    );
    assert_eq!(keys[0].scan_code, 17);
    assert_eq!(keys[0].code_point, u32::from(b'w'));
    hub.input(RawInput::Key {
        virtual_key: b'W',
        scan_code: 17,
        down: false,
    });
    let (_, r) = gi.current(kind::KEYBOARD, std::ptr::null_mut());
    assert_eq!(r.keys().len(), 1);
    assert!(r.gamepad().is_none());
}

#[test]
fn mouse_readings_accumulate_movement_buttons_and_wheel() {
    let (hub, _) = hub_with(no_virtual());
    let gi = Gi::new(&hub);
    hub.input(RawInput::MouseMove { dx: 5, dy: -2 });
    hub.input(RawInput::MouseMove { dx: 3, dy: 4 });
    hub.input(RawInput::MouseButton {
        button: MouseButton::Left,
        down: true,
    });
    hub.input(RawInput::MouseWheel { dx: 0, dy: 120 });
    let (hr, r) = gi.current(kind::MOUSE, std::ptr::null_mut());
    assert_eq!(hr, S_OK);
    let m = r.mouse().unwrap();
    assert_eq!((m.position_x, m.position_y, m.wheel_y), (8, 2, 120));
    assert_eq!(m.buttons, mouse_buttons::LEFT);
    assert!(r.gamepad().is_none());
    assert_eq!(Device(r.device().0).info().device_family, FAMILY_HID);
}

#[test]
fn input_while_the_window_is_unfocused_is_ignored_and_focus_loss_releases_everything() {
    let (hub, clock) = hub_with(InputConfig {
        virtual_pad: VirtualPadMode::Always,
        ..InputConfig::default()
    });
    let settle = || {
        for _ in 0..30 {
            clock.advance_ms(10);
            hub.tick();
        }
    };
    let gi = Gi::new(&hub);
    let key = |vk, down| RawInput::Key {
        virtual_key: vk,
        scan_code: 0,
        down,
    };
    hub.input(key(vk::W, true));
    hub.input(RawInput::MouseButton {
        button: MouseButton::Left,
        down: true,
    });
    settle();
    let (_, r) = gi.current(kind::GAMEPAD, std::ptr::null_mut());
    let s = r.gamepad().unwrap();
    assert_eq!((s.left_y, s.right_trigger), (1.0, 1.0));
    hub.input(RawInput::Focus(false));
    let (_, r) = gi.current(kind::GAMEPAD, std::ptr::null_mut());
    assert_eq!(r.gamepad().unwrap(), GamepadState::default());
    hub.input(key(vk::W, true));
    let (_, r) = gi.current(kind::GAMEPAD, std::ptr::null_mut());
    assert_eq!(r.gamepad().unwrap(), GamepadState::default());
    hub.input(RawInput::Focus(true));
    hub.input(key(vk::W, true));
    settle();
    let (_, r) = gi.current(kind::GAMEPAD, std::ptr::null_mut());
    assert_eq!(r.gamepad().unwrap().left_y, 1.0);
}

#[test]
fn the_keyboard_pad_is_present_only_while_no_gamepad_is_in_auto_mode() {
    let (hub, _) = hub_with(InputConfig {
        virtual_pad: VirtualPadMode::Auto,
        ..InputConfig::default()
    });
    let gi = Gi::new(&hub);
    let (log, context) = Log::leak();
    let none = std::ptr::null_mut();
    let (hr, r) = gi.current(kind::GAMEPAD, none);
    assert_eq!(hr, S_OK);
    assert_eq!(r.device().status(), status::ONLINE);
    gi.register_device(
        none,
        kind::GAMEPAD,
        status::CONNECTED,
        0,
        context,
        on_device,
    );
    hub.connect_pad(1, &pad("Pad", 1));
    assert_eq!(r.device().status(), status::NONE);
    assert_eq!(log.lines().len(), 2);
    hub.disconnect_pad(1);
    assert_eq!(r.device().status(), status::ONLINE);
}

#[test]
fn always_mode_keeps_the_keyboard_pad_beside_a_real_pad_and_newest_input_wins() {
    let (hub, _) = hub_with(InputConfig {
        virtual_pad: VirtualPadMode::Always,
        ..InputConfig::default()
    });
    let gi = Gi::new(&hub);
    hub.connect_pad(1, &pad("Pad", 1));
    hub.pad_state(1, state(buttons::Y, 0.0));
    let (_, r) = gi.current(kind::GAMEPAD, std::ptr::null_mut());
    assert_eq!(r.gamepad().unwrap().buttons, buttons::Y);
    hub.input(RawInput::Key {
        virtual_key: vk::RETURN,
        scan_code: 28,
        down: true,
    });
    let (_, r) = gi.current(kind::GAMEPAD, std::ptr::null_mut());
    assert_eq!(r.gamepad().unwrap().buttons, buttons::A);
}

#[test]
fn mouse_movement_pushes_the_right_stick_past_the_dead_zone_and_it_returns_to_zero() {
    let (hub, clock) = hub_with(InputConfig::default());
    let gi = Gi::new(&hub);
    let none = std::ptr::null_mut();
    for _ in 0..40 {
        clock.advance_ms(1);
        hub.input(RawInput::MouseMove { dx: 1, dy: 0 });
        hub.tick();
    }
    let (_, r) = gi.current(kind::GAMEPAD, none);
    let moving = r.gamepad().unwrap().right_x;
    assert!(moving > 0.25 && moving < 1.0, "{moving}");
    for _ in 0..40 {
        clock.advance_ms(1);
        hub.tick();
    }
    let (_, r) = gi.current(kind::GAMEPAD, none);
    assert_eq!(r.gamepad().unwrap().right_x, 0.0);
    clock.advance_ms(1);
    hub.input(RawInput::MouseMove { dx: 0, dy: 3 });
    hub.tick();
    let (_, r) = gi.current(kind::GAMEPAD, none);
    assert!(r.gamepad().unwrap().right_y < 0.0);
}

fn merged() -> (Arc<Hub>, Clock) {
    hub_with(InputConfig::default())
}

#[test]
fn merged_mode_shows_one_pad_that_has_both_devices_in_it() {
    let (hub, clock) = merged();
    let gi = Gi::new(&hub);
    let none = std::ptr::null_mut();
    let (log, context) = Log::leak();
    gi.register_device(
        none,
        kind::GAMEPAD,
        status::CONNECTED,
        2,
        context,
        on_device,
    );
    assert_eq!(log.lines().len(), 1);
    hub.connect_pad(1, &pad("Pad", 1));
    assert_eq!(
        log.lines().len(),
        1,
        "the controller is not shown to the title"
    );
    hub.pad_state(1, state(buttons::Y, 0.0));
    let (_, r) = gi.current(kind::GAMEPAD, none);
    assert_eq!(r.gamepad().unwrap().buttons, buttons::Y);
    hub.input(RawInput::Key {
        virtual_key: vk::RETURN,
        scan_code: 28,
        down: true,
    });
    clock.advance_ms(5);
    hub.tick();
    let (_, r) = gi.current(kind::GAMEPAD, none);
    assert_eq!(r.gamepad().unwrap().buttons, buttons::A | buttons::Y);
    let physical = hub.find_device(&[0; 32]);
    assert!(physical.is_none());
}

#[test]
fn the_last_used_device_wins_a_stick_both_push_and_is_reported() {
    let (hub, clock) = merged();
    let gi = Gi::new(&hub);
    let none = std::ptr::null_mut();
    hub.connect_pad(1, &pad("Pad", 1));
    hub.pad_state(1, state(0, -1.0));
    assert_eq!(hub.last_used(), super::mapping::LastUsed::Pad);
    hub.input(RawInput::Key {
        virtual_key: vk::D,
        scan_code: 32,
        down: true,
    });
    for _ in 0..20 {
        clock.advance_ms(10);
        hub.tick();
    }
    assert_eq!(hub.last_used(), super::mapping::LastUsed::KeyboardMouse);
    let (_, r) = gi.current(kind::GAMEPAD, none);
    assert_eq!(r.gamepad().unwrap().left_x, 1.0);
    hub.pad_state(1, state(0, -0.9));
    assert_eq!(hub.last_used(), super::mapping::LastUsed::Pad);
    let (_, r) = gi.current(kind::GAMEPAD, none);
    assert_eq!(r.gamepad().unwrap().left_x, -0.9);
}

#[test]
fn rumble_for_the_merged_pad_goes_to_the_connected_controllers() {
    let (hub, _) = merged();
    hub.connect_pad(7, &pad("Pad", 1));
    hub.set_rumble(
        super::hub::VIRTUAL_PAD,
        RumbleParams {
            low_frequency: 0.5,
            ..RumbleParams::default()
        },
    );
    let out = hub.take_rumble();
    assert_eq!(out.len(), 1);
    assert_eq!((out[0].0, out[0].1.low_frequency), (7, 0.5));
    assert!(hub.take_rumble().is_empty());
}

#[test]
fn wheel_notches_press_the_bound_button_once_each() {
    let (hub, clock) = merged();
    let gi = Gi::new(&hub);
    let none = std::ptr::null_mut();
    let wheel = hub.config().bindings.len();
    assert!(wheel > 0);
    let mut config = hub.config();
    config.bindings.push(super::config::Binding {
        source: super::config::Source::Wheel(super::config::WheelDir::Down),
        control: super::config::Control::Button(buttons::DPAD_RIGHT),
    });
    hub.set_config(config);
    hub.input(RawInput::MouseWheel { dx: 0, dy: -240 });
    let mut presses = 0;
    let mut was = false;
    for _ in 0..300 {
        clock.advance_ms(1);
        hub.tick();
        let (_, r) = gi.current(kind::GAMEPAD, none);
        let down = r.gamepad().unwrap().buttons & buttons::DPAD_RIGHT != 0;
        if down && !was {
            presses += 1;
        }
        was = down;
    }
    assert_eq!(presses, 2);
}
