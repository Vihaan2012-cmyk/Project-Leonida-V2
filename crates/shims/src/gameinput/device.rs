//! The `GameInputDeviceInfo` block each device hands out, with everything it points to.

use std::ptr::null;

use super::abi::{
    AxisInfo, ButtonInfo, DeviceInfo, FAMILY_HID, FAMILY_XBOX_ONE, GamepadInfo, GiString,
    KeyboardInfo, MouseInfo, UiNavigationInfo, Usage, Version, kind, label, mouse_buttons,
    rumble_motors,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// A controller that gilrs reports.
    Pad,
    /// The gamepad the keyboard and mouse make.
    VirtualPad,
    Keyboard,
    Mouse,
}

impl Class {
    pub fn supported_input(self) -> u32 {
        match self {
            Class::Pad | Class::VirtualPad => kind::PAD,
            Class::Keyboard => kind::KEYBOARD,
            Class::Mouse => kind::MOUSE,
        }
    }

    pub fn is_pad(self) -> bool {
        matches!(self, Class::Pad | Class::VirtualPad)
    }
}

/// The info and the tables and strings its pointers lead to. It lives in a box that is never moved or freed
/// while the device exists.
pub struct InfoBlock {
    pub info: DeviceInfo,
    _name: Vec<u8>,
    _display: Box<GiString>,
    _gamepad: Box<GamepadInfo>,
    _ui: Box<UiNavigationInfo>,
    _axes: Vec<AxisInfo>,
    _buttons: Vec<ButtonInfo>,
    _mouse: Box<MouseInfo>,
    _keyboard: Box<KeyboardInfo>,
}

// SAFETY: the pointers in `info` lead only into this block's own heap allocations, which are never written
// after construction.
unsafe impl Send for InfoBlock {}

const GAMEPAD_LABELS: [i32; 14] = [
    label::XBOX_MENU,
    label::XBOX_VIEW,
    label::XBOX_A,
    label::XBOX_B,
    label::XBOX_X,
    label::XBOX_Y,
    label::XBOX_DPAD_UP,
    label::XBOX_DPAD_DOWN,
    label::XBOX_DPAD_LEFT,
    label::XBOX_DPAD_RIGHT,
    label::XBOX_LEFT_SHOULDER,
    label::XBOX_RIGHT_SHOULDER,
    label::XBOX_LEFT_STICK_BUTTON,
    label::XBOX_RIGHT_STICK_BUTTON,
];

const AXIS_LABELS: [i32; 6] = [
    label::NONE,
    label::NONE,
    label::NONE,
    label::NONE,
    label::XBOX_LEFT_TRIGGER,
    label::XBOX_RIGHT_TRIGGER,
];

fn empty_info() -> DeviceInfo {
    DeviceInfo {
        info_size: size_of::<DeviceInfo>() as u32,
        vendor_id: 0,
        product_id: 0,
        revision_number: 0,
        interface_number: 0,
        collection_number: 0,
        usage: Usage { page: 0, id: 0 },
        hardware_version: Version {
            major: 0,
            minor: 0,
            build: 0,
            revision: 0,
        },
        firmware_version: Version {
            major: 0,
            minor: 0,
            build: 0,
            revision: 0,
        },
        device_id: [0; 32],
        device_root_id: [0; 32],
        device_family: 0,
        supported_input: 0,
        supported_rumble_motors: 0,
        input_report_count: 0,
        output_report_count: 0,
        feature_report_count: 0,
        controller_axis_count: 0,
        controller_button_count: 0,
        controller_switch_count: 0,
        touch_point_count: 0,
        touch_sensor_count: 0,
        force_feedback_motor_count: 0,
        haptic_feedback_motor_count: 0,
        device_string_count: 0,
        device_descriptor_size: 0,
        input_report_info: null(),
        output_report_info: null(),
        feature_report_info: null(),
        controller_axis_info: null(),
        controller_button_info: null(),
        controller_switch_info: null(),
        keyboard_info: null(),
        mouse_info: null(),
        touch_sensor_info: null(),
        motion_info: null(),
        arcade_stick_info: null(),
        flight_stick_info: null(),
        gamepad_info: null(),
        racing_wheel_info: null(),
        ui_navigation_info: null(),
        force_feedback_motor_info: null(),
        haptic_feedback_motor_info: null(),
        display_name: null(),
        device_strings: null(),
        device_descriptor_data: null(),
    }
}

