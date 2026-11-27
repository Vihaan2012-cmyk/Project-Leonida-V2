use super::tiling::*;

#[test]
fn tile_shapes_follow_the_element_size() {
    let shape = |mode, bytes| Tiled2D::new(mode, bytes, 1, 1).unwrap().tile_extent();
    assert_eq!(shape(TileMode::Sw64kD, 1), (256, 256));
    assert_eq!(shape(TileMode::Sw64kD, 2), (256, 128));
    assert_eq!(shape(TileMode::Sw64kD, 4), (128, 128));
    assert_eq!(shape(TileMode::Sw64kD, 8), (128, 64));
    assert_eq!(shape(TileMode::Sw64kD, 16), (64, 64));
    assert_eq!(shape(TileMode::Sw4kS, 4), (32, 32));
    assert_eq!(shape(TileMode::Sw4kS, 16), (16, 16));
}

#[test]
fn every_tile_holds_each_element_once() {
    for mode in [TileMode::Sw4kS, TileMode::Sw64kD] {
        for bytes in [1, 2, 4, 8, 16] {
            let s = Tiled2D::new(mode, bytes, 1, 1).unwrap();
            let (w, h) = s.tile_extent();
            let mut seen = vec![false; (mode.tile_bytes() / u64::from(bytes)) as usize];
            for y in 0..h {
                for x in 0..w {
                    let offset = s.intra_offset(x, y);
                    assert_eq!(offset % u64::from(bytes), 0);
                    let slot = (offset / u64::from(bytes)) as usize;
                    assert!(!seen[slot], "{mode:?} {bytes} bytes: ({x},{y}) collides");
                    seen[slot] = true;
                }
            }
            assert!(seen.iter().all(|s| *s));
        }
    }
}

#[test]
fn the_first_elements_of_a_64k_four_byte_tile() {
    let s = Tiled2D::new(TileMode::Sw64kD, 4, 256, 256).unwrap();
    assert_eq!(s.offset(0, 0), Some(0));
    assert_eq!(s.offset(1, 0), Some(4));
    assert_eq!(s.offset(2, 0), Some(8));
    assert_eq!(s.offset(0, 1), Some(16));
    assert_eq!(s.offset(0, 2), Some(32));
    assert_eq!(s.offset(128, 0), Some(65536));
    assert_eq!(s.offset(0, 128), Some(2 * 65536));
    assert_eq!(s.offset(256, 0), None);
    assert_eq!(s.size(), 4 * 65536);
}

#[test]
fn tiling_and_detiling_are_inverses_and_keep_row_padding_out() {
    for mode in [TileMode::Sw4kS, TileMode::Sw64kD] {
        for bytes in [1u32, 2, 4, 8, 16] {
            let (w, h) = (37u32, 21u32);
            let s = Tiled2D::new(mode, bytes, w, h).unwrap();
            let pitch = (w * bytes + 12) as usize;
            let linear: Vec<u8> = (0..pitch * h as usize).map(|i| (i * 7 + 3) as u8).collect();
            let mut tiled = vec![0u8; s.size() as usize];
            assert!(s.tile(&linear, &mut tiled, pitch));
            let mut back = vec![0xEEu8; linear.len()];
            assert!(s.detile(&tiled, &mut back, pitch));
            for y in 0..h as usize {
                let row = y * pitch..y * pitch + (w * bytes) as usize;
                assert_eq!(back[row.clone()], linear[row]);
                let pad = y * pitch + (w * bytes) as usize..(y + 1) * pitch;
                if y + 1 < h as usize {
                    assert!(back[pad].iter().all(|b| *b == 0xEE));
                }
            }
        }
    }
}

#[test]
fn short_buffers_are_refused() {
    let s = Tiled2D::new(TileMode::Sw4kS, 4, 8, 8).unwrap();
    let mut tiled = vec![0u8; s.size() as usize - 1];
    assert!(!s.tile(&[0; 256], &mut tiled, 32));
    let mut full = vec![0u8; s.size() as usize];
    assert!(!s.tile(&[0; 255], &mut full, 32));
    assert!(!s.tile(&[0; 256], &mut full, 31));
    assert!(Tiled2D::new(TileMode::Sw4kS, 12, 8, 8).is_none());
    assert!(Tiled2D::new(TileMode::Sw4kS, 4, 0, 8).is_none());
}
