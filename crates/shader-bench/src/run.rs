//! Running one program on the GPU, for its output and for its time.

use anyhow::{Context, Result, anyhow};
use gpu::{FirstStage, Gpu, Resource, Target, TextureFormat, Timing};
use shader_census::Analysis;
use shader_container::Fssm;
use shader_store::{Built, translate_built};
use shader_translate::images::Number;
use shader_translate::{ComputeOptions, FloatMode, PixelOptions, VertexOptions};

use crate::Settings;
use crate::compare::{Output, Verdict, compare, heap_diff};
use crate::inputs;
use crate::pick::Program;

const SCREEN: [u32; 2] = [2560, 1440];
const SMALL: [u32; 2] = [320, 180];
const TARGET_THREADS: u64 = 2560 * 1440;
const LONGEST_NANOS: f64 = 80e6;

fn fullscreen_vertex() -> Result<Vec<u32>> {
    let mut source = String::from(
        "v_and_b32_e32 v1, 1, v0\n\
         v_lshrrev_b32_e32 v2, 1, v0\n\
         v_cvt_f32_u32_e32 v1, v1\n\
         v_cvt_f32_u32_e32 v2, v2\n\
         v_fma_f32 v3, v1, 4.0, -1.0\n\
         v_fma_f32 v4, v2, 4.0, -1.0\n\
         v_mov_b32_e32 v5, 0\n\
         v_mov_b32_e32 v6, 1.0\n\
         exp pos0 v3, v4, v5, v6 done\n",
    );
    for parameter in 0..32 {
        source += &format!("exp param{parameter} v1, v2, v6, v6\n");
    }
    source += "s_endpgm";
    let code = rdna::assemble(&source, true).map_err(|e| anyhow!("{e:?}"))?;
    let function = shader_ir::lift(&code, false)?;
    let options = VertexOptions {
        user_sgprs: 0,
        inputs: Vec::new(),
        float: FloatMode::default(),
    };
    Ok(shader_translate::vertex(&function, &options)?)
}

enum Mode {
    Check,
    Time { repeats: u32, work: u64 },
}

struct Job<'a> {
    program: &'a Program,
    built: &'a Built,
    analysis: &'a Analysis,
    plain: u32,
    vertex: &'a [u32],
}

fn compute_options(program: &Program) -> Result<ComputeOptions> {
    let fssm = Fssm::parse(&program.source.fssm)?;
    let (rsrc2, group) = fssm.compute_setup().context("no compute setup")?;
    Ok(ComputeOptions::from_rsrc2(
        rsrc2,
        group,
        FloatMode::default(),
    ))
}

fn shape(options: &ComputeOptions, groups: u64) -> [u32; 3] {
    let groups = groups.max(1);
    let [x, y, z] = options.workgroup_ids;
    let side = |n: u64, per: u64| n.div_ceil(per).max(1) as u32;
    match (x, y, z) {
        (_, false, false) => [groups as u32, 1, 1],
        (_, true, false) => [64, side(groups, 64), 1],
        _ => [16, 16, side(groups, 256)],
    }
}

fn targets(options: &PixelOptions, extent: [u32; 2], exact: bool) -> (Vec<Target>, Option<Target>) {
    let count = options
        .outputs
        .iter()
        .map(|o| usize::from(o.target) + 1)
        .max()
        .unwrap_or(0);
    let mut formats = vec![Number::Float; count];
    for output in &options.outputs {
        formats[usize::from(output.target)] = output.number;
    }
    let make = |format: TextureFormat, clear: [u32; 4]| Target {
        format,
        clear,
        data: vec![0; (extent[0] * extent[1]) as usize * format.texel_bytes()],
    };
    let colours = formats
        .into_iter()
        .map(|number| match number {
            Number::Float if exact => make(TextureFormat::Rgba32Float, [0; 4]),
            Number::Float => make(TextureFormat::Rgba8Unorm, [0; 4]),
            Number::Uint => make(TextureFormat::Rgba32Uint, [0; 4]),
            Number::Sint => make(TextureFormat::Rgba32Sint, [0; 4]),
        })
        .collect();
    let depth = options
        .depth
        .map(|_| make(TextureFormat::Depth32Float, [1.0f32.to_bits(), 0, 0, 0]));
    (colours, depth)
}

