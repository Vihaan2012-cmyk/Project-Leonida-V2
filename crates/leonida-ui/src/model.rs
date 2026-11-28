//! The settings as the window edits them: one piece of text per field, converted to and from the settings files.
//!
//! The window reads and writes text; this module turns it into values in `config`, checks it with the same rules
//! the game uses and reports each problem against the field it belongs to.

use std::collections::BTreeMap;
use std::path::Path;

use config::{Config, ConfigError, Section};
use toml::{Table, Value};

use crate::form::{FRAME_LIMIT, Field, Kind, Page, VSYNC};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Stops the settings from being saved.
    Error,
    /// Worth knowing; the settings are still saved.
    Note,
    /// Information shown beside the field, in the usual hint colour.
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldError {
    /// The field it belongs to; `None` for a problem with a whole file.
    pub id: Option<String>,
    pub message: String,
    pub severity: Severity,
}

impl FieldError {
    pub fn error(id: &str, message: impl Into<String>) -> Self {
        Self {
            id: Some(id.to_string()),
            message: message.into(),
            severity: Severity::Error,
        }
    }

    pub fn note(id: &str, message: impl Into<String>) -> Self {
        Self {
            id: Some(id.to_string()),
            message: message.into(),
            severity: Severity::Note,
        }
    }

    pub fn info(id: &str, message: impl Into<String>) -> Self {
        Self {
            id: Some(id.to_string()),
            message: message.into(),
            severity: Severity::Info,
        }
    }
}

/// What a frame limit says, in the words `xbox_d3d12::pacing::FrameLimit` reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    Console,
    Uncapped,
    Vsync,
    Cap(u32),
}

pub fn parse_limit(text: &str) -> Result<Limit, String> {
    let trimmed = text.trim();
    match trimmed.to_ascii_lowercase().as_str() {
        "console" | "default" => Ok(Limit::Console),
        "uncapped" | "unlimited" | "off" | "0" => Ok(Limit::Uncapped),
        "vsync" => Ok(Limit::Vsync),
        other => match other.parse::<f64>() {
            Ok(fps) if fps.is_finite() && fps > 0.0 => {
                if fps.fract() == 0.0 && fps <= f64::from(u32::MAX) {
                    Ok(Limit::Cap(fps as u32))
                } else {
                    Err("a frame limit in frames a second is a whole number".to_string())
                }
            }
            _ => Err(format!(
                "frame limit must be console, uncapped, vsync or a number of frames per second above 0: {trimmed:?}"
            )),
        },
    }
}

fn limit_text(graphics: &Table) -> String {
    let mode = graphics
        .get("frame_rate_mode")
        .and_then(Value::as_str)
        .unwrap_or("console");
    match mode {
        "cap" => graphics
            .get("frame_rate_cap")
            .and_then(Value::as_integer)
            .map_or_else(|| "console".to_string(), |cap| cap.to_string()),
        other => other.to_string(),
    }
}

fn apply_limit(graphics: &mut Table, text: &str) -> Result<(), String> {
    let (mode, cap) = match parse_limit(text)? {
        Limit::Console => ("console", None),
        Limit::Uncapped => ("uncapped", None),
        Limit::Vsync => ("vsync", None),
        Limit::Cap(fps) => ("cap", Some(fps)),
    };
    graphics.insert("frame_rate_mode".into(), Value::String(mode.into()));
    if let Some(fps) = cap {
        graphics.insert("frame_rate_cap".into(), Value::Integer(i64::from(fps)));
    }
    Ok(())
}

/// The settings files as tables, so a field can be changed by its path without a function for every setting.
#[derive(Default)]
struct Sections {
    general: Table,
    graphics: Table,
    input: Table,
    mods: Table,
    display: Table,
    widescreen: Table,
    upscaling: Table,
}

fn table_of<T: serde::Serialize>(section: &T) -> Table {
    Table::try_from(section).unwrap_or_default()
}

impl Sections {
    fn of(config: &Config) -> Self {
        Self {
            general: table_of(&config.general),
            graphics: table_of(&config.graphics),
            input: table_of(&config.input),
            mods: table_of(&config.mods),
            display: table_of(&config.display),
            widescreen: table_of(&config.widescreen),
            upscaling: table_of(&config.upscaling),
        }
    }

