use std::time::Instant;

use shader_translate::{PUSH_HEAP, PUSH_WORDS};

use crate::harness::{Heap, Query, SORTED, translate};
use crate::scene::{Rng, blas, node_queries, pointers};

/// A shader that intersects `repeat` times with an origin that moves each time, and folds the results.
fn looping_program(wide: bool, repeat: u32) -> String {
    let (name, addresses) = if wide {
        ("image_bvh64_intersect_ray", "v[1:12]")
    } else {
        (
            "image_bvh_intersect_ray",
            "[v1,v3,v4,v5,v6,v7,v8,v9,v10,v11,v12]",
        )
    };
    let body = format!(
        "{name} v[14:17], {addresses}, s[0:3]
         v_xor_b32_e32 v18, v18, v14
         v_add_nc_u32_e32 v19, v19, v15
         v_xor_b32_e32 v22, v22, v16
         v_add_nc_u32_e32 v23, v23, v17
         v_add_f32_e32 v4, 0x3a83126f, v4
         s_add_u32 s10, s10, 1
         s_cmp_lt_u32 s10, {repeat}"
    );
    let words = rdna::assemble(&body, true).unwrap().len() / 4;
    format!(
        "s_lshl_b32 s9, s8, 5
         v_add_nc_u32_e32 v20, s9, v0
         v_mul_u32_u24_e32 v13, 48, v20
         global_load_dwordx4 v[1:4], v13, s[4:5]
         global_load_dwordx4 v[5:8], v13, s[4:5] offset:16
         global_load_dwordx4 v[9:12], v13, s[4:5] offset:32
         s_waitcnt vmcnt(0)
         s_mov_b32 s10, 0
         v_mov_b32_e32 v18, 0
         v_mov_b32_e32 v19, 0
         v_mov_b32_e32 v22, 0
         v_mov_b32_e32 v23, 0
         {body}
         s_cbranch_scc1 -{back}
         v_lshlrev_b32_e32 v21, 4, v20
         v_add_nc_u32_e32 v18, v18, v19
         v_add_nc_u32_e32 v22, v22, v23
         global_store_dwordx2 v21, v[18:19], s[6:7]
         s_endpgm",
        back = words + 1
    )
}

fn seconds(
    gpu: &gpu::Gpu,
    spirv: &[u32],
    heap: &mut Vec<u32>,
    rays_at: u32,
    out_at: u32,
    groups: u32,
) -> f64 {
    let desc = [0, SORTED, u32::MAX, 0x8900_03FF];
    let mut best = f64::MAX;
    for _ in 0..6 {
        let start = Instant::now();
        gpu.dispatch_with(
            spirv,
            &mut [heap.as_mut_slice()],
            |addresses| {
                let mut push = vec![0u32; PUSH_WORDS as usize];
                push[..4].copy_from_slice(&desc);
                push[4] = rays_at;
                push[6] = out_at;
                push[PUSH_HEAP as usize] = addresses[0] as u32;
                push[PUSH_HEAP as usize + 1] = (addresses[0] >> 32) as u32;
                push
            },
            [groups, 1, 1],
            Some(32),
        )
        .unwrap();
        best = best.min(start.elapsed().as_secs_f64());
    }
    best
}

/// GPU time of one intersection by the difference of two loop lengths. Run with `--ignored --nocapture`.
#[test]
#[ignore]
fn intersect_cost_per_call() {
    let Some(gpu) = crate::common::gpu() else {
        return;
    };
    let mut rng = Rng(77);
    for box16 in [false, true] {
        let bytes = blas(&mut rng, 3000, box16);
        let all = pointers(&bytes);
        for (label, kinds) in [("box", [4u32, 5]), ("triangle", [0, 0])] {
            let queries: Vec<Query> = node_queries(&mut rng, &bytes, 8, false)
                .into_iter()
                .filter(|q| q.pointer & 7 == kinds[0] || q.pointer & 7 == kinds[1])
                .collect();
            assert!(!queries.is_empty() && !all.is_empty());
            let groups = 2048usize;
            let mut heap = Heap::default();
            heap.place(&bytes);
            let rays_at = heap.bytes.len().div_ceil(256) * 256;
            let out_at = rays_at + groups * 32 * 48;
            heap.bytes.resize(out_at + groups * 32 * 16 + 4096, 0);
            for i in 0..groups * 32 {
                let q = queries[i % queries.len()];
                let words = [
                    q.pointer,
                    0,
                    q.extent.to_bits(),
                    q.origin[0].to_bits(),
                    q.origin[1].to_bits(),
                    q.origin[2].to_bits(),
                    q.direction[0].to_bits(),
                    q.direction[1].to_bits(),
                    q.direction[2].to_bits(),
                    q.inverse[0].to_bits(),
                    q.inverse[1].to_bits(),
                    q.inverse[2].to_bits(),
                ];
                for (k, w) in words.iter().enumerate() {
                    let at = rays_at + 48 * i + 4 * k;
                    heap.bytes[at..at + 4].copy_from_slice(&w.to_le_bytes());
                }
            }
            let mut words: Vec<u32> = heap
                .bytes
                .chunks(4)
                .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
                .collect();
            let (short, long) = (1u32, 2049u32);
            let lanes = (groups * 32) as f64;
            for wide in [false, true] {
                let spirv_short = translate(&gpu, &looping_program(wide, short), "bench");
                let spirv_long = translate(&gpu, &looping_program(wide, long), "bench");
                let a = seconds(
                    &gpu,
                    &spirv_short,
                    &mut words,
                    rays_at as u32,
                    out_at as u32,
                    groups as u32,
                );
                let b = seconds(
                    &gpu,
                    &spirv_long,
                    &mut words,
                    rays_at as u32,
                    out_at as u32,
                    groups as u32,
                );
                let sum: u32 = words[out_at / 4..out_at / 4 + 2 * groups * 32]
                    .iter()
                    .fold(0, |a, b| a ^ b);
                eprintln!("checksum {sum:#x}");
                let per_call = (b - a) / f64::from(long - short) / lanes;
                eprintln!(
                    "box16 {box16} {label} wide {wide}: {:.2} ns per lane-call ({:.1} Mcalls/s), loop {short}: {:.2} ms, {long}: {:.2} ms",
                    per_call * 1e9,
                    1e-6 / per_call,
                    a * 1e3,
                    b * 1e3,
                );
            }
        }
    }
}
