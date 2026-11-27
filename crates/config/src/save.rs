//! Writing settings back into the file they came from: comments, spacing and the order of the lines stay, only the
//! values that changed are replaced.

use std::path::Path;

use toml_edit::{DocumentMut, Item, Table, Value};

use crate::{ConfigError, Section};

/// Added to the name of a file that could not be read as TOML, which is copied there before its replacement is
/// written.
pub const BACKUP_SUFFIX: &str = ".bak";

/// `existing` with the values of `section` merged in. Names the section does not know are kept; names it knows but
/// leaves out (an optional setting that is now unset) are removed.
pub fn merge<S: Section>(existing: &str, section: &S) -> Result<String, String> {
    let mut document: DocumentMut = existing
        .parse()
        .map_err(|e: toml_edit::TomlError| e.to_string())?;
    let mut wanted: DocumentMut = section
        .to_text()
        .parse()
        .map_err(|e: toml_edit::TomlError| e.to_string())?;
    compact(wanted.as_table_mut());
    merge_table::<S>(document.as_table_mut(), wanted.as_table(), &mut Vec::new());
    Ok(document.to_string())
}

/// `to_string_pretty` spreads lists over several lines; the settings files keep them on one.
fn compact(table: &mut Table) {
    for (_, item) in table.iter_mut() {
        match item {
            Item::Table(inner) => compact(inner),
            Item::Value(Value::Array(array)) => array.fmt(),
            _ => {}
        }
    }
}

fn merge_table<S: Section>(old: &mut Table, new: &Table, path: &mut Vec<String>) {
    let stale: Vec<String> = old
        .iter()
        .map(|(key, _)| key.to_string())
        .filter(|key| new.get(key).is_none() && is_known::<S>(old, path, key))
        .collect();
    for key in stale {
        old.remove(&key);
    }
    for (key, item) in new.iter() {
        match old.get_mut(key) {
            Some(existing) => {
                if let (Item::Table(old_table), Item::Table(new_table)) = (&mut *existing, item) {
                    path.push(key.to_string());
                    merge_table::<S>(old_table, new_table, path);
                    path.pop();
                } else {
                    replace_value(existing, item);
                }
            }
            None => {
                old.insert(key, item.clone());
            }
        }
    }
}

/// Whether the section would accept `key` at `path`: it does not complain that the name is unknown.
fn is_known<S: Section>(table: &Table, path: &[String], key: &str) -> bool {
    let Some(item) = table.get(key) else {
        return false;
    };
    let mut probe = DocumentMut::new();
    let mut at = probe.as_table_mut();
    for part in path {
        at.insert(part, Item::Table(Table::new()));
        at = at
            .get_mut(part)
            .and_then(Item::as_table_mut)
            .expect("just inserted");
    }
    at.insert(key, item.clone());
    match toml::from_str::<S>(&probe.to_string()) {
        Ok(_) => true,
        Err(error) => !error.message().contains("unknown field"),
    }
}

fn replace_value(old: &mut Item, new: &Item) {
    if let (Item::Value(before), Item::Value(after)) = (&*old, new)
        && same_value(before, after)
    {
        return;
    }
    let mut replacement = new.clone();
    if let (Item::Value(before), Item::Value(after)) = (&*old, &mut replacement) {
        *after.decor_mut() = before.decor().clone();
    }
    *old = replacement;
}

fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| same_value(p, q))
        }
        (Value::String(x), Value::String(y)) => x.value() == y.value(),
        (Value::Boolean(x), Value::Boolean(y)) => x.value() == y.value(),
        _ => match (number(a), number(b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        },
    }
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Integer(n) => Some(*n.value() as f64),
        Value::Float(n) => Some(*n.value()),
        _ => None,
    }
}

