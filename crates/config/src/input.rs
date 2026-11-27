//! `input.toml`: key and button bindings, mouse and stick sensitivity, inversion and dead zones.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};

use crate::{Problems, Section};

/// A thing the player can bind a key or button to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Action {
    MoveForward,
    MoveBack,
    MoveLeft,
    MoveRight,
    Jump,
    Sprint,
    Crouch,
    Interact,
    Fire,
    Aim,
    Reload,
    Melee,
    TakeCover,
    WeaponWheel,
    Phone,
    Pause,
    EnterVehicle,
    CameraView,
    WeaponNext,
    WeaponPrevious,
}

impl Action {
    pub const ALL: [Action; 20] = [
        Action::MoveForward,
        Action::MoveBack,
        Action::MoveLeft,
        Action::MoveRight,
        Action::Jump,
        Action::Sprint,
        Action::Crouch,
        Action::Interact,
        Action::Fire,
        Action::Aim,
        Action::Reload,
        Action::Melee,
        Action::TakeCover,
        Action::WeaponWheel,
        Action::Phone,
        Action::Pause,
        Action::EnterVehicle,
        Action::CameraView,
        Action::WeaponNext,
        Action::WeaponPrevious,
    ];

    /// The name used in `input.toml`.
    pub fn name(self) -> &'static str {
        match self {
            Action::MoveForward => "move_forward",
            Action::MoveBack => "move_back",
            Action::MoveLeft => "move_left",
            Action::MoveRight => "move_right",
            Action::Jump => "jump",
            Action::Sprint => "sprint",
            Action::Crouch => "crouch",
            Action::Interact => "interact",
            Action::Fire => "fire",
            Action::Aim => "aim",
            Action::Reload => "reload",
            Action::Melee => "melee",
            Action::TakeCover => "take_cover",
            Action::WeaponWheel => "weapon_wheel",
            Action::Phone => "phone",
            Action::Pause => "pause",
            Action::EnterVehicle => "enter_vehicle",
            Action::CameraView => "camera_view",
            Action::WeaponNext => "weapon_next",
            Action::WeaponPrevious => "weapon_previous",
        }
    }

    pub fn from_name(name: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|action| action.name() == name)
    }

    fn defaults(self) -> (&'static [&'static str], &'static [&'static str]) {
        match self {
            Action::MoveForward => (&["W"], &[]),
            Action::MoveBack => (&["S"], &[]),
            Action::MoveLeft => (&["A"], &[]),
            Action::MoveRight => (&["D"], &[]),
            Action::Jump => (&["Space"], &["A"]),
            Action::Sprint => (&["LeftShift"], &["LeftStick"]),
            Action::Crouch => (&["LeftCtrl"], &["RightStick"]),
            Action::Interact => (&["E"], &["Y"]),
            Action::Fire => (&["Mouse1"], &["RightTrigger"]),
            Action::Aim => (&["Mouse2"], &["LeftTrigger"]),
            Action::Reload => (&["R"], &["X"]),
            Action::Melee => (&["V"], &["B"]),
            Action::TakeCover => (&["Q"], &["RightBumper"]),
            Action::WeaponWheel => (&["Tab"], &["LeftBumper"]),
            Action::Phone => (&["Up"], &["DPadUp"]),
            Action::Pause => (&["Escape"], &["Menu"]),
            Action::EnterVehicle => (&["F"], &["Y"]),
            Action::CameraView => (&["C"], &["View"]),
            Action::WeaponNext => (&[], &["DPadRight"]),
            Action::WeaponPrevious => (&[], &["DPadLeft"]),
        }
    }
}

