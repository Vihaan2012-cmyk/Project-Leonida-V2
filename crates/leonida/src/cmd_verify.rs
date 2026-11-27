use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap};
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use gpu::{
    BindingKind, Check, FirstStage, Gpu, TextureFormat, VertexAttribute, VertexBuffer, VertexFormat,
};
use shader_container::{
    CarvedShader, CodeRange, Fssm, Interface, Stage, WaveSize, carve, interface, parts, unpack_all,
};
use shader_ir::Function;
use shader_translate::images::{self, NUMBERS, Number, VIEWS};
use shader_translate::{
    EsOptions, FloatMode, PUSH_WORDS, PixelOptions, VertexInput, VertexOptions,
};

use crate::cli::VerifyArgs;
use crate::cmd_shaders::with_archive;
use crate::shader_build::{Built, build, build_first_half};

const ARRAY: u32 = 64;

/// The translator's descriptor set 1, with room for `ARRAY` textures and samplers of each kind.
fn bindings() -> Vec<(u32, BindingKind, u32)> {
    let mut list = vec![
        (images::TEXTURE_TABLE, BindingKind::Buffer, 1),
        (images::SAMPLER_TABLE, BindingKind::Buffer, 1),
        (images::SAMPLERS, BindingKind::Samplers, ARRAY),
        (images::COMPARE_SAMPLERS, BindingKind::Samplers, ARRAY),
    ];
    for view in VIEWS {
        for number in NUMBERS {
            list.push((images::binding(view, number), BindingKind::Textures, ARRAY));
            list.push((
                images::storage_binding(view, number),
                BindingKind::StorageTextures,
                ARRAY,
            ));
        }
        for wide in [false, true] {
            list.push((
                images::atomic_binding(view, wide),
                BindingKind::StorageTextures,
                ARRAY,
            ));
        }
    }
    list
}

fn lifted(source: &str) -> Result<Function> {
    let code = rdna::assemble(source, true).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    Ok(shader_ir::lift(&code, false)?)
}

/// The stages a checked shader is built with: a fragment shader that does nothing, a vertex shader writing
/// every parameter location, and first halves writing items of each size.
struct Partners {
    fragment: Vec<u32>,
    vertex: Vec<u32>,
    first_halves: HashMap<u32, Vec<u32>>,
}

impl Partners {
    fn new() -> Result<Partners> {
        let empty = PixelOptions {
            user_sgprs: 0,
            inputs: 0x2,
            outputs: Vec::new(),
            attribute_locations: Vec::new(),
            depth: None,
            float: FloatMode::default(),
        };
        let fragment = shader_translate::pixel(&lifted("s_endpgm")?, &empty)?;
        let mut source = String::from("v_mov_b32_e32 v1, 0\nexp pos0 v1, v1, v1, v1 done\n");
        for parameter in 0..32 {
            source += &format!("exp param{parameter} v1, v1, v1, v1\n");
        }
        source += "s_endpgm";
        let options = VertexOptions {
            user_sgprs: 0,
            inputs: Vec::new(),
            float: FloatMode::default(),
        };
        let vertex = shader_translate::vertex(&lifted(&source)?, &options)?;
        Ok(Partners {
            fragment,
            vertex,
            first_halves: HashMap::new(),
        })
    }

    fn first_half(&mut self, item_bytes: u32) -> Result<&[u32]> {
        if let Entry::Vacant(slot) = self.first_halves.entry(item_bytes) {
            let options = EsOptions {
                user_sgprs: 0,
                inputs: Vec::new(),
                item_bytes,
                float: FloatMode::default(),
            };
            slot.insert(shader_translate::es(&lifted("s_endpgm")?, &options)?);
        }
        Ok(&self.first_halves[&item_bytes])
    }
}

