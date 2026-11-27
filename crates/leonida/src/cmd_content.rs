use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use content_filter::archive::WalkOptions;
use content_filter::blocklist::FILE_NAME;
use content_filter::{CatalogueFile, ContentSettings, Scanner};
use game_package::{Manifest, match_version, package_path};

use crate::cli::ContentScanArgs;
use crate::game::resolve_game;

const DEFAULT_ARCHIVES: [&str; 2] = ["packs.rpf", "pack/core_restofworld/core_restofworld.rpf"];

pub fn scan_command(args: &ContentScanArgs) -> Result<ExitCode> {
    let manifests = Manifest::builtin();
    let game = resolve_game(args.game.as_ref(), &manifests)?;
    let version = match_version(&game, &manifests)
        .map_or_else(|| "unknown".to_string(), |m| m.version.clone());
    let settings = ContentSettings::load(&args.config).context("reading the content settings")?;
    println!(
        "nudity is set to {} in {}",
        if settings.blocks_nudity() {
            "block"
        } else {
            "allow"
        },
        args.config.display()
    );

    let archives: Vec<String> = if args.archives.is_empty() {
        DEFAULT_ARCHIVES
            .iter()
            .map(|name| name.to_string())
            .collect()
    } else {
        args.archives.clone()
    };
    let mut scanner = Scanner::new();
    let options = WalkOptions {
        start: None,
        limit: args.limit,
    };
    for archive in &archives {
        let path = package_path(&game, archive);
        if !path.is_file() {
            bail!("{} is not in the game folder", path.display());
        }
        println!("reading {archive}");
        scanner.add_archive_file(&path, options)?;
    }
    let catalogues = match &args.catalogues {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            serde_json::from_str::<CatalogueFile>(&text)
                .with_context(|| format!("{} is not a catalogue file", path.display()))?
                .catalogues
        }
        None => Vec::new(),
    };
    let (list, report) = scanner.finish(&version, &catalogues);
    let out = args.output.join(FILE_NAME);
    list.save(&out)?;

    for (archive, summary) in &report.archives {
        println!(
            "{archive}: {} streams, {} with names, {} bytes walked",
            summary.streams, summary.named_streams, summary.bytes_walked
        );
    }
    println!("{} resource names recognised", report.names_seen);
    for (category, count) in &report.blocked_by_category {
        println!("blocked {category:?}: {count}");
    }
    println!(
        "{} textures only used by blocked drawables; {} catalogue rules ({} emptied for lack of underwear); {} catalogue states with unnamed drawables",
        report.textures_blocked, report.catalogue_rules, report.emptied, report.unnamed_states
    );
    let bare = report
        .base_bodies
        .iter()
        .filter(|body| !body.has_cover)
        .count();
    if !report.base_bodies.is_empty() {
        println!(
            "{} base skin layers in catalogues, {bare} of them in catalogues with no underwear to cover them",
            report.base_bodies.len()
        );
    }
    for note in &list.notes {
        println!("note: {note}");
    }
    println!(
        "blocklist written to {} (derived from your game; do not share or commit it)",
        out.display()
    );
    Ok(ExitCode::SUCCESS)
}