/// Writes `section` to `dir/FILE`: into the file's own text when there is one, otherwise into the documented
/// defaults. A file that is not TOML is first copied to `FILE.bak`, then replaced.
pub fn save<S: Section>(section: &S, dir: &Path) -> Result<(), ConfigError> {
    let io = |path: &Path| {
        let path = path.to_path_buf();
        move |source| ConfigError::Io { path, source }
    };
    std::fs::create_dir_all(dir).map_err(io(dir))?;
    let path = dir.join(S::FILE);
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => S::default_text(),
        Err(source) => return Err(ConfigError::Io { path, source }),
    };
    let text = match merge(&existing, section) {
        Ok(text) => text,
        Err(_) => {
            let backup = dir.join(format!("{}{BACKUP_SUFFIX}", S::FILE));
            std::fs::copy(&path, &backup).map_err(io(&backup))?;
            merge(&S::default_text(), section).unwrap_or_else(|_| section.to_text())
        }
    };
    let staging = dir.join(format!("{}.new", S::FILE));
    std::fs::write(&staging, text).map_err(io(&staging))?;
    std::fs::rename(&staging, &path).map_err(io(&path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{General, Graphics, Input, Mods, Upscaling};
    use std::path::PathBuf;

    fn read(dir: &Path, file: &str) -> String {
        std::fs::read_to_string(dir.join(file)).unwrap()
    }

    #[test]
    fn changed_values_replace_in_place_and_comments_stay() {
        let dir = tempfile::tempdir().unwrap();
        Graphics::write_default(dir.path()).unwrap();
        let mut graphics = Graphics::load(dir.path()).unwrap();
        graphics.resolution_scale = 0.75;
        graphics.hdr = true;
        save(&graphics, dir.path()).unwrap();
        let text = read(dir.path(), "graphics.toml");
        assert!(text.contains("resolution_scale = 0.75"), "{text}");
        assert!(text.contains("hdr = true"), "{text}");
        assert!(text.contains("# High dynamic range output."), "{text}");
        assert_eq!(Graphics::parse(&text).unwrap(), graphics);
    }

    #[test]
    fn unchanged_values_keep_their_exact_text() {
        let dir = tempfile::tempdir().unwrap();
        let written = "# mine\nresolution_scale = 1   # one\nhdr = false\n";
        std::fs::write(dir.path().join("graphics.toml"), written).unwrap();
        let graphics = Graphics::load(dir.path()).unwrap();
        save(&graphics, dir.path()).unwrap();
        let text = read(dir.path(), "graphics.toml");
        assert!(text.starts_with(written), "{text}");
        assert_eq!(Graphics::parse(&text).unwrap(), graphics);
    }

    #[test]
    fn a_missing_file_starts_from_the_documented_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let general = General {
            language: "fr".into(),
            ..General::default()
        };
        save(&general, dir.path()).unwrap();
        let text = read(dir.path(), "leonida.toml");
        assert!(text.contains("language = \"fr\""), "{text}");
        assert!(text.contains("# Language code for Leonida"), "{text}");
    }

    #[test]
    fn an_unset_option_is_removed_and_a_set_one_added() {
        let dir = tempfile::tempdir().unwrap();
        let mut general = General {
            game_folder: Some(PathBuf::from("E:/Games/GTA6")),
            ..General::default()
        };
        save(&general, dir.path()).unwrap();
        let text = read(dir.path(), "leonida.toml");
        assert!(text.contains("\ngame_folder = \"E:/Games/GTA6\""), "{text}");
        assert_eq!(General::parse(&text).unwrap(), general);
        general.game_folder = None;
        save(&general, dir.path()).unwrap();
        let text = read(dir.path(), "leonida.toml");
        assert!(!text.contains("\ngame_folder = "), "{text}");
        assert_eq!(General::parse(&text).unwrap(), general);
    }

    #[test]
    fn names_the_section_does_not_know_are_kept() {
        let existing =
            "# notes\nresolution_scale = 1.0\nfuture_setting = 3 # added by a newer Leonida\n";
        let graphics = Graphics {
            resolution_scale: 0.5,
            ..Graphics::default()
        };
        let text = merge(existing, &graphics).unwrap();
        assert!(
            text.contains("future_setting = 3 # added by a newer Leonida"),
            "{text}"
        );
        assert!(text.contains("resolution_scale = 0.5"), "{text}");
        assert!(text.contains("# notes"), "{text}");
    }

    #[test]
    fn lists_and_nested_tables_are_replaced() {
        let dir = tempfile::tempdir().unwrap();
        Input::write_default(dir.path()).unwrap();
        let mut input = Input::load(dir.path()).unwrap();
        input
            .keyboard
            .insert("jump".into(), vec!["J".into(), "Mouse4".into()]);
        input.dead_zones.left_stick = 0.3;
        save(&input, dir.path()).unwrap();
        let text = read(dir.path(), "input.toml");
        assert!(text.contains("jump = [\"J\", \"Mouse4\"]"), "{text}");
        assert!(text.contains("left_stick = 0.3"), "{text}");
        assert!(
            text.contains("# How fast the mouse turns the camera."),
            "{text}"
        );
        assert_eq!(Input::parse(&text).unwrap(), input);
        let mods = Mods {
            load_order: vec!["b".into(), "a".into()],
        };
        save(&mods, dir.path()).unwrap();
        let text = read(dir.path(), "mods.toml");
        assert_eq!(Mods::parse(&text).unwrap().load_order, ["b", "a"]);
    }

    #[test]
    fn a_file_that_is_not_toml_is_backed_up_before_it_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("upscaling.toml"), "this is = = not toml").unwrap();
        let upscaling = Upscaling {
            dlss_path: Some(PathBuf::from("C:/x/nvngx_dlss.dll")),
        };
        save(&upscaling, dir.path()).unwrap();
        assert_eq!(
            read(dir.path(), "upscaling.toml.bak"),
            "this is = = not toml"
        );
        let text = read(dir.path(), "upscaling.toml");
        assert_eq!(Upscaling::parse(&text).unwrap(), upscaling);
    }
}
