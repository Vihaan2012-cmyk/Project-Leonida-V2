mod common;

use common::{gpu, spirv_val};
use gpu::{Gpu, Resource, Sampler, Texture, TextureFormat, TextureView};
use shader_translate::images::{self, NUMBERS, Number, VIEWS, View};
use shader_translate::{ComputeOptions, FloatMode, PUSH_HEAP, PUSH_WORDS};

const INPUT: u32 = 0x1000;
const OUTPUT: u32 = 0x2000;
const HEAP_WORDS: usize = 0x3000 / 4;

type Texel = [u32; 4];

struct Registered {
    descriptor: [u32; 8],
    view: View,
    number: Number,
    width: u32,
    height: u32,
    depth: u32,
    mips: Vec<Vec<Texel>>,
    depth_format: bool,
}

fn f(value: f32) -> u32 {
    value.to_bits()
}

fn descriptor(id: u32, format: u32) -> [u32; 8] {
    [
        id << 8,
        format << 20 | 0x3F,
        0x1234_0000 | id,
        id,
        0,
        0,
        0,
        0,
    ]
}

const FLOAT_FORMAT: u32 = 77;
const UINT_FORMAT: u32 = 75;
const SINT_FORMAT: u32 = 76;

/// Texel (x, y, layer or slice) of mip `mip`.
fn texel(t: &Registered, mip: usize, x: u32, y: u32, z: u32) -> Texel {
    let width = (t.width >> mip).max(1);
    let height = (t.height >> mip).max(1);
    t.mips[mip][((z * height + y) * width + x) as usize]
}

fn build(
    id: u32,
    view: View,
    number: Number,
    [width, height, depth]: [u32; 3],
    mips: u32,
    value: impl Fn(u32, u32, u32, u32) -> Texel,
) -> Registered {
    let format = match number {
        Number::Float => FLOAT_FORMAT,
        Number::Uint => UINT_FORMAT,
        Number::Sint => SINT_FORMAT,
    };
    let mips = (0..mips)
        .map(|mip| {
            let (w, h) = ((width >> mip).max(1), (height >> mip).max(1));
            let d = if view == View::D3 {
                (depth >> mip).max(1)
            } else {
                depth
            };
            let mut texels = Vec::new();
            for z in 0..d {
                for y in 0..h {
                    for x in 0..w {
                        texels.push(value(mip, x, y, z));
                    }
                }
            }
            texels
        })
        .collect();
    Registered {
        descriptor: descriptor(id, format),
        view,
        number,
        width,
        height,
        depth,
        mips,
        depth_format: false,
    }
}

fn depth_texture(id: u32, size: u32, value: impl Fn(u32, u32) -> f32) -> Registered {
    let mut texture = build(
        id,
        View::D2,
        Number::Float,
        [size, size, 1],
        1,
        |_, x, y, _| [f(value(x, y)), 0, 0, f(1.0)],
    );
    texture.descriptor = descriptor(id, DEPTH_FORMAT);
    texture.depth_format = true;
    texture
}

const DEPTH_FORMAT: u32 = 20;

struct Scene {
    textures: Vec<Registered>,
    nearest: [u32; 4],
    linear: [u32; 4],
    compare: [u32; 4],
}

fn scene() -> Scene {
    let textures = vec![
        build(1, View::D2, Number::Float, [4, 4, 1], 2, |mip, x, y, _| {
            if mip == 0 {
                [
                    f((x + 10 * y) as f32),
                    f(100.0 + x as f32),
                    f(200.0 + y as f32),
                    f(1000.0),
                ]
            } else {
                [f(500.0 + (x + 10 * y) as f32), f(600.0), f(700.0), f(800.0)]
            }
        }),
        build(2, View::D2, Number::Uint, [4, 4, 1], 1, |_, x, y, _| {
            [x, y, x * y, 0xDEAD_0000 | (x + 4 * y)]
        }),
        build(3, View::D2, Number::Sint, [4, 4, 1], 1, |_, x, y, _| {
            [
                -(x as i32 + 1) as u32,
                (-(y as i32 + 1) * 1000) as u32,
                (x as i32 - y as i32) as u32,
                (i32::MIN + x as i32) as u32,
            ]
        }),
        build(
            4,
            View::D2Array,
            Number::Float,
            [2, 2, 3],
            1,
            |_, x, y, z| {
                [
                    f((z * 100 + x + 10 * y) as f32),
                    f(z as f32),
                    f(0.0),
                    f(1.0),
                ]
            },
        ),
        build(
            5,
            View::D2Array,
            Number::Float,
            [2, 2, 6],
            1,
            |_, x, y, z| [f((z * 1000 + x + 10 * y) as f32), f(1.0), f(2.0), f(3.0)],
        ),
        build(6, View::D3, Number::Uint, [2, 2, 2], 1, |_, x, y, z| {
            [x, y, z, x + 2 * y + 4 * z]
        }),
        build(7, View::D2, Number::Float, [4, 1, 1], 1, |_, x, _, _| {
            [f(3.0 * x as f32), f(0.5), f(0.25), f(0.125)]
        }),
        depth_texture(8, 4, |x, y| (x + 4 * y) as f32 / 16.0),
    ];
    Scene {
        textures,
        nearest: [0x10, 0x5010, 0, 0],
        linear: [0x11, 0x5011, 0, 0],
        compare: [0x3012, 0x5012, 0, 0],
    }
}

