//! The COM objects the title sees: `IGameInput`, `IGameInputDevice`, `IGameInputReading` and
//! `IGameInputDispatcher`, as v0 vtables of `extern "win64"` methods over the hub.
//!
//! The slot order follows the public v0 header. Slots the title could call but nothing here provides answer
//! `E_NOTIMPL` (or false, or zero) and leave a trace note naming the method.

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use super::abi::{
    self, BatteryState, E_INVALIDARG, E_NOTIMPL, E_POINTER, GAMEINPUT_E_DEVICE_NOT_FOUND,
    GamepadState, KeyState, MouseState, RumbleParams, S_OK, kind,
};
use super::hub::{DeviceFn, GuideFn, Hub, Reading, ReadingFn};
use super::mapping;
use crate::trace::note;

type Hresult = i32;
type This = *mut c_void;

#[repr(C)]
pub struct Guid {
    d1: u32,
    d2: u16,
    d3: u16,
    d4: [u8; 8],
}

const IID_IUNKNOWN: Guid = Guid {
    d1: 0,
    d2: 0,
    d3: 0,
    d4: [0xC0, 0, 0, 0, 0, 0, 0, 0x46],
};
const IID_IAGILE_OBJECT: Guid = Guid {
    d1: 0x94EA2B94,
    d2: 0xE9CC,
    d3: 0x49E0,
    d4: [0xC0, 0xFF, 0xEE, 0x64, 0xCA, 0x8F, 0x5B, 0x90],
};

fn same(a: &Guid, b: &Guid) -> bool {
    a.d1 == b.d1 && a.d2 == b.d2 && a.d3 == b.d3 && a.d4 == b.d4
}

#[repr(C)]
struct Unknown {
    query_interface: unsafe extern "win64" fn(This, *const Guid, *mut *mut c_void) -> Hresult,
    add_ref: unsafe extern "win64" fn(This) -> u32,
    release: unsafe extern "win64" fn(This) -> u32,
}

#[repr(C)]
struct Obj<V: 'static, T> {
    vtable: &'static V,
    refs: AtomicU32,
    data: T,
}

/// # Safety
/// `this` must be a pointer to an `Obj<V, T>` made here and not yet freed.
unsafe fn obj<'a, V, T>(this: This) -> &'a Obj<V, T> {
    // SAFETY: the caller's contract.
    unsafe { &*(this as *const Obj<V, T>) }
}

fn new_obj<V: 'static, T>(vtable: &'static V, data: T) -> *mut c_void {
    Box::into_raw(Box::new(Obj {
        vtable,
        refs: AtomicU32::new(1),
        data,
    })) as *mut c_void
}

unsafe extern "win64" fn query_interface<V: 'static, T>(
    this: This,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> Hresult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: `out` is the caller's out pointer; `iid` is read only when non-null.
    unsafe {
        *out = std::ptr::null_mut();
        if iid.is_null() {
            return E_INVALIDARG;
        }
        let iid = &*iid;
        if !same(iid, &IID_IUNKNOWN) && !same(iid, &IID_IAGILE_OBJECT) {
            note(format!(
                "GameInput QueryInterface for {:08X}-{:04X}-{:04X}: answered with the object itself",
                iid.d1, iid.d2, iid.d3
            ));
        }
        add_ref::<V, T>(this);
        *out = this;
    }
    S_OK
}

unsafe extern "win64" fn add_ref<V: 'static, T>(this: This) -> u32 {
    // SAFETY: `this` is one of our objects.
    unsafe { obj::<V, T>(this) }
        .refs
        .fetch_add(1, Ordering::AcqRel)
        + 1
}

unsafe extern "win64" fn release_owned<V: 'static, T>(this: This) -> u32 {
    // SAFETY: `this` is one of our objects.
    let left = unsafe { obj::<V, T>(this) }
        .refs
        .fetch_sub(1, Ordering::AcqRel)
        - 1;
    if left == 0 {
        // SAFETY: the last reference is gone, and the object came from `Box::into_raw`.
        drop(unsafe { Box::from_raw(this as *mut Obj<V, T>) });
    }
    left
}

/// Devices live as long as the process, so a release never frees them.
unsafe extern "win64" fn release_device(this: This) -> u32 {
    // SAFETY: `this` is a device object.
    let object = unsafe { obj::<DeviceVt, DeviceData>(this) };
    let _ = object
        .refs
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
            Some(n.saturating_sub(1).max(1))
        });
    object.refs.load(Ordering::Acquire)
}

/// Calls `Release` on any of our objects.
///
/// # Safety
/// `object` is a live COM object with an `IUnknown` vtable.
pub unsafe fn release(object: *mut c_void) {
    // SAFETY: every object starts with a vtable pointer whose first three slots are `IUnknown`.
    unsafe {
        let vtable = *(object as *const *const Unknown);
        ((*vtable).release)(object);
    }
}

