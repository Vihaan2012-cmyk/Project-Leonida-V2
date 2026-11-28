//! Choosing which of the game's shaders to time.

use std::fs::File;
use std::ops::ControlFlow;
use std::path::Path;

use anyhow::{Context, Result};
use game_package::{map_file, package_path};
use shader_container::{CodeRange, Fssm, Stage, WaveSize, carve};
use shader_store::{Role, Source, container_programs, key_hex};

pub struct Program {
    pub name: String,
    pub kind: &'static str,
    pub source: Source,
    pub code: Vec<u8>,
    pub wave64: bool,
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Every pixel and compute program in shaders.rpf, then the `compute` and `pixel` of them that give a spread of
/// sizes (the largest few first), or the ones whose names contain one of `names`.
pub fn pick(
    game: &Path,
    compute: usize,
    pixel: usize,
    names: &[String],
    max_code: usize,
    skip: &[String],
) -> Result<Vec<Program>> {
    let path = package_path(game, "shaders.rpf");
    let file = File::open(&path).with_context(|| format!("opening {}", path.display()))?;
    let archive = map_file(&file)?;
    let mut all: Vec<Program> = Vec::new();
    carve(&archive, |shader| {
        let kind = match shader.stage {
            Some(Stage::Compute) => "compute",
            Some(Stage::Pixel) => "pixel",
            _ => return ControlFlow::Continue(()),
        };
        let name = sanitize(&shader.name);
        if skip.iter().any(|s| name.contains(s.as_str())) {
            return ControlFlow::Continue(());
        }
        let Ok(sources) = container_programs(shader.stage, &shader.dxbc) else {
            return ControlFlow::Continue(());
        };
        for source in sources.into_iter().filter(|s| s.role == Role::Main) {
            let Ok(fssm) = Fssm::parse(&source.fssm) else {
                continue;
            };
            let CodeRange::Header { start, end } = fssm.code_range() else {
                continue;
            };
            let Some(code) = source.fssm.get(start..end) else {
                continue;
            };
            let wave64 = fssm.wave_size() == WaveSize::Wave64;
            if code.len() > max_code {
                continue;
            }
            let code = code.to_vec();
            let unique = format!("{name}.{}", &key_hex(&source.key())[..8]);
            if !names.is_empty() && !names.iter().any(|n| unique.contains(n.as_str())) {
                continue;
            }
            all.push(Program {
                name: unique,
                kind,
                source,
                code,
                wave64,
            });
        }
        ControlFlow::Continue(())
    });
    if !names.is_empty() {
        all.sort_by(|a, b| a.name.cmp(&b.name));
        let mut found = Vec::new();
        for pattern in names {
            let position = all.iter().position(|p| p.name.contains(pattern.as_str()));
            if let Some(position) = position {
                found.push(all.remove(position));
            }
        }
        return Ok(found);
    }
    let mut chosen = Vec::new();
    for (kind, count) in [("compute", compute), ("pixel", pixel)] {
        let mut of_kind: Vec<Program> = all
            .iter()
            .filter(|p| p.kind == kind)
            .map(|p| Program {
                name: p.name.clone(),
                kind: p.kind,
                source: p.source.clone(),
                code: p.code.clone(),
                wave64: p.wave64,
            })
            .collect();
        of_kind.sort_by_key(|p| std::cmp::Reverse(p.code.len()));
        let big = count.min(8) / 2;
        let rest = count - big;
        let tail: Vec<usize> = (big..of_kind.len()).collect();
        let mut picks: Vec<usize> = (0..big).collect();
        for k in 0..rest {
            if let Some(index) = tail.get(k * tail.len() / rest.max(1)) {
                picks.push(*index);
            }
        }
        picks.dedup();
        let mut taken: Vec<Option<Program>> = of_kind.into_iter().map(Some).collect();
        for index in picks {
            if let Some(program) = taken[index].take() {
                chosen.push(program);
            }
        }
    }
    Ok(chosen)
}
