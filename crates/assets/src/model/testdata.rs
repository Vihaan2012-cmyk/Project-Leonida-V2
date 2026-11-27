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

