//! The tree builder: a Morton order of the element boxes, a binary radix tree over it (Karras 2012), and the
//! collapse of that tree to four-wide nodes by keeping the nodes at even depth. The GPU builder runs the same
//! passes in the same arithmetic, so both write the same bytes.

use crate::half;
use crate::layout::{EMPTY_BOX, INVALID, NodeKind};
use crate::{NodeBuilder, NodeOutput, NodeParams, Result};

const LEAF: u32 = 1 << 31;

/// The grid element centres are quantized on: the lowest centre and a power-of-two scale that fits the
/// widest axis into 1024 cells, so the arithmetic is exact and identical on any hardware.
#[derive(Debug, Clone, Copy)]
pub struct Quantizer {
    pub minimum: [f32; 3],
    pub scale: f32,
}

fn is_empty(b: &[f32; 6]) -> bool {
    b[0] > b[3]
}

fn centre(b: &[f32; 6]) -> [f32; 3] {
    [0, 1, 2].map(|a| (b[a] + b[3 + a]) * 0.5)
}

pub fn quantizer(boxes: &[[f32; 6]]) -> Quantizer {
    let mut minimum = [f32::INFINITY; 3];
    let mut maximum = [f32::NEG_INFINITY; 3];
    for b in boxes.iter().filter(|b| !is_empty(b)) {
        let c = centre(b);
        for a in 0..3 {
            minimum[a] = minimum[a].min(c[a]);
            maximum[a] = maximum[a].max(c[a]);
        }
    }
    let mut extent = 0.0f32;
    for a in 0..3 {
        if maximum[a] >= minimum[a] {
            extent = extent.max(maximum[a] - minimum[a]);
        }
    }
    Quantizer {
        minimum,
        scale: scale_for(extent),
    }
}

pub fn scale_for(extent: f32) -> f32 {
    let bits = extent.to_bits();
    let exponent = ((bits >> 23) & 255) as i32;
    if exponent == 0 || exponent == 255 {
        return 1.0;
    }
    let ceiling = exponent - 127 + i32::from(bits & 0x7F_FFFF != 0);
    let shift = (10 - ceiling).clamp(-126, 127);
    f32::from_bits(((shift + 127) as u32) << 23)
}

fn spread(mut x: u32) -> u32 {
    x = (x | (x << 16)) & 0x0300_00FF;
    x = (x | (x << 8)) & 0x0300_F00F;
    x = (x | (x << 4)) & 0x030C_30C3;
    (x | (x << 2)) & 0x0924_9249
}

pub fn morton(b: &[f32; 6], q: &Quantizer) -> u32 {
    if is_empty(b) {
        return 0x3FFF_FFFF;
    }
    let c = centre(b);
    let cell = |a: usize| ((c[a] - q.minimum[a]) * q.scale).clamp(0.0, 1023.0) as u32;
    (spread(cell(0)) << 2) | (spread(cell(1)) << 1) | spread(cell(2))
}

fn delta(keys: &[u32], i: i32, j: i32) -> i32 {
    if j < 0 || j as usize >= keys.len() {
        return -1;
    }
    let (a, b) = (keys[i as usize], keys[j as usize]);
    if a == b {
        32 + (i as u32 ^ j as u32).leading_zeros() as i32
    } else {
        (a ^ b).leading_zeros() as i32
    }
}

/// The two children of internal node `i`, as sorted positions with `LEAF` set for an element.
pub fn radix_children(keys: &[u32], i: i32) -> (u32, u32) {
    let direction = if delta(keys, i, i + 1) - delta(keys, i, i - 1) > 0 {
        1
    } else {
        -1
    };
    let minimum = delta(keys, i, i - direction);
    let mut limit = 2;
    while delta(keys, i, i + limit * direction) > minimum {
        limit *= 2;
    }
    let mut length = 0;
    let mut step = limit / 2;
    while step >= 1 {
        if delta(keys, i, i + (length + step) * direction) > minimum {
            length += step;
        }
        step /= 2;
    }
    let j = i + length * direction;
    let common = delta(keys, i, j);
    let mut split = 0;
    let mut t = length;
    loop {
        t = (t + 1) >> 1;
        if delta(keys, i, i + (split + t) * direction) > common {
            split += t;
        }
        if t <= 1 {
            break;
        }
    }
    let gamma = i + split * direction + direction.min(0);
    let left = if i.min(j) == gamma {
        gamma as u32 | LEAF
    } else {
        gamma as u32
    };
    let right = if i.max(j) == gamma + 1 {
        (gamma + 1) as u32 | LEAF
    } else {
        (gamma + 1) as u32
    };
    (left, right)
}

pub fn union(a: &[f32; 6], b: &[f32; 6]) -> [f32; 6] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].min(b[2]),
        a[3].max(b[3]),
        a[4].max(b[4]),
        a[5].max(b[5]),
    ]
}

