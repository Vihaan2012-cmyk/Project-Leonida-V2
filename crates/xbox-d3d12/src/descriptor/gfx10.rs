//! The words a console shader loads from a descriptor heap: the image descriptor (T#, 8 dwords), the sampler
//! descriptor (S#, 4 dwords) and the buffer descriptor (V#, 4 dwords), in the GFX10 layout.
//!
//! Only what a Vulkan view or sampler needs is encoded; the tiling mode, compression metadata and performance
//! hints are written as zero and ignored when decoding.

pub type ImageWords = [u32; 8];
pub type SamplerWords = [u32; 4];
pub type BufferWords = [u32; 4];
/// `DST_SEL` X, Y, Z, W.
pub type Swizzle = [u8; 4];

pub const SELECT_ZERO: u8 = 0;
pub const SELECT_ONE: u8 = 1;
pub const SELECT_X: u8 = 4;
pub const SELECT_Y: u8 = 5;
pub const SELECT_Z: u8 = 6;
pub const SELECT_W: u8 = 7;
pub const IDENTITY: Swizzle = [SELECT_X, SELECT_Y, SELECT_Z, SELECT_W];

fn field(value: u32, low: u32, width: u32) -> u32 {
    (value & ((1 << width) - 1)) << low
}

fn get(word: u32, low: u32, width: u32) -> u32 {
    (word >> low) & ((1 << width) - 1)
}

/// 4.8 fixed point after clamping; NaN becomes `low`.
fn unsigned_fixed(value: f32, low: f32, high: f32) -> u32 {
    let clamped = if value > low { value.min(high) } else { low };
    (clamped * 256.0) as u32
}

fn signed_fixed(value: f32, low: f32, high: f32) -> u32 {
    let clamped = if value > low { value.min(high) } else { low };
    (clamped * 256.0) as i32 as u32
}

/// 2 and 3 are reserved.
pub fn valid_swizzle(swizzle: Swizzle) -> bool {
    swizzle.iter().all(|s| *s <= 7 && *s != 2 && *s != 3)
}

fn swizzle_bits(swizzle: Swizzle) -> u32 {
    swizzle.iter().enumerate().fold(0, |word, (i, s)| {
        word | field(u32::from(*s), 3 * i as u32, 3)
    })
}

fn swizzle_of(word: u32) -> Swizzle {
    std::array::from_fn(|i| get(word, 3 * i as u32, 3) as u8)
}

/// `TYPE`, word 3 bits 31:28. Values below 8 are reserved, which is how a heap slot holding a V# is told apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageType {
    D1 = 8,
    D2 = 9,
    D3 = 10,
    Cube = 11,
    D1Array = 12,
    D2Array = 13,
    D2Msaa = 14,
    D2MsaaArray = 15,
}

impl ImageType {
    pub fn from_bits(bits: u32) -> Option<ImageType> {
        Some(match bits {
            8 => ImageType::D1,
            9 => ImageType::D2,
            10 => ImageType::D3,
            11 => ImageType::Cube,
            12 => ImageType::D1Array,
            13 => ImageType::D2Array,
            14 => ImageType::D2Msaa,
            15 => ImageType::D2MsaaArray,
            _ => return None,
        })
    }

    pub fn multisampled(self) -> bool {
        matches!(self, ImageType::D2Msaa | ImageType::D2MsaaArray)
    }
}

/// What a T# says, in normal form: `depth` is 1 unless the type is 3D, `layer_count` is 1 for 3D, `samples` is 1
/// unless multisampled, and a multisampled image has one mip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Image {
    /// 256-byte aligned, 48 bits.
    pub address: u64,
    pub format: u32,
    pub ty: ImageType,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub first_layer: u32,
    pub layer_count: u32,
    pub first_mip: u32,
    pub mip_count: u32,
    pub resource_mips: u32,
    pub samples: u32,
    pub swizzle: Swizzle,
    /// The view's LOD clamp, 0 to 15 in steps of 1/256.
    pub min_lod: f32,
}