fn gpu_texture(
    view: View,
    number: Number,
    width: u32,
    height: u32,
    depth: u32,
    mips: &[Vec<Texel>],
) -> Texture {
    let format = match number {
        Number::Float => TextureFormat::Rgba32Float,
        Number::Uint => TextureFormat::Rgba32Uint,
        Number::Sint => TextureFormat::Rgba32Sint,
    };
    colour_or_depth(view, format, width, height, depth, mips)
}

fn registered_texture(t: &Registered) -> Texture {
    if t.depth_format {
        colour_or_depth(
            t.view,
            TextureFormat::Depth32Float,
            t.width,
            t.height,
            t.depth,
            &t.mips,
        )
    } else {
        gpu_texture(t.view, t.number, t.width, t.height, t.depth, &t.mips)
    }
}

fn colour_or_depth(
    view: View,
    format: TextureFormat,
    width: u32,
    height: u32,
    depth: u32,
    mips: &[Vec<Texel>],
) -> Texture {
    let channels = if format == TextureFormat::Depth32Float {
        1
    } else {
        4
    };
    let view = match view {
        View::D2 => TextureView::D2,
        View::D2Array => TextureView::D2Array,
        View::D3 => TextureView::D3,
    };
    Texture {
        view,
        format,
        width,
        height,
        depth,
        mips: mips.len() as u32,
        data: mips
            .iter()
            .flatten()
            .flat_map(|texel| texel[..channels].to_vec())
            .flat_map(|w| w.to_le_bytes())
            .collect(),
    }
}

const DEFAULT: Texel = [0x40E0_0000; 4];

fn resources(scene: &Scene) -> Vec<(u32, Resource)> {
    let mut entries = Vec::new();
    let mut out = Vec::new();
    for view in VIEWS {
        for number in NUMBERS {
            let mut list = vec![gpu_texture(view, number, 1, 1, 1, &[vec![DEFAULT]])];
            for t in scene
                .textures
                .iter()
                .filter(|t| t.view == view && t.number == number)
            {
                entries.push((t.descriptor, images::slot(list.len() as u32, number)));
                list.push(registered_texture(t));
            }
            out.push((images::binding(view, number), Resource::Textures(list)));
        }
    }
    let sampler = |linear: bool, compare: Option<i32>| Sampler {
        linear,
        repeat: false,
        compare,
    };
    let samplers = vec![
        sampler(false, None),
        sampler(false, None),
        sampler(true, None),
        sampler(false, None),
    ];
    let less_or_equal = Some(3);
    let compare_samplers = vec![
        sampler(false, less_or_equal),
        sampler(false, less_or_equal),
        sampler(true, less_or_equal),
        sampler(false, less_or_equal),
    ];
    let sampler_entries = [
        (images::sampler_key(&scene.nearest), 1),
        (images::sampler_key(&scene.linear), 2),
        (images::sampler_key(&scene.compare), 3),
    ];
    out.push((
        images::COMPARE_SAMPLERS,
        Resource::Samplers(compare_samplers),
    ));
    out.push((
        images::TEXTURE_TABLE,
        Resource::Buffer(images::table(&entries).unwrap()),
    ));
    out.push((
        images::SAMPLER_TABLE,
        Resource::Buffer(images::table(&sampler_entries).unwrap()),
    ));
    out.push((images::SAMPLERS, Resource::Samplers(samplers)));
    out
}

type Input<'a> = Box<dyn Fn(u32) -> [u32; 8] + 'a>;

struct Case<'a> {
    name: &'a str,
    instruction: &'a str,
    texture: [u32; 8],
    sampler: [u32; 4],
    input: Input<'a>,
    expect: Box<dyn Fn(u32) -> Texel + 'a>,
}

fn four<'a>(input: impl Fn(u32) -> Texel + 'a) -> Input<'a> {
    Box::new(move |lane| {
        let mut words = [0; 8];
        words[..4].copy_from_slice(&input(lane));
        words
    })
}

fn compile(source: &str, lanes: u32, gpu: &Gpu, wave64: bool, name: &str) -> Vec<u32> {
    let code = rdna::assemble(source, !wave64).unwrap_or_else(|e| panic!("{e:?}\n{source}"));
    let function = shader_ir::lift(&code, wave64).unwrap_or_else(|e| panic!("{e}\n{source}"));
    let float = FloatMode {
        nan_preserve: gpu.info.nan_preserve_32,
        flush_denormals: false,
        preserve_denormals: gpu.info.denorm_preserve_32,
    };
    let options = ComputeOptions::from_rsrc2(20 << 1, [lanes, 1, 1], float);
    let spirv = shader_translate::compute(&function, &options)
        .unwrap_or_else(|e| panic!("{name}: {e}\n{function}"));
    spirv_val(&spirv, &format!("image_{name}_{lanes}"));
    spirv
}

