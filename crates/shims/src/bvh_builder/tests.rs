use std::cell::{Cell, RefCell};

use bvh_build::{Cpu, Geometry, Header, Triangle, build_blas};

use super::*;

struct Arena {
    base: u64,
    data: RefCell<Vec<u8>>,
    next: Cell<u64>,
}

impl Arena {
    fn new() -> Arena {
        Arena {
            base: 0x10_0000,
            data: RefCell::new(vec![0; 4 << 20]),
            next: Cell::new(0x10_0000),
        }
    }

    fn alloc(&self, bytes: &[u8]) -> u64 {
        let at = self.next.get();
        self.next
            .set((at + bytes.len() as u64).div_ceil(256) * 256 + 256);
        assert!(self.write(at, bytes));
        at
    }

    fn zeroed(&self, length: usize) -> u64 {
        self.alloc(&vec![0; length])
    }

    fn range(&self, address: u64, length: usize) -> Option<std::ops::Range<usize>> {
        let start = address.checked_sub(self.base)? as usize;
        (start + length <= self.data.borrow().len()).then_some(start..start + length)
    }
}

impl Memory for Arena {
    fn read(&self, address: u64, length: usize) -> Option<Vec<u8>> {
        let range = self.range(address, length)?;
        Some(self.data.borrow()[range].to_vec())
    }

    fn write(&self, address: u64, data: &[u8]) -> bool {
        match self.range(address, data.len()) {
            Some(range) => {
                self.data.borrow_mut()[range].copy_from_slice(data);
                true
            }
            None => false,
        }
    }
}