impl Default for Image {
    fn default() -> Image {
        Image {
            address: 0,
            format: 0,
            ty: ImageType::D2,
            width: 1,
            height: 1,
            depth: 1,
            first_layer: 0,
            layer_count: 1,
            first_mip: 0,
            mip_count: 1,
            resource_mips: 1,
            samples: 1,
            swizzle: IDENTITY,
            min_lod: 0.0,
        }
    }
}

/// Word 0 `BASE_ADDRESS` (address >> 8); word 1 `BASE_ADDRESS_HI` [7:0], `MIN_LOD` [19:8], `FORMAT` [28:20],
/// `WIDTH_LO` [31:30]; word 2 `WIDTH_HI` [11:0], `HEIGHT` [27:14] (width and height less one); word 3 `DST_SEL`
/// [11:0], `BASE_LEVEL` [15:12], `LAST_LEVEL` [19:16], `TYPE` [31:28]; word 4 `DEPTH` [12:0] (last layer, or depth
/// less one for 3D) and `BASE_ARRAY` [28:16]; word 5 `MAX_MIP` [7:4]. A multisampled image has base level 0 and
/// log2(samples) in the last level and max mip.
pub fn encode_image(image: &Image) -> ImageWords {
    let msaa = image.ty.multisampled();
    let sample_log2 = image.samples.max(1).ilog2();
    let base_level = if msaa { 0 } else { image.first_mip };
    let last_level = if msaa {
        sample_log2
    } else {
        image.first_mip + image.mip_count.max(1) - 1
    };
    let max_mip = if msaa {
        sample_log2
    } else {
        image.resource_mips.max(1) - 1
    };
    let last = if image.ty == ImageType::D3 {
        image.depth.max(1) - 1
    } else {
        image.first_layer + image.layer_count.max(1) - 1
    };
    let units = image.address >> 8;
    let width = image.width.max(1) - 1;
    let height = image.height.max(1) - 1;
    let mut words = [0; 8];
    words[0] = units as u32;
    words[1] = field((units >> 32) as u32, 0, 8)
        | field(unsigned_fixed(image.min_lod, 0.0, 15.0), 8, 12)
        | field(image.format, 20, 9)
        | field(width, 30, 2);
    words[2] = field(width >> 2, 0, 12) | field(height, 14, 14);
    words[3] = swizzle_bits(image.swizzle)
        | field(base_level, 12, 4)
        | field(last_level, 16, 4)
        | field(image.ty as u32, 28, 4);
    words[4] = field(last, 0, 13) | field(image.first_layer, 16, 13);
    words[5] = field(max_mip, 4, 4);
    words
}

/// Whether these eight dwords are a T#: `TYPE` names an image.
pub fn is_image(words: &ImageWords) -> bool {
    get(words[3], 28, 4) >= 8
}

/// `None` when the dwords are not an image descriptor or a field contradicts another: a reserved `DST_SEL`, a last
/// level or layer before the first, or a cube whose layers are not whole cubes.
pub fn decode_image(words: &ImageWords) -> Option<Image> {
    let ty = ImageType::from_bits(get(words[3], 28, 4))?;
    let swizzle = swizzle_of(words[3]);
    if !valid_swizzle(swizzle) {
        return None;
    }
    let mut image = Image {
        ty,
        address: ((u64::from(get(words[1], 0, 8)) << 32) | u64::from(words[0])) << 8,
        min_lod: get(words[1], 8, 12) as f32 / 256.0,
        format: get(words[1], 20, 9),
        width: (get(words[1], 30, 2) | (get(words[2], 0, 12) << 2)) + 1,
        height: get(words[2], 14, 14) + 1,
        swizzle,
        ..Image::default()
    };
    let base_level = get(words[3], 12, 4);
    let last_level = get(words[3], 16, 4);
    if ty.multisampled() {
        if base_level != 0 {
            return None;
        }
        image.samples = 1 << last_level;
    } else {
        if last_level < base_level {
            return None;
        }
        image.first_mip = base_level;
        image.mip_count = last_level - base_level + 1;
        image.resource_mips = get(words[5], 4, 4) + 1;
    }
    let last = get(words[4], 0, 13);
    image.first_layer = get(words[4], 16, 13);
    if ty == ImageType::D3 {
        image.depth = last + 1;
    } else {
        if last < image.first_layer {
            return None;
        }
        image.layer_count = last - image.first_layer + 1;
        if ty == ImageType::Cube && !image.layer_count.is_multiple_of(6) {
            return None;
        }
    }
    Some(image)
}