fn run(gpu: &Gpu, scene: &Scene, case: &Case<'_>, wave64: bool) {
    let lanes = if wave64 { 64 } else { 32 };
    let source = format!(
        "v_lshlrev_b32_e32 v20, 5, v0
         v_lshlrev_b32_e32 v21, 4, v0
         global_load_dwordx4 v[10:13], v20, s[16:17]
         global_load_dwordx4 v[14:17], v20, s[16:17] offset:16
         s_waitcnt vmcnt(0)
         {}
         s_waitcnt vmcnt(0)
         global_store_dwordx4 v21, v[4:7], s[18:19]
         s_endpgm",
        case.instruction
    );
    let spirv = compile(&source, lanes, gpu, wave64, case.name);
    let mut heap = vec![0u32; HEAP_WORDS];
    for lane in 0..lanes {
        let at = (INPUT / 4 + lane * 8) as usize;
        heap[at..at + 8].copy_from_slice(&(case.input)(lane));
    }
    let mut user = vec![0u32; 20];
    user[..8].copy_from_slice(&case.texture);
    user[8..12].copy_from_slice(&case.sampler);
    user[16] = INPUT;
    user[18] = OUTPUT;
    gpu.dispatch_resources(
        &spirv,
        &mut [&mut heap],
        |addresses| {
            let mut push = vec![0u32; PUSH_WORDS as usize];
            push[..user.len()].copy_from_slice(&user);
            push[PUSH_HEAP as usize] = addresses[0] as u32;
            push[PUSH_HEAP as usize + 1] = (addresses[0] >> 32) as u32;
            push
        },
        [1, 1, 1],
        Some(32),
        &mut resources(scene),
    )
    .unwrap();
    assert_eq!(gpu::validation_errors(), 0);
    for lane in 0..lanes {
        let at = (OUTPUT / 4 + lane * 4) as usize;
        let expected = (case.expect)(lane);
        assert_eq!(
            heap[at..at + 4],
            expected,
            "{} lane {lane}: got {:08x?} expected {:08x?}",
            case.name,
            &heap[at..at + 4],
            expected
        );
    }
}

fn image_gpu() -> Option<Gpu> {
    let gpu = gpu()?;
    if !gpu.info.descriptor_indexing || !gpu.info.storage_images {
        eprintln!(
            "skipping: {} lacks descriptor indexing or format-less storage images",
            gpu.info.name
        );
        return None;
    }
    Some(gpu)
}

fn half(value: f32) -> u32 {
    rdna::format::f32_to_f16(value.to_bits())
}

const CLEAR: &str =
    "v_mov_b32_e32 v4, 0\nv_mov_b32_e32 v5, 0\nv_mov_b32_e32 v6, 0\nv_mov_b32_e32 v7, 0\n";

fn grid(lane: u32, size: u32) -> (u32, u32) {
    (lane % size, lane / size % size)
}

fn center(i: u32, size: u32) -> u32 {
    f((i as f32 + 0.5) / size as f32)
}

