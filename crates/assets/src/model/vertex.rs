use super::ModelError;
use super::pages::Pages;
use super::types::{Geometry, SkinData, SkinLayout};

const MAX_VERTICES: u32 = 1_000_000;
const SLOTS: usize = 23;
const FVF_SIZE: usize = 0x13C;
const WEIGHT_SEMANTICS: [usize; 2] = [16, 17];
const INDEX_SEMANTICS: [usize; 2] = [20, 21];
const NORMAL_SLOT: usize = 2;
const COLOR_SLOT: usize = 1;
const UV_SLOTS: [usize; 4] = [3, 4, 14, 15];

/// The vertex block header the graphics buffer starts with: 23 words, each a stream tag (low byte)
/// and half the stream offset (high 24 bits). The tag's bits 5 to 7 times four are the stride.
struct Block<'a> {
    data: &'a [u8],
    slots: [Option<(u8, usize)>; SLOTS],
}

impl<'a> Block<'a> {
    fn parse(data: &'a [u8]) -> Result<Self, ModelError> {
        if data.len() < SLOTS * 4 {
            return Err(ModelError::Bounds {
                what: "vertex block header",
            });
        }
        let mut slots = [None; SLOTS];
        for (i, slot) in slots.iter_mut().enumerate() {
            let w = u32::from_le_bytes(data[4 * i..4 * i + 4].try_into().expect("4 bytes"));
            if w != 0 {
                *slot = Some(((w & 0xFF) as u8, (w >> 8) as usize * 2));
            }
        }
        Ok(Self { data, slots })
    }

    /// The bytes of slot `i` for `n` vertices, with its tag.
    fn stream(&self, i: usize, n: usize) -> Option<(u8, &'a [u8])> {
        let (tag, offset) = self.slots[i]?;
        let stride = (tag as usize >> 5) * 4;
        let end = offset.checked_add(stride.checked_mul(n)?)?;
        Some((tag, self.data.get(offset..end)?))
    }

    fn f32_at(&self, at: usize) -> f32 {
        self.data
            .get(at..at + 4)
            .map_or(0.0, |b| f32::from_le_bytes(b.try_into().expect("4 bytes")))
    }
}

fn le32(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes(b[4 * i..4 * i + 4].try_into().expect("4 bytes"))
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 1e-6 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        [0.0, 0.0, 1.0]
    }
}

fn octahedral(x: f32, y: f32) -> [f32; 3] {
    let z = 1.0 - x.abs() - y.abs();
    let (x, y) = if z < 0.0 {
        let sx = if x >= 0.0 { 1.0 } else { -1.0 };
        let sy = if y >= 0.0 { 1.0 } else { -1.0 };
        ((1.0 - y.abs()) * sx, (1.0 - x.abs()) * sy)
    } else {
        (x, y)
    };
    normalize([x, y, z])
}

/// Tag 0x21: bit 31 selects a 10-bit octahedral pair (bits 0-9, 10-19), otherwise bytes 0 and 1.
fn normal_0x21(w: u32) -> [f32; 3] {
    if w >> 31 != 0 {
        octahedral(
            (w & 1023) as f32 * 2.0 / 1023.0 - 1.0,
            ((w >> 10) & 1023) as f32 * 2.0 / 1023.0 - 1.0,
        )
    } else {
        octahedral(
            (w & 255) as f32 / 127.5 - 1.0,
            ((w >> 8) & 255) as f32 / 127.5 - 1.0,
        )
    }
}

/// Tag 0x27: three 10-bit unorm components remapped to [-1, 1].
fn normal_0x27(w: u32) -> [f32; 3] {
    let c = |s: u32| ((w >> s) & 1023) as f32 / 1023.0 * 2.0 - 1.0;
    normalize([c(0), c(10), c(20)])
}

