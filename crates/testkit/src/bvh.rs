use rdna::{Inst, Operand};

use crate::Result;
use crate::state::{Memory, Wave, arg, bad};
use crate::valu::{fmax, fmin, nan_rule};

const INVALID: u32 = u32::MAX;

fn add(a: f32, b: f32) -> f32 {
    nan_rule(a + b, &[a, b])
}

fn sub(a: f32, b: f32) -> f32 {
    nan_rule(a - b, &[a, b])
}

fn mul(a: f32, b: f32) -> f32 {
    nan_rule(a * b, &[a, b])
}

fn cross(p: [f32; 3], q: [f32; 3]) -> [f32; 3] {
    [
        sub(mul(p[1], q[2]), mul(p[2], q[1])),
        sub(mul(p[2], q[0]), mul(p[0], q[2])),
        sub(mul(p[0], q[1]), mul(p[1], q[0])),
    ]
}

fn dot(p: [f32; 3], q: [f32; 3]) -> f32 {
    add(add(mul(p[0], q[0]), mul(p[1], q[1])), mul(p[2], q[2]))
}

fn minus(p: [f32; 3], q: [f32; 3]) -> [f32; 3] {
    [sub(p[0], q[0]), sub(p[1], q[1]), sub(p[2], q[2])]
}

struct Ray {
    extent: f32,
    origin: [f32; 3],
    direction: [f32; 3],
    inverse: [f32; 3],
}

fn half(bits: u32) -> f32 {
    f32::from_bits(rdna::format::f16_to_f32(bits & 0xFFFF, true))
}

/// The four children a box node's boxes let the ray reach, nearest first when sorting.
fn boxes(words: &[u32], sixteen: bool, ray: &Ray, grow: u32, sorting: bool) -> [u32; 4] {
    let float = |i: usize| f32::from_bits(words[i]);
    let scale = add(mul(grow as f32, f32::from_bits(0x3380_0000)), 1.0);
    let mut children = [0; 4];
    let mut keys = [0.0f32; 4];
    for child in 0..4 {
        let (lower, upper) = if sixteen {
            let p = [0, 1, 2].map(|i| words[4 + 3 * child + i]);
            (
                [half(p[0]), half(p[0] >> 16), half(p[1])],
                [half(p[1] >> 16), half(p[2]), half(p[2] >> 16)],
            )
        } else {
            let at = 4 + 6 * child;
            (
                [float(at), float(at + 1), float(at + 2)],
                [float(at + 3), float(at + 4), float(at + 5)],
            )
        };
        let near = minus(lower, ray.origin);
        let far = minus(upper, ray.origin);
        let mut entries = [0.0f32; 3];
        let mut exits = [0.0f32; 3];
        for axis in 0..3 {
            let a = mul(near[axis], ray.inverse[axis]);
            let b = mul(far[axis], ray.inverse[axis]);
            if ray.inverse[axis] >= 0.0 {
                (entries[axis], exits[axis]) = (a, b);
            } else {
                (entries[axis], exits[axis]) = (b, a);
            }
        }
        let entry = fmax(fmax(entries[0], entries[1]), entries[2]);
        let exit = fmin(fmin(exits[0], exits[1]), exits[2]);
        let (mut first, mut last) = (fmax(entry, 0.0), fmin(exit, ray.extent));
        if entry.is_nan() || exit.is_nan() {
            (first, last) = (f32::INFINITY, f32::NEG_INFINITY);
        }
        children[child] = if first <= mul(last, scale) {
            words[child]
        } else {
            INVALID
        };
        keys[child] = first;
    }
    if sorting {
        for (a, b) in [(0, 2), (1, 3), (0, 1), (2, 3), (1, 2)] {
            if (children[b] != INVALID && keys[b] < keys[a]) || children[a] == INVALID {
                children.swap(a, b);
                keys.swap(a, b);
            }
        }
    }
    children
}

