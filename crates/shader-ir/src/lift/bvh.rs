use rdna::{Inst, Operand};

use super::{LiftError, Lifter, unsupported, vgprs};
use crate::ir::{Bin, Cmp, Expr, Reg, Stmt, Tmp, Un};

const INVALID: u32 = u32::MAX;
const BOX16: u32 = 4;
const BOX32: u32 = 5;

/// The sorting network that orders a box node's four children nearest first.
const SORT: [(usize, usize); 5] = [(0, 2), (1, 3), (0, 1), (2, 3), (1, 2)];

impl Lifter {
    fn fc(&mut self, value: f32) -> Tmp {
        self.c(value.to_bits())
    }

    fn fop(&mut self, op: Bin, a: Tmp, b: Tmp) -> Tmp {
        self.bin(op, a, b)
    }

    fn select(&mut self, condition: Tmp, a: Tmp, b: Tmp) -> Tmp {
        self.value(Expr::Select(condition, a, b))
    }

    fn sub3(&mut self, a: [Tmp; 3], b: [Tmp; 3]) -> [Tmp; 3] {
        [0, 1, 2].map(|i| self.fop(Bin::FSub, a[i], b[i]))
    }

    fn cross(&mut self, p: [Tmp; 3], q: [Tmp; 3]) -> [Tmp; 3] {
        [(1, 2), (2, 0), (0, 1)].map(|(i, j)| {
            let first = self.fop(Bin::FMul, p[i], q[j]);
            let second = self.fop(Bin::FMul, p[j], q[i]);
            self.fop(Bin::FSub, first, second)
        })
    }

    fn dot(&mut self, p: [Tmp; 3], q: [Tmp; 3]) -> Tmp {
        let terms = [0, 1, 2].map(|i| self.fop(Bin::FMul, p[i], q[i]));
        let pair = self.fop(Bin::FAdd, terms[0], terms[1]);
        self.fop(Bin::FAdd, pair, terms[2])
    }

