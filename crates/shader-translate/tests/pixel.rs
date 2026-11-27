mod common;

use common::{gpu, spirv_val};
use gpu::{FirstStage, Gpu, Resource, Sampler, Target, Texture, TextureFormat, TextureView};
use rspirv::binary::Assemble;
use rspirv::dr::{Builder, Operand};
use rspirv::spirv::{
    AddressingModel, BuiltIn, Capability, Decoration, ExecutionModel, FunctionControl, MemoryModel,
    StorageClass,
};
use shader_translate::images::{self, NUMBERS, Number, VIEWS, View};
use shader_translate::{DepthOutput, FloatMode, PUSH_HEAP, PUSH_WORDS, PixelOptions, PixelOutput};

const SIZE: u32 = 8;

/// One triangle covering the target: framebuffer corners (0, 0), (2W, 0) and (0, 2H) at depth 0.5, each
/// vertex passing its own words for every attribute.
fn vertex_shader(attributes: &[[[u32; 4]; 3]]) -> Vec<u32> {
    let mut b = Builder::new();
    b.set_version(1, 3);
    b.capability(Capability::Shader);
    b.memory_model(AddressingModel::Logical, MemoryModel::GLSL450);
    let void = b.type_void();
    let f32_ = b.type_float(32, None);
    let vec4 = b.type_vector(f32_, 4);
    let i32_ = b.type_int(32, 1);
    let u32_ = b.type_int(32, 0);
    let uvec4 = b.type_vector(u32_, 4);
    let three = b.constant_bit32(u32_, 3);
    let float = |b: &mut Builder, v: f32| b.constant_bit32(f32_, v.to_bits());
    let ptr_in = b.type_pointer(None, StorageClass::Input, i32_);
    let index = b.variable(ptr_in, None, StorageClass::Input, None);
    b.decorate(
        index,
        Decoration::BuiltIn,
        [Operand::BuiltIn(BuiltIn::VertexIndex)],
    );
    let ptr_position = b.type_pointer(None, StorageClass::Output, vec4);
    let position = b.variable(ptr_position, None, StorageClass::Output, None);
    b.decorate(
        position,
        Decoration::BuiltIn,
        [Operand::BuiltIn(BuiltIn::Position)],
    );
    let mut corners = Vec::new();
    for (x, y) in [(-1.0, -1.0), (3.0, -1.0), (-1.0, 3.0)] {
        let parts = [
            float(&mut b, x),
            float(&mut b, y),
            float(&mut b, 0.5),
            float(&mut b, 1.0),
        ];
        corners.push(b.constant_composite(vec4, parts));
    }
    let corner_array = b.type_array(vec4, three);
    let corner_values = b.constant_composite(corner_array, corners);
    let word_array = b.type_array(uvec4, three);
    let ptr_out_words = b.type_pointer(None, StorageClass::Output, uvec4);
    let mut outputs = Vec::new();
    for (location, vertices) in attributes.iter().enumerate() {
        let per_vertex: Vec<u32> = vertices
            .iter()
            .map(|words| {
                let parts: Vec<u32> = words.iter().map(|w| b.constant_bit32(u32_, *w)).collect();
                b.constant_composite(uvec4, parts)
            })
            .collect();
        let values = b.constant_composite(word_array, per_vertex);
        let var = b.variable(ptr_out_words, None, StorageClass::Output, None);
        b.decorate(
            var,
            Decoration::Location,
            [Operand::LiteralBit32(location as u32)],
        );
        outputs.push((var, values));
    }
    let function_type = b.type_function(void, []);
    let main = b
        .begin_function(void, None, FunctionControl::NONE, function_type)
        .unwrap();
    b.begin_block(None).unwrap();
    let ptr_corners = b.type_pointer(None, StorageClass::Function, corner_array);
    let corner_var = b.variable(
        ptr_corners,
        None,
        StorageClass::Function,
        Some(corner_values),
    );
    let ptr_words = b.type_pointer(None, StorageClass::Function, word_array);
    let word_vars: Vec<u32> = outputs
        .iter()
        .map(|(_, values)| b.variable(ptr_words, None, StorageClass::Function, Some(*values)))
        .collect();
    let which = b.load(i32_, None, index, None, []).unwrap();
    let ptr_corner = b.type_pointer(None, StorageClass::Function, vec4);
    let element = b
        .access_chain(ptr_corner, None, corner_var, [which])
        .unwrap();
    let value = b.load(vec4, None, element, None, []).unwrap();
    b.store(position, value, None, []).unwrap();
    let ptr_word = b.type_pointer(None, StorageClass::Function, uvec4);
    for ((output, _), var) in outputs.iter().zip(&word_vars) {
        let element = b.access_chain(ptr_word, None, *var, [which]).unwrap();
        let value = b.load(uvec4, None, element, None, []).unwrap();
        b.store(*output, value, None, []).unwrap();
    }
    b.ret().unwrap();
    b.end_function().unwrap();
    let mut interface = vec![index, position];
    interface.extend(outputs.iter().map(|(var, _)| *var));
    b.entry_point(ExecutionModel::Vertex, main, "main", interface);
    b.module().assemble()
}

