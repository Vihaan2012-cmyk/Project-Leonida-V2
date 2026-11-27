#![cfg(feature = "gpu")]

mod common;

use bvh_build::gpu::GpuBuilder;
use bvh_build::{Cpu, blas_info, build_blas, build_tlas, flags};
use common::*;

fn builder() -> Option<GpuBuilder> {
    bvh_build::gpu::compile_kernels().unwrap();
    match GpuBuilder::new() {
        Ok(builder) => {
            eprintln!("GPU build on {}", builder.device_name());
            Some(builder)
        }
        Err(error) => {
            eprintln!("skipping: {error}");
            None
        }
    }
}

#[test]
fn the_kernels_compile() {
    bvh_build::gpu::compile_kernels().unwrap();
}

#[test]
fn gpu_and_cpu_builds_write_the_same_bytes() {
    let Some(mut gpu) = builder() else { return };
    for (count, seed) in [
        (2usize, 1u64),
        (3, 2),
        (5, 3),
        (17, 4),
        (300, 5),
        (1000, 6),
        (4097, 7),
    ] {
        let mut rng = Rng(seed);
        let geometries = vec![
            geometry(triangles(&mut rng, count / 2, 1.0)),
            geometry(triangles(&mut rng, count - count / 2, 0.5)),
        ];
        for build_flags in [0, flags::MINIMIZE_MEMORY] {
            let on_cpu = build_blas(&geometries, build_flags, &mut Cpu).unwrap();
            let on_gpu = build_blas(&geometries, build_flags, &mut gpu).unwrap();
            assert!(
                on_cpu == on_gpu,
                "{count} triangles, flags {build_flags}: bytes differ"
            );
        }
    }
}

#[test]
fn identical_and_inactive_elements_build_the_same_on_both() {
    let Some(mut gpu) = builder() else { return };
    let mut rng = Rng(8);
    let mut list = triangles(&mut rng, 40, 1.0);
    let copy = list[3];
    list.extend(std::iter::repeat_n(copy, 25));
    list[7].vertices[0] = [f32::NAN; 3];
    list[30].vertices[2] = [f32::INFINITY, 0.0, 0.0];
    let geometries = vec![geometry(list)];
    let on_cpu = build_blas(&geometries, 0, &mut Cpu).unwrap();
    let on_gpu = build_blas(&geometries, 0, &mut gpu).unwrap();
    assert!(on_cpu == on_gpu);
    check_structure(&on_gpu);
}

#[test]
fn a_gpu_tlas_matches_the_cpu_one() {
    let Some(mut gpu) = builder() else { return };
    let mut rng = Rng(9);
    let geometries = vec![geometry(triangles(&mut rng, 80, 1.0))];
    let blas = build_blas(&geometries, 0, &mut Cpu).unwrap();
    let instances: Vec<_> = (0..120)
        .map(|i| {
            let t = rng.point(-30.0, 30.0);
            let s = rng.range(0.5, 2.0);
            instance(
                [s, 0.0, 0.0, t[0], 0.0, s, 0.0, t[1], 0.0, 0.0, s, t[2]],
                i,
                0x1000,
            )
        })
        .collect();
    let info = |address: u64| (address == 0x1000).then(|| blas_info(&blas).unwrap());
    let on_cpu = build_tlas(&instances, &info, &mut Cpu).unwrap();
    let on_gpu = build_tlas(&instances, &info, &mut gpu).unwrap();
    assert!(on_cpu == on_gpu);
    check_structure(&on_gpu);
}

#[test]
fn a_gpu_blas_is_traced_correctly() {
    let Some(mut gpu) = builder() else { return };
    let mut rng = Rng(10);
    let geometries = vec![geometry(triangles(&mut rng, 2000, 1.0))];
    let bytes = build_blas(&geometries, flags::MINIMIZE_MEMORY, &mut gpu).unwrap();
    let mut scene = Scene::new(bytes.len());
    let base = scene.place(&bytes);
    let mut wrong = 0;
    for ray in rays(&mut rng, 100) {
        let found = scene.trace_blas(base, &ray);
        wrong += usize::from(!agrees(
            &geometries,
            &ray,
            found,
            brute_force(&geometries, &ray),
        ));
    }
    assert!(wrong <= 1, "{wrong} wrong");
}

#[test]
fn a_large_build_agrees_between_cpu_and_gpu() {
    let Some(mut gpu) = builder() else { return };
    let mut rng = Rng(14);
    let geometries = vec![geometry(triangles(&mut rng, 40_000, 0.4))];
    let on_cpu = build_blas(&geometries, flags::MINIMIZE_MEMORY, &mut Cpu).unwrap();
    let on_gpu = build_blas(&geometries, flags::MINIMIZE_MEMORY, &mut gpu).unwrap();
    assert!(on_cpu == on_gpu);
    check_structure(&on_gpu);
}
