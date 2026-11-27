use shader_ir::{Machine, lift};
use testkit::random::{Generator, Rng, memory, start};
use testkit::{Memory, Wave};

fn check(source: &str, wave: Wave, memory: Memory) -> bool {
    let code = rdna::assemble(source, !wave.wave64).unwrap_or_else(|e| panic!("{e:?}\n{source}"));
    let mut machine = Machine {
        sgpr: wave.sgpr,
        vgpr: wave.vgpr.clone(),
        scc: wave.scc,
        lds: wave.lds.clone(),
        memory: memory.bytes.clone(),
        flush_denormals: wave.flush_denormals,
        canonical_nan: wave.canonical_nan,
        exports: wave.exports.clone(),
        messages: wave.messages.clone(),
        code_base: wave.code_base,
        attributes: wave.attributes.clone(),
    };
    let (mut reference, mut reference_memory) = (wave, memory);
    let expected = testkit::run(&code, &mut reference, &mut reference_memory);
    let function = lift(&code, reference.wave64).unwrap_or_else(|e| panic!("{e}\n{source}"));
    if let Err(error) = function.verify() {
        panic!("{error}\n{source}\n{function}");
    }
    let actual = shader_ir::run(&function, &mut machine);
    assert_eq!(
        expected.is_ok(),
        actual.is_ok(),
        "{expected:?} vs {actual:?}\n{source}\n{function}"
    );
    let context = || format!("{source}\n{function}");
    for (reg, (a, b)) in machine.sgpr.iter().zip(&reference.sgpr).enumerate() {
        assert_eq!(a, b, "s{reg}\n{}", context());
    }
    assert_eq!(machine.scc, reference.scc, "scc\n{}", context());
    for (reg, (a, b)) in machine.vgpr.iter().zip(&reference.vgpr).enumerate() {
        for (lane, (x, y)) in a.iter().zip(b).enumerate() {
            assert!(
                x == y,
                "v{reg} lane {lane}: {x:#x} vs {y:#x}\n{}",
                context()
            );
        }
    }
    assert!(machine.lds == reference.lds, "lds\n{}", context());
    assert!(
        machine.exports == reference.exports,
        "exports\n{}",
        context()
    );
    assert_eq!(
        machine.messages,
        reference.messages,
        "messages\n{}",
        context()
    );
    assert!(
        machine.memory == reference_memory.bytes,
        "memory\n{}",
        context()
    );
    expected.is_ok()
}

fn check_both(source: &str) {
    for wave64 in [false, true] {
        assert!(check(source, start(wave64, 7), memory(7)));
    }
}

#[test]
fn scalar_loop() {
    check_both(
        "s_mov_b32 s0, 0
         s_mov_b32 s1, 1
         s_add_u32 s0, s0, s1
         s_add_u32 s1, s1, 1
         s_cmp_le_u32 s1, 10
         s_cbranch_scc1 -4
         s_endpgm",
    );
}

#[test]
fn if_else_with_exec_masks() {
    assert!(check(
        "v_cmp_gt_u32_e32 vcc_lo, 16, v0
         s_and_saveexec_b32 s8, vcc_lo
         v_mov_b32_e32 v1, 0x64
         s_andn2_b32 exec_lo, s8, exec_lo
         v_mov_b32_e32 v1, 0xc8
         s_or_b32 exec_lo, exec_lo, s8
         s_endpgm",
        start(false, 1),
        memory(1),
    ));
    assert!(check(
        "v_cmp_gt_u32_e32 vcc, 40, v0
         s_and_saveexec_b64 s[8:9], vcc
         s_cbranch_execz 2
         v_mov_b32_e32 v1, 0x64
         s_andn2_b64 exec, s[8:9], exec
         v_mov_b32_e32 v1, 0xc8
         s_or_b64 exec, exec, s[8:9]
         s_endpgm",
        start(true, 1),
        memory(1),
    ));
}

