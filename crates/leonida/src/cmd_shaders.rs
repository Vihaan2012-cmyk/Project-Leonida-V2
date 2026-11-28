use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::ops::ControlFlow;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use game_package::{Manifest, map_file, package_path};
use shader_container::{
    CodeRange, Fssm, Interface, ScanSummary, SignatureElement, Stage, WaveSize, carve, interface,
    parts, scan::scan, unpack_all,
};

use crate::cli::{DisasmArgs, ReportArgs, ScanArgs};
use crate::game::resolve_game;
use shader_ir::Pattern;
use shader_store::translate::{Built, build, build_first_half};

pub(crate) fn with_archive<T>(
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
        let mut seen = 0;
        carve(archive, |shader| {
            if shader.name == args.name {
                seen += 1;
                if seen < args.nth {
                    return ControlFlow::Continue(());
                }
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
        if let Ok(list) = parts(&dxbc)
            && let Ok(found) = interface(&list, None)
        {
            print_interface(&found);
        }
        print_program(Path::new(&args.name), blob)
    })
}

fn describe(element: &SignatureElement) -> String {
    let channels: String = "xyzw"
        .chars()
        .enumerate()
        .filter(|(i, _)| element.mask >> i & 1 != 0)
        .map(|(_, c)| c)
        .collect();
    let kind = match element.component_type {
        1 => "uint",
        2 => "int",
        3 => "float",
        _ => "?",
    };
    format!(
        "{}{} r{}.{channels} {kind}",
        element.semantic, element.semantic_index, element.register
    )
}

fn print_interface(found: &Interface) {
    let list = |elements: &[SignatureElement]| {
        elements.iter().map(describe).collect::<Vec<_>>().join(", ")
    };
    if !found.inputs.is_empty() {
        println!("; inputs: {}", list(&found.inputs));
    }
    if !found.outputs.is_empty() {
        println!("; outputs: {}", list(&found.outputs));
    }
    if let Some(pipeline) = &found.pipeline {
        if let Some([x, y, z]) = pipeline.threads {
            println!("; threads: {x}x{y}x{z}");
        }
        if let Some(mesh) = pipeline.mesh {
            println!(
                "; mesh: up to {} vertices and {} primitives, topology {}, payload {} bytes, shared {} bytes",
                mesh.max_vertices,
                mesh.max_primitives,
                mesh.topology,
                mesh.payload_bytes,
                mesh.shared_bytes
            );
        }
    }
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
    compute_entries: usize,
    pixel_entries: usize,
    vertex_entries: usize,
    mesh_entries: usize,
    first_halves: usize,
    geometry_entries: usize,
    narrow_by_stage: [usize; 3],
    approximate: BTreeMap<&'static str, [usize; 3]>,
    approximate_shaders: [usize; 3],
    hazard_examples: BTreeMap<&'static str, Vec<String>>,
    patterns: BTreeMap<Pattern, ([usize; 3], String)>,
    raised_without_hazard: [usize; 3],
    wide_ops: BTreeMap<String, ([usize; 3], String)>,
    instructions: usize,
    unsupported: usize,
    missing: BTreeMap<String, (usize, usize, String, String)>,
    lift_errors: BTreeMap<String, usize>,
    examples: bool,
    lanes_only: bool,
}

impl Report {
    fn add(
        &mut self,
        name: &str,
        stage: Option<Stage>,
        blob: &[u8],
        found: Option<&Interface>,
    ) -> Option<()> {
        let fssm = Fssm::parse(blob).ok()?;
        let CodeRange::Header { start, end } = fssm.code_range() else {
            return None;
        };
        let wave64 = fssm.wave_size() == WaveSize::Wave64;
        let code = blob.get(start..end)?;
        self.shaders += 1;
        self.wave64 += usize::from(wave64);
        let coverage = shader_ir::coverage(code, wave64);
        if wave64 {
            self.count_narrow(name, stage, code);
        }
        self.instructions += coverage.instructions;
        self.unsupported += coverage.unsupported.len();
        self.invalid += usize::from(coverage.invalid_words > 0);
        for name in &coverage.unsupported {
            self.missing.entry(name.clone()).or_default().1 += 1;
        }
        let mut seen = BTreeSet::new();
        for (missing, text) in coverage.unsupported.iter().zip(&coverage.texts) {
            if !seen.insert(missing) {
                continue;
            }
            let entry = self.missing.entry(missing.clone()).or_default();
            entry.0 += 1;
            if entry.2.is_empty() {
                entry.2 = name.to_string();
                entry.3 = text.clone();
            }
        }
        if coverage.unsupported.is_empty() && coverage.invalid_words == 0 && !self.lanes_only {
            self.complete += 1;
            match shader_ir::lift(code, wave64) {
                Ok(function) if function.verify().is_ok() => {
                    self.lifted += 1;
                    let result = build(stage, &fssm, found, &function);
                    self.tally(result);
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

    fn count_narrow(&mut self, name: &str, stage: Option<Stage>, code: &[u8]) {
        let slot = match stage {
            Some(Stage::Pixel) => 0,
            Some(Stage::Vertex) => 1,
            Some(Stage::Geometry) => 2,
            _ => return,
        };
        self.narrow_by_stage[slot] += 1;
        let hazards = shader_ir::upper_lane_hazards(code);
        self.approximate_shaders[slot] += usize::from(!hazards.is_empty());
        for hazard in hazards {
            self.approximate.entry(hazard.describe()).or_default()[slot] += 1;
            let examples = self.hazard_examples.entry(hazard.describe()).or_default();
            if examples.len() < if self.examples { 12 } else { 3 } {
                examples.push(name.to_string());
            }
        }
        let found = shader_ir::lane_patterns(code);
        if found.contains(&Pattern::ExecRaised) && !found.iter().any(|p| p.needs_upper_lanes()) {
            self.raised_without_hazard[slot] += 1;
        }
        for pattern in found {
            let entry = self.patterns.entry(pattern).or_default();
            entry.0[slot] += 1;
            if entry.1.is_empty() {
                entry.1 = name.to_string();
            }
        }
        for op in shader_ir::wide_scalar_ops(code) {
            let entry = self.wide_ops.entry(op).or_default();
            entry.0[slot] += 1;
            if entry.1.is_empty() {
                entry.1 = name.to_string();
            }
        }
    }

    /// The first half of a merged vertex-and-geometry shader, the extra program a vertex shader carries.
    fn add_first_half(&mut self, blob: &[u8], found: Option<&Interface>) {
        let Ok(fssm) = Fssm::parse(blob) else {
            return;
        };
        let CodeRange::Header { start, end } = fssm.code_range() else {
            return;
        };
        let wave64 = fssm.wave_size() == WaveSize::Wave64;
        let Some(code) = blob.get(start..end) else {
            return;
        };
        if wave64 {
            self.count_narrow("(first half)", Some(Stage::Vertex), code);
        }
        if self.lanes_only {
            return;
        }
        let result = shader_ir::lift(code, wave64)
            .map_err(|e| e.to_string())
            .and_then(|function| build_first_half(&fssm, found, &function));
        self.tally(result);
    }

    fn tally(&mut self, result: Result<Built, String>) {
        let entries = match result {
            Ok(Built::Compute(_)) => &mut self.compute_entries,
            Ok(Built::Pixel(..)) => &mut self.pixel_entries,
            Ok(Built::Vertex(..)) => &mut self.vertex_entries,
            Ok(Built::Mesh(_)) => &mut self.mesh_entries,
            Ok(Built::Geometry(..)) => &mut self.geometry_entries,
            Ok(Built::FirstHalf(..)) => &mut self.first_halves,
            Err(error) => {
                *self.lift_errors.entry(error).or_default() += 1;
                return;
            }
        };
        *entries += 1;
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
            "shaders the translator fully handles: {} ({:.1}%), lifted and checked: {}, turned into real compute shaders: {}, pixel shaders: {}, vertex shaders: {}, mesh shaders: {}, geometry shaders: {} (with {} vertex halves)",
            self.complete,
            percent(self.complete, self.shaders),
            self.lifted,
            self.compute_entries,
            self.pixel_entries,
            self.vertex_entries,
            self.mesh_entries,
            self.geometry_entries,
            self.first_halves
        );
        for (error, count) in &self.lift_errors {
            println!("  {count} stopped at: {error}");
        }
        let [np, nv, ng] = self.narrow_by_stage;
        let [ap, av, ag] = self.approximate_shaders;
        println!(
            "64-lane graphics programs, which run 32 lanes wide (pixel / vertex including merged first halves / geometry): {np} / {nv} / {ng}; of which name lanes 32-63 by number: {ap} / {av} / {ag}"
        );
        for (reason, [p, v, g]) in &self.approximate {
            let examples = self
                .hazard_examples
                .get(reason)
                .map(|names| names.join(", "));
            println!(
                "  {p} / {v} / {g} {reason}  e.g. {}",
                examples.unwrap_or_default()
            );
        }
        println!(
            "programs that switch on all lanes but name no lane past 31 (pixel / vertex / geometry): {} / {} / {}",
            self.raised_without_hazard[0],
            self.raised_without_hazard[1],
            self.raised_without_hazard[2]
        );
        println!("lane-mask patterns in those programs (pixel / vertex / geometry):");
        for (pattern, ([p, v, g], example)) in &self.patterns {
            let kind = if pattern.needs_upper_lanes() {
                "needs upper lanes"
            } else {
                "empty upper half is enough"
            };
            println!(
                "  {p} / {v} / {g} {} [{kind}]  e.g. {example}",
                pattern.describe()
            );
        }
        if self.examples {
            println!("64-bit scalar instructions in those programs (pixel / vertex / geometry):");
            for (op, ([p, v, g], example)) in &self.wide_ops {
                println!("  {p} / {v} / {g} {op}  e.g. {example}");
            }
        }
        let mut missing: Vec<_> = self.missing.iter().collect();
        missing.sort_by(|a, b| (b.1.0, b.1.1).cmp(&(a.1.0, a.1.1)).then(a.0.cmp(b.0)));
        println!(
            "missing instructions ({} kinds), by shaders that need them:",
            missing.len()
        );
        for (name, (shaders, uses, example, text)) in missing.into_iter().take(top) {
            println!("  {name:<32} {shaders:>6} shaders {uses:>9} uses   e.g. {example}");
            if self.examples {
                println!("      {text}");
            }
        }
    }
}

pub fn report_command(args: &ReportArgs) -> Result<ExitCode> {
    with_archive(args.game.as_ref(), |archive| {
        let mut report = Report {
            examples: args.examples,
            lanes_only: args.lanes_only,
            ..Report::default()
        };
        carve(archive, |shader| {
            let found = parts(&shader.dxbc)
                .ok()
                .and_then(|list| interface(&list, None).ok());
            let added = unpack_all(&shader.dxbc).ok().and_then(|blobs| {
                if shader.stage == Some(Stage::Vertex) && blobs.len() == 2 {
                    report.add_first_half(&blobs[0], found.as_ref());
                }
                blobs
                    .last()
                    .and_then(|blob| report.add(&shader.name, shader.stage, blob, found.as_ref()))
            });
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
