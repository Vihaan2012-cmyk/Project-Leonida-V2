mod common;

use bvh_build::{
    BlasInfo, Cpu, Header, blas_info, build_blas, build_tlas, compact, flags, max_nodes,
    prebuild_size, refit_blas, refit_tlas,
};
use common::*;

fn scene_of(structures: &[&[u8]]) -> (Scene, Vec<u64>) {
    let total: usize = structures.iter().map(|s| s.len().div_ceil(256) * 256).sum();
    let mut scene = Scene::new(total);
    let bases = structures.iter().map(|s| scene.place(s)).collect();
    (scene, bases)
}

fn check_blas(count: usize, build_flags: u32, seed: u64) {
    let mut rng = Rng(seed);
    let geometries = vec![
        geometry(triangles(&mut rng, count / 2, 1.5)),
        geometry(triangles(&mut rng, count - count / 2, 0.7)),
    ];
    let bytes = build_blas(&geometries, build_flags, &mut Cpu).unwrap();
    check_structure(&bytes);
    let (mut scene, bases) = scene_of(&[&bytes]);
    let mut wrong = 0;
    let mut hits = 0;
    for ray in rays(&mut rng, 120) {
        let found = scene.trace_blas(bases[0], &ray);
        let expected = brute_force(&geometries, &ray);
        hits += usize::from(expected.is_some());
        wrong += usize::from(!agrees(&geometries, &ray, found, expected));
    }
    assert!(wrong <= 1, "{wrong} of 120 rays disagree with brute force");
    assert!(count < 100 || hits > 5, "{hits} hits");
}

#[test]
fn a_box32_blas_finds_the_nearest_triangle() {
    check_blas(1500, 0, 1);
}

#[test]
fn a_box16_blas_finds_the_nearest_triangle() {
    check_blas(1500, flags::MINIMIZE_MEMORY, 2);
}

#[test]
fn small_blas_shapes_work() {
    for (count, seed) in [(1, 3), (2, 4), (3, 5), (4, 6), (5, 7), (9, 8), (40, 9)] {
        check_blas(count, 0, seed);
        check_blas(count, flags::MINIMIZE_MEMORY, seed + 100);
    }
}

#[test]
fn an_empty_blas_has_no_root() {
    let bytes = build_blas(&[], 0, &mut Cpu).unwrap();
    let header = Header::read(&bytes).unwrap();
    assert_eq!(header.root, u32::MAX);
    assert_eq!(header.elements_count, 0);
    let info = blas_info(&bytes).unwrap();
    assert_eq!(info.root, u32::MAX);
}

#[test]
fn header_fields_follow_the_console_layout() {
    let mut rng = Rng(11);
    let geometries = vec![geometry(triangles(&mut rng, 300, 1.0))];
    let bytes = build_blas(&geometries, flags::MINIMIZE_MEMORY, &mut Cpu).unwrap();
    let header = Header::read(&bytes).unwrap();
    assert_eq!(header.flags, 0x8000_0006);
    assert_eq!(header.elements_count, 300);
    assert_eq!(header.size(), bytes.len());
    assert_eq!(header.elements % 128, 0);
    assert_eq!(header.nodes % 128, 0);
    assert_eq!(header.nodes - header.elements, 19200);
    assert_eq!(header.root & 7, 4);
    assert_eq!(
        u32::from_le_bytes(bytes[0x2C..0x30].try_into().unwrap()),
        0,
        "no primitive remap"
    );
    let all: Vec<[f32; 3]> = geometries[0]
        .triangles
        .iter()
        .flat_map(|t| t.vertices)
        .collect();
    for a in 0..3 {
        let low = all.iter().map(|v| v[a]).fold(f32::INFINITY, f32::min);
        let high = all.iter().map(|v| v[a]).fold(f32::NEG_INFINITY, f32::max);
        assert_eq!((header.bounds[a], header.bounds[3 + a]), (low, high));
    }
}

#[test]
fn the_node_count_stays_under_the_bound_and_the_size_under_the_prebuild() {
    for count in [2usize, 3, 7, 50, 777] {
        for build_flags in [0, flags::MINIMIZE_MEMORY] {
            let mut rng = Rng(count as u64);
            let geometries = vec![geometry(triangles(&mut rng, count, 1.0))];
            let bytes = build_blas(&geometries, build_flags, &mut Cpu).unwrap();
            let nodes = check_structure(&bytes);
            assert!(nodes <= max_nodes(count), "{nodes} nodes for {count}");
            assert!(bytes.len() <= prebuild_size(count, true, build_flags).unwrap());
            assert_eq!(compact(&bytes).unwrap(), bytes);
        }
    }
}

