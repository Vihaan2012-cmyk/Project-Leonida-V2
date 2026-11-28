//! A 64-lane graphics program that runs 32 lanes wide has an empty upper half. These programs name lanes
//! 32-63 only through masks and in-row exchanges, so the lower half's results must not depend on what the
//! upper lanes hold, and the masks must stay consistent with an upper half that is off.

use shader_ir::{Machine, lift, run};

fn fill(machine: &mut Machine, upper_seed: u32) {
    for reg in 1..9 {
        for lane in 0..64 {
            let seed = if lane < 32 { 17 } else { upper_seed };
            let (reg, lane) = (reg as u32, lane as u32);
            let mut x = seed ^ reg.wrapping_mul(0x9E37_79B9) ^ lane.wrapping_mul(0x85EB_CA6B);
            x ^= x >> 15;
            x = x.wrapping_mul(0x2C1B_3C6D);
            x ^= x >> 12;
            machine.vgpr[reg as usize][lane as usize] = x % 4096;
        }
    }
    machine.sgpr[0] = 0x7654_3210;
    machine.sgpr[1] = 0xFEDC_BA98;
    machine.sgpr[126] = u32::MAX;
    machine.sgpr[127] = 0;
}

fn execute(source: &str, upper_seed: u32) -> Machine {
    let code = rdna::assemble(source, false).unwrap_or_else(|e| panic!("{e:?}\n{source}"));
    let function = lift(&code, true).unwrap_or_else(|e| panic!("{e}\n{source}"));
    function.verify().unwrap();
    let mut machine = Machine::new(true, 0);
    fill(&mut machine, upper_seed);
    run(&function, &mut machine).unwrap_or_else(|e| panic!("{e}\n{source}"));
    machine
}

/// Runs `source` with two different contents of lanes 32-63 and checks that lanes 0-31 of every VGPR and the
/// named SGPRs are the same.
fn lower_half_ignores_upper(source: &str, scalars: &[usize]) -> Machine {
    let a = execute(source, 1);
    let b = execute(source, 99);
    for (reg, (x, y)) in a.vgpr.iter().zip(&b.vgpr).enumerate() {
        assert_eq!(x[..32], y[..32], "v{reg}\n{source}");
    }
    for &reg in scalars {
        assert_eq!(a.sgpr[reg], b.sgpr[reg], "s{reg}\n{source}");
    }
    a
}

#[test]
fn masks_of_a_32_lane_wave_have_an_empty_upper_half() {
    let machine = execute(
        "v_mbcnt_lo_u32_b32 v10, -1, 0
         v_mbcnt_hi_u32_b32 v10, -1, v10
         v_cmp_gt_u32 vcc, 5, v10
         s_bcnt1_i32_b64 s4, vcc
         s_ff1_i32_b64 s5, vcc
         s_wqm_b64 s[6:7], vcc
         s_and_b64 s[8:9], exec, vcc
         s_not_b64 s[12:13], vcc
         s_endpgm",
        1,
    );
    for lane in 0..32 {
        assert_eq!(
            machine.vgpr[10][lane], lane as u32,
            "lane id of lane {lane}"
        );
    }
    assert_eq!(machine.sgpr[4], 5);
    assert_eq!(machine.sgpr[5], 0);
    assert_eq!([machine.sgpr[6], machine.sgpr[7]], [0xFF, 0]);
    assert_eq!([machine.sgpr[8], machine.sgpr[9]], [0x1F, 0]);
    assert_eq!(
        [machine.sgpr[12], machine.sgpr[13]],
        [0xFFFF_FFE0, u32::MAX]
    );
}

#[test]
fn comparisons_and_selects_do_not_read_upper_lanes() {
    lower_half_ignores_upper(
        "v_mbcnt_lo_u32_b32 v10, -1, 0
         v_mbcnt_hi_u32_b32 v10, -1, v10
         v_cmp_gt_u32 vcc, v2, v1
         s_bcnt1_i32_b64 s4, vcc
         s_ff1_i32_b64 s5, vcc
         s_and_b64 s[8:9], exec, vcc
         v_cndmask_b32 v5, v1, v3, vcc
         s_endpgm",
        &[4, 5, 8, 106],
    );
}

#[test]
fn switching_all_lanes_on_and_off_again_leaves_the_lower_half_alone() {
    lower_half_ignores_upper(
        "s_mov_b64 s[10:11], exec
         s_mov_b64 exec, -1
         v_add_nc_u32 v4, v1, v2
         v_cmp_gt_u32 vcc, v2, v1
         s_mov_b64 exec, s[10:11]
         v_cndmask_b32 v5, v1, v2, vcc
         s_endpgm",
        &[10, 106],
    );
}

#[test]
fn in_row_exchanges_stay_inside_their_half() {
    lower_half_ignores_upper(
        "v_add_f32_dpp v6, v1, v2 row_shr:3 row_mask:0xf bank_mask:0xf
         v_mov_b32_dpp v7, v1 row_ror:5 row_mask:0xf bank_mask:0xf
         v_mov_b32_dpp v8, v2 quad_perm:[1,0,3,2] row_mask:0xf bank_mask:0xf
         ds_swizzle_b32 v3, v1 offset:swizzle(SWAP,16)
         ds_swizzle_b32 v4, v2 offset:swizzle(REVERSE,32)
         v_permlanex16_b32 v5, v1, s0, s1
         v_permlane16_b32 v9, v2, s0, s1
         v_readfirstlane_b32 s14, v1
         v_readlane_b32 s15, v2, 31
         s_endpgm",
        &[14, 15],
    );
}