/// `SQ_TEX_CLAMP`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AddressMode {
    Wrap = 0,
    Mirror = 1,
    ClampLastTexel = 2,
    MirrorOnceLastTexel = 3,
    ClampHalfBorder = 4,
    MirrorOnceHalfBorder = 5,
    ClampBorder = 6,
    MirrorOnceBorder = 7,
}

impl AddressMode {
    fn from_bits(bits: u32) -> AddressMode {
        [
            AddressMode::Wrap,
            AddressMode::Mirror,
            AddressMode::ClampLastTexel,
            AddressMode::MirrorOnceLastTexel,
            AddressMode::ClampHalfBorder,
            AddressMode::MirrorOnceHalfBorder,
            AddressMode::ClampBorder,
            AddressMode::MirrorOnceBorder,
        ][(bits & 7) as usize]
    }
}

/// `SQ_TEX_XY_FILTER`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Filter {
    Point = 0,
    Bilinear = 1,
    AnisoPoint = 2,
    AnisoBilinear = 3,
}

impl Filter {
    fn from_bits(bits: u32) -> Filter {
        [
            Filter::Point,
            Filter::Bilinear,
            Filter::AnisoPoint,
            Filter::AnisoBilinear,
        ][(bits & 3) as usize]
    }

    pub fn linear(self) -> bool {
        matches!(self, Filter::Bilinear | Filter::AnisoBilinear)
    }
}

/// `SQ_TEX_MIP_FILTER`; 3 is not named and is kept as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MipFilter {
    None = 0,
    Point = 1,
    Linear = 2,
    Unnamed = 3,
}

impl MipFilter {
    fn from_bits(bits: u32) -> MipFilter {
        [
            MipFilter::None,
            MipFilter::Point,
            MipFilter::Linear,
            MipFilter::Unnamed,
        ][(bits & 3) as usize]
    }
}

/// `SQ_TEX_DEPTH_COMPARE`, the same values as `VkCompareOp`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Compare {
    Never = 0,
    Less = 1,
    Equal = 2,
    LessEqual = 3,
    Greater = 4,
    NotEqual = 5,
    GreaterEqual = 6,
    Always = 7,
}

impl Compare {
    fn from_bits(bits: u32) -> Compare {
        [
            Compare::Never,
            Compare::Less,
            Compare::Equal,
            Compare::LessEqual,
            Compare::Greater,
            Compare::NotEqual,
            Compare::GreaterEqual,
            Compare::Always,
        ][(bits & 7) as usize]
    }
}

/// `SQ_TEX_BORDER_COLOR`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Border {
    TransparentBlack = 0,
    OpaqueBlack = 1,
    OpaqueWhite = 2,
    /// The colour at `border_index` in the device's table.
    Table = 3,
}

impl Border {
    fn from_bits(bits: u32) -> Border {
        [
            Border::TransparentBlack,
            Border::OpaqueBlack,
            Border::OpaqueWhite,
            Border::Table,
        ][(bits & 3) as usize]
    }
}

/// `SQ_IMG_FILTER_TYPE`: D3D12's minimum and maximum filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reduction {
    Blend = 0,
    Minimum = 1,
    Maximum = 2,
    Unnamed = 3,
}

impl Reduction {
    fn from_bits(bits: u32) -> Reduction {
        [
            Reduction::Blend,
            Reduction::Minimum,
            Reduction::Maximum,
            Reduction::Unnamed,
        ][(bits & 3) as usize]
    }
}

