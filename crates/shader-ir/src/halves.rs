//! Where a 64-lane program depends on lanes 32-63, which graphics stages on 32-wide GPUs do not have.

use rdna::{Inst, Operand};

/// What makes a 64-lane program approximate when only lanes 0-31 exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Hazard {
    /// `v_readlane_b32` of lane 32 or above.
    ReadsUpperLane,
    /// `v_readlane_b32` of a lane held in a register.
    ReadsComputedLane,
    /// `v_writelane_b32` of lane 32 or above, or of a lane held in a register.
    WritesUpperLane,
    /// `ds_bpermute_b32` or `ds_permute_b32`, which may pick any of the 64 lanes.
    PermutesAllLanes,
}

impl Hazard {
    pub fn describe(self) -> &'static str {
        match self {
            Hazard::ReadsUpperLane => "reads a lane past 31",
            Hazard::ReadsComputedLane => "reads a lane chosen at run time",
            Hazard::WritesUpperLane => "writes a lane past 31 or one chosen at run time",
            Hazard::PermutesAllLanes => "permutes across all 64 lanes",
        }
    }
}

fn lane_operand(inst: &Inst) -> Option<&Operand> {
    inst.args.get(2).map(|arg| &arg.operand)
}

/// The cross-lane instructions of a 64-lane program whose result depends on lanes 32-63; every other
/// cross-lane operation (DPP, swizzles, 16-lane permutes, first-lane reads, ballots) stays within lanes 0-31.
pub fn upper_lane_hazards(code: &[u8]) -> Vec<Hazard> {
    hazards_of(&rdna::decode_all(code, false, 0).insts)
}

pub(crate) fn hazards_of(insts: &[Inst]) -> Vec<Hazard> {
    let mut found = Vec::new();
    for inst in insts {
        let name = inst.name.strip_suffix("_e64").unwrap_or(&inst.name);
        let hazard = match name {
            "v_readlane_b32" => match lane_operand(inst) {
                Some(Operand::Int(lane)) if *lane & 63 < 32 => None,
                Some(Operand::Int(_)) => Some(Hazard::ReadsUpperLane),
                _ => Some(Hazard::ReadsComputedLane),
            },
            "v_writelane_b32" => match lane_operand(inst) {
                Some(Operand::Int(lane)) if *lane & 63 < 32 => None,
                _ => Some(Hazard::WritesUpperLane),
            },
            "ds_bpermute_b32" | "ds_permute_b32" => Some(Hazard::PermutesAllLanes),
            _ => None,
        };
        if let Some(hazard) = hazard
            && !found.contains(&hazard)
        {
            found.push(hazard);
        }
    }
    found.sort();
    found
}

/// Every way a 64-lane program names lanes 32-63, whether or not it needs them to exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Pattern {
    /// A hazard: the result depends on lanes 32-63 holding values.
    Hazard(Hazard),
    /// Names `exec_hi` or `vcc_hi` as a scalar operand.
    MaskHighHalf,
    /// Turns lanes on in exec by setting bits no comparison produced (`exec = -1`, `s_not_b64 exec`, ...).
    ExecRaised,
    /// Counts or searches the bits of a 64-bit mask (`s_bcnt1_i32_b64`, `s_ff1_i32_b64`, `s_flbit_i32_b64`).
    MaskCount,
    /// Rounds a 64-bit mask to whole quads (`s_wqm_b64`, `s_quadmask_b64`).
    QuadMask,
    /// `v_mbcnt_hi_u32_b32`, which counts mask bits below a lane only for lanes 32-63.
    MbcntHigh,
    /// Branches on all 64 bits of vcc or exec.
    MaskBranch,
    /// Other 64-bit scalar arithmetic on exec or vcc (`s_and_b64 exec, ...`, `s_andn2_saveexec_b64`, ...).
    MaskArithmetic,
    /// A comparison or carry that writes a 64-bit lane mask to a scalar register pair.
    LaneMaskResult,
    /// A lane mask read per lane by `v_cndmask_b32` or a carry-in.
    LaneMaskSelect,
    /// `ds_swizzle_b32`, `v_permlane16_b32`, `v_permlanex16_b32`, which act within 32 lanes.
    WithinThirtyTwo,
    /// DPP, which acts within 16 lanes.
    WithinSixteen,
}

