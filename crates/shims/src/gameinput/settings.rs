//! `input.toml` as the keyboard-and-mouse gamepad's settings.

use config::Input;
use config::input::Action;

use super::abi::buttons;
use super::analog::Movement;
use super::config::{
    Binding, Control, Direction, InputConfig, MouseButton, Source, WheelDir, default_bindings,
};
use super::curve::LookSettings;

/// The Windows virtual-key code a key name in input.toml stands for. Shift, Ctrl and Alt are the generic keys, as a
/// window message reports them without the left and right split.
pub fn virtual_key(name: &str) -> Option<u8> {
    let mut chars = name.chars();
    let first = chars.next()?;
    if chars.next().is_none() && (first.is_ascii_uppercase() || first.is_ascii_digit()) {
        return Some(first as u8);
    }
    if let Some(number) = name.strip_prefix('F').and_then(|n| n.parse::<u8>().ok())
        && (1..=12).contains(&number)
    {
        return Some(0x6F + number);
    }
    Some(match name {
        "Space" => 0x20,
        "Enter" => 0x0D,
        "Escape" => 0x1B,
        "Tab" => 0x09,
        "Backspace" => 0x08,
        "LeftShift" | "RightShift" => 0x10,
        "LeftCtrl" | "RightCtrl" => 0x11,
        "LeftAlt" | "RightAlt" => 0x12,
        "CapsLock" => 0x14,
        "Up" => 0x26,
        "Down" => 0x28,
        "Left" => 0x25,
        "Right" => 0x27,
        "Insert" => 0x2D,
        "Delete" => 0x2E,
        "Home" => 0x24,
        "End" => 0x23,
        "PageUp" => 0x21,
        "PageDown" => 0x22,
        "Minus" => 0xBD,
        "Equals" => 0xBB,
        "LeftBracket" => 0xDB,
        "RightBracket" => 0xDD,
        "Semicolon" => 0xBA,
        "Quote" => 0xDE,
        "Comma" => 0xBC,
        "Period" => 0xBE,
        "Slash" => 0xBF,
        "Backslash" => 0xDC,
        "Grave" => 0xC0,
        _ => return None,
    })
}

fn source(name: &str) -> Result<Source, String> {
    match name {
        "Mouse1" => Ok(Source::Mouse(MouseButton::Left)),
        "Mouse2" => Ok(Source::Mouse(MouseButton::Right)),
        "Mouse3" => Ok(Source::Mouse(MouseButton::Middle)),
        "Mouse4" => Ok(Source::Mouse(MouseButton::X1)),
        "Mouse5" => Ok(Source::Mouse(MouseButton::X2)),
        "WheelUp" => Ok(Source::Wheel(WheelDir::Up)),
        "WheelDown" => Ok(Source::Wheel(WheelDir::Down)),
        other => virtual_key(other)
            .map(Source::Key)
            .ok_or_else(|| format!("{other} is not a key the input shim can read")),
    }
}

fn button_control(name: &str) -> Option<Control> {
    Some(match name {
        "A" => Control::Button(buttons::A),
        "B" => Control::Button(buttons::B),
        "X" => Control::Button(buttons::X),
        "Y" => Control::Button(buttons::Y),
        "LeftBumper" => Control::Button(buttons::LEFT_SHOULDER),
        "RightBumper" => Control::Button(buttons::RIGHT_SHOULDER),
        "LeftStick" => Control::Button(buttons::LEFT_THUMBSTICK),
        "RightStick" => Control::Button(buttons::RIGHT_THUMBSTICK),
        "LeftTrigger" => Control::LeftTrigger,
        "RightTrigger" => Control::RightTrigger,
        "View" => Control::Button(buttons::VIEW),
        "Menu" => Control::Button(buttons::MENU),
        "DPadUp" => Control::Button(buttons::DPAD_UP),
        "DPadDown" => Control::Button(buttons::DPAD_DOWN),
        "DPadLeft" => Control::Button(buttons::DPAD_LEFT),
        "DPadRight" => Control::Button(buttons::DPAD_RIGHT),
        _ => return None,
    })
}

