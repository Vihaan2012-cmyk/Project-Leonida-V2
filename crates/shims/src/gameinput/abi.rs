//! GameInput v0 data layouts and constants, as the public header lays them out.

pub const S_OK: i32 = 0;
pub const E_NOTIMPL: i32 = 0x8000_4001_u32 as i32;
pub const E_NOINTERFACE: i32 = 0x8000_4002_u32 as i32;
pub const E_POINTER: i32 = 0x8000_4003_u32 as i32;
pub const E_INVALIDARG: i32 = 0x8007_0057_u32 as i32;
pub const GAMEINPUT_E_DEVICE_DISCONNECTED: i32 = 0x838A_0001_u32 as i32;
pub const GAMEINPUT_E_DEVICE_NOT_FOUND: i32 = 0x838A_0002_u32 as i32;
pub const GAMEINPUT_E_READING_NOT_FOUND: i32 = 0x838A_0003_u32 as i32;
pub const GAMEINPUT_E_REFERENCE_READING_TOO_OLD: i32 = 0x838A_0004_u32 as i32;

pub mod kind {
    pub const RAW_DEVICE_REPORT: u32 = 0x1;
    pub const CONTROLLER_AXIS: u32 = 0x2;
    pub const CONTROLLER_BUTTON: u32 = 0x4;
    pub const CONTROLLER_SWITCH: u32 = 0x8;
    pub const KEYBOARD: u32 = 0x10;
    pub const MOUSE: u32 = 0x20;
    pub const TOUCH: u32 = 0x100;
    pub const MOTION: u32 = 0x1000;
    pub const ARCADE_STICK: u32 = 0x1_0000;
    pub const FLIGHT_STICK: u32 = 0x2_0000;
    pub const GAMEPAD: u32 = 0x4_0000;
    pub const RACING_WHEEL: u32 = 0x8_0000;
    pub const UI_NAVIGATION: u32 = 0x100_0000;

    /// What a gamepad reading carries.
    pub const PAD: u32 = GAMEPAD | UI_NAVIGATION | CONTROLLER_AXIS | CONTROLLER_BUTTON;
}

pub mod status {
    pub const NONE: u32 = 0;
    pub const CONNECTED: u32 = 0x1;
    pub const INPUT_ENABLED: u32 = 0x2;
    pub const OUTPUT_ENABLED: u32 = 0x4;
    pub const WIRELESS: u32 = 0x80;
    pub const ANY: u32 = 0x00FF_FFFF;

    pub const ONLINE: u32 = CONNECTED | INPUT_ENABLED | OUTPUT_ENABLED;
}

pub mod buttons {
    pub const MENU: u32 = 0x1;
    pub const VIEW: u32 = 0x2;
    pub const A: u32 = 0x4;
    pub const B: u32 = 0x8;
    pub const X: u32 = 0x10;
    pub const Y: u32 = 0x20;
    pub const DPAD_UP: u32 = 0x40;
    pub const DPAD_DOWN: u32 = 0x80;
    pub const DPAD_LEFT: u32 = 0x100;
    pub const DPAD_RIGHT: u32 = 0x200;
    pub const LEFT_SHOULDER: u32 = 0x400;
    pub const RIGHT_SHOULDER: u32 = 0x800;
    pub const LEFT_THUMBSTICK: u32 = 0x1000;
    pub const RIGHT_THUMBSTICK: u32 = 0x2000;
    pub const COUNT: usize = 14;
}

pub mod ui {
    pub const MENU: u32 = 0x1;
    pub const VIEW: u32 = 0x2;
    pub const ACCEPT: u32 = 0x4;
    pub const CANCEL: u32 = 0x8;
    pub const UP: u32 = 0x10;
    pub const DOWN: u32 = 0x20;
    pub const LEFT: u32 = 0x40;
    pub const RIGHT: u32 = 0x80;
    pub const CONTEXT1: u32 = 0x100;
    pub const CONTEXT2: u32 = 0x200;
    pub const CONTEXT3: u32 = 0x400;
    pub const CONTEXT4: u32 = 0x800;
    pub const PAGE_UP: u32 = 0x1000;
    pub const PAGE_DOWN: u32 = 0x2000;
    pub const PAGE_LEFT: u32 = 0x4000;
    pub const PAGE_RIGHT: u32 = 0x8000;
    pub const SCROLL_UP: u32 = 0x1_0000;
    pub const SCROLL_DOWN: u32 = 0x2_0000;
    pub const SCROLL_LEFT: u32 = 0x4_0000;
    pub const SCROLL_RIGHT: u32 = 0x8_0000;
}