fn exec(gpu: &Gpu, job: &Job<'_>, spirv: &[u32], mode: &Mode) -> Result<(Output, Vec<f64>)> {
    let program = job.program;
    let mut heap = inputs::heap(&program.code);
    let initial = heap.clone();
    let mut resources = inputs::resources(spirv);
    let repeats = match mode {
        Mode::Check => 1,
        Mode::Time { repeats, .. } => *repeats,
    };
    let mut timing = Timing {
        repeats,
        nanos: Vec::new(),
    };
    let mut output = Output::default();
    match job.built {
        Built::Compute(_) => {
            let options = compute_options(program)?;
            let threads: u64 = options.group_size.iter().map(|n| u64::from(*n)).product();
            let groups = match mode {
                Mode::Check => 64,
                Mode::Time { work, .. } => (*work).min(TARGET_THREADS.div_ceil(threads.max(1))),
            };
            let (analysis, plain, user) = (job.analysis, job.plain, options.user_sgprs);
            gpu.dispatch_timed(
                spirv,
                &mut [heap.as_mut_slice()],
                |addresses| inputs::push(analysis, user, addresses[0], plain),
                shape(&options, groups),
                Some(32),
                &mut resources,
                &mut timing,
            )?;
        }
        Built::Pixel(_, options) => {
            let extent = match mode {
                Mode::Check => SMALL,
                Mode::Time { .. } => SCREEN,
            };
            let (mut colours, mut depth) = targets(options, extent, matches!(mode, Mode::Check));
            let (analysis, plain, user) = (job.analysis, job.plain, options.user_sgprs);
            gpu.draw_timed(
                FirstStage::Vertex {
                    spirv: job.vertex,
                    buffer: None,
                    geometry: None,
                },
                spirv,
                extent,
                &mut colours,
                depth.as_mut(),
                &mut [heap.as_mut_slice()],
                |addresses| inputs::push(analysis, user, addresses[0], plain),
                &mut resources,
                &mut timing,
            )?;
            if matches!(mode, Mode::Check) {
                output.images.extend(colours.into_iter().map(|t| t.data));
                output.images.extend(depth.map(|t| t.data));
            }
        }
        _ => return Err(anyhow!("only compute and pixel shaders are timed")),
    }
    output.heap = heap_diff(&initial, &heap);
    for (_, resource) in &resources {
        if let Resource::StorageTextures(list) = resource {
            output.images.extend(list.iter().map(|t| t.data.clone()));
        }
    }
    Ok((output, timing.nanos))
}

fn median(nanos: &[f64], skip: usize) -> f64 {
    let mut kept: Vec<f64> = nanos.iter().skip(skip).copied().collect();
    if kept.is_empty() {
        return f64::NAN;
    }
    kept.sort_by(f64::total_cmp);
    kept[kept.len() / 2]
}

pub struct Row {
    pub name: String,
    pub kind: &'static str,
    pub words: [usize; 2],
    pub nanos: [f64; 2],
    pub verdict: Verdict,
}

fn bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

fn words(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks_exact(4)
        .map(|w| u32::from_le_bytes(w.try_into().expect("4 bytes")))
        .collect()
}