// ---- events ----------------------------------------------------------------------------------------------

#[cfg(windows)]
mod events {
    use windows_sys::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE};
    use windows_sys::Win32::System::Threading::{
        CreateEventW, GetCurrentProcess, ResetEvent, SetEvent,
    };

    pub fn new() -> usize {
        // SAFETY: an unnamed manual-reset event, initially unsignalled.
        unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) as usize }
    }
    pub fn set(event: usize) {
        // SAFETY: `event` is a handle this module created.
        unsafe { SetEvent(event as HANDLE) };
    }
    pub fn reset(event: usize) {
        // SAFETY: as above.
        unsafe { ResetEvent(event as HANDLE) };
    }
    pub fn duplicate(event: usize) -> usize {
        let mut out: HANDLE = std::ptr::null_mut();
        // SAFETY: duplicates one of our handles inside this process.
        unsafe {
            let process = GetCurrentProcess();
            DuplicateHandle(
                process,
                event as HANDLE,
                process,
                &mut out,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            );
        }
        out as usize
    }
}

#[cfg(not(windows))]
mod events {
    pub fn new() -> usize {
        0
    }
    pub fn set(_: usize) {}
    pub fn reset(_: usize) {}
    pub fn duplicate(event: usize) -> usize {
        event
    }
}

pub fn event_new() -> usize {
    events::new()
}
pub fn event_set(event: usize) {
    events::set(event);
}
pub fn event_reset(event: usize) {
    events::reset(event);
}

// ---- IGameInput ------------------------------------------------------------------------------------------

struct GameInputData {
    hub: Arc<Hub>,
}

#[repr(C)]
struct GameInputVt {
    unknown: Unknown,
    get_current_timestamp: unsafe extern "win64" fn(This) -> u64,
    get_current_reading: unsafe extern "win64" fn(This, u32, This, *mut *mut c_void) -> Hresult,
    get_next_reading: unsafe extern "win64" fn(This, This, u32, This, *mut *mut c_void) -> Hresult,
    get_previous_reading:
        unsafe extern "win64" fn(This, This, u32, This, *mut *mut c_void) -> Hresult,
    get_temporal_reading: unsafe extern "win64" fn(This, u64, This, *mut *mut c_void) -> Hresult,
    register_reading_callback: unsafe extern "win64" fn(
        This,
        This,
        u32,
        f32,
        *mut c_void,
        Option<ReadingFn>,
        *mut u64,
    ) -> Hresult,
    register_device_callback: unsafe extern "win64" fn(
        This,
        This,
        u32,
        u32,
        u32,
        *mut c_void,
        Option<DeviceFn>,
        *mut u64,
    ) -> Hresult,
    register_guide_button_callback:
        unsafe extern "win64" fn(This, This, *mut c_void, Option<GuideFn>, *mut u64) -> Hresult,
    register_keyboard_layout_callback:
        unsafe extern "win64" fn(This, This, *mut c_void, *const c_void, *mut u64) -> Hresult,
    stop_callback: unsafe extern "win64" fn(This, u64),
    unregister_callback: unsafe extern "win64" fn(This, u64, u64) -> bool,
    create_dispatcher: unsafe extern "win64" fn(This, *mut *mut c_void) -> Hresult,
    create_aggregate_device: unsafe extern "win64" fn(This, u32, *mut *mut c_void) -> Hresult,
    find_device_from_id:
        unsafe extern "win64" fn(This, *const [u8; 32], *mut *mut c_void) -> Hresult,
    find_device_from_object: unsafe extern "win64" fn(This, This, *mut *mut c_void) -> Hresult,
    find_device_from_platform_handle:
        unsafe extern "win64" fn(This, *mut c_void, *mut *mut c_void) -> Hresult,
    find_device_from_platform_string:
        unsafe extern "win64" fn(This, *const u16, *mut *mut c_void) -> Hresult,
    enable_oem_device_support: unsafe extern "win64" fn(This, u16, u16, u8, u8) -> Hresult,
    set_focus_policy: unsafe extern "win64" fn(This, u32),
}

static GAME_INPUT_VT: GameInputVt = GameInputVt {
    unknown: Unknown {
        query_interface: query_interface::<GameInputVt, GameInputData>,
        add_ref: add_ref::<GameInputVt, GameInputData>,
        release: release_owned::<GameInputVt, GameInputData>,
    },
    get_current_timestamp: gi_timestamp,
    get_current_reading: gi_current_reading,
    get_next_reading: gi_next_reading,
    get_previous_reading: gi_previous_reading,
    get_temporal_reading: gi_temporal_reading,
    register_reading_callback: gi_register_reading,
    register_device_callback: gi_register_device,
    register_guide_button_callback: gi_register_guide,
    register_keyboard_layout_callback: gi_register_layout,
    stop_callback: gi_stop_callback,
    unregister_callback: gi_unregister_callback,
    create_dispatcher: gi_create_dispatcher,
    create_aggregate_device: gi_create_aggregate,
    find_device_from_id: gi_find_from_id,
    find_device_from_object: gi_find_from_object,
    find_device_from_platform_handle: gi_find_from_handle,
    find_device_from_platform_string: gi_find_from_string,
    enable_oem_device_support: gi_enable_oem,
    set_focus_policy: gi_set_focus_policy,
};