pub(crate) fn read_geometry(
    p: &Pages<'_>,
    geometry: usize,
    bone_count: usize,
    warnings: &mut Vec<String>,
) -> Result<Geometry, ModelError> {
    let vb = p.pointer(geometry + 8, 80, "vertex buffer header")?;
    let fvf = p.pointer(vb + 64, FVF_SIZE, "vertex format")?;
    let flags = p.u32(fvf + 0x138, "vertex flags")?;
    let vertices = flags >> 10;
    if flags & 1 == 0 || vertices == 0 || vertices > MAX_VERTICES {
        return Err(ModelError::Invalid(format!("vertex flags {flags:#x}")));
    }
    let n = vertices as usize;
    let length =
        p.u32(vb + 8, "vertex buffer length")? as usize * p.u16(vb + 12, "vertex stride")? as usize;
    let at = p.graphics_pointer(vb + 24, length, "vertex data")?;
    let data = p.slice_graphics(at, length)?;
    let block = Block::parse(data)?;

    let positions = read_positions(p, fvf, data, n)?;
    let (indices, constant_bone) = read_indices(p, geometry, n)?;
    let mut out = Geometry {
        positions,
        indices,
        ..Geometry::default()
    };

    if let Some((tag, bytes)) = block.stream(NORMAL_SLOT, n) {
        match tag {
            0x63 => {
                out.normals = (0..n)
                    .map(|i| {
                        let f = |k: usize| {
                            f32::from_le_bytes(
                                bytes[12 * i + 4 * k..12 * i + 4 * k + 4]
                                    .try_into()
                                    .expect("4"),
                            )
                        };
                        normalize([f(0), f(1), f(2)])
                    })
                    .collect()
            }
            0x21 => out.normals = (0..n).map(|i| normal_0x21(le32(bytes, i))).collect(),
            0x27 => out.normals = (0..n).map(|i| normal_0x27(le32(bytes, i))).collect(),
            other => warnings.push(format!("normal stream tag {other:#x} not decoded")),
        }
    }
    if let Some((0x26, bytes)) = block.stream(COLOR_SLOT, n) {
        out.colors = bytes
            .chunks_exact(4)
            .map(|c| [c[0], c[1], c[2], c[3]])
            .collect();
    }
    let uv_scale = block.f32_at(0x7C);
    for slot in UV_SLOTS {
        let Some((tag, bytes)) = block.stream(slot, n) else {
            continue;
        };
        match tag {
            0x28 if has_wide_header(&block) => {
                let scale = if uv_scale.is_finite() && uv_scale != 0.0 {
                    uv_scale
                } else {
                    4.0
                };
                let snorm = |v: i16| (f32::from(v) / 32767.0).max(-1.0) * scale;
                out.uv_sets.push(
                    bytes
                        .chunks_exact(4)
                        .map(|c| {
                            [
                                snorm(i16::from_le_bytes([c[0], c[1]])),
                                snorm(i16::from_le_bytes([c[2], c[3]])),
                            ]
                        })
                        .collect(),
                );
            }
            0x42 => out.uv_sets.push(
                bytes
                    .chunks_exact(8)
                    .map(|c| {
                        [
                            f32::from_le_bytes(c[0..4].try_into().expect("4")),
                            f32::from_le_bytes(c[4..8].try_into().expect("4")),
                        ]
                    })
                    .collect(),
            ),
            other => warnings.push(format!(
                "texture coordinate slot {slot} tag {other:#x} not decoded"
            )),
        }
    }

    out.skin = read_skin(p, fvf, data, n, constant_bone, bone_count)?;
    orient_winding(&mut out);
    Ok(out)
}

fn has_wide_header(block: &Block<'_>) -> bool {
    block.slots[0].is_some_and(|(tag, offset)| tag == 0x42 && offset >= 0x80)
}

