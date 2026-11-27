//! What `leonida shaders verify` and `leonida pipelines` share: carving containers out of shaders.rpf for a set
//! of workers, and building a translated program into a pipeline with the stages it needs beside it.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};

use anyhow::Result;
use gpu::{
    BindingKind, Check, FirstStage, Gpu, TextureFormat, VertexAttribute, VertexBuffer, VertexFormat,
};
use shader_container::{CarvedShader, carve};
use shader_ir::Function;
use shader_store::Built;
use shader_translate::images::{self, NUMBERS, Number, VIEWS};
use shader_translate::{
    EsOptions, FloatMode, PUSH_WORDS, PixelOptions, VertexInput, VertexOptions,
};

use crate::cmd_shaders::with_archive;

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
pub struct Partners {
    fragment: Vec<u32>,
    vertex: Vec<u32>,
    first_halves: HashMap<u32, Vec<u32>>,
}

impl Partners {
    pub fn new() -> Result<Partners> {
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

/// How every distinct program built so far went, by fingerprint; shared by the workers.
pub type Outcomes = Arc<Mutex<HashMap<u64, Result<(), String>>>>;

/// Builds the pipeline for `built` unless an identical program was built before. The second value is true when an
/// earlier outcome was reused.
pub fn build_pipeline(
    gpu: &Gpu,
    partners: &mut Partners,
    built: &Built,
    outcomes: &Outcomes,
) -> (Result<(), String>, bool) {
    let fingerprint = built.fingerprint();
    let earlier = outcomes
        .lock()
        .expect("no worker panicked")
        .get(&fingerprint)
        .cloned();
    if let Some(outcome) = earlier {
        return (outcome, true);
    }
    let errors = gpu::validation_errors();
    let outcome = match check(gpu, partners, built) {
        Err(error) => Err(format!("{} pipeline: {error}", built.kind())),
        Ok(()) if gpu::validation_errors() > errors => {
            Err(format!("{} pipeline: validation errors", built.kind()))
        }
        Ok(()) => Ok(()),
    };
    outcomes
        .lock()
        .expect("no worker panicked")
        .insert(fingerprint, outcome.clone());
    (outcome, false)
}

/// Which containers of shaders.rpf to work on.
#[derive(Default)]
pub struct Selection<'a> {
    pub game: Option<&'a PathBuf>,
    pub limit: Option<usize>,
    /// ps, vs, gs, ms or cs.
    pub stage: Option<&'a str>,
    pub name: Option<&'a str>,
}

/// Carves the selected containers out of shaders.rpf and hands each to one of `workers` threads, each with a
/// state of its own from `make`. Returns the states when every container has been worked on.
pub fn for_each_container<W: Send>(
    selection: &Selection<'_>,
    workers: usize,
    make: impl Fn() -> Result<W> + Sync,
    work: impl Fn(&mut W, &CarvedShader) + Sync,
) -> Result<Vec<W>> {
    let workers = workers.max(1);
    let (sender, receiver) = mpsc::sync_channel::<CarvedShader>(4 * workers);
    let receiver = Arc::new(Mutex::new(receiver));
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                let receiver = Arc::clone(&receiver);
                let (make, work) = (&make, &work);
                scope.spawn(move || -> Result<W> {
                    let mut state = make()?;
                    loop {
                        let next = receiver.lock().expect("no worker panicked").recv();
                        let Ok(shader) = next else {
                            return Ok(state);
                        };
                        work(&mut state, &shader);
                    }
                })
            })
            .collect();
        drop(receiver);
        let carved = with_archive(selection.game, |archive| {
            let mut sent = 0;
            carve(archive, |shader| {
                if selection.limit.is_some_and(|limit| sent >= limit) {
                    return ControlFlow::Break(());
                }
                let stage = shader.stage.map(|s| s.short_name());
                if selection.stage.is_some() && selection.stage != stage.as_deref() {
                    return ControlFlow::Continue(());
                }
                if selection
                    .name
                    .is_some_and(|name| !shader.name.contains(name))
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
        });
        drop(sender);
        let mut states = Vec::new();
        let mut failure = None;
        for handle in handles {
            match handle.join().expect("worker finished") {
                Ok(state) => states.push(state),
                Err(error) => failure = failure.or(Some(error)),
            }
        }
        carved?;
        match failure {
            Some(error) => Err(error),
            None => Ok(states),
        }
    })
}
