use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::ops::ControlFlow;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use game_package::{Manifest, map_file, package_path};
use shader_container::{CodeRange, Fssm, ScanSummary, WaveSize, carve, scan::scan, unpack_all};

use crate::cli::{DisasmArgs, ReportArgs, ScanArgs};
use crate::game::resolve_game;

fn with_archive<T>(
    game: Option<&std::path::PathBuf>,
    work: impl FnOnce(&[u8]) -> Result<T>,
) -> Result<T> {
    let game = resolve_game(game, &Manifest::builtin())?;
    let path = package_path(&game, "shaders.rpf");
    let file = File::open(&path).with_context(|| format!("opening {}", path.display()))?;
    let archive = map_file(&file).with_context(|| format!("mapping {}", path.display()))?;
    work(&archive)
}

pub fn scan_command(args: &ScanArgs) -> Result<ExitCode> {
    with_archive(args.game.as_ref(), |archive| {
        let result = scan(archive, |_| ControlFlow::Continue(()));
        print!("{}", ScanSummary::of(&result).report());
        Ok(if result.stats.carved == 0 {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        })
    })
}

pub fn disasm_command(args: &DisasmArgs) -> Result<ExitCode> {
    with_archive(args.game.as_ref(), |archive| {
        let mut container = None;
        carve(archive, |shader| {
            if shader.name == args.name {
                container = Some(shader.dxbc);
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        });
        let Some(dxbc) = container else {
            bail!("no shader named {} in shaders.rpf", args.name);
        };
        let blobs = unpack_all(&dxbc)?;
        let Some(blob) = blobs.last() else {
            bail!("{} has no GPU program", args.name);
        };
        print_program(Path::new(&args.name), blob)
    })
}

fn print_program(name: &Path, blob: &[u8]) -> Result<ExitCode> {
    let fssm = Fssm::parse(blob)?;
    let CodeRange::Header { start, end } = fssm.code_range() else {
        bail!(
            "could not find where {}'s instructions start",
            name.display()
        );
    };
    let wave32 = fssm.wave_size() == WaveSize::Wave32;
    let (listing, invalid) = rdna::disassemble(&blob[start..end], wave32, start);
    println!(
        "; {}  code 0x{start:x}..0x{end:x}, {}, {invalid} invalid words",
        name.display(),
        if wave32 { "wave32" } else { "wave64" }
    );
    print!("{listing}");
    Ok(if invalid == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

#[derive(Default)]
struct Report {
    shaders: usize,
    wave64: usize,
    unreadable: usize,
    invalid: usize,
    complete: usize,
    lifted: usize,
    emitted: usize,
    instructions: usize,
    unsupported: usize,
    missing: BTreeMap<String, (usize, usize, String)>,
    lift_errors: BTreeMap<String, usize>,
}

impl Report {
    fn add(&mut self, name: &str, blob: &[u8]) -> Option<()> {
        let fssm = Fssm::parse(blob).ok()?;
        let CodeRange::Header { start, end } = fssm.code_range() else {
            return None;
        };
        let wave64 = fssm.wave_size() == WaveSize::Wave64;
        let code = blob.get(start..end)?;
        self.shaders += 1;
        self.wave64 += usize::from(wave64);
        let coverage = shader_ir::coverage(code, wave64);
        self.instructions += coverage.instructions;
        self.unsupported += coverage.unsupported.len();
        self.invalid += usize::from(coverage.invalid_words > 0);
        for name in &coverage.unsupported {
            self.missing.entry(name.clone()).or_default().1 += 1;
        }
        for missing in coverage.unsupported.iter().collect::<BTreeSet<_>>() {
            let entry = self.missing.entry(missing.clone()).or_default();
            entry.0 += 1;
            if entry.2.is_empty() {
                entry.2 = name.to_string();
            }
        }
        if coverage.unsupported.is_empty() && coverage.invalid_words == 0 {
            self.complete += 1;
            match shader_ir::lift(code, wave64) {
                Ok(function) if function.verify().is_ok() => {
                    self.lifted += 1;
                    let options = shader_translate::HarnessOptions {
                        lds_words: 4096,
                        nan_preserve: false,
                        flush_denormals: false,
                        preserve_denormals: false,
                    };
                    self.emitted +=
                        usize::from(shader_translate::harness(&function, &options).is_ok());
                }
                Ok(_) => {
                    *self
                        .lift_errors
                        .entry("does not verify".into())
                        .or_default() += 1
                }
                Err(error) => *self.lift_errors.entry(error.to_string()).or_default() += 1,
            }
        }
        Some(())
    }

    fn print(&self, top: usize) {
        let percent = |part: usize, whole: usize| 100.0 * part as f64 / whole.max(1) as f64;
        println!(
            "shaders: {} ({} wave64, {} wave32), {} unreadable",
            self.shaders,
            self.wave64,
            self.shaders - self.wave64,
            self.unreadable
        );
        println!(
            "instructions: {}, of which the translator handles {:.1}%",
            self.instructions,
            percent(self.instructions - self.unsupported, self.instructions)
        );
        println!("shaders with undecodable words: {}", self.invalid);
        println!(
            "shaders the translator fully handles: {} ({:.1}%), lifted and checked: {}, turned into SPIR-V: {}",
            self.complete,
            percent(self.complete, self.shaders),
            self.lifted,
            self.emitted
        );
        for (error, count) in &self.lift_errors {
            println!("  {count} stopped at: {error}");
        }
        let mut missing: Vec<_> = self.missing.iter().collect();
        missing.sort_by(|a, b| (b.1.0, b.1.1).cmp(&(a.1.0, a.1.1)).then(a.0.cmp(b.0)));
        println!(
            "missing instructions ({} kinds), by shaders that need them:",
            missing.len()
        );
        for (name, (shaders, uses, example)) in missing.into_iter().take(top) {
            println!("  {name:<32} {shaders:>6} shaders {uses:>9} uses   e.g. {example}");
        }
    }
}

pub fn report_command(args: &ReportArgs) -> Result<ExitCode> {
    with_archive(args.game.as_ref(), |archive| {
        let mut report = Report::default();
        carve(archive, |shader| {
            let added = unpack_all(&shader.dxbc)
                .ok()
                .and_then(|blobs| blobs.last().and_then(|blob| report.add(&shader.name, blob)));
            if added.is_none() {
                report.unreadable += 1;
            }
            ControlFlow::Continue(())
        });
        report.print(args.top);
        Ok(if report.shaders == 0 {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        })
    })
}
