//! Everything a run needs from the settings and the command line, worked out once and without touching the PC.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Result, anyhow, bail};
use config::{AspectMode, Config, FrameRate, Graphics, Nudity, Upscaler, WindowMode};
use enhancements::widescreen;
use shims::audio::OutputMode;
use shims::gameinput::InputConfig;
use xbox_d3d12::pacing::FrameLimit;

use super::input;
use crate::cli::RunArgs;

/// The settings files, read together.
pub type Settings = Config;

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimePlan {
    pub game_dir: PathBuf,
    pub output_dir: PathBuf,
    pub config_dir: PathBuf,
    /// The packages folder and Leonida's own fixes, which sit next to the settings.
    pub mods_dir: PathBuf,
    pub fixes_dir: PathBuf,
    pub mod_load_order: Vec<String>,
    pub frame_limit: FrameLimit,
    /// Whether the frame limit is anything but the console's, which is what the game's pacing code is patched for.
    pub custom_limit: bool,
    pub graphics: Graphics,
    pub upscaler: Upscaler,
    pub frame_generation: bool,
    pub widescreen: widescreen::Config,
    pub input: InputConfig,
    /// Bindings in input.toml that cannot be carried out, each with the reason.
    pub input_skipped: Vec<String>,
    pub block_nudity: bool,
    pub blocklist: PathBuf,
    pub audio: OutputMode,
    pub language: String,
    pub trace: bool,
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

fn frame_limit(args: &RunArgs, graphics: &Graphics) -> Result<FrameLimit> {
    if let Some(text) = &args.frame_limit {
        return FrameLimit::from_str(text).map_err(|error| anyhow!("--frame-limit: {error}"));
    }
    Ok(match graphics.frame_rate() {
        FrameRate::Console => FrameLimit::Console,
        FrameRate::Uncapped => FrameLimit::Uncapped,
        FrameRate::Vsync => FrameLimit::VSync,
        FrameRate::Cap(0) => bail!("graphics.toml: frame_rate_cap must be above 0"),
        FrameRate::Cap(fps) => FrameLimit::Cap(f64::from(fps)),
    })
}

fn widescreen_config(graphics: &Graphics) -> widescreen::Config {
    let mut chosen = if graphics.aspect_mode == AspectMode::Native {
        widescreen::Config::ultrawide()
    } else {
        widescreen::Config::default()
    };
    if graphics.fov_scale != 1.0 {
        chosen.fov_scale = Some(graphics.fov_scale);
    }
    chosen
}

impl RuntimePlan {
    /// The plan for starting `game_dir` with `settings`. Command-line choices win over the settings.
    pub fn from(args: &RunArgs, settings: &Settings, game_dir: &Path) -> Result<Self> {
        let output = args
            .output
            .clone()
            .unwrap_or_else(|| settings.general.output_folder.clone());
        let output_dir = absolute(&output);
        let config_dir = absolute(&args.config);
        let home = config_dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| config_dir.clone());
        let frame_limit = frame_limit(args, &settings.graphics)?;
        let (input, input_skipped) = input::input_config(&settings.input);
        Ok(RuntimePlan {
            game_dir: absolute(game_dir),
            blocklist: output_dir.join(content_filter::blocklist::FILE_NAME),
            output_dir,
            config_dir,
            mods_dir: home.join("Mods"),
            fixes_dir: home.join("Fixes"),
            mod_load_order: settings.mods.load_order.clone(),
            custom_limit: !matches!(frame_limit, FrameLimit::Console),
            frame_limit,
            graphics: settings.graphics.clone(),
            upscaler: settings.graphics.upscaler,
            frame_generation: settings.graphics.frame_generation,
            widescreen: widescreen_config(&settings.graphics),
            input,
            input_skipped,
            block_nudity: settings.general.content.nudity == Nudity::Block,
            audio: match settings.general.audio.output {
                config::AudioOutput::Auto => OutputMode::Auto,
                config::AudioOutput::Off => OutputMode::Off,
            },
            language: settings.general.language.clone(),
            trace: args.trace,
        })
    }

    /// The lines `leonida run` prints before it starts, and `--dry-run` prints instead of starting.
    pub fn summary(&self) -> Vec<String> {
        let mut lines = vec![
            "plan".to_string(),
            format!("  game         {}", self.game_dir.display()),
            format!("  output       {}", self.output_dir.display()),
            format!("  settings     {}", self.config_dir.display()),
            format!("  language     {}", self.language),
            format!("  frame limit  {}", describe_limit(self.frame_limit)),
            format!("  upscaler     {}", self.describe_upscaler()),
            format!("  window       {}", self.describe_window()),
            format!("  widescreen   {}", describe_widescreen(&self.widescreen)),
            format!("  patches      {}", self.describe_patches()),
            format!("  input        {}", self.describe_input()),
            format!(
                "  nudity       {}",
                if self.block_nudity {
                    format!("blocked, from {}", self.blocklist.display())
                } else {
                    "allowed".to_string()
                }
            ),
            format!(
                "  audio        {}",
                match self.audio {
                    OutputMode::Auto => "default output device",
                    OutputMode::Off => "off, mixed but not played",
                }
            ),
        ];
        let mods = if self.mod_load_order.is_empty() {
            "none".to_string()
        } else {
            self.mod_load_order.join(", ")
        };
        lines.push(format!(
            "  mods         {mods} (from {})",
            self.mods_dir.display()
        ));
        lines
    }

    fn describe_upscaler(&self) -> String {
        let name = match self.upscaler {
            Upscaler::Off => return format!("off{}", self.describe_generation()),
            Upscaler::Fsr1 => "fsr1",
            Upscaler::Temporal => "temporal",
            Upscaler::Dlss => "dlss",
        };
        format!(
            "{name}, rendering at {:.0}% of the window{}",
            self.graphics.resolution_scale * 100.0,
            self.describe_generation()
        )
    }

    fn describe_generation(&self) -> &'static str {
        if self.frame_generation {
            ", frame generation on"
        } else {
            ""
        }
    }

    fn describe_window(&self) -> String {
        let mode = match self.graphics.window_mode {
            WindowMode::Windowed => "windowed",
            WindowMode::Borderless => "borderless",
            WindowMode::Fullscreen => "fullscreen",
        };
        let aspect = match self.graphics.aspect_mode {
            AspectMode::Letterbox => "letterbox",
            AspectMode::Native => "native shape",
            AspectMode::Stretch => "stretch",
        };
        format!(
            "{}x{} {mode}, {aspect}, hdr {}",
            self.graphics.window_width,
            self.graphics.window_height,
            if self.graphics.hdr { "on" } else { "off" }
        )
    }

    fn describe_patches(&self) -> String {
        let mut names = Vec::new();
        if self.custom_limit {
            names.push("framerate".to_string());
        }
        if let Ok(groups) = self.widescreen.groups() {
            names.extend(groups.iter().map(|group| group.id.to_string()));
        }
        if names.is_empty() {
            "none".to_string()
        } else {
            names.join(", ")
        }
    }

    fn describe_input(&self) -> String {
        let mut text = format!(
            "{} bindings, mouse sensitivity {}",
            self.input.bindings.len(),
            self.input.mouse_sensitivity
        );
        if !self.input_skipped.is_empty() {
            text.push_str(&format!(", {} not carried out", self.input_skipped.len()));
        }
        text
    }
}

