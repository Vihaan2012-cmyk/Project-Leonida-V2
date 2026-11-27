//! `input.toml` as the keyboard-and-mouse gamepad the game's input shim offers.

use config::Input;
use config::input::Action;
use shims::gameinput::abi::buttons;
use shims::gameinput::config::{
    Binding, Control, Direction, InputConfig, MouseButton, Source, default_bindings,
};
use shims::gameinput::set_config;

/// The Windows virtual-key code a key name in input.toml stands for. Shift, Ctrl and Alt are the generic keys, as
/// a window message reports them without the left and right split.
fn virtual_key(name: &str) -> Option<u8> {
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
        "WheelUp" | "WheelDown" => Err("the mouse wheel cannot press a gamepad control".into()),
        other => virtual_key(other)
            .map(Source::Key)
            .ok_or_else(|| format!("{other} is not a key Leonida can read")),
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

/// The input shim's settings from `input.toml`, and what could not be carried out.
///
/// Keys the file leaves unbound keep the shim's own menu bindings (Enter, Backspace, the arrows), so menus stay
/// usable. The mouse look sensitivity is carried over; inversion and dead zones have no place in the shim's
/// settings and are listed as not carried out when they differ from the defaults.
pub fn input_config(input: &Input) -> (InputConfig, Vec<String>) {
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
                    for control in &controls {
                        let binding = Binding {
                            source,
                            control: *control,
                        };
                        if !bindings.contains(&binding) {
                            bindings.push(binding);
                        }
                    }
                }
                Err(reason) => skipped.push(format!("{key} for {}: {reason}", action.name())),
            }
        }
    }
    let used: Vec<Source> = bindings.iter().map(|binding| binding.source).collect();
    bindings.extend(
        default_bindings()
            .into_iter()
            .filter(|binding| !used.contains(&binding.source)),
    );
    let defaults = Input::default();
    if input.mouse_invert_x || input.mouse_invert_y {
        skipped.push("mouse inversion: the input shim cannot invert the mouse".into());
    }
    if input.dead_zones != defaults.dead_zones
        || input.gamepad_invert_y
        || input.gamepad_look_sensitivity != defaults.gamepad_look_sensitivity
    {
        skipped.push(
            "gamepad dead zones, look sensitivity and inversion: the input shim leaves a real gamepad as it is"
                .into(),
        );
    }
    let config = InputConfig {
        bindings,
        mouse_sensitivity: input.mouse_sensitivity,
        ..InputConfig::default()
    };
    (config, skipped)
}

/// Gives the input shim the settings. The window that feeds `submit_input` is the presentation side's.
pub fn configure(config: &InputConfig) {
    set_config(config.clone());
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::Section;
    use shims::gameinput::config::vk;

    fn has(config: &InputConfig, source: Source, control: Control) -> bool {
        config
            .bindings
            .iter()
            .any(|binding| binding.source == source && binding.control == control)
    }

    #[test]
    fn the_default_settings_bind_every_action() {
        let (config, skipped) = input_config(&Input::default());
        assert!(skipped.is_empty(), "{skipped:?}");
        assert!(has(
            &config,
            Source::Key(vk::W),
            Control::LeftStick(Direction::Up)
        ));
        assert!(has(
            &config,
            Source::Key(vk::SPACE),
            Control::Button(buttons::A)
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
            Source::Key(0x1B),
            Control::Button(buttons::MENU)
        ));
        assert_eq!(config.mouse_sensitivity, 1.0);
    }

    #[test]
    fn menu_keys_the_file_does_not_use_keep_the_shims_bindings() {
        let (config, _) = input_config(&Input::default());
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
    fn a_key_the_file_binds_is_not_bound_twice() {
        let input = Input::parse("[keyboard]\njump = [\"Enter\"]\nreload = [\"X\"]\n").unwrap();
        let (config, _) = input_config(&input);
        assert!(has(
            &config,
            Source::Key(vk::RETURN),
            Control::Button(buttons::A)
        ));
        assert!(has(
            &config,
            Source::Key(vk::X),
            Control::Button(buttons::X)
        ));
        let on_enter = config
            .bindings
            .iter()
            .filter(|binding| binding.source == Source::Key(vk::RETURN))
            .count();
        assert_eq!(on_enter, 1);
        let on_x: Vec<_> = config
            .bindings
            .iter()
            .filter(|binding| binding.source == Source::Key(vk::X))
            .collect();
        assert_eq!(on_x.len(), 1);
    }

    #[test]
    fn what_cannot_be_carried_out_is_listed() {
        let input = Input::parse(
            "mouse_invert_y = true\nmouse_sensitivity = 2.5\n[keyboard]\nfire = [\"WheelUp\"]\nphone = [\"F5\"]\n[dead_zones]\nleft_stick = 0.3\n",
        )
        .unwrap();
        let (config, skipped) = input_config(&input);
        assert_eq!(config.mouse_sensitivity, 2.5);
        assert!(has(
            &config,
            Source::Key(0x74),
            Control::Button(buttons::DPAD_UP)
        ));
        let text = skipped.join("\n");
        assert!(text.contains("WheelUp for fire"), "{text}");
        assert!(text.contains("mouse inversion"), "{text}");
        assert!(text.contains("dead zones"), "{text}");
    }

    #[test]
    fn every_key_name_input_toml_accepts_is_a_virtual_key_or_a_mouse_button() {
        for name in config::input::KEYBOARD_KEYS {
            let known = source(name).is_ok() || name.starts_with("Wheel");
            assert!(known, "{name}");
        }
    }

    #[test]
    fn every_gamepad_button_name_input_toml_accepts_has_a_control() {
        for name in config::input::GAMEPAD_BUTTONS {
            assert!(button_control(name).is_some(), "{name}");
        }
    }
}
