//! What Leonida has already made in the output folder.

use std::path::Path;
use std::time::SystemTime;

pub const DATABASE: &str = "leonida.db";
pub const PIPELINE_CACHE: &str = "pipelines.cache";
pub const BLOCKLIST: &str = "content-blocklist.json";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Made {
    pub bytes: u64,
    pub modified: Option<SystemTime>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cache {
    /// The translated shaders.
    pub shaders: Option<Made>,
    /// The graphics pipelines the driver built.
    pub pipelines: Option<Made>,
    /// What the nudity filter hides.
    pub blocklist: Option<Made>,
}

fn made(path: &Path) -> Option<Made> {
    let meta = std::fs::metadata(path).ok().filter(|meta| meta.is_file())?;
    Some(Made {
        bytes: meta.len(),
        modified: meta.modified().ok(),
    })
}

pub fn inspect(output: &Path) -> Cache {
    Cache {
        shaders: made(&output.join(DATABASE)),
        pipelines: made(&output.join(PIPELINE_CACHE)),
        blocklist: made(&output.join(BLOCKLIST)),
    }
}

pub fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["bytes", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} bytes")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// How long ago, in the largest whole unit.
pub fn age_text(modified: Option<SystemTime>, now: SystemTime) -> String {
    let Some(secs) = modified
        .and_then(|time| now.duration_since(time).ok())
        .map(|age| age.as_secs())
    else {
        return String::new();
    };
    let (count, unit) = match secs {
        0..=89 => return "just now".to_string(),
        90..=5399 => (secs / 60, "minute"),
        5400..=129_599 => (secs / 3600, "hour"),
        _ => (secs / 86_400, "day"),
    };
    format!("{count} {unit}{} ago", if count == 1 { "" } else { "s" })
}

fn line(name: &str, made: Option<Made>, now: SystemTime, missing: &str) -> String {
    match made {
        Some(made) => {
            let age = age_text(made.modified, now);
            let age = if age.is_empty() {
                age
            } else {
                format!(", {age}")
            };
            format!("{name}: {}{age}", size_text(made.bytes))
        }
        None => format!("{name}: {missing}"),
    }
}

impl Cache {
    pub fn shader_line(&self, now: SystemTime) -> String {
        line("Shader database", self.shaders, now, "none yet")
    }

    pub fn pipeline_line(&self, now: SystemTime) -> String {
        line("Pipeline cache", self.pipelines, now, "none yet")
    }

    pub fn blocklist_line(&self, now: SystemTime) -> String {
        line("Blocklist", self.blocklist, now, "not made yet")
    }

    /// Both the translated shaders and the pipeline cache exist.
    pub fn is_prepared(&self) -> bool {
        self.shaders.is_some() && self.pipelines.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn sizes_read_in_the_nearest_unit() {
        assert_eq!(size_text(0), "0 bytes");
        assert_eq!(size_text(1023), "1023 bytes");
        assert_eq!(size_text(1536), "1.5 KiB");
        assert_eq!(size_text(5 * 1024 * 1024), "5.0 MiB");
        assert_eq!(size_text(3 * 1024 * 1024 * 1024), "3.0 GiB");
    }

    #[test]
    fn ages_read_in_the_largest_whole_unit() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000_000);
        let ago = |secs| age_text(Some(now - Duration::from_secs(secs)), now);
        assert_eq!(ago(10), "just now");
        assert_eq!(ago(600), "10 minutes ago");
        assert_eq!(ago(3600), "60 minutes ago");
        assert_eq!(ago(7200), "2 hours ago");
        assert_eq!(ago(3 * 86_400), "3 days ago");
        assert_eq!(ago(86_400 * 2), "2 days ago");
        assert_eq!(age_text(None, now), "");
        assert_eq!(age_text(Some(now + Duration::from_secs(5)), now), "");
    }

    #[test]
    fn the_output_folder_is_read_for_what_is_there() {
        let dir = tempfile::tempdir().unwrap();
        let empty = inspect(dir.path());
        assert_eq!(empty, Cache::default());
        assert!(!empty.is_prepared());
        let now = SystemTime::now();
        assert_eq!(empty.shader_line(now), "Shader database: none yet");
        assert_eq!(empty.blocklist_line(now), "Blocklist: not made yet");
        std::fs::write(dir.path().join(DATABASE), vec![0u8; 2048]).unwrap();
        std::fs::write(dir.path().join(PIPELINE_CACHE), b"x").unwrap();
        std::fs::create_dir(dir.path().join(BLOCKLIST)).unwrap();
        let found = inspect(dir.path());
        let now = SystemTime::now() + Duration::from_secs(5);
        assert_eq!(found.shaders.unwrap().bytes, 2048);
        assert!(found.is_prepared());
        assert_eq!(found.blocklist, None, "a folder is not a blocklist");
        assert!(
            found
                .shader_line(now)
                .starts_with("Shader database: 2.0 KiB, ")
        );
        assert!(
            found
                .pipeline_line(now)
                .starts_with("Pipeline cache: 1 bytes")
        );
    }
}