#[test]
fn image_instructions_read_registered_textures() {
    let Some(gpu) = image_gpu() else { return };
    let scene = scene();
    let t = |i: usize| &scene.textures[i];
    let cases = vec![
        Case {
            name: "load_uint",
            instruction: "image_load v[4:7], v[10:11], s[0:7] dmask:0xf dim:SQ_RSRC_IMG_2D unorm",
            texture: t(1).descriptor,
            sampler: [0; 4],
            input: four(|lane| {
                let (x, y) = grid(lane, 4);
                [x, y, 0, 0]
            }),
            expect: Box::new(|lane| {
                let (x, y) = grid(lane, 4);
                texel(t(1), 0, x, y, 0)
            }),
        },
        Case {
            name: "load_sint_two_components",
            instruction: "image_load v[4:5], v[10:11], s[0:7] dmask:0x9 dim:SQ_RSRC_IMG_2D unorm",
            texture: t(2).descriptor,
            sampler: [0; 4],
            input: four(|lane| {
                let (x, y) = grid(lane, 4);
                [x, y, 0, 0]
            }),
            expect: Box::new(|lane| {
                let (x, y) = grid(lane, 4);
                let value = texel(t(2), 0, x, y, 0);
                [value[0], value[3], 0, 0]
            }),
        },
        Case {
            name: "sample_nearest",
            instruction: "image_sample_lz v[4:7], v[10:11], s[0:7], s[8:11] dmask:0xf dim:SQ_RSRC_IMG_2D",
            texture: t(0).descriptor,
            sampler: scene.nearest,
            input: four(|lane| {
                let (x, y) = grid(lane, 4);
                [center(x, 4), center(y, 4), 0, 0]
            }),
            expect: Box::new(|lane| {
                let (x, y) = grid(lane, 4);
                texel(t(0), 0, x, y, 0)
            }),
        },
        Case {
            name: "sample_linear_between_texels",
            instruction: "image_sample_lz v[4:7], v[10:11], s[0:7], s[8:11] dmask:0xf dim:SQ_RSRC_IMG_2D",
            texture: t(0).descriptor,
            sampler: scene.linear,
            input: four(|lane| {
                let (x, y) = (1 + lane % 3, lane / 3 % 4);
                [f(x as f32 / 4.0), center(y, 4), 0, 0]
            }),
            expect: Box::new(|lane| {
                let (x, y) = (1 + lane % 3, lane / 3 % 4);
                let (a, b) = (texel(t(0), 0, x - 1, y, 0), texel(t(0), 0, x, y, 0));
                std::array::from_fn(|c| f((f32::from_bits(a[c]) + f32::from_bits(b[c])) * 0.5))
            }),
        },
        Case {
            name: "sample_explicit_lod",
            instruction: "image_sample_l v[4:7], v[10:12], s[0:7], s[8:11] dmask:0xf dim:SQ_RSRC_IMG_2D",
            texture: t(0).descriptor,
            sampler: scene.nearest,
            input: four(|lane| {
                let (x, y) = grid(lane, 2);
                [center(x, 2), center(y, 2), f(1.0), 0]
            }),
            expect: Box::new(|lane| {
                let (x, y) = grid(lane, 2);
                texel(t(0), 1, x, y, 0)
            }),
        },
        Case {
            name: "load_mip",
            instruction: "image_load_mip v[4:7], v[10:12], s[0:7] dmask:0xf dim:SQ_RSRC_IMG_2D unorm",
            texture: t(0).descriptor,
            sampler: [0; 4],
            input: four(|lane| {
                let (x, y) = grid(lane, 2);
                [x, y, 1, 0]
            }),
            expect: Box::new(|lane| {
                let (x, y) = grid(lane, 2);
                texel(t(0), 1, x, y, 0)
            }),
        },
        Case {
            name: "gather_red",
            instruction: "image_gather4_lz v[4:7], v[10:11], s[0:7], s[8:11] dmask:0x1 dim:SQ_RSRC_IMG_2D",
            texture: t(0).descriptor,
            sampler: scene.nearest,
            input: four(|lane| {
                let (x, y) = (1 + lane % 3, 1 + lane / 3 % 3);
                [f(x as f32 / 4.0), f(y as f32 / 4.0), 0, 0]
            }),
            expect: Box::new(|lane| {
                let (x, y) = (1 + lane % 3, 1 + lane / 3 % 3);
                let red = |x, y| texel(t(0), 0, x, y, 0)[0];
                [red(x - 1, y), red(x, y), red(x, y - 1), red(x - 1, y - 1)]
            }),
        },
        Case {
            name: "array_layer",
            instruction: "image_sample_lz v[4:7], v[10:12], s[0:7], s[8:11] dmask:0xf dim:SQ_RSRC_IMG_2D_ARRAY",
            texture: t(3).descriptor,
            sampler: scene.nearest,
            input: four(|lane| {
                let (x, y) = grid(lane, 2);
                [center(x, 2), center(y, 2), f((lane % 3) as f32), 0]
            }),
            expect: Box::new(|lane| {
                let (x, y) = grid(lane, 2);
                texel(t(3), 0, x, y, lane % 3)
            }),
        },
        Case {
            name: "cube_face",
            instruction: "image_sample_lz v[4:7], v[10:12], s[0:7], s[8:11] dmask:0xf dim:SQ_RSRC_IMG_CUBE",
            texture: t(4).descriptor,
            sampler: scene.nearest,
            input: four(|lane| {
                let (x, y) = grid(lane, 2);
                let face = lane % 6;
                [
                    f(1.0 + f32::from_bits(center(x, 2))),
                    f(1.0 + f32::from_bits(center(y, 2))),
                    f(face as f32),
                    0,
                ]
            }),
            expect: Box::new(|lane| {
                let (x, y) = grid(lane, 2);
                texel(t(4), 0, x, y, lane % 6)
            }),
        },
        Case {
            name: "volume_load",
            instruction: "image_load v[4:7], v[10:12], s[0:7] dmask:0xf dim:SQ_RSRC_IMG_3D unorm",
            texture: t(5).descriptor,
            sampler: [0; 4],
            input: four(|lane| [lane % 2, lane / 2 % 2, lane / 4 % 2, 0]),
            expect: Box::new(|lane| texel(t(5), 0, lane % 2, lane / 2 % 2, lane / 4 % 2)),
        },
        Case {
            name: "line_load",
            instruction: "image_load v[4:7], v10, s[0:7] dmask:0xf dim:SQ_RSRC_IMG_1D unorm",
            texture: t(6).descriptor,
            sampler: [0; 4],
            input: four(|lane| [lane % 4, 0, 0, 0]),
            expect: Box::new(|lane| texel(t(6), 0, lane % 4, 0, 0)),
        },
        Case {
            name: "line_sample",
            instruction: "image_sample_lz v[4:7], v10, s[0:7], s[8:11] dmask:0xf dim:SQ_RSRC_IMG_1D",
            texture: t(6).descriptor,
            sampler: scene.nearest,
            input: four(|lane| [center(lane % 4, 4), 0, 0, 0]),
            expect: Box::new(|lane| texel(t(6), 0, lane % 4, 0, 0)),
        },
        Case {
            name: "unnormalized_coordinates",
            instruction: "image_sample_lz v[4:7], v[10:11], s[0:7], s[8:11] dmask:0xf dim:SQ_RSRC_IMG_2D unorm",
            texture: t(0).descriptor,
            sampler: scene.nearest,
            input: four(|lane| {
                let (x, y) = grid(lane, 4);
                [f(x as f32 + 0.5), f(y as f32 + 0.5), 0, 0]
            }),
            expect: Box::new(|lane| {
                let (x, y) = grid(lane, 4);
                texel(t(0), 0, x, y, 0)
            }),
        },
        Case {
            name: "unregistered_texture_reads_the_default",
            instruction: "image_sample_lz v[4:7], v[10:11], s[0:7], s[8:11] dmask:0xf dim:SQ_RSRC_IMG_2D",
            texture: descriptor(99, FLOAT_FORMAT),
            sampler: scene.nearest,
            input: four(|lane| {
                let (x, y) = grid(lane, 4);
                [center(x, 4), center(y, 4), 0, 0]
            }),
            expect: Box::new(|_| DEFAULT),
        },
    ];
    for case in &cases {
        run(&gpu, &scene, case, false);
    }
    run(&gpu, &scene, &cases[2], true);
    run(&gpu, &scene, &cases[6], true);
}