    fn table(&self, prefix: &str) -> Option<&Table> {
        Some(match prefix {
            "general" => &self.general,
            "graphics" => &self.graphics,
            "input" => &self.input,
            "mods" => &self.mods,
            "display" => &self.display,
            "widescreen" => &self.widescreen,
            "upscaling" => &self.upscaling,
            _ => return None,
        })
    }

    fn table_mut(&mut self, prefix: &str) -> Option<&mut Table> {
        Some(match prefix {
            "general" => &mut self.general,
            "graphics" => &mut self.graphics,
            "input" => &mut self.input,
            "mods" => &mut self.mods,
            "display" => &mut self.display,
            "widescreen" => &mut self.widescreen,
            "upscaling" => &mut self.upscaling,
            _ => return None,
        })
    }

    fn into_config(self) -> Result<Config, Vec<FieldError>> {
        let mut errors = Vec::new();
        let config = Config {
            general: read_section("general", self.general, &mut errors),
            graphics: read_section("graphics", self.graphics, &mut errors),
            input: read_section("input", self.input, &mut errors),
            mods: read_section("mods", self.mods, &mut errors),
            display: read_section("display", self.display, &mut errors),
            widescreen: read_section("widescreen", self.widescreen, &mut errors),
            upscaling: read_section("upscaling", self.upscaling, &mut errors),
        };
        if errors.is_empty() {
            Ok(config)
        } else {
            Err(errors)
        }
    }
}

fn read_section<T: Section>(prefix: &str, table: Table, errors: &mut Vec<FieldError>) -> T {
    let value: T = match table.try_into() {
        Ok(value) => value,
        Err(error) => {
            errors.push(FieldError {
                id: None,
                message: format!("{}: {error}", T::FILE),
                severity: Severity::Error,
            });
            return T::default();
        }
    };
    if let Err(ConfigError::Invalid { problems, .. }) = value.validate() {
        for problem in problems {
            errors.push(FieldError::error(
                &route(prefix, &problem.field),
                shorten(&problem.message),
            ));
        }
    }
    value
}

/// The field a problem in `field` of a file is shown against.
fn route(prefix: &str, field: &str) -> String {
    match (prefix, field) {
        ("graphics", "frame_rate_cap" | "frame_rate_mode") => FRAME_LIMIT.to_string(),
        _ => format!("{prefix}.{field}"),
    }
}

/// The part of a problem that fits beside a field; the whole text goes to the log.
pub fn shorten(message: &str) -> String {
    let message = message.split("; use one of").next().unwrap_or(message);
    let message = message.split(", the actions are").next().unwrap_or(message);
    match message
        .strip_prefix("entry ")
        .and_then(|rest| rest.split_once(' '))
    {
        Some((_, rest)) => rest.to_string(),
        None => message.to_string(),
    }
}

fn split(id: &str) -> Option<(&str, Vec<&str>)> {
    let mut parts = id.split('.');
    let prefix = parts.next()?;
    let path: Vec<&str> = parts.collect();
    (!path.is_empty()).then_some((prefix, path))
}

fn lookup<'a>(table: &'a Table, path: &[&str]) -> Option<&'a Value> {
    let (last, parents) = path.split_last()?;
    let mut at = table;
    for part in parents {
        at = at.get(*part)?.as_table()?;
    }
    at.get(*last)
}

fn text_of(value: &Value) -> String {
    match value {
        Value::Boolean(flag) => flag.to_string(),
        Value::Integer(number) => number.to_string(),
        Value::Float(number) => (*number as f32).to_string(),
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map_or_else(|| item.to_string(), str::to_string)
            })
            .collect::<Vec<_>>()
            .join(", "),
        other => other.to_string(),
    }
}