/// A new `IGameInput` over `hub`, with one reference for the caller.
pub fn new_game_input(hub: Arc<Hub>) -> *mut c_void {
    new_obj(&GAME_INPUT_VT, GameInputData { hub })
}

/// # Safety
/// `this` is an `IGameInput` made by `new_game_input`.
unsafe fn hub_of<'a>(this: This) -> &'a Arc<Hub> {
    // SAFETY: the caller's contract.
    &unsafe { obj::<GameInputVt, GameInputData>(this) }.data.hub
}

/// The device a `IGameInputDevice*` argument stands for: none for null, an error for a pointer that is not ours.
fn device_arg(hub: &Hub, device: This) -> Result<Option<usize>, Hresult> {
    if device.is_null() {
        return Ok(None);
    }
    hub.device_of_com(device as usize)
        .map(Some)
        .ok_or(E_INVALIDARG)
}

/// # Safety
/// `reading` is null or an object made by `new_reading_object`.
unsafe fn reading_arg<'a>(reading: This) -> Option<&'a Arc<Reading>> {
    if reading.is_null() {
        return None;
    }
    // SAFETY: the caller's contract.
    let object = unsafe { obj::<ReadingVt, ReadingData>(reading) };
    std::ptr::eq(object.vtable, &READING_VT).then_some(&object.data.reading)
}

unsafe extern "win64" fn gi_timestamp(this: This) -> u64 {
    // SAFETY: `this` is our IGameInput.
    unsafe { hub_of(this) }.now()
}

fn answer(hub: &Arc<Hub>, result: Result<Arc<Reading>, Hresult>, out: *mut *mut c_void) -> Hresult {
    match result {
        Ok(reading) => {
            // SAFETY: `out` was checked non-null by the caller.
            unsafe { *out = new_reading_object(hub.clone(), reading) };
            S_OK
        }
        Err(code) => code,
    }
}

unsafe extern "win64" fn gi_current_reading(
    this: This,
    kinds: u32,
    device: This,
    out: *mut *mut c_void,
) -> Hresult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: `this` is our IGameInput and `out` is non-null.
    unsafe {
        *out = std::ptr::null_mut();
        let hub = hub_of(this);
        match device_arg(hub, device) {
            Ok(d) => answer(hub, hub.current_reading(kinds, d), out),
            Err(code) => code,
        }
    }
}

unsafe fn step(
    this: This,
    reference: This,
    kinds: u32,
    device: This,
    out: *mut *mut c_void,
    forward: bool,
) -> Hresult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: as in `gi_current_reading`; `reference` is checked by `reading_arg`.
    unsafe {
        *out = std::ptr::null_mut();
        let hub = hub_of(this);
        let Some(reference) = reading_arg(reference) else {
            return E_INVALIDARG;
        };
        let d = match device_arg(hub, device) {
            Ok(d) => d,
            Err(code) => return code,
        };
        let result = if forward {
            hub.next_reading(reference, kinds, d)
        } else {
            hub.previous_reading(reference, kinds, d)
        };
        answer(hub, result, out)
    }
}

unsafe extern "win64" fn gi_next_reading(
    this: This,
    reference: This,
    kinds: u32,
    device: This,
    out: *mut *mut c_void,
) -> Hresult {
    // SAFETY: forwards the caller's arguments.
    unsafe { step(this, reference, kinds, device, out, true) }
}

unsafe extern "win64" fn gi_previous_reading(
    this: This,
    reference: This,
    kinds: u32,
    device: This,
    out: *mut *mut c_void,
) -> Hresult {
    // SAFETY: forwards the caller's arguments.
    unsafe { step(this, reference, kinds, device, out, false) }
}

/// Never implemented in v0 either.
unsafe extern "win64" fn gi_temporal_reading(
    _this: This,
    _timestamp: u64,
    _device: This,
    out: *mut *mut c_void,
) -> Hresult {
    if !out.is_null() {
        // SAFETY: `out` is the caller's out pointer.
        unsafe { *out = std::ptr::null_mut() };
    }
    E_NOTIMPL
}

unsafe extern "win64" fn gi_register_reading(
    this: This,
    device: This,
    kinds: u32,
    _analog_threshold: f32,
    context: *mut c_void,
    func: Option<ReadingFn>,
    token: *mut u64,
) -> Hresult {
    // SAFETY: `this` is our IGameInput; `token` is checked before the write.
    unsafe {
        let hub = hub_of(this);
        let (Some(func), false) = (func, token.is_null()) else {
            return E_INVALIDARG;
        };
        match device_arg(hub, device) {
            Ok(d) => {
                *token = hub.register_reading_callback(d, kinds, context as usize, func);
                S_OK
            }
            Err(code) => code,
        }
    }
}

