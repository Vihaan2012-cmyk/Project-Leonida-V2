use gpu::Gpu;
use shader_translate::{ComputeOptions, FloatMode, PUSH_HEAP, PUSH_WORDS};
use testkit::{Memory, Wave};

use crate::common::spirv_val;

pub const SORTED: u32 = 0x8000_0000;
pub const UNSORTED: u32 = 0;

/// One intersection step: a node pointer and the ray the instruction receives. The inverse direction is its own
/// field because shaders compute it and tests set it independently of the direction.
#[derive(Clone, Copy, Debug)]
pub struct Query {
    pub pointer: u32,
    pub extent: f32,
    pub origin: [f32; 3],
    pub direction: [f32; 3],
    pub inverse: [f32; 3],
}

impl Query {
    pub fn new(pointer: u32, extent: f32, origin: [f32; 3], direction: [f32; 3]) -> Query {
        Query {
            pointer,
            extent,
            origin,
            direction,
            inverse: direction.map(|d| 1.0 / d),
        }
    }

    fn words(&self) -> [u32; 12] {
        let mut words = [
            self.pointer,
            0,
            self.extent.to_bits(),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ];
        for i in 0..3 {
            words[3 + i] = self.origin[i].to_bits();
            words[6 + i] = self.direction[i].to_bits();
            words[9 + i] = self.inverse[i].to_bits();
        }
        words
    }
}

/// Memory shared by the reference and the GPU: structures at 256-byte aligned bases.
#[derive(Default, Clone)]
pub struct Heap {
    pub bytes: Vec<u8>,
}

impl Heap {
    pub fn place(&mut self, data: &[u8]) -> u64 {
        let base = self.bytes.len().div_ceil(256) * 256;
        self.bytes.resize(base, 0);
        self.bytes.extend_from_slice(data);
        base as u64
    }
}

pub struct Mismatch {
    pub index: usize,
    pub query: Query,
    pub gpu: [u32; 4],
    pub cpu: [u32; 4],
}

#[derive(Default)]
pub struct Report {
    pub queries: usize,
    pub mismatches: Vec<Mismatch>,
    pub inexact: usize,
    pub denormal: usize,
    pub hits: usize,
    pub results: Vec<[u32; 4]>,
}

impl Report {
    pub fn assert_clean(&self, label: &str) {
        eprintln!(
            "{label}: {} queries, {} with a result, {} equal up to NaN payload ({} denormal words), {} different",
            self.queries,
            self.hits,
            self.inexact,
            self.denormal,
            self.mismatches.len()
        );
        for m in self.mismatches.iter().take(8) {
            eprintln!(
                "  #{} {:#x} extent {:?} origin {:?} direction {:?} inverse {:?}\n    gpu {:x?}\n    cpu {:x?}",
                m.index,
                m.query.pointer,
                m.query.extent,
                m.query.origin,
                m.query.direction,
                m.query.inverse,
                m.gpu,
                m.cpu
            );
        }
        assert!(
            self.mismatches.is_empty(),
            "{label}: GPU differs from the reference"
        );
    }
}

fn nan(x: u32) -> bool {
    x & 0x7F80_0000 == 0x7F80_0000 && x & 0x007F_FFFF != 0
}

fn tiny(x: u32) -> bool {
    x & 0x7F80_0000 == 0
}

fn equivalent(a: u32, b: u32) -> bool {
    a == b || (nan(a) && nan(b)) || (tiny(a) && tiny(b))
}

pub fn program(wide: bool) -> String {
    let (name, addresses) = if wide {
        ("image_bvh64_intersect_ray", "v[1:12]")
    } else {
        (
            "image_bvh_intersect_ray",
            "[v1,v3,v4,v5,v6,v7,v8,v9,v10,v11,v12]",
        )
    };
    format!(
        "s_lshl_b32 s9, s8, 5
         v_add_nc_u32_e32 v20, s9, v0
         v_mul_u32_u24_e32 v13, 48, v20
         global_load_dwordx4 v[1:4], v13, s[4:5]
         global_load_dwordx4 v[5:8], v13, s[4:5] offset:16
         global_load_dwordx4 v[9:12], v13, s[4:5] offset:32
         s_waitcnt vmcnt(0)
         {name} v[14:17], {addresses}, s[0:3]
         v_lshlrev_b32_e32 v21, 4, v20
         global_store_dwordx4 v21, v[14:17], s[6:7]
         s_endpgm"
    )
}

pub struct Runner {
    pub gpu: Gpu,
    spirv: [Vec<u32>; 2],
}

pub fn float_mode(gpu: &Gpu) -> FloatMode {
    FloatMode {
        nan_preserve: gpu.info.nan_preserve_32,
        flush_denormals: gpu.info.denorm_flush_32 && !gpu.info.denorm_preserve_32,
        preserve_denormals: gpu.info.denorm_preserve_32,
    }
}

