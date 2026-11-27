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
}

impl Action {
    pub const ALL: [Action; 16] = [
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
        }
    }
}

const KEYBOARD_KEYS: &[&str] = &[
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

const GAMEPAD_BUTTONS: &[&str] = &[
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Input {
    pub mouse_sensitivity: f32,
    pub mouse_invert_x: bool,
    pub mouse_invert_y: bool,
    pub gamepad_look_sensitivity: f32,
    pub gamepad_invert_y: bool,
    pub dead_zones: DeadZones,
    #[serde(deserialize_with = "merged_keyboard")]
    pub keyboard: Bindings,
    #[serde(deserialize_with = "merged_gamepad")]
    pub gamepad: Bindings,
}

impl Default for Input {
    fn default() -> Self {
        Self {
            mouse_sensitivity: 1.0,
            mouse_invert_x: false,
            mouse_invert_y: false,
            gamepad_look_sensitivity: 1.0,
            gamepad_invert_y: false,
            dead_zones: DeadZones::default(),
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

# How fast the mouse turns the camera. 0.05 to 10.0, 1.0 is the game's own speed.
mouse_sensitivity = 1.0
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