impl Pattern {
    pub fn describe(self) -> &'static str {
        match self {
            Pattern::Hazard(hazard) => hazard.describe(),
            Pattern::MaskHighHalf => "names exec_hi or vcc_hi",
            Pattern::ExecRaised => "switches on lanes with exec = -1 or a complemented exec",
            Pattern::MaskCount => "counts or searches the bits of a 64-bit mask",
            Pattern::QuadMask => "rounds a 64-bit mask to whole quads",
            Pattern::MbcntHigh => "counts mask bits for lanes 32-63 (v_mbcnt_hi)",
            Pattern::MaskBranch => "branches on all 64 bits of vcc or exec",
            Pattern::MaskArithmetic => "64-bit scalar arithmetic on exec or vcc",
            Pattern::LaneMaskResult => "writes a 64-bit lane mask (compare, carry)",
            Pattern::LaneMaskSelect => "selects per lane by a 64-bit mask",
            Pattern::WithinThirtyTwo => "swizzle or permlane inside 32 lanes",
            Pattern::WithinSixteen => "DPP inside 16 lanes",
        }
    }

    /// Whether lanes 32-63 must exist and hold values for the result to be right.
    pub fn needs_upper_lanes(self) -> bool {
        matches!(self, Pattern::Hazard(_))
    }
}

fn names(inst: &Inst, wanted: &[&str]) -> bool {
    inst.args
        .iter()
        .any(|arg| matches!(&arg.operand, Operand::Named(name) if wanted.contains(&name.as_str())))
}

fn is_minus_one(inst: &Inst, index: usize) -> bool {
    matches!(
        inst.args.get(index).map(|arg| &arg.operand),
        Some(Operand::Int(-1))
    )
}

fn writes_exec(inst: &Inst) -> bool {
    matches!(
        inst.args.first().map(|arg| &arg.operand),
        Some(Operand::Named(name)) if name == "exec"
    )
}

/// Every pattern of 64-lane lane naming in `code`, sorted and without repeats.
pub fn lane_patterns(code: &[u8]) -> Vec<Pattern> {
    patterns_of(&rdna::decode_all(code, false, 0).insts)
}

