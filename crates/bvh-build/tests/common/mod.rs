#![allow(dead_code)]

use bvh_build::{Geometry, Header, Instance, Triangle};
use testkit::{Memory, Wave};

pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }

    pub fn unit(&mut self) -> f32 {
        (self.next() & 0xFF_FFFF) as f32 / 16_777_216.0
    }

    pub fn range(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * self.unit()
    }

    pub fn point(&mut self, low: f32, high: f32) -> [f32; 3] {
        [
            self.range(low, high),
            self.range(low, high),
            self.range(low, high),
        ]
    }
}

pub fn triangles(rng: &mut Rng, count: usize, size: f32) -> Vec<Triangle> {
    (0..count)
        .map(|_| {
            let centre = rng.point(-10.0, 10.0);
            let vertices = [0, 1, 2].map(|_| {
                let o = rng.point(-size, size);
                [centre[0] + o[0], centre[1] + o[1], centre[2] + o[2]]
            });
            Triangle { vertices }
        })
        .collect()
}

pub fn geometry(triangles: Vec<Triangle>) -> Geometry {
    Geometry {
        triangles,
        flags: 1,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Ray {
    pub origin: [f32; 3],
    pub direction: [f32; 3],
    pub extent: f32,
}

pub fn rays(rng: &mut Rng, count: usize) -> Vec<Ray> {
    (0..count)
        .map(|_| {
            let origin = rng.point(-14.0, 14.0);
            let target = rng.point(-8.0, 8.0);
            let mut direction = [0, 1, 2].map(|a| target[a] - origin[a]);
            for d in &mut direction {
                if d.abs() < 1e-3 {
                    *d = 1e-3;
                }
            }
            Ray {
                origin,
                direction,
                extent: 100.0,
            }
        })
        .collect()
}

const PROGRAM: &str = "v_mul_u32_u24_e32 v13, 48, v0
global_load_dwordx4 v[1:4], v13, s[24:25]
global_load_dwordx4 v[5:8], v13, s[24:25] offset:16
global_load_dwordx4 v[9:12], v13, s[24:25] offset:32
s_waitcnt vmcnt(0)
image_bvh_intersect_ray v[14:17], [v1,v3,v4,v5,v6,v7,v8,v9,v10,v11,v12], s[0:3]
s_endpgm";

/// Structures placed in the memory the reference intersector reads, each at its own 256-byte aligned base.
pub struct Scene {
    code: Vec<u8>,
    pub memory: Memory,
    rays: u64,
    next: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    pub t: f32,
    pub geometry: u32,
    pub primitive: u32,
}

impl Scene {
    pub fn new(capacity: usize) -> Scene {
        Scene {
            code: rdna::assemble(PROGRAM, true).expect("assembles"),
            memory: Memory::new(capacity + 4096),
            rays: capacity as u64,
            next: 0,
        }
    }

    /// Places a structure and returns its base address.
    pub fn place(&mut self, bytes: &[u8]) -> u64 {
        let base = self.next;
        self.memory.bytes[base..base + bytes.len()].copy_from_slice(bytes);
        self.next = (base + bytes.len()).div_ceil(256) * 256;
        base as u64
    }

    fn step(
        &mut self,
        base: u64,
        pointer: u32,
        extent: f32,
        origin: [f32; 3],
        direction: [f32; 3],
    ) -> [u32; 4] {
        let mut ray = vec![pointer, 0, extent.to_bits()];
        ray.extend(origin.map(f32::to_bits));
        ray.extend(direction.map(f32::to_bits));
        ray.extend(direction.map(|d| (1.0 / d).to_bits()));
        for (i, word) in ray.iter().enumerate() {
            self.memory
                .write(self.rays + 4 * i as u64, *word)
                .expect("ray fits");
        }
        let mut wave = Wave::new(false);
        wave.set_mask(testkit_exec(), 1);
        wave.vgpr[0][0] = 0;
        wave.sgpr[24] = self.rays as u32;
        wave.sgpr[0] = (base >> 8) as u32;
        wave.sgpr[1] = 0x8000_0000;
        wave.sgpr[2] = u32::MAX;
        wave.sgpr[3] = 0x8900_03FF;
        testkit::run(&self.code, &mut wave, &mut self.memory).expect("the ray program runs");
        [14, 15, 16, 17].map(|r| wave.vgpr[r][0])
    }

    fn word(&self, address: u64) -> u32 {
        self.memory.read(address).expect("inside the scene")
    }

    /// The closest hit in a bottom-level structure, walked with the instruction's own results.
    pub fn trace_blas(&mut self, base: u64, ray: &Ray) -> Option<Hit> {
        let root = self.word(base + 0x30);
        let mut extent = ray.extent;
        let mut best = None;
        let mut stack = vec![root];
        while let Some(pointer) = stack.pop() {
            if pointer == u32::MAX {
                continue;
            }
            let result = self.step(base, pointer, extent, ray.origin, ray.direction);
            match pointer & 7 {
                4 | 5 => {
                    for child in result.iter().rev() {
                        if *child != u32::MAX {
                            stack.push(*child);
                        }
                    }
                }
                0 => {
                    let [numerator, denominator, ..] = result.map(f32::from_bits);
                    let t = numerator / denominator;
                    if numerator.is_finite() && denominator != 0.0 && t >= 0.0 && t <= extent {
                        extent = t;
                        let at = base + u64::from(pointer >> 3) * 64;
                        best = Some(Hit {
                            t,
                            geometry: self.word(at + 48) >> 8,
                            primitive: self.word(at + 52),
                        });
                    }
                }
                other => panic!("unexpected node type {other}"),
            }
        }
        best
    }

    /// The closest hit through a top-level structure's instances, found the way a shader does: the box
    /// nodes through the instruction, an instance record read for its transform and BLAS.
    pub fn trace_tlas(&mut self, base: u64, ray: &Ray) -> Option<(u32, Hit)> {
        let root = self.word(base + 0x30);
        let mut extent = ray.extent;
        let mut best = None;
        let mut stack = vec![root];
        while let Some(pointer) = stack.pop() {
            if pointer == u32::MAX {
                continue;
            }
            match pointer & 7 {
                4 | 5 => {
                    let result = self.step(base, pointer, extent, ray.origin, ray.direction);
                    for child in result.iter().rev() {
                        if *child != u32::MAX {
                            stack.push(*child);
                        }
                    }
                }
                6 => {
                    let at = base + u64::from(pointer >> 3) * 64;
                    let m = |i: u64| f32::from_bits(self.word(at + 64 + 4 * i));
                    let transform = |p: [f32; 3], w: f32| {
                        [0, 1, 2].map(|r| {
                            m(4 * r) * p[0]
                                + m(4 * r + 1) * p[1]
                                + m(4 * r + 2) * p[2]
                                + m(4 * r + 3) * w
                        })
                    };
                    let local = Ray {
                        origin: transform(ray.origin, 1.0),
                        direction: transform(ray.direction, 0.0),
                        extent,
                    };
                    let blas = u64::from(self.word(at + 60)) << 8;
                    assert_eq!(self.word(at + 56) & 7, 4 | (self.word(at + 56) & 1));
                    if let Some(hit) = self.trace_blas(blas, &local) {
                        extent = hit.t;
                        best = Some((self.word(at + 48) & 0xFF_FFFF, hit));
                    }
                }
                other => panic!("unexpected node type {other}"),
            }
        }
        best
    }
}

fn testkit_exec() -> usize {
    126
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn wide(v: [f32; 3]) -> [f64; 3] {
    v.map(f64::from)
}

/// Distance along the ray to a triangle, double sided, in double precision.
pub fn triangle_distance(triangle: &Triangle, ray: &Ray) -> Option<f64> {
    let [a, b, c] = triangle.vertices.map(wide);
    let (origin, direction) = (wide(ray.origin), wide(ray.direction));
    let (e1, e2) = (sub(b, a), sub(c, a));
    let p = cross(direction, e2);
    let det = dot(e1, p);
    if det == 0.0 {
        return None;
    }
    let s = sub(origin, a);
    let u = dot(s, p) / det;
    let q = cross(s, e1);
    let v = dot(direction, q) / det;
    let t = dot(e2, q) / det;
    (u >= 0.0 && v >= 0.0 && u + v <= 1.0 && t >= 0.0 && t <= f64::from(ray.extent)).then_some(t)
}

/// The nearest triangle by brute force: its distance and the geometry and primitive indices.
pub fn brute_force(geometries: &[Geometry], ray: &Ray) -> Option<Hit> {
    let mut best: Option<(f64, u32, u32)> = None;
    for (g, geometry) in geometries.iter().enumerate() {
        for (p, triangle) in geometry.triangles.iter().enumerate() {
            if let Some(t) = triangle_distance(triangle, ray)
                && best.is_none_or(|b| t < b.0)
            {
                best = Some((t, g as u32, p as u32));
            }
        }
    }
    best.map(|(t, geometry, primitive)| Hit {
        t: t as f32,
        geometry,
        primitive,
    })
}

/// Whether two answers agree: both a miss, or the same distance and a primitive that the ray reaches there.
pub fn agrees(
    geometries: &[Geometry],
    ray: &Ray,
    found: Option<Hit>,
    expected: Option<Hit>,
) -> bool {
    match (found, expected) {
        (None, None) => true,
        (Some(f), Some(e)) => {
            let close = (f.t - e.t).abs() <= 2e-4 * e.t.abs().max(1.0);
            let own = geometries[f.geometry as usize].triangles[f.primitive as usize];
            let reached = triangle_distance(&own, ray)
                .is_some_and(|t| (t as f32 - f.t).abs() <= 2e-4 * f.t.abs().max(1.0));
            close && reached
        }
        _ => false,
    }
}

pub fn instance(transform: [f32; 12], id: u32, blas: u64) -> Instance {
    Instance {
        transform,
        id,
        mask: 0xFF,
        contribution: 0,
        flags: 0,
        blas,
    }
}

/// Every pointer of a structure checked: children lie inside the structure, each element is the child of
/// exactly one node and the parent table names that node.
pub fn check_structure(bytes: &[u8]) -> usize {
    let header = Header::read(bytes).expect("a header");
    let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let elements = header.elements as usize;
    let unit = if header.is_bottom_level() { 64 } else { 128 };
    let count = header.elements_count as usize;
    let mut seen = vec![0u32; count];
    let mut nodes = 0;
    let mut stack = vec![(header.root, u32::MAX)];
    while let Some((pointer, parent)) = stack.pop() {
        if pointer == u32::MAX {
            continue;
        }
        let at = (pointer >> 3) as usize * 64;
        assert!(at + 64 <= bytes.len(), "a pointer inside the structure");
        let entry = header.parents as usize + 4 * ((at - elements) / 64);
        assert_eq!(word(entry), parent, "parent of {pointer:#x}");
        match pointer & 7 {
            0 | 6 => {
                let index = (at - elements) / unit;
                seen[index] += 1;
            }
            4 | 5 => {
                nodes += 1;
                for slot in 0..4 {
                    stack.push((word(at + 4 * slot), pointer));
                }
            }
            other => panic!("node type {other}"),
        }
    }
    assert!(seen.iter().all(|s| *s == 1), "every element reached once");
    assert_eq!(header.size(), bytes.len());
    nodes
}