unsafe extern "win64" fn gi_register_device(
    this: This,
    device: This,
    kinds: u32,
    status_filter: u32,
    enumeration: u32,
    context: *mut c_void,
    func: Option<DeviceFn>,
    token: *mut u64,
) -> Hresult {
    // SAFETY: as in `gi_register_reading`.
    unsafe {
        let hub = hub_of(this);
        let (Some(func), false) = (func, token.is_null()) else {
            return E_INVALIDARG;
        };
        match device_arg(hub, device) {
            Ok(d) => {
                *token = hub.register_device_callback(
                    d,
                    kinds,
                    status_filter,
                    enumeration,
                    context as usize,
                    func,
                );
                S_OK
            }
            Err(code) => code,
        }
    }
}

unsafe extern "win64" fn gi_register_guide(
    this: This,
    device: This,
    context: *mut c_void,
    func: Option<GuideFn>,
    token: *mut u64,
) -> Hresult {
    // SAFETY: as in `gi_register_reading`.
    unsafe {
        let hub = hub_of(this);
        let (Some(func), false) = (func, token.is_null()) else {
            return E_INVALIDARG;
        };
        match device_arg(hub, device) {
            Ok(d) => {
                *token = hub.register_guide_callback(d, context as usize, func);
                S_OK
            }
            Err(code) => code,
        }
    }
}

unsafe extern "win64" fn gi_register_layout(
    this: This,
    device: This,
    context: *mut c_void,
    _func: *const c_void,
    token: *mut u64,
) -> Hresult {
    // SAFETY: as in `gi_register_reading`.
    unsafe {
        let hub = hub_of(this);
        if token.is_null() {
            return E_INVALIDARG;
        }
        match device_arg(hub, device) {
            Ok(d) => {
                *token = hub.register_keyboard_layout_callback(d, context as usize);
                S_OK
            }
            Err(code) => code,
        }
    }
}

unsafe extern "win64" fn gi_stop_callback(this: This, token: u64) {
    // SAFETY: `this` is our IGameInput.
    unsafe { hub_of(this) }.remove_callback(token);
}

unsafe extern "win64" fn gi_unregister_callback(this: This, token: u64, _timeout: u64) -> bool {
    // SAFETY: `this` is our IGameInput.
    unsafe { hub_of(this) }.remove_callback(token)
}

unsafe extern "win64" fn gi_create_dispatcher(this: This, out: *mut *mut c_void) -> Hresult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: `this` is our IGameInput and `out` is non-null.
    unsafe {
        let hub = hub_of(this).clone();
        hub.create_dispatcher();
        *out = new_obj(&DISPATCHER_VT, DispatcherData { hub });
    }
    S_OK
}

unsafe extern "win64" fn gi_create_aggregate(
    _this: This,
    _kind: u32,
    out: *mut *mut c_void,
) -> Hresult {
    note("IGameInput::CreateAggregateDevice is not implemented".to_string());
    if !out.is_null() {
        // SAFETY: `out` is the caller's out pointer.
        unsafe { *out = std::ptr::null_mut() };
    }
    E_NOTIMPL
}

unsafe extern "win64" fn gi_find_from_id(
    this: This,
    id: *const [u8; 32],
    out: *mut *mut c_void,
) -> Hresult {
    if id.is_null() || out.is_null() {
        return E_POINTER;
    }
    // SAFETY: both pointers are non-null; `this` is our IGameInput.
    unsafe {
        *out = std::ptr::null_mut();
        let hub = hub_of(this);
        match hub.find_device(&*id).and_then(|d| hub.device_com(d)) {
            Some(device) => {
                add_ref::<DeviceVt, DeviceData>(device as This);
                *out = device as *mut c_void;
                S_OK
            }
            None => GAMEINPUT_E_DEVICE_NOT_FOUND,
        }
    }
}

unsafe extern "win64" fn gi_find_from_object(
    this: This,
    device: This,
    out: *mut *mut c_void,
) -> Hresult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: `out` is non-null; `this` is our IGameInput.
    unsafe {
        *out = std::ptr::null_mut();
        match device_arg(hub_of(this), device) {
            Ok(Some(_)) => {
                add_ref::<DeviceVt, DeviceData>(device);
                *out = device;
                S_OK
            }
            Ok(None) => E_INVALIDARG,
            Err(code) => code,
        }
    }
}

unsafe extern "win64" fn gi_find_from_handle(
    _this: This,
    _handle: *mut c_void,
    out: *mut *mut c_void,
) -> Hresult {
    note("IGameInput::FindDeviceFromPlatformHandle is not implemented".to_string());
    if !out.is_null() {
        // SAFETY: `out` is the caller's out pointer.
        unsafe { *out = std::ptr::null_mut() };
    }
    E_NOTIMPL
}