#[test]
fn a_box16_structure_ends_its_nodes_with_a_spare_unit() {
    let mut rng = Rng(12);
    let geometries = vec![geometry(triangles(&mut rng, 100, 1.0))];
    let bytes = build_blas(&geometries, flags::MINIMIZE_MEMORY, &mut Cpu).unwrap();
    let header = Header::read(&bytes).unwrap();
    let nodes = check_structure(&bytes);
    let end = header.nodes as usize + 64 * nodes;
    assert!(bytes[end..end + 64].iter().all(|b| *b == 0));
    assert!(end + 64 <= header.parents as usize);
}

#[test]
fn inactive_triangles_are_never_hit() {
    let mut rng = Rng(13);
    let mut list = triangles(&mut rng, 50, 1.0);
    list[10].vertices[1] = [f32::NAN; 3];
    let geometries = vec![geometry(list)];
    let bytes = build_blas(&geometries, 0, &mut Cpu).unwrap();
    check_structure(&bytes);
    let (mut scene, bases) = scene_of(&[&bytes]);
    for ray in rays(&mut rng, 80) {
        let found = scene.trace_blas(bases[0], &ray);
        assert_ne!(found.map(|h| h.primitive), Some(10));
    }
}

#[test]
fn a_tlas_reaches_instanced_geometry() {
    let mut rng = Rng(21);
    let a = vec![geometry(triangles(&mut rng, 400, 1.0))];
    let b = vec![geometry(triangles(&mut rng, 150, 2.0))];
    let blas_a = build_blas(&a, 0, &mut Cpu).unwrap();
    let blas_b = build_blas(&b, flags::MINIMIZE_MEMORY, &mut Cpu).unwrap();
    let transforms = [
        [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0, 6.0, 0.0, 1.0, 0.0, -3.0, 0.0, 0.0, 1.0, 2.0],
        [0.0, -1.0, 0.0, 1.0, 1.0, 0.0, 0.0, 2.0, 0.0, 0.0, 2.0, -4.0],
        [0.5, 0.0, 0.0, -5.0, 0.0, 0.5, 0.0, 5.0, 0.0, 0.0, 0.5, 0.0],
    ];
    let probe = scene_of(&[&blas_a, &blas_b]);
    let [base_a, base_b] = [probe.1[0], probe.1[1]];
    let instances = vec![
        instance(transforms[0], 7, base_a),
        instance(transforms[1], 8, base_b),
        instance(transforms[2], 9, base_a),
        instance(transforms[3], 10, base_b),
    ];
    let info = |address: u64| {
        if address == base_a {
            blas_info(&blas_a).ok()
        } else if address == base_b {
            blas_info(&blas_b).ok()
        } else {
            None::<BlasInfo>
        }
    };
    let tlas = build_tlas(&instances, &info, &mut Cpu).unwrap();
    check_structure(&tlas);
    let header = Header::read(&tlas).unwrap();
    assert_eq!(header.flags, 6);
    let (mut scene, bases) = scene_of(&[&blas_a, &blas_b, &tlas]);
    assert_eq!([bases[0], bases[1]], [base_a, base_b]);
    let mut wrong = 0;
    let mut hits = 0;
    for ray in rays(&mut rng, 150) {
        let found = scene.trace_tlas(bases[2], &ray);
        let mut expected: Option<(u32, Hit)> = None;
        for instance in &instances {
            let geometries = if instance.blas == base_a { &a } else { &b };
            let world: Vec<_> = geometries
                .iter()
                .map(|g| bvh_build::Geometry {
                    flags: g.flags,
                    triangles: g
                        .triangles
                        .iter()
                        .map(|t| bvh_build::Triangle {
                            vertices: t.vertices.map(|p| {
                                let m = &instance.transform;
                                [0, 1, 2].map(|r| {
                                    m[4 * r] * p[0]
                                        + m[4 * r + 1] * p[1]
                                        + m[4 * r + 2] * p[2]
                                        + m[4 * r + 3]
                                })
                            }),
                        })
                        .collect(),
                })
                .collect();
            if let Some(hit) = brute_force(&world, &ray)
                && expected.is_none_or(|e| hit.t < e.1.t)
            {
                expected = Some((instance.id, hit));
            }
        }
        hits += usize::from(expected.is_some());
        let same = match (found, expected) {
            (None, None) => true,
            (Some(f), Some(e)) => f.0 == e.0 && (f.1.t - e.1.t).abs() <= 3e-4 * e.1.t.max(1.0),
            _ => false,
        };
        wrong += usize::from(!same);
    }
    assert!(hits > 10, "{hits} hits");
    assert!(wrong <= 1, "{wrong} of 150 rays disagree");
}