fn f(value: f32) -> u32 {
    value.to_bits()
}

struct Case<'a> {
    name: &'a str,
    source: &'a str,
    inputs: u32,
    user_sgprs: u32,
    number: Number,
    depth: Option<DepthOutput>,
    expect: Box<dyn Fn(u32, u32) -> [u32; 4] + 'a>,
    expect_depth: Option<Box<dyn Fn(u32, u32) -> f32 + 'a>>,
    close: bool,
}

fn pixel_gpu() -> Option<Gpu> {
    let gpu = gpu()?;
    if !gpu.info.pixel_shaders || !gpu.info.descriptor_indexing {
        eprintln!(
            "skipping: {} cannot run translated pixel shaders",
            gpu.info.name
        );
        return None;
    }
    Some(gpu)
}

fn near(a: u32, b: u32) -> bool {
    let (x, y) = (f32::from_bits(a), f32::from_bits(b));
    a == b || (x - y).abs() <= 1e-3 * y.abs().max(1.0)
}

#[allow(clippy::too_many_arguments)]
fn run(
    gpu: &Gpu,
    case: &Case<'_>,
    wave64: bool,
    attributes: &[[[u32; 4]; 3]],
    user: &[u32],
    resources: &mut [(u32, Resource)],
) {
    let source = if wave64 {
        case.source.to_string()
    } else {
        case.source.replace(
            "s_and_b64 exec, exec, vcc",
            "s_and_b32 exec_lo, exec_lo, vcc_lo",
        )
    };
    let code = rdna::assemble(&source, !wave64).unwrap_or_else(|e| panic!("{e:?}\n{source}"));
    let function = shader_ir::lift(&code, wave64).unwrap_or_else(|e| panic!("{e}\n{source}"));
    let options = PixelOptions {
        user_sgprs: case.user_sgprs,
        inputs: case.inputs,
        outputs: vec![PixelOutput {
            target: 0,
            number: case.number,
        }],
        depth: case.depth,
        float: FloatMode {
            nan_preserve: gpu.info.nan_preserve_32,
            flush_denormals: false,
            preserve_denormals: gpu.info.denorm_preserve_32,
        },
    };
    let fragment = shader_translate::pixel(&function, &options)
        .unwrap_or_else(|e| panic!("{}: {e}\n{function}", case.name));
    let lanes = if wave64 { 64 } else { 32 };
    spirv_val(&fragment, &format!("pixel_{}_{lanes}", case.name));
    let vertex = vertex_shader(attributes);
    spirv_val(&vertex, "pixel_vertex");
    let format = match case.number {
        Number::Float => TextureFormat::Rgba32Float,
        Number::Uint => TextureFormat::Rgba32Uint,
        Number::Sint => TextureFormat::Rgba32Sint,
    };
    let texels = (SIZE * SIZE) as usize;
    let mut targets = [Target {
        format,
        clear: [0; 4],
        data: vec![0; texels * 16],
    }];
    let mut depth_target = case.depth.map(|_| Target {
        format: TextureFormat::Depth32Float,
        clear: [f(1.0), 0, 0, 0],
        data: vec![0; texels * 4],
    });
    let mut heap = vec![0u32; 256];
    let user = user.to_vec();
    gpu.draw(
        FirstStage::Vertex {
            spirv: &vertex,
            buffer: None,
            geometry: None,
        },
        &fragment,
        [SIZE, SIZE],
        &mut targets,
        depth_target.as_mut(),
        &mut [&mut heap],
        |addresses| {
            let mut push = vec![0u32; PUSH_WORDS as usize];
            push[..user.len()].copy_from_slice(&user);
            push[PUSH_HEAP as usize] = addresses[0] as u32;
            push[PUSH_HEAP as usize + 1] = (addresses[0] >> 32) as u32;
            push
        },
        resources,
    )
    .unwrap();
    assert_eq!(gpu::validation_errors(), 0, "{}", case.name);
    let words: Vec<u32> = targets[0]
        .data
        .chunks(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect();
    for y in 0..SIZE {
        for x in 0..SIZE {
            let at = ((y * SIZE + x) * 4) as usize;
            let got = &words[at..at + 4];
            let want = (case.expect)(x, y);
            let fine = if case.close {
                got.iter().zip(&want).all(|(a, b)| near(*a, *b))
            } else {
                got == want
            };
            assert!(
                fine,
                "{} wave64 {wave64} pixel {x},{y}: got {got:08x?} expected {want:08x?}",
                case.name
            );
        }
    }
    if let (Some(target), Some(expect)) = (&depth_target, &case.expect_depth) {
        for y in 0..SIZE {
            for x in 0..SIZE {
                let at = ((y * SIZE + x) * 4) as usize;
                let got = f32::from_le_bytes(target.data[at..at + 4].try_into().unwrap());
                let want = expect(x, y);
                assert!(
                    (got - want).abs() < 1e-6,
                    "{} depth {x},{y}: {got} vs {want}",
                    case.name
                );
            }
        }
    }
}

fn centre(i: u32) -> f32 {
    i as f32 + 0.5
}

#[test]
fn pixel_shaders_read_inputs_and_write_targets() {
    let Some(gpu) = pixel_gpu() else { return };
    let span = (2 * SIZE) as f32;
    let attributes = [[
        [f(0.0), f(0.0), f(7.0), f(3.0)],
        [f(span), f(0.0), f(7.0), f(5.0)],
        [f(0.0), f(span), f(7.0), f(9.0)],
    ]];
    let cases = [
        Case {
            name: "position",
            source: "v_mov_b32_e32 v4, 1.0\nexp mrt0 v2, v3, v4, v4 done vm\ns_endpgm",
            inputs: 0x302,
            user_sgprs: 0,
            number: Number::Float,
            depth: None,
            expect: Box::new(|x, y| [f(centre(x)), f(centre(y)), f(1.0), f(1.0)]),
            expect_depth: None,
            close: false,
        },
        Case {
            name: "interpolation",
            source: "s_mov_b32 m0, s0
                 v_interp_p1_f32 v4, v0, attr0.x
                 v_interp_p2_f32 v4, v1, attr0.x
                 v_interp_p1_f32 v5, v0, attr0.y
                 v_interp_p2_f32 v5, v1, attr0.y
                 v_interp_p1_f32 v6, v0, attr0.z
                 v_interp_p2_f32 v6, v1, attr0.z
                 v_interp_mov_f32 v7, p0, attr0.w
                 exp mrt0 v4, v5, v6, v7 done vm
                 s_endpgm",
            inputs: 0x2,
            user_sgprs: 0,
            number: Number::Float,
            depth: None,
            expect: Box::new(|x, y| [f(centre(x)), f(centre(y)), f(7.0), f(3.0)]),
            expect_depth: None,
            close: true,
        },
        Case {
            name: "discard",
            source: "v_cmp_le_f32_e32 vcc, 4.0, v2
                 s_and_b64 exec, exec, vcc
                 v_mov_b32_e32 v4, 1.0
                 exp mrt0 v4, v4, v4, v4 done vm
                 s_endpgm",
            inputs: 0x302,
            user_sgprs: 0,
            number: Number::Float,
            depth: None,
            expect: Box::new(|x, _| if x >= 4 { [f(1.0); 4] } else { [0; 4] }),
            expect_depth: None,
            close: false,
        },
        Case {
            name: "compressed",
            source: "v_cvt_pkrtz_f16_f32_e32 v4, v2, v3
                 v_cvt_pkrtz_f16_f32_e64 v5, 1.0, 0.5
                 exp mrt0 v4, v4, v5, v5 done compr vm
                 s_endpgm",
            inputs: 0x302,
            user_sgprs: 0,
            number: Number::Float,
            depth: None,
            expect: Box::new(|x, y| [f(centre(x)), f(centre(y)), f(1.0), f(0.5)]),
            expect_depth: None,
            close: false,
        },
        Case {
            name: "depth",
            source: "v_mul_f32_e32 v5, 0.5, v4
                 exp mrtz v5, off, off, off
                 v_mov_b32_e32 v6, 2.0
                 exp mrt0 v6, v6, v6, v6 done vm
                 s_endpgm",
            inputs: 0x702,
            user_sgprs: 0,
            number: Number::Float,
            depth: Some(DepthOutput::Any),
            expect: Box::new(|_, _| [f(2.0); 4]),
            expect_depth: Some(Box::new(|_, _| 0.25)),
            close: false,
        },
        Case {
            name: "facing_integers",
            source: "v_cvt_u32_f32_e32 v5, v2
                 v_cvt_u32_f32_e32 v6, v3
                 exp mrt0 v5, v6, v4, v4 done vm
                 s_endpgm",
            inputs: 0x1302,
            user_sgprs: 0,
            number: Number::Uint,
            depth: None,
            expect: Box::new(|x, y| [x, y, 0, 0]),
            expect_depth: None,
            close: false,
        },
    ];
    for case in &cases {
        for wave64 in [false, true] {
            run(&gpu, case, wave64, &attributes, &[], &mut []);
        }
    }
}

fn descriptor(id: u32) -> [u32; 8] {
    [id << 8, 77 << 20 | 0x3F, 0x1234_0000 | id, id, 0, 0, 0, 0]
}

#[test]
fn pixel_shaders_sample_with_screen_derivatives() {
    let Some(gpu) = pixel_gpu() else { return };
    let mip = |level: u32, x: u32, y: u32| {
        [
            f((100 * level + x + 16 * y) as f32),
            f(level as f32),
            0,
            f(1.0),
        ]
    };
    let mut data = Vec::new();
    for level in 0..2 {
        let side = 16 >> level;
        for y in 0..side {
            for x in 0..side {
                data.extend(mip(level, x, y).iter().flat_map(|w| w.to_le_bytes()));
            }
        }
    }
    let texture = Texture {
        view: TextureView::D2,
        format: TextureFormat::Rgba32Float,
        width: 16,
        height: 16,
        depth: 1,
        mips: 2,
        data,
    };
    let key = descriptor(1);
    let sampler = [0x10, 0x5010, 0, 0];
    let mut resources = Vec::new();
    for view in VIEWS {
        for number in NUMBERS {
            let tex_view = match view {
                View::D2 => TextureView::D2,
                View::D2Array => TextureView::D2Array,
                View::D3 => TextureView::D3,
            };
            let format = match number {
                Number::Float => TextureFormat::Rgba32Float,
                Number::Uint => TextureFormat::Rgba32Uint,
                Number::Sint => TextureFormat::Rgba32Sint,
            };
            let mut list = vec![Texture {
                view: tex_view,
                format,
                width: 1,
                height: 1,
                depth: 1,
                mips: 1,
                data: vec![0; 16],
            }];
            if view == View::D2 && number == Number::Float {
                list.push(texture.clone());
            }
            resources.push((images::binding(view, number), Resource::Textures(list)));
        }
    }
    let plain = Sampler {
        linear: false,
        repeat: false,
        compare: None,
    };
    resources.push((
        images::TEXTURE_TABLE,
        Resource::Buffer(images::table(&[(key, images::slot(1, Number::Float))]).unwrap()),
    ));
    resources.push((
        images::SAMPLER_TABLE,
        Resource::Buffer(images::table(&[(images::sampler_key(&sampler), 1)]).unwrap()),
    ));
    resources.push((images::SAMPLERS, Resource::Samplers(vec![plain, plain])));
    let mut user = key.to_vec();
    user.extend(sampler);
    for wave64 in [false, true] {
        let case = Case {
            name: "implicit_lod",
            source: "v_mul_f32_e32 v4, 0x3e000000, v2
                 v_mul_f32_e32 v5, 0x3e000000, v3
                 image_sample v[6:9], v[4:5], s[0:7], s[8:11] dmask:0xf dim:SQ_RSRC_IMG_2D
                 s_waitcnt vmcnt(0)
                 exp mrt0 v6, v7, v8, v9 done vm
                 s_endpgm",
            inputs: 0x302,
            user_sgprs: 12,
            number: Number::Float,
            depth: None,
            expect: Box::new(|x, y| mip(1, x, y)),
            expect_depth: None,
            close: false,
        };
        run(&gpu, &case, wave64, &[], &user, &mut resources);
        let case = Case {
            name: "implicit_lod_offset",
            source: "v_mov_b32_e32 v10, 0x3f01
                 v_mul_f32_e32 v11, 0x3e000000, v2
                 v_mul_f32_e32 v12, 0x3e000000, v3
                 image_sample_o v[6:9], v[10:12], s[0:7], s[8:11] dmask:0xf dim:SQ_RSRC_IMG_2D
                 s_waitcnt vmcnt(0)
                 exp mrt0 v6, v7, v8, v9 done vm
                 s_endpgm",
            inputs: 0x302,
            user_sgprs: 12,
            number: Number::Float,
            depth: None,
            expect: Box::new(|x, y| mip(1, (x + 1).min(7), y.saturating_sub(1))),
            expect_depth: None,
            close: false,
        };
        run(&gpu, &case, wave64, &[], &user, &mut resources);
    }
}

#[test]
fn ordered_pixel_shaders_run_inside_an_interlock() {
    let Some(gpu) = pixel_gpu() else { return };
    if !gpu.info.pixel_interlock {
        eprintln!("skipping: {} lacks pixel interlock", gpu.info.name);
        return;
    }
    let case = Case {
        name: "ordered",
        source: "s_mov_b32 s18, s15
             s_bfe_u32 s1, s18, 0x2001c
             s_bfe_u32 s2, s18, 0xa0000
             s_lshl_b32 s1, s1, 1
             s_bfe_u32 s5, s18, 0xa0010
             s_not_b32 s4, s2
             s_lshr_b32 s0, s18, 31
             s_or_b32 s1, s1, 1
             s_add_i32 s5, s5, s4
             s_cmp_eq_u32 s0, 0
             s_setreg_b32 hwreg(25, 0, 3), s1
             s_cselect_b64 s[0:1], exec, 0
             s_add_i32 s2, src_239, s4
             s_cmp_gt_u32 s2, s5
             s_cselect_b64 s[2:3], exec, 0
             s_or_b64 s[0:1], s[0:1], s[2:3]
             s_and_b64 vcc, exec, s[0:1]
             s_cbranch_vccnz 4
             s_sleep 0xffff
             s_add_i32 s0, src_239, s4
             s_cmp_lt_u32 s5, s0
             s_cbranch_scc0 -4
             v_mov_b32_e32 v4, 1.0
             exp mrt0 v2, v3, v4, v4 done vm
             s_endpgm",
        inputs: 0x302,
        user_sgprs: 0,
        number: Number::Float,
        depth: None,
        expect: Box::new(|x, y| [f(centre(x)), f(centre(y)), f(1.0), f(1.0)]),
        expect_depth: None,
        close: false,
    };
    for wave64 in [false, true] {
        run(&gpu, &case, wave64, &[], &[], &mut []);
    }
}