/// Every key and mouse button name a keyboard binding may use.
pub const KEYBOARD_KEYS: &[&str] = &[
    "A",
    "B",
    "C",
    "D",
    "E",
    "F",
    "G",
    "H",
    "I",
    "J",
    "K",
    "L",
    "M",
    "N",
    "O",
    "P",
    "Q",
    "R",
    "S",
    "T",
    "U",
    "V",
    "W",
    "X",
    "Y",
    "Z",
    "0",
    "1",
    "2",
    "3",
    "4",
    "5",
    "6",
    "7",
    "8",
    "9",
    "F1",
    "F2",
    "F3",
    "F4",
    "F5",
    "F6",
    "F7",
    "F8",
    "F9",
    "F10",
    "F11",
    "F12",
    "Space",
    "Enter",
    "Escape",
    "Tab",
    "Backspace",
    "LeftShift",
    "RightShift",
    "LeftCtrl",
    "RightCtrl",
    "LeftAlt",
    "RightAlt",
    "CapsLock",
    "Up",
    "Down",
    "Left",
    "Right",
    "Insert",
    "Delete",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "Minus",
    "Equals",
    "LeftBracket",
    "RightBracket",
    "Semicolon",
    "Quote",
    "Comma",
    "Period",
    "Slash",
    "Backslash",
    "Grave",
    "Mouse1",
    "Mouse2",
    "Mouse3",
    "Mouse4",
    "Mouse5",
    "WheelUp",
    "WheelDown",
];

/// Every gamepad button name a gamepad binding may use.
pub const GAMEPAD_BUTTONS: &[&str] = &[
    "A",
    "B",
    "X",
    "Y",
    "LeftBumper",
    "RightBumper",
    "LeftTrigger",
    "RightTrigger",
    "LeftStick",
    "RightStick",
    "View",
    "Menu",
    "DPadUp",
    "DPadDown",
    "DPadLeft",
    "DPadRight",
];

type Bindings = BTreeMap<String, Vec<String>>;

fn default_bindings(keyboard: bool) -> Bindings {
    Action::ALL
        .into_iter()
        .map(|action| {
            let (keys, buttons) = action.defaults();
            let chosen = if keyboard { keys } else { buttons };
            (
                action.name().to_string(),
                chosen.iter().map(|name| name.to_string()).collect(),
            )
        })
        .collect()
}

/// Bindings given in the file replace the default for that action; actions left out keep their default.
fn merged<'de, D: Deserializer<'de>>(
    deserializer: D,
    keyboard: bool,
) -> Result<Bindings, D::Error> {
    let mut bindings = default_bindings(keyboard);
    bindings.extend(Bindings::deserialize(deserializer)?);
    Ok(bindings)
}

fn merged_keyboard<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Bindings, D::Error> {
    merged(deserializer, true)
}

fn merged_gamepad<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Bindings, D::Error> {
    merged(deserializer, false)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DeadZones {
    pub left_stick: f32,
    pub right_stick: f32,
    pub trigger: f32,
}

impl Default for DeadZones {
    fn default() -> Self {
        Self {
            left_stick: 0.15,
            right_stick: 0.15,
            trigger: 0.05,
        }
    }
}

/// How the keyboard stands in for the left stick.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Movement {
    /// Stick deflection of the movement keys alone, 0.1 to 1.0.
    pub level: f32,
    /// Stick deflection while a walk key is held.
    pub walk_level: f32,
    /// Stick deflection while a run key is held.
    pub run_level: f32,
    /// Milliseconds the stick takes to go from rest to full, 0 for no ramp.
    pub ramp_up_ms: u32,
    /// Milliseconds the stick takes to fall back to rest, 0 for no ramp.
    pub ramp_down_ms: u32,
    pub walk_keys: Vec<String>,
    pub run_keys: Vec<String>,
}

impl Default for Movement {
    fn default() -> Self {
        Self {
            level: 1.0,
            walk_level: 0.4,
            run_level: 1.0,
            ramp_up_ms: 80,
            ramp_down_ms: 50,
            walk_keys: vec!["LeftAlt".into()],
            run_keys: vec!["LeftShift".into()],
        }
    }
}

/// What a turn of the mouse wheel does, as an action name; empty leaves the direction unbound.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MouseWheel {
    pub up: String,
    pub down: String,
}