/// What an S# says. Decoding gives the normal form: `max_anisotropy` is 1, 2, 4, 8 or 16.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sampler {
    pub address_u: AddressMode,
    pub address_v: AddressMode,
    pub address_w: AddressMode,
    pub mag: Filter,
    pub min: Filter,
    pub mip: MipFilter,
    pub max_anisotropy: u32,
    pub compare: Compare,
    pub min_lod: f32,
    pub max_lod: f32,
    pub lod_bias: f32,
    pub border: Border,
    pub border_index: u32,
    pub unnormalized: bool,
    pub reduction: Reduction,
    /// This layer's: the border colour is read as integers (`D3D12_SAMPLER_FLAG_UINT_BORDER_COLOR`), kept in the
    /// border pointer field, which the hardware reads only for `Border::Table`.
    pub integer_border: bool,
    /// This layer's: a comparison sampler whose function is NEVER, which the hardware field cannot tell from no
    /// comparison. Kept in word 3 bit 12, unused with `DERIV_ADJUST_EN` clear.
    pub compare_never: bool,
}

impl Default for Sampler {
    fn default() -> Sampler {
        Sampler {
            address_u: AddressMode::Wrap,
            address_v: AddressMode::Wrap,
            address_w: AddressMode::Wrap,
            mag: Filter::Point,
            min: Filter::Point,
            mip: MipFilter::None,
            max_anisotropy: 1,
            compare: Compare::Never,
            min_lod: 0.0,
            max_lod: 0.0,
            lod_bias: 0.0,
            border: Border::TransparentBlack,
            border_index: 0,
            unnormalized: false,
            reduction: Reduction::Blend,
            integer_border: false,
            compare_never: false,
        }
    }
}

const INTEGER_BORDER_POINTER: u32 = 0x800;
const COMPARE_NEVER_BIT: u32 = 1 << 12;

/// Word 0 `CLAMP_X/Y/Z` [8:0], `MAX_ANISO_RATIO` [11:9] (log2, at most 4), `DEPTH_COMPARE_FUNC` [14:12],
/// `FORCE_UNNORMALIZED` [15], `FILTER_MODE` [30:29]; word 1 `MIN_LOD` [11:0], `MAX_LOD` [23:12] (4.8 fixed, 0 to
/// 15); word 2 `LOD_BIAS` [13:0] (signed 5.8 fixed, -32 to 31), `XY_MAG_FILTER` [21:20], `XY_MIN_FILTER` [23:22],
/// `MIP_FILTER` [27:26]; word 3 `BORDER_COLOR_PTR` [11:0], `BORDER_COLOR_TYPE` [31:30].
pub fn encode_sampler(s: &Sampler) -> SamplerWords {
    let aniso = s.max_anisotropy.max(1).ilog2().min(4);
    let marked = s.integer_border && s.border != Border::Table;
    let mut words = [0; 4];
    words[0] = field(s.address_u as u32, 0, 3)
        | field(s.address_v as u32, 3, 3)
        | field(s.address_w as u32, 6, 3)
        | field(aniso, 9, 3)
        | field(s.compare as u32, 12, 3)
        | field(u32::from(s.unnormalized), 15, 1)
        | field(s.reduction as u32, 29, 2);
    words[1] = field(unsigned_fixed(s.min_lod, 0.0, 15.0), 0, 12)
        | field(unsigned_fixed(s.max_lod, 0.0, 15.0), 12, 12);
    words[2] = field(signed_fixed(s.lod_bias, -32.0, 31.0), 0, 14)
        | field(s.mag as u32, 20, 2)
        | field(s.min as u32, 22, 2)
        | field(s.mip as u32, 26, 2);
    words[3] = field(
        if marked {
            INTEGER_BORDER_POINTER
        } else {
            s.border_index
        },
        0,
        12,
    ) | field(s.border as u32, 30, 2);
    if s.compare == Compare::Never && s.compare_never {
        words[3] |= COMPARE_NEVER_BIT;
    }
    words
}

