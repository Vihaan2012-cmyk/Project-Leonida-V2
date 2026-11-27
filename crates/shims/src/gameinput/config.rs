//! How keyboard and mouse become a gamepad, and the output switches. A plain struct for now; `Config/input.toml`
//! fills it once the config crate lands.

use super::abi::{buttons, mouse_buttons};
use super::analog::Movement;
use super::curve::LookSettings;

/// Virtual-key codes the default bindings use.
pub mod vk {
    pub const BACK: u8 = 0x08;
    pub const MENU: u8 = 0x12;
    pub const TAB: u8 = 0x09;
    pub const RETURN: u8 = 0x0D;
    pub const SHIFT: u8 = 0x10;
    pub const ESCAPE: u8 = 0x1B;
    pub const SPACE: u8 = 0x20;
    pub const LEFT: u8 = 0x25;
    pub const UP: u8 = 0x26;
    pub const RIGHT: u8 = 0x27;
    pub const DOWN: u8 = 0x28;
    pub const A: u8 = b'A';
    pub const D: u8 = b'D';
    pub const E: u8 = b'E';
    pub const Q: u8 = b'Q';
    pub const S: u8 = b'S';
    pub const W: u8 = b'W';
    pub const X: u8 = b'X';
    pub const Y: u8 = b'Y';
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

impl MouseButton {
    /// The `GameInputMouseButtons` bit.
    pub fn bit(self) -> u32 {
        match self {
            MouseButton::Left => mouse_buttons::LEFT,
            MouseButton::Right => mouse_buttons::RIGHT,
            MouseButton::Middle => mouse_buttons::MIDDLE,
            MouseButton::X1 => mouse_buttons::X1,
            MouseButton::X2 => mouse_buttons::X2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelDir {
    Up,
    Down,
}

/// What the player presses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Key(u8),
    Mouse(MouseButton),
    /// One notch of the mouse wheel is a short press.
    Wheel(WheelDir),
}

/// What it does on the virtual pad.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// One `GameInputGamepadButtons` bit.
    Button(u32),
    LeftTrigger,
    RightTrigger,
    LeftStick(Direction),
    RightStick(Direction),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub source: Source,
    pub control: Control,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VirtualPadMode {
    Off,
    Always,
    /// Present only while no physical gamepad is connected.
    Auto,
    /// The only gamepad the title sees: connected controllers are folded into it with the keyboard and mouse, and
    /// whichever was used last wins where both push the same stick or trigger.
    #[default]
    Merged,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InputConfig {
    pub virtual_pad: VirtualPadMode,
    pub bindings: Vec<Binding>,
    /// Scales how far mouse movement pushes the right stick.
    pub mouse_sensitivity: f32,
    pub look: LookSettings,
    pub movement: Movement,
    /// Whether the mouse moves the right stick at all.
    pub mouse_look: bool,
    /// Turns every rumble output off when false.
    pub rumble: bool,
    /// Multiplies rumble levels before they reach the pad.
    pub rumble_scale: f32,
}

impl Default for InputConfig {
    fn default() -> Self {
        InputConfig {
            virtual_pad: VirtualPadMode::default(),
            bindings: default_bindings(),
            mouse_sensitivity: 1.0,
            look: LookSettings::default(),
            movement: Movement::default(),
            mouse_look: true,
            rumble: true,
            rumble_scale: 1.0,
        }
    }
}

pub fn default_bindings() -> Vec<Binding> {
    use Control::*;
    use Direction::*;
    let key = |code, control| Binding {
        source: Source::Key(code),
        control,
    };
    vec![
        key(vk::UP, Button(buttons::DPAD_UP)),
        key(vk::DOWN, Button(buttons::DPAD_DOWN)),
        key(vk::LEFT, Button(buttons::DPAD_LEFT)),
        key(vk::RIGHT, Button(buttons::DPAD_RIGHT)),
        key(vk::RETURN, Button(buttons::A)),
        key(vk::SPACE, Button(buttons::A)),
        key(vk::BACK, Button(buttons::B)),
        key(vk::X, Button(buttons::X)),
        key(vk::Y, Button(buttons::Y)),
        key(vk::Q, Button(buttons::LEFT_SHOULDER)),
        key(vk::E, Button(buttons::RIGHT_SHOULDER)),
        key(vk::ESCAPE, Button(buttons::MENU)),
        key(vk::TAB, Button(buttons::VIEW)),
        key(vk::W, LeftStick(Up)),
        key(vk::A, LeftStick(Left)),
        key(vk::S, LeftStick(Down)),
        key(vk::D, LeftStick(Right)),
        Binding {
            source: Source::Mouse(MouseButton::Right),
            control: LeftTrigger,
        },
        Binding {
            source: Source::Mouse(MouseButton::Left),
            control: RightTrigger,
        },
    ]
}