/// Child `slot`'s box in a node record's words: 32-bit floats after the four pointers, or six halves in three
/// words, minimum rounded down and maximum rounded up.
pub fn write_box(words: &mut [u32], kind: NodeKind, slot: usize, b: &[f32; 6]) {
    match kind {
        NodeKind::Box32 => {
            for (k, value) in b.iter().enumerate() {
                words[4 + 6 * slot + k] = value.to_bits();
            }
        }
        NodeKind::Box16 => {
            let h = [
                half::round(b[0], false),
                half::round(b[1], false),
                half::round(b[2], false),
                half::round(b[3], true),
                half::round(b[4], true),
                half::round(b[5], true),
            ]
            .map(u32::from);
            words[4 + 3 * slot] = h[0] | (h[1] << 16);
            words[5 + 3 * slot] = h[2] | (h[3] << 16);
            words[6 + 3 * slot] = h[4] | (h[5] << 16);
        }
    }
}

/// The builder that runs on the CPU.
pub struct Cpu;

impl NodeBuilder for Cpu {
    fn build(&mut self, boxes: &[[f32; 6]], params: &NodeParams) -> Result<NodeOutput> {
        Ok(build_nodes(boxes, params))
    }
}

/// The nodes and parent table for at least two element boxes.
pub fn build_nodes(boxes: &[[f32; 6]], params: &NodeParams) -> NodeOutput {
    let n = boxes.len();
    let q = quantizer(boxes);
    let mut order: Vec<(u32, u32)> = boxes
        .iter()
        .enumerate()
        .map(|(i, b)| (morton(b, &q), i as u32))
        .collect();
    order.sort_unstable();
    let keys: Vec<u32> = order.iter().map(|p| p.0).collect();
    let sorted: Vec<u32> = order.iter().map(|p| p.1).collect();
    let m = n - 1;

    let kids: Vec<(u32, u32)> = (0..m).map(|i| radix_children(&keys, i as i32)).collect();
    let mut parent = vec![INVALID; m];
    for (i, kid) in kids.iter().enumerate() {
        for c in [kid.0, kid.1] {
            if c & LEAF == 0 {
                parent[c as usize] = i as u32;
            }
        }
    }
    let depth: Vec<u32> = (0..m)
        .map(|i| {
            let mut d = 0;
            let mut at = i;
            while parent[at] != INVALID {
                at = parent[at] as usize;
                d += 1;
            }
            d
        })
        .collect();
    let mut inclusive = vec![0u32; m];
    let mut running = 0;
    for i in 0..m {
        running += u32::from(depth[i].is_multiple_of(2));
        inclusive[i] = running;
    }
    let node_count = running;

    let mut node_box = vec![EMPTY_BOX; m];
    let mut by_depth: Vec<usize> = (0..m).collect();
    by_depth.sort_by_key(|i| std::cmp::Reverse(depth[*i]));
    let child_box = |c: u32, node_box: &[[f32; 6]]| {
        if c & LEAF != 0 {
            boxes[sorted[(c & !LEAF) as usize] as usize]
        } else {
            node_box[c as usize]
        }
    };
    for i in by_depth {
        let merged = union(
            &child_box(kids[i].0, &node_box),
            &child_box(kids[i].1, &node_box),
        );
        node_box[i] = merged;
    }

    let words = params.node_units as usize * 16;
    let table = params.node_table_base as usize + node_count as usize * params.node_units as usize;
    let mut nodes = vec![0u32; node_count as usize * words];
    let mut parents = vec![0u32; table];
    let kind = if params.node_units == 1 {
        NodeKind::Box16
    } else {
        NodeKind::Box32
    };
    let element_unit =
        |c: u32| params.element_base + sorted[(c & !LEAF) as usize] * params.element_stride;
    let node_unit = |i: u32| params.node_base + (inclusive[i as usize] - 1) * params.node_units;
    let table_index = |unit: u32| (unit - params.element_base) as usize;
    parents[table_index(node_unit(0))] = INVALID;

    for i in 0..m {
        if !depth[i].is_multiple_of(2) {
            continue;
        }
        let mut items = [0u32; 4];
        let mut count = 0;
        for c in [kids[i].0, kids[i].1] {
            if c & LEAF == 0 {
                items[count] = kids[c as usize].0;
                items[count + 1] = kids[c as usize].1;
                count += 2;
            } else {
                items[count] = c;
                count += 1;
            }
        }
        let index = (inclusive[i] - 1) as usize;
        let record = &mut nodes[index * words..(index + 1) * words];
        let own_pointer = (node_unit(i as u32) << 3) | params.node_type;
        for slot in 0..4 {
            if slot < count {
                let c = items[slot];
                let (pointer, b, unit) = if c & LEAF != 0 {
                    let unit = element_unit(c);
                    (
                        (unit << 3) | params.element_type,
                        boxes[sorted[(c & !LEAF) as usize] as usize],
                        unit,
                    )
                } else {
                    let unit = node_unit(c);
                    ((unit << 3) | params.node_type, node_box[c as usize], unit)
                };
                record[slot] = pointer;
                write_box(record, kind, slot, &b);
                parents[table_index(unit)] = own_pointer;
            } else {
                record[slot] = INVALID;
                write_box(record, kind, slot, &EMPTY_BOX);
            }
        }
    }
    NodeOutput {
        node_count,
        nodes,
        parents,
    }
}