/// What an action does on the pad: the walking actions push the left stick, and every other action presses what
/// the gamepad table binds it to.
fn controls(input: &Input, action: Action) -> Vec<Control> {
    let stick = |direction| vec![Control::LeftStick(direction)];
    match action {
        Action::MoveForward => stick(Direction::Up),
        Action::MoveBack => stick(Direction::Down),
        Action::MoveLeft => stick(Direction::Left),
        Action::MoveRight => stick(Direction::Right),
        _ => input
            .buttons(action)
            .iter()
            .filter_map(|name| button_control(name))
            .collect(),
    }
}

fn add(bindings: &mut Vec<Binding>, source: Source, controls: &[Control]) {
    for &control in controls {
        let binding = Binding { source, control };
        if !bindings.contains(&binding) {
            bindings.push(binding);
        }
    }
}

fn key_codes(names: &[String], skipped: &mut Vec<String>, what: &str) -> Vec<u8> {
    names
        .iter()
        .filter_map(|name| match virtual_key(name) {
            Some(code) => Some(code),
            None => {
                skipped.push(format!(
                    "{name} for {what}: not a key the input shim can read"
                ));
                None
            }
        })
        .collect()
}

impl InputConfig {
    /// The input shim's settings from `input.toml`, and what could not be carried out.
    ///
    /// Keys the file leaves unbound keep the shim's own menu bindings (Enter, Backspace, the arrows), so menus stay
    /// usable. The gamepad dead zones, look sensitivity and inversion of the file apply to a real gamepad, which
    /// the game reads untouched, and are listed as not carried out when they differ from the defaults.
    pub fn from_input(input: &Input) -> (InputConfig, Vec<String>) {
        let mut skipped = Vec::new();
        let mut bindings: Vec<Binding> = Vec::new();
        for action in Action::ALL {
            let controls = controls(input, action);
            for key in input.keys(action) {
                match source(key) {
                    Ok(source) => {
                        if controls.is_empty() {
                            skipped.push(format!(
                                "{key} for {}: the action has no gamepad control",
                                action.name()
                            ));
                        }
                        add(&mut bindings, source, &controls);
                    }
                    Err(reason) => skipped.push(format!("{key} for {}: {reason}", action.name())),
                }
            }
        }
        for (direction, name) in [
            (WheelDir::Up, &input.mouse_wheel.up),
            (WheelDir::Down, &input.mouse_wheel.down),
        ] {
            if let Some(action) = Action::from_name(name) {
                add(
                    &mut bindings,
                    Source::Wheel(direction),
                    &controls(input, action),
                );
            }
        }
        let used: Vec<Source> = bindings.iter().map(|binding| binding.source).collect();
        bindings.extend(
            default_bindings()
                .into_iter()
                .filter(|binding| !used.contains(&binding.source)),
        );
        let defaults = Input::default();
        if input.dead_zones != defaults.dead_zones
            || input.gamepad_invert_y
            || input.gamepad_look_sensitivity != defaults.gamepad_look_sensitivity
        {
            skipped.push(
                "gamepad dead zones, look sensitivity and inversion: the input shim leaves a real gamepad as it is"
                    .into(),
            );
        }
        let m = &input.movement;
        let config = InputConfig {
            bindings,
            mouse_sensitivity: input.mouse_sensitivity,
            look: LookSettings {
                sensitivity: [input.mouse_sensitivity_x, input.mouse_sensitivity_y],
                invert: [input.mouse_invert_x, input.mouse_invert_y],
                smoothing: input.mouse_smoothing,
                ..LookSettings::default()
            },
            movement: Movement {
                level: m.level,
                walk_level: m.walk_level,
                run_level: m.run_level,
                ramp_up_ms: m.ramp_up_ms,
                ramp_down_ms: m.ramp_down_ms,
                walk_keys: key_codes(&m.walk_keys, &mut skipped, "movement.walk_keys"),
                run_keys: key_codes(&m.run_keys, &mut skipped, "movement.run_keys"),
            },
            ..InputConfig::default()
        };
        (config, skipped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gameinput::config::vk;
    use config::Section;

    fn has(config: &InputConfig, source: Source, control: Control) -> bool {
        config
            .bindings
            .iter()
            .any(|b| b.source == source && b.control == control)
    }

    #[test]
    fn the_default_file_gives_the_pc_layout() {
        let (config, skipped) = InputConfig::from_input(&Input::default());
        assert!(skipped.is_empty(), "{skipped:?}");
        let up = Control::LeftStick(Direction::Up);
        assert!(has(&config, Source::Key(vk::W), up));
        assert!(has(
            &config,
            Source::Key(vk::SPACE),
            Control::Button(buttons::A)
        ));
        assert!(has(&config, Source::Key(b'F'), Control::Button(buttons::Y)));
        assert!(has(
            &config,
            Source::Key(vk::TAB),
            Control::Button(buttons::LEFT_SHOULDER)
        ));
        assert!(has(
            &config,
            Source::Key(vk::ESCAPE),
            Control::Button(buttons::MENU)
        ));
        assert!(has(
            &config,
            Source::Mouse(MouseButton::Left),
            Control::RightTrigger
        ));
        assert!(has(
            &config,
            Source::Mouse(MouseButton::Right),
            Control::LeftTrigger
        ));
        assert!(has(
            &config,
            Source::Wheel(WheelDir::Down),
            Control::Button(buttons::DPAD_RIGHT)
        ));
        assert!(has(
            &config,
            Source::Wheel(WheelDir::Up),
            Control::Button(buttons::DPAD_LEFT)
        ));
        assert_eq!(config.movement.walk_keys, [0x12]);
        assert_eq!(config.movement.run_keys, [0x10]);
    }

    #[test]
    fn remapping_in_the_file_changes_the_bindings_and_the_look() {
        let input = Input::parse(
            "mouse_sensitivity = 2.0\nmouse_sensitivity_x = 1.5\nmouse_sensitivity_y = 0.5\nmouse_invert_y = true\n\
mouse_smoothing = 0.25\n[movement]\nwalk_level = 0.3\nwalk_keys = [\"CapsLock\"]\n\
[mouse_wheel]\nup = \"\"\ndown = \"jump\"\n[keyboard]\nfire = [\"Mouse4\", \"WheelUp\"]\nreload = [\"G\"]\n",
        )
        .unwrap();
        let (config, skipped) = InputConfig::from_input(&input);
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(config.mouse_sensitivity, 2.0);
        assert_eq!(config.look.sensitivity, [1.5, 0.5]);
        assert_eq!(config.look.invert, [false, true]);
        assert_eq!(config.look.smoothing, 0.25);
        assert_eq!(config.movement.walk_level, 0.3);
        assert_eq!(config.movement.walk_keys, [0x14]);
        assert!(has(
            &config,
            Source::Mouse(MouseButton::X1),
            Control::RightTrigger
        ));
        assert!(has(
            &config,
            Source::Wheel(WheelDir::Up),
            Control::RightTrigger
        ));
        assert!(has(&config, Source::Key(b'G'), Control::Button(buttons::X)));
        assert!(has(
            &config,
            Source::Wheel(WheelDir::Down),
            Control::Button(buttons::A)
        ));
        assert!(!has(
            &config,
            Source::Key(b'R'),
            Control::Button(buttons::X)
        ));
    }

    #[test]
    fn menu_keys_the_file_does_not_use_keep_the_shims_bindings() {
        let (config, _) = InputConfig::from_input(&Input::default());
        assert!(has(
            &config,
            Source::Key(vk::RETURN),
            Control::Button(buttons::A)
        ));
        assert!(has(
            &config,
            Source::Key(vk::BACK),
            Control::Button(buttons::B)
        ));
        assert!(has(
            &config,
            Source::Key(vk::DOWN),
            Control::Button(buttons::DPAD_DOWN)
        ));
    }

    #[test]
    fn every_key_name_input_toml_accepts_has_a_source() {
        for name in config::input::KEYBOARD_KEYS {
            assert!(source(name).is_ok(), "{name}");
        }
        for name in config::input::GAMEPAD_BUTTONS {
            assert!(button_control(name).is_some(), "{name}");
        }
    }

    #[test]
    fn what_cannot_be_carried_out_is_listed() {
        let input =
            Input::parse("[keyboard]\nweapon_next = [\"F5\"]\n[dead_zones]\nleft_stick = 0.3\n")
                .unwrap();
        let (_, skipped) = InputConfig::from_input(&input);
        assert!(skipped.join("\n").contains("dead zones"), "{skipped:?}");
    }
}
