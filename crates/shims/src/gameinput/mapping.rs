//! Tables and pure functions between devices and GameInput readings.

use gilrs::{Axis, Button, Gamepad};

use super::abi::{GamepadState, buttons, ui};
use super::config::{Binding, Control, Direction, InputConfig, Source, WheelDir};

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

/// Where the movement keys alone push the left stick: the keys bound to it, at the level the walk and run keys
/// choose, on the round gate.
pub fn move_target(config: &InputConfig, keys: &[bool; 256]) -> [f32; 2] {
    let mut left = [0f32; 2];
    for &Binding { source, control } in &config.bindings {
        if let (Source::Key(code), Control::LeftStick(dir)) = (source, control)
            && keys[code as usize]
        {
            push(&mut left, dir);
        }
    }
    let level = config.movement.level_for(keys);
    gate(left).map(|v| v * level)
}

/// The gamepad state the keyboard and mouse make under `config`. `left` is the ramped movement stick and `look` the
/// mouse's right stick; the buttons, triggers and right-stick keys come from the bindings.
pub fn virtual_pad(
    config: &InputConfig,
    keys: &[bool; 256],
    mouse_buttons: u32,
    wheel: [bool; 2],
    left: [f32; 2],
    look: [f32; 2],
) -> GamepadState {
    let mut state = GamepadState::default();
    let mut right = [0f32; 2];
    for &Binding { source, control } in &config.bindings {
        let down = match source {
            Source::Key(code) => keys[code as usize],
            Source::Mouse(button) => mouse_buttons & button.bit() != 0,
            Source::Wheel(WheelDir::Up) => wheel[0],
            Source::Wheel(WheelDir::Down) => wheel[1],
        };
        if !down {
            continue;
        }
        match control {
            Control::Button(bit) => state.buttons |= bit,
            Control::LeftTrigger => state.left_trigger = 1.0,
            Control::RightTrigger => state.right_trigger = 1.0,
            Control::LeftStick(_) => {}
            Control::RightStick(dir) => push(&mut right, dir),
        }
    }
    let mut right = gate(right);
    if config.mouse_look {
        right[0] += look[0];
        right[1] += look[1];
    }
    state.left_x = left[0].clamp(-1.0, 1.0);
    state.left_y = left[1].clamp(-1.0, 1.0);
    state.right_x = right[0].clamp(-1.0, 1.0);
    state.right_y = right[1].clamp(-1.0, 1.0);
    state
}

/// Which device the player touched last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LastUsed {
    #[default]
    None,
    Pad,
    KeyboardMouse,
}

fn pick_axis(kbm: [f32; 2], pad: [f32; 2], last: LastUsed) -> [f32; 2] {
    let live = |v: [f32; 2]| v[0].hypot(v[1]) > 1e-3;
    match (live(kbm), live(pad)) {
        (true, true) if last == LastUsed::Pad => pad,
        (true, true) => kbm,
        (true, false) => kbm,
        (false, _) => pad,
    }
}

/// Folds controllers and the keyboard and mouse into one gamepad: buttons add up, and a stick or trigger both push
/// takes the value of the device used last.
pub fn merge(kbm: &GamepadState, pads: &[GamepadState], last: LastUsed) -> GamepadState {
    let mut pad = GamepadState::default();
    for p in pads {
        pad.buttons |= p.buttons;
        pad.left_trigger = pad.left_trigger.max(p.left_trigger);
        pad.right_trigger = pad.right_trigger.max(p.right_trigger);
        let longer = |a: [f32; 2], b: [f32; 2]| {
            if b[0].hypot(b[1]) > a[0].hypot(a[1]) {
                b
            } else {
                a
            }
        };
        let l = longer([pad.left_x, pad.left_y], [p.left_x, p.left_y]);
        let r = longer([pad.right_x, pad.right_y], [p.right_x, p.right_y]);
        (pad.left_x, pad.left_y) = (l[0], l[1]);
        (pad.right_x, pad.right_y) = (r[0], r[1]);
    }
    let l = pick_axis([kbm.left_x, kbm.left_y], [pad.left_x, pad.left_y], last);
    let r = pick_axis([kbm.right_x, kbm.right_y], [pad.right_x, pad.right_y], last);
    let trigger = |k: f32, p: f32| {
        if k > 0.0 && p > 0.0 && last == LastUsed::Pad {
            p
        } else if k > 0.0 {
            k
        } else {
            p
        }
    };
    GamepadState {
        buttons: kbm.buttons | pad.buttons,
        left_trigger: trigger(kbm.left_trigger, pad.left_trigger),
        right_trigger: trigger(kbm.right_trigger, pad.right_trigger),
        left_x: l[0],
        left_y: l[1],
        right_x: r[0],
        right_y: r[1],
    }
}