unsafe extern "win64" fn gi_find_from_string(
    _this: This,
    _name: *const u16,
    out: *mut *mut c_void,
) -> Hresult {
    note("IGameInput::FindDeviceFromPlatformString is not implemented".to_string());
    if !out.is_null() {
        // SAFETY: `out` is the caller's out pointer.
        unsafe { *out = std::ptr::null_mut() };
    }
    E_NOTIMPL
}

unsafe extern "win64" fn gi_enable_oem(_: This, _: u16, _: u16, _: u8, _: u8) -> Hresult {
    S_OK
}

unsafe extern "win64" fn gi_set_focus_policy(_: This, _: u32) {}

// ---- IGameInputDevice ------------------------------------------------------------------------------------

struct DeviceData {
    hub: Arc<Hub>,
    index: usize,
}

#[repr(C)]
struct DeviceVt {
    unknown: Unknown,
    get_device_info: unsafe extern "win64" fn(This) -> *const abi::DeviceInfo,
    get_device_status: unsafe extern "win64" fn(This) -> u32,
    get_battery_state: unsafe extern "win64" fn(This, *mut BatteryState),
    create_force_feedback_effect:
        unsafe extern "win64" fn(This, u32, *const c_void, *mut *mut c_void) -> Hresult,
    is_force_feedback_motor_powered_on: unsafe extern "win64" fn(This, u32) -> bool,
    set_force_feedback_motor_gain: unsafe extern "win64" fn(This, u32, f32),
    set_haptic_motor_state: unsafe extern "win64" fn(This, u32, *const c_void) -> Hresult,
    set_rumble_state: unsafe extern "win64" fn(This, *const RumbleParams),
    set_input_synchronization_state: unsafe extern "win64" fn(This, bool),
    send_input_synchronization_hint: unsafe extern "win64" fn(This),
    power_off: unsafe extern "win64" fn(This),
    create_raw_device_report: unsafe extern "win64" fn(This, u32, u32, *mut *mut c_void) -> Hresult,
    get_raw_device_feature: unsafe extern "win64" fn(This, u32, *mut *mut c_void) -> Hresult,
    set_raw_device_feature: unsafe extern "win64" fn(This, *mut c_void) -> Hresult,
    send_raw_device_output: unsafe extern "win64" fn(This, *mut c_void) -> Hresult,
    send_raw_device_output_with_response:
        unsafe extern "win64" fn(This, *mut c_void, *mut *mut c_void) -> Hresult,
    execute_raw_device_io_control: unsafe extern "win64" fn(
        This,
        u32,
        usize,
        *const c_void,
        usize,
        *mut c_void,
        *mut usize,
    ) -> Hresult,
    acquire_exclusive_raw_device_access: unsafe extern "win64" fn(This, u64) -> bool,
    release_exclusive_raw_device_access: unsafe extern "win64" fn(This),
}

static DEVICE_VT: DeviceVt = DeviceVt {
    unknown: Unknown {
        query_interface: query_interface::<DeviceVt, DeviceData>,
        add_ref: add_ref::<DeviceVt, DeviceData>,
        release: release_device,
    },
    get_device_info: dev_info,
    get_device_status: dev_status,
    get_battery_state: dev_battery,
    create_force_feedback_effect: dev_create_ffb,
    is_force_feedback_motor_powered_on: dev_ffb_powered,
    set_force_feedback_motor_gain: dev_ffb_gain,
    set_haptic_motor_state: dev_haptic,
    set_rumble_state: dev_rumble,
    set_input_synchronization_state: dev_sync_state,
    send_input_synchronization_hint: dev_sync_hint,
    power_off: dev_power_off,
    create_raw_device_report: dev_raw_create,
    get_raw_device_feature: dev_raw_get_feature,
    set_raw_device_feature: dev_raw_set_feature,
    send_raw_device_output: dev_raw_output,
    send_raw_device_output_with_response: dev_raw_output_response,
    execute_raw_device_io_control: dev_raw_ioctl,
    acquire_exclusive_raw_device_access: dev_acquire,
    release_exclusive_raw_device_access: dev_release_access,
};

pub fn new_device_object(hub: Arc<Hub>, index: usize) -> *mut c_void {
    new_obj(&DEVICE_VT, DeviceData { hub, index })
}

/// # Safety
/// `this` is a device object.
unsafe fn device_of<'a>(this: This) -> &'a DeviceData {
    // SAFETY: the caller's contract.
    &unsafe { obj::<DeviceVt, DeviceData>(this) }.data
}

unsafe extern "win64" fn dev_info(this: This) -> *const abi::DeviceInfo {
    // SAFETY: `this` is a device object.
    let d = unsafe { device_of(this) };
    d.hub.device_info(d.index).unwrap_or(std::ptr::null())
}

