use super::layout::*;
use super::tiling::TileMode;

#[test]
fn one_mip_is_its_padded_surface() {
    let layout = compact_layout(TileMode::Sw64kD, 100, 100, 1, 4, 1).unwrap();
    assert_eq!(layout.size, 65536);
    assert_eq!(layout.first_tail, 1);
    assert_eq!(layout.mips[0].padded_w, 128);
    assert!(!layout.mips[0].in_tail);
}

#[test]
fn small_mips_share_the_first_tile() {
    let layout = compact_layout(TileMode::Sw64kD, 256, 256, 9, 4, 1).unwrap();
    assert_eq!(layout.first_tail, 2);
    assert_eq!(layout.size, 65536 + 65536 + 262144);
    assert_eq!(layout.mips[1].offset, 65536);
    assert_eq!(layout.mips[0].offset, 131072);
    let origin = |i: usize| (layout.mips[i].tile_x, layout.mips[i].tile_y);
    assert_eq!(origin(2), (64, 0));
    assert_eq!(origin(3), (0, 64));
    assert!(
        layout.mips[2..]
            .iter()
            .all(|m| m.in_tail && m.region == 65536)
    );
}

#[test]
fn tail_mips_stay_inside_the_first_tile() {
    for (w, h, element, block) in [
        (256, 256, 4, 1),
        (512, 128, 8, 4),
        (1024, 1024, 16, 4),
        (64, 64, 1, 1),
    ] {
        let mips = 32 - u32::max(w, h).leading_zeros();
        let layout = compact_layout(TileMode::Sw64kD, w, h, mips, element, block).unwrap();
        for m in layout.mips.iter().filter(|m| m.in_tail) {
            for (x, y) in [(0, 0), (m.blocks_w - 1, m.blocks_h - 1)] {
                let at = layout.block_offset(m.index, x, y).unwrap();
                assert!(at < 65536, "{w}x{h} mip {}: {at}", m.index);
            }
        }
    }
}

#[test]
fn blocks_of_different_mips_never_share_a_byte() {
    let layout = compact_layout(TileMode::Sw4kS, 100, 70, 7, 4, 1).unwrap();
    let mut owner = std::collections::HashMap::new();
    for m in &layout.mips {
        for y in 0..m.blocks_h {
            for x in 0..m.blocks_w {
                let at = layout.block_offset(m.index, x, y).unwrap();
                assert!(at + 4 <= layout.size);
                assert_eq!(owner.insert(at, m.index), None);
            }
        }
    }
}

#[test]
fn compressed_formats_count_blocks() {
    let layout = compact_layout(TileMode::Sw64kD, 1024, 512, 11, 8, 4).unwrap();
    assert_eq!(layout.mips[0].blocks_w, 256);
    assert_eq!(layout.mips[0].blocks_h, 128);
    assert_eq!(layout.mips[10].blocks_w, 1);
    assert_eq!(layout.mips[3].blocks_w, 32);
}

#[test]
fn bad_descriptions_are_refused() {
    assert_eq!(
        compact_layout(TileMode::Sw64kD, 64, 64, 8, 4, 1),
        Err(LayoutError::Mips {
            width: 64,
            height: 64,
            mips: 8
        })
    );
    assert_eq!(
        compact_layout(TileMode::Sw64kD, 64, 64, 1, 12, 1),
        Err(LayoutError::Element(12))
    );
    assert_eq!(
        compact_layout(TileMode::Sw64kD, 64, 64, 1, 4, 2),
        Err(LayoutError::Block(2))
    );
    assert_eq!(
        compact_layout(TileMode::Sw64kD, 0, 64, 1, 4, 1),
        Err(LayoutError::Empty)
    );
}

