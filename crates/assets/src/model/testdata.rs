//! Builds synthetic resources in the measured layout for the tests.

const SYS: u64 = 0x5000_0000;
const GFX: u64 = 0x6000_0000;

#[derive(Default)]
pub struct Arena {
    pub buf: Vec<u8>,
}

impl Arena {
    pub fn alloc(&mut self, size: usize, align: usize) -> usize {
        while !self.buf.len().is_multiple_of(align) {
            self.buf.push(0);
        }
        let at = self.buf.len();
        self.buf.resize(at + size, 0);
        at
    }

    pub fn u8(&mut self, at: usize, v: u8) {
        self.buf[at] = v;
    }

    pub fn u16(&mut self, at: usize, v: u16) {
        self.buf[at..at + 2].copy_from_slice(&v.to_le_bytes());
    }

    pub fn u32(&mut self, at: usize, v: u32) {
        self.buf[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    pub fn u64(&mut self, at: usize, v: u64) {
        self.buf[at..at + 8].copy_from_slice(&v.to_le_bytes());
    }

    pub fn f32(&mut self, at: usize, v: f32) {
        self.u32(at, v.to_bits());
    }

    pub fn sys_ptr(&mut self, at: usize, target: usize) {
        self.u64(at, SYS + target as u64);
    }

    pub fn gfx_ptr(&mut self, at: usize, target: usize) {
        self.u64(at, GFX + target as u64);
    }

    pub fn bytes(&mut self, at: usize, data: &[u8]) {
        self.buf[at..at + data.len()].copy_from_slice(data);
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum PositionForm {
    /// Quantized 8-byte positions with the given axis word.
    Quantized(u32),
    Float,
}

#[derive(Clone, Copy, PartialEq)]
pub enum SkinForm {
    None,
    /// Four 8-bit indices per weight stream, one or two streams.
    Bytes(usize),
    Packed(usize),
    Words(usize),
    Rigid8,
    Constant(u16),
}

#[derive(Clone)]
pub struct GeometrySpec {
    pub positions: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    pub form: PositionForm,
    pub skin: SkinForm,
    /// Per vertex (index, weight) lists for the skin forms with weights.
    pub influences: Vec<Vec<(u16, u8)>>,
    pub normals: Option<Vec<[f32; 3]>>,
    pub uvs: Option<Vec<[f32; 2]>>,
    pub colors: Option<Vec<[u8; 4]>>,
    pub shader: u16,
    pub short_refs: bool,
}

impl GeometrySpec {
    pub fn quad() -> Self {
        Self {
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 2.0, 0.0],
                [0.0, 2.0, 0.5],
            ],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            form: PositionForm::Quantized(2),
            skin: SkinForm::None,
            influences: Vec::new(),
            normals: None,
            uvs: None,
            colors: None,
            shader: 0,
            short_refs: false,
        }
    }
}

pub struct ModelSpec {
    pub geometries: Vec<GeometrySpec>,
    pub palette: Vec<u32>,
}

pub struct BoneSpec {
    pub name: &'static str,
    pub hash: u32,
    pub parent: Option<u16>,
    pub rotation: [f32; 4],
    pub translation: [f32; 3],
    /// World bind position, used to build the inverse bind matrix (translation only).
    pub world: [f32; 3],
}

pub struct TextureSpec {
    pub name_hash: u32,
    pub param: u32,
    pub id64: u64,
}

pub struct MaterialSpec {
    pub shader: u32,
    pub bucket: u8,
    pub textures: Vec<TextureSpec>,
}

pub struct DrawableSpec {
    pub lods: Vec<(f32, Vec<ModelSpec>)>,
    pub materials: Vec<MaterialSpec>,
    pub skeleton: Vec<BoneSpec>,
    pub name: Option<&'static str>,
}

impl DrawableSpec {
    pub fn simple(geometry: GeometrySpec) -> Self {
        Self {
            lods: vec![(
                15.0,
                vec![ModelSpec {
                    geometries: vec![geometry],
                    palette: Vec::new(),
                }],
            )],
            materials: vec![MaterialSpec {
                shader: 0xDEAD_BEEF,
                bucket: 0,
                textures: Vec::new(),
            }],
            skeleton: Vec::new(),
            name: None,
        }
    }
}

pub struct Resource {
    pub system: Vec<u8>,
    pub graphics: Vec<u8>,
}

struct Ctx {
    sys: Arena,
    gfx: Arena,
}

pub fn drawable_resource(spec: &DrawableSpec) -> Resource {
    let mut c = Ctx {
        sys: Arena::default(),
        gfx: Arena::default(),
    };
    let at = c.sys.alloc(0x110, 16);
    write_drawable(&mut c, at, spec);
    Resource {
        system: c.sys.buf,
        graphics: c.gfx.buf,
    }
}

pub fn dictionary_resource(children: &[(u32, DrawableSpec)]) -> Resource {
    let mut c = Ctx {
        sys: Arena::default(),
        gfx: Arena::default(),
    };
    let header = c.sys.alloc(0xC0, 16);
    let hashes = c.sys.alloc(children.len() * 4, 16);
    let array = c.sys.alloc(children.len() * 8, 16);
    let pages = c.sys.alloc(16, 16);
    c.sys.sys_ptr(header + 8, pages);
    c.sys.sys_ptr(header + 32, hashes);
    c.sys.u16(header + 40, children.len() as u16);
    c.sys.u16(header + 42, children.len() as u16);
    c.sys.sys_ptr(header + 48, array);
    c.sys.u16(header + 56, children.len() as u16);
    c.sys.u16(header + 58, children.len() as u16);
    for (i, (hash, spec)) in children.iter().enumerate() {
        let at = c.sys.alloc(0x110, 16);
        write_drawable(&mut c, at, spec);
        c.sys.sys_ptr(array + 8 * i, at);
        c.sys.u32(hashes + 4 * i, *hash);
    }
    Resource {
        system: c.sys.buf,
        graphics: c.gfx.buf,
    }
}

pub fn fragment_resource(drawables: &[DrawableSpec]) -> Resource {
    let mut c = Ctx {
        sys: Arena::default(),
        gfx: Arena::default(),
    };
    let header = c.sys.alloc(0xC0, 16);
    let pages = c.sys.alloc(16, 16);
    let name = c.sys.alloc(16, 16);
    c.sys.bytes(name, b"frag\0");
    let records = c.sys.alloc(0x110 * drawables.len(), 16);
    c.sys.sys_ptr(header + 8, pages);
    c.sys.sys_ptr(header + 24, records);
    c.sys.u16(header + 48, drawables.len() as u16);
    c.sys.u16(header + 50, 0);
    c.sys.u16(header + 52, 0);
    c.sys.u16(header + 54, 0);
    c.sys.sys_ptr(header + 168, name);
    for (i, spec) in drawables.iter().enumerate() {
        write_drawable(&mut c, records + 0x110 * i, spec);
    }
    Resource {
        system: c.sys.buf,
        graphics: c.gfx.buf,
    }
}

fn write_drawable(c: &mut Ctx, at: usize, spec: &DrawableSpec) {
    let group = write_shader_group(c, &spec.materials);
    c.sys.sys_ptr(at + 16, group);
    if !spec.skeleton.is_empty() {
        let skeleton = write_skeleton(c, &spec.skeleton);
        c.sys.sys_ptr(at + 24, skeleton);
    }
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for (_, models) in &spec.lods {
        for m in models {
            for g in &m.geometries {
                for p in &g.positions {
                    for k in 0..3 {
                        min[k] = min[k].min(p[k]);
                        max[k] = max[k].max(p[k]);
                    }
                }
            }
        }
    }
    c.sys.f32(at + 44, 5.0);
    for k in 0..3 {
        c.sys.f32(at + 48 + 4 * k, min[k]);
        c.sys.f32(at + 64 + 4 * k, max[k]);
    }
    for (level, (distance, models)) in spec.lods.iter().enumerate() {
        let list = write_lod(c, models);
        c.sys.sys_ptr(at + 80 + 8 * level, list);
        c.sys.f32(at + 0x70 + 4 * level, *distance);
    }
    if let Some(name) = spec.name {
        let s = c.sys.alloc(name.len() + 1, 16);
        c.sys.bytes(s, name.as_bytes());
        c.sys.sys_ptr(at + 168, s);
    }
}

fn write_lod(c: &mut Ctx, models: &[ModelSpec]) -> usize {
    let list = c.sys.alloc(16, 16);
    let array = c.sys.alloc(models.len() * 8, 16);
    c.sys.sys_ptr(list, array);
    c.sys.u16(list + 8, models.len() as u16);
    c.sys.u16(list + 10, models.len() as u16);
    for (i, m) in models.iter().enumerate() {
        let model = c.sys.alloc(64, 16);
        let n = m.geometries.len();
        let geos = c.sys.alloc(n * 8, 16);
        let mapping = c.sys.alloc(n * 2 + 2, 16);
        c.sys.sys_ptr(model + 8, geos);
        c.sys.u16(model + 16, n as u16);
        c.sys.u16(model + 18, n as u16);
        c.sys.u16(model + 54, n as u16);
        c.sys.u16(model + 20, m.palette.len() as u16);
        c.sys.sys_ptr(model + 0x20, mapping);
        if !m.palette.is_empty() {
            let pal = c.sys.alloc(m.palette.len() * 4, 16);
            for (k, h) in m.palette.iter().enumerate() {
                c.sys.u32(pal + 4 * k, *h);
            }
            c.sys.sys_ptr(model + 0x28, pal);
        }
        for (k, g) in m.geometries.iter().enumerate() {
            let geo = write_geometry(c, g);
            c.sys.sys_ptr(geos + 8 * k, geo);
            c.sys.u16(mapping + 2 * k, g.shader);
        }
        c.sys.sys_ptr(array + 8 * i, model);
    }
    list
}

fn tag_for(stride: usize, ty: u8) -> u32 {
    ((stride / 4) as u32) << 5 | u32::from(ty)
}

fn write_geometry(c: &mut Ctx, g: &GeometrySpec) -> usize {
    let n = g.positions.len();
    let wide = matches!(g.form, PositionForm::Quantized(_));
    let header = if wide { 0x80 } else { 0x60 };
    // (slot, tag, bytes)
    let mut streams: Vec<(usize, u32, Vec<u8>)> = Vec::new();
    let pos_tag = if wide { 0x42 } else { 0x63 };
    let (pos_bytes, bias, scale, axis) = encode_positions(g);
    streams.push((0, pos_tag, pos_bytes));
    if let Some(normals) = &g.normals {
        let mut b = Vec::new();
        for v in normals {
            for k in v {
                b.extend_from_slice(&k.to_le_bytes());
            }
        }
        if wide {
            let mut o = Vec::new();
            for v in normals {
                o.extend_from_slice(&octahedral_encode(*v).to_le_bytes());
            }
            streams.push((2, tag_for(4, 1), o));
        } else {
            streams.push((2, 0x63, b));
        }
    }
    if let Some(colors) = &g.colors {
        streams.push((1, tag_for(4, 6), colors.iter().flatten().copied().collect()));
    }
    if let Some(uvs) = &g.uvs {
        if wide {
            let mut b = Vec::new();
            for uv in uvs {
                for k in uv {
                    b.extend_from_slice(&((k / 4.0 * 32767.0).round() as i16).to_le_bytes());
                }
            }
            streams.push((3, tag_for(4, 8), b));
        } else {
            let mut b = Vec::new();
            for uv in uvs {
                for k in uv {
                    b.extend_from_slice(&k.to_le_bytes());
                }
            }
            streams.push((3, tag_for(8, 2), b));
        }
    }
    let mut fvf_skin: Vec<(usize, u8, u8, usize)> = Vec::new(); // semantic, size, format, stream index
    let sets = match g.skin {
        SkinForm::Bytes(s) | SkinForm::Packed(s) | SkinForm::Words(s) => s,
        _ => 0,
    };
    for set in 0..sets {
        let mut w = Vec::new();
        let mut j = Vec::new();
        for v in 0..n {
            let infl = &g.influences[v];
            for k in 0..4 {
                let (bone, weight) = infl.get(4 * set + k).copied().unwrap_or((0, 0));
                w.push(weight);
                match g.skin {
                    SkinForm::Bytes(_) => j.push(bone as u8),
                    SkinForm::Words(_) => j.extend_from_slice(&bone.to_le_bytes()),
                    _ => {
                        j.extend_from_slice(&(u32::from(bone) | 0xABC0_0000).to_le_bytes());
                    }
                }
            }
        }
        let (jtag, jsize, jfmt) = match g.skin {
            SkinForm::Bytes(_) => (0x25u32, 4u8, 30u8),
            SkinForm::Words(_) => (0x4B, 8, 12),
            _ => (0x8C, 16, 3),
        };
        streams.push((6 + set, tag_for(4, 6), w));
        fvf_skin.push((16 + set, 4, 28, streams.len() - 1));
        streams.push((8 + set, jtag, j));
        fvf_skin.push((20 + set, jsize, jfmt, streams.len() - 1));
    }
    if g.skin == SkinForm::Rigid8 {
        let j: Vec<u8> = (0..n).map(|v| g.influences[v][0].0 as u8).collect();
        streams.push((8, tag_for(4, 5), j));
        fvf_skin.push((20, 1, 62, streams.len() - 1));
    }

    let mut data = vec![0u8; header];
    let mut offsets = Vec::new();
    for (_, _, bytes) in &streams {
        while data.len() % 16 != 0 {
            data.push(0);
        }
        offsets.push(data.len());
        data.extend_from_slice(bytes);
    }
    for (i, (slot, tag, _)) in streams.iter().enumerate() {
        let w = (offsets[i] as u32 / 2) << 8 | tag;
        data[4 * slot..4 * slot + 4].copy_from_slice(&w.to_le_bytes());
    }
    if wide {
        for k in 0..3 {
            data[96 + 4 * k..100 + 4 * k].copy_from_slice(&bias[k].to_le_bytes());
            data[112 + 4 * k..116 + 4 * k].copy_from_slice(&scale[k].to_le_bytes());
        }
        data[108..112].copy_from_slice(&axis.to_le_bytes());
        data[0x7C..0x80].copy_from_slice(&4.0f32.to_le_bytes());
    }
    while data.len() % 16 != 0 {
        data.push(0);
    }
    let vb_gfx = c.gfx.alloc(data.len(), 16);
    c.gfx.bytes(vb_gfx, &data);

    let fvf = c.sys.alloc(0x13C, 16);
    c.sys.u32(fvf, offsets[0] as u32);
    c.sys.u8(fvf + 0xD0, if wide { 8 } else { 12 });
    c.sys.u8(fvf + 0x104, if wide { 16 } else { 6 });
    for (semantic, size, format, stream) in fvf_skin {
        c.sys.u32(fvf + 4 * semantic, offsets[stream] as u32);
        c.sys.u8(fvf + 0xD0 + semantic, size);
        c.sys.u8(fvf + 0x104 + semantic, format);
    }
    c.sys.u32(fvf + 0x138, (n as u32) << 10 | 1);
    let vb = c.sys.alloc(80, 16);
    c.sys.u32(vb + 8, (data.len() / 4) as u32);
    c.sys.u16(vb + 12, 4);
    c.sys.gfx_ptr(vb + 24, vb_gfx);
    c.sys.sys_ptr(vb + 64, fvf);

    let ib_data = encode_indices(g);
    let ib_gfx = c.gfx.alloc(ib_data.len(), 16);
    c.gfx.bytes(ib_gfx, &ib_data);
    let ib = c.sys.alloc(64, 16);
    c.sys.u32(ib + 8, (ib_data.len() / 4) as u32);
    c.sys.u16(ib + 12, 4);
    c.sys.gfx_ptr(ib + 24, ib_gfx);
    let descriptor = c.sys.alloc(16, 16);
    c.sys.sys_ptr(descriptor, ib);
    let geo = c.sys.alloc(160, 16);
    c.sys.sys_ptr(geo + 8, vb);
    c.sys.sys_ptr(geo + 64, descriptor);
    geo
}

fn octahedral_encode(n: [f32; 3]) -> u32 {
    let l1 = n[0].abs() + n[1].abs() + n[2].abs();
    let (mut x, mut y) = (n[0] / l1, n[1] / l1);
    if n[2] < 0.0 {
        let (ox, oy) = (x, y);
        x = (1.0 - oy.abs()) * if ox >= 0.0 { 1.0 } else { -1.0 };
        y = (1.0 - ox.abs()) * if oy >= 0.0 { 1.0 } else { -1.0 };
    }
    let q = |v: f32| (((v + 1.0) * 0.5 * 1023.0).round() as u32).min(1023);
    1 << 31 | q(x) | q(y) << 10
}

fn encode_positions(g: &GeometrySpec) -> (Vec<u8>, [f32; 3], [f32; 3], u32) {
    let n = g.positions.len();
    match g.form {
        PositionForm::Float => {
            let mut b = Vec::new();
            for p in &g.positions {
                for k in p {
                    b.extend_from_slice(&k.to_le_bytes());
                }
            }
            (b, [0.0; 3], [0.0; 3], 0)
        }
        PositionForm::Quantized(axis) => {
            // The stored triple (a, b, c) maps to the model position through the axis word.
            let stored = |p: &[f32; 3]| match axis {
                0 => [p[2], p[1], p[0]],
                1 => [p[2], p[0], p[1]],
                _ => *p,
            };
            let rows: Vec<[f32; 3]> = g.positions.iter().map(stored).collect();
            let mut bias = [f32::INFINITY; 3];
            let mut hi = [f32::NEG_INFINITY; 3];
            for r in &rows {
                for k in 0..3 {
                    bias[k] = bias[k].min(r[k]);
                    hi[k] = hi[k].max(r[k]);
                }
            }
            let steps = [(1u64 << 24) - 1, (1u64 << 24) - 1, (1u64 << 16) - 1];
            let scale = [0, 1, 2].map(|k| ((hi[k] - bias[k]) / steps[k] as f32).max(1e-6));
            let mut out = Vec::with_capacity(n * 8);
            for r in &rows {
                let q = |k: usize| (((r[k] - bias[k]) / scale[k]).round() as u64).min(steps[k]);
                let (a, b, c) = (q(0), q(1), q(2));
                let word = (a << 8) | ((b >> 8) << 32) | (b & 0xFF) | (c << 48);
                out.extend_from_slice(&word.to_le_bytes());
            }
            (out, bias, scale, axis)
        }
    }
}

fn encode_indices(g: &GeometrySpec) -> Vec<u8> {
    struct Cluster {
        refs: Vec<u32>,
        tris: Vec<[u32; 3]>,
    }
    let mut clusters: Vec<Cluster> = Vec::new();
    for t in &g.triangles {
        let fits = clusters.last().is_some_and(|c| {
            let new = t.iter().filter(|v| !c.refs.contains(v)).count();
            c.refs.len() + new <= 128 && c.tris.len() < 255
        });
        if !fits {
            clusters.push(Cluster {
                refs: Vec::new(),
                tris: Vec::new(),
            });
        }
        let c = clusters.last_mut().expect("cluster");
        let local = t.map(|v| {
            c.refs.iter().position(|r| *r == v).unwrap_or_else(|| {
                c.refs.push(v);
                c.refs.len() - 1
            }) as u32
        });
        c.tris.push(local);
    }
    let total_refs: usize = clusters.iter().map(|c| c.refs.len()).sum();
    let total_tris: usize = clusters.iter().map(|c| c.tris.len()).sum();
    let mut flags = clusters.len() as u32;
    let mut constant = None;
    if let SkinForm::Constant(bone) = g.skin {
        flags |= 0x4000_0000 | 0x2000_0000;
        constant = Some(bone);
    }
    if g.short_refs {
        flags |= 0x8000_0000;
    }
    if let Some(bone) = constant {
        flags |= (u32::from(bone) * 2) << 16;
    }
    let mut out = Vec::new();
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&(total_tris as u32).to_le_bytes());
    out.extend_from_slice(&(total_refs as u32).to_le_bytes());
    let (mut vs, mut ts) = (0u32, 0u32);
    for c in &clusters {
        let packed = (c.refs.len() as u32 - 1) | ((c.tris.len() as u32 - 1) << 7);
        out.extend_from_slice(&packed.to_le_bytes());
        out.extend_from_slice(&(vs & 0xFFFF | (ts & 0xFFFF) << 16).to_le_bytes());
        out.extend_from_slice(&[0u8; 8]);
        vs += c.refs.len() as u32;
        ts += c.tris.len() as u32;
    }
    for (ci, c) in clusters.iter().enumerate() {
        for t in &c.tris {
            let word = t[0] | t[1] << 7 | t[2] << 14 | (ci as u32) << 21;
            out.extend_from_slice(&word.to_le_bytes());
        }
    }
    for c in &clusters {
        for r in &c.refs {
            if g.short_refs {
                out.extend_from_slice(&(*r as u16).to_le_bytes());
            } else {
                out.extend_from_slice(&r.to_le_bytes());
            }
        }
    }
    while out.len() % 16 != 0 {
        out.push(0);
    }
    out
}

fn write_shader_group(c: &mut Ctx, materials: &[MaterialSpec]) -> usize {
    let group = c.sys.alloc(64, 16);
    let array = c.sys.alloc(materials.len().max(1) * 8, 16);
    c.sys.sys_ptr(group + 16, array);
    c.sys.u16(group + 24, materials.len() as u16);
    c.sys.u16(group + 26, materials.len() as u16);
    for (i, m) in materials.iter().enumerate() {
        let shader = c.sys.alloc(64, 16);
        c.sys.u32(shader, m.shader);
        c.sys.u32(shader + 4, 0x6D65_7461);
        c.sys.u8(shader + 0x39, m.bucket);
        c.sys.u32(shader + 0x3C, 0xFF00 | (1 << (m.bucket & 0x7F)));
        let params = m.textures.len() + 1;
        let info = c.sys.alloc(8 + params * 8, 16);
        c.sys.u8(info + 1, m.textures.len() as u8);
        c.sys.u8(info + 4, params as u8);
        c.sys.u32(info + 8, 0x1111_2222);
        c.sys.u32(info + 12, 0x3333_4444);
        c.sys.sys_ptr(shader + 32, info);
        let tarr = c.sys.alloc(m.textures.len().max(1) * 8, 16);
        c.sys.sys_ptr(shader + 16, tarr);
        for (t, tex) in m.textures.iter().enumerate() {
            let reference = c.sys.alloc(80, 16);
            let asset_block = c.sys.alloc(32, 16);
            c.sys.u32(asset_block, 6);
            c.sys.u32(asset_block + 16, tex.name_hash);
            c.sys.u32(asset_block + 20, tex.param);
            c.sys.u64(asset_block + 24, tex.id64);
            c.sys.sys_ptr(reference + 48, asset_block + 16);
            c.sys.sys_ptr(tarr + 8 * t, reference);
        }
        c.sys.sys_ptr(array + 8 * i, shader);
    }
    group
}

fn write_skeleton(c: &mut Ctx, bones: &[BoneSpec]) -> usize {
    let n = bones.len();
    let header = c.sys.alloc(0x80, 16);
    let records = c.sys.alloc(n * 0x50, 16);
    let inverse = c.sys.alloc(n * 64, 16);
    let parents = c.sys.alloc(n * 2 + 2, 16);
    c.sys.sys_ptr(header + 0x28, records);
    c.sys.sys_ptr(header + 0x30, inverse);
    c.sys.sys_ptr(header + 0x40, parents);
    c.sys.u16(header + 0x6C, n as u16);
    c.sys.u16(header + 0x6E, n as u16);
    for (i, b) in bones.iter().enumerate() {
        let r = records + i * 0x50;
        for k in 0..4 {
            c.sys.f32(r + 4 * k, b.rotation[k]);
        }
        for k in 0..3 {
            c.sys.f32(r + 0x10 + 4 * k, b.translation[k]);
            c.sys.f32(r + 0x20 + 4 * k, 1.0);
        }
        c.sys.u32(r + 0x1C, b.hash);
        let name = c.sys.alloc(b.name.len() + 1, 16);
        c.sys.bytes(name, b.name.as_bytes());
        c.sys.sys_ptr(r + 0x30, name);
        let parent = b.parent.unwrap_or(0xFFFF);
        c.sys.u16(r + 0x46, parent);
        c.sys.u16(r + 0x48, i as u16);
        c.sys.u16(parents + 2 * i, parent);
        let m = inverse + 64 * i;
        for d in 0..4 {
            c.sys.f32(m + 20 * d, 1.0);
        }
        for k in 0..3 {
            c.sys.f32(m + 48 + 4 * k, -b.world[k]);
        }
    }
    header
}
