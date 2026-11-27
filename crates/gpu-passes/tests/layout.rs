mod common;

use common::noise;
use gpu_passes::layout::{Format, Kind, SurfaceDesc};
use gpu_passes::swizzle::{Swizzle, detile, tile};

#[test]
fn layouts_match_the_documented_allocations() {
    // 512x256 BC1, 7 mips, 4 KiB tiles: mips 0-2 at 24576, 8192, 4096; mips 3-6 share the tile at 0.
    let desc = SurfaceDesc {
        kind: Kind::Texture2d,
        swizzle: Swizzle::Sw4kbS,
        format: Format::block(8),
        width: 512,
        height: 256,
        mips: 7,
    };
    let layout = desc.layout().unwrap();
    let at = |mip: usize| layout.subresources[mip].job;
    assert_eq!(
        [at(0).tiled_offset, at(1).tiled_offset, at(2).tiled_offset],
        [24576, 8192, 4096]
    );
    assert_eq!(at(3).origin, [16, 0]);
    assert_eq!(at(4).origin, [8, 8]);
    assert_eq!(at(5).origin, [0, 12]);
    assert_eq!(at(6).origin, [0, 8]);
    assert!((3..7).all(|m| at(m).tiled_offset == 0 && at(m).tiles == [1, 1]));
    assert_eq!(layout.tiled_size, 24576 + 65536);

    // A 512x512 BC7 face with 10 mips is 352,256 bytes; six of them are a cube.
    let face = SurfaceDesc {
        kind: Kind::Texture2d,
        swizzle: Swizzle::Sw4kbS,
        format: Format::block(16),
        width: 512,
        height: 512,
        mips: 10,
    };
    assert_eq!(face.layout().unwrap().tiled_size, 352_256);
    let cube = SurfaceDesc {
        kind: Kind::Cube { data_offset: 0 },
        ..face
    };
    let layout = cube.layout().unwrap();
    assert_eq!(layout.tiled_size, 6 * 352_256);
    assert_eq!(layout.subresources.len(), 60);

    // Volumes: allocation spans measured in the corpus.
    let volume = |w, h, d, eb| SurfaceDesc {
        kind: Kind::Volume { depth: d },
        swizzle: Swizzle::Sw4kbS3d,
        format: Format::pixel(eb),
        width: w,
        height: h,
        mips: 1,
    };
    assert_eq!(volume(256, 32, 8, 2).layout().unwrap().tiled_size, 262_144);
    assert_eq!(volume(32, 32, 32, 8).layout().unwrap().tiled_size, 262_144);
    assert_eq!(volume(32, 64, 32, 8).layout().unwrap().tiled_size, 524_288);
}

#[test]
fn array_layout_is_mip_major_behind_the_tail_slab() {
    let desc = SurfaceDesc {
        kind: Kind::Array { layers: 3 },
        swizzle: Swizzle::Sw64kbD,
        format: Format::block(16),
        width: 256,
        height: 256,
        mips: 9,
    };
    let layout = desc.layout().unwrap();
    let find = |mip: u32, slice: u32| {
        layout
            .subresources
            .iter()
            .find(|s| s.mip == mip && s.slice == slice)
            .unwrap()
            .job
    };
    let tail = find(8, 2);
    assert_eq!(tail.tiled_offset, 2 * 65536);
    let first_standalone = (0..9)
        .filter(|m| find(*m, 0).tiled_offset >= 3 * 65536)
        .max()
        .unwrap();
    assert_eq!(find(first_standalone, 0).tiled_offset, 3 * 65536);
    let m0 = find(0, 0);
    assert_eq!(
        find(0, 1).tiled_offset - m0.tiled_offset,
        m0.tiled_bytes().unwrap()
    );
    assert_eq!(
        layout.tiled_size,
        find(0, 2).tiled_offset + m0.tiled_bytes().unwrap()
    );
}

#[test]
fn every_layout_round_trips_on_the_cpu() {
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
            Swizzle::Sw4kbS,
            Format::block(8),
            130,
            70,
            8,
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
        let linear = noise(layout.linear_size as usize, 5);
        let mut tiled = noise(layout.tiled_size as usize, 6);
        let mut back = vec![0u8; linear.len()];
        for s in &layout.subresources {
            tile(&s.job, &linear, &mut tiled).unwrap();
        }
        for s in &layout.subresources {
            detile(&s.job, &tiled, &mut back).unwrap();
        }
        for s in &layout.subresources {
            let range =
                s.job.linear_offset as usize..(s.job.linear_offset + s.job.linear_bytes()) as usize;
            assert_eq!(
                back[range.clone()],
                linear[range],
                "{kind:?} mip {} slice {}",
                s.mip,
                s.slice
            );
        }
    }
}

#[test]
fn descriptor_modes_pick_the_measured_swizzle() {
    for (mode, want) in [
        (0, Swizzle::Sw64kbD),
        (1, Swizzle::Sw64kbD),
        (32, Swizzle::Sw64kbD),
    ] {
        assert_eq!(Swizzle::from_descriptor_mode(mode, false).unwrap(), want);
    }
    for mode in [2, 4, 5, 6, 7, 36, 38, 18, 20, 22, 24] {
        assert_eq!(
            Swizzle::from_descriptor_mode(mode, false).unwrap(),
            Swizzle::Sw4kbS,
            "{mode}"
        );
    }
    assert_eq!(
        Swizzle::from_descriptor_mode(2, true).unwrap(),
        Swizzle::Sw4kbS3d
    );
    assert!(Swizzle::from_descriptor_mode(8, false).is_err());
    let desc = SurfaceDesc::from_ttpf(20, 2, 6, 128, 128, 8, Format::block(8)).unwrap();
    assert_eq!(desc.kind, Kind::Cube { data_offset: 0 });
    assert!(SurfaceDesc::from_ttpf(0, 2, 5, 128, 128, 8, Format::block(8)).is_err());
}