pub(crate) fn patterns_of(insts: &[Inst]) -> Vec<Pattern> {
    let mut found: Vec<Pattern> = hazards_of(insts).into_iter().map(Pattern::Hazard).collect();
    let mut add = |pattern: Pattern| {
        if !found.contains(&pattern) {
            found.push(pattern);
        }
    };
    for inst in insts {
        let name = inst.name.as_str();
        let base = name
            .strip_suffix("_e64")
            .or_else(|| name.strip_suffix("_e32"))
            .unwrap_or(name);
        let scalar = name.starts_with("s_");
        if names(inst, &["exec_hi", "vcc_hi"]) {
            add(Pattern::MaskHighHalf);
        }
        let on_mask = names(inst, &["exec", "vcc"]);
        match base {
            "s_bcnt1_i32_b64" | "s_bcnt0_i32_b64" | "s_ff1_i32_b64" | "s_ff0_i32_b64"
            | "s_flbit_i32_b64" | "s_flbit_i32_i64" => add(Pattern::MaskCount),
            "s_wqm_b64" | "s_quadmask_b64" => add(Pattern::QuadMask),
            "v_mbcnt_hi_u32_b32" => add(Pattern::MbcntHigh),
            "s_cbranch_vccz" | "s_cbranch_vccnz" | "s_cbranch_execz" | "s_cbranch_execnz" => {
                add(Pattern::MaskBranch)
            }
            "ds_swizzle_b32" | "v_permlane16_b32" | "v_permlanex16_b32" => {
                add(Pattern::WithinThirtyTwo)
            }
            "v_cndmask_b32" | "v_addc_co_u32" | "v_subb_co_u32" | "v_subbrev_co_u32" => {
                add(Pattern::LaneMaskSelect)
            }
            _ => {}
        }
        if name.ends_with("_dpp") {
            add(Pattern::WithinSixteen);
        }
        if name.starts_with("v_cmp") || name.starts_with("v_cmpx") {
            add(Pattern::LaneMaskResult);
        }
        let wide = scalar && (name.ends_with("_b64") || name.contains("saveexec_b64"));
        if wide && on_mask {
            let raising = match base {
                "s_mov_b64" | "s_or_b64" | "s_orn2_b64" => {
                    writes_exec(inst) && is_minus_one(inst, 1)
                }
                "s_or_saveexec_b64" | "s_orn2_saveexec_b64" => is_minus_one(inst, 1),
                "s_not_b64" | "s_nand_b64" | "s_nor_b64" | "s_xnor_b64" => writes_exec(inst),
                "s_nand_saveexec_b64" | "s_nor_saveexec_b64" | "s_xnor_saveexec_b64" => true,
                _ => false,
            };
            if raising {
                add(Pattern::ExecRaised);
            } else if !matches!(
                base,
                "s_bcnt1_i32_b64"
                    | "s_bcnt0_i32_b64"
                    | "s_ff1_i32_b64"
                    | "s_ff0_i32_b64"
                    | "s_flbit_i32_b64"
                    | "s_flbit_i32_i64"
                    | "s_wqm_b64"
                    | "s_quadmask_b64"
            ) {
                add(Pattern::MaskArithmetic);
            }
        }
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patterns(source: &str) -> Vec<Pattern> {
        let code = rdna::assemble(source, false).expect("assembles");
        lane_patterns(&code)
    }

    #[test]
    fn lower_lane_reads_and_writes_are_not_hazards() {
        let found = patterns(
            "v_readlane_b32 s0, v1, 31
             v_writelane_b32 v1, s0, 5
             v_readfirstlane_b32 s1, v2
             s_endpgm",
        );
        assert!(found.iter().all(|p| !p.needs_upper_lanes()), "{found:?}");
    }

    #[test]
    fn upper_lane_access_by_number_or_register_is_a_hazard() {
        for (source, hazard) in [
            (
                "v_readlane_b32 s0, v1, 40\ns_endpgm",
                Hazard::ReadsUpperLane,
            ),
            (
                "v_readlane_b32 s0, v1, s2\ns_endpgm",
                Hazard::ReadsComputedLane,
            ),
            (
                "v_writelane_b32 v1, s0, 63\ns_endpgm",
                Hazard::WritesUpperLane,
            ),
            (
                "v_writelane_b32 v1, s0, s3\ns_endpgm",
                Hazard::WritesUpperLane,
            ),
            (
                "ds_bpermute_b32 v1, v2, v3\ns_endpgm",
                Hazard::PermutesAllLanes,
            ),
            (
                "ds_permute_b32 v1, v2, v3\ns_endpgm",
                Hazard::PermutesAllLanes,
            ),
        ] {
            let found = patterns(source);
            assert!(
                found.contains(&Pattern::Hazard(hazard)),
                "{source}: {found:?}"
            );
        }
    }

    #[test]
    fn mask_patterns_are_told_apart() {
        let found = patterns(
            "s_mov_b64 exec, -1
             s_bcnt1_i32_b64 s4, vcc
             s_wqm_b64 s[6:7], vcc
             s_and_b64 s[8:9], exec, vcc
             v_mbcnt_hi_u32_b32 v1, -1, v1
             v_cmp_lt_u32 vcc, 5, v1
             v_cndmask_b32 v2, 0, v3, vcc
             s_mov_b32 s5, vcc_hi
             s_endpgm",
        );
        for pattern in [
            Pattern::ExecRaised,
            Pattern::MaskCount,
            Pattern::QuadMask,
            Pattern::MaskArithmetic,
            Pattern::MbcntHigh,
            Pattern::LaneMaskResult,
            Pattern::LaneMaskSelect,
            Pattern::MaskHighHalf,
        ] {
            assert!(found.contains(&pattern), "{pattern:?} in {found:?}");
        }
        assert!(found.iter().all(|p| !p.needs_upper_lanes()));
    }

    #[test]
    fn narrowing_exec_is_not_raising_it() {
        let found = patterns(
            "s_and_b64 exec, exec, vcc
             s_andn2_b64 exec, exec, vcc
             s_and_saveexec_b64 s[4:5], vcc
             s_endpgm",
        );
        assert!(!found.contains(&Pattern::ExecRaised), "{found:?}");
        assert!(found.contains(&Pattern::MaskArithmetic));
    }
}