pub fn translate(gpu: &Gpu, source: &str, name: &str) -> Vec<u32> {
    let code = rdna::assemble(source, true).unwrap_or_else(|e| panic!("{e:?}\n{source}"));
    let function = shader_ir::lift(&code, false).unwrap_or_else(|e| panic!("{e}\n{source}"));
    let rsrc2 = (8 << 1) | (1 << 7);
    let options = ComputeOptions::from_rsrc2(rsrc2, [32, 1, 1], float_mode(gpu));
    let spirv =
        shader_translate::compute(&function, &options).unwrap_or_else(|e| panic!("{e}\n{source}"));
    spirv_val(&spirv, name);
    spirv
}

impl Runner {
    pub fn new(gpu: Gpu) -> Runner {
        eprintln!(
            "{} preserve {} flush {} nan {}",
            gpu.info.name,
            gpu.info.denorm_preserve_32,
            gpu.info.denorm_flush_32,
            gpu.info.nan_preserve_32
        );
        let spirv = [false, true].map(|wide| translate(&gpu, &program(wide), "intersect"));
        Runner { gpu, spirv }
    }

    pub fn preserves_denormals(&self) -> bool {
        self.gpu.info.denorm_preserve_32
    }

    /// Runs every query through the reference interpreter and the translated shader, with the descriptor's
    /// base at `base` and the second word `high`, and compares the four result words.
    pub fn compare(
        &self,
        heap: &Heap,
        base: u64,
        high: u32,
        wide: bool,
        queries: &[Query],
    ) -> Report {
        let groups = queries.len().div_ceil(32);
        let rays_at = heap.bytes.len().div_ceil(256) * 256;
        let out_at = rays_at + groups * 32 * 48;
        let total = out_at + groups * 32 * 16 + 4096;
        let mut memory = Memory::new(total);
        memory.bytes[..heap.bytes.len()].copy_from_slice(&heap.bytes);
        let idle = Query::new(u32::MAX, 0.0, [0.0; 3], [1.0; 3]);
        for i in 0..groups * 32 {
            let query = queries.get(i).unwrap_or(&idle);
            for (k, word) in query.words().iter().enumerate() {
                memory
                    .write((rays_at + 48 * i + 4 * k) as u64, *word)
                    .unwrap();
            }
        }
        let desc = [
            (base >> 8) as u32,
            high | (base >> 40) as u32 & 0xFF,
            u32::MAX,
            0x8900_03FF,
        ];

        let mut gpu_words: Vec<u32> = memory
            .bytes
            .chunks(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        let mut reference = memory;
        let code = rdna::assemble(&program(wide), true).unwrap();
        for group in 0..groups {
            let mut wave = Wave::new(false);
            wave.flush_denormals = !self.gpu.info.denorm_preserve_32;
            wave.canonical_nan = (self.gpu.info.vendor == 0x10DE).then_some(0x7FFF_FFFF);
            wave.sgpr[..4].copy_from_slice(&desc);
            wave.sgpr[4] = rays_at as u32;
            wave.sgpr[6] = out_at as u32;
            wave.sgpr[8] = group as u32;
            for lane in 0..32 {
                wave.vgpr[0][lane] = lane as u32;
            }
            testkit::run(&code, &mut wave, &mut reference).expect("the reference runs the program");
        }

        let spirv = &self.spirv[usize::from(wide)];
        self.gpu
            .dispatch_with(
                spirv,
                &mut [&mut gpu_words],
                |addresses| {
                    let mut push = vec![0u32; PUSH_WORDS as usize];
                    push[..4].copy_from_slice(&desc);
                    push[4] = rays_at as u32;
                    push[6] = out_at as u32;
                    push[PUSH_HEAP as usize] = addresses[0] as u32;
                    push[PUSH_HEAP as usize + 1] = (addresses[0] >> 32) as u32;
                    push
                },
                [groups as u32, 1, 1],
                Some(32),
            )
            .unwrap();
        assert_eq!(gpu::validation_errors(), 0);

        let mut report = Report {
            queries: queries.len(),
            ..Report::default()
        };
        for (i, query) in queries.iter().enumerate() {
            let at = out_at / 4 + 4 * i;
            let got: [u32; 4] = std::array::from_fn(|k| gpu_words[at + k]);
            let want: [u32; 4] = std::array::from_fn(|k| reference.word(at + k));
            if got.iter().zip(&want).all(|(a, b)| a == b) {
            } else if got.iter().zip(&want).all(|(a, b)| equivalent(*a, *b)) {
                report.inexact += 1;
                report.denormal += got
                    .iter()
                    .zip(&want)
                    .filter(|(a, b)| a != b && tiny(**a))
                    .count();
            } else {
                report.mismatches.push(Mismatch {
                    index: i,
                    query: *query,
                    gpu: got,
                    cpu: want,
                });
            }
            report.hits += usize::from(want != [u32::MAX; 4]);
            report.results.push(got);
        }
        report
    }
}
