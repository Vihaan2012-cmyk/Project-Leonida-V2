mod common;

use common::{gpu, spirv_val};
use gpu::{FirstStage, Gpu, Target, TextureFormat, VertexAttribute, VertexBuffer, VertexFormat};
use shader_translate::images::Number;
use shader_translate::{
    EsOptions, FloatMode, GsInput, GsOptions, GsOutput, MeshOptions, PUSH_HEAP, PUSH_WORDS,
    PixelOptions, PixelOutput, VertexInput, VertexOptions,
};

const SIZE: u32 = 8;

fn f(value: f32) -> u32 {
    value.to_bits()
}

fn float_mode(gpu: &Gpu) -> FloatMode {
    FloatMode {
        nan_preserve: gpu.info.nan_preserve_32,
        flush_denormals: false,
        preserve_denormals: gpu.info.denorm_preserve_32,
    }
}

fn lift(source: &str, wave64: bool) -> shader_ir::Function {
    let code = rdna::assemble(source, !wave64).unwrap_or_else(|e| panic!("{e:?}\n{source}"));
    shader_ir::lift(&code, wave64).unwrap_or_else(|e| panic!("{e}\n{source}"))
}

/// Interpolates attribute 0's x, y and z across the triangle and passes its w from the first vertex.
fn fragment(gpu: &Gpu) -> Vec<u32> {
    let function = lift(
        "s_mov_b32 m0, s0
         v_interp_p1_f32 v4, v0, attr0.x
         v_interp_p2_f32 v4, v1, attr0.x
         v_interp_p1_f32 v5, v0, attr0.y
         v_interp_p2_f32 v5, v1, attr0.y
         v_interp_p1_f32 v6, v0, attr0.z
         v_interp_p2_f32 v6, v1, attr0.z
         v_interp_mov_f32 v7, p0, attr0.w
         exp mrt0 v4, v5, v6, v7 done vm
         s_endpgm",
        false,
    );
    let options = PixelOptions {
        user_sgprs: 0,
        inputs: 0x2,
        outputs: vec![PixelOutput {
            target: 0,
            number: Number::Float,
        }],
        attribute_locations: Vec::new(),
        depth: None,
        float: float_mode(gpu),
    };
    let spirv = shader_translate::pixel(&function, &options).unwrap();
    spirv_val(&spirv, "geometry_pixel");
    spirv
}

