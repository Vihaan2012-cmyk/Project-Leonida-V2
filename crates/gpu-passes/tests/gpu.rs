#![allow(unsafe_code)]
mod common;

use common::{Ctx, noise};
use gpu_passes::layout::{Format, Kind, SurfaceDesc};
use gpu_passes::swizzle::{Equation, Job, Swizzle, detile, tile};
use gpu_passes::{Barrier, Passes};

fn context() -> Option<Ctx> {
    let ctx = Ctx::new();
    if ctx.is_none() {
        eprintln!("skipping: no Vulkan GPU");
    }
    ctx
}

fn gpu_detile(
    ctx: &Ctx,
    passes: &Passes,
    tiled: &[u8],
    linear_len: usize,
    jobs: &[Job],
) -> Vec<u8> {
    let tiled = ctx.buffer(tiled);
    let linear = ctx.buffer(&vec![0u8; linear_len]);
    ctx.run(|cmd| {
        // SAFETY: `cmd` is recording on the context's device and both buffers outlive the submission.
        unsafe {
            passes
                .record_detile(cmd, tiled.buffer, linear.buffer, jobs, Barrier::HOST_READ)
                .unwrap()
        }
    });
    let out = ctx.read(&linear, linear_len);
    ctx.free(tiled);
    ctx.free(linear);
    out
}

fn gpu_tile(ctx: &Ctx, passes: &Passes, linear: &[u8], tiled: &[u8], jobs: &[Job]) -> Vec<u8> {
    let tiled_buf = ctx.buffer(tiled);
    let linear_buf = ctx.buffer(linear);
    ctx.run(|cmd| {
        // SAFETY: as in `gpu_detile`.
        unsafe {
            passes
                .record_tile(
                    cmd,
                    tiled_buf.buffer,
                    linear_buf.buffer,
                    jobs,
                    Barrier::HOST_READ,
                )
                .unwrap()
        }
    });
    let out = ctx.read(&tiled_buf, tiled.len());
    ctx.free(tiled_buf);
    ctx.free(linear_buf);
    out
}

fn check_jobs(
    ctx: &Ctx,
    passes: &Passes,
    jobs: &[Job],
    tiled_len: usize,
    linear_len: usize,
    label: &str,
) {
    let tiled = noise(tiled_len, 11);
    let mut want = vec![0u8; linear_len];
    for job in jobs {
        detile(job, &tiled, &mut want).unwrap();
    }
    let got = gpu_detile(ctx, passes, &tiled, linear_len, jobs);
    assert!(got == want, "detile differs: {label}");

    let linear = noise(linear_len, 12);
    let before = noise(tiled_len, 13);
    let mut want_tiled = before.clone();
    for job in jobs {
        tile(job, &linear, &mut want_tiled).unwrap();
    }
    let got_tiled = gpu_tile(ctx, passes, &linear, &before, jobs);
    assert!(got_tiled == want_tiled, "tile differs: {label}");

    let back = gpu_detile(ctx, passes, &got_tiled, linear_len, jobs);
    for job in jobs {
        let range = job.linear_offset as usize..(job.linear_offset + job.linear_bytes()) as usize;
        assert!(
            back[range.clone()] == linear[range],
            "round trip differs: {label}"
        );
    }
    passes.reset().unwrap();
}

fn single(swizzle: Swizzle, eb: u32, extent: [u32; 3], origin: [u32; 2]) -> Job {
    let eq = Equation::new(swizzle, eb).unwrap();
    let [tw, th, _] = eq.tile_extent();
    Job {
        swizzle,
        elem_bytes: eb,
        extent,
        origin,
        tiles: [
            (origin[0] + extent[0]).div_ceil(tw),
            (origin[1] + extent[1]).div_ceil(th),
        ],
        tiled_offset: 0,
        linear_offset: 0,
    }
}

#[test]
fn every_swizzle_and_element_size_matches_the_cpu() {
    let Some(ctx) = context() else { return };
    let passes = Passes::new(&ctx.device, 64).unwrap();
    for swizzle in [Swizzle::Sw64kbD, Swizzle::Sw4kbS, Swizzle::Sw4kbS3d] {
        for log in 0..5 {
            let eb = 1u32 << log;
            let volume = swizzle == Swizzle::Sw4kbS3d;
            let extents: &[([u32; 3], [u32; 2])] = if volume {
                &[
                    ([21, 13, 11], [0, 0]),
                    ([1, 1, 1], [0, 0]),
                    ([3, 5, 2], [0, 0]),
                ]
            } else {
                &[
                    ([45, 37, 1], [0, 0]),
                    ([1, 1, 1], [0, 0]),
                    ([7, 3, 1], [5, 9]),
                    ([130, 3, 1], [0, 0]),
                ]
            };
            for (extent, origin) in extents {
                let mut job = single(swizzle, eb, *extent, *origin);
                job.tiled_offset = 256;
                job.linear_offset = 64;
                let tiled_len = 256 + job.tiled_bytes().unwrap() as usize + 64;
                let linear_len = 64 + job.linear_bytes() as usize + 12;
                check_jobs(
                    &ctx,
                    &passes,
                    &[job],
                    tiled_len,
                    linear_len,
                    &format!("{swizzle:?} eb {eb} {extent:?}"),
                );
            }
        }
    }
}

