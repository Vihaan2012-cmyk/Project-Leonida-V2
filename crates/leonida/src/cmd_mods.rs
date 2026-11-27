use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Result, anyhow};
use game_package::Manifest;
use vfs::{LooseSource, ModStatus, Vfs};

use crate::cli::{ModsArgs, ModsCommand};

pub fn mods_command(args: &ModsArgs) -> Result<ExitCode> {
    match &args.command {
        ModsCommand::List => list(args),
        ModsCommand::Conflicts { game } => conflicts(args, game.as_ref()),
    }
}

fn load(args: &ModsArgs, game: Option<&PathBuf>) -> Result<(Vfs, vfs::LoadReport)> {
    let version = match &args.game_version {
        Some(version) => version.clone(),
        None => Manifest::builtin()
            .last()
            .map(|m| m.version.clone())
            .ok_or_else(|| anyhow!("no game version is known; pass --game-version"))?,
    };
    let mut builder = Vfs::builder(&version)
        .map_err(|e| anyhow!(e))?
        .mods_dir(args.root.join("Mods"))
        .mods_config(args.root.join("Config").join("mods.toml"));
    let fixes = args
        .fixes
        .clone()
        .or_else(|| Some(args.root.join("Fixes")).filter(|p| p.is_dir()));
    if let Some(fixes) = fixes {
        builder = builder.fixes(fixes);
    }
    if let Some(game) = game {
        builder = builder.base(LooseSource::new(game));
    }
    Ok(builder.build()?)
}

fn list(args: &ModsArgs) -> Result<ExitCode> {
    let (vfs, report) = load(args, None)?;
    for layer in vfs.layers() {
        if layer.kind == vfs::LayerKind::Fixes {
            println!("fixes   {}  ({} files)", layer.name, layer.files);
        }
    }
    if report.mods.active.is_empty() && report.mods.inactive.is_empty() {
        println!("no packages in {}", args.root.join("Mods").display());
    }
    let files = |name: &str| {
        vfs.layers()
            .into_iter()
            .find(|l| l.name == name)
            .map_or(0, |l| l.files)
    };
    for (index, entry) in report.mods.active.iter().enumerate() {
        let Some(manifest) = &entry.manifest else {
            continue;
        };
        println!(
            "{:>3}  {}  {} {}  [{}]  by {}  ({} files)",
            index + 1,
            entry.id,
            manifest.title,
            manifest.version,
            manifest.kind.name(),
            manifest.author,
            files(&entry.id)
        );
    }
    for entry in &report.mods.inactive {
        match &entry.status {
            ModStatus::Disabled => println!("off  {}  turned off in Config/mods.toml", entry.id),
            ModStatus::Error(error) => println!("ERR  {}  {error}", entry.id),
            ModStatus::Active => {}
        }
    }
    for note in report.mods.notes.iter().chain(&report.notes) {
        println!("note: {note}");
    }
    println!("higher in the list wins when two packages provide the same file");
    Ok(ExitCode::SUCCESS)
}

fn conflicts(args: &ModsArgs, game: Option<&PathBuf>) -> Result<ExitCode> {
    let (vfs, report) = load(args, game)?;
    for problem in report.problems() {
        println!("not loaded: {problem}");
    }
    let found = vfs.conflicts(game.is_some());
    for conflict in &found {
        println!(
            "{}\n    {} (wins) over {}",
            conflict.path,
            conflict.winner(),
            conflict.layers[..conflict.layers.len() - 1]
                .iter()
                .rev()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    println!("{} contested file(s)", found.len());
    Ok(ExitCode::SUCCESS)
}
