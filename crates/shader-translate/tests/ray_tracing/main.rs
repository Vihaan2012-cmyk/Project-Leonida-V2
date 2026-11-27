mod bench;
#[path = "../common/mod.rs"]
mod common;
mod harness;
mod nodes;
mod scene;

use harness::Query;
use harness::{Heap, Runner, SORTED, UNSORTED};
use nodes::{Nodes, child_pointer, special_box, special_query, to_half};
use scene::{Rng, aimed, blas, node_queries, on_triangle, tlas};

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

fn compare_all(runner: &Runner, nodes: &Nodes, queries: &[Query], label: &str) {
    let (heap, base) = nodes.heap();
    for high in [SORTED, UNSORTED, 0xFF80_0000] {
        for wide in [false, true] {
            let report = runner.compare(&heap, base, high, wide, queries);
            report.assert_clean(&format!("{label} high {high:#x} wide {wide}"));
        }
    }
}

fn node_pointers(nodes: &Nodes) -> Vec<u32> {
    (0..nodes.bytes.len() / 128)
        .map(|i| (2 * i as u32) << 3)
        .collect()
}

#[test]
fn box32_nodes_with_special_values_match_the_reference() {
    let Some(runner) = runner() else { return };
    let denormals = runner.preserves_denormals();
    let mut rng = Rng(21);
    let mut nodes = Nodes::default();
    for _ in 0..160 {
        let boxes = [0; 4].map(|_| special_box(&mut rng, denormals));
        nodes.box32([0; 4].map(|_| child_pointer(&mut rng)), boxes);
    }
    let mut queries = Vec::new();
    for (i, pointer) in node_pointers(&nodes).into_iter().enumerate() {
        let pointer = pointer | 5;
        let at = i * 128 + 16;
        for _ in 0..40 {
            queries.push(special_query(&mut rng, pointer, denormals));
        }
        for _ in 0..24 {
            let target = [0; 3].map(|a| {
                let at = at + 4 * a;
                let value = f32::from_le_bytes(nodes.bytes[at..at + 4].try_into().unwrap());
                if value.is_finite() { value } else { 0.0 }
            });
            let inside = rng.chance(30);
            queries.push(aimed(&mut rng, pointer, target, inside));
        }
    }
    compare_all(&runner, &nodes, &queries, "special box32");
}

#[test]
fn box16_nodes_with_any_half_bits_match_the_reference() {
    let Some(runner) = runner() else { return };
    let denormals = runner.preserves_denormals();
    let mut rng = Rng(22);
    let mut nodes = Nodes::default();
    for _ in 0..160 {
        let half = |rng: &mut Rng| {
            if rng.chance(25) {
                rng.next() & 0xFFFF
            } else if rng.chance(25) {
                to_half(rng.special(denormals))
            } else {
                to_half(rng.range(-3.0, 3.0))
            }
        };
        let halves = [0; 4].map(|_| [0; 3].map(|_| half(&mut rng) | half(&mut rng) << 16));
        nodes.box16([0; 4].map(|_| child_pointer(&mut rng)), halves);
    }
    let mut queries = Vec::new();
    for pointer in node_pointers(&nodes) {
        for _ in 0..64 {
            queries.push(special_query(&mut rng, pointer | 4, denormals));
        }
        for _ in 0..16 {
            let origin = rng.point(-3.0, 3.0);
            let target = rng.point(-3.0, 3.0);
            let direction = [0, 1, 2].map(|a| target[a] - origin[a]);
            queries.push(Query::new(pointer | 4, 100.0, origin, direction));
        }
    }
    compare_all(&runner, &nodes, &queries, "special box16");
}