fn le(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn floats(values: &[f32]) -> Vec<u8> {
    le(&values.iter().map(|v| v.to_bits()).collect::<Vec<_>>())
}

fn quads(values: &[u64]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

struct Mesh {
    vertices: Vec<f32>,
    indices: Option<Vec<u16>>,
}

fn grid(count: usize, shift: f32) -> Mesh {
    let mut vertices = Vec::new();
    for t in 0..count {
        let x = (t % 17) as f32 * 1.5 + shift;
        let y = (t / 17) as f32 * 1.5;
        vertices.extend_from_slice(&[x, y, 0.0, x + 1.0, y, 0.0, x, y + 1.0, 0.5]);
    }
    Mesh {
        vertices,
        indices: None,
    }
}

fn geometry_desc(arena: &Arena, mesh: &Mesh, transform: Option<[f32; 12]>) -> Vec<u8> {
    let vertex_buffer = arena.alloc(&floats(&mesh.vertices));
    let vertex_count = mesh.vertices.len() / 3;
    let (index_format, index_count, index_buffer) = match &mesh.indices {
        Some(indices) => {
            let bytes: Vec<u8> = indices.iter().flat_map(|i| i.to_le_bytes()).collect();
            (57, indices.len() as u32, arena.alloc(&bytes))
        }
        None => (0, 0, 0),
    };
    let transform = transform.map_or(0, |m| arena.alloc(&floats(&m)));
    let mut desc = le(&[0, 1]);
    desc.extend(quads(&[transform]));
    desc.extend(le(&[index_format, 6, index_count, vertex_count as u32]));
    desc.extend(quads(&[index_buffer, vertex_buffer, 12]));
    assert_eq!(desc.len(), 56);
    desc
}

fn inputs(kind: u32, build_flags: u32, count: u32, layout: u32, address: u64) -> Vec<u8> {
    let mut out = le(&[kind, build_flags, count, layout]);
    out.extend(quads(&[address]));
    out
}

struct Batch {
    destination: u64,
    inputs: Vec<u8>,
    source: u64,
    postbuild: Vec<(u32, u64)>,
}

fn batch(arena: &Arena, b: &Batch) -> u64 {
    let post: Vec<u8> = b
        .postbuild
        .iter()
        .flat_map(|(kind, address)| {
            let mut record = quads(&[*address]);
            record.extend(le(&[*kind, 0]));
            record
        })
        .collect();
    let post_address = if post.is_empty() {
        0
    } else {
        arena.alloc(&post)
    };
    let mut desc = quads(&[b.destination]);
    desc.extend(&b.inputs);
    desc.extend(quads(&[b.source, 0]));
    desc.extend(le(&[b.postbuild.len() as u32, 0]));
    desc.extend(quads(&[post_address]));
    desc.extend(vec![0; 16]);
    assert_eq!(desc.len(), 0x50);
    arena.alloc(&desc)
}

fn header_at(arena: &Arena, address: u64) -> Header {
    let head = arena.read(address, 64).unwrap();
    let size = 64 * (u32::from_le_bytes(head[4..8].try_into().unwrap()) as usize + 1);
    Header::read(&arena.read(address, size).unwrap()).unwrap()
}

fn expected(mesh: &Mesh, build_flags: u32) -> Vec<u8> {
    let triangles = mesh
        .vertices
        .chunks(9)
        .map(|c| Triangle {
            vertices: [0, 1, 2].map(|k| [c[3 * k], c[3 * k + 1], c[3 * k + 2]]),
        })
        .collect();
    build_blas(
        &[Geometry {
            triangles,
            flags: 1,
        }],
        build_flags,
        &mut Cpu,
    )
    .unwrap()
}

#[test]
fn prebuild_answers_the_layout_size() {
    let arena = Arena::new();
    let mesh = grid(100, 0.0);
    let geometry = arena.alloc(&geometry_desc(&arena, &mesh, None));
    let input = arena.alloc(&inputs(1, 0, 1, 0, geometry));
    let out = arena.zeroed(24);
    prebuild(&arena, input, out).unwrap();
    let result = arena.read(out, 24).unwrap();
    let size = u64::from_le_bytes(result[..8].try_into().unwrap());
    assert_eq!(size, prebuild_size(100, true, 0).unwrap() as u64);
    assert_eq!(u64::from_le_bytes(result[8..16].try_into().unwrap()), 256);
    let small = arena.alloc(&inputs(1, flags::MINIMIZE_MEMORY, 1, 0, geometry));
    prebuild(&arena, small, out).unwrap();
    let smaller = u64::from_le_bytes(arena.read(out, 8).unwrap().try_into().unwrap());
    assert!(smaller < size);
}

#[test]
fn a_batched_build_writes_the_structure_and_postbuild_sizes() {
    let arena = Arena::new();
    let mesh = grid(250, 0.0);
    let geometry = arena.alloc(&geometry_desc(&arena, &mesh, None));
    let input = inputs(1, 0, 1, 0, geometry);
    let destination = arena.zeroed(1 << 20);
    let compacted = arena.zeroed(8);
    let current = arena.zeroed(8);
    let desc = batch(
        &arena,
        &Batch {
            destination,
            inputs: input,
            source: 0,
            postbuild: vec![(0, compacted), (3, current), (1, 0)],
        },
    );
    assert!(build_batched(&arena, 1, desc).is_empty());
    let want = expected(&mesh, 0);
    assert_eq!(arena.read(destination, want.len()).unwrap(), want);
    let size = |at| u64::from_le_bytes(arena.read(at, 8).unwrap().try_into().unwrap());
    assert_eq!(size(compacted), want.len() as u64);
    assert_eq!(size(current), want.len() as u64);
    let header = Header::read(&want).unwrap();
    assert_eq!(header.elements_count, 250);
    assert!(want.len() as u64 <= prebuild_size(250, true, 0).unwrap() as u64);
}

#[test]
fn indices_and_transforms_reach_the_triangles() {
    let arena = Arena::new();
    let mesh = Mesh {
        vertices: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0],
        indices: Some(vec![0, 1, 2, 2, 1, 3, 0xFFFF, 1, 2]),
    };
    let shift = [
        1.0, 0.0, 0.0, 10.0, 0.0, 1.0, 0.0, 20.0, 0.0, 0.0, 1.0, 30.0,
    ];
    let geometry = arena.alloc(&geometry_desc(&arena, &mesh, Some(shift)));
    let destination = arena.zeroed(1 << 16);
    let desc = batch(
        &arena,
        &Batch {
            destination,
            inputs: inputs(1, 0, 1, 0, geometry),
            source: 0,
            postbuild: vec![],
        },
    );
    assert!(build_batched(&arena, 1, desc).is_empty());
    let header = header_at(&arena, destination);
    assert_eq!(header.elements_count, 3);
    assert_eq!(&header.bounds[..3], &[10.0, 20.0, 30.0]);
    assert_eq!(&header.bounds[3..], &[11.0, 21.0, 30.0]);
}

#[test]
fn an_update_refits_the_source_in_place() {
    let arena = Arena::new();
    let before = grid(120, 0.0);
    let geometry = arena.alloc(&geometry_desc(&arena, &before, None));
    let destination = arena.zeroed(1 << 20);
    let build = batch(
        &arena,
        &Batch {
            destination,
            inputs: inputs(1, flags::ALLOW_UPDATE, 1, 0, geometry),
            source: 0,
            postbuild: vec![],
        },
    );
    assert!(build_batched(&arena, 1, build).is_empty());
    let after = grid(120, 100.0);
    let moved = arena.alloc(&geometry_desc(&arena, &after, None));
    let update = batch(
        &arena,
        &Batch {
            destination,
            inputs: inputs(1, flags::ALLOW_UPDATE | flags::PERFORM_UPDATE, 1, 0, moved),
            source: destination,
            postbuild: vec![],
        },
    );
    assert!(build_batched(&arena, 1, update).is_empty());
    let header = header_at(&arena, destination);
    assert!(header.bounds[0] >= 100.0, "{:?}", header.bounds);
}

#[test]
fn a_tlas_names_its_blas_by_address() {
    let arena = Arena::new();
    let mesh = grid(60, 0.0);
    let blas = arena.alloc(&expected(&mesh, 0));
    let mut instances = Vec::new();
    for (k, x) in [0.0f32, 50.0].iter().enumerate() {
        instances.extend(floats(&[
            1.0, 0.0, 0.0, *x, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0,
        ]));
        instances.extend(le(&[(7 + k as u32) | 0xFF00_0000, 3 | 0x0400_0000]));
        instances.extend(quads(&[blas]));
    }
    let list = arena.alloc(&instances);
    let destination = arena.zeroed(1 << 16);
    let desc = batch(
        &arena,
        &Batch {
            destination,
            inputs: inputs(0, 0, 2, 0, list),
            source: 0,
            postbuild: vec![],
        },
    );
    assert!(build_batched(&arena, 1, desc).is_empty());
    let header = header_at(&arena, destination);
    assert_eq!(header.flags, 6);
    assert_eq!(header.elements_count, 2);
    let structure = arena.read(destination, header.size()).unwrap();
    let word = |at: usize| u32::from_le_bytes(structure[at..at + 4].try_into().unwrap());
    let record = header.elements as usize + 128;
    assert_eq!(word(record + 48), 8 | 0xFF00_0000);
    assert_eq!(word(record + 52), 3 | 0x0400_0000);
    assert_eq!(word(record + 60), (blas >> 8) as u32);
    assert!(header.bounds[3] > 50.0);
}

#[test]
fn clone_and_compact_copy_the_bytes_the_header_names() {
    let arena = Arena::new();
    let mesh = grid(40, 0.0);
    let structure = expected(&mesh, flags::MINIMIZE_MEMORY);
    let source = arena.alloc(&structure);
    let destination = arena.zeroed(1 << 16);
    for mode in [0, 1] {
        copy(&arena, destination, source, mode).unwrap();
        assert_eq!(arena.read(destination, structure.len()).unwrap(), structure);
    }
    for mode in [2, 3, 4] {
        assert!(matches!(
            copy(&arena, destination, source, mode),
            Err(Failure::Unsupported(_))
        ));
    }
}

#[test]
fn what_is_not_measured_fails_instead_of_guessing() {
    let arena = Arena::new();
    let mut aabbs = le(&[1, 0]);
    aabbs.extend(vec![0; 48]);
    let geometry = arena.alloc(&aabbs);
    let destination = arena.zeroed(1 << 12);
    let procedural = batch(
        &arena,
        &Batch {
            destination,
            inputs: inputs(1, 0, 1, 0, geometry),
            source: 0,
            postbuild: vec![],
        },
    );
    let failures = build_batched(&arena, 1, procedural);
    assert!(matches!(failures[..], [Failure::Unsupported(_)]));

    let mesh = grid(10, 0.0);
    let triangles = arena.alloc(&geometry_desc(&arena, &mesh, None));
    let statistics = batch(
        &arena,
        &Batch {
            destination,
            inputs: inputs(1, 0, 1, 0, triangles),
            source: 0,
            postbuild: vec![(0x10, 0)],
        },
    );
    assert!(matches!(
        build_batched(&arena, 1, statistics)[..],
        [Failure::Unsupported(_)]
    ));
    assert!(arena.read(destination, 64).unwrap().iter().all(|b| *b == 0));
}

#[test]
fn an_unreadable_address_is_a_failure_not_a_crash() {
    let arena = Arena::new();
    let desc = batch(
        &arena,
        &Batch {
            destination: 0x10,
            inputs: inputs(1, 0, 1, 0, 0x20),
            source: 0,
            postbuild: vec![],
        },
    );
    assert!(matches!(
        build_batched(&arena, 1, desc)[..],
        [Failure::Unreadable(_)]
    ));
}

#[cfg(windows)]
#[test]
fn the_seven_imports_are_provided_and_rehydration_says_no() {
    let shims = shims();
    assert_eq!(shims.len(), 7);
    assert!(
        shims
            .iter()
            .all(|s| s.dll == "XboxBvhBuilder.dll" && s.address != 0)
    );
    assert_eq!(host::rehydrate_batched_2x(1, 2, 1, 0x1000, 0), E_NOTIMPL);
    assert_eq!(host::rehydrate_batched_2x(1, 2, 0, 0, 0), 0);
    let this = 0xABCD0;
    assert_eq!(host::construct(this, 5, 0), this);
    host::set_64_bit_morton(this, 1);
    host::destruct(this);
}