pub mod mouse_buttons {
    pub const LEFT: u32 = 0x1;
    pub const RIGHT: u32 = 0x2;
    pub const MIDDLE: u32 = 0x4;
    pub const X1: u32 = 0x8;
    pub const X2: u32 = 0x10;
    pub const TILT_LEFT: u32 = 0x20;
    pub const TILT_RIGHT: u32 = 0x40;
}

pub mod rumble_motors {
    pub const LOW_FREQUENCY: u32 = 0x1;
    pub const HIGH_FREQUENCY: u32 = 0x2;
    pub const LEFT_TRIGGER: u32 = 0x4;
    pub const RIGHT_TRIGGER: u32 = 0x8;
}

pub const FAMILY_XBOX_ONE: u32 = 1;
pub const FAMILY_HID: u32 = 3;

pub const ENUMERATE_ASYNC: u32 = 1;
pub const ENUMERATE_BLOCKING: u32 = 2;

pub mod label {
    pub const NONE: i32 = 0;
    pub const XBOX_MENU: i32 = 4;
    pub const XBOX_VIEW: i32 = 5;
    pub const XBOX_A: i32 = 7;
    pub const XBOX_B: i32 = 8;
    pub const XBOX_X: i32 = 9;
    pub const XBOX_Y: i32 = 10;
    pub const XBOX_DPAD_UP: i32 = 11;
    pub const XBOX_DPAD_DOWN: i32 = 12;
    pub const XBOX_DPAD_LEFT: i32 = 13;
    pub const XBOX_DPAD_RIGHT: i32 = 14;
    pub const XBOX_LEFT_SHOULDER: i32 = 15;
    pub const XBOX_LEFT_TRIGGER: i32 = 16;
    pub const XBOX_LEFT_STICK_BUTTON: i32 = 17;
    pub const XBOX_RIGHT_SHOULDER: i32 = 18;
    pub const XBOX_RIGHT_TRIGGER: i32 = 19;
    pub const XBOX_RIGHT_STICK_BUTTON: i32 = 20;
}

pub const BATTERY_NOT_PRESENT: u32 = 0;
pub const BATTERY_DISCHARGING: u32 = 1;
pub const BATTERY_IDLE: u32 = 2;
pub const BATTERY_CHARGING: u32 = 3;

/// `GameInputGamepadState`, 28 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct GamepadState {
    pub buttons: u32,
    pub left_trigger: f32,
    pub right_trigger: f32,
    pub left_x: f32,
    pub left_y: f32,
    pub right_x: f32,
    pub right_y: f32,
}

/// `GameInputKeyState`, 12 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyState {
    pub scan_code: u32,
    pub code_point: u32,
    pub virtual_key: u8,
    pub is_dead_key: bool,
}

/// `GameInputMouseState`, 40 bytes. Positions and wheels are running totals of the movement.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MouseState {
    pub buttons: u32,
    pub position_x: i64,
    pub position_y: i64,
    pub wheel_x: i64,
    pub wheel_y: i64,
}

/// `GameInputBatteryState`, 20 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BatteryState {
    pub charge_rate: f32,
    pub max_charge_rate: f32,
    pub remaining_capacity: f32,
    pub full_charge_capacity: f32,
    pub status: u32,
}