fn vertex_buffer(inputs: &[VertexInput]) -> VertexBuffer {
    VertexBuffer {
        data: Vec::new(),
        stride: 16,
        attributes: inputs
            .iter()
            .map(|input| VertexAttribute {
                location: u32::from(input.register),
                format: match input.number {
                    Number::Float => VertexFormat::Float32x4,
                    Number::Uint => VertexFormat::Uint32x4,
                    Number::Sint => VertexFormat::Sint32x4,
                },
                offset: 0,
            })
            .collect(),
    }
}

fn target_format(number: Number) -> TextureFormat {
    match number {
        Number::Float => TextureFormat::Rgba32Float,
        Number::Uint => TextureFormat::Rgba32Uint,
        Number::Sint => TextureFormat::Rgba32Sint,
    }
}

/// Builds the pipeline a translated shader belongs in, paired with `partners` where it needs other stages.
fn check(gpu: &Gpu, partners: &mut Partners, built: &Built) -> Result<()> {
    let bindings = bindings();
    let push = PUSH_WORDS * 4;
    let draw =
        |gpu: &Gpu, first: FirstStage<'_>, fragment: &[u32], targets: &[TextureFormat], depth| {
            let work = Check::Draw {
                first,
                fragment,
                targets,
                depth,
            };
            gpu.check(work, &bindings, push)
        };
    match built {
        Built::Compute(spirv) => gpu.check(
            Check::Compute {
                spirv,
                subgroup_size: Some(32),
            },
            &bindings,
            push,
        )?,
        Built::Pixel(spirv, options) => {
            let count = options
                .outputs
                .iter()
                .map(|o| o.target + 1)
                .max()
                .unwrap_or(0);
            let mut targets = vec![TextureFormat::Rgba32Float; usize::from(count)];
            for output in &options.outputs {
                targets[usize::from(output.target)] = target_format(output.number);
            }
            let depth = options.depth.map(|_| TextureFormat::Depth32Float);
            let first = FirstStage::Vertex {
                spirv: &partners.vertex,
                buffer: None,
                geometry: None,
            };
            draw(gpu, first, spirv, &targets, depth)?
        }
        Built::Vertex(spirv, options) => {
            let buffer = vertex_buffer(&options.inputs);
            let first = FirstStage::Vertex {
                spirv,
                buffer: Some(&buffer),
                geometry: None,
            };
            draw(gpu, first, &partners.fragment, &[], None)?
        }
        Built::FirstHalf(spirv, options) => {
            let buffer = vertex_buffer(&options.inputs);
            let first = FirstStage::Vertex {
                spirv,
                buffer: Some(&buffer),
                geometry: None,
            };
            draw(gpu, first, &partners.fragment, &[], None)?
        }
        Built::Mesh(spirv) => {
            let first = FirstStage::Mesh {
                spirv,
                groups: [1, 1, 1],
            };
            draw(gpu, first, &partners.fragment, &[], None)?
        }
        Built::Geometry(spirv, options) => {
            let fragment = partners.fragment.clone();
            let first_half = partners.first_half(options.item_bytes)?.to_vec();
            let first = FirstStage::Vertex {
                spirv: &first_half,
                buffer: None,
                geometry: Some(spirv),
            };
            draw(gpu, first, &fragment, &[], None)?
        }
    }
    Ok(())
}

fn kind(built: &Built) -> &'static str {
    match built {
        Built::Compute(_) => "compute",
        Built::Pixel(..) => "pixel",
        Built::Vertex(..) => "vertex",
        Built::Mesh(_) => "mesh",
        Built::Geometry(..) => "geometry",
        Built::FirstHalf(..) => "vertex half of geometry",
    }
}

#[derive(Default)]
struct Tally {
    translated: usize,
    words: usize,
    passed: usize,
    translate_time: Duration,
    pipeline_time: Duration,
}

#[derive(Default)]
struct Verify {
    each: bool,
    save: Option<PathBuf>,
    replacement: Option<Vec<u32>>,
    no_build: bool,
    kinds: BTreeMap<&'static str, Tally>,
    failures: BTreeMap<String, (usize, String)>,
    containers: usize,
}

