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
    emitted: usize,
    compute_entries: usize,
    pixel_entries: usize,
    vertex_entries: usize,
    mesh_entries: usize,
    first_halves: usize,
    geometry_entries: usize,
    instructions: usize,
    unsupported: usize,
    missing: BTreeMap<String, (usize, usize, String, String)>,
    lift_errors: BTreeMap<String, usize>,
    examples: bool,
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
        if coverage.unsupported.is_empty() && coverage.invalid_words == 0 {
            self.complete += 1;
            match shader_ir::lift(code, wave64) {
                Ok(function) if function.verify().is_ok() => {
                    self.lifted += 1;
                    match (stage, fssm.compute_setup()) {
                        (Some(Stage::Pixel), _) => {
                            let result = pixel_options(&fssm, found)
                                .ok_or_else(|| "no pixel setup in the header".to_string())
                                .and_then(|options| {
                                    shader_translate::pixel(&function, &options)
                                        .map_err(|e| e.to_string())
                                });
                            self.count(result, |r| &mut r.pixel_entries);
                        }
                        (Some(Stage::Vertex), _) => {
                            let options = vertex_options(&fssm, found);
                            let result = shader_translate::vertex(&function, &options)
                                .map_err(|e| e.to_string());
                            self.count(result, |r| &mut r.vertex_entries);
                        }
                        (Some(Stage::Geometry), _) => {
                            let result = geometry_options(&fssm, found).and_then(|options| {
                                shader_translate::gs(&function, &options).map_err(|e| e.to_string())
                            });
                            self.count(result, |r| &mut r.geometry_entries);
                        }
                        (Some(Stage::Mesh), _) => {
                            let result = mesh_options(&fssm, found).and_then(|options| {
                                shader_translate::mesh(&function, &options)
                                    .map_err(|e| e.to_string())
                            });
                            self.count(result, |r| &mut r.mesh_entries);
                        }
                        (Some(Stage::Compute), Some((rsrc2, group))) => {
                            let options = shader_translate::ComputeOptions::from_rsrc2(
                                rsrc2,
                                group,
                                shader_translate::FloatMode::default(),
                            );
                            match shader_translate::compute(&function, &options) {
                                Ok(_) => self.compute_entries += 1,
                                Err(error) => {
                                    *self.lift_errors.entry(error.to_string()).or_default() += 1
                                }
                            }
                        }
                        _ => {
                            let options = shader_translate::HarnessOptions {
                                lds_words: 4096,
                                nan_preserve: false,
                                flush_denormals: false,
                                preserve_denormals: false,
                                code_base: 0,
                            };
                            self.emitted +=
                                usize::from(shader_translate::harness(&function, &options).is_ok());
                        }
                    }
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
        let result = shader_ir::lift(code, wave64)
            .map_err(|e| e.to_string())
            .and_then(|function| {
                shader_translate::es(&function, &first_half_options(&fssm, found))
                    .map_err(|e| e.to_string())
            });
        self.count(result, |r| &mut r.first_halves);
    }

    fn count<T>(&mut self, result: Result<T, String>, entries: impl Fn(&mut Self) -> &mut usize) {
        match result {
            Ok(_) => *entries(self) += 1,
            Err(error) => *self.lift_errors.entry(error).or_default() += 1,
        }
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
            "shaders the translator fully handles: {} ({:.1}%), lifted and checked: {}, turned into test-form SPIR-V: {}, into real compute shaders: {}, pixel shaders: {}, vertex shaders: {}, mesh shaders: {}, geometry shaders: {} (with {} vertex halves)",
            self.complete,
            percent(self.complete, self.shaders),
            self.lifted,
            self.emitted,
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

/// The fragment entry's inputs from the header and its colour targets from the output signature.
fn pixel_options(fssm: &Fssm, found: Option<&Interface>) -> Option<shader_translate::PixelOptions> {
    let setup = fssm.pixel_setup()?;
    let outputs = found
        .map(|f| f.outputs.as_slice())
        .unwrap_or_default()
        .iter()
        .filter(|element| element.system_value == 64 && element.register < 8)
        .map(|element| shader_translate::PixelOutput {
            target: element.register as u8,
            number: number(element),
        })
        .collect();
    Some(shader_translate::PixelOptions {
        user_sgprs: setup.user_sgprs,
        inputs: setup.inputs,
        outputs,
        depth: shader_translate::DepthOutput::from_control(setup.depth_control),
        float: shader_translate::FloatMode::default(),
    })
}

fn number(element: &SignatureElement) -> shader_translate::images::Number {
    use shader_translate::images::Number;
    match element.component_type {
        1 => Number::Uint,
        2 => Number::Sint,
        _ => Number::Float,
    }
}

/// The vertex entry's fetched attributes: every input-signature element that is not a system value.
fn vertex_options(fssm: &Fssm, found: Option<&Interface>) -> shader_translate::VertexOptions {
    let inputs = found
        .map(|f| f.inputs.as_slice())
        .unwrap_or_default()
        .iter()
        .filter(|element| element.system_value == 0 && element.register < 16)
        .map(|element| shader_translate::VertexInput {
            register: element.register as u8,
            number: number(element),
        })
        .collect();
    shader_translate::VertexOptions {
        user_sgprs: fssm.user_sgprs(),
        inputs,
        float: shader_translate::FloatMode::default(),
    }
}

fn signature_bytes(elements: &[SignatureElement]) -> u32 {
    elements
        .iter()
        .map(|element| element.register + 1)
        .max()
        .unwrap_or(0)
        * 16
}

/// The first half's fetched attributes as for a vertex shader, and one LDS item of 16 bytes per output register.
fn first_half_options(fssm: &Fssm, found: Option<&Interface>) -> shader_translate::EsOptions {
    let vertex = vertex_options(fssm, found);
    shader_translate::EsOptions {
        user_sgprs: vertex.user_sgprs,
        inputs: vertex.inputs,
        item_bytes: signature_bytes(found.map(|f| f.outputs.as_slice()).unwrap_or_default()),
        float: vertex.float,
    }
}

/// The geometry entry's primitives from the pipeline-state part and its vertex sizes from the signatures.
fn geometry_options(
    fssm: &Fssm,
    found: Option<&Interface>,
) -> Result<shader_translate::GsOptions, String> {
    use shader_translate::{GsInput, GsOutput};
    let found = found.ok_or("no geometry signatures")?;
    let info = found
        .pipeline
        .as_ref()
        .and_then(|p| p.geometry)
        .ok_or("no geometry pipeline state")?;
    let input = match info.input_primitive {
        1 => GsInput::Points,
        2 => GsInput::Lines,
        3 => GsInput::Triangles,
        6 => GsInput::LinesAdjacency,
        7 => GsInput::TrianglesAdjacency,
        _ => return Err("geometry shader input that is not points, lines or triangles".into()),
    };
    let output = match info.output_topology {
        1 => GsOutput::Points,
        3 => GsOutput::LineStrip,
        5 => GsOutput::TriangleStrip,
        _ => return Err("geometry shader output that is not points, lines or triangles".into()),
    };
    if info.stream_mask > 1 {
        return Err("geometry output to a stream past the first".into());
    }
    Ok(shader_translate::GsOptions {
        user_sgprs: fssm.user_sgprs(),
        input,
        item_bytes: signature_bytes(&found.inputs),
        output,
        max_vertices: u32::from(info.max_vertices),
        vertex_bytes: signature_bytes(&found.outputs),
        position: found
            .outputs
            .iter()
            .find(|element| element.system_value == 1)
            .map(|element| element.register as u8),
        float: shader_translate::FloatMode::default(),
    })
}

/// The mesh entry's group size, output counts and shared memory from the pipeline-state part.
fn mesh_options(
    fssm: &Fssm,
    found: Option<&Interface>,
) -> Result<shader_translate::MeshOptions, String> {
    let pipeline = found
        .and_then(|f| f.pipeline.as_ref())
        .ok_or("no mesh pipeline state")?;
    let (Some([x, y, z]), Some(mesh)) = (pipeline.threads, pipeline.mesh) else {
        return Err("no mesh pipeline state".into());
    };
    if mesh.topology != 2 {
        return Err("mesh shader drawing lines".into());
    }
    Ok(shader_translate::MeshOptions {
        user_sgprs: fssm.user_sgprs(),
        group_size: x * y * z,
        max_vertices: u32::from(mesh.max_vertices),
        max_primitives: u32::from(mesh.max_primitives),
        lds_bytes: mesh.shared_bytes,
        float: shader_translate::FloatMode::default(),
    })
}

pub fn report_command(args: &ReportArgs) -> Result<ExitCode> {
    with_archive(args.game.as_ref(), |archive| {
        let mut report = Report {
            examples: args.examples,
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