fn parse_value(field: &Field, existing: Option<&Value>, text: &str) -> Result<Value, String> {
    let text = text.trim();
    if field.kind == Kind::List {
        let names = text
            .split([',', ';'])
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(|name| Value::String(name.to_string()))
            .collect();
        return Ok(Value::Array(names));
    }
    let whole = || {
        text.parse::<i64>()
            .map(Value::Integer)
            .map_err(|_| "enter a whole number".to_string())
    };
    let float = || match text.parse::<f32>() {
        Ok(number) if number.is_finite() => Ok(Value::Float(f64::from(number))),
        _ => Err("enter a number".to_string()),
    };
    let flag = || match text {
        "true" => Ok(Value::Boolean(true)),
        "false" => Ok(Value::Boolean(false)),
        _ => Err("choose on or off".to_string()),
    };
    match existing {
        Some(Value::Integer(_)) => whole(),
        Some(Value::Float(_)) => float(),
        Some(Value::Boolean(_)) => flag(),
        Some(Value::String(_)) => Ok(Value::String(text.to_string())),
        _ => match &field.kind {
            Kind::Number => whole().or_else(|_| float()),
            Kind::Switch { on, .. } if on == "true" => flag(),
            _ => Ok(Value::String(text.to_string())),
        },
    }
}

fn apply(sections: &mut Sections, field: &Field, text: &str) -> Result<(), String> {
    let Some((prefix, path)) = split(&field.id) else {
        return Ok(());
    };
    let Some(root) = sections.table_mut(prefix) else {
        return Ok(());
    };
    if field.id == FRAME_LIMIT {
        return apply_limit(root, text);
    }
    let Some((last, parents)) = path.split_last() else {
        return Ok(());
    };
    let mut at = root;
    for part in parents {
        let entry = at
            .entry(part.to_string())
            .or_insert_with(|| Value::Table(Table::new()));
        match entry {
            Value::Table(inner) => at = inner,
            _ => return Err(format!("{} is not a table", field.id)),
        }
    }
    let text = match field.kind {
        Kind::Folder | Kind::File { .. } | Kind::Text => text.trim(),
        _ => text,
    };
    if text.is_empty() && field.optional {
        at.remove(*last);
        return Ok(());
    }
    let value = parse_value(field, at.get(*last), text)?;
    at.insert(last.to_string(), value);
    Ok(())
}

/// Every field's text, and what it takes to turn that text into settings.
#[derive(Debug, Clone)]
pub struct Form {
    base: Config,
    fields: Vec<Field>,
    values: BTreeMap<String, String>,
    saved: BTreeMap<String, String>,
}

impl Form {
    pub fn new(config: &Config, pages: &[Page]) -> Self {
        let fields = pages
            .iter()
            .flat_map(Page::fields)
            .filter(|field| field.is_input())
            .cloned()
            .collect();
        let mut form = Self {
            base: config.clone(),
            fields,
            values: BTreeMap::new(),
            saved: BTreeMap::new(),
        };
        form.reload(config);
        form
    }

    /// Takes `config` as what is on disk and shows it in every field.
    pub fn reload(&mut self, config: &Config) {
        let sections = Sections::of(config);
        let mut values = BTreeMap::new();
        for field in &self.fields {
            let text = match field.id.as_str() {
                FRAME_LIMIT => limit_text(&sections.graphics),
                VSYNC => (limit_text(&sections.graphics) == "vsync").to_string(),
                id => split(id)
                    .and_then(|(prefix, path)| lookup(sections.table(prefix)?, &path).map(text_of))
                    .unwrap_or_default(),
            };
            values.insert(field.id.clone(), text);
        }
        self.base = config.clone();
        self.saved = values.clone();
        self.values = values;
    }

    pub fn config(&self) -> &Config {
        &self.base
    }

    pub fn value(&self, id: &str) -> &str {
        self.values.get(id).map_or("", String::as_str)
    }

    pub fn is_dirty(&self) -> bool {
        self.values != self.saved
    }

    pub fn is_enabled(&self, field: &Field) -> bool {
        field
            .when
            .as_ref()
            .is_none_or(|when| when.any_of.iter().any(|v| v == self.value(&when.field)))
    }

    /// Stores what the field now says. Returns the other fields that changed with it: the frame limit and vertical
    /// sync are one setting seen two ways.
    pub fn set(&mut self, id: &str, text: &str) -> Vec<String> {
        self.values.insert(id.to_string(), text.to_string());
        let mut changed = Vec::new();
        let mut put = |form: &mut Form, id: &str, text: &str| {
            if form.value(id) != text {
                form.values.insert(id.to_string(), text.to_string());
                changed.push(id.to_string());
            }
        };
        if id == FRAME_LIMIT {
            let on = matches!(parse_limit(text), Ok(Limit::Vsync));
            put(self, VSYNC, if on { "true" } else { "false" });
        } else if id == VSYNC {
            if text == "true" {
                put(self, FRAME_LIMIT, "vsync");
            } else if matches!(parse_limit(self.value(FRAME_LIMIT)), Ok(Limit::Vsync)) {
                put(self, FRAME_LIMIT, "console");
            }
        }
        changed
    }

