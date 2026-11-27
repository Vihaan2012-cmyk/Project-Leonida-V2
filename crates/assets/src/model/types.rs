/// What a resource holds, from the signature of its first system page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Drawable,
    /// A drawable dictionary: one independent drawable per entry.
    Dictionary,
    /// A fragment: a main drawable (carrying the skeleton) plus optional child drawables.
    Fragment,
}

/// Where a resource's bytes split, and what is known about it from outside.
#[derive(Debug, Clone, Default)]
pub struct ResourceInfo {
    /// Resource name when the archive layer knows it.
    pub name: Option<String>,
    /// Length of the system pages at the start of the data; the graphics pages follow.
    pub system_len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub radius: f32,
}

/// Bone skinning of one geometry. Every vertex has eight slots, strongest first; unused slots have
/// weight 0 and joint 0. Joints index the model's bone palette.
#[derive(Debug, Clone, PartialEq)]
pub struct SkinData {
    pub layout: SkinLayout,
    pub joints: Vec<[u16; 8]>,
    pub weights: Vec<[u8; 8]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkinLayout {
    /// Four 8-bit indices per weight stream.
    Bytes,
    /// Four 12-bit indices in 32-bit words per weight stream.
    Packed,
    /// Four 16-bit indices per weight stream.
    Words,
    /// One index per vertex, no weight stream.
    Rigid,
    /// No bone streams; the whole geometry is bound to one palette entry.
    Constant,
}

impl SkinLayout {
    pub fn name(self) -> &'static str {
        match self {
            SkinLayout::Bytes => "u8x4",
            SkinLayout::Packed => "packed",
            SkinLayout::Words => "u16x4",
            SkinLayout::Rigid => "rigid",
            SkinLayout::Constant => "constant",
        }
    }
}

/// Decoded vertex and index data of one geometry, in the game's own Z-up space.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Geometry {
    /// Index into the drawable's materials.
    pub material: usize,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// One list per texture-coordinate stream, in slot order.
    pub uv_sets: Vec<Vec<[f32; 2]>>,
    pub colors: Vec<[u8; 4]>,
    /// Triangle corner indices, three per triangle.
    pub indices: Vec<u32>,
    pub skin: Option<SkinData>,
}

impl Geometry {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Model {
    /// `joaat32` hashes of the bone names the skin indices refer to, sorted ascending in the game.
    pub bone_palette: Vec<u32>,
    pub geometries: Vec<Geometry>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Lod {
    /// Slot 0 to 3 in the drawable's LOD table.
    pub level: usize,
    /// Switch distance in metres from the drawable's distance table.
    pub distance: f32,
    pub models: Vec<Model>,
}

/// One texture binding of a material. The slot record layout is inferred (name hash, parameter hash,
/// then the 64-bit id), so match `id64` against a texture pool rather than trusting `param_hash` alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterialTexture {
    pub param_hash: u32,
    pub name_hash: u32,
    pub id64: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Material {
    pub shader_hash: u32,
    /// Byte at shader +0x39: low 7 bits are the bucket (0 opaque, 1 alpha, 2 decal, 3 cutout, 6 water).
    pub render_bucket: u8,
    pub render_bucket_mask: u32,
    pub textures: Vec<MaterialTexture>,
    /// The parameter table words, undecoded.
    pub raw_parameters: Vec<[u32; 2]>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Bone {
    pub name: Option<String>,
    /// `joaat32` of the lowercase bone name.
    pub hash: u32,
    pub parent: Option<u16>,
    /// Local default rotation `(x, y, z, w)`.
    pub rotation: [f32; 4],
    pub translation: [f32; 3],
    pub scale: [f32; 3],
    /// Inverse bind matrix, 16 floats in glTF column-major order (the game stores D3D row-vector
    /// matrices, whose bytes are exactly that).
    pub inverse_bind: [f32; 16],
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Skeleton {
    pub bones: Vec<Bone>,
}

#[derive(Debug, Clone)]
pub struct Drawable {
    /// Name of the drawable when the resource or a dictionary hash gives one.
    pub name: Option<String>,
    /// Name hash from a drawable dictionary's key array.
    pub name_hash: Option<u32>,
    pub bounds: Bounds,
    pub lods: Vec<Lod>,
    pub materials: Vec<Material>,
    pub skeleton: Option<Skeleton>,
    pub embedded_textures: usize,
}

/// Everything decoded from one resource.
#[derive(Debug, Clone)]
pub struct ModelSet {
    pub name: Option<String>,
    pub kind: ResourceKind,
    pub drawables: Vec<Drawable>,
    /// Things that decoded but rest on an unmeasured assumption, or were skipped.
    pub warnings: Vec<String>,
}
