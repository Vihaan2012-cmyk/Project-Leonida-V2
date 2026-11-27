use crate::cpu::union;
use crate::flags::MINIMIZE_MEMORY;
use crate::input::{BlasInfo, Geometry, Instance, transform_point};
use crate::layout::{
    BLAS_FLAGS, EMPTY_BOX, ElementKind, HEADER_BYTES, Header, INVALID, NodeKind, TLAS_FLAGS,
    layout, max_nodes, pointer,
};
use crate::{BuildError, NodeBuilder, NodeOutput, NodeParams, Result};

/// The leaves of a build: each element's record words (16 for a triangle, 32 for an instance) and box, in
/// input order. An inactive element keeps its record and has an empty box.
#[derive(Debug, Clone, PartialEq)]
pub struct ElementSet {
    pub kind: ElementKind,
    pub records: Vec<u32>,
    pub boxes: Vec<[f32; 6]>,
}

impl ElementSet {
    pub fn len(&self) -> usize {
        self.boxes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.boxes.is_empty()
    }
}

fn triangle_box(vertices: &[[f32; 3]; 3]) -> [f32; 6] {
    if vertices.iter().flatten().any(|v| !v.is_finite()) {
        return EMPTY_BOX;
    }
    let mut b = [
        f32::INFINITY,
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    for v in vertices {
        for a in 0..3 {
            b[a] = b[a].min(v[a]);
            b[3 + a] = b[3 + a].max(v[a]);
        }
    }
    b
}

/// Leaves for triangles: vertex slots 0 to 2, the geometry index and flags, the primitive index, and the
/// barycentric remap byte 0x09 that makes the node's two barycentrics D3D12's (weights of vertices 1 and 2).
pub fn triangle_elements(geometries: &[Geometry]) -> ElementSet {
    let mut records = Vec::new();
    let mut boxes = Vec::new();
    for (g, geometry) in geometries.iter().enumerate() {
        for (p, triangle) in geometry.triangles.iter().enumerate() {
            let mut record = [0u32; 16];
            for (k, vertex) in triangle.vertices.iter().enumerate() {
                for a in 0..3 {
                    record[3 * k + a] = vertex[a].to_bits();
                }
            }
            record[12] = ((g as u32) << 8) | (geometry.flags & 0xFF);
            record[13] = p as u32;
            record[15] = 0x09;
            records.extend_from_slice(&record);
            boxes.push(triangle_box(&triangle.vertices));
        }
    }
    ElementSet {
        kind: ElementKind::Triangle,
        records,
        boxes,
    }
}

fn inverse(m: &[f32; 12]) -> Option<[f32; 12]> {
    let a = |r: usize, c: usize| f64::from(m[4 * r + c]);
    let cof = |r0: usize, r1: usize, c0: usize, c1: usize| {
        a(r0, c0) * a(r1, c1) - a(r0, c1) * a(r1, c0)
    };
    let c = [
        [cof(1, 2, 1, 2), -cof(1, 2, 0, 2), cof(1, 2, 0, 1)],
        [-cof(0, 2, 1, 2), cof(0, 2, 0, 2), -cof(0, 2, 0, 1)],
        [cof(0, 1, 1, 2), -cof(0, 1, 0, 2), cof(0, 1, 0, 1)],
    ];
    let det = a(0, 0) * c[0][0] + a(0, 1) * c[0][1] + a(0, 2) * c[0][2];
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    // The adjugate: the inverse's entry (r, c) is the cofactor of (c, r) over the determinant.
    let r = |i: usize, j: usize| c[j][i] / det;
    let t = [a(0, 3), a(1, 3), a(2, 3)];
    let mut out = [0f32; 12];
    for i in 0..3 {
        for j in 0..3 {
            out[4 * i + j] = r(i, j) as f32;
        }
        out[4 * i + 3] = -(r(i, 0) * t[0] + r(i, 1) * t[1] + r(i, 2) * t[2]) as f32;
    }
    out.iter().all(|v| v.is_finite()).then_some(out)
}

fn rotation_is_identity(m: &[f32; 12]) -> bool {
    (0..3).all(|r| (0..3).all(|c| m[4 * r + c] == if r == c { 1.0 } else { 0.0 }))
}

fn transformed_box(m: &[f32; 12], b: &[f32; 6]) -> [f32; 6] {
    let mut out = EMPTY_BOX;
    for corner in 0..8 {
        let p = [
            b[if corner & 1 != 0 { 3 } else { 0 }],
            b[if corner & 2 != 0 { 4 } else { 1 }],
            b[if corner & 4 != 0 { 5 } else { 2 }],
        ];
        let q = transform_point(m, p);
        out = union(&out, &[q[0], q[1], q[2], q[0], q[1], q[2]]);
    }
    out
}

/// Instance records: the object-to-world transform, ID and mask, hit group contribution and flags, the BLAS's
/// root pointer (bit 31 set when the transform's rotation is the identity) and address in 256-byte units, the
/// world-to-object transform, the instance index and, once built, the parent. An instance with no mask, no
/// BLAS or a singular transform is inactive.
pub fn instance_elements(
    instances: &[Instance],
    lookup: &dyn Fn(u64) -> Option<BlasInfo>,
) -> ElementSet {
    let mut records = Vec::new();
    let mut boxes = Vec::new();
    for (index, instance) in instances.iter().enumerate() {
        let mut record = [0u32; 32];
        let info = lookup(instance.blas);
        let inverse = inverse(&instance.transform);
        for (k, value) in instance.transform.iter().enumerate() {
            record[k] = value.to_bits();
        }
        record[12] = (instance.id & 0xFF_FFFF) | ((instance.mask & 0xFF) << 24);
        record[13] = (instance.contribution & 0xFF_FFFF) | ((instance.flags & 0xFF) << 24);
        record[14] = INVALID;
        let mut b = EMPTY_BOX;
        if let (Some(info), Some(inverse)) = (info, inverse) {
            record[14] = info.root
                | if rotation_is_identity(&instance.transform) {
                    0x8000_0000
                } else {
                    0
                };
            record[15] = (instance.blas >> 8) as u32;
            for (k, value) in inverse.iter().enumerate() {
                record[16 + k] = value.to_bits();
            }
            if instance.mask & 0xFF != 0 && info.bounds[0] <= info.bounds[3] {
                b = transformed_box(&instance.transform, &info.bounds);
                if b.iter().any(|v| v.is_nan()) {
                    b = EMPTY_BOX;
                }
            }
        }
        record[28] = index as u32;
        records.extend_from_slice(&record);
        boxes.push(b);
    }
    ElementSet {
        kind: ElementKind::Instance,
        records,
        boxes,
    }
}

fn bounds_of(boxes: &[[f32; 6]]) -> [f32; 6] {
    boxes.iter().fold(EMPTY_BOX, |a, b| union(&a, b))
}

pub(crate) fn node_params(
    count: usize,
    kind: ElementKind,
    node: NodeKind,
) -> Result<NodeParams> {
    let l = layout(count, kind, node, 0)?;
    Ok(NodeParams {
        element_base: (l.elements / 64) as u32,
        element_stride: kind.units() as u32,
        element_type: kind.pointer_type(),
        node_base: (l.nodes / 64) as u32,
        node_units: node.units() as u32,
        node_type: node.pointer_type(),
        node_table_base: l.node_table_base as u32,
    })
}

fn put(bytes: &mut [u8], at: usize, words: &[u32]) {
    for (i, word) in words.iter().enumerate() {
        bytes[at + 4 * i..at + 4 * i + 4].copy_from_slice(&word.to_le_bytes());
    }
}

/// Builds a structure over the elements with the given node type and tree builder.
pub fn build_elements(
    set: &ElementSet,
    node: NodeKind,
    builder: &mut dyn NodeBuilder,
) -> Result<Vec<u8>> {
    let count = set.len();
    let params = node_params(count, set.kind, node)?;
    let output = match count {
        0 | 1 => {
            let mut parents = vec![0u32; params.node_table_base as usize];
            if count == 1 {
                parents[0] = INVALID;
            }
            NodeOutput {
                node_count: 0,
                nodes: Vec::new(),
                parents,
            }
        }
        _ => builder.build(&set.boxes, &params)?,
    };
    let l = layout(count, set.kind, node, output.node_count as usize)?;
    if output.nodes.len() != output.node_count as usize * node.words()
        || output.parents.len() != l.node_table_base + output.node_count as usize * node.units()
    {
        return Err(BuildError::Malformed("a node builder returned the wrong sizes"));
    }
    let mut bytes = vec![0u8; l.total];
    let unit_words = set.kind.words();
    put(&mut bytes, l.elements, &set.records);
    if set.kind == ElementKind::Instance {
        for i in 0..count {
            let parent = output.parents[i * set.kind.units()];
            put(&mut bytes, l.elements + 4 * unit_words * i + 4 * 29, &[parent]);
        }
    }
    put(&mut bytes, l.nodes, &output.nodes);
    put(&mut bytes, l.parents, &output.parents);
    let root = match count {
        0 => INVALID,
        1 => pointer(l.elements / 64, set.kind.pointer_type()),
        _ => pointer(l.nodes / 64, node.pointer_type()),
    };
    let header = Header {
        flags: match set.kind {
            ElementKind::Triangle => BLAS_FLAGS,
            ElementKind::Instance => TLAS_FLAGS,
        },
        total_nodes: (l.total / 64 - 1) as u32,
        elements_count: count as u32,
        bounds: bounds_of(&set.boxes),
        nodes: l.nodes as u32,
        root,
        elements: l.elements as u32,
        parents: l.parents as u32,
    };
    header.write(&mut bytes[..HEADER_BYTES]);
    Ok(bytes)
}

fn blas_node(flags: u32) -> NodeKind {
    if flags & MINIMIZE_MEMORY != 0 {
        NodeKind::Box16
    } else {
        NodeKind::Box32
    }
}

/// A bottom-level structure over triangle geometries; BOX16 nodes with `MINIMIZE_MEMORY`, else BOX32.
pub fn build_blas(
    geometries: &[Geometry],
    flags: u32,
    builder: &mut dyn NodeBuilder,
) -> Result<Vec<u8>> {
    build_elements(&triangle_elements(geometries), blas_node(flags), builder)
}

/// A top-level structure over instances; its nodes are always BOX32, since world-space boxes need the range.
pub fn build_tlas(
    instances: &[Instance],
    lookup: &dyn Fn(u64) -> Option<BlasInfo>,
    builder: &mut dyn NodeBuilder,
) -> Result<Vec<u8>> {
    build_elements(&instance_elements(instances, lookup), NodeKind::Box32, builder)
}

/// The most a build of `count` elements can write: what a prebuild-info query answers.
pub fn prebuild_size(count: usize, bottom_level: bool, flags: u32) -> Result<usize> {
    let (kind, node) = if bottom_level {
        (ElementKind::Triangle, blas_node(flags))
    } else {
        (ElementKind::Instance, NodeKind::Box32)
    };
    Ok(layout(count, kind, node, max_nodes(count))?.total)
}

/// A structure's bytes without the room the layout left for more nodes than the build wrote. A build already
/// writes only its nodes, so this trims to the size the header names; the records are position independent.
pub fn compact(structure: &[u8]) -> Result<Vec<u8>> {
    let header = Header::read(structure)?;
    Ok(structure[..header.size()].to_vec())
}

/// A bottom-level structure's root pointer and bounds, as a TLAS build needs them.
pub fn blas_info(structure: &[u8]) -> Result<BlasInfo> {
    let header = Header::read(structure)?;
    if !header.is_bottom_level() {
        return Err(BuildError::Malformed("not a bottom-level structure"));
    }
    Ok(BlasInfo {
        root: header.root,
        bounds: header.bounds,
    })
}

pub use crate::refit::{refit_blas, refit_tlas};