#[test]
fn compare_gradient_size_and_sixteen_bit_forms() {
    let Some(gpu) = image_gpu() else { return };
    let scene = scene();
    let t = |i: usize| &scene.textures[i];
    let depth = |x: u32, y: u32| f32::from_bits(texel(t(7), 0, x, y, 0)[0]);
    let passes = |lane: u32, x: u32, y: u32| {
        let reference = depth(x, y) + if lane.is_multiple_of(2) { -0.01 } else { 0.01 };
        (reference, if lane.is_multiple_of(2) { 1.0f32 } else { 0.0 })
    };
    let red = |x, y| texel(t(0), 0, x, y, 0)[0];
    let instructions: Vec<String> = [
        "image_sample_c_lz v4, v[10:12], s[0:7], s[8:11] dmask:0x1 dim:SQ_RSRC_IMG_2D",
        "image_gather4_c_lz v[4:7], v[10:12], s[0:7], s[8:11] dmask:0x1 dim:SQ_RSRC_IMG_2D",
        "image_sample_d v[4:7], v[10:15], s[0:7], s[8:11] dmask:0xf dim:SQ_RSRC_IMG_2D",
        "image_sample_d_cl v[4:7], v[10:16], s[0:7], s[8:11] dmask:0xf dim:SQ_RSRC_IMG_2D",
        "image_get_resinfo v[4:7], v10, s[0:7] dmask:0xf dim:SQ_RSRC_IMG_2D unorm",
        "image_get_resinfo v[4:7], v10, s[0:7] dmask:0xf dim:SQ_RSRC_IMG_2D_ARRAY unorm",
        "image_get_resinfo v[4:7], v10, s[0:7] dmask:0xf dim:SQ_RSRC_IMG_CUBE unorm",
        "image_sample_lz v[4:7], v10, s[0:7], s[8:11] dmask:0xf dim:SQ_RSRC_IMG_2D a16",
        "image_load v[4:7], v10, s[0:7] dmask:0xf dim:SQ_RSRC_IMG_2D unorm a16",
        "image_load v[4:5], v[10:11], s[0:7] dmask:0xf dim:SQ_RSRC_IMG_2D unorm d16",
        "image_load v[4:5], v[10:11], s[0:7] dmask:0xf dim:SQ_RSRC_IMG_2D unorm d16",
        "image_gather4_lz_o v[4:7], v[10:12], s[0:7], s[8:11] dmask:0x1 dim:SQ_RSRC_IMG_2D",
    ]
    .iter()
    .map(|i| format!("{CLEAR}{i}"))
    .collect();
    let cases = vec![
        Case {
            name: "compare_lz",
            instruction: &instructions[0],
            texture: t(7).descriptor,
            sampler: scene.compare,
            input: four(move |lane| {
                let (x, y) = grid(lane, 4);
                let (reference, _) = passes(lane, x, y);
                [f(reference), center(x, 4), center(y, 4), 0]
            }),
            expect: Box::new(move |lane| {
                let (x, y) = grid(lane, 4);
                [f(passes(lane, x, y).1), 0, 0, 0]
            }),
        },
        Case {
            name: "compare_gather",
            instruction: &instructions[1],
            texture: t(7).descriptor,
            sampler: scene.compare,
            input: four(|lane| {
                let (x, y) = (1 + lane % 3, 1 + lane / 3 % 3);
                [f(depth(x, y)), f(x as f32 / 4.0), f(y as f32 / 4.0), 0]
            }),
            expect: Box::new(|lane| {
                let (x, y) = (1 + lane % 3, 1 + lane / 3 % 3);
                let at = |i, j| f(if depth(x, y) <= depth(i, j) { 1.0 } else { 0.0 });
                [at(x - 1, y), at(x, y), at(x, y - 1), at(x - 1, y - 1)]
            }),
        },
        Case {
            name: "gradients_pick_the_mip",
            instruction: &instructions[2],
            texture: t(0).descriptor,
            sampler: scene.nearest,
            input: Box::new(|lane| {
                let (x, y) = grid(lane, 2);
                [f(0.5), 0, 0, 0, center(x, 2), center(y, 2), 0, 0]
            }),
            expect: Box::new(|lane| {
                let (x, y) = grid(lane, 2);
                texel(t(0), 1, x, y, 0)
            }),
        },
        Case {
            name: "gradient_clamp",
            instruction: &instructions[3],
            texture: t(0).descriptor,
            sampler: scene.nearest,
            input: Box::new(|lane| {
                let (x, y) = grid(lane, 2);
                [f(0.25), 0, 0, 0, center(x, 2), center(y, 2), f(1.0), 0]
            }),
            expect: Box::new(|lane| {
                let (x, y) = grid(lane, 2);
                texel(t(0), 1, x, y, 0)
            }),
        },
        Case {
            name: "size_by_mip",
            instruction: &instructions[4],
            texture: t(0).descriptor,
            sampler: [0; 4],
            input: four(|lane| [lane % 3, 0, 0, 0]),
            expect: Box::new(|lane| match lane % 3 {
                0 => [4, 4, 1, 2],
                1 => [2, 2, 1, 2],
                _ => [0, 0, 0, 2],
            }),
        },
        Case {
            name: "size_of_array",
            instruction: &instructions[5],
            texture: t(3).descriptor,
            sampler: [0; 4],
            input: four(|_| [0; 4]),
            expect: Box::new(|_| [2, 2, 3, 1]),
        },
        Case {
            name: "size_of_cube",
            instruction: &instructions[6],
            texture: t(4).descriptor,
            sampler: [0; 4],
            input: four(|_| [0; 4]),
            expect: Box::new(|_| [2, 2, 1, 1]),
        },
        Case {
            name: "sample_half_coordinates",
            instruction: &instructions[7],
            texture: t(0).descriptor,
            sampler: scene.nearest,
            input: four(|lane| {
                let (x, y) = grid(lane, 4);
                let at = |i: u32| half((i as f32 + 0.5) / 4.0);
                [at(x) | at(y) << 16, 0, 0, 0]
            }),
            expect: Box::new(|lane| {
                let (x, y) = grid(lane, 4);
                texel(t(0), 0, x, y, 0)
            }),
        },
        Case {
            name: "load_half_coordinates",
            instruction: &instructions[8],
            texture: t(1).descriptor,
            sampler: [0; 4],
            input: four(|lane| {
                let (x, y) = grid(lane, 4);
                [x | y << 16, 0, 0, 0]
            }),
            expect: Box::new(|lane| {
                let (x, y) = grid(lane, 4);
                texel(t(1), 0, x, y, 0)
            }),
        },
        Case {
            name: "load_half_results",
            instruction: &instructions[9],
            texture: t(0).descriptor,
            sampler: [0; 4],
            input: four(|lane| {
                let (x, y) = grid(lane, 4);
                [x, y, 0, 0]
            }),
            expect: Box::new(|lane| {
                let (x, y) = grid(lane, 4);
                let value = texel(t(0), 0, x, y, 0).map(|w| half(f32::from_bits(w)));
                [value[0] | value[1] << 16, value[2] | value[3] << 16, 0, 0]
            }),
        },
        Case {
            name: "load_half_integers",
            instruction: &instructions[10],
            texture: t(1).descriptor,
            sampler: [0; 4],
            input: four(|lane| {
                let (x, y) = grid(lane, 4);
                [x, y, 0, 0]
            }),
            expect: Box::new(|lane| {
                let (x, y) = grid(lane, 4);
                let value = texel(t(1), 0, x, y, 0).map(|w| w & 0xFFFF);
                [value[0] | value[1] << 16, value[2] | value[3] << 16, 0, 0]
            }),
        },
        Case {
            name: "gather_offset",
            instruction: &instructions[11],
            texture: t(0).descriptor,
            sampler: scene.nearest,
            input: four(|lane| {
                let (x, y) = (1 + lane % 2, 1 + lane / 2 % 3);
                [1, f(x as f32 / 4.0), f(y as f32 / 4.0), 0]
            }),
            expect: Box::new(move |lane| {
                let (x, y) = (1 + lane % 2, 1 + lane / 2 % 3);
                [red(x, y), red(x + 1, y), red(x + 1, y - 1), red(x, y - 1)]
            }),
        },
    ];
    for case in &cases {
        run(&gpu, &scene, case, false);
    }
    run(&gpu, &scene, &cases[0], true);
}

