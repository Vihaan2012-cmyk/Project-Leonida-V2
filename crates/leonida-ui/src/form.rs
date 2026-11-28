//! The settings pages as data: which settings each page shows, how each is edited and when it applies.
//!
//! Nothing here knows about the window. A field's id is the name of its settings file's section and the setting's
//! path in that file, such as `graphics.resolution_scale` or `input.keyboard.jump`.

use config::input::Action;

pub const GAME: &str = "game";
pub const DISPLAY: &str = "display";
pub const PERFORMANCE: &str = "performance";
pub const WIDESCREEN: &str = "widescreen";
pub const CONTROLS: &str = "controls";
pub const CONTENT: &str = "content";
pub const MODS: &str = "mods";
pub const TRACE: &str = "trace";

pub const FRAME_LIMIT: &str = "graphics.frame_limit";
pub const VSYNC: &str = "graphics.vsync";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub value: String,
    pub label: String,
}

impl Choice {
    pub fn new(value: &str, label: &str) -> Self {
        Self {
            value: value.to_string(),
            label: label.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// A title over the fields that follow.
    Heading,
    /// A check box; `on` and `off` are the two values it stands for.
    Switch {
        on: String,
        off: String,
    },
    /// One of a fixed list.
    Choice(Vec<Choice>),
    /// Free text with a list of suggestions.
    Combo(Vec<Choice>),
    Text,
    Number,
    Folder,
    /// A file picked with a dialog that offers files ending in `extension`.
    File {
        extension: &'static str,
    },
    /// Names separated by commas.
    List,
}

impl Kind {
    pub fn flag() -> Kind {
        Kind::Switch {
            on: "true".to_string(),
            off: "false".to_string(),
        }
    }
}

/// A field is used only while another field has one of the given values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct When {
    pub field: String,
    pub any_of: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub id: String,
    pub label: String,
    pub help: String,
    pub kind: Kind,
    /// Clearing the text removes the setting instead of storing an empty value.
    pub optional: bool,
    pub when: Option<When>,
}

impl Field {
    pub fn new(id: &str, label: &str, help: &str, kind: Kind) -> Self {
        Self {
            id: id.to_string(),
            label: label.to_string(),
            help: help.to_string(),
            kind,
            optional: false,
            when: None,
        }
    }

    pub fn heading(title: &str) -> Self {
        Self::new("", title, "", Kind::Heading)
    }

    pub fn optional(mut self) -> Self {
        self.optional = true;
        self
    }

    pub fn only_when(mut self, field: &str, any_of: &[&str]) -> Self {
        self.when = Some(When {
            field: field.to_string(),
            any_of: any_of.iter().map(|value| value.to_string()).collect(),
        });
        self
    }

