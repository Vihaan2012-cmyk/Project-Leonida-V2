use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::File;
use std::process::ExitCode;

use anyhow::{Context, Result};
use game_package::{Manifest, map_file, package_path};
use shader_container::{
    CarveStats, CarvedShader, CodeRange, Fssm, WaveSize, carve, parts, unpack_all,
};

use crate::cli::ScanArgs;
use crate::game::resolve_game;

pub fn scan(args: &ScanArgs) -> Result<ExitCode> {
    let game = resolve_game(args.game.as_ref(), &Manifest::builtin())?;
    let path = package_path(&game, "shaders.rpf");
    let file = File::open(&path).with_context(|| format!("opening {}", path.display()))?;
    let archive = map_file(&file).with_context(|| format!("mapping {}", path.display()))?;
    let mut summary = ScanSummary::default();
    let stats = carve(&archive, |shader| summary.add(&shader));
    print!("{}", summary.report(&stats));
    Ok(if stats.carved == 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// Counts gathered over every carved container. BTreeMaps keep the printed order stable.
#[derive(Debug, Default)]
struct ScanSummary {
    stages: BTreeMap<String, usize>,
    /// How many containers carry each DXBC part. Reflection parts (RDEF, STAT, PSV0, RDAT, ISG1, OSG1) here would
    /// give constant-buffer layouts for the renderer harness (spec section 13, risk 1).
    parts: BTreeMap<String, usize>,
    payloads: usize,
    multi_payload: usize,
    wave_by_stage: BTreeMap<String, usize>,
    wave: BTreeMap<&'static str, usize>,
    ranges: BTreeMap<&'static str, usize>,
    problems: BTreeMap<String, usize>,
}

impl ScanSummary {
    fn add(&mut self, shader: &CarvedShader) {
        let stage = shader
            .stage
            .map_or_else(|| "none".to_string(), |stage| stage.short_name());
        *self.stages.entry(stage.clone()).or_default() += 1;
        if let Ok(container_parts) = parts(&shader.dxbc) {
            for part in container_parts {
                *self
                    .parts
                    .entry(String::from_utf8_lossy(&part.fourcc).into_owned())
                    .or_default() += 1;
            }
        }
        let blobs = match unpack_all(&shader.dxbc) {
            Ok(blobs) => blobs,
            Err(error) => {
                *self.problems.entry(error.to_string()).or_default() += 1;
                return;
            }
        };
        self.payloads += blobs.len();
        if blobs.len() > 1 {
            self.multi_payload += 1;
        }
        let Some(last) = blobs.last() else {
            *self
                .problems
                .entry("no XBCH payload".to_string())
                .or_default() += 1;
            return;
        };
        match Fssm::parse(last) {
            Ok(fssm) => {
                let wave = match fssm.wave_size() {
                    WaveSize::Wave32 => "wave32",
                    WaveSize::Wave64 => "wave64",
                };
                *self.wave.entry(wave).or_default() += 1;
                *self
                    .wave_by_stage
                    .entry(format!("{stage} {wave}"))
                    .or_default() += 1;
                let range = match fssm.code_range() {
                    CodeRange::Header { .. } => "header",
                    CodeRange::Scan { .. } => "scan",
                    CodeRange::NotFound => "not found",
                };
                *self.ranges.entry(range).or_default() += 1;
            }
            Err(error) => *self.problems.entry(error.to_string()).or_default() += 1,
        }
    }

    fn report(&self, stats: &CarveStats) -> String {
        let mut out = String::new();
        let w = &mut out;
        writeln!(
            w,
            "shaders.rpf: {} containers carved, {} candidates rejected",
            stats.carved, stats.rejected
        )
        .ok();
        writeln!(w, "stages: {}", counts(&self.stages)).ok();
        writeln!(w, "container parts: {}", counts(&self.parts)).ok();
        writeln!(
            w,
            "FSSM payloads: {} ({} container{} more than one; the last is used below)",
            self.payloads,
            self.multi_payload,
            if self.multi_payload == 1 {
                " has"
            } else {
                "s have"
            }
        )
        .ok();
        writeln!(w, "wave size by stage: {}", counts(&self.wave_by_stage)).ok();
        writeln!(w, "wave size: {}", counts(&self.wave)).ok();
        writeln!(w, "code range: {}", counts(&self.ranges)).ok();
        if self.problems.is_empty() {
            writeln!(w, "problems: none").ok();
        } else {
            writeln!(w, "problems:").ok();
            for (problem, count) in &self.problems {
                writeln!(w, "  {count} x {problem}").ok();
            }
        }
        out
    }
}

/// "a 1, b 2" from a map of counts.
fn counts<K: std::fmt::Display>(map: &BTreeMap<K, usize>) -> String {
    let parts: Vec<String> = map
        .iter()
        .map(|(key, count)| format!("{key} {count}"))
        .collect();
    if parts.is_empty() {
        "none".to_string()
    } else {
        parts.join(", ")
    }
}
