//! Index every archive of a game package.

use std::path::Path;

use game_package::{Manifest, package_path};
use rayon::prelude::*;

use crate::archive::Archive;
use crate::error::Rpf9Error;
use crate::index::{ArchiveInfo, Index};
use crate::model::Resource;

const BATCH: usize = 10_000;

#[derive(Debug, Clone)]
pub struct IndexOptions {
    /// Only archives whose path contains this text.
    pub only: Option<String>,
    /// How many archives are scanned at once.
    pub jobs: usize,
}

impl Default for IndexOptions {
    fn default() -> Self {
        IndexOptions {
            only: None,
            jobs: 2,
        }
    }
}

pub enum Progress<'a> {
    Started { name: &'a str, size: u64 },
    Finished(&'a ArchiveInfo),
}

/// The archives of a package, by path inside it, largest listed with their manifest sizes.
pub fn package_archives(manifest: &Manifest) -> Vec<(String, u64)> {
    manifest
        .files
        .iter()
        .filter(|(name, _)| name.ends_with(".rpf"))
        .map(|(name, entry)| (name.clone(), entry.size))
        .collect()
}

/// Scan one archive of the package into the index under the id `id`.
pub fn index_archive(
    index: &Index,
    game: &Path,
    name: &str,
    id: u32,
) -> Result<ArchiveInfo, Rpf9Error> {
    let archive = Archive::open(&package_path(game, name))?;
    let mut batch: Vec<Resource> = Vec::with_capacity(BATCH);
    let mut failed: Option<Rpf9Error> = None;
    let report = {
        let mut push = |mut resource: Resource| {
            if failed.is_some() {
                return;
            }
            resource.archive = id;
            batch.push(resource);
            if batch.len() >= BATCH {
                if let Err(error) = index.add(&batch) {
                    failed = Some(error);
                }
                batch.clear();
            }
        };
        if name.rsplit('/').next() == Some("shaders.rpf") {
            archive.scan_shaders(&mut push)
        } else {
            archive.scan(&mut push)
        }
    };
    if let Some(error) = failed {
        return Err(error);
    }
    index.add(&batch)?;
    index.finish_archive(id, name, archive.len(), &report)
}

/// Scan the archives of `manifest`'s package under `game` into `index`, `options.jobs` at a time. Archives
/// already in the index are scanned again. Returns one entry per archive, in path order.
pub fn index_package(
    index: &Index,
    game: &Path,
    manifest: &Manifest,
    options: &IndexOptions,
    progress: &(dyn Fn(Progress<'_>) + Sync),
) -> Result<Vec<ArchiveInfo>, Rpf9Error> {
    let archives: Vec<(String, u64)> = package_archives(manifest)
        .into_iter()
        .filter(|(name, _)| options.only.as_ref().is_none_or(|only| name.contains(only.as_str())))
        .collect();
    let mut work: Vec<(String, u64, u32)> = Vec::new();
    for (name, size) in archives {
        let id = index.begin_archive(&name, size)?;
        work.push((name, size, id));
    }
    work.sort_by_key(|(_, size, _)| std::cmp::Reverse(*size));
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(options.jobs.max(1))
        .build()
        .map_err(|error| Rpf9Error::Index(error.to_string()))?;
    let mut done: Vec<ArchiveInfo> = pool.install(|| {
        work.par_iter()
            .map(|(name, size, id)| {
                progress(Progress::Started { name, size: *size });
                let info = index_archive(index, game, name, *id)?;
                progress(Progress::Finished(&info));
                Ok(info)
            })
            .collect::<Result<Vec<_>, Rpf9Error>>()
    })?;
    done.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(done)
}
