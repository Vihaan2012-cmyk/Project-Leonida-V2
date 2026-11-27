use bvh_build::{
    BlasInfo, Cpu, Geometry, Header, Instance, Triangle, blas_info, build_blas, build_tlas, flags,
};

use crate::harness::Query;

pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }

    pub fn below(&mut self, n: u32) -> u32 {
        self.next() % n
    }

    pub fn chance(&mut self, percent: u32) -> bool {
        self.below(100) < percent
    }

    pub fn unit(&mut self) -> f32 {
        (self.next() & 0xFF_FFFF) as f32 / 16_777_216.0
    }

    pub fn range(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * self.unit()
    }

    pub fn point(&mut self, low: f32, high: f32) -> [f32; 3] {
        [0; 3].map(|_| self.range(low, high))
    }

    pub fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len() as u32) as usize]
    }

    /// A float that is often one of the values ray tracing code trips over.
    pub fn special(&mut self, denormals: bool) -> f32 {
        let mut values = vec![
            0.0,
            -0.0,
            1.0,
            -1.0,
            0.5,
            2.0,
            1e-3,
            -1e-3,
            1e30,
            -1e30,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NAN,
            f32::from_bits(0xFFC0_1234),
            1e-12,
            -1e-12,
        ];
        if denormals {
            values.extend([
                1e-40,
                -1e-40,
                f32::from_bits(1),
                f32::MAX,
                f32::MIN_POSITIVE,
                1e-20,
            ]);
        }
        self.pick(&values)
    }
}

pub fn triangles(rng: &mut Rng, count: usize, size: f32) -> Vec<Triangle> {
    (0..count)
        .map(|_| {
            let centre = rng.point(-10.0, 10.0);
            Triangle {
                vertices: [0; 3].map(|_| {
                    let o = rng.point(-size, size);
                    [centre[0] + o[0], centre[1] + o[1], centre[2] + o[2]]
                }),
            }
        })
        .collect()
}

pub fn geometry(triangles: Vec<Triangle>) -> Geometry {
    Geometry {
        triangles,
        flags: 1,
    }
}

pub fn blas(rng: &mut Rng, count: usize, box16: bool) -> Vec<u8> {
    let geometries = vec![
        geometry(triangles(rng, count / 2, 1.5)),
        geometry(triangles(rng, count - count / 2, 0.7)),
    ];
    let flags = if box16 { flags::MINIMIZE_MEMORY } else { 0 };
    build_blas(&geometries, flags, &mut Cpu).unwrap()
}

/// A top-level structure over instances of the BLAS placed at `bases`; its instance records name each BLAS by
/// its absolute address.
pub fn tlas(structures: &[(u64, &[u8])], transforms: &[[f32; 12]]) -> Vec<u8> {
    let instances: Vec<Instance> = transforms
        .iter()
        .enumerate()
        .map(|(i, transform)| Instance {
            transform: *transform,
            id: i as u32,
            mask: 0xFF,
            contribution: 0,
            flags: 0,
            blas: structures[i % structures.len()].0,
        })
        .collect();
    let lookup = |address: u64| -> Option<BlasInfo> {
        structures
            .iter()
            .find(|(base, _)| *base == address)
            .and_then(|(_, bytes)| blas_info(bytes).ok())
    };
    build_tlas(&instances, &lookup, &mut Cpu).unwrap()
}

pub fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

/// Every node pointer of a structure, root first.
pub fn pointers(bytes: &[u8]) -> Vec<u32> {
    let header = Header::read(bytes).unwrap();
    let mut found = Vec::new();
    let mut stack = vec![header.root];
    while let Some(pointer) = stack.pop() {
        if pointer == u32::MAX {
            continue;
        }
        found.push(pointer);
        if pointer & 7 >= 4 && pointer & 7 <= 5 {
            let at = (pointer >> 3) as usize * 64;
            for slot in (0..4).rev() {
                stack.push(word(bytes, at + 4 * slot));
            }
        }
    }
    found
}

pub fn half(bits: u32) -> f32 {
    f32::from_bits(rdna::format::f16_to_f32(bits & 0xFFFF, true))
}

/// The boxes of a box node's present children.
pub fn child_boxes(bytes: &[u8], pointer: u32) -> Vec<[f32; 6]> {
    let at = (pointer >> 3) as usize * 64;
    (0..4)
        .filter(|c| word(bytes, at + 4 * c) != u32::MAX)
        .map(|c| {
            if pointer & 7 == 4 {
                let p = [0, 1, 2].map(|i| word(bytes, at + 16 + 12 * c + 4 * i));
                [
                    half(p[0]),
                    half(p[0] >> 16),
                    half(p[1]),
                    half(p[1] >> 16),
                    half(p[2]),
                    half(p[2] >> 16),
                ]
            } else {
                [0, 1, 2, 3, 4, 5].map(|i| f32::from_bits(word(bytes, at + 16 + 24 * c + 4 * i)))
            }
        })
        .collect()
}