#[test]
fn inactive_instances_stay_out_of_the_tree() {
    let mut rng = Rng(22);
    let a = vec![geometry(triangles(&mut rng, 30, 1.0))];
    let blas = build_blas(&a, 0, &mut Cpu).unwrap();
    let identity = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
    let singular = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let mut masked = instance(identity, 1, 0x1000);
    masked.mask = 0;
    let instances = vec![
        instance(identity, 2, 0x1000),
        masked,
        instance(singular, 3, 0x1000),
        instance(identity, 4, 0x2000),
    ];
    let info = |address: u64| (address == 0x1000).then(|| blas_info(&blas).unwrap());
    let tlas = build_tlas(&instances, &info, &mut Cpu).unwrap();
    check_structure(&tlas);
    let header = Header::read(&tlas).unwrap();
    assert_eq!(header.elements_count, 4);
    let word = |at: usize| u32::from_le_bytes(tlas[at..at + 4].try_into().unwrap());
    let record = |i: usize| header.elements as usize + 128 * i;
    assert_eq!(
        word(record(0) + 56) & 0x8000_0000,
        0x8000_0000,
        "identity rotation flag"
    );
    assert_eq!(word(record(0) + 60), 0x10);
    assert_eq!(word(record(0) + 48), 0xFF00_0002);
    assert_eq!(word(record(1) + 48), 1, "no mask");
    assert_eq!(word(record(3) + 56), u32::MAX, "no BLAS");
    assert_eq!(word(record(3) + 112), 3);
    assert!(header.bounds[0].is_finite());
}

#[test]
fn a_refit_follows_moving_geometry() {
    let mut rng = Rng(31);
    let before = vec![geometry(triangles(&mut rng, 600, 1.0))];
    let mut bytes = build_blas(&before, flags::ALLOW_UPDATE, &mut Cpu).unwrap();
    let after: Vec<_> = before
        .iter()
        .map(|g| bvh_build::Geometry {
            flags: g.flags,
            triangles: g
                .triangles
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let shift = [3.0 * (i % 5) as f32 - 6.0, 2.0, -((i % 3) as f32)];
                    bvh_build::Triangle {
                        vertices: t
                            .vertices
                            .map(|v| [v[0] + shift[0], v[1] + shift[1], v[2] + shift[2]]),
                    }
                })
                .collect(),
        })
        .collect();
    refit_blas(&mut bytes, &after).unwrap();
    check_structure(&bytes);
    let header = Header::read(&bytes).unwrap();
    assert_eq!(header.elements_count, 600);
    let (mut scene, bases) = scene_of(&[&bytes]);
    let mut wrong = 0;
    for ray in rays(&mut rng, 120) {
        let found = scene.trace_blas(bases[0], &ray);
        wrong += usize::from(!agrees(&after, &ray, found, brute_force(&after, &ray)));
    }
    assert!(wrong <= 1, "{wrong} wrong after the refit");
    let mut fewer = after.clone();
    fewer[0].triangles.pop();
    assert!(refit_blas(&mut bytes, &fewer).is_err());
}

#[test]
fn a_tlas_refit_moves_its_instances() {
    let mut rng = Rng(32);
    let a = vec![geometry(triangles(&mut rng, 60, 1.0))];
    let blas = build_blas(&a, 0, &mut Cpu).unwrap();
    let info = |address: u64| (address == 0x1000).then(|| blas_info(&blas).unwrap());
    let at = |x: f32| [1.0, 0.0, 0.0, x, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
    let instances: Vec<_> = (0..6)
        .map(|i| instance(at(i as f32 * 5.0), i, 0x1000))
        .collect();
    let mut tlas = build_tlas(&instances, &info, &mut Cpu).unwrap();
    let moved: Vec<_> = (0..6)
        .map(|i| instance(at(100.0 + i as f32 * 5.0), i, 0x1000))
        .collect();
    refit_tlas(&mut tlas, &moved, &info).unwrap();
    check_structure(&tlas);
    let header = Header::read(&tlas).unwrap();
    assert!(header.bounds[0] > 80.0, "{:?}", header.bounds);
}
