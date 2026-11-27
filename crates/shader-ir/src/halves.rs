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