#[test]
fn triangle_nodes_with_degenerate_geometry_match_the_reference() {
    let Some(runner) = runner() else { return };
    let denormals = runner.preserves_denormals();
    let mut rng = Rng(23);
    let mut nodes = Nodes::default();
    let mut shapes = Vec::new();
    for i in 0..240 {
        let mut vertices = [0; 4].map(|_| {
            if rng.chance(15) {
                [0; 3].map(|_| rng.special(denormals))
            } else {
                rng.point(-2.0, 2.0)
            }
        });
        match i % 6 {
            1 => vertices[1] = vertices[0],
            2 => {
                let k = rng.range(-2.0, 2.0);
                vertices[2] =
                    [0, 1, 2].map(|a| vertices[0][a] + k * (vertices[1][a] - vertices[0][a]));
            }
            3 => (vertices[1], vertices[2]) = (vertices[0], vertices[0]),
            4 => vertices[2][rng.below(3) as usize] = f32::NAN,
            5 => {
                vertices[2] = vertices[1];
                vertices[3] = vertices[0];
            }
            _ => {}
        }
        shapes.push(vertices);
        nodes.leaf(vertices, rng.next());
    }
    let mut queries = Vec::new();
    for (pointer, vertices) in node_pointers(&nodes).into_iter().zip(&shapes) {
        for _ in 0..32 {
            let kind = rng.below(4);
            queries.push(special_query(&mut rng, pointer | kind, denormals));
        }
        let finite = vertices.iter().flatten().all(|v| v.is_finite());
        for _ in 0..32 {
            let kind = rng.below(4);
            if finite {
                queries.push(on_triangle(
                    &mut rng,
                    pointer | kind,
                    [vertices[0], vertices[1], vertices[2]],
                ));
            }
        }
    }
    compare_all(&runner, &nodes, &queries, "degenerate triangles");
}

fn unit_box(corner: [f32; 3]) -> [f32; 6] {
    [
        corner[0],
        corner[1],
        corner[2],
        corner[0] + 1.0,
        corner[1] + 1.0,
        corner[2] + 1.0,
    ]
}

#[test]
fn axis_aligned_rays_against_unit_boxes_match_the_reference() {
    let Some(runner) = runner() else { return };
    let mut nodes = Nodes::default();
    let corners = [
        [0.0; 3],
        [1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        [-1.0, -1.0, -1.0],
    ];
    let boxes = corners.map(unit_box);
    let halves = boxes.map(|b| [0, 1, 2].map(|i| to_half(b[2 * i]) | to_half(b[2 * i + 1]) << 16));
    let children = [0x100, 0x108, 0x110, 0x118];
    nodes.box32(children, boxes);
    nodes.box16(
        children,
        [
            [halves[0][0], halves[0][1], halves[0][2]],
            [halves[1][0], halves[1][1], halves[1][2]],
            [halves[2][0], halves[2][1], halves[2][2]],
            [halves[3][0], halves[3][1], halves[3][2]],
        ],
    );
    let mut queries = Vec::new();
    let signs = [0.0, -0.0, 1.0, -1.0];
    let places = [-1.0, 0.0, 0.5, 1.0, 2.0];
    for kind in [5, 4] {
        let pointer = if kind == 5 { 5 } else { (2 << 3) | 4 };
        for dx in signs {
            for dy in signs {
                for dz in signs {
                    for ox in places {
                        for oy in places {
                            for oz in places {
                                for extent in [f32::INFINITY, 10.0] {
                                    queries.push(Query::new(
                                        pointer,
                                        extent,
                                        [ox, oy, oz],
                                        [dx, dy, dz],
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    compare_all(&runner, &nodes, &queries, "unit boxes");
}

#[test]
fn special_rays_over_real_structures_match_the_reference() {
    let Some(runner) = runner() else { return };
    let denormals = runner.preserves_denormals();
    let mut rng = Rng(24);
    for box16 in [false, true] {
        let bytes = blas(&mut rng, 120, box16);
        let mut heap = Heap::default();
        let base = heap.place(&bytes);
        let mut queries = Vec::new();
        for pointer in scene::pointers(&bytes) {
            for _ in 0..48 {
                queries.push(special_query(&mut rng, pointer, denormals));
            }
        }
        for wide in [false, true] {
            let report = runner.compare(&heap, base, SORTED, wide, &queries);
            report.assert_clean(&format!("special rays box16 {box16} wide {wide}"));
        }
    }
}

#[test]
fn nodes_of_other_types_return_all_ones() {
    let Some(runner) = runner() else { return };
    let mut rng = Rng(25);
    let bytes = blas(&mut rng, 60, false);
    let mut heap = Heap::default();
    let base = heap.place(&bytes);
    let mut queries = node_queries(&mut rng, &bytes, 4, false);
    for query in &mut queries {
        query.pointer = (query.pointer & !7) | (6 + rng.below(2));
    }
    for wide in [false, true] {
        let report = runner.compare(&heap, base, SORTED, wide, &queries);
        report.assert_clean("other types");
        assert!(report.results.iter().all(|r| *r == [u32::MAX; 4]));
    }
}
