//! Tables and pure functions between devices and GameInput readings.

use gilrs::{Axis, Button, Gamepad};

use super::abi::{GamepadState, buttons, ui};
use super::config::{Binding, Control, Direction, InputConfig, Source};

/// The `GameInputGamepadButtons` bit a gilrs button sets, by position as on an Xbox pad. The triggers are
/// analog and the guide button is a callback, so neither has a bit.
pub fn gilrs_button_bit(button: Button) -> Option<u32> {
    Some(match button {
        Button::South => buttons::A,
        Button::East => buttons::B,
        Button::West => buttons::X,
        Button::North => buttons::Y,
        Button::Select => buttons::VIEW,
        Button::Start => buttons::MENU,
        Button::DPadUp => buttons::DPAD_UP,
        Button::DPadDown => buttons::DPAD_DOWN,
        Button::DPadLeft => buttons::DPAD_LEFT,
        Button::DPadRight => buttons::DPAD_RIGHT,
        Button::LeftTrigger => buttons::LEFT_SHOULDER,
        Button::RightTrigger => buttons::RIGHT_SHOULDER,
        Button::LeftThumb => buttons::LEFT_THUMBSTICK,
        Button::RightThumb => buttons::RIGHT_THUMBSTICK,
        _ => return None,
    })
}

const BIT_BUTTONS: [Button; 14] = [
    Button::Start,
    Button::Select,
    Button::South,
    Button::East,
    Button::West,
    Button::North,
    Button::DPadUp,
    Button::DPadDown,
    Button::DPadLeft,
    Button::DPadRight,
    Button::LeftTrigger,
    Button::RightTrigger,
    Button::LeftThumb,
    Button::RightThumb,
];

/// A gilrs gamepad's present state as a GameInput gamepad state. Sticks are -1..1 with +Y up and triggers
/// 0..1, with no deadzone, as on console.
pub fn pad_from_gilrs(pad: &Gamepad<'_>) -> GamepadState {
    let mut state = GamepadState::default();
    for button in BIT_BUTTONS {
        if pad.is_pressed(button)
            && let Some(bit) = gilrs_button_bit(button)
        {
            state.buttons |= bit;
        }
    }
    let analog = |button| {
        pad.button_data(button)
            .map(|d| d.value())
            .unwrap_or_default()
            .clamp(0.0, 1.0)
    };
    state.left_trigger = analog(Button::LeftTrigger2);
    state.right_trigger = analog(Button::RightTrigger2);
    let axis = |axis: Axis| pad.value(axis).clamp(-1.0, 1.0);
    state.left_x = axis(Axis::LeftStickX);
    state.left_y = axis(Axis::LeftStickY);
    state.right_x = axis(Axis::RightStickX);
    state.right_y = axis(Axis::RightStickY);
    state
}

/// `GetControllerAxisState`: left X, left Y, right X, right Y, left trigger, right trigger, all 0..1 with the
/// sticks at rest on 0.5.
pub fn controller_axes(state: &GamepadState) -> [f32; 6] {
    let stick = |v: f32| (v.clamp(-1.0, 1.0) + 1.0) * 0.5;
    [
        stick(state.left_x),
        stick(state.left_y),
        stick(state.right_x),
        stick(state.right_y),
        state.left_trigger,
        state.right_trigger,
    ]
}

/// `GetControllerButtonState`: one entry per gamepad button, in bit order.
pub fn controller_buttons(state: &GamepadState) -> [bool; buttons::COUNT] {
    std::array::from_fn(|i| state.buttons & (1 << i) != 0)
}

