use testkit::{Memory, Wave, run};

fn wave(wave64: bool) -> Wave {
    let mut wave = Wave::new(wave64);
    for lane in 0..wave.lanes() {
        wave.vgpr[0][lane] = lane as u32;
    }
    wave
}

fn buffer_descriptor(wave: &mut Wave, first: usize, base: u32, records: u32) {
    wave.sgpr[first] = base;
    wave.sgpr[first + 1] = 0;
    wave.sgpr[first + 2] = records;
    wave.sgpr[first + 3] = 0;
}

fn exec(source: &str, wave: &mut Wave, memory: &mut Memory) -> usize {
    let code = rdna::assemble(source, !wave.wave64).unwrap();
    run(&code, wave, memory).unwrap()
}

#[test]
fn scalar_loop_sums_one_to_ten() {
    let mut wave = wave(false);
    let mut memory = Memory::new(64);
    let steps = exec(
        "s_mov_b32 s0, 0
         s_mov_b32 s1, 1
         s_add_u32 s0, s0, s1
         s_add_u32 s1, s1, 1
         s_cmp_le_u32 s1, 10
         s_cbranch_scc1 -4
         s_endpgm",
        &mut wave,
        &mut memory,
    );
    assert_eq!(wave.sgpr[0], 55);
    assert_eq!(steps, 2 + 10 * 4 + 1);
}

#[test]
fn per_lane_math_stores_to_a_buffer() {
    let mut wave = wave(false);
    let mut memory = Memory::new(4096);
    buffer_descriptor(&mut wave, 4, 0, 1024);
    exec(
        "v_mad_u32_u24 v1, v0, 3, 1
         v_lshlrev_b32_e32 v2, 2, v0
         buffer_store_dword v1, v2, s[4:7], 0 offen
         s_endpgm",
        &mut wave,
        &mut memory,
    );
    for lane in 0..32 {
        assert_eq!(memory.word(lane), 3 * lane as u32 + 1);
    }
}

#[test]
fn exec_masking_runs_both_sides_of_an_if() {
    let mut wave = wave(false);
    let mut memory = Memory::new(16);
    exec(
        "v_cmp_gt_u32_e32 vcc_lo, 16, v0
         s_and_saveexec_b32 s8, vcc_lo
         v_mov_b32_e32 v1, 0x64
         s_andn2_b32 exec_lo, s8, exec_lo
         v_mov_b32_e32 v1, 0xc8
         s_or_b32 exec_lo, exec_lo, s8
         s_endpgm",
        &mut wave,
        &mut memory,
    );
    for lane in 0..32 {
        assert_eq!(
            wave.vgpr[1][lane],
            if lane < 16 { 100 } else { 200 },
            "lane {lane}"
        );
    }
    assert_eq!(wave.exec(), 0xFFFF_FFFF);
}

#[test]
fn lds_reverses_lanes() {
    let mut wave = wave(false);
    let mut memory = Memory::new(16);
    exec(
        "v_lshlrev_b32_e32 v2, 2, v0
         ds_write_b32 v2, v0
         s_waitcnt lgkmcnt(0)
         v_sub_nc_u32_e32 v3, 31, v0
         v_lshlrev_b32_e32 v3, 2, v3
         ds_read_b32 v4, v3
         s_endpgm",
        &mut wave,
        &mut memory,
    );
    for lane in 0..32 {
        assert_eq!(wave.vgpr[4][lane], 31 - lane as u32);
    }
}

#[test]
fn wave64_masks_and_lane_counts() {
    let mut wave = wave(true);
    let mut memory = Memory::new(16);
    exec(
        "v_mbcnt_lo_u32_b32 v5, -1, 0
         v_mbcnt_hi_u32_b32 v5, -1, v5
         v_cmp_lt_u32_e64 s[10:11], v0, 40
         s_bcnt1_i32_b64 s12, s[10:11]
         v_cndmask_b32_e64 v6, 7, 9, s[10:11]
         v_readlane_b32 s13, v0, 50
         s_endpgm",
        &mut wave,
        &mut memory,
    );
    for lane in 0..64 {
        assert_eq!(wave.vgpr[5][lane], lane as u32);
        assert_eq!(wave.vgpr[6][lane], if lane < 40 { 9 } else { 7 });
    }
    assert_eq!(wave.sgpr[12], 40);
    assert_eq!(wave.sgpr[13], 50);
}

#[test]
fn float_math_with_modifiers() {
    let mut wave = wave(false);
    let mut memory = Memory::new(16);
    exec(
        "v_cvt_f32_u32_e32 v1, v0
         v_mul_f32_e32 v2, 0.5, v1
         v_fma_f32 v3, v1, 2.0, -v2
         v_add_f32_e64 v4, v1, -1.0 clamp
         v_max_f32_e64 v5, -|v1|, -4.0
         s_endpgm",
        &mut wave,
        &mut memory,
    );
    for lane in 0..32 {
        let x = lane as f32;
        assert_eq!(f32::from_bits(wave.vgpr[2][lane]), x * 0.5);
        assert_eq!(
            f32::from_bits(wave.vgpr[3][lane]),
            x.mul_add(2.0, -(x * 0.5))
        );
        assert_eq!(
            f32::from_bits(wave.vgpr[4][lane]),
            (x - 1.0).clamp(0.0, 1.0)
        );
        assert_eq!(f32::from_bits(wave.vgpr[5][lane]), (-x).max(-4.0));
    }
}

#[test]
fn scalar_and_global_memory() {
    let mut wave = wave(false);
    let mut memory = Memory::new(4096);
    memory.write(0x20, 0x1111).unwrap();
    memory.write(0x24, 0x2222).unwrap();
    wave.sgpr[2] = 0x10;
    wave.sgpr[3] = 0;
    exec(
        "s_load_dwordx2 s[10:11], s[2:3], 0x10
         s_waitcnt lgkmcnt(0)
         v_lshlrev_b32_e32 v1, 2, v0
         v_add_nc_u32_e32 v2, s10, v0
         global_store_dword v1, v2, s[2:3] offset:256
         s_endpgm",
        &mut wave,
        &mut memory,
    );
    assert_eq!((wave.sgpr[10], wave.sgpr[11]), (0x1111, 0x2222));
    for lane in 0..32 {
        assert_eq!(
            memory.read(0x10 + 256 + 4 * lane as u64).unwrap(),
            0x1111 + lane as u32
        );
    }
}

#[test]
fn unsupported_instructions_are_reported() {
    let mut wave = wave(false);
    let mut memory = Memory::new(16);
    let code = rdna::assemble("v_frexp_mant_f64_e32 v[0:1], v[2:3]", true).unwrap();
    let error = run(&code, &mut wave, &mut memory).unwrap_err();
    assert!(
        matches!(error, testkit::ExecError::Unsupported { .. }),
        "{error}"
    );
}
