mod common;

use common::noise;
use gpu_passes::swizzle::{Bit, Equation, Job, Swizzle, detile, tile};

const ALL: [Swizzle; 3] = [Swizzle::Sw64kbD, Swizzle::Sw4kbS, Swizzle::Sw4kbS3d];

#[test]
fn equations_use_every_coordinate_bit_once_and_fill_the_tile() {
    for swizzle in ALL {
        for log in 0..5 {
            let eq = Equation::new(swizzle, 1 << log).unwrap();
            assert_eq!(eq.bits.len() as u32, swizzle.tile_bits());
            let bytes: Vec<_> = eq.bits.iter().take_while(|b| **b == Bit::Byte).collect();
            assert_eq!(bytes.len(), log, "{swizzle:?} {log}");
            assert!(!eq.bits[log..].contains(&Bit::Byte));
            for (axis, count) in eq.tile_log2.iter().enumerate() {
                for n in 0..*count as u8 {
                    let want = match axis {
                        0 => Bit::X(n),
                        1 => Bit::Y(n),
                        _ => Bit::Z(n),
                    };
                    assert_eq!(eq.bits.iter().filter(|b| **b == want).count(), 1);
                }
            }
            let [w, h, d] = eq.tile_extent();
            assert_eq!((w * h * d) << log, swizzle.tile_bytes());
        }
    }
}

#[test]
fn tile_extents_match_the_measured_block_shapes() {
    for log in 0..5u32 {
        let eq = Equation::new(Swizzle::Sw4kbS, 1 << log).unwrap();
        assert_eq!(
            eq.tile_extent()[..2],
            [1 << (6 - log / 2), 1 << (6 - log.div_ceil(2))]
        );
        let eq = Equation::new(Swizzle::Sw64kbD, 1 << log).unwrap();
        assert_eq!(
            eq.tile_extent()[..2],
            [1 << (8 - log / 2), 1 << (8 - log.div_ceil(2))]
        );
    }
    let volume = [[16, 16, 16], [8, 16, 16], [8, 16, 8], [8, 8, 8], [4, 8, 8]];
    for (log, want) in volume.iter().enumerate() {
        let eq = Equation::new(Swizzle::Sw4kbS3d, 1 << log).unwrap();
        assert_eq!(eq.tile_extent(), *want);
    }
}

fn job(swizzle: Swizzle, eb: u32, extent: [u32; 3]) -> Job {
    let eq = Equation::new(swizzle, eb).unwrap();
    let [tw, th, _] = eq.tile_extent();
    Job {
        swizzle,
        elem_bytes: eb,
        extent,
        origin: [0, 0],
        tiles: [extent[0].div_ceil(tw), extent[1].div_ceil(th)],
        tiled_offset: 0,
        linear_offset: 0,
    }
}

#[test]
fn pinned_addresses() {
    let eq = Equation::new(Swizzle::Sw4kbS, 4).unwrap();
    let j = job(Swizzle::Sw4kbS, 4, [64, 64, 1]);
    assert_eq!(j.element_offset(&eq, 1, 0, 0), 4);
    assert_eq!(j.element_offset(&eq, 0, 1, 0), 16);
    assert_eq!(j.element_offset(&eq, 4, 0, 0), 128);
    // RDNA2 SW_64KB_D at 4 bytes puts x bit 2 before y bit 2 (Navi 1x has them the other way round).
    let eq = Equation::new(Swizzle::Sw64kbD, 4).unwrap();
    let j = job(Swizzle::Sw64kbD, 4, [256, 256, 1]);
    assert_eq!(j.element_offset(&eq, 4, 0, 0), 64);
    assert_eq!(j.element_offset(&eq, 0, 4, 0), 128);
    // Next tile along x and along y.
    assert_eq!(j.element_offset(&eq, 128, 0, 0), 65536);
    assert_eq!(j.element_offset(&eq, 0, 128, 0), 2 * 65536);
}

#[test]
fn cpu_tile_then_detile_is_the_identity() {
    for swizzle in ALL {
        for log in 0..5 {
            let eb = 1u32 << log;
            let extent = if swizzle == Swizzle::Sw4kbS3d {
                [21, 13, 11]
            } else {
                [45, 37, 1]
            };
            let mut j = job(swizzle, eb, extent);
            j.origin = if swizzle == Swizzle::Sw4kbS3d {
                [0, 0]
            } else {
                [3, 5]
            };
            j.tiles = {
                let [tw, th, _] = j.equation().unwrap().tile_extent();
                [
                    (j.origin[0] + extent[0]).div_ceil(tw),
                    (j.origin[1] + extent[1]).div_ceil(th),
                ]
            };
            let linear = noise(j.linear_bytes() as usize, 7 + log as u64);
            let mut tiled = noise(j.tiled_bytes().unwrap() as usize, 99);
            tile(&j, &linear, &mut tiled).unwrap();
            let mut back = vec![0u8; linear.len()];
            detile(&j, &tiled, &mut back).unwrap();
            assert_eq!(back, linear, "{swizzle:?} eb {eb}");
        }
    }
}