    /// The settings these fields describe, or every problem that stops them being settings.
    pub fn build(&self) -> Result<Config, Vec<FieldError>> {
        let mut sections = Sections::of(&self.base);
        let mut errors = Vec::new();
        for field in self.fields.iter().filter(|field| field.id != VSYNC) {
            if let Err(message) = apply(&mut sections, field, self.value(&field.id)) {
                errors.push(FieldError::error(&field.id, message));
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        sections.into_config()
    }
}

/// Writes the files of `new` that differ from `old` into `dir`, keeping their comments. Returns their names.
pub fn save_changed(
    dir: &Path,
    old: &Config,
    new: &Config,
) -> Result<Vec<&'static str>, ConfigError> {
    let mut written = Vec::new();
    macro_rules! each {
        ($($part:ident: $ty:ty),*) => {$(
            if old.$part != new.$part {
                new.$part.save(dir)?;
                written.push(<$ty>::FILE);
            }
        )*};
    }
    each!(
        general: config::General,
        graphics: config::Graphics,
        input: config::Input,
        mods: config::Mods,
        display: config::DisplayConfig,
        widescreen: config::Widescreen,
        upscaling: config::Upscaling
    );
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::form::{Env, pages};
    use std::path::PathBuf;

    fn form() -> Form {
        Form::new(&Config::default(), &pages(&Env::default()))
    }

    fn ids_of(errors: &[FieldError]) -> Vec<&str> {
        errors.iter().filter_map(|e| e.id.as_deref()).collect()
    }

    #[test]
    fn the_defaults_go_through_the_form_unchanged() {
        assert_eq!(form().build().unwrap(), Config::default());
    }

    #[test]
    fn a_changed_config_goes_through_the_form_unchanged() {
        let mut config = Config::default();
        config.general.game_folder = Some(PathBuf::from("E:/Games/GTA6"));
        config.general.language = "pt-BR".into();
        config.general.content.nudity = config::Nudity::Allow;
        config.graphics.window_width = 2560;
        config.graphics.resolution_scale = 0.67;
        config.graphics.upscaler = config::Upscaler::Dlss;
        config.graphics.frame_rate_mode = config::FrameRateMode::Cap;
        config.graphics.frame_rate_cap = 90;
        config.graphics.fov_scale = 1.2;
        config.display.monitor = 2;
        config.upscaling.dlss_path = Some(PathBuf::from("C:/x/nvngx_dlss.dll"));
        config.widescreen.live_aspect = true;
        config.input.mouse_sensitivity = 2.5;
        config
            .input
            .keyboard
            .insert("jump".into(), vec!["J".into(), "Mouse4".into()]);
        config.input.dead_zones.trigger = 0.2;
        let form = Form::new(&config, &pages(&Env::default()));
        assert_eq!(form.value("graphics.resolution_scale"), "0.67");
        assert_eq!(form.value("input.keyboard.jump"), "J, Mouse4");
        assert_eq!(form.value("general.content.nudity"), "allow");
        assert_eq!(form.value(FRAME_LIMIT), "90");
        assert_eq!(form.build().unwrap(), config);
    }

    #[test]
    fn edits_land_in_the_right_settings() {
        let mut form = form();
        form.set("graphics.resolution_scale", "0.5");
        form.set("graphics.hdr", "true");
        form.set("display.hdr_peak_nits", "1400");
        form.set("widescreen.no_pillarbox", "true");
        form.set("general.content.nudity", "allow");
        form.set("graphics.window_mode", "borderless");
        form.set("input.keyboard.jump", " J ; K ,");
        form.set("upscaling.dlss_path", "D:/dlss/nvngx_dlss.dll");
        let config = form.build().unwrap();
        assert_eq!(config.graphics.resolution_scale, 0.5);
        assert!(config.graphics.hdr);
        assert_eq!(config.display.hdr_peak_nits, 1400);
        assert!(config.widescreen.no_pillarbox && !config.widescreen.live_aspect);
        assert_eq!(config.general.content.nudity, config::Nudity::Allow);
        assert_eq!(config.graphics.window_mode, config::WindowMode::Borderless);
        assert_eq!(config.input.keyboard["jump"], ["J", "K"]);
        assert_eq!(
            config.upscaling.dlss_path,
            Some(PathBuf::from("D:/dlss/nvngx_dlss.dll"))
        );
        assert!(form.is_dirty());
    }

    #[test]
    fn clearing_an_optional_field_unsets_the_setting() {
        let mut config = Config::default();
        config.general.game_folder = Some(PathBuf::from("E:/Games/GTA6"));
        let mut form = Form::new(&config, &pages(&Env::default()));
        assert!(!form.is_dirty());
        form.set("general.game_folder", "  ");
        assert_eq!(form.build().unwrap().general.game_folder, None);
        assert!(form.is_dirty());
    }

    #[test]
    fn text_that_is_not_a_number_is_reported_on_its_field() {
        let mut form = form();
        form.set("graphics.resolution_scale", "big");
        form.set("graphics.window_width", "1920.5");
        form.set("display.monitor", "");
        let errors = form.build().unwrap_err();
        let mut ids = ids_of(&errors);
        ids.sort();
        assert_eq!(
            ids,
            [
                "display.monitor",
                "graphics.resolution_scale",
                "graphics.window_width"
            ]
        );
        let message = |id: &str| {
            errors
                .iter()
                .find(|e| e.id.as_deref() == Some(id))
                .unwrap()
                .message
                .clone()
        };
        assert_eq!(message("graphics.resolution_scale"), "enter a number");
        assert_eq!(message("graphics.window_width"), "enter a whole number");
        assert_eq!(message("display.monitor"), "enter a whole number");
    }

    #[test]
    fn range_problems_come_back_next_to_their_field() {
        let mut form = form();
        form.set("graphics.resolution_scale", "9");
        form.set("graphics.fov_scale", "3");
        form.set("input.mouse_sensitivity", "0");
        form.set("display.hdr_peak_nits", "10");
        form.set("general.language", "english");
        let errors = form.build().unwrap_err();
        let by_id: BTreeMap<_, _> = errors
            .iter()
            .map(|e| (e.id.clone().unwrap(), e.message.clone()))
            .collect();
        assert_eq!(
            by_id["graphics.resolution_scale"],
            "must be between 0.25 and 2, got 9"
        );
        assert_eq!(
            by_id["graphics.fov_scale"],
            "must be between 0.5 and 1.8, got 3"
        );
        assert!(by_id["input.mouse_sensitivity"].starts_with("must be between 0.05 and 10"));
        assert!(by_id["display.hdr_peak_nits"].contains("between 80 and 10000"));
        assert!(by_id["general.language"].starts_with("must be a language code"));
    }

    #[test]
    fn a_wrong_key_name_is_short_beside_the_field() {
        let mut form = form();
        form.set("input.keyboard.jump", "Spacebar, E, E");
        let errors = form.build().unwrap_err();
        let messages: Vec<&str> = errors
            .iter()
            .filter(|e| e.id.as_deref() == Some("input.keyboard.jump"))
            .map(|e| e.message.as_str())
            .collect();
        assert_eq!(
            messages,
            ["\"Spacebar\" is not a known key", "\"E\" is listed twice"]
        );
    }

    #[test]
    fn every_problem_in_every_file_is_reported_together() {
        let mut form = form();
        form.set("graphics.resolution_scale", "9");
        form.set("display.hdr_peak_nits", "10");
        form.set("input.gamepad.fire", "Z");
        assert_eq!(form.build().unwrap_err().len(), 4);
    }

    #[test]
    fn the_frame_limit_takes_the_words_the_runtime_takes() {
        for (text, expected) in [
            ("console", Limit::Console),
            ("Default", Limit::Console),
            ("uncapped", Limit::Uncapped),
            ("unlimited", Limit::Uncapped),
            ("off", Limit::Uncapped),
            ("0", Limit::Uncapped),
            ("VSYNC", Limit::Vsync),
            (" 90 ", Limit::Cap(90)),
        ] {
            assert_eq!(parse_limit(text), Ok(expected), "{text}");
        }
        for bad in ["", "fast", "-30", "nan", "inf", "59.94"] {
            assert!(parse_limit(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_frame_limit_sets_the_mode_and_the_cap() {
        let mut form = form();
        form.set(FRAME_LIMIT, "120");
        let graphics = form.build().unwrap().graphics;
        assert_eq!(graphics.frame_rate(), config::FrameRate::Cap(120));
        form.set(FRAME_LIMIT, "uncapped");
        assert_eq!(
            form.build().unwrap().graphics.frame_rate(),
            config::FrameRate::Uncapped
        );
        form.set(FRAME_LIMIT, "5");
        let errors = form.build().unwrap_err();
        assert_eq!(ids_of(&errors), [FRAME_LIMIT]);
        assert!(errors[0].message.contains("between 15 and 1000"));
        form.set(FRAME_LIMIT, "fast");
        assert!(
            form.build().unwrap_err()[0]
                .message
                .contains("console, uncapped, vsync")
        );
    }

    #[test]
    fn vertical_sync_and_the_frame_limit_stay_one_setting() {
        let mut form = form();
        assert_eq!(form.value(VSYNC), "false");
        assert_eq!(form.set(VSYNC, "true"), [FRAME_LIMIT]);
        assert_eq!(form.value(FRAME_LIMIT), "vsync");
        assert_eq!(
            form.build().unwrap().graphics.frame_rate(),
            config::FrameRate::Vsync
        );
        assert_eq!(form.set(VSYNC, "false"), [FRAME_LIMIT]);
        assert_eq!(form.value(FRAME_LIMIT), "console");
        assert_eq!(form.set(FRAME_LIMIT, "vsync"), [VSYNC]);
        assert_eq!(form.value(VSYNC), "true");
        assert_eq!(form.set(FRAME_LIMIT, "60"), [VSYNC]);
        assert_eq!(form.value(VSYNC), "false");
        form.set(VSYNC, "false");
        assert_eq!(form.value(FRAME_LIMIT), "60");
    }

    #[test]
    fn dependent_fields_follow_the_setting_they_wait_for() {
        let mut form = form();
        let all = pages(&Env::default());
        let find = |id: &str| {
            all.iter()
                .flat_map(Page::fields)
                .find(|field| field.id == id)
                .unwrap()
                .clone()
        };
        let peak = find("display.hdr_peak_nits");
        let dlss = find("upscaling.dlss_path");
        assert!(!form.is_enabled(&peak) && !form.is_enabled(&dlss));
        form.set("graphics.hdr", "true");
        form.set("graphics.upscaler", "dlss");
        assert!(form.is_enabled(&peak) && form.is_enabled(&dlss));
    }

    #[test]
    fn only_the_files_that_changed_are_written() {
        let dir = tempfile::tempdir().unwrap();
        let old = Config::default();
        old.save(dir.path()).unwrap();
        let before = std::fs::read_to_string(dir.path().join("input.toml")).unwrap();
        let mut form = Form::new(&old, &pages(&Env::default()));
        form.set("graphics.resolution_scale", "0.8");
        form.set("widescreen.live_aspect", "true");
        let new = form.build().unwrap();
        let written = save_changed(dir.path(), &old, &new).unwrap();
        assert_eq!(written, ["graphics.toml", "widescreen.toml"]);
        assert_eq!(Config::load(dir.path()).unwrap(), new);
        let graphics = std::fs::read_to_string(dir.path().join("graphics.toml")).unwrap();
        assert!(graphics.contains("# How big the game renders"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("input.toml")).unwrap(),
            before
        );
        form.reload(&new);
        assert!(!form.is_dirty());
    }

    #[test]
    fn messages_are_shortened_for_the_space_beside_a_field() {
        assert_eq!(
            shorten("entry 2 \"Z\" is not a known button; use one of A, B, X"),
            "\"Z\" is not a known button"
        );
        assert_eq!(
            shorten("unknown action, the actions are jump, fire"),
            "unknown action"
        );
        assert_eq!(
            shorten("must be between 1 and 2, got 3"),
            "must be between 1 and 2, got 3"
        );
    }
}