/// `GetUiNavigationState`, derived from the pad: A accept, B cancel, X and Y context 1 and 2, stick clicks
/// context 3 and 4, d-pad or left stick past half for the four directions, shoulders for page left and right,
/// triggers past half for page up and down, right stick past half for scrolling, Menu and View as themselves.
pub fn ui_navigation(s: &GamepadState) -> u32 {
    let b = s.buttons;
    let mut out = 0;
    let mut set = |on: bool, bit: u32| {
        if on {
            out |= bit;
        }
    };
    set(b & buttons::MENU != 0, ui::MENU);
    set(b & buttons::VIEW != 0, ui::VIEW);
    set(b & buttons::A != 0, ui::ACCEPT);
    set(b & buttons::B != 0, ui::CANCEL);
    set(b & buttons::X != 0, ui::CONTEXT1);
    set(b & buttons::Y != 0, ui::CONTEXT2);
    set(b & buttons::LEFT_THUMBSTICK != 0, ui::CONTEXT3);
    set(b & buttons::RIGHT_THUMBSTICK != 0, ui::CONTEXT4);
    set(b & buttons::DPAD_UP != 0 || s.left_y > 0.5, ui::UP);
    set(b & buttons::DPAD_DOWN != 0 || s.left_y < -0.5, ui::DOWN);
    set(b & buttons::DPAD_LEFT != 0 || s.left_x < -0.5, ui::LEFT);
    set(b & buttons::DPAD_RIGHT != 0 || s.left_x > 0.5, ui::RIGHT);
    set(b & buttons::LEFT_SHOULDER != 0, ui::PAGE_LEFT);
    set(b & buttons::RIGHT_SHOULDER != 0, ui::PAGE_RIGHT);
    set(s.left_trigger > 0.5, ui::PAGE_UP);
    set(s.right_trigger > 0.5, ui::PAGE_DOWN);
    set(s.right_y > 0.5, ui::SCROLL_UP);
    set(s.right_y < -0.5, ui::SCROLL_DOWN);
    set(s.right_x < -0.5, ui::SCROLL_LEFT);
    set(s.right_x > 0.5, ui::SCROLL_RIGHT);
    out
}

const AXIS_FULL: i32 = 32767;
const MOUSE_COUNT_UNITS: f32 = 1024.0;

/// The right stick as the mouse drives it: each poll keeps 7/8 of the value and adds 1024 axis units per
/// count the mouse moved, so steady motion of four counts a poll is full deflection and a stopped mouse lets
/// the stick fall back to exactly zero.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MouseStick {
    pub x: i32,
    pub y: i32,
}

impl MouseStick {
    /// `dy` is positive when the mouse moves down, which pushes the stick down.
    pub fn step(&mut self, dx: i64, dy: i64, sensitivity: f32) {
        let push = |count: i64| (count as f32 * MOUSE_COUNT_UNITS * sensitivity) as i32;
        self.x = (self.x * 7 / 8 + push(dx)).clamp(-AXIS_FULL, AXIS_FULL);
        self.y = (self.y * 7 / 8 - push(dy)).clamp(-AXIS_FULL, AXIS_FULL);
    }

    pub fn reset(&mut self) {
        *self = MouseStick::default();
    }
}

/// The gamepad state the keyboard and mouse make under `config`.
pub fn virtual_pad(
    config: &InputConfig,
    keys: &[bool; 256],
    mouse_buttons: u32,
    stick: MouseStick,
) -> GamepadState {
    let mut state = GamepadState::default();
    let mut left = [0f32; 2];
    let mut right = [0f32; 2];
    for &Binding { source, control } in &config.bindings {
        let down = match source {
            Source::Key(code) => keys[code as usize],
            Source::Mouse(button) => mouse_buttons & button.bit() != 0,
        };
        if !down {
            continue;
        }
        match control {
            Control::Button(bit) => state.buttons |= bit,
            Control::LeftTrigger => state.left_trigger = 1.0,
            Control::RightTrigger => state.right_trigger = 1.0,
            Control::LeftStick(dir) => push(&mut left, dir),
            Control::RightStick(dir) => push(&mut right, dir),
        }
    }
    let left = gate(left);
    let mut right = gate(right);
    if config.mouse_look {
        right[0] += stick.x as f32 / AXIS_FULL as f32;
        right[1] += stick.y as f32 / AXIS_FULL as f32;
    }
    state.left_x = left[0];
    state.left_y = left[1];
    state.right_x = right[0].clamp(-1.0, 1.0);
    state.right_y = right[1].clamp(-1.0, 1.0);
    state
}