/// Every bit pattern is a sampler.
pub fn decode_sampler(words: &SamplerWords) -> Sampler {
    let bias = get(words[2], 0, 14);
    let compare = Compare::from_bits(get(words[0], 12, 3));
    let deriv_adjust = get(words[2], 31, 1) != 0;
    let mut sampler = Sampler {
        address_u: AddressMode::from_bits(get(words[0], 0, 3)),
        address_v: AddressMode::from_bits(get(words[0], 3, 3)),
        address_w: AddressMode::from_bits(get(words[0], 6, 3)),
        max_anisotropy: 1 << get(words[0], 9, 3).min(4),
        compare,
        compare_never: !deriv_adjust
            && compare == Compare::Never
            && words[3] & COMPARE_NEVER_BIT != 0,
        unnormalized: get(words[0], 15, 1) != 0,
        reduction: Reduction::from_bits(get(words[0], 29, 2)),
        min_lod: get(words[1], 0, 12) as f32 / 256.0,
        max_lod: get(words[1], 12, 12) as f32 / 256.0,
        lod_bias: ((bias ^ 0x2000) as i32 - 0x2000) as f32 / 256.0,
        mag: Filter::from_bits(get(words[2], 20, 2)),
        min: Filter::from_bits(get(words[2], 22, 2)),
        mip: MipFilter::from_bits(get(words[2], 26, 2)),
        border_index: get(words[3], 0, 12),
        border: Border::from_bits(get(words[3], 30, 2)),
        ..Sampler::default()
    };
    if sampler.border != Border::Table && sampler.border_index == INTEGER_BORDER_POINTER {
        sampler.integer_border = true;
        sampler.border_index = 0;
    }
    sampler
}

/// `OOB_SELECT`, word 3 bits 29:28: the bounds check the hardware makes (`docs/specs/buffer_bounds.md`). Out of
/// range, a load returns 0 and a store does nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutOfBounds {
    /// Index at or past the record count, or offset + 4 past the stride.
    StructuredWithOffset = 0,
    /// Index at or past the record count.
    Structured = 1,
    /// Only a record count of zero.
    Disabled = 2,
    /// Offset + soffset + 4 past the byte count.
    Raw = 3,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Buffer {
    /// 48 bits.
    pub address: u64,
    /// Bytes a record, 14 bits; 0 for a raw buffer.
    pub stride: u32,
    /// Records, or bytes when the stride is 0.
    pub num_records: u32,
    /// A GFX10 format code below 128, for typed loads and stores.
    pub format: u32,
    pub swizzle: Swizzle,
    pub out_of_bounds: OutOfBounds,
}

impl Default for Buffer {
    fn default() -> Buffer {
        Buffer {
            address: 0,
            stride: 0,
            num_records: 0,
            format: 0,
            swizzle: IDENTITY,
            out_of_bounds: OutOfBounds::StructuredWithOffset,
        }
    }
}

/// Word 0 `BASE_ADDRESS`; word 1 `BASE_ADDRESS_HI` [15:0] and `STRIDE` [29:16]; word 2 `NUM_RECORDS`; word 3
/// `DST_SEL` [11:0], `FORMAT` [18:12], `OOB_SELECT` [29:28], `TYPE` [31:30] = 0.
pub fn encode_buffer(b: &Buffer) -> BufferWords {
    [
        b.address as u32,
        field((b.address >> 32) as u32, 0, 16) | field(b.stride, 16, 14),
        b.num_records,
        swizzle_bits(b.swizzle) | field(b.format, 12, 7) | field(b.out_of_bounds as u32, 28, 2),
    ]
}

/// `None` when `TYPE` is not a buffer's or a `DST_SEL` is reserved.
pub fn decode_buffer(words: &BufferWords) -> Option<Buffer> {
    if get(words[3], 30, 2) != 0 {
        return None;
    }
    let swizzle = swizzle_of(words[3]);
    if !valid_swizzle(swizzle) {
        return None;
    }
    Some(Buffer {
        address: (u64::from(get(words[1], 0, 16)) << 32) | u64::from(words[0]),
        stride: get(words[1], 16, 14),
        num_records: words[2],
        format: get(words[3], 12, 7),
        swizzle,
        out_of_bounds: [
            OutOfBounds::StructuredWithOffset,
            OutOfBounds::Structured,
            OutOfBounds::Disabled,
            OutOfBounds::Raw,
        ][get(words[3], 28, 2) as usize],
    })
}