#[test]
fn a_mip_survives_writing_and_reading_back() {
    let layout = compact_layout(TileMode::Sw64kD, 256, 256, 9, 4, 1).unwrap();
    let mut memory = vec![0u8; layout.size as usize];
    for mip in [0u32, 1, 2, 5, 8] {
        let m = &layout.mips[mip as usize];
        let data: Vec<u8> = (0..m.blocks_w * m.blocks_h * 4)
            .map(|i| (i * 13 + mip) as u8)
            .collect();
        layout.write_mip(mip, &data, &mut memory).unwrap();
        assert_eq!(layout.detile_mip(mip, &memory).unwrap(), data, "mip {mip}");
    }
    assert!(layout.detile_mip(0, &memory[..100]).is_none());
}

#[test]
fn arrays_keep_slices_of_a_mip_together() {
    let array = array_layout(TileMode::Sw64kD, 256, 256, 6, 9, 4, 1).unwrap();
    assert_eq!(array.slices, 6);
    assert_eq!(array.mips[0], (6 * 65536 + 6 * 65536, 262144));
    assert_eq!(array.mips[1], (6 * 65536, 65536));
    assert_eq!(array.mips[2], (0, 65536));
    assert_eq!(array.size, 6 * 65536 + 6 * 65536 + 6 * 262144);
    let single = array_layout(TileMode::Sw64kD, 100, 100, 3, 1, 4, 1).unwrap();
    assert_eq!(single.size, 3 * 65536);
}

#[test]
fn row_major_footprints_use_the_d3d12_alignments() {
    let (footprints, total) = row_major_footprints(100, 50, 1, 3, 1, [1, 1], 4);
    assert_eq!(footprints[0].row_pitch, 512);
    assert_eq!(footprints[0].row_bytes, 400);
    assert_eq!(footprints[0].size(), 512 * 50);
    assert_eq!(footprints[1].offset, 512 * 50);
    assert_eq!(footprints[1].row_pitch, 256);
    assert_eq!(footprints[2].offset % 512, 0);
    assert_eq!(total, footprints[2].offset + footprints[2].size());
    let (bc, _) = row_major_footprints(10, 10, 1, 1, 1, [4, 4], 8);
    assert_eq!(
        (bc[0].width_blocks, bc[0].rows, bc[0].row_bytes),
        (3, 3, 24)
    );
}

#[test]
fn array_subresources_do_not_share_bytes_and_round_trip() {
    let array = array_layout(TileMode::Sw64kD, 64, 64, 3, 7, 4, 1).unwrap();
    let mut owner = std::collections::HashMap::new();
    for slice in 0..3 {
        for m in &array.slice.mips {
            for y in 0..m.blocks_h {
                for x in 0..m.blocks_w {
                    let at = array.block_offset(slice, m.index, x, y).unwrap();
                    assert!(at + 4 <= array.size);
                    assert_eq!(owner.insert(at, (slice, m.index)), None);
                }
            }
        }
    }
    let mut memory = vec![0u8; array.size as usize];
    for (slice, mip) in [(0, 0), (2, 0), (1, 3), (2, 6)] {
        let m = &array.slice.mips[mip as usize];
        let rows: Vec<u8> = (0..m.blocks_w * m.blocks_h * 4)
            .map(|i| (i + slice * 7 + mip) as u8)
            .collect();
        array.tile(slice, mip, &rows, &mut memory).unwrap();
        assert_eq!(array.detile(slice, mip, &memory).unwrap(), rows);
    }
    assert!(array.block_offset(3, 0, 0, 0).is_none());
}

#[test]
fn a_mip_has_the_blocks_of_its_halved_texel_size() {
    let layout = compact_layout(TileMode::Sw64kD, 100, 60, 7, 4, 1).unwrap();
    assert_eq!((layout.mips[3].blocks_w, layout.mips[3].blocks_h), (12, 7));
    let bc = compact_layout(TileMode::Sw64kD, 100, 100, 7, 8, 4).unwrap();
    assert_eq!((bc.mips[3].blocks_w, bc.mips[3].blocks_h), (3, 3));
    assert_eq!((bc.mips[6].blocks_w, bc.mips[6].blocks_h), (1, 1));
}