unsafe extern "win64" fn dev_status(this: This) -> u32 {
    // SAFETY: `this` is a device object.
    let d = unsafe { device_of(this) };
    d.hub.device_status(d.index)
}

unsafe extern "win64" fn dev_battery(this: This, out: *mut BatteryState) {
    if out.is_null() {
        return;
    }
    // SAFETY: `this` is a device object and `out` is the caller's buffer.
    unsafe {
        let d = device_of(this);
        *out = d.hub.battery(d.index);
    }
}

/// Gamepads have no force-feedback motors, as on a real Xbox pad; their vibration goes through
/// `SetRumbleState`.
unsafe extern "win64" fn dev_create_ffb(
    _: This,
    _motor: u32,
    _params: *const c_void,
    out: *mut *mut c_void,
) -> Hresult {
    if !out.is_null() {
        // SAFETY: `out` is the caller's out pointer.
        unsafe { *out = std::ptr::null_mut() };
    }
    E_INVALIDARG
}

unsafe extern "win64" fn dev_ffb_powered(_: This, _motor: u32) -> bool {
    false
}

unsafe extern "win64" fn dev_ffb_gain(_: This, _motor: u32, _gain: f32) {}

unsafe extern "win64" fn dev_haptic(_: This, _motor: u32, _params: *const c_void) -> Hresult {
    E_INVALIDARG
}

unsafe extern "win64" fn dev_rumble(this: This, params: *const RumbleParams) {
    // SAFETY: `this` is a device object; `params` is null or the caller's struct. Null stops the motors.
    unsafe {
        let d = device_of(this);
        let params = if params.is_null() {
            RumbleParams::default()
        } else {
            *params
        };
        d.hub.set_rumble(d.index, params);
    }
}

unsafe extern "win64" fn dev_sync_state(_: This, _on: bool) {}
unsafe extern "win64" fn dev_sync_hint(_: This) {}
unsafe extern "win64" fn dev_power_off(_: This) {}

fn raw_unsupported(name: &str) -> Hresult {
    note(format!("IGameInputDevice::{name} is not implemented"));
    E_NOTIMPL
}

unsafe extern "win64" fn dev_raw_create(_: This, _: u32, _: u32, out: *mut *mut c_void) -> Hresult {
    if !out.is_null() {
        // SAFETY: `out` is the caller's out pointer.
        unsafe { *out = std::ptr::null_mut() };
    }
    raw_unsupported("CreateRawDeviceReport")
}

unsafe extern "win64" fn dev_raw_get_feature(_: This, _: u32, out: *mut *mut c_void) -> Hresult {
    if !out.is_null() {
        // SAFETY: `out` is the caller's out pointer.
        unsafe { *out = std::ptr::null_mut() };
    }
    raw_unsupported("GetRawDeviceFeature")
}

unsafe extern "win64" fn dev_raw_set_feature(_: This, _: *mut c_void) -> Hresult {
    raw_unsupported("SetRawDeviceFeature")
}

unsafe extern "win64" fn dev_raw_output(_: This, _: *mut c_void) -> Hresult {
    raw_unsupported("SendRawDeviceOutput")
}

unsafe extern "win64" fn dev_raw_output_response(
    _: This,
    _: *mut c_void,
    out: *mut *mut c_void,
) -> Hresult {
    if !out.is_null() {
        // SAFETY: `out` is the caller's out pointer.
        unsafe { *out = std::ptr::null_mut() };
    }
    raw_unsupported("SendRawDeviceOutputWithResponse")
}

unsafe extern "win64" fn dev_raw_ioctl(
    _: This,
    _: u32,
    _: usize,
    _: *const c_void,
    _: usize,
    _: *mut c_void,
    _: *mut usize,
) -> Hresult {
    raw_unsupported("ExecuteRawDeviceIoControl")
}

unsafe extern "win64" fn dev_acquire(_: This, _: u64) -> bool {
    false
}

unsafe extern "win64" fn dev_release_access(_: This) {}

// ---- IGameInputReading -----------------------------------------------------------------------------------

struct ReadingData {
    hub: Arc<Hub>,
    reading: Arc<Reading>,
}

