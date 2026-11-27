use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use game_package::{Manifest, match_version};
use rpf9::{ArchiveInfo, Index, IndexOptions, Kind, Progress, Query, Resource};

use crate::cli::{AssetsArgs, AssetsCommand, IndexArgs, ListArgs};
use crate::game::resolve_game;

const DB_NAME: &str = "leonida.db";

pub fn assets_command(args: &AssetsArgs) -> Result<ExitCode> {
    let db = args.output.join(DB_NAME);
    match &args.command {
        AssetsCommand::Index(index) => index_command(index, &db),
        AssetsCommand::List(list) => list_command(list, &db),
    }
}

fn mib(bytes: u64) -> String {
    format!("{:.1} MiB", bytes as f64 / (1 << 20) as f64)
}

fn index_command(args: &IndexArgs, db: &Path) -> Result<ExitCode> {
    let manifests = Manifest::builtin();
    let game = resolve_game(args.game.as_ref(), &manifests)?;
    let Some(manifest) = match_version(&game, &manifests) else {
        bail!("{} is not a version Leonida knows", game.display());
    };
    let index = Index::open(db).with_context(|| format!("opening {}", db.display()))?;
    let options = IndexOptions {
        only: args.only.clone(),
        jobs: args.jobs,
    };
    let done = rpf9::index_package(
        &index,
        &game,
        manifest,
        &options,
        &|progress| match progress {
            Progress::Started { name, size } => eprintln!("reading {name} ({})", mib(size)),
            Progress::Finished(info) => eprintln!(
                "  {}: {} resources from {} streams, {} unread",
                info.name,
                info.resources,
                info.streams,
                mib(info.unread_bytes)
            ),
        },
    )?;
    if done.is_empty() {
        bail!("no archive matched");
    }
    println!("{}: {} archives indexed", db.display(), done.len());
    print_counts(&index)?;
    Ok(ExitCode::SUCCESS)
}

fn print_counts(index: &Index) -> Result<()> {
    let counts = index.counts()?;
    let total: u64 = counts.values().sum();
    println!("{total} resources");
    for kind in Kind::ALL {
        if let Some(count) = counts.get(&kind) {
            println!("  {:<14}{count:>10}", kind.name());
        }
    }
    Ok(())
}

fn list_command(args: &ListArgs, db: &Path) -> Result<ExitCode> {
    if !db.is_file() {
        bail!(
            "no index at {}: run `leonida assets index` first",
            db.display()
        );
    }
    let index = Index::open(db).with_context(|| format!("opening {}", db.display()))?;
    let kind = match &args.kind {
        Some(text) => Some(Kind::parse(text).with_context(|| {
            let names: Vec<&str> = Kind::ALL.iter().map(|kind| kind.name()).collect();
            format!("unknown type {text}; the types are {}", names.join(", "))
        })?),
        None => None,
    };
    let archives: BTreeMap<u32, ArchiveInfo> = index
        .archives()?
        .into_iter()
        .map(|info| (info.id, info))
        .collect();
    let found = index.list(&Query {
        kind,
        find: args.find.clone(),
        limit: Some(args.limit),
    })?;
    for resource in &found {
        println!("{}", describe(resource, &archives));
    }
    if found.is_empty() {
        println!("nothing found");
        return Ok(ExitCode::FAILURE);
    }
    Ok(ExitCode::SUCCESS)
}

fn describe(resource: &Resource, archives: &BTreeMap<u32, ArchiveInfo>) -> String {
    let archive = archives
        .get(&resource.archive)
        .map_or("?", |info| info.name.as_str());
    let name = resource.name.as_deref().unwrap_or("-");
    let hash = resource
        .name_hash
        .map_or_else(|| "-".to_string(), |hash| format!("{hash:08x}"));
    format!(
        "{archive}:{:#x}  {:<13} {} v{}  {} bytes  {}+{}  name {name}  namehash {hash}  content {}",
        resource.offset,
        resource.kind.name(),
        resource.magic_text(),
        resource.version,
        resource.size,
        resource.layout.virtual_size,
        resource.layout.physical_size,
        &resource.content_hash_hex()[..16],
    )
}