#[test]
fn memory_programs() {
    assert!(check(
        "v_mad_u32_u24 v1, v0, 3, 1
         v_lshlrev_b32_e32 v2, 2, v0
         buffer_store_dword v1, v2, s[20:23], 0 offen
         buffer_load_dword v3, v2, s[20:23], s17 offen offset:8
         v_lshlrev_b32_e32 v2, 2, v0
         ds_write_b32 v2, v0
         v_sub_nc_u32_e32 v3, 31, v0
         v_lshlrev_b32_e32 v3, 2, v3
         ds_read_b32 v4, v3
         s_load_dwordx2 s[10:11], s[24:25], 0x10
         v_add_nc_u32_e32 v5, s10, v0
         global_store_dword v2, v5, s[24:25] offset:256
         global_load_dword v6, v2, s[24:25] offset:4
         s_endpgm",
        start(false, 3),
        memory(3),
    ));
}

#[test]
fn wave64_lane_operations() {
    assert!(check(
        "v_mbcnt_lo_u32_b32 v5, -1, 0
         v_mbcnt_hi_u32_b32 v5, -1, v5
         v_cmp_lt_u32_e64 s[10:11], v0, 40
         s_bcnt1_i32_b64 s12, s[10:11]
         v_cndmask_b32_e64 v6, 7, 9, s[10:11]
         v_readlane_b32 s13, v0, 50
         v_readfirstlane_b32 s14, v3
         v_writelane_b32 v7, s14, 33
         s_endpgm",
        start(true, 5),
        memory(5),
    ));
}

#[test]
fn float_modifiers() {
    check_both(
        "v_cvt_f32_u32_e32 v1, v0
         v_mul_f32_e32 v2, 0.5, v1
         v_fma_f32 v3, v1, 2.0, -v2
         v_add_f32_e64 v4, v1, -1.0 clamp
         v_max_f32_e64 v5, -|v1|, -4.0
         v_mul_f32_e64 v6, v1, v2 mul:2
         v_fmac_f32_e32 v6, v1, v2
         s_endpgm",
    );
}

#[test]
fn long_jumps_land_where_the_computed_address_points() {
    for (wave64, seed) in [(false, 3), (true, 4)] {
        let mut wave = start(wave64, seed);
        wave.code_base = 0x1234_5678_0000;
        let source = "s_mov_b32 s5, 1
             s_mov_b32 s6, 0
             s_getpc_b64 s[0:1]
             s_add_u32 s0, s0, 16
             s_addc_u32 s1, s1, 0
             s_setpc_b64 s[0:1]
             s_mov_b32 s5, 2
             s_add_u32 s6, s6, 1
             s_cmp_ge_u32 s6, 3
             s_cbranch_scc1 4
             s_getpc_b64 s[2:3]
             s_sub_u32 s2, s2, 16
             s_subb_u32 s3, s3, 0
             s_setpc_b64 s[2:3]
             s_mov_b32 s7, 4
             s_endpgm";
        assert!(check(source, wave, memory(seed)), "{source}");
    }
}

#[test]
fn unsupported_instructions_are_reported() {
    let code = rdna::assemble("v_frexp_mant_f64_e32 v[0:1], v[2:3]", true).unwrap();
    assert!(matches!(
        lift(&code, false),
        Err(shader_ir::LiftError::Unsupported { .. })
    ));
}

#[test]
fn random_programs_match_the_reference() {
    let mut completed = 0;
    for seed in 1..=1500u64 {
        for wave64 in [false, true] {
            let mut generator = Generator::new(
                Rng(seed.wrapping_mul(0x2545_F491_4F6C_DD1D) | 1),
                wave64,
                false,
            );
            let source = generator.program();
            let mut wave = start(wave64, seed);
            wave.flush_denormals = seed % 3 == 0;
            wave.code_base = seed.wrapping_mul(0x1_0000_1000);
            wave.canonical_nan = (seed % 4 == 0).then_some(0x7FFF_FFFF);
            completed += usize::from(check(&source, wave, memory(seed)));
        }
    }
    assert!(completed > 2950, "{completed}");
}