/// Whether a controller state is a deliberate act and not a resting stick: a button, a trigger pulled a fair way,
/// or a stick well out of the dead zone.
pub fn is_activity(state: &GamepadState) -> bool {
    state.buttons != 0
        || state.left_trigger > 0.3
        || state.right_trigger > 0.3
        || state.left_x.hypot(state.left_y) > 0.5
        || state.right_x.hypot(state.right_y) > 0.5
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
        let k = keys(down);
        let left = move_target(config, &k);
        virtual_pad(config, &k, mouse, [false; 2], left, [0.0; 2])
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
    fn the_mouse_look_adds_to_the_right_stick_and_can_be_turned_off() {
        let mut config = InputConfig::default();
        let k = keys(&[]);
        let s = virtual_pad(&config, &k, 0, [false; 2], [0.0; 2], [0.5, -0.25]);
        assert_eq!((s.right_x, s.right_y), (0.5, -0.25));
        config.mouse_look = false;
        let s = virtual_pad(&config, &k, 0, [false; 2], [0.0; 2], [0.5, -0.25]);
        assert_eq!((s.right_x, s.right_y), (0.0, 0.0));
    }

    #[test]
    fn the_walk_key_scales_the_movement_target() {
        let config = InputConfig::default();
        let s = pad(&config, &[vk::W, vk::MENU], 0);
        assert!((s.left_y - 0.4).abs() < 1e-6);
        let s = pad(&config, &[vk::W, vk::A, vk::MENU], 0);
        assert!((s.left_x + 0.4 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
    }

    #[test]
    fn the_wheel_presses_what_it_is_bound_to() {
        let config = InputConfig {
            bindings: vec![Binding {
                source: Source::Wheel(WheelDir::Down),
                control: Control::Button(buttons::DPAD_RIGHT),
            }],
            ..InputConfig::default()
        };
        let k = keys(&[]);
        let s = virtual_pad(&config, &k, 0, [false, true], [0.0; 2], [0.0; 2]);
        assert_eq!(s.buttons, buttons::DPAD_RIGHT);
        let s = virtual_pad(&config, &k, 0, [true, false], [0.0; 2], [0.0; 2]);
        assert_eq!(s.buttons, 0);
    }

    fn analog(lx: f32, rx: f32, lt: f32, buttons: u32) -> GamepadState {
        GamepadState {
            left_x: lx,
            right_x: rx,
            left_trigger: lt,
            buttons,
            ..GamepadState::default()
        }
    }

    #[test]
    fn merging_adds_buttons_and_lets_the_last_used_device_win_a_contested_stick() {
        let kbm = analog(1.0, 0.0, 1.0, buttons::A);
        let pad = analog(-0.5, 0.3, 0.5, buttons::B);
        let m = merge(&kbm, &[pad], LastUsed::KeyboardMouse);
        assert_eq!(m.buttons, buttons::A | buttons::B);
        assert_eq!((m.left_x, m.right_x, m.left_trigger), (1.0, 0.3, 1.0));
        let m = merge(&kbm, &[pad], LastUsed::Pad);
        assert_eq!((m.left_x, m.right_x, m.left_trigger), (-0.5, 0.3, 0.5));
    }

    #[test]
    fn merging_with_an_idle_device_passes_the_other_through() {
        let idle = GamepadState::default();
        let pad = analog(0.6, 0.0, 0.0, 0);
        assert_eq!(merge(&idle, &[pad], LastUsed::KeyboardMouse).left_x, 0.6);
        assert_eq!(merge(&pad, &[idle], LastUsed::Pad).left_x, 0.6);
        assert_eq!(merge(&idle, &[], LastUsed::None), idle);
    }

    #[test]
    fn two_controllers_fold_by_the_longer_stick() {
        let a = analog(0.2, 0.0, 0.0, 0);
        let b = analog(-0.7, 0.0, 0.0, 0);
        let m = merge(&GamepadState::default(), &[a, b], LastUsed::None);
        assert_eq!(m.left_x, -0.7);
    }

    #[test]
    fn a_resting_stick_is_not_activity_and_a_button_is() {
        assert!(!is_activity(&analog(0.1, 0.0, 0.1, 0)));
        assert!(is_activity(&analog(0.0, 0.0, 0.0, buttons::A)));
        assert!(is_activity(&analog(0.9, 0.0, 0.0, 0)));
    }
}