fn push(axis: &mut [f32; 2], dir: Direction) {
    match dir {
        Direction::Up => axis[1] += 1.0,
        Direction::Down => axis[1] -= 1.0,
        Direction::Left => axis[0] -= 1.0,
        Direction::Right => axis[0] += 1.0,
    }
}

/// Opposite keys cancel inside `push`; a diagonal sits on the round gate, about 0.707 on each axis.
fn gate(axis: [f32; 2]) -> [f32; 2] {
    let clamped = [axis[0].clamp(-1.0, 1.0), axis[1].clamp(-1.0, 1.0)];
    if clamped[0] != 0.0 && clamped[1] != 0.0 {
        let k = std::f32::consts::FRAC_1_SQRT_2;
        [clamped[0] * k, clamped[1] * k]
    } else {
        clamped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gameinput::config::{MouseButton, vk};

    fn keys(down: &[u8]) -> [bool; 256] {
        let mut keys = [false; 256];
        for &k in down {
            keys[k as usize] = true;
        }
        keys
    }

    fn pad(config: &InputConfig, down: &[u8], mouse: u32) -> GamepadState {
        virtual_pad(config, &keys(down), mouse, MouseStick::default())
    }

    #[test]
    fn face_buttons_go_by_position() {
        assert_eq!(gilrs_button_bit(Button::South), Some(buttons::A));
        assert_eq!(gilrs_button_bit(Button::East), Some(buttons::B));
        assert_eq!(gilrs_button_bit(Button::West), Some(buttons::X));
        assert_eq!(gilrs_button_bit(Button::North), Some(buttons::Y));
        assert_eq!(gilrs_button_bit(Button::Select), Some(buttons::VIEW));
        assert_eq!(gilrs_button_bit(Button::Start), Some(buttons::MENU));
        assert_eq!(
            gilrs_button_bit(Button::LeftTrigger),
            Some(buttons::LEFT_SHOULDER)
        );
        assert_eq!(
            gilrs_button_bit(Button::RightThumb),
            Some(buttons::RIGHT_THUMBSTICK)
        );
        assert_eq!(gilrs_button_bit(Button::LeftTrigger2), None);
        assert_eq!(gilrs_button_bit(Button::Mode), None);
    }

    #[test]
    fn every_gamepad_bit_has_exactly_one_gilrs_button() {
        let mut all = 0;
        for button in BIT_BUTTONS {
            let bit = gilrs_button_bit(button).unwrap();
            assert_eq!(all & bit, 0);
            all |= bit;
        }
        assert_eq!(all, (1 << buttons::COUNT) - 1);
        for (i, button) in BIT_BUTTONS.iter().enumerate() {
            assert_eq!(gilrs_button_bit(*button), Some(1 << i));
        }
    }

    #[test]
    fn controller_axes_put_sticks_at_half_when_centered() {
        let s = GamepadState {
            left_trigger: 0.3,
            right_x: 1.0,
            left_y: -1.0,
            ..GamepadState::default()
        };
        assert_eq!(controller_axes(&s), [0.5, 0.0, 1.0, 0.5, 0.3, 0.0]);
    }

    #[test]
    fn ui_navigation_follows_the_pad() {
        let none = GamepadState::default();
        assert_eq!(ui_navigation(&none), 0);
        let s = GamepadState {
            buttons: buttons::X
                | buttons::LEFT_SHOULDER
                | buttons::MENU
                | buttons::RIGHT_THUMBSTICK,
            left_x: 0.6,
            right_x: -0.6,
            left_trigger: 0.9,
            ..none
        };
        assert_eq!(
            ui_navigation(&s),
            ui::CONTEXT1
                | ui::PAGE_LEFT
                | ui::MENU
                | ui::CONTEXT4
                | ui::RIGHT
                | ui::SCROLL_LEFT
                | ui::PAGE_UP
        );
        let at_half = GamepadState {
            left_y: 0.5,
            right_trigger: 0.5,
            ..none
        };
        assert_eq!(ui_navigation(&at_half), 0);
    }

    #[test]
    fn default_bindings_press_the_documented_buttons() {
        let config = InputConfig::default();
        assert_eq!(pad(&config, &[vk::RETURN], 0).buttons, buttons::A);
        assert_eq!(pad(&config, &[vk::BACK], 0).buttons, buttons::B);
        assert_eq!(
            pad(&config, &[vk::Q, vk::E], 0).buttons,
            buttons::LEFT_SHOULDER | buttons::RIGHT_SHOULDER
        );
        assert_eq!(
            pad(&config, &[vk::ESCAPE, vk::TAB], 0).buttons,
            buttons::MENU | buttons::VIEW
        );
        assert_eq!(
            pad(&config, &[vk::UP, vk::LEFT], 0).buttons,
            buttons::DPAD_UP | buttons::DPAD_LEFT
        );
        let mouse = pad(
            &config,
            &[],
            MouseButton::Left.bit() | MouseButton::Right.bit(),
        );
        assert_eq!((mouse.right_trigger, mouse.left_trigger), (1.0, 1.0));
    }

    #[test]
    fn wasd_drives_the_left_stick_with_a_round_gate() {
        let config = InputConfig::default();
        let s = pad(&config, &[vk::W], 0);
        assert_eq!((s.left_x, s.left_y), (0.0, 1.0));
        let s = pad(&config, &[vk::S, vk::A], 0);
        let d = std::f32::consts::FRAC_1_SQRT_2;
        assert_eq!((s.left_x, s.left_y), (-d, -d));
        let s = pad(&config, &[vk::W, vk::S], 0);
        assert_eq!((s.left_x, s.left_y), (0.0, 0.0));
        let s = pad(&config, &[vk::W, vk::S, vk::D], 0);
        assert_eq!((s.left_x, s.left_y), (1.0, 0.0));
    }

    #[test]
    fn rebinding_changes_the_pad() {
        let config = InputConfig {
            bindings: vec![Binding {
                source: Source::Key(vk::SHIFT),
                control: Control::RightStick(Direction::Left),
            }],
            ..InputConfig::default()
        };
        let s = pad(&config, &[vk::SHIFT, vk::W], 0);
        assert_eq!((s.right_x, s.left_y, s.buttons), (-1.0, 0.0, 0));
    }

    #[test]
    fn the_mouse_stick_follows_the_documented_dynamics() {
        let mut stick = MouseStick::default();
        stick.step(1, 0, 1.0);
        assert_eq!(stick.x, 1024);
        stick.step(0, 0, 1.0);
        assert_eq!(stick.x, 896);
        stick.step(0, 5, 1.0);
        assert_eq!(stick.y, -5120);
        stick.step(1000, 0, 1.0);
        assert_eq!(stick.x, 32767);
        let mut slow = MouseStick::default();
        slow.step(2, 0, 0.5);
        assert_eq!(slow.x, 1024);
    }

    #[test]
    fn mouse_look_can_be_turned_off_and_adds_to_the_stick_keys() {
        let mut config = InputConfig::default();
        let stick = MouseStick { x: 16384, y: 0 };
        let s = virtual_pad(&config, &keys(&[]), 0, stick);
        assert!((s.right_x - 0.5).abs() < 1e-3);
        config.mouse_look = false;
        let s = virtual_pad(&config, &keys(&[]), 0, stick);
        assert_eq!(s.right_x, 0.0);
    }
}
