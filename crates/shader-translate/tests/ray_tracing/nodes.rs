use crate::harness::{Heap, Query};
use crate::scene::Rng;

/// Nodes written one after another, 128 bytes apart, so any pointer type reads inside them.
#[derive(Default)]
pub struct Nodes {
    pub bytes: Vec<u8>,
}

fn put(bytes: &mut Vec<u8>, words: &[u32]) {
    for word in words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
}

impl Nodes {
    fn next_pointer(&self, kind: u32) -> u32 {
        ((self.bytes.len() / 64) as u32) << 3 | kind
    }

    fn pad(&mut self) {
        self.bytes.resize(self.bytes.len().div_ceil(128) * 128, 0);
    }

    pub fn box32(&mut self, children: [u32; 4], boxes: [[f32; 6]; 4]) -> u32 {
        let pointer = self.next_pointer(5);
        put(&mut self.bytes, &children);
        for b in boxes {
            put(&mut self.bytes, &b.map(f32::to_bits));
        }
        self.pad();
        pointer
    }

    pub fn box16(&mut self, children: [u32; 4], halves: [[u32; 3]; 4]) -> u32 {
        let pointer = self.next_pointer(4);
        put(&mut self.bytes, &children);
        for h in halves {
            put(&mut self.bytes, &h);
        }
        self.pad();
        pointer
    }

    pub fn leaf(&mut self, vertices: [[f32; 3]; 4], remap: u32) -> u32 {
        let pointer = self.next_pointer(0);
        for v in vertices {
            put(&mut self.bytes, &v.map(f32::to_bits));
        }
        put(&mut self.bytes, &[0x101, 7, 8, remap]);
        self.pad();
        pointer
    }

    pub fn heap(&self) -> (Heap, u64) {
        let mut heap = Heap::default();
        let base = heap.place(&self.bytes);
        (heap, base)
    }
}

/// The half-precision bits of `x`, rounded toward zero, with infinities kept.
pub fn to_half(x: f32) -> u32 {
    let bits = x.to_bits();
    let sign = (bits >> 16) & 0x8000;
    let magnitude = bits & 0x7FFF_FFFF;
    if x.is_nan() {
        return sign | 0x7E00 | ((bits >> 13) & 0x1FF);
    }
    if magnitude >= 0x477F_E000 {
        return sign | if x.is_infinite() { 0x7C00 } else { 0x7BFF };
    }
    if magnitude < 0x3880_0000 {
        let shift = 126 - (magnitude >> 23);
        if shift > 24 {
            return sign;
        }
        let mantissa = (magnitude & 0x7F_FFFF) | 0x80_0000;
        return sign | ((mantissa >> shift) & 0x3FF);
    }
    sign | (((magnitude >> 13) - (112 << 10)) & 0x7FFF)
}

pub fn child_pointer(rng: &mut Rng) -> u32 {
    match rng.below(4) {
        0 => u32::MAX,
        1 => rng.next(),
        _ => (rng.below(64) << 3) | rng.below(8),
    }
}

pub fn special_box(rng: &mut Rng, denormals: bool) -> [f32; 6] {
    let mut b = [0.0; 6];
    for a in 0..3 {
        let (mut low, mut high) = (rng.range(-3.0, 3.0), rng.range(-3.0, 3.0));
        if low > high && rng.chance(80) {
            (low, high) = (high, low);
        }
        if rng.chance(25) {
            low = rng.special(denormals);
        }
        if rng.chance(25) {
            high = rng.special(denormals);
        }
        if rng.chance(10) {
            high = low;
        }
        b[a] = low;
        b[3 + a] = high;
    }
    b
}

pub fn special_query(rng: &mut Rng, pointer: u32, denormals: bool) -> Query {
    let mut query = Query::new(pointer, 0.0, [0.0; 3], [0.0; 3]);
    for a in 0..3 {
        query.origin[a] = if rng.chance(50) {
            rng.special(denormals)
        } else {
            rng.range(-3.0, 3.0)
        };
        query.direction[a] = if rng.chance(30) {
            rng.special(denormals)
        } else {
            rng.range(-1.0, 1.0)
        };
        query.inverse[a] = if rng.chance(85) {
            1.0 / query.direction[a]
        } else {
            rng.special(denormals)
        };
    }
    query.extent = match rng.below(5) {
        0 => rng.special(denormals),
        1 => f32::INFINITY,
        _ => rng.range(0.0, 12.0),
    };
    query
}