    pub fn is_input(&self) -> bool {
        self.kind != Kind::Heading
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub id: &'static str,
    pub title: &'static str,
    /// Fields in columns, each read top to bottom.
    pub columns: Vec<Vec<Field>>,
}

impl Page {
    pub fn fields(&self) -> impl Iterator<Item = &Field> {
        self.columns.iter().flatten()
    }
}

/// What the pages need to know about the PC.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Env {
    /// The name of every screen, in the order Windows lists them.
    pub monitors: Vec<String>,
}

fn choices(list: &[(&str, &str)]) -> Vec<Choice> {
    list.iter()
        .map(|(value, label)| Choice::new(value, label))
        .collect()
}

pub fn pages(env: &Env) -> Vec<Page> {
    vec![
        game(),
        display(env),
        performance(),
        widescreen(),
        controls(),
        content(),
        Page {
            id: MODS,
            title: "Mods",
            columns: vec![vec![Field::new(
                "mods.load_order",
                "Load order",
                "Packages that load, lowest priority first, separated by commas.",
                Kind::List,
            )]],
        },
        Page {
            id: TRACE,
            title: "Trace",
            columns: Vec::new(),
        },
    ]
}

fn game() -> Page {
    Page {
        id: GAME,
        title: "Game",
        columns: vec![vec![
            Field::new(
                "general.game_folder",
                "Game folder",
                "The folder with MicrosoftGame.Config. Left empty, Leonida looks for it.",
                Kind::Folder,
            )
            .optional(),
            Field::new(
                "general.output_folder",
                "Output folder",
                "Where Leonida keeps what it makes: shaders, caches, logs.",
                Kind::Folder,
            ),
            Field::new(
                "general.language",
                "Language",
                "A code such as en, fr or pt-BR.",
                Kind::Text,
            ),
            Field::new(
                "general.audio.output",
                "Sound",
                "Off mixes the game's sound but never plays it.",
                Kind::Choice(choices(&[("auto", "Default device"), ("off", "Off")])),
            ),
        ]],
    }
}

fn display(env: &Env) -> Page {
    let mut screens = vec![Choice::new("0", "Primary screen")];
    for (index, name) in env.monitors.iter().enumerate() {
        screens.push(Choice::new(
            &(index + 1).to_string(),
            &format!("Screen {}: {name}", index + 1),
        ));
    }
    Page {
        id: DISPLAY,
        title: "Display",
        columns: vec![
            vec![
                Field::heading("Window"),
                Field::new(
                    "graphics.window_mode",
                    "Window mode",
                    "Borderless covers the screen without a border.",
                    Kind::Choice(choices(&[
                        ("windowed", "Windowed"),
                        ("borderless", "Borderless fullscreen"),
                        ("fullscreen", "Fullscreen"),
                    ])),
                ),
                Field::new(
                    "graphics.window_width",
                    "Width",
                    "In pixels, 640 to 16384.",
                    Kind::Number,
                ),
                Field::new(
                    "graphics.window_height",
                    "Height",
                    "In pixels, 640 to 16384.",
                    Kind::Number,
                ),
                Field::new(
                    "display.monitor",
                    "Screen",
                    "Which screen the window opens on.",
                    Kind::Choice(screens),
                ),
                Field::new(
                    VSYNC,
                    "Vertical sync",
                    "One frame per screen refresh. The same as a frame limit of vsync.",
                    Kind::flag(),
                ),
            ],
            vec![
                Field::heading("High dynamic range"),
                Field::new(
                    "graphics.hdr",
                    "HDR",
                    "Needs a screen that supports it.",
                    Kind::flag(),
                ),
                Field::new(
                    "display.hdr_peak_nits",
                    "Peak brightness",
                    "In nits, 80 to 10000.",
                    Kind::Number,
                )
                .only_when("graphics.hdr", &["true"]),
                Field::new(
                    "display.hdr_sdr_white_nits",
                    "SDR white",
                    "In nits, 80 to 1000. Windows uses 203.",
                    Kind::Number,
                )
                .only_when("graphics.hdr", &["true"]),
                Field::heading("Textures"),
                Field::new(
                    "graphics.anisotropic_filtering",
                    "Anisotropic filtering",
                    "Sharpness of textures seen at a slant.",
                    Kind::Choice(choices(&[
                        ("1", "Off"),
                        ("2", "2x"),
                        ("4", "4x"),
                        ("8", "8x"),
                        ("16", "16x"),
                    ])),
                ),
                Field::new(
                    "graphics.texture_lod_bias",
                    "Texture sharpness bias",
                    "-4 sharper to 4 blurrier.",
                    Kind::Number,
                ),
            ],
        ],
    }
}

fn performance() -> Page {
    Page {
        id: PERFORMANCE,
        title: "Performance",
        columns: vec![vec![
            Field::heading("Frame rate"),
            Field::new(
                FRAME_LIMIT,
                "Frame limit",
                "uncapped, vsync, console or a number of frames a second.",
                Kind::Combo(choices(&[
                    ("uncapped", "uncapped"),
                    ("console", "console"),
                    ("vsync", "vsync"),
                    ("30", "30"),
                    ("60", "60"),
                    ("120", "120"),
                    ("144", "144"),
                ])),
            ),
            Field::new(
                "graphics.frame_generation",
                "Frame generation",
                "Makes extra frames between the game's own. Adds a little delay.",
                Kind::flag(),
            ),
            Field::heading("Upscaling"),
            Field::new(
                "graphics.upscaler",
                "Upscaler",
                "Brings a smaller picture up to the window's size.",
                Kind::Choice(choices(&[
                    ("off", "Off"),
                    ("fsr1", "FSR 1 (spatial)"),
                    ("temporal", "Temporal"),
                    ("dlss", "DLSS"),
                ])),
            ),
            Field::new(
                "graphics.resolution_scale",
                "Render scale",
                "0.25 to 2. Below 1 the upscaler fills the rest.",
                Kind::Number,
            ),
            Field::new(
                "upscaling.dlss_path",
                "DLSS library",
                "Your own copy of nvngx_dlss.dll.",
                Kind::File { extension: "dll" },
            )
            .optional()
            .only_when("graphics.upscaler", &["dlss"]),
        ]],
    }
}

fn widescreen() -> Page {
    Page {
        id: WIDESCREEN,
        title: "Widescreen",
        columns: vec![vec![
            Field::heading("Patches to the game's code"),
            Field::new(
                "widescreen.live_aspect",
                "Live aspect ratio",
                "Camera, HUD and effects follow the window's real shape.",
                Kind::flag(),
            ),
            Field::new(
                "widescreen.no_pillarbox",
                "No black bars",
                "The scene fills the window for shapes other than 16:9.",
                Kind::flag(),
            ),
            Field::heading("Picture"),
            Field::new(
                "graphics.aspect_mode",
                "Other shapes",
                "What happens when the window is not 16:9.",
                Kind::Choice(choices(&[
                    ("letterbox", "Black bars"),
                    ("native", "Fill the window"),
                    ("stretch", "Stretch"),
                ])),
            ),
            Field::new(
                "graphics.fov_offset",
                "Field of view offset",
                "Degrees added, -30 to 30.",
                Kind::Number,
            ),
            Field::new(
                "graphics.fov_scale",
                "Field of view scale",
                "A factor on the game's own, 0.5 to 1.8.",
                Kind::Number,
            ),
        ]],
    }
}

fn content() -> Page {
    Page {
        id: CONTENT,
        title: "Content",
        columns: vec![vec![Field::new(
            "general.content.nudity",
            "Hide nudity",
            "On by default. Only nudity is filtered.",
            Kind::Switch {
                on: "block".to_string(),
                off: "allow".to_string(),
            },
        )]],
    }
}

/// `move_forward` as `Move forward`.
pub fn humanize(name: &str) -> String {
    let mut text = name.replace('_', " ");
    if let Some(first) = text.get(..1) {
        text = first.to_uppercase() + &text[1..];
    }
    text
}

const LABELS: &[(&str, &str, &str)] = &[
    (
        "input.mouse_camera",
        "Turn the camera directly",
        "On: the mouse turns the camera through a code patch. Off: the mouse acts as the right stick.",
    ),
    (
        "input.mouse_sensitivity",
        "Sensitivity",
        "How fast the mouse turns the camera, 0.05 to 10.",
    ),
    ("input.mouse_invert_x", "Invert horizontal", ""),
    ("input.mouse_invert_y", "Invert vertical", ""),
    (
        "input.gamepad_look_sensitivity",
        "Look speed",
        "How fast the right stick turns the camera, 0.1 to 5.",
    ),
    ("input.gamepad_invert_y", "Invert vertical", ""),
    (
        "input.dead_zones.left_stick",
        "Left stick",
        "How far it moves before it counts, 0 to 0.9.",
    ),
    (
        "input.dead_zones.right_stick",
        "Right stick",
        "How far it moves before it counts, 0 to 0.9.",
    ),
    (
        "input.dead_zones.trigger",
        "Triggers",
        "How far they move before they count, 0 to 0.9.",
    ),
];

/// One field for a setting found in the file's own defaults, so a setting added to `input.toml` shows up here
/// without a change to this page.
fn generic(id: &str, key: &str, value: &toml::Value) -> Option<Field> {
    let (label, help) = LABELS
        .iter()
        .find(|(known, _, _)| *known == id)
        .map(|(_, label, help)| (label.to_string(), help.to_string()))
        .unwrap_or_else(|| (humanize(key), String::new()));
    let kind = match value {
        toml::Value::Boolean(_) => Kind::flag(),
        toml::Value::Integer(_) | toml::Value::Float(_) => Kind::Number,
        toml::Value::String(_) => Kind::Text,
        toml::Value::Array(items) if items.iter().all(toml::Value::is_str) => Kind::List,
        _ => return None,
    };
    Some(Field::new(id, &label, &help, kind))
}

fn controls() -> Page {
    controls_from(&toml::Table::try_from(config::Input::default()).unwrap_or_default())
}

fn controls_from(defaults: &toml::Table) -> Page {
    let mut mouse = vec![Field::heading("Mouse")];
    let mut pad = vec![Field::heading("Gamepad sticks")];
    let mut other = Vec::new();
    let mut keyboard = vec![Field::heading("Keyboard and mouse buttons")];
    let mut gamepad = vec![Field::heading("Gamepad buttons")];
    for (key, value) in defaults {
        let id = format!("input.{key}");
        match value {
            toml::Value::Table(table) if key == "keyboard" || key == "gamepad" => {
                let target = if key == "keyboard" {
                    &mut keyboard
                } else {
                    &mut gamepad
                };
                target.extend(bindings(&id, table));
            }
            toml::Value::Table(table) => {
                for (inner, value) in table {
                    let id = format!("{id}.{inner}");
                    pad.extend(generic(&id, inner, value));
                }
            }
            _ => {
                let target = if key.starts_with("mouse_") {
                    &mut mouse
                } else if key.starts_with("gamepad_") {
                    &mut pad
                } else {
                    &mut other
                };
                target.extend(generic(&id, key, value));
            }
        }
    }
    if !other.is_empty() {
        mouse.push(Field::heading("Other"));
        mouse.extend(other);
    }
    mouse.extend(pad);
    Page {
        id: CONTROLS,
        title: "Controls",
        columns: vec![mouse, keyboard, gamepad],
    }
}

/// The actions in the order the game lists them, then any the file has beyond those.
fn bindings(prefix: &str, table: &toml::Table) -> Vec<Field> {
    let mut names: Vec<&str> = Action::ALL.iter().map(|action| action.name()).collect();
    names.extend(
        table
            .keys()
            .map(String::as_str)
            .filter(|key| Action::from_name(key).is_none()),
    );
    names
        .into_iter()
        .filter(|name| table.contains_key(*name))
        .map(|name| Field::new(&format!("{prefix}.{name}"), &humanize(name), "", Kind::List))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_fields() -> Vec<Field> {
        pages(&Env::default())
            .iter()
            .flat_map(Page::fields)
            .cloned()
            .collect()
    }

    #[test]
    fn every_page_is_present_and_ids_are_unique() {
        let pages = pages(&Env::default());
        let ids: Vec<&str> = pages.iter().map(|page| page.id).collect();
        assert_eq!(
            ids,
            [
                GAME,
                DISPLAY,
                PERFORMANCE,
                WIDESCREEN,
                CONTROLS,
                CONTENT,
                MODS,
                TRACE
            ]
        );
        let mut seen = std::collections::BTreeSet::new();
        for field in all_fields().iter().filter(|field| field.is_input()) {
            assert!(seen.insert(field.id.clone()), "{} appears twice", field.id);
        }
    }

    #[test]
    fn every_action_has_a_keyboard_and_a_gamepad_field() {
        let fields = all_fields();
        for action in Action::ALL {
            for table in ["keyboard", "gamepad"] {
                let id = format!("input.{table}.{}", action.name());
                assert!(fields.iter().any(|field| field.id == id), "{id}");
            }
        }
    }

    #[test]
    fn controls_follow_what_the_input_file_holds() {
        let page = controls();
        let ids: Vec<&str> = page.fields().map(|field| field.id.as_str()).collect();
        for expected in [
            "input.mouse_camera",
            "input.mouse_sensitivity",
            "input.mouse_invert_y",
            "input.gamepad_look_sensitivity",
            "input.dead_zones.trigger",
        ] {
            assert!(ids.contains(&expected), "{expected}");
        }
        let sensitivity = page
            .fields()
            .find(|field| field.id == "input.mouse_sensitivity")
            .unwrap();
        assert_eq!(sensitivity.kind, Kind::Number);
        let camera = page
            .fields()
            .find(|field| field.id == "input.mouse_camera")
            .unwrap();
        assert_eq!(camera.kind, Kind::flag());
        assert_eq!(camera.label, "Turn the camera directly");
        assert!(
            page.columns[0].iter().position(|f| f.id == camera.id)
                < page.columns[0].iter().position(|f| f.id == sensitivity.id)
        );
        assert_eq!(humanize("take_cover"), "Take cover");
    }

    #[test]
    fn a_setting_added_to_the_input_file_shows_up_on_its_own() {
        let mut table = toml::Table::try_from(config::Input::default()).unwrap();
        table.insert("mouse_smoothing".into(), toml::Value::Boolean(true));
        table.insert("scroll_speed".into(), toml::Value::Float(1.5));
        table.insert("quick_turn".into(), toml::Value::Array(vec!["Q".into()]));
        let page = controls_from(&table);
        let kind = |id: &str| page.fields().find(|f| f.id == id).map(|f| f.kind.clone());
        assert_eq!(kind("input.mouse_smoothing"), Some(Kind::flag()));
        assert_eq!(kind("input.scroll_speed"), Some(Kind::Number));
        assert_eq!(kind("input.quick_turn"), Some(Kind::List));
        let first_column: Vec<&str> = page.columns[0].iter().map(|f| f.id.as_str()).collect();
        assert!(first_column.contains(&"input.mouse_smoothing"));
        let label = page
            .fields()
            .find(|f| f.id == "input.scroll_speed")
            .unwrap();
        assert_eq!(label.label, "Scroll speed");
    }
    #[test]
    fn monitors_become_choices_after_the_primary_screen() {
        let env = Env {
            monitors: vec!["DELL U2720Q".into(), "LG 27GP850".into()],
        };
        let page = display(&env);
        let screen = page
            .fields()
            .find(|field| field.id == "display.monitor")
            .unwrap();
        let Kind::Choice(list) = &screen.kind else {
            panic!("a list of screens");
        };
        assert_eq!(list.len(), 3);
        assert_eq!(list[0], Choice::new("0", "Primary screen"));
        assert_eq!(list[2].value, "2");
        assert!(list[2].label.contains("LG 27GP850"));
    }

    #[test]
    fn dependent_fields_name_the_field_they_wait_for() {
        let fields = all_fields();
        for field in fields.iter().filter(|field| field.when.is_some()) {
            let when = field.when.as_ref().unwrap();
            assert!(
                fields.iter().any(|other| other.id == when.field),
                "{} waits for {}",
                field.id,
                when.field
            );
        }
    }
}