impl Verify {
    fn fail(&mut self, error: String, name: &str) {
        println!("failed: {name}: {error}");
        let entry = self.failures.entry(error).or_default();
        entry.0 += 1;
        if entry.1.is_empty() {
            entry.1 = name.to_string();
        }
    }

    fn program(
        &mut self,
        gpu: &Gpu,
        partners: &mut Partners,
        name: &str,
        blob: &[u8],
        translate: impl FnOnce(&Fssm, &Function) -> Result<Built, String>,
    ) {
        let started = Instant::now();
        let built = Fssm::parse(blob)
            .map_err(|e| e.to_string())
            .and_then(|fssm| {
                let CodeRange::Header { start, end } = fssm.code_range() else {
                    return Err("no code range in the header".to_string());
                };
                let wave64 = fssm.wave_size() == WaveSize::Wave64;
                let code = blob.get(start..end).ok_or("code past the blob")?;
                let function = shader_ir::lift(code, wave64).map_err(|e| e.to_string())?;
                translate(&fssm, &function)
            });
        let translate_time = started.elapsed();
        let mut built = match built {
            Ok(built) => built,
            Err(error) => return self.fail(format!("translation: {error}"), name),
        };
        if let Some(folder) = &self.save {
            let bytes: Vec<u8> = built.spirv().iter().flat_map(|w| w.to_le_bytes()).collect();
            let path = folder.join(format!("{name}.{}.spv", kind(&built).replace(' ', "_")));
            if let Err(error) = std::fs::write(&path, bytes) {
                println!("could not write {}: {error}", path.display());
            }
        }
        if let Some(words) = &self.replacement {
            *built.spirv_mut() = words.clone();
        }
        if self.no_build {
            if self.each {
                println!("{name}: {} {} KiB", kind(&built), built.spirv().len() / 256);
            }
            return;
        }
        let tally = self.kinds.entry(kind(&built)).or_default();
        tally.translated += 1;
        tally.words += built.spirv().len();
        tally.translate_time += translate_time;
        let errors = gpu::validation_errors();
        let started = Instant::now();
        let result = check(gpu, partners, &built);
        let took = started.elapsed();
        if self.each {
            println!(
                "{name}: {} {} KiB, {:.2} s",
                kind(&built),
                built.spirv().len() / 256,
                took.as_secs_f64()
            );
        }
        let tally = self.kinds.entry(kind(&built)).or_default();
        tally.pipeline_time += took;
        let new_errors = gpu::validation_errors() - errors;
        match result {
            Err(error) => self.fail(format!("{} pipeline: {error}", kind(&built)), name),
            Ok(()) if new_errors > 0 => self.fail(
                format!("{} pipeline: validation errors", kind(&built)),
                name,
            ),
            Ok(()) => self.kinds.entry(kind(&built)).or_default().passed += 1,
        }
    }

    fn print(&self) {
        println!("containers: {}", self.containers);
        for (kind, tally) in &self.kinds {
            let per =
                |total: Duration| total.as_secs_f64() * 1000.0 / tally.translated.max(1) as f64;
            println!(
                "{kind}: {} translated, {} built into pipelines; {} KiB of SPIR-V, {:.1} ms to translate and {:.1} ms to build each",
                tally.translated,
                tally.passed,
                tally.words * 4 / 1024 / tally.translated.max(1),
                per(tally.translate_time),
                per(tally.pipeline_time)
            );
        }
        let failed: usize = self.failures.values().map(|(count, _)| count).sum();
        println!("failures: {failed}");
        for (error, (count, example)) in &self.failures {
            println!("  {count:>6}  {error}   e.g. {example}");
        }
    }
}