#[repr(C)]
struct ReadingVt {
    unknown: Unknown,
    get_input_kind: unsafe extern "win64" fn(This) -> u32,
    get_sequence_number: unsafe extern "win64" fn(This, u32) -> u64,
    get_timestamp: unsafe extern "win64" fn(This) -> u64,
    get_device: unsafe extern "win64" fn(This, *mut *mut c_void),
    get_raw_report: unsafe extern "win64" fn(This, *mut *mut c_void) -> bool,
    get_controller_axis_count: unsafe extern "win64" fn(This) -> u32,
    get_controller_axis_state: unsafe extern "win64" fn(This, u32, *mut f32) -> u32,
    get_controller_button_count: unsafe extern "win64" fn(This) -> u32,
    get_controller_button_state: unsafe extern "win64" fn(This, u32, *mut bool) -> u32,
    get_controller_switch_count: unsafe extern "win64" fn(This) -> u32,
    get_controller_switch_state: unsafe extern "win64" fn(This, u32, *mut c_void) -> u32,
    get_key_count: unsafe extern "win64" fn(This) -> u32,
    get_key_state: unsafe extern "win64" fn(This, u32, *mut KeyState) -> u32,
    get_mouse_state: unsafe extern "win64" fn(This, *mut MouseState) -> bool,
    get_touch_count: unsafe extern "win64" fn(This) -> u32,
    get_touch_state: unsafe extern "win64" fn(This, u32, *mut c_void) -> u32,
    get_motion_state: unsafe extern "win64" fn(This, *mut c_void) -> bool,
    get_arcade_stick_state: unsafe extern "win64" fn(This, *mut c_void) -> bool,
    get_flight_stick_state: unsafe extern "win64" fn(This, *mut c_void) -> bool,
    get_gamepad_state: unsafe extern "win64" fn(This, *mut GamepadState) -> bool,
    get_racing_wheel_state: unsafe extern "win64" fn(This, *mut c_void) -> bool,
    get_ui_navigation_state: unsafe extern "win64" fn(This, *mut u32) -> bool,
}

static READING_VT: ReadingVt = ReadingVt {
    unknown: Unknown {
        query_interface: query_interface::<ReadingVt, ReadingData>,
        add_ref: add_ref::<ReadingVt, ReadingData>,
        release: release_owned::<ReadingVt, ReadingData>,
    },
    get_input_kind: rd_kind,
    get_sequence_number: rd_sequence,
    get_timestamp: rd_timestamp,
    get_device: rd_device,
    get_raw_report: rd_raw_report,
    get_controller_axis_count: rd_axis_count,
    get_controller_axis_state: rd_axis_state,
    get_controller_button_count: rd_button_count,
    get_controller_button_state: rd_button_state,
    get_controller_switch_count: rd_zero,
    get_controller_switch_state: rd_zero_state,
    get_key_count: rd_key_count,
    get_key_state: rd_key_state,
    get_mouse_state: rd_mouse_state,
    get_touch_count: rd_zero,
    get_touch_state: rd_zero_state,
    get_motion_state: rd_no_state,
    get_arcade_stick_state: rd_no_state,
    get_flight_stick_state: rd_no_state,
    get_gamepad_state: rd_gamepad_state,
    get_racing_wheel_state: rd_no_state,
    get_ui_navigation_state: rd_ui_state,
};

/// A reading object with one reference for the caller.
pub fn new_reading_object(hub: Arc<Hub>, reading: Arc<Reading>) -> *mut c_void {
    new_obj(&READING_VT, ReadingData { hub, reading })
}

/// # Safety
/// `this` is a reading object.
unsafe fn reading_of<'a>(this: This) -> &'a ReadingData {
    // SAFETY: the caller's contract.
    &unsafe { obj::<ReadingVt, ReadingData>(this) }.data
}

unsafe extern "win64" fn rd_kind(this: This) -> u32 {
    // SAFETY: `this` is a reading object.
    unsafe { reading_of(this) }.reading.kinds
}

unsafe extern "win64" fn rd_sequence(this: This, kinds: u32) -> u64 {
    // SAFETY: `this` is a reading object.
    let r = &unsafe { reading_of(this) }.reading;
    if r.kinds & kinds != 0 { r.sequence } else { 0 }
}

unsafe extern "win64" fn rd_timestamp(this: This) -> u64 {
    // SAFETY: `this` is a reading object.
    unsafe { reading_of(this) }.reading.timestamp
}

unsafe extern "win64" fn rd_device(this: This, out: *mut *mut c_void) {
    if out.is_null() {
        return;
    }
    // SAFETY: `this` is a reading object and `out` is the caller's out pointer.
    unsafe {
        let data = reading_of(this);
        *out = std::ptr::null_mut();
        if let Some(device) = data.hub.device_com(data.reading.device) {
            add_ref::<DeviceVt, DeviceData>(device as This);
            *out = device as *mut c_void;
        }
    }
}

unsafe extern "win64" fn rd_raw_report(_: This, out: *mut *mut c_void) -> bool {
    if !out.is_null() {
        // SAFETY: `out` is the caller's out pointer.
        unsafe { *out = std::ptr::null_mut() };
    }
    false
}

fn is_pad(r: &Reading) -> bool {
    r.kinds & kind::GAMEPAD != 0
}

unsafe extern "win64" fn rd_axis_count(this: This) -> u32 {
    // SAFETY: `this` is a reading object.
    if is_pad(&unsafe { reading_of(this) }.reading) {
        6
    } else {
        0
    }
}