type StoreCase = (&'static str, usize, Box<dyn Fn(u32, u32) -> Texel>);

#[test]
fn stores_write_storage_textures() {
    let Some(gpu) = image_gpu() else { return };
    let targets = [
        (descriptor(20, UINT_FORMAT), Number::Uint),
        (descriptor(21, FLOAT_FORMAT), Number::Float),
    ];
    let blank = |number| gpu_texture(View::D2, number, 4, 4, 1, &[vec![[0; 4]; 16]]);
    let cases: [StoreCase; 3] = [
        (
            "image_store v[12:15], v[10:11], s[0:7] dmask:0xf dim:SQ_RSRC_IMG_2D unorm",
            0,
            Box::new(|x, y| [x, y, x * y, 7 + x + 4 * y]),
        ),
        (
            "image_store v[12:13], v[10:11], s[0:7] dmask:0x5 dim:SQ_RSRC_IMG_2D unorm",
            0,
            Box::new(|x, y| [x + 100, 0, y + 200, 0]),
        ),
        (
            "image_store v[12:13], v[10:11], s[0:7] dmask:0xf dim:SQ_RSRC_IMG_2D unorm d16",
            1,
            Box::new(|x, y| [f(x as f32), f(y as f32 * 0.5), f(-1.0), f(1000.0)]),
        ),
    ];
    for (instruction, target, value) in &cases {
        let source = format!(
            "v_and_b32_e32 v10, 3, v0
             v_bfe_u32 v11, v0, 2, 2
             v_lshlrev_b32_e32 v20, 4, v0
             global_load_dwordx4 v[12:15], v20, s[16:17]
             s_waitcnt vmcnt(0)
             {instruction}
             s_endpgm"
        );
        let spirv = compile(&source, 32, &gpu, false, "store");
        let d16 = instruction.ends_with("d16");
        let masked = instruction.contains("dmask:0x5");
        let mut heap = vec![0u32; HEAP_WORDS];
        for lane in 0..32 {
            let (x, y) = grid(lane, 4);
            let texel = value(x, y);
            let words = if d16 {
                let h = texel.map(|w| half(f32::from_bits(w)));
                [h[0] | h[1] << 16, h[2] | h[3] << 16, 0, 0]
            } else if masked {
                [texel[0], texel[2], 0, 0]
            } else {
                texel
            };
            let at = (INPUT / 4 + lane * 4) as usize;
            heap[at..at + 4].copy_from_slice(&words);
        }
        let mut entries = Vec::new();
        let mut resources = Vec::new();
        for view in VIEWS {
            for number in NUMBERS {
                let mut sampled = vec![gpu_texture(view, number, 1, 1, 1, &[vec![DEFAULT]])];
                let mut writable = vec![gpu_texture(view, number, 1, 1, 1, &[vec![DEFAULT]])];
                if view == View::D2 {
                    for (key, _) in targets.iter().filter(|(_, n)| *n == number) {
                        entries.push((*key, images::slot(writable.len() as u32, number)));
                        sampled.push(blank(number));
                        writable.push(blank(number));
                    }
                }
                resources.push((images::binding(view, number), Resource::Textures(sampled)));
                resources.push((
                    images::storage_binding(view, number),
                    Resource::StorageTextures(writable),
                ));
            }
        }
        resources.push((
            images::TEXTURE_TABLE,
            Resource::Buffer(images::table(&entries).unwrap()),
        ));
        resources.push((
            images::SAMPLER_TABLE,
            Resource::Buffer(images::table(&[]).unwrap()),
        ));
        let mut user = vec![0u32; 20];
        user[..8].copy_from_slice(&targets[*target].0);
        user[16] = INPUT;
        gpu.dispatch_resources(
            &spirv,
            &mut [&mut heap],
            |addresses| {
                let mut push = vec![0u32; PUSH_WORDS as usize];
                push[..user.len()].copy_from_slice(&user);
                push[PUSH_HEAP as usize] = addresses[0] as u32;
                push[PUSH_HEAP as usize + 1] = (addresses[0] >> 32) as u32;
                push
            },
            [1, 1, 1],
            Some(32),
            &mut resources,
        )
        .unwrap();
        assert_eq!(gpu::validation_errors(), 0);
        let number = targets[*target].1;
        let binding = images::storage_binding(View::D2, number);
        let Some((_, Resource::StorageTextures(list))) =
            resources.iter().find(|(b, _)| *b == binding)
        else {
            panic!("no storage list");
        };
        let words: Vec<u32> = list[1]
            .data
            .chunks(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        for y in 0..4 {
            for x in 0..4 {
                let at = ((y * 4 + x) * 4) as usize;
                assert_eq!(
                    words[at..at + 4],
                    value(x, y),
                    "{instruction} texel {x},{y}"
                );
            }
        }
    }
}

fn storage_scene(width: TextureFormat, initial: impl Fn(u32) -> u64) -> Vec<(u32, Resource)> {
    let mut resources = Vec::new();
    let mut entries = Vec::new();
    for view in VIEWS {
        for number in NUMBERS {
            let dummy = vec![gpu_texture(view, number, 1, 1, 1, &[vec![DEFAULT]])];
            resources.push((images::binding(view, number), Resource::Textures(dummy)));
        }
        for wide in [false, true] {
            let format = if wide {
                TextureFormat::R64Uint
            } else {
                TextureFormat::R32Uint
            };
            let texel = |value: u64| -> Vec<u8> {
                if wide {
                    value.to_le_bytes().to_vec()
                } else {
                    (value as u32).to_le_bytes().to_vec()
                }
            };
            let single = |data: Vec<u8>, width: u32, height: u32| Texture {
                view: match view {
                    View::D2 => TextureView::D2,
                    View::D2Array => TextureView::D2Array,
                    View::D3 => TextureView::D3,
                },
                format,
                width,
                height,
                depth: 1,
                mips: 1,
                data,
            };
            let mut list = vec![single(texel(0), 1, 1)];
            if view == View::D2 && (format == width) {
                entries.push((descriptor(30, UINT_FORMAT), images::slot(1, Number::Uint)));
                list.push(single(
                    (0..16).flat_map(|i| texel(initial(i))).collect(),
                    4,
                    4,
                ));
            }
            resources.push((
                images::atomic_binding(view, wide),
                Resource::StorageTextures(list),
            ));
        }
    }
    resources.push((
        images::TEXTURE_TABLE,
        Resource::Buffer(images::table(&entries).unwrap()),
    ));
    resources.push((
        images::SAMPLER_TABLE,
        Resource::Buffer(images::table(&[]).unwrap()),
    ));
    resources
}

#[test]
fn image_atomics_update_each_active_lanes_texel() {
    let Some(gpu) = image_gpu() else { return };
    if !gpu.info.image_int64_atomics {
        eprintln!("skipping: {} lacks 64-bit image atomics", gpu.info.name);
        return;
    }
    let initial = |i: u32| u64::from(100 + i);
    type Expect = Box<dyn Fn(u32) -> (u64, Option<u32>)>;
    let cases: [(&str, TextureFormat, Expect); 4] = [
        (
            "image_atomic_add v12, v[10:11], s[0:7] dmask:0x1 dim:SQ_RSRC_IMG_2D unorm",
            TextureFormat::R32Uint,
            Box::new(move |i| (initial(i) + 2 * 5, None)),
        ),
        (
            "v_cmpx_gt_u32_e32 16, v0\nimage_atomic_add v12, v[10:11], s[0:7] dmask:0x1 dim:SQ_RSRC_IMG_2D unorm glc",
            TextureFormat::R32Uint,
            Box::new(move |i| (initial(i) + 5, Some(initial(i) as u32))),
        ),
        (
            "image_atomic_umax v[12:13], v[10:11], s[0:7] dmask:0x3 dim:SQ_RSRC_IMG_2D unorm",
            TextureFormat::R64Uint,
            Box::new(move |i| ((u64::from(16 + i) << 32) | 7, None)),
        ),
        (
            "v_cmpx_gt_u32_e32 16, v0\nimage_atomic_cmpswap v[12:13], v[10:11], s[0:7] dmask:0x3 dim:SQ_RSRC_IMG_2D unorm glc",
            TextureFormat::R32Uint,
            Box::new(move |i| (u64::from(0xC000 + i), Some(initial(i) as u32))),
        ),
    ];
    for (instruction, format, expect) in &cases {
        let source = format!(
            "v_and_b32_e32 v10, 3, v0
             v_bfe_u32 v11, v0, 2, 2
             v_lshlrev_b32_e32 v20, 4, v0
             global_load_dwordx4 v[12:15], v20, s[16:17]
             s_waitcnt vmcnt(0)
             {instruction}
             s_mov_b32 exec_lo, -1
             global_store_dword v20, v12, s[18:19]
             s_endpgm"
        );
        let spirv = compile(&source, 32, &gpu, false, "atomic");
        let mut heap = vec![0u32; HEAP_WORDS];
        for lane in 0..32u32 {
            let texel = lane % 16;
            let words = if instruction.contains("umax") {
                [7, lane, 0, 0]
            } else if instruction.contains("cmpswap") {
                [0xC000 + texel, initial(texel) as u32, 0, 0]
            } else {
                [5, 0, 0, 0]
            };
            let at = (INPUT / 4 + lane * 4) as usize;
            heap[at..at + 4].copy_from_slice(&words);
        }
        let mut resources = storage_scene(*format, initial);
        let mut user = vec![0u32; 20];
        user[..8].copy_from_slice(&descriptor(30, UINT_FORMAT));
        user[16] = INPUT;
        user[18] = OUTPUT;
        gpu.dispatch_resources(
            &spirv,
            &mut [&mut heap],
            |addresses| {
                let mut push = vec![0u32; PUSH_WORDS as usize];
                push[..user.len()].copy_from_slice(&user);
                push[PUSH_HEAP as usize] = addresses[0] as u32;
                push[PUSH_HEAP as usize + 1] = (addresses[0] >> 32) as u32;
                push
            },
            [1, 1, 1],
            Some(32),
            &mut resources,
        )
        .unwrap();
        assert_eq!(gpu::validation_errors(), 0);
        let wide = *format == TextureFormat::R64Uint;
        let binding = images::atomic_binding(View::D2, wide);
        let Some((_, Resource::StorageTextures(list))) =
            resources.iter().find(|(b, _)| *b == binding)
        else {
            panic!("no atomic list");
        };
        for texel in 0..16u32 {
            let size = if wide { 8 } else { 4 };
            let at = texel as usize * size;
            let mut bytes = [0u8; 8];
            bytes[..size].copy_from_slice(&list[1].data[at..at + size]);
            let (value, returned) = expect(texel);
            assert_eq!(
                u64::from_le_bytes(bytes),
                value,
                "{instruction} texel {texel}"
            );
            if let Some(old) = returned {
                let lane = texel as usize;
                assert_eq!(
                    heap[(OUTPUT / 4) as usize + lane * 4],
                    old,
                    "{instruction} lane {lane}"
                );
            }
        }
    }
}
