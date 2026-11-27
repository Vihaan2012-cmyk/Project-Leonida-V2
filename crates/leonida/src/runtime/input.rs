//! `input.toml` as the keyboard-and-mouse gamepad the game's input shim offers.

use config::Input;
use shims::gameinput::InputConfig;
use shims::gameinput::set_config;

/// The input shim's settings from `input.toml`, and what could not be carried out.
pub fn input_config(input: &Input) -> (InputConfig, Vec<String>) {
    InputConfig::from_input(input)
}

/// Gives the input shim the settings. The window that feeds `submit_input` is the presentation side's.
pub fn configure(config: &InputConfig) {
    set_config(config.clone());
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::Section;
    use shims::gameinput::abi::buttons;
    use shims::gameinput::config::{Control, Direction, MouseButton, Source, WheelDir, vk};

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
        let input = Input::parse(
            "[keyboard]
jump = [\"Enter\"]
reload = [\"X\"]
",
        )
        .unwrap();
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
        let on_x = config
            .bindings
            .iter()
            .filter(|binding| binding.source == Source::Key(vk::X))
            .count();
        assert_eq!(on_x, 1);
    }

    #[test]
    fn mouse_look_settings_are_carried_out() {
        let input = Input::parse(
            "mouse_invert_y = true
mouse_sensitivity = 2.5
mouse_sensitivity_x = 1.5
mouse_smoothing = 0.4
",
        )
        .unwrap();
        let (config, skipped) = input_config(&input);
        assert_eq!(config.mouse_sensitivity, 2.5);
        assert_eq!(config.look.invert, [false, true]);
        assert_eq!(config.look.sensitivity[0], 1.5);
        assert!((config.look.smoothing - 0.4).abs() < 1e-6);
        let text = skipped.join(
            "
",
        );
        assert!(!text.contains("mouse"), "{text}");
    }

    #[test]
    fn only_gamepad_settings_are_listed_as_not_carried_out() {
        let input = Input::parse(
            "[keyboard]
fire = [\"WheelUp\"]
phone = [\"F5\"]
[dead_zones]
left_stick = 0.3
",
        )
        .unwrap();
        let (config, skipped) = input_config(&input);
        assert!(has(
            &config,
            Source::Key(0x74),
            Control::Button(buttons::DPAD_UP)
        ));
        assert!(has(
            &config,
            Source::Wheel(WheelDir::Up),
            Control::RightTrigger
        ));
        let text = skipped.join(
            "
",
        );
        assert!(!text.contains("WheelUp"), "{text}");
        assert!(text.contains("dead zones"), "{text}");
    }

    #[test]
    fn a_wheel_binding_reaches_the_shim() {
        let input = Input::parse(
            "[mouse_wheel]
up = \"jump\"
",
        )
        .unwrap();
        let (config, _) = input_config(&input);
        assert!(has(
            &config,
            Source::Wheel(WheelDir::Up),
            Control::Button(buttons::A)
        ));
    }

    #[test]
    fn every_key_name_input_toml_accepts_is_known_to_the_shim() {
        for name in config::input::KEYBOARD_KEYS {
            let known = shims::gameinput::settings::virtual_key(name).is_some()
                || name.starts_with("Mouse")
                || name.starts_with("Wheel");
            assert!(known, "{name}");
        }
    }
}