    /// `image_bvh_intersect_ray` and its 64-bit form, as AMD's ray-tracing library emulates them for this
    /// hardware: a box node gives its children whose boxes the ray enters within its extent, nearest first when
    /// the descriptor enables sorting and the rest invalid; a triangle node gives the hit distance's numerator
    /// and denominator and the two barycentric numerators the triangle ID selects; any other node gives all
    /// ones. Nodes sit at the descriptor's base times 256 plus the pointer without its type bits times 8.
    pub(super) fn bvh_intersect(&mut self, inst: &Inst, wide: bool) -> Result<(), LiftError> {
        let arg = |i: usize| inst.args.get(i).ok_or_else(|| unsupported(inst));
        if inst.mods.iter().any(|m| m == "a16") {
            return Err(unsupported(inst));
        }
        let (data, count) = vgprs(arg(0)?, inst)?;
        let regs: Vec<u8> = match &arg(1)?.operand {
            Operand::Vgpr { reg, count } => (0..*count).map(|i| reg + i).collect(),
            Operand::VgprList(regs) => regs.clone(),
            _ => return Err(unsupported(inst)),
        };
        let Operand::Sgpr {
            reg: descriptor,
            count: 4,
        } = arg(2)?.operand
        else {
            return Err(unsupported(inst));
        };
        if count != 4 || regs.len() != if wide { 12 } else { 11 } {
            return Err(unsupported(inst));
        }
        let words: Vec<Tmp> = regs
            .iter()
            .map(|r| self.value(Expr::Get(Reg::V(*r))))
            .collect();
        let (pointer, ray) = if wide {
            (self.bin(Bin::Pack64, words[0], words[1]), &words[2..])
        } else {
            (self.un(Un::Zext, words[0]), &words[1..])
        };
        let extent = ray[0];
        let origin = [ray[1], ray[2], ray[3]];
        let direction = [ray[4], ray[5], ray[6]];
        let inverse = [ray[7], ray[8], ray[9]];
        let desc: Vec<Tmp> = (0..4)
            .map(|i| self.value(Expr::Get(Reg::S(descriptor + i))))
            .collect();

        let byte = self.c(0xFF);
        let high = self.bin(Bin::And, desc[1], byte);
        let base = self.bin(Bin::Pack64, desc[0], high);
        let eight = self.c64(8);
        let base = self.bin(Bin::Shl64, base, eight);
        let clear = self.c64(!7);
        let offset = self.bin(Bin::And64, pointer, clear);
        let three = self.c64(3);
        let offset = self.bin(Bin::Shl64, offset, three);
        let node = self.bin(Bin::Add64, base, offset);
        let low = self.un(Un::Lo, pointer);
        let seven = self.c(7);
        let kind = self.bin(Bin::And, low, seven);
        let (box16, box32) = (self.c(BOX16), self.c(BOX32));
        let is_box16 = self.cmp(Cmp::Eq, kind, box16);
        let is_box32 = self.cmp(Cmp::Eq, kind, box32);
        let is_box = self.either(is_box16, is_box32);
        let last_triangle = self.c(3);
        let is_triangle = self.cmp(Cmp::LeU, kind, last_triangle);
        let load = |me: &mut Self, word: u64, when: Tmp| {
            let step = me.c64(4 * word);
            let address = me.bin(Bin::Add64, node, step);
            me.value(Expr::Load {
                when: Some(when),
                address,
            })
        };

        let mut box_words: Vec<Tmp> = (0..16).map(|i| load(self, i, is_box)).collect();
        box_words.extend(
            (16..32)
                .map(|i| load(self, i, is_box32))
                .collect::<Vec<_>>(),
        );
        let sixteen = self.c(16);
        let zero = self.fc(0.0);
        let infinity = self.fc(f32::INFINITY);
        let negative_infinity = self.fc(f32::NEG_INFINITY);
        let invalid = self.c(INVALID);
        let twenty_three = self.c(23);
        let grow = self.bin(Bin::ShrU, desc[1], twenty_three);
        let grow = self.bin(Bin::And, grow, byte);
        let grow = self.un(Un::CvtF32U32, grow);
        let epsilon = self.fc(f32::from_bits(0x3380_0000));
        let grow = self.fop(Bin::FMul, grow, epsilon);
        let one = self.fc(1.0);
        let scale = self.fop(Bin::FAdd, grow, one);
        let mut children = Vec::new();
        let mut keys = Vec::new();
        for child in 0..4 {
            let wide_box = |i: usize| box_words[4 + 6 * child + i];
            let packed = [0, 1, 2].map(|i| box_words[4 + 3 * child + i]);
            let mut halves = Vec::new();
            for word in packed {
                halves.push(self.un(Un::F16ToF32, word));
                let upper = self.bin(Bin::ShrU, word, sixteen);
                halves.push(self.un(Un::F16ToF32, upper));
            }
            let mut lower = [0; 3];
            let mut upper = [0; 3];
            for axis in 0..3 {
                lower[axis] = self.select(is_box16, halves[axis], wide_box(axis));
                upper[axis] = self.select(is_box16, halves[3 + axis], wide_box(3 + axis));
            }
            let near_planes = self.sub3(lower, origin);
            let far_planes = self.sub3(upper, origin);
            let mut entries = Vec::new();
            let mut exits = Vec::new();
            for axis in 0..3 {
                let near = self.fop(Bin::FMul, near_planes[axis], inverse[axis]);
                let far = self.fop(Bin::FMul, far_planes[axis], inverse[axis]);
                let forward = self.cmp(Cmp::FGe, inverse[axis], zero);
                entries.push(self.select(forward, near, far));
                exits.push(self.select(forward, far, near));
            }
            let entry = self.fop(Bin::FMax, entries[0], entries[1]);
            let entry = self.fop(Bin::FMax, entry, entries[2]);
            let exit = self.fop(Bin::FMin, exits[0], exits[1]);
            let exit = self.fop(Bin::FMin, exit, exits[2]);
            let first = self.fop(Bin::FMax, entry, zero);
            let last = self.fop(Bin::FMin, exit, extent);
            let entry_nan = self.cmp(Cmp::FU, entry, entry);
            let exit_nan = self.cmp(Cmp::FU, exit, exit);
            let nan = self.either(entry_nan, exit_nan);
            let first = self.select(nan, infinity, first);
            let last = self.select(nan, negative_infinity, last);
            let grown = self.fop(Bin::FMul, last, scale);
            let hit = self.cmp(Cmp::FLe, first, grown);
            children.push(self.select(hit, box_words[child], invalid));
            keys.push(first);
        }
        let sorting = self.c(1 << 31);
        let sorting = self.bin(Bin::And, desc[1], sorting);
        let none = self.c(0);
        let sorting = self.cmp(Cmp::Ne, sorting, none);
        for (a, b) in SORT {
            let present = self.cmp(Cmp::Ne, children[b], invalid);
            let nearer = self.cmp(Cmp::FLt, keys[b], keys[a]);
            let earlier = self.both(present, nearer);
            let empty = self.cmp(Cmp::Eq, children[a], invalid);
            let swap = self.either(earlier, empty);
            let swap = self.both(swap, sorting);
            let (child_a, child_b) = (children[a], children[b]);
            let (key_a, key_b) = (keys[a], keys[b]);
            children[a] = self.select(swap, child_b, child_a);
            children[b] = self.select(swap, child_a, child_b);
            keys[a] = self.select(swap, key_b, key_a);
            keys[b] = self.select(swap, key_a, key_b);
        }

        let triangle: Vec<Tmp> = (0..16).map(|i| load(self, i, is_triangle)).collect();
        let vertex = |k: usize| [0, 1, 2].map(|i| triangle[3 * k + i]);
        let one_kind = self.c(1);
        let second = self.cmp(Cmp::Eq, kind, one_kind);
        let (v0, v1, v2, v3) = (vertex(0), vertex(1), vertex(2), vertex(3));
        let a = [0, 1, 2].map(|i| self.select(second, v1[i], v0[i]));
        let b = [0, 1, 2].map(|i| self.select(second, v3[i], v1[i]));
        let c = v2;
        let e1 = self.sub3(b, a);
        let e2 = self.sub3(c, a);
        let e3 = self.sub3(origin, a);
        let s1 = self.cross(direction, e2);
        let s2 = self.cross(e3, e1);
        let numerator = self.dot(e2, s2);
        let denominator = self.dot(s1, e1);
        let i_numerator = self.dot(e3, s1);
        let j_numerator = self.dot(direction, s2);
        let reciprocal = self.un(Un::FRcp, denominator);
        let t = self.fop(Bin::FMul, numerator, reciprocal);
        let u = self.fop(Bin::FMul, i_numerator, reciprocal);
        let v = self.fop(Bin::FMul, j_numerator, reciprocal);
        let uv = self.fop(Bin::FAdd, u, v);
        let misses = [
            self.cmp(Cmp::FLt, u, zero),
            self.cmp(Cmp::FGt, u, one),
            self.cmp(Cmp::FLt, v, zero),
            self.cmp(Cmp::FGt, uv, one),
            self.cmp(Cmp::FLt, t, zero),
        ];
        let mut missed = misses[0];
        for miss in &misses[1..] {
            missed = self.either(missed, *miss);
        }
        let numerator = self.select(missed, infinity, numerator);
        let denominator = self.select(missed, one, denominator);
        let rest = self.fop(Bin::FSub, denominator, i_numerator);
        let rest = self.fop(Bin::FSub, rest, j_numerator);
        let barycentrics = [rest, i_numerator, j_numerator];
        let id = triangle[15];
        let stride = self.c(8);
        let shift = self.bin(Bin::Mul, kind, stride);
        let sources = self.bin(Bin::ShrU, id, shift);
        let mut picked = Vec::new();
        for field in [0, 2] {
            let amount = self.c(field);
            let index = self.bin(Bin::ShrU, sources, amount);
            let three = self.c(3);
            let index = self.bin(Bin::And, index, three);
            let (one, two) = (self.c(1), self.c(2));
            let is_one = self.cmp(Cmp::Eq, index, one);
            let is_two = self.cmp(Cmp::Eq, index, two);
            let value = self.select(is_two, barycentrics[2], barycentrics[0]);
            picked.push(self.select(is_one, barycentrics[1], value));
        }
        let hit = [numerator, denominator, picked[0], picked[1]];

        let mut results = Vec::new();
        for k in 0..4 {
            let other = self.select(is_triangle, hit[k], invalid);
            results.push(self.select(is_box, children[k], other));
        }
        for (k, value) in results.into_iter().enumerate() {
            self.stmts.push(Stmt::SetLane(data + k as u8, value));
        }
        Ok(())
    }
}
