#[path = "../common/mod.rs"]
mod common;
mod harness;
mod scene;

use harness::{Heap, Runner, SORTED, UNSORTED};
use scene::{Rng, blas, node_queries, tlas};

fn runner() -> Option<Runner> {
    common::gpu().map(Runner::new)
}

fn check_structure(runner: &Runner, bytes: &[u8], seed: u64, per_node: usize, label: &str) {
    let mut rng = Rng(seed);
    let mut heap = Heap::default();
    let base = heap.place(bytes);
    let queries = node_queries(&mut rng, bytes, per_node, true);
    for high in [SORTED, UNSORTED, 0xFF80_0000, 0x3800_0000] {
        for wide in [false, true] {
            let report = runner.compare(&heap, base, high, wide, &queries);
            assert!(report.hits > 0);
            report.assert_clean(&format!("{label} high {high:#x} wide {wide}"));
        }
    }
}

#[test]
fn rays_over_a_box32_blas_match_the_reference() {
    let Some(runner) = runner() else { return };
    let mut rng = Rng(3);
    let bytes = blas(&mut rng, 400, false);
    check_structure(&runner, &bytes, 11, 24, "box32 blas");
}

#[test]
fn rays_over_a_box16_blas_match_the_reference() {
    let Some(runner) = runner() else { return };
    let mut rng = Rng(4);
    let bytes = blas(&mut rng, 400, true);
    check_structure(&runner, &bytes, 12, 24, "box16 blas");
}

#[test]
fn rays_over_a_tlas_match_the_reference() {
    let Some(runner) = runner() else { return };
    let mut rng = Rng(5);
    let first = blas(&mut rng, 150, false);
    let second = blas(&mut rng, 150, true);
    let mut heap = Heap::default();
    let bases = [heap.place(&first), heap.place(&second)];
    let transforms: Vec<[f32; 12]> = (0..9)
        .map(|i| {
            let (x, y, z) = (i as f32 * 3.0 - 12.0, (i % 3) as f32 * 2.0, 1.0);
            [1.0, 0.0, 0.0, x, 0.0, 1.0, 0.0, y, 0.0, 0.0, 1.0, z]
        })
        .collect();
    let top = tlas(&[(bases[0], &first), (bases[1], &second)], &transforms);
    let base = heap.place(&top);
    let queries = node_queries(&mut rng, &top, 40, true);
    for wide in [false, true] {
        let report = runner.compare(&heap, base, SORTED, wide, &queries);
        assert!(report.hits > 0);
        report.assert_clean(&format!("tlas wide {wide}"));
    }
}
