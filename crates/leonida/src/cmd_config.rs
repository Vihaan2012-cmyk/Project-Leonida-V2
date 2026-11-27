use std::path::Path;

use anyhow::{Result, bail};
use config::{
    Config, DisplayConfig, FILES, General, Graphics, Input, Mods, Section, Upscaling, Widescreen,
};

use crate::cli::{ConfigArgs, ConfigCommand};

pub fn config_command(args: &ConfigArgs) -> Result<std::process::ExitCode> {
    let dir = &args.dir;
    match &args.command {
        None | Some(ConfigCommand::Show) => show(dir),
        Some(ConfigCommand::Reset { file }) => reset(dir, file),
    }
    .map(|()| std::process::ExitCode::SUCCESS)
}

fn show(dir: &Path) -> Result<()> {
    let config = Config::load(dir)?;
    println!("settings in {}", dir.display());
    print_file(General::FILE, &config.general.to_text());
    print_file(Graphics::FILE, &config.graphics.to_text());
    print_file(Input::FILE, &config.input.to_text());
    print_file(Mods::FILE, &config.mods.to_text());
    print_file(DisplayConfig::FILE, &config.display.to_text());
    print_file(Widescreen::FILE, &config.widescreen.to_text());
    print_file(Upscaling::FILE, &config.upscaling.to_text());
    Ok(())
}

fn print_file(name: &str, text: &str) {
    println!("\n# {name}\n{}", text.trim_end());
}

fn reset(dir: &Path, only: &[String]) -> Result<()> {
    let mut names = Vec::new();
    for given in only {
        let wanted = if given.ends_with(".toml") {
            given.clone()
        } else {
            format!("{given}.toml")
        };
        match FILES.iter().find(|file| **file == wanted) {
            Some(file) => names.push(*file),
            None => bail!(
                "no settings file named {given}; they are {}",
                FILES.join(", ")
            ),
        }
    }
    for file in Config::reset(dir, &names)? {
        println!("wrote the defaults to {}", dir.join(file).display());
    }
    Ok(())
}