#[test]
fn whole_surfaces_match_the_cpu() {
    let Some(ctx) = context() else { return };
    let passes = Passes::new(&ctx.device, 64).unwrap();
    let descs = [
        (
            Kind::Texture2d,
            Swizzle::Sw64kbD,
            Format::pixel(4),
            300,
            170,
            9,
        ),
        (
            Kind::Texture2d,
            Swizzle::Sw64kbD,
            Format::pixel(1),
            130,
            66,
            8,
        ),
        (
            Kind::Texture2d,
            Swizzle::Sw64kbD,
            Format::pixel(2),
            513,
            33,
            6,
        ),
        (
            Kind::Texture2d,
            Swizzle::Sw4kbS,
            Format::block(8),
            512,
            256,
            7,
        ),
        (
            Kind::Texture2d,
            Swizzle::Sw4kbS,
            Format::block(16),
            130,
            70,
            8,
        ),
        (
            Kind::Texture2d,
            Swizzle::Sw4kbS,
            Format::pixel(8),
            100,
            60,
            1,
        ),
        (
            Kind::Array { layers: 3 },
            Swizzle::Sw64kbD,
            Format::block(16),
            128,
            64,
            8,
        ),
        (
            Kind::Array { layers: 2 },
            Swizzle::Sw64kbD,
            Format::pixel(4),
            64,
            64,
            7,
        ),
        (
            Kind::Cube { data_offset: 8192 },
            Swizzle::Sw4kbS,
            Format::block(8),
            64,
            64,
            7,
        ),
        (
            Kind::Volume { depth: 9 },
            Swizzle::Sw4kbS3d,
            Format::pixel(2),
            20,
            18,
            1,
        ),
        (
            Kind::Volume { depth: 8 },
            Swizzle::Sw4kbS3d,
            Format::pixel(8),
            16,
            16,
            1,
        ),
        (
            Kind::Volume { depth: 5 },
            Swizzle::Sw4kbS3d,
            Format::pixel(4),
            9,
            17,
            1,
        ),
    ];
    for (kind, swizzle, format, width, height, mips) in descs {
        let layout = SurfaceDesc {
            kind,
            swizzle,
            format,
            width,
            height,
            mips,
        }
        .layout()
        .unwrap();
        let jobs: Vec<Job> = layout.subresources.iter().map(|s| s.job).collect();
        let label = format!("{kind:?} {swizzle:?} {format:?} {width}x{height} mips {mips}");
        check_jobs(
            &ctx,
            &passes,
            &jobs,
            layout.tiled_size as usize,
            layout.linear_size as usize,
            &label,
        );
        passes.reset().unwrap();
    }
}

#[test]
fn a_surface_larger_than_one_grid_row_of_groups_works() {
    let Some(ctx) = context() else { return };
    let passes = Passes::new(&ctx.device, 4).unwrap();
    let job = single(Swizzle::Sw64kbD, 4, [2048, 1100, 1], [0, 0]);
    assert!(job.linear_bytes() / 4 > 32768 * 64);
    let tiled_len = job.tiled_bytes().unwrap() as usize;
    let linear_len = job.linear_bytes() as usize;
    let tiled = noise(tiled_len, 3);
    let mut want = vec![0u8; linear_len];
    detile(&job, &tiled, &mut want).unwrap();
    assert!(gpu_detile(&ctx, &passes, &tiled, linear_len, &[job]) == want);
}

#[test]
fn the_descriptor_pool_runs_out_and_resets() {
    let Some(ctx) = context() else { return };
    let passes = Passes::new(&ctx.device, 1).unwrap();
    let job = single(Swizzle::Sw4kbS, 4, [16, 16, 1], [0, 0]);
    let tiled = ctx.buffer(&noise(4096, 1));
    let linear = ctx.buffer(&[0u8; 1024]);
    ctx.run(|cmd| {
        // SAFETY: `cmd` is recording and the buffers outlive the submission.
        unsafe {
            passes
                .record_detile(cmd, tiled.buffer, linear.buffer, &[job], Barrier::HOST_READ)
                .unwrap();
            let second =
                passes.record_detile(cmd, tiled.buffer, linear.buffer, &[job], Barrier::HOST_READ);
            assert!(matches!(second, Err(gpu_passes::PassError::PoolExhausted)));
        }
    });
    passes.reset().unwrap();
    ctx.free(tiled);
    ctx.free(linear);
}