impl Default for MouseWheel {
    fn default() -> Self {
        Self {
            up: Action::WeaponPrevious.name().into(),
            down: Action::WeaponNext.name().into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Input {
    pub mouse_sensitivity: f32,
    /// Multiplies `mouse_sensitivity` for sideways movement.
    pub mouse_sensitivity_x: f32,
    /// Multiplies `mouse_sensitivity` for up and down movement.
    pub mouse_sensitivity_y: f32,
    /// 0 turns the camera by exactly what the mouse moved; up to 1 averages over about a tenth of a second.
    pub mouse_smoothing: f32,
    pub mouse_invert_x: bool,
    pub mouse_invert_y: bool,
    pub gamepad_look_sensitivity: f32,
    pub gamepad_invert_y: bool,
    pub dead_zones: DeadZones,
    pub movement: Movement,
    pub mouse_wheel: MouseWheel,
    #[serde(deserialize_with = "merged_keyboard")]
    pub keyboard: Bindings,
    #[serde(deserialize_with = "merged_gamepad")]
    pub gamepad: Bindings,
}

impl Default for Input {
    fn default() -> Self {
        Self {
            mouse_sensitivity: 1.0,
            mouse_sensitivity_x: 1.0,
            mouse_sensitivity_y: 1.0,
            mouse_smoothing: 0.0,
            mouse_invert_x: false,
            mouse_invert_y: false,
            gamepad_look_sensitivity: 1.0,
            gamepad_invert_y: false,
            dead_zones: DeadZones::default(),
            movement: Movement::default(),
            mouse_wheel: MouseWheel::default(),
            keyboard: default_bindings(true),
            gamepad: default_bindings(false),
        }
    }
}

impl Input {
    /// Keys and mouse buttons bound to `action`, as spelled in the file.
    pub fn keys(&self, action: Action) -> &[String] {
        self.keyboard.get(action.name()).map_or(&[], Vec::as_slice)
    }

    /// Gamepad buttons bound to `action`.
    pub fn buttons(&self, action: Action) -> &[String] {
        self.gamepad.get(action.name()).map_or(&[], Vec::as_slice)
    }
}

fn check_bindings(problems: &mut Problems, table: &str, bindings: &Bindings, valid: &[&str]) {
    for (action, names) in bindings {
        let field = format!("{table}.{action}");
        if Action::from_name(action).is_none() {
            let known: Vec<&str> = Action::ALL.iter().map(|a| a.name()).collect();
            problems.add(
                &field,
                format!("unknown action, the actions are {}", known.join(", ")),
            );
            continue;
        }
        for (index, name) in names.iter().enumerate() {
            if !valid.contains(&name.as_str()) {
                problems.add(
                    &field,
                    format!(
                        "entry {} \"{name}\" is not a known {}; use one of {}",
                        index + 1,
                        if table == "keyboard" { "key" } else { "button" },
                        valid.join(", ")
                    ),
                );
            } else if names[..index].contains(name) {
                problems.add(&field, format!("\"{name}\" is listed twice"));
            }
        }
    }
}

fn section(title: &str, bindings: &Bindings) -> String {
    let mut text = format!("[{title}]\n");
    for action in Action::ALL {
        let names: Vec<String> = bindings[action.name()]
            .iter()
            .map(|name| format!("\"{name}\""))
            .collect();
        text.push_str(&format!("{} = [{}]\n", action.name(), names.join(", ")));
    }
    text
}

impl Section for Input {
    const FILE: &'static str = "input.toml";
    fn default_text() -> String {
        documented_defaults()
    }

    fn check(&self, problems: &mut Problems) {
        problems.range_f32("mouse_sensitivity", self.mouse_sensitivity, 0.05, 10.0);
        problems.range_f32("mouse_sensitivity_x", self.mouse_sensitivity_x, 0.05, 10.0);
        problems.range_f32("mouse_sensitivity_y", self.mouse_sensitivity_y, 0.05, 10.0);
        problems.range_f32("mouse_smoothing", self.mouse_smoothing, 0.0, 1.0);
        problems.range_f32("movement.level", self.movement.level, 0.1, 1.0);
        problems.range_f32("movement.walk_level", self.movement.walk_level, 0.1, 1.0);
        problems.range_f32("movement.run_level", self.movement.run_level, 0.1, 1.0);
        problems.range_u32("movement.ramp_up_ms", self.movement.ramp_up_ms, 0, 1000);
        problems.range_u32("movement.ramp_down_ms", self.movement.ramp_down_ms, 0, 1000);
        for (field, names) in [
            ("movement.walk_keys", &self.movement.walk_keys),
            ("movement.run_keys", &self.movement.run_keys),
        ] {
            for name in names {
                if !KEYBOARD_KEYS.contains(&name.as_str()) {
                    problems.add(field, format!("\"{name}\" is not a known key"));
                }
            }
        }
        for (field, name) in [
            ("mouse_wheel.up", &self.mouse_wheel.up),
            ("mouse_wheel.down", &self.mouse_wheel.down),
        ] {
            if !name.is_empty() && Action::from_name(name).is_none() {
                problems.add(field, format!("\"{name}\" is not an action"));
            }
        }
        problems.range_f32(
            "gamepad_look_sensitivity",
            self.gamepad_look_sensitivity,
            0.1,
            5.0,
        );
        problems.range_f32(
            "dead_zones.left_stick",
            self.dead_zones.left_stick,
            0.0,
            0.9,
        );
        problems.range_f32(
            "dead_zones.right_stick",
            self.dead_zones.right_stick,
            0.0,
            0.9,
        );
        problems.range_f32("dead_zones.trigger", self.dead_zones.trigger, 0.0, 0.9);
        check_bindings(problems, "keyboard", &self.keyboard, KEYBOARD_KEYS);
        check_bindings(problems, "gamepad", &self.gamepad, GAMEPAD_BUTTONS);
    }
}

/// The documented default file, with the bindings listed from `Action::ALL`.
fn documented_defaults() -> String {
    let defaults = Input::default();
    format!(
        "\
# Leonida input settings. Delete this file (or run `leonida config reset`) to get it back with the defaults.

# How fast the mouse turns the camera. 0.05 to 10.0, 1.0 is the game's own speed. The two axis values
# multiply it for sideways and up-and-down movement.
mouse_sensitivity = 1.0
mouse_sensitivity_x = 1.0
mouse_sensitivity_y = 1.0
# 0.0 turns the camera by exactly what the mouse moved; up to 1.0 averages over about a tenth of a second.
mouse_smoothing = 0.0
mouse_invert_x = false
mouse_invert_y = false

# How fast the right stick turns the camera. 0.1 to 5.0.
gamepad_look_sensitivity = 1.0
gamepad_invert_y = false

# How far a stick or trigger has to move before it counts, as a share of its travel. 0.0 to 0.9.
[dead_zones]
left_stick = 0.15
right_stick = 0.15
trigger = 0.05

# The keyboard standing in for the left stick. A level is how far the stick is pushed (0.1 to 1.0); the ramps
# are the milliseconds it takes to get there and back (0 for none); walk and run keys switch the level.
[movement]
level = 1.0
walk_level = 0.4
run_level = 1.0
ramp_up_ms = 80
ramp_down_ms = 50
walk_keys = [\"LeftAlt\"]
run_keys = [\"LeftShift\"]

# What turning the mouse wheel does, as an action name (\"\" for nothing).
[mouse_wheel]
up = \"weapon_previous\"
down = \"weapon_next\"

# Each action lists the keys or buttons that trigger it; an empty list leaves it unbound, and an action left
# out keeps its default. Moving and looking with the gamepad are on its sticks and cannot be rebound.
# Keys: A-Z, 0-9, F1-F12, Space, Enter, Escape, Tab, Backspace, LeftShift, RightShift, LeftCtrl, RightCtrl,
# LeftAlt, RightAlt, CapsLock, Up, Down, Left, Right, Insert, Delete, Home, End, PageUp, PageDown, Minus,
# Equals, LeftBracket, RightBracket, Semicolon, Quote, Comma, Period, Slash, Backslash, Grave, and for the
# mouse Mouse1 to Mouse5, WheelUp and WheelDown.
{keyboard}
# Gamepad buttons: A, B, X, Y, LeftBumper, RightBumper, LeftTrigger, RightTrigger, LeftStick, RightStick
# (pressing the stick in), View, Menu, DPadUp, DPadDown, DPadLeft, DPadRight.
{gamepad}",
        keyboard = section("keyboard", &defaults.keyboard),
        gamepad = section("gamepad", &defaults.gamepad),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_documented_default_text_is_the_default() {
        assert_eq!(
            Input::parse(&documented_defaults()).unwrap(),
            Input::default()
        );
    }

    #[test]
    fn every_default_binding_is_a_known_name() {
        let input = Input::default();
        for action in Action::ALL {
            assert!(input.keyboard.contains_key(action.name()));
            assert!(input.gamepad.contains_key(action.name()));
        }
        assert_eq!(input.keys(Action::Jump), ["Space"]);
        assert_eq!(input.buttons(Action::Fire), ["RightTrigger"]);
        assert!(input.buttons(Action::MoveForward).is_empty());
    }

    #[test]
    fn a_partial_table_keeps_the_other_defaults() {
        let input = Input::parse("[keyboard]\njump = [\"J\", \"Mouse4\"]\nsprint = []\n").unwrap();
        assert_eq!(input.keys(Action::Jump), ["J", "Mouse4"]);
        assert!(input.keys(Action::Sprint).is_empty());
        assert_eq!(input.keys(Action::Fire), ["Mouse1"]);
        assert_eq!(input.buttons(Action::Jump), ["A"]);
    }

    #[test]
    fn bad_bindings_and_ranges_are_reported() {
        let text = "\
mouse_sensitivity = 0\n[dead_zones]\ntrigger = 1.5\n[keyboard]\njump = [\"Spacebar\", \"E\", \"E\"]\nteleport = [\"T\"]\n\
[gamepad]\nfire = [\"Z\"]\n";
        let error = Input::parse(text).unwrap_err().to_string();
        assert!(
            error.contains("mouse_sensitivity: must be between 0.05 and 10, got 0"),
            "{error}"
        );
        assert!(
            error.contains("dead_zones.trigger: must be between 0 and 0.9, got 1.5"),
            "{error}"
        );
        assert!(
            error.contains("keyboard.jump: entry 1 \"Spacebar\" is not a known key"),
            "{error}"
        );
        assert!(
            error.contains("keyboard.jump: \"E\" is listed twice"),
            "{error}"
        );
        assert!(
            error.contains("keyboard.teleport: unknown action"),
            "{error}"
        );
        assert!(
            error.contains("gamepad.fire: entry 1 \"Z\" is not a known button"),
            "{error}"
        );
    }
}

#[cfg(test)]
mod keyboard_mouse_tests {
    use super::*;

    #[test]
    fn the_default_layout_is_a_pc_open_world_one() {
        let input = Input::default();
        for (action, key) in [
            (Action::Jump, "Space"),
            (Action::Sprint, "LeftShift"),
            (Action::Interact, "E"),
            (Action::EnterVehicle, "F"),
            (Action::WeaponWheel, "Tab"),
            (Action::Fire, "Mouse1"),
            (Action::Aim, "Mouse2"),
            (Action::Pause, "Escape"),
        ] {
            assert_eq!(input.keys(action), [key], "{}", action.name());
        }
        assert_eq!(input.mouse_wheel.up, "weapon_previous");
        assert_eq!(input.mouse_wheel.down, "weapon_next");
        assert_eq!(input.movement.walk_keys, ["LeftAlt"]);
    }

    #[test]
    fn mouse_and_movement_settings_remap_from_toml() {
        let input = Input::parse(
            "mouse_sensitivity_x = 2.0\nmouse_sensitivity_y = 0.5\nmouse_smoothing = 0.3\n\
[movement]\nwalk_level = 0.25\nwalk_keys = [\"CapsLock\"]\nramp_up_ms = 0\n\
[mouse_wheel]\nup = \"\"\ndown = \"jump\"\n[keyboard]\nfire = [\"Mouse4\"]\n",
        )
        .unwrap();
        assert_eq!(input.mouse_sensitivity_x, 2.0);
        assert_eq!(input.mouse_sensitivity_y, 0.5);
        assert_eq!(input.mouse_smoothing, 0.3);
        assert_eq!(input.movement.walk_level, 0.25);
        assert_eq!(input.movement.walk_keys, ["CapsLock"]);
        assert_eq!(input.movement.run_keys, ["LeftShift"]);
        assert_eq!(input.movement.ramp_up_ms, 0);
        assert_eq!(input.mouse_wheel.up, "");
        assert_eq!(input.mouse_wheel.down, "jump");
        assert_eq!(input.keys(Action::Fire), ["Mouse4"]);
    }

    #[test]
    fn bad_movement_and_wheel_settings_are_reported() {
        let text = "mouse_smoothing = 2.0\n[movement]\nlevel = 0.0\nrun_keys = [\"Hyper\"]\n\
[mouse_wheel]\nup = \"fly\"\n";
        let error = Input::parse(text).unwrap_err().to_string();
        assert!(
            error.contains("mouse_smoothing: must be between"),
            "{error}"
        );
        assert!(error.contains("movement.level: must be between"), "{error}");
        assert!(error.contains("movement.run_keys: \"Hyper\""), "{error}");
        assert!(
            error.contains("mouse_wheel.up: \"fly\" is not an action"),
            "{error}"
        );
    }
}