unsafe extern "win64" fn rd_axis_state(this: This, count: u32, out: *mut f32) -> u32 {
    // SAFETY: `this` is a reading object; `out` holds `count` floats.
    unsafe {
        let r = &reading_of(this).reading;
        if !is_pad(r) || out.is_null() {
            return 0;
        }
        let axes = mapping::controller_axes(&r.pad);
        let n = (count as usize).min(axes.len());
        std::ptr::copy_nonoverlapping(axes.as_ptr(), out, n);
        n as u32
    }
}

unsafe extern "win64" fn rd_button_count(this: This) -> u32 {
    // SAFETY: `this` is a reading object.
    if is_pad(&unsafe { reading_of(this) }.reading) {
        abi::buttons::COUNT as u32
    } else {
        0
    }
}

unsafe extern "win64" fn rd_button_state(this: This, count: u32, out: *mut bool) -> u32 {
    // SAFETY: `this` is a reading object; `out` holds `count` bools.
    unsafe {
        let r = &reading_of(this).reading;
        if !is_pad(r) || out.is_null() {
            return 0;
        }
        let buttons = mapping::controller_buttons(&r.pad);
        let n = (count as usize).min(buttons.len());
        std::ptr::copy_nonoverlapping(buttons.as_ptr(), out, n);
        n as u32
    }
}

unsafe extern "win64" fn rd_zero(_: This) -> u32 {
    0
}

unsafe extern "win64" fn rd_zero_state(_: This, _: u32, _: *mut c_void) -> u32 {
    0
}

unsafe extern "win64" fn rd_key_count(this: This) -> u32 {
    // SAFETY: `this` is a reading object.
    unsafe { reading_of(this) }.reading.keys.len() as u32
}

unsafe extern "win64" fn rd_key_state(this: This, count: u32, out: *mut KeyState) -> u32 {
    // SAFETY: `this` is a reading object; `out` holds `count` entries.
    unsafe {
        let keys = &reading_of(this).reading.keys;
        if out.is_null() {
            return 0;
        }
        let n = (count as usize).min(keys.len());
        std::ptr::copy_nonoverlapping(keys.as_ptr(), out, n);
        n as u32
    }
}

unsafe extern "win64" fn rd_mouse_state(this: This, out: *mut MouseState) -> bool {
    // SAFETY: `this` is a reading object; `out` is the caller's struct.
    unsafe {
        let r = &reading_of(this).reading;
        if r.kinds & kind::MOUSE == 0 || out.is_null() {
            return false;
        }
        *out = r.mouse;
    }
    true
}

unsafe extern "win64" fn rd_no_state(_: This, _: *mut c_void) -> bool {
    false
}

unsafe extern "win64" fn rd_gamepad_state(this: This, out: *mut GamepadState) -> bool {
    // SAFETY: `this` is a reading object; `out` is the caller's struct.
    unsafe {
        let r = &reading_of(this).reading;
        if !is_pad(r) || out.is_null() {
            return false;
        }
        *out = r.pad;
    }
    true
}

unsafe extern "win64" fn rd_ui_state(this: This, out: *mut u32) -> bool {
    // SAFETY: `this` is a reading object; `out` is the caller's struct.
    unsafe {
        let r = &reading_of(this).reading;
        if !is_pad(r) || out.is_null() {
            return false;
        }
        *out = mapping::ui_navigation(&r.pad);
    }
    true
}

// ---- IGameInputDispatcher --------------------------------------------------------------------------------

struct DispatcherData {
    hub: Arc<Hub>,
}

#[repr(C)]
struct DispatcherVt {
    unknown: Unknown,
    dispatch: unsafe extern "win64" fn(This, u64) -> bool,
    open_wait_handle: unsafe extern "win64" fn(This, *mut *mut c_void) -> Hresult,
}

static DISPATCHER_VT: DispatcherVt = DispatcherVt {
    unknown: Unknown {
        query_interface: query_interface::<DispatcherVt, DispatcherData>,
        add_ref: add_ref::<DispatcherVt, DispatcherData>,
        release: release_owned::<DispatcherVt, DispatcherData>,
    },
    dispatch: disp_dispatch,
    open_wait_handle: disp_open_wait_handle,
};

unsafe extern "win64" fn disp_dispatch(this: This, quota: u64) -> bool {
    // SAFETY: `this` is a dispatcher object.
    unsafe { obj::<DispatcherVt, DispatcherData>(this) }
        .data
        .hub
        .dispatch(quota)
}

unsafe extern "win64" fn disp_open_wait_handle(this: This, out: *mut *mut c_void) -> Hresult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: `this` is a dispatcher object and `out` is non-null.
    unsafe {
        let hub = &obj::<DispatcherVt, DispatcherData>(this).data.hub;
        match hub.dispatcher_event() {
            Some(event) => {
                *out = events::duplicate(event) as *mut c_void;
                S_OK
            }
            None => E_INVALIDARG,
        }
    }
}