/// `GameInputRumbleParams`, 16 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RumbleParams {
    pub low_frequency: f32,
    pub high_frequency: f32,
    pub left_trigger: f32,
    pub right_trigger: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Usage {
    pub page: u16,
    pub id: u16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Version {
    pub major: u16,
    pub minor: u16,
    pub build: u16,
    pub revision: u16,
}

/// `GameInputString`.
#[repr(C)]
pub struct GiString {
    pub size_in_bytes: u32,
    pub code_point_count: u32,
    pub data: *const u8,
}

/// `GameInputGamepadInfo`: a label per control.
#[repr(C)]
pub struct GamepadInfo {
    pub labels: [i32; 14],
}

/// `GameInputUiNavigationInfo`.
#[repr(C)]
pub struct UiNavigationInfo {
    pub labels: [i32; 21],
}

/// `GameInputMouseInfo`.
#[repr(C)]
pub struct MouseInfo {
    pub supported_buttons: u32,
    pub sample_rate: u32,
    pub sensor_dpi: u32,
    pub has_wheel_x: bool,
    pub has_wheel_y: bool,
}

/// `GameInputKeyboardInfo`.
#[repr(C)]
pub struct KeyboardInfo {
    pub kind: u32,
    pub layout: u32,
    pub key_count: u32,
    pub function_key_count: u32,
    pub max_simultaneous_keys: u32,
    pub platform_type: u32,
    pub platform_subtype: u32,
    pub native_language: *const GiString,
}

/// `GameInputControllerAxisInfo`, 48 bytes: the label sits at offset 4.
#[repr(C)]
pub struct AxisInfo {
    pub mapped_input_kinds: u32,
    pub label: i32,
    pub is_continuous: bool,
    pub is_non_linear: bool,
    pub is_quantized: bool,
    pub has_null_value: bool,
    pub resolution: f32,
    pub min_value: f64,
    pub max_value: f64,
    pub null_value: f64,
    pub quantization: f64,
}

/// `GameInputControllerButtonInfo`, 32 bytes: the label sits at offset 4.
#[repr(C)]
pub struct ButtonInfo {
    pub mapped_input_kinds: u32,
    pub label: i32,
    pub reserved: [u32; 6],
}

/// `GameInputDeviceInfo`, 320 bytes.
#[repr(C)]
pub struct DeviceInfo {
    pub info_size: u32,
    pub vendor_id: u16,
    pub product_id: u16,
    pub revision_number: u16,
    pub interface_number: u8,
    pub collection_number: u8,
    pub usage: Usage,
    pub hardware_version: Version,
    pub firmware_version: Version,
    pub device_id: [u8; 32],
    pub device_root_id: [u8; 32],
    pub device_family: u32,
    pub supported_input: u32,
    pub supported_rumble_motors: u32,
    pub input_report_count: u32,
    pub output_report_count: u32,
    pub feature_report_count: u32,
    pub controller_axis_count: u32,
    pub controller_button_count: u32,
    pub controller_switch_count: u32,
    pub touch_point_count: u32,
    pub touch_sensor_count: u32,
    pub force_feedback_motor_count: u32,
    pub haptic_feedback_motor_count: u32,
    pub device_string_count: u32,
    pub device_descriptor_size: u32,
    pub input_report_info: *const u8,
    pub output_report_info: *const u8,
    pub feature_report_info: *const u8,
    pub controller_axis_info: *const AxisInfo,
    pub controller_button_info: *const ButtonInfo,
    pub controller_switch_info: *const u8,
    pub keyboard_info: *const KeyboardInfo,
    pub mouse_info: *const MouseInfo,
    pub touch_sensor_info: *const u8,
    pub motion_info: *const u8,
    pub arcade_stick_info: *const u8,
    pub flight_stick_info: *const u8,
    pub gamepad_info: *const GamepadInfo,
    pub racing_wheel_info: *const u8,
    pub ui_navigation_info: *const UiNavigationInfo,
    pub force_feedback_motor_info: *const u8,
    pub haptic_feedback_motor_info: *const u8,
    pub display_name: *const GiString,
    pub device_strings: *const GiString,
    pub device_descriptor_data: *const u8,
}

const _: () = {
    assert!(size_of::<GamepadState>() == 28);
    assert!(size_of::<KeyState>() == 12);
    assert!(size_of::<MouseState>() == 40);
    assert!(size_of::<BatteryState>() == 20);
    assert!(size_of::<AxisInfo>() == 48);
    assert!(size_of::<ButtonInfo>() == 32);
    assert!(size_of::<DeviceInfo>() == 320);
    assert!(std::mem::offset_of!(DeviceInfo, device_family) == 96);
    assert!(std::mem::offset_of!(DeviceInfo, input_report_info) == 160);
    assert!(std::mem::offset_of!(AxisInfo, label) == 4);
};