/// The hit distance's numerator and denominator and the barycentric numerators the triangle ID picks. A NaN
/// barycentric counts as inside, as in AMD's emulation, so the range test stays two comparisons.
#[allow(clippy::manual_range_contains)]
fn triangle(words: &[u32], kind: u32, ray: &Ray) -> [u32; 4] {
    let vertex = |k: usize| [0, 1, 2].map(|i| f32::from_bits(words[3 * k + i]));
    let (a, b, c) = if kind == 1 {
        (vertex(1), vertex(3), vertex(2))
    } else {
        (vertex(0), vertex(1), vertex(2))
    };
    let e1 = minus(b, a);
    let e2 = minus(c, a);
    let e3 = minus(ray.origin, a);
    let s1 = cross(ray.direction, e2);
    let s2 = cross(e3, e1);
    let mut numerator = dot(e2, s2);
    let mut denominator = dot(s1, e1);
    let i = dot(e3, s1);
    let j = dot(ray.direction, s2);
    let reciprocal = nan_rule(1.0 / denominator, &[denominator]);
    let (t, u, v) = (
        mul(numerator, reciprocal),
        mul(i, reciprocal),
        mul(j, reciprocal),
    );
    if u < 0.0 || u > 1.0 || v < 0.0 || add(u, v) > 1.0 || t < 0.0 {
        (numerator, denominator) = (f32::INFINITY, 1.0);
    }
    let barycentrics = [sub(sub(denominator, i), j), i, j];
    let sources = words[15] >> (8 * kind);
    let pick = |field: u32| match (sources >> field) & 3 {
        1 => barycentrics[1],
        2 => barycentrics[2],
        _ => barycentrics[0],
    };
    [
        numerator.to_bits(),
        denominator.to_bits(),
        pick(0).to_bits(),
        pick(2).to_bits(),
    ]
}

/// `image_bvh_intersect_ray` and `image_bvh64_intersect_ray`, from AMD's emulation of the instruction.
pub fn intersect(name: &str, inst: &Inst, wave: &mut Wave, memory: &Memory) -> Result<()> {
    let wide = name == "image_bvh64_intersect_ray";
    let Operand::Vgpr { reg: data, .. } = arg(inst, 0)?.operand else {
        return Err(bad(inst));
    };
    let regs: Vec<usize> = match &arg(inst, 1)?.operand {
        Operand::Vgpr { reg, count } => (0..*count).map(|i| usize::from(reg + i)).collect(),
        Operand::VgprList(regs) => regs.iter().map(|r| usize::from(*r)).collect(),
        _ => return Err(bad(inst)),
    };
    let Operand::Sgpr { reg: desc, .. } = arg(inst, 2)?.operand else {
        return Err(bad(inst));
    };
    let desc: Vec<u32> = (0..4).map(|i| wave.sgpr[usize::from(desc) + i]).collect();
    let base = (u64::from(desc[0]) | (u64::from(desc[1] & 0xFF) << 32)) << 8;
    let grow = (desc[1] >> 23) & 0xFF;
    let sorting = desc[1] >> 31 != 0;
    for lane in wave.active_lanes() {
        let word = |i: usize| wave.vgpr[regs[i]][lane];
        let (pointer, first) = if wide {
            (u64::from(word(0)) | (u64::from(word(1)) << 32), 2)
        } else {
            (u64::from(word(0)), 1)
        };
        let float = |i: usize| f32::from_bits(word(first + i));
        let ray = Ray {
            extent: float(0),
            origin: [float(1), float(2), float(3)],
            direction: [float(4), float(5), float(6)],
            inverse: [float(7), float(8), float(9)],
        };
        let node = base.wrapping_add((pointer & !7) << 3);
        let kind = (pointer & 7) as u32;
        let load = |count: u64| -> Result<Vec<u32>> {
            (0..count).map(|i| memory.read(node + 4 * i)).collect()
        };
        let result = match kind {
            4 => boxes(&load(16)?, true, &ray, grow, sorting),
            5 => boxes(&load(32)?, false, &ray, grow, sorting),
            0..=3 => triangle(&load(16)?, kind, &ray),
            _ => [INVALID; 4],
        };
        for (k, value) in result.into_iter().enumerate() {
            wave.vgpr[usize::from(data) + k][lane] = value;
        }
    }
    Ok(())
}