impl Verify {
    fn container(&mut self, gpu: &Gpu, partners: &mut Partners, shader: &CarvedShader) {
        let found: Option<Interface> = parts(&shader.dxbc)
            .ok()
            .and_then(|list| interface(&list, None).ok());
        let Ok(blobs) = unpack_all(&shader.dxbc) else {
            return self.fail("unreadable container".into(), &shader.name);
        };
        if shader.stage == Some(Stage::Vertex) && blobs.len() == 2 {
            self.program(gpu, partners, &shader.name, &blobs[0], |fssm, function| {
                build_first_half(fssm, found.as_ref(), function)
            });
        }
        if let Some(blob) = blobs.last() {
            self.program(gpu, partners, &shader.name, blob, |fssm, function| {
                build(shader.stage, fssm, found.as_ref(), function)
            });
        }
    }

    fn merge(&mut self, other: Verify) {
        self.containers += other.containers;
        for (kind, tally) in other.kinds {
            let mine = self.kinds.entry(kind).or_default();
            mine.translated += tally.translated;
            mine.passed += tally.passed;
            mine.words += tally.words;
            mine.translate_time += tally.translate_time;
            mine.pipeline_time += tally.pipeline_time;
        }
        for (error, (count, example)) in other.failures {
            let entry = self.failures.entry(error).or_default();
            entry.0 += count;
            if entry.1.is_empty() {
                entry.1 = example;
            }
        }
    }
}

pub fn verify_command(args: &VerifyArgs) -> Result<ExitCode> {
    let replacement = match &args.use_spirv {
        Some(path) => {
            let bytes =
                std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
            Some(
                bytes
                    .chunks_exact(4)
                    .map(|w| u32::from_le_bytes(w.try_into().expect("4 bytes")))
                    .collect::<Vec<u32>>(),
            )
        }
        None => None,
    };
    if let Some(folder) = &args.save_spirv {
        std::fs::create_dir_all(folder)
            .with_context(|| format!("creating {}", folder.display()))?;
    }
    let gpu = Gpu::new().context("opening the GPU")?;
    let workers = args.jobs.max(1);
    let started = Instant::now();
    let (sender, receiver) = mpsc::sync_channel::<CarvedShader>(4 * workers);
    let receiver = Mutex::new(receiver);
    let report = std::thread::scope(|scope| -> Result<Verify> {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| -> Result<Verify> {
                    let mut partners = Partners::new()?;
                    let mut verify = Verify {
                        each: args.each,
                        save: args.save_spirv.clone(),
                        replacement: replacement.clone(),
                        no_build: args.no_build,
                        ..Verify::default()
                    };
                    loop {
                        let next = receiver.lock().expect("no worker panicked").recv();
                        let Ok(shader) = next else {
                            return Ok(verify);
                        };
                        verify.containers += 1;
                        verify.container(&gpu, &mut partners, &shader);
                    }
                })
            })
            .collect();
        with_archive(args.game.as_ref(), |archive| {
            let mut sent = 0;
            carve(archive, |shader| {
                if args.limit.is_some_and(|limit| sent >= limit) {
                    return ControlFlow::Break(());
                }
                let stage = shader.stage.map(|s| s.short_name());
                if args.stage.is_some() && args.stage != stage {
                    return ControlFlow::Continue(());
                }
                if args
                    .name
                    .as_ref()
                    .is_some_and(|name| !shader.name.contains(name.as_str()))
                {
                    return ControlFlow::Continue(());
                }
                sent += 1;
                if sent % 2000 == 0 {
                    println!("progress: {sent} containers sent");
                }
                match sender.send(shader) {
                    Ok(()) => ControlFlow::Continue(()),
                    Err(_) => ControlFlow::Break(()),
                }
            });
            Ok(())
        })?;
        drop(sender);
        let mut total = Verify::default();
        for handle in handles {
            total.merge(handle.join().expect("worker finished")?);
        }
        Ok(total)
    })?;
    report.print();
    println!("took {:.1} s", started.elapsed().as_secs_f64());
    Ok(if report.failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
