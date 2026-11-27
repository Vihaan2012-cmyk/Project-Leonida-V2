use gpu_passes::bc1::{Bc1Tables, LANES, rebuild_cpu};

fn flat_tables() -> Bc1Tables {
    let mut quadrant = [0u8; 256];
    for (i, q) in quadrant.iter_mut().enumerate() {
        *q = (i as u8).wrapping_mul(37).wrapping_add(11);
    }
    let mut history = [None; 256];
    for (entry, h) in history.iter_mut().enumerate().skip(1) {
        if entry % 7 != 0 {
            *h = Some([
                (entry % 32) as u8,
                (entry % 64) as u8,
                (entry * 3 % 32) as u8,
                (entry * 5 % 32) as u8,
                (entry * 11 % 64) as u8,
                (entry * 13 % 32) as u8,
            ]);
        }
    }
    Bc1Tables { quadrant, history }
}

#[test]
fn bc1_rebuild_follows_the_history_chain() {
    let tables = flat_tables();
    // Block 0 is absolute (d = 0) with deltas +1 -1 +2 0 +3 -2; block 1 refers to block 0 (d = 1, symbol 2).
    let mut lanes: [Vec<u8>; LANES] = std::array::from_fn(|_| vec![0u8; 2]);
    lanes[0] = vec![0, 2];
    for (lane, zig) in [(2, 2u8), (3, 1), (4, 4), (5, 0), (6, 6), (7, 3)] {
        lanes[lane][0] = zig;
    }
    lanes[8] = vec![5, 0];
    let refs: [&[u8]; LANES] = std::array::from_fn(|i| lanes[i].as_slice());
    let out = rebuild_cpu(&refs, &tables).unwrap();
    // Block 0 endpoints: r0 1, g0 63 (-1 mod 64), b0 2, r1 0, g1 3, b1 30 (-2 mod 32).
    let c0 = (1u16 << 11) | (63 << 5) | 2;
    let c1 = (3u16 << 5) | 30;
    assert_eq!(&out[0..2], &c0.to_le_bytes());
    assert_eq!(&out[2..4], &c1.to_le_bytes());
    assert_eq!(out[4] & 15, tables.quadrant[5] & 15);
    // Block 1 has no deltas of its own: it repeats block 0's endpoints.
    assert_eq!(&out[8..12], &out[0..4]);
}

#[test]
fn bc1_rebuild_reports_an_unrecovered_history_entry() {
    let tables = flat_tables();
    // d = 7 from block 0 asks for entry 256 - 7 = 249; entry 249 % 7 == 4, so build one that is missing.
    let mut tables = tables;
    tables.history[249] = None;
    let mut lanes: [Vec<u8>; LANES] = std::array::from_fn(|_| vec![0u8; 1]);
    lanes[0] = vec![14];
    let refs: [&[u8]; LANES] = std::array::from_fn(|i| lanes[i].as_slice());
    assert!(rebuild_cpu(&refs, &tables).is_err());
}