fn one(gpu: &Gpu, settings: &Settings, vertex: &[u32], program: &Program) -> Result<Row> {
    let translated = translate_built(&program.source).map_err(|f| anyhow!("{}", f.error))?;
    let built = translated.built;
    let current = built.spirv().to_vec();
    let file = format!("{}.spv", program.name);
    if let Some(folder) = &settings.save {
        std::fs::write(folder.join(&file), bytes(&current))?;
    }
    let base = match &settings.baseline {
        Some(folder) => words(
            &std::fs::read(folder.join(&file)).with_context(|| format!("no baseline {file}"))?,
        ),
        None => current.clone(),
    };
    let analysis = shader_census::analyze(&program.code, !program.wave64);
    let job = Job {
        program,
        built: &built,
        analysis: &analysis,
        plain: settings.plain,
        vertex,
    };
    let (a, _) = exec(gpu, &job, &base, &Mode::Check)?;
    let (b, _) = exec(gpu, &job, &current, &Mode::Check)?;
    let (again, _) = exec(gpu, &job, &base, &Mode::Check)?;
    let verdict = compare(&a, &b, &again);

    let skip = settings.repeats as usize / 4;
    let mut work = match built {
        Built::Compute(_) => 64,
        _ => 0,
    };
    if matches!(built, Built::Compute(_)) {
        let (_, probe) = exec(gpu, &job, &base, &Mode::Time { repeats: 4, work })?;
        let t = median(&probe, 1);
        let per_group = t / work as f64;
        work = ((LONGEST_NANOS / per_group.max(1.0)) as u64).clamp(64, 1 << 20);
    }
    let mut totals = [Vec::new(), Vec::new()];
    for _round in 0..3 {
        for (slot, spirv) in [&base, &current].into_iter().enumerate() {
            let (_, nanos) = exec(
                gpu,
                &job,
                spirv,
                &Mode::Time {
                    repeats: settings.repeats,
                    work,
                },
            )?;
            totals[slot].push(median(&nanos, skip));
        }
    }
    let best = |list: &[f64]| list.iter().copied().fold(f64::INFINITY, f64::min);
    Ok(Row {
        name: program.name.clone(),
        kind: program.kind,
        words: [base.len(), current.len()],
        nanos: [best(&totals[0]), best(&totals[1])],
        verdict,
    })
}

pub fn all(settings: &Settings, programs: Vec<Program>) -> Result<()> {
    let gpu = Gpu::new().context("opening the GPU")?;
    gpu.use_local_memory(true);
    println!("gpu: {}", gpu.info.name);
    let vertex = fullscreen_vertex()?;
    let mut rows = Vec::new();
    for program in &programs {
        println!(
            "running {} {} ({} code bytes)",
            program.kind,
            program.name,
            program.code.len()
        );
        match one(&gpu, settings, &vertex, program) {
            Ok(row) => {
                println!(
                    "  {:>8.3} ms -> {:>8.3} ms  x{:.3}  spirv {} -> {} words  {}",
                    row.nanos[0] / 1e6,
                    row.nanos[1] / 1e6,
                    row.nanos[0] / row.nanos[1],
                    row.words[0],
                    row.words[1],
                    row.verdict.text()
                );
                rows.push(row);
            }
            Err(error) => println!("  skipped: {error:#}"),
        }
    }
    for kind in ["compute", "pixel"] {
        let of: Vec<&Row> = rows.iter().filter(|r| r.kind == kind).collect();
        if of.is_empty() {
            continue;
        }
        let (before, after): (f64, f64) = of
            .iter()
            .fold((0.0, 0.0), |(a, b), r| (a + r.nanos[0], b + r.nanos[1]));
        let geo = of
            .iter()
            .map(|r| (r.nanos[0] / r.nanos[1]).ln())
            .sum::<f64>()
            / of.len() as f64;
        println!(
            "{kind}: {} shaders, total {:.3} ms -> {:.3} ms (x{:.3}), geometric mean x{:.3}, {} with differing output",
            of.len(),
            before / 1e6,
            after / 1e6,
            before / after,
            geo.exp(),
            of.iter().filter(|r| r.verdict.mismatches > 0).count()
        );
    }
    Ok(())
}