fn describe_limit(limit: FrameLimit) -> String {
    match limit {
        FrameLimit::Console => "console (what the game asks for, 30 or 60)".to_string(),
        FrameLimit::Cap(fps) => format!("{fps} frames a second"),
        FrameLimit::Uncapped => "uncapped".to_string(),
        FrameLimit::VSync => "vsync".to_string(),
    }
}

fn describe_widescreen(config: &widescreen::Config) -> String {
    let mut parts = Vec::new();
    if config.live_aspect {
        parts.push("live aspect".to_string());
    }
    if config.no_pillarbox {
        parts.push("no bars".to_string());
    }
    if let Some(scale) = config.fov_scale {
        parts.push(format!("field of view x{scale}"));
    }
    if parts.is_empty() {
        "off".to_string()
    } else {
        parts.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::{FrameRateMode, Section};

    fn args() -> RunArgs {
        RunArgs {
            game: None,
            file: None,
            dry_run: false,
            start_the_game: true,
            all: false,
            trace: false,
            output: None,
            config: PathBuf::from("E:\\Leo\\Config"),
            frame_limit: None,
        }
    }

    fn plan(args: &RunArgs, settings: &Settings) -> Result<RuntimePlan> {
        RuntimePlan::from(args, settings, Path::new("E:\\Games\\GTA6"))
    }

    fn graphics(text: &str) -> Settings {
        Settings {
            graphics: Graphics::parse(text).unwrap(),
            ..Settings::default()
        }
    }

    #[test]
    fn the_defaults_are_the_consoles_and_change_nothing_in_the_game() {
        let plan = plan(&args(), &Settings::default()).unwrap();
        assert_eq!(plan.frame_limit, FrameLimit::Console);
        assert!(!plan.custom_limit);
        assert_eq!(plan.upscaler, Upscaler::Off);
        assert!(!plan.frame_generation);
        assert_eq!(plan.widescreen, widescreen::Config::default());
        assert!(plan.block_nudity);
        assert_eq!(plan.audio, OutputMode::Auto);
        assert_eq!(plan.game_dir, PathBuf::from("E:\\Games\\GTA6"));
        assert_eq!(plan.output_dir, absolute(Path::new("Output")));
        assert_eq!(
            plan.blocklist,
            plan.output_dir.join("content-blocklist.json")
        );
        assert_eq!(plan.mods_dir, PathBuf::from("E:\\Leo\\Mods"));
        assert_eq!(plan.config_dir, PathBuf::from("E:\\Leo\\Config"));
        assert_eq!(plan.describe_patches(), "none");
    }

    #[test]
    fn each_frame_rate_mode_becomes_its_limit() {
        let cases = [
            ("frame_rate_mode = \"console\"", FrameLimit::Console, false),
            ("frame_rate_mode = \"uncapped\"", FrameLimit::Uncapped, true),
            ("frame_rate_mode = \"vsync\"", FrameLimit::VSync, true),
            (
                "frame_rate_mode = \"cap\"\nframe_rate_cap = 90",
                FrameLimit::Cap(90.0),
                true,
            ),
        ];
        for (text, limit, custom) in cases {
            let plan = plan(&args(), &graphics(text)).unwrap();
            assert_eq!(plan.frame_limit, limit, "{text}");
            assert_eq!(plan.custom_limit, custom, "{text}");
        }
    }

    #[test]
    fn the_command_line_limit_wins_and_accepts_every_form() {
        let settings = graphics("frame_rate_mode = \"vsync\"");
        for (text, limit) in [
            ("console", FrameLimit::Console),
            ("uncapped", FrameLimit::Uncapped),
            ("vsync", FrameLimit::VSync),
            ("144", FrameLimit::Cap(144.0)),
            ("59.94", FrameLimit::Cap(59.94)),
        ] {
            let mut given = args();
            given.frame_limit = Some(text.to_string());
            assert_eq!(
                plan(&given, &settings).unwrap().frame_limit,
                limit,
                "{text}"
            );
        }
    }

    #[test]
    fn a_bad_limit_is_an_error_that_names_where_it_came_from() {
        let mut given = args();
        given.frame_limit = Some("fast".to_string());
        let error = plan(&given, &Settings::default()).unwrap_err().to_string();
        assert!(
            error.starts_with("--frame-limit: frame limit must be"),
            "{error}"
        );
        assert!(error.contains("fast"), "{error}");
        given.frame_limit = Some("-5".to_string());
        assert!(plan(&given, &Settings::default()).is_err());
        let zero = Settings {
            graphics: Graphics {
                frame_rate_mode: FrameRateMode::Cap,
                frame_rate_cap: 0,
                ..Graphics::default()
            },
            ..Settings::default()
        };
        let error = plan(&args(), &zero).unwrap_err().to_string();
        assert!(error.contains("frame_rate_cap"), "{error}");
    }

    #[test]
    fn a_native_aspect_and_a_field_of_view_scale_turn_on_widescreen_groups() {
        let native = plan(
            &args(),
            &graphics("aspect_mode = \"native\"\nfov_scale = 1.2"),
        )
        .unwrap();
        assert!(native.widescreen.live_aspect && native.widescreen.no_pillarbox);
        assert_eq!(native.widescreen.fov_scale, Some(1.2));
        let stretch = plan(&args(), &graphics("aspect_mode = \"stretch\"")).unwrap();
        assert_eq!(stretch.widescreen, widescreen::Config::default());
    }

    #[test]
    fn upscaler_nudity_audio_and_mods_follow_the_settings() {
        let mut settings =
            graphics("upscaler = \"temporal\"\nframe_generation = true\nresolution_scale = 0.67");
        settings.general.content.nudity = Nudity::Allow;
        settings.general.audio.output = config::AudioOutput::Off;
        settings.mods.load_order = vec!["a".into(), "b".into()];
        let plan = plan(&args(), &settings).unwrap();
        assert_eq!(plan.upscaler, Upscaler::Temporal);
        assert!(plan.frame_generation);
        assert!(!plan.block_nudity);
        assert_eq!(plan.audio, OutputMode::Off);
        assert_eq!(plan.mod_load_order, ["a", "b"]);
        let text = plan.summary().join("\n");
        assert!(
            text.contains("temporal, rendering at 67% of the window, frame generation on"),
            "{text}"
        );
        assert!(text.contains("nudity       allowed"), "{text}");
        assert!(text.contains("mods         a, b"), "{text}");
    }

    #[test]
    fn the_output_folder_comes_from_the_command_line_then_the_settings() {
        let mut settings = Settings::default();
        settings.general.output_folder = PathBuf::from("E:\\Out\\Leo");
        assert_eq!(
            plan(&args(), &settings).unwrap().output_dir,
            PathBuf::from("E:\\Out\\Leo")
        );
        let mut given = args();
        given.output = Some(PathBuf::from("E:\\Elsewhere"));
        assert_eq!(
            plan(&given, &settings).unwrap().output_dir,
            PathBuf::from("E:\\Elsewhere")
        );
    }

    #[test]
    fn the_summary_names_every_part_of_the_plan() {
        let lines = plan(&args(), &Settings::default()).unwrap().summary();
        for label in [
            "game",
            "output",
            "settings",
            "frame limit",
            "upscaler",
            "window",
            "widescreen",
            "patches",
            "input",
            "nudity",
            "audio",
            "mods",
        ] {
            assert!(
                lines
                    .iter()
                    .any(|line| line.trim_start().starts_with(label)),
                "{label} in {lines:?}"
            );
        }
    }
}