fn read_positions(
    p: &Pages<'_>,
    fvf: usize,
    data: &[u8],
    n: usize,
) -> Result<Vec<[f32; 3]>, ModelError> {
    let offset = p.u32(fvf, "position offset")? as usize;
    let size = p.u8(fvf + 0xD0, "position size")?;
    let format = p.u8(fvf + 0x104, "position format")?;
    let need = size as usize * n;
    if offset < 96 || offset > data.len() || need > data.len() - offset {
        return Err(ModelError::Bounds {
            what: "position stream",
        });
    }
    let stream = &data[offset..offset + need];
    match (size, format) {
        (12, 6) => {
            if data[0] & 127 != 0x63 {
                return Err(ModelError::Invalid("float position block tag".into()));
            }
            Ok(stream
                .chunks_exact(12)
                .map(|c| {
                    let f =
                        |k: usize| f32::from_le_bytes(c[4 * k..4 * k + 4].try_into().expect("4"));
                    [f(0), f(1), f(2)]
                })
                .collect())
        }
        (8, 16) => {
            if offset < 128 || data[0] & 127 != 0x42 {
                return Err(ModelError::Invalid("quantized position block tag".into()));
            }
            let f = |at: usize| f32::from_le_bytes(data[at..at + 4].try_into().expect("4"));
            let bias = [f(96), f(100), f(104)];
            let axis = u32::from_le_bytes(data[108..112].try_into().expect("4"));
            let scale = [f(112), f(116), f(120)];
            if axis > 2 || scale.iter().any(|s| !s.is_finite() || *s < 0.0) {
                return Err(ModelError::Invalid(
                    "quantized position axis or scale".into(),
                ));
            }
            Ok(stream
                .chunks_exact(8)
                .map(|c| {
                    let q = u64::from_le_bytes(c.try_into().expect("8"));
                    let a = bias[0] + scale[0] * ((q >> 8) & 0xFF_FFFF) as f32;
                    let b = bias[1] + scale[1] * ((((q >> 32) & 0xFFFF) << 8) | (q & 0xFF)) as f32;
                    let c = bias[2] + scale[2] * (q >> 48) as f32;
                    match axis {
                        0 => [c, b, a],
                        1 => [b, c, a],
                        _ => [a, b, c],
                    }
                })
                .collect())
        }
        _ => Err(ModelError::Unsupported(format!(
            "position encoding {size}/{format}"
        ))),
    }
}

/// The clustered index buffer: cluster records (16 bytes), packed triangles (three 7-bit corners and
/// the cluster number in the top 11 bits), then the vertex reference list. Returns the triangle
/// corners and, for palette-bound constant geometries, the bone palette entry.
fn read_indices(
    p: &Pages<'_>,
    geometry: usize,
    vertices: usize,
) -> Result<(Vec<u32>, Option<u16>), ModelError> {
    let descriptor = p.pointer(geometry + 64, 16, "index descriptor")?;
    let buffer = p.pointer(descriptor, 64, "index buffer")?;
    if p.u16(buffer + 12, "index stride")? != 4 {
        return Err(ModelError::Invalid("index buffer stride is not 4".into()));
    }
    let length = p.u32(buffer + 8, "index buffer length")? as usize * 4;
    let at = p.graphics_pointer(buffer + 24, length, "index data")?;
    let data = p.slice_graphics(at, length)?;
    if data.len() < 12 {
        return Err(ModelError::Bounds { what: "index data" });
    }
    let count_flags = le32(data, 0);
    let palette = count_flags & 0x4000_0000 != 0;
    let clusters = (count_flags & if palette { 0xFFFF } else { 0x7FFF_FFFF }) as usize;
    let triangles = le32(data, 1) as usize;
    let refs = le32(data, 2) as usize;
    if clusters == 0
        || clusters > 2048
        || triangles == 0
        || triangles > 65535
        || refs == 0
        || refs > 65535
    {
        return Err(ModelError::Invalid(format!(
            "index counts {clusters}/{triangles}/{refs}"
        )));
    }
    let ref_size = if count_flags & 0x8000_0000 != 0 { 2 } else { 4 };
    let tri_at = 12 + clusters * 16;
    let refs_at = tri_at + triangles * 4;
    if refs_at + refs * ref_size > data.len() {
        return Err(ModelError::Bounds {
            what: "index buffer body",
        });
    }
    let reference = |i: usize| -> u32 {
        let b = &data[refs_at + i * ref_size..];
        if ref_size == 2 {
            u32::from(u16::from_le_bytes([b[0], b[1]]))
        } else {
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        }
    };
    if (0..refs).any(|i| reference(i) as usize >= vertices) {
        return Err(ModelError::Invalid(
            "vertex reference past the vertex count".into(),
        ));
    }
    let mut out = Vec::with_capacity(triangles * 3);
    let (mut seen_refs, mut seen_tris) = (0usize, 0usize);
    for c in 0..clusters {
        let packed = le32(data, 3 + 4 * c);
        let starts = le32(data, 4 + 4 * c);
        let cluster_vertices = (packed & 127) as usize + 1;
        let cluster_tris = ((packed >> 7) & 255) as usize + 1;
        if (starts & 0xFFFF) as usize != seen_refs & 0xFFFF
            || (starts >> 16) as usize != seen_tris & 0xFFFF
        {
            return Err(ModelError::Invalid(format!(
                "cluster {c} starts do not follow the previous one"
            )));
        }
        if seen_tris + cluster_tris > triangles || seen_refs + cluster_vertices > refs {
            return Err(ModelError::Invalid(format!(
                "cluster {c} runs past the buffer"
            )));
        }
        for t in seen_tris..seen_tris + cluster_tris {
            let word = le32(data, (tri_at / 4) + t);
            if (word >> 21) as usize != c & 0x7FF {
                return Err(ModelError::Invalid("triangle cluster tag".into()));
            }
            for corner in 0..3 {
                let local = ((word >> (7 * corner)) & 127) as usize;
                if local >= cluster_vertices {
                    return Err(ModelError::Invalid(
                        "triangle corner past its cluster".into(),
                    ));
                }
                out.push(reference(seen_refs + local));
            }
        }
        seen_refs += cluster_vertices;
        seen_tris += cluster_tris;
    }
    if seen_refs != refs || seen_tris != triangles {
        return Err(ModelError::Invalid(
            "clusters do not cover the index buffer".into(),
        ));
    }
    let constant = (palette && count_flags & 0x2000_0000 != 0)
        .then_some((((count_flags >> 16) & 0x1FFF) / 2) as u16);
    Ok((out, constant))
}