fn draw(gpu: &Gpu, first: FirstStage<'_>, fragment: &[u32], user: &[u32], name: &str) {
    let mut targets = [Target {
        format: TextureFormat::Rgba32Float,
        clear: [0; 4],
        data: vec![0; (SIZE * SIZE * 16) as usize],
    }];
    let mut heap = vec![0u32; 64];
    gpu.draw(
        first,
        fragment,
        [SIZE, SIZE],
        &mut targets,
        None,
        &mut [&mut heap],
        |addresses| {
            let mut push = vec![0u32; PUSH_WORDS as usize];
            push[..user.len()].copy_from_slice(user);
            push[PUSH_HEAP as usize] = addresses[0] as u32;
            push[PUSH_HEAP as usize + 1] = (addresses[0] >> 32) as u32;
            push
        },
        &mut [],
    )
    .unwrap();
    assert_eq!(gpu::validation_errors(), 0, "{name}");
    let words: Vec<u32> = targets[0]
        .data
        .chunks(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect();
    for y in 0..SIZE {
        for x in 0..SIZE {
            let at = ((y * SIZE + x) * 4) as usize;
            let got = &words[at..at + 4];
            let want = [x as f32 + 0.5, y as f32 + 0.5, 7.0, 3.0];
            let close = got
                .iter()
                .zip(want)
                .all(|(a, b)| (f32::from_bits(*a) - b).abs() <= 1e-3 * b.abs().max(1.0));
            assert!(close, "{name} pixel {x},{y}: got {got:08x?}");
        }
    }
}

/// Corner `t` of a triangle covering the target, and its attribute: x and y run 0 to 16 across the
/// corners, z comes from `s{z}` and w is 3 + 2t.
fn corner_program(index: u8, z: &str) -> String {
    format!(
        "v_and_b32_e32 v20, 1, v{index}
         v_cvt_f32_u32_e32 v20, v20
         v_lshrrev_b32_e32 v21, 1, v{index}
         v_and_b32_e32 v21, 1, v21
         v_cvt_f32_u32_e32 v21, v21
         v_fma_f32 v22, v20, 4.0, -1.0
         v_fma_f32 v23, v21, 4.0, -1.0
         v_mov_b32_e32 v24, 0.5
         v_mov_b32_e32 v25, 1.0
         exp pos0 v22, v23, v24, v25 done
         v_mul_f32_e32 v20, 0x41800000, v20
         v_mul_f32_e32 v21, 0x41800000, v21
         v_mov_b32_e32 v26, {z}
         v_lshlrev_b32_e32 v27, 1, v{index}
         v_cvt_f32_u32_e32 v27, v27
         v_add_f32_e32 v27, 0x40400000, v27
         exp param0 v20, v21, v26, v27
         "
    )
}

#[test]
fn vertex_shaders_place_corners_and_pass_parameters() {
    let Some(gpu) = gpu() else { return };
    if !gpu.info.pixel_shaders || !gpu.info.descriptor_indexing {
        eprintln!("skipping: {} cannot run translated shaders", gpu.info.name);
        return;
    }
    let pixel = fragment(&gpu);
    for wave64 in [false, true] {
        let source = format!("{}s_endpgm", corner_program(0, "s0"));
        let options = VertexOptions {
            user_sgprs: 1,
            inputs: Vec::new(),
            float: float_mode(&gpu),
        };
        let spirv = shader_translate::vertex(&lift(&source, wave64), &options).unwrap();
        spirv_val(&spirv, &format!("vertex_index_{wave64}"));
        let first = FirstStage::Vertex {
            spirv: &spirv,
            buffer: None,
            geometry: None,
        };
        draw(&gpu, first, &pixel, &[f(7.0)], "vertex index");

        let source = "s_swappc_b64 s[2:3], s[4:5]
             exp pos0 v4, v5, v6, v7 done
             exp param0 v8, v9, v10, v11
             s_endpgm";
        let options = VertexOptions {
            user_sgprs: 0,
            inputs: vec![
                VertexInput {
                    register: 0,
                    number: Number::Float,
                },
                VertexInput {
                    register: 1,
                    number: Number::Float,
                },
            ],
            float: float_mode(&gpu),
        };
        let spirv = shader_translate::vertex(&lift(source, wave64), &options).unwrap();
        spirv_val(&spirv, &format!("vertex_fetch_{wave64}"));
        let corners = [
            ([-1.0, -1.0], [0.0, 0.0, 7.0, 3.0]),
            ([3.0, -1.0], [16.0, 0.0, 7.0, 5.0]),
            ([-1.0, 3.0], [0.0, 16.0, 7.0, 9.0]),
        ];
        let mut data = Vec::new();
        for ([x, y], attribute) in corners {
            for value in [x, y, 0.5, 1.0].into_iter().chain(attribute) {
                data.extend(f32::to_le_bytes(value));
            }
        }
        let buffer = VertexBuffer {
            data,
            stride: 32,
            attributes: vec![
                VertexAttribute {
                    location: 0,
                    format: VertexFormat::Float32x4,
                    offset: 0,
                },
                VertexAttribute {
                    location: 1,
                    format: VertexFormat::Float32x4,
                    offset: 16,
                },
            ],
        };
        let first = FirstStage::Vertex {
            spirv: &spirv,
            buffer: Some(&buffer),
            geometry: None,
        };
        draw(&gpu, first, &pixel, &[], "vertex fetch");
    }
}

#[test]
fn mesh_shaders_export_vertices_and_primitives() {
    let Some(gpu) = gpu() else { return };
    if !gpu.info.mesh_shaders || !gpu.info.pixel_shaders || !gpu.info.descriptor_indexing {
        eprintln!(
            "skipping: {} cannot run translated mesh shaders",
            gpu.info.name
        );
        return;
    }
    let pixel = fragment(&gpu);
    for wave64 in [false, true] {
        let vcc = if wave64 { "vcc" } else { "vcc_lo" };
        let source = format!(
            "{}v_mov_b32_e32 v28, 0x200400
             v_mov_b32_e32 v29, 0x80000000
             v_cmp_eq_u32_e32 {vcc}, 1, v5
             v_cndmask_b32_e32 v28, v28, v29, {vcc}
             s_sendmsg sendmsg(MSG_GS_ALLOC_REQ)
             exp prim v28, off, off, off done
             s_endpgm",
            corner_program(5, "s8")
        );
        let lanes = if wave64 { 64 } else { 32 };
        let options = MeshOptions {
            user_sgprs: 1,
            group_size: lanes,
            max_vertices: 3,
            max_primitives: 2,
            lds_bytes: 0,
            float: float_mode(&gpu),
        };
        let spirv = shader_translate::mesh(&lift(&source, wave64), &options).unwrap();
        spirv_val(&spirv, &format!("mesh_{wave64}"));
        let first = FirstStage::Mesh {
            spirv: &spirv,
            groups: [1, 1, 1],
        };
        draw(&gpu, first, &pixel, &[f(7.0)], "mesh");
    }
}

fn whole_wave(count: &str, wave64: bool) -> String {
    if wave64 {
        format!(
            "s_bfm_b64 exec, {count}, 0
             s_cmp_eq_u32 {count}, 64
             s_cselect_b64 exec, -1, exec"
        )
    } else {
        format!(
            "s_bfm_b32 exec_lo, {count}, 0
             s_cmp_eq_u32 {count}, 32
             s_cselect_b32 exec_lo, -1, exec_lo"
        )
    }
}

fn lane_index(target: &str, wave64: bool) -> String {
    let high = if wave64 {
        format!("\nv_mbcnt_hi_u32_b32 {target}, -1, {target}")
    } else {
        String::new()
    };
    format!("v_mbcnt_lo_u32_b32 {target}, -1, 0{high}")
}

#[test]
fn merged_vertex_and_geometry_halves_draw_through_lds() {
    let Some(gpu) = gpu() else { return };
    if !gpu.info.pixel_shaders || !gpu.info.descriptor_indexing || !gpu.info.geometry_shaders {
        eprintln!(
            "skipping: {} cannot run translated geometry shaders",
            gpu.info.name
        );
        return;
    }
    let pixel = fragment(&gpu);
    for wave64 in [false, true] {
        let first = format!(
            "s_bfe_i32 s23, s3, 0x80000
             {}
             s_swappc_b64 s[12:13], s[12:13]
             {}
             s_bfe_u32 s6, s3, 0x40018
             s_lshl_b32 s6, s6, 11
             v_mad_u32_u24 v15, v1, 32, s6
             v_and_b32_e32 v20, 1, v5
             v_cvt_f32_u32_e32 v20, v20
             v_lshrrev_b32_e32 v21, 1, v5
             v_and_b32_e32 v21, 1, v21
             v_cvt_f32_u32_e32 v21, v21
             v_fma_f32 v22, v20, 4.0, -1.0
             v_fma_f32 v23, v21, 4.0, -1.0
             v_mov_b32_e32 v24, 0.5
             v_mov_b32_e32 v25, 1.0
             v_mul_f32_e32 v20, 0x41800000, v20
             v_mul_f32_e32 v21, 0x41800000, v21
             v_mov_b32_e32 v26, s8
             v_lshlrev_b32_e32 v27, 1, v5
             v_cvt_f32_u32_e32 v27, v27
             v_add_f32_e32 v27, 0x40400000, v27
             ds_write2_b64 v15, v[20:21], v[26:27] offset1:1
             ds_write_b64 v15, v[22:23] offset:16
             ds_write_b64 v15, v[24:25] offset:24
             s_setpc_b64 s[6:7]",
            whole_wave("s23", wave64),
            lane_index("v1", wave64)
        );
        let mut second = format!(
            "s_bfe_i32 s0, s3, 0x80008
             {}
             {}
             v_lshlrev_b32_e32 v3, 2, v0
             v_and_b32_e32 v10, 0x3fffc, v3
             v_lshrrev_b32_e32 v8, 16, v0
             v_lshlrev_b32_e32 v8, 2, v8
             v_lshlrev_b32_e32 v1, 2, v1
             v_and_b32_e32 v6, 0x3fffc, v1
             s_bfe_u32 s3, s3, 0x80010
             s_mov_b32 m0, s3
             v_mad_u32_u24 v12, 0x60, v2, s2
",
            whole_wave("s0", wave64),
            lane_index("v2", wave64)
        );
        for (k, offset) in ["v10", "v8", "v6"].into_iter().enumerate() {
            second += &format!(
                "ds_read2_b64 v[20:23], {offset} offset1:1
                 ds_read2_b64 v[24:27], {offset} offset0:2 offset1:3
                 ds_write2_b64 v12, v[20:21], v[22:23] offset0:{} offset1:{}
                 ds_write2_b64 v12, v[24:25], v[26:27] offset0:{} offset1:{}
                 s_sendmsg sendmsg(MSG_GS|GS_OP_EMIT|0)
",
                4 * k,
                4 * k + 1,
                4 * k + 2,
                4 * k + 3
            );
        }
        second += "s_sendmsg sendmsg(MSG_GS|GS_OP_CUT|0)
             s_sendmsg sendmsg(MSG_GS_DONE|GS_OP_NOP)
             s_endpgm";
        let es = shader_translate::es(
            &lift(&first, wave64),
            &EsOptions {
                user_sgprs: 1,
                inputs: Vec::new(),
                item_bytes: 32,
                float: float_mode(&gpu),
            },
        )
        .unwrap();
        spirv_val(&es, &format!("first_half_{wave64}"));
        let gs = shader_translate::gs(
            &lift(&second, wave64),
            &GsOptions {
                user_sgprs: 1,
                input: GsInput::Triangles,
                item_bytes: 32,
                output: GsOutput::TriangleStrip,
                max_vertices: 3,
                vertex_bytes: 32,
                position: Some(1),
                float: float_mode(&gpu),
            },
        )
        .unwrap();
        spirv_val(&gs, &format!("second_half_{wave64}"));
        let first = FirstStage::Vertex {
            spirv: &es,
            buffer: None,
            geometry: Some(&gs),
        };
        draw(&gpu, first, &pixel, &[f(7.0)], "merged halves");
    }
}