pub fn leaf_vertices(bytes: &[u8], pointer: u32) -> [[f32; 3]; 4] {
    let at = (pointer >> 3) as usize * 64;
    [0, 1, 2, 3].map(|v| [0, 1, 2].map(|i| f32::from_bits(word(bytes, at + 12 * v + 4 * i))))
}

fn extent_for(rng: &mut Rng, distance: f32) -> f32 {
    match rng.below(8) {
        0 => f32::INFINITY,
        1 => distance * 0.5,
        2 => distance * 0.999,
        3 => distance * 1.001,
        4 => distance * 2.0,
        5 => 1e30,
        _ => distance + rng.range(-1.0, 8.0),
    }
}

pub fn aimed(rng: &mut Rng, pointer: u32, target: [f32; 3], inside: bool) -> Query {
    let reach = if inside { 0.05 } else { rng.range(0.5, 30.0) };
    let away = rng.point(-1.0, 1.0);
    let length = (away[0] * away[0] + away[1] * away[1] + away[2] * away[2])
        .sqrt()
        .max(1e-3);
    let origin = [0, 1, 2].map(|a| target[a] + away[a] / length * reach);
    let mut direction = [0, 1, 2].map(|a| target[a] - origin[a]);
    if rng.chance(15) {
        for d in &mut direction {
            *d += rng.range(-0.01, 0.01);
        }
    }
    if rng.chance(30) {
        let scale = rng.pick(&[0.001, 0.1, 10.0, 1000.0]);
        direction = direction.map(|d| d * scale);
    }
    let distance = reach * (1.0 + rng.range(0.0, 0.5));
    Query::new(pointer, extent_for(rng, distance), origin, direction)
}

/// Rays that skim a box face or run along an edge, with a zero direction component and so an infinite inverse.
pub fn grazing(rng: &mut Rng, pointer: u32, b: [f32; 6]) -> Query {
    let axis = rng.below(3) as usize;
    let mut origin = [0; 3].map(|a| rng.range(b[a] - 1.0, b[3 + a] + 1.0));
    origin[axis] = if rng.chance(50) { b[axis] } else { b[3 + axis] };
    let mut direction = [0.0f32; 3];
    let moving = (axis + 1 + rng.below(2) as usize) % 3;
    direction[moving] = if rng.chance(50) { 1.0 } else { -1.0 };
    if rng.chance(30) {
        direction[3 - axis - moving] = rng.range(-0.5, 0.5);
    }
    for d in &mut direction {
        if *d == 0.0 && rng.chance(50) {
            *d = -0.0;
        }
    }
    Query::new(
        pointer,
        rng.pick(&[f32::INFINITY, 100.0, 5.0]),
        origin,
        direction,
    )
}

pub fn on_triangle(rng: &mut Rng, pointer: u32, v: [[f32; 3]; 3]) -> Query {
    let mut u = rng.unit();
    let mut w = rng.unit();
    match rng.below(8) {
        0 => (u, w) = (0.0, w),
        1 => (u, w) = (u, 0.0),
        2 => {
            u = 0.0;
            w = 0.0
        }
        3 => (u, w) = (u * 0.5, 1.0 - u * 0.5),
        _ => {}
    }
    if u + w > 1.0 {
        (u, w) = (1.0 - u, 1.0 - w);
    }
    let target = [0, 1, 2].map(|a| v[0][a] + u * (v[1][a] - v[0][a]) + w * (v[2][a] - v[0][a]));
    let mut query = aimed(rng, pointer, target, false);
    match rng.below(10) {
        0 => {
            let edge = [0, 1, 2].map(|a| v[1][a] - v[0][a]);
            query.direction = edge;
            query.inverse = edge.map(|d| 1.0 / d);
        }
        1 => {
            query.origin = target;
        }
        2 => {
            query.direction = query.direction.map(|d| -d);
            query.inverse = query.direction.map(|d| 1.0 / d);
        }
        _ => {}
    }
    query
}

/// Queries for every node of a structure: box nodes get rays at their children, triangle nodes rays at the
/// triangle, and each node also gets rays that miss it. With `kinds`, some queries read the node as another type.
pub fn node_queries(rng: &mut Rng, bytes: &[u8], per_node: usize, kinds: bool) -> Vec<Query> {
    let mut queries = Vec::new();
    for pointer in pointers(bytes) {
        for i in 0..per_node {
            let mut query = match pointer & 7 {
                4 | 5 => {
                    let mut boxes = child_boxes(bytes, pointer);
                    if boxes.is_empty() {
                        boxes.push([0.0; 6]);
                    }
                    let b = boxes[rng.below(boxes.len() as u32) as usize];
                    let centre = [0, 1, 2].map(|a| (b[a] + b[3 + a]) * 0.5);
                    match i % 4 {
                        3 => grazing(rng, pointer, b),
                        2 => aimed(rng, pointer, centre, true),
                        _ => aimed(rng, pointer, centre, false),
                    }
                }
                _ => {
                    let v = leaf_vertices(bytes, pointer);
                    on_triangle(rng, pointer, [v[0], v[1], v[2]])
                }
            };
            if kinds && rng.chance(10) {
                query.pointer = (pointer & !7) | rng.below(8);
            }
            queries.push(query);
        }
    }
    queries
}