/// Skin streams by FVF semantic: 16 and 17 four unorm8 weights each, 20 and 21 the bone indices.
fn read_skin(
    p: &Pages<'_>,
    fvf: usize,
    data: &[u8],
    n: usize,
    constant_bone: Option<u16>,
    bone_count: usize,
) -> Result<Option<SkinData>, ModelError> {
    struct Slot {
        size: u8,
        format: u8,
        offset: usize,
    }
    let slot = |i: usize| -> Result<Option<Slot>, ModelError> {
        let size = p.u8(fvf + 0xD0 + i, "semantic size")?;
        if size == 0 {
            return Ok(None);
        }
        let offset = p.u32(fvf + 4 * i, "semantic offset")? as usize;
        if offset > data.len() || size as usize * n > data.len() - offset {
            return Err(ModelError::Bounds {
                what: "skin stream",
            });
        }
        Ok(Some(Slot {
            size,
            format: p.u8(fvf + 0x104 + i, "semantic format")?,
            offset,
        }))
    };
    let mut weights = Vec::new();
    for s in WEIGHT_SEMANTICS {
        if let Some(slot) = slot(s)? {
            if (slot.size, slot.format) != (4, 28) {
                return Err(ModelError::Unsupported(format!(
                    "weight stream {}/{}",
                    slot.size, slot.format
                )));
            }
            weights.push(slot);
        }
    }
    let mut indices = Vec::new();
    for s in INDEX_SEMANTICS {
        if let Some(slot) = slot(s)? {
            indices.push(slot);
        }
    }
    if indices.is_empty() {
        let Some(bone) = constant_bone else {
            return Ok(None);
        };
        if bone_count == 0 {
            return Ok(None);
        }
        if bone as usize >= bone_count {
            return Err(ModelError::Invalid(format!(
                "constant bone {bone} of {bone_count} bones"
            )));
        }
        let mut joints = vec![[0u16; 8]; n];
        let mut w = vec![[0u8; 8]; n];
        for i in 0..n {
            joints[i][0] = bone;
            w[i][0] = 255;
        }
        return Ok(Some(SkinData {
            layout: SkinLayout::Constant,
            joints,
            weights: w,
        }));
    }
    let first = (indices[0].size, indices[0].format);
    if indices.iter().any(|s| (s.size, s.format) != first) {
        return Err(ModelError::Unsupported("mixed bone index streams".into()));
    }
    let layout = match first {
        (4, 30) => SkinLayout::Bytes,
        (16, 3) => SkinLayout::Packed,
        (8, 12) => SkinLayout::Words,
        (1, 62) | (2, 57) => SkinLayout::Rigid,
        other => {
            return Err(ModelError::Unsupported(format!(
                "bone index stream {}/{}",
                other.0, other.1
            )));
        }
    };
    let mut joints = vec![[0u16; 8]; n];
    let mut w = vec![[0u8; 8]; n];
    if layout == SkinLayout::Rigid {
        if !weights.is_empty() {
            return Err(ModelError::Invalid(
                "rigid index stream with weights".into(),
            ));
        }
        let s = &indices[0];
        for v in 0..n {
            let b = &data[s.offset..];
            let bone = if s.size == 1 {
                u16::from(b[v])
            } else {
                u16::from_le_bytes([b[2 * v], b[2 * v + 1]])
            };
            joints[v][0] = bone;
            w[v][0] = 255;
        }
    } else {
        if weights.len() != indices.len() {
            return Err(ModelError::Invalid(
                "weight and index stream counts differ".into(),
            ));
        }
        for (set, (ws, is)) in weights.iter().zip(&indices).enumerate() {
            for v in 0..n {
                let wb = &data[ws.offset + 4 * v..ws.offset + 4 * v + 4];
                let base = is.offset;
                for c in 0..4 {
                    let bone = match layout {
                        SkinLayout::Bytes => u16::from(data[base + 4 * v + c]),
                        SkinLayout::Words => {
                            let at = base + 8 * v + 2 * c;
                            u16::from_le_bytes([data[at], data[at + 1]])
                        }
                        _ => {
                            let at = base + 16 * v + 4 * c;
                            (u32::from_le_bytes(data[at..at + 4].try_into().expect("4")) & 0xFFF)
                                as u16
                        }
                    };
                    joints[v][4 * set + c] = bone;
                    w[v][4 * set + c] = wb[c];
                }
            }
        }
    }
    for v in 0..n {
        let mut order: Vec<usize> = (0..8).filter(|&k| w[v][k] != 0).collect();
        order.sort_by_key(|&k| std::cmp::Reverse(w[v][k]));
        let mut sj = [0u16; 8];
        let mut sw = [0u8; 8];
        for (dst, &k) in order.iter().enumerate() {
            if joints[v][k] as usize >= bone_count {
                return Err(ModelError::Invalid(format!(
                    "vertex {v} weights bone {} of {bone_count} bones",
                    joints[v][k]
                )));
            }
            sj[dst] = joints[v][k];
            sw[dst] = w[v][k];
        }
        joints[v] = sj;
        w[v] = sw;
    }
    Ok(Some(SkinData {
        layout,
        joints,
        weights: w,
    }))
}

/// glTF wants counter-clockwise front faces. When the stored normals say most triangles wind the
/// other way, reverse them; without normals the stored order is kept.
fn orient_winding(g: &mut Geometry) {
    if g.normals.len() != g.positions.len() {
        return;
    }
    let (mut agree, mut disagree) = (0usize, 0usize);
    for t in g.indices.chunks_exact(3) {
        let [a, b, c] = [t[0] as usize, t[1] as usize, t[2] as usize];
        let (pa, pb, pc) = (g.positions[a], g.positions[b], g.positions[c]);
        let u = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
        let v = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
        let face = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let dot: f32 = (0..3)
            .map(|k| face[k] * (g.normals[a][k] + g.normals[b][k] + g.normals[c][k]))
            .sum();
        if dot > 0.0 {
            agree += 1;
        } else if dot < 0.0 {
            disagree += 1;
        }
    }
    if disagree > agree {
        for t in g.indices.chunks_exact_mut(3) {
            t.swap(1, 2);
        }
    }
}