/// Builds the block for a device. Every pad is presented as an Xbox One pad with all four rumble motors, the
/// family the title was built for, whatever it really is.
pub fn build(class: Class, name: &str, vendor: u16, product: u16, id: [u8; 32]) -> Box<InfoBlock> {
    let name_bytes = name.as_bytes().to_vec();
    let display = Box::new(GiString {
        size_in_bytes: name_bytes.len() as u32 + 1,
        code_point_count: name.chars().count() as u32,
        data: null(),
    });
    let mut block = Box::new(InfoBlock {
        info: empty_info(),
        _name: name_bytes,
        _display: display,
        _gamepad: Box::new(GamepadInfo {
            labels: GAMEPAD_LABELS,
        }),
        _ui: Box::new(UiNavigationInfo { labels: [0; 21] }),
        _axes: Vec::new(),
        _buttons: Vec::new(),
        _mouse: Box::new(MouseInfo {
            supported_buttons: mouse_buttons::LEFT
                | mouse_buttons::RIGHT
                | mouse_buttons::MIDDLE
                | mouse_buttons::X1
                | mouse_buttons::X2,
            sample_rate: 1000,
            sensor_dpi: 800,
            has_wheel_x: true,
            has_wheel_y: true,
        }),
        _keyboard: Box::new(KeyboardInfo {
            kind: 0,
            layout: 0,
            key_count: 104,
            function_key_count: 12,
            max_simultaneous_keys: 6,
            platform_type: 0,
            platform_subtype: 0,
            native_language: null(),
        }),
    });
    // The name is stored with a terminating zero so `data` is a C string.
    block._name.push(0);
    block._display.data = block._name.as_ptr();
    block._axes = AXIS_LABELS
        .iter()
        .map(|&label| AxisInfo {
            mapped_input_kinds: kind::GAMEPAD,
            label,
            is_continuous: true,
            is_non_linear: false,
            is_quantized: false,
            has_null_value: false,
            resolution: 1.0 / 65535.0,
            min_value: 0.0,
            max_value: 1.0,
            null_value: 0.0,
            quantization: 0.0,
        })
        .collect();
    block._buttons = GAMEPAD_LABELS
        .iter()
        .map(|&label| ButtonInfo {
            mapped_input_kinds: kind::GAMEPAD,
            label,
            reserved: [0; 6],
        })
        .collect();

    let info = &mut block.info;
    info.vendor_id = vendor;
    info.product_id = product;
    info.device_id = id;
    info.device_root_id = id;
    info.supported_input = class.supported_input();
    info.display_name = &*block._display;
    match class {
        Class::Pad | Class::VirtualPad => {
            info.device_family = FAMILY_XBOX_ONE;
            info.usage = Usage { page: 1, id: 5 };
            info.supported_rumble_motors = rumble_motors::LOW_FREQUENCY
                | rumble_motors::HIGH_FREQUENCY
                | rumble_motors::LEFT_TRIGGER
                | rumble_motors::RIGHT_TRIGGER;
            info.controller_axis_count = block._axes.len() as u32;
            info.controller_button_count = block._buttons.len() as u32;
            info.controller_axis_info = block._axes.as_ptr();
            info.controller_button_info = block._buttons.as_ptr();
            info.gamepad_info = &*block._gamepad;
            info.ui_navigation_info = &*block._ui;
        }
        Class::Keyboard => {
            info.device_family = FAMILY_HID;
            info.usage = Usage { page: 1, id: 6 };
            info.keyboard_info = &*block._keyboard;
        }
        Class::Mouse => {
            info.device_family = FAMILY_HID;
            info.usage = Usage { page: 1, id: 2 };
            info.mouse_info = &*block._mouse;
        }
    }
    block
}

/// FNV-1a, for the second half of a device id.
pub fn fnv(bytes: &[u8]) -> u128 {
    let mut hash: u128 = 0x6c62272e07bb014262b821756295c58d;
    for &b in bytes {
        hash ^= u128::from(b);
        hash = hash.wrapping_mul(0x0000000001000000000000000000013B);
    }
    hash
}

/// The 32-byte `APP_LOCAL_DEVICE_ID`: the device's GUID, then a hash of what tells it from a twin.
pub fn device_id(uuid: [u8; 16], tag: &str) -> [u8; 32] {
    let mut id = [0; 32];
    id[..16].copy_from_slice(&uuid);
    id[16..].copy_from_slice(&fnv(tag.as_bytes()).to_le_bytes());
    id
}
