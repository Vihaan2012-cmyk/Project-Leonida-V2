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
    }
}

struct Scene {
    textures: Vec<Registered>,
    nearest: [u32; 4],
    linear: [u32; 4],
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
    ];
    Scene {
        textures,
        nearest: [0x10, 0x5010, 0, 0],
        linear: [0x11, 0x5011, 0, 0],
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
            .flatten()
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
                list.push(gpu_texture(
                    view, number, t.width, t.height, t.depth, &t.mips,
                ));
            }
            out.push((images::binding(view, number), Resource::Textures(list)));
        }
    }
    let samplers = vec![
        Sampler {
            linear: false,
            repeat: false,
        },
        Sampler {
            linear: false,
            repeat: false,
        },
        Sampler {
            linear: true,
            repeat: false,
        },
    ];
    let sampler_entries = [
        (images::sampler_key(&scene.nearest), 1),
        (images::sampler_key(&scene.linear), 2),
    ];
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

struct Case<'a> {
    name: &'a str,
    instruction: &'a str,
    texture: [u32; 8],
    sampler: [u32; 4],
    input: Box<dyn Fn(u32) -> Texel + 'a>,
    expect: Box<dyn Fn(u32) -> Texel + 'a>,
}

fn run(gpu: &Gpu, scene: &Scene, case: &Case<'_>, wave64: bool) {
    let lanes = if wave64 { 64 } else { 32 };
    let source = format!(
        "v_lshlrev_b32_e32 v20, 4, v0
         global_load_dwordx4 v[10:13], v20, s[16:17]
         s_waitcnt vmcnt(0)
         {}
         s_waitcnt vmcnt(0)
         global_store_dwordx4 v20, v[4:7], s[18:19]
         s_endpgm",
        case.instruction
    );
    let code = rdna::assemble(&source, !wave64).unwrap_or_else(|e| panic!("{e:?}\n{source}"));
    let function = shader_ir::lift(&code, wave64).unwrap_or_else(|e| panic!("{e}\n{source}"));
    let float = FloatMode {
        nan_preserve: gpu.info.nan_preserve_32,
        flush_denormals: false,
        preserve_denormals: gpu.info.denorm_preserve_32,
    };
    let options = ComputeOptions::from_rsrc2(20 << 1, [lanes, 1, 1], float);
    let spirv = shader_translate::compute(&function, &options)
        .unwrap_or_else(|e| panic!("{}: {e}\n{function}", case.name));
    spirv_val(&spirv, &format!("image_{}_{lanes}", case.name));
    let mut heap = vec![0u32; HEAP_WORDS];
    for lane in 0..lanes {
        let at = (INPUT / 4 + lane * 4) as usize;
        heap[at..at + 4].copy_from_slice(&(case.input)(lane));
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
        &resources(scene),
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
    if !gpu.info.descriptor_indexing {
        eprintln!("skipping: {} lacks descriptor indexing", gpu.info.name);
        return None;
    }
    Some(gpu)
}

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
            input: Box::new(|lane| {
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
            input: Box::new(|lane| {
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
            input: Box::new(|lane| {
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
            input: Box::new(|lane| {
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
            input: Box::new(|lane| {
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
            input: Box::new(|lane| {
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
            input: Box::new(|lane| {
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
            input: Box::new(|lane| {
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
            input: Box::new(|lane| {
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
            input: Box::new(|lane| [lane % 2, lane / 2 % 2, lane / 4 % 2, 0]),
            expect: Box::new(|lane| texel(t(5), 0, lane % 2, lane / 2 % 2, lane / 4 % 2)),
        },
        Case {
            name: "line_load",
            instruction: "image_load v[4:7], v10, s[0:7] dmask:0xf dim:SQ_RSRC_IMG_1D unorm",
            texture: t(6).descriptor,
            sampler: [0; 4],
            input: Box::new(|lane| [lane % 4, 0, 0, 0]),
            expect: Box::new(|lane| texel(t(6), 0, lane % 4, 0, 0)),
        },
        Case {
            name: "line_sample",
            instruction: "image_sample_lz v[4:7], v10, s[0:7], s[8:11] dmask:0xf dim:SQ_RSRC_IMG_1D",
            texture: t(6).descriptor,
            sampler: scene.nearest,
            input: Box::new(|lane| [center(lane % 4, 4), 0, 0, 0]),
            expect: Box::new(|lane| texel(t(6), 0, lane % 4, 0, 0)),
        },
        Case {
            name: "unnormalized_coordinates",
            instruction: "image_sample_lz v[4:7], v[10:11], s[0:7], s[8:11] dmask:0xf dim:SQ_RSRC_IMG_2D unorm",
            texture: t(0).descriptor,
            sampler: scene.nearest,
            input: Box::new(|lane| {
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
            input: Box::new(|lane| {
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
