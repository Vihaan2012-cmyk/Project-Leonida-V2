//! Root signatures: how root parameters map to user SGPRs and spill tables.
//!
//! A serialized root signature (the PC format, optionally inside a DXBC container's `RTS0` part, with the
//! console's flag bits above bit 23) is parsed into [`RootSignature`]. [`RootSignature::layout`] places its
//! parameters the way the console's shader compiler does: in root order from a per-stage base SGPR, each while it
//! fits whole, a constant taking one dword per value, a root view two (its 64-bit VA) and a descriptor table one
//! (the 32-bit VA of its first descriptor); the first parameter that does not fit, and every later one, goes to a
//! packed array behind a 40-bit pointer in `s[0:1]`. [`pack`] fills the 32 user dwords from the arguments a
//! command list has set.
//!
//! # What is measured
//!
//! `leonida shaders census` reads every program of `shaders.rpf` (26,698: 2,689 compute, 12,101 pixel, 2,236
//! vertex, 195 geometry, 9,282 mesh and 195 first halves of vertex shaders merged with a geometry shader) and
//! follows which SGPRs each reads before writing. No game container carries an `RTS0` (0 of 26,698), so no
//! signature is paired with a program directly. The programs are named `<Stage>_<Name>_<hash>_Wrapped`, and the
//! hash behaves as the root signature's identity: programs of different stages that share it have the same
//! header user SGPR count (pixel and vertex: 662 hashes with one count in each stage, all equal; pixel and mesh
//! 412 of 412; pixel and geometry 12 of 12) and use each parameter SGPR the same way, at the shifts below.
//!
//! - **Compute, base s4.** `RTS0` of the 29 FidelityFX programs (an earlier measurement) plus the census: the
//!   lowest pointer is s4 in 2,364 of 2,689 programs and s5 in 318, none below s4 (s1..s3 are read as plain
//!   values by at most 11). The header's user SGPR count (bits 16-21 of the word at 0xD4) equals
//!   `COMPUTE_PGM_RSRC2.USER_SGPR` in 2,689 of 2,689 programs, so that field is the user count; the smallest
//!   count is 4 (4 programs).
//! - **Vertex and pixel, base s8.** No pixel program reads s0..s7 (0 of 12,101); the lowest user SGPR read is
//!   s8 in 9,832 and s9 in 1,856 (s8 unused). The smallest header count is 8 (5 pixel and 8 vertex programs: an
//!   empty signature takes exactly the reserved block, as compute takes 4). Pixel and vertex programs of one
//!   hash agree on how every SGPR both read is used in 379,611 of 379,620 cases at shift 0, and in 40,547 of
//!   134,372 at shift 8. 788 of 2,236 vertex programs jump through `s[4:5]`, a code address below the base.
//! - **Mesh and geometry, base s16.** Mesh against pixel programs of one hash: 5,099,693 agree and 11,182 differ
//!   at shift +8, 981,383 and 1,099,689 at shift 0. Geometry: 36,856 and 3 at +8, 1,043 and 28,918 at 0. The
//!   lowest mesh pointer is s16 in 7,977 of 9,282 programs and s17 in the other 1,305; none is lower. Geometry
//!   programs also read a 64-bit pointer at `s[8:9]` (195 of 195) and s2, s3 as plain values, which are not
//!   parameters. The first half of a merged vertex shader sits in the geometry window too (33,675 agree and 6
//!   differ at +8 against the same hash's vertex program): [`UserStage::GeometryVertex`].
//! - **The header count is stage independent.** It is 8 plus the signature's dwords for every graphics stage,
//!   so a mesh or geometry window really ends at `count + 7` (highest pointer minus count is 7 in all 1,338 mesh
//!   programs that index both heaps, against -1 in 7,038 of 7,055 such pixel programs).
//!   [`StageLayout::header_count`] gives the count. Largest counts: pixel 20, vertex 20, mesh 17, geometry 17,
//!   compute 14; no signature comes near 32 dwords.
//! - **Heap bases come last, resource then sampler.** Of the pixel programs whose `SFI0` sets both
//!   heap-indexing bits, 7,038 of 7,055 have their last pointer at the final user SGPR; the 1,043 that set only
//!   the sampler bit always end there; vertex programs that set only the resource bit end one SGPR before the
//!   final one in 821 of 864 (the sampler base follows, unused by them). That is the order
//!   [`RootSignature::layout`] uses: resource base, then sampler base, after the parameters.
//! - **Tables are one dword.** Every `s_load` base in a pixel, vertex or mesh program is 32-bit (the high half
//!   is a constant). Root views appear as descriptors built from an SGPR pair in vertex and mesh programs only;
//!   no pixel program builds one.
//! - **Code base.** 271 compute, 93 pixel, 54 vertex and 266 mesh programs call `s_getpc_b64`; none combines it
//!   with a user SGPR (0 of 26,698), so the code base is not user data.
//! - **Visibility.** Vertex and pixel programs of one hash never show a shifted window, so there is no sign
//!   that a parameter is hidden from one stage; whether any signature restricts visibility is not measured,
//!   and the layout ignores it.
//!
//! # Not measured
//!
//! - The spill pointer in `s[0:1]` for graphics: no graphics program reads s0 or s1 (0 of 24,009), and no
//!   graphics signature is wide enough to spill, so nothing confirms or contradicts it.
//! - [`SPILL_TABLE_BYTES`]: no program reads a spill array.
//! - Which parameter feeds which SGPR in a graphics signature: the window's start and end and the kinds of
//!   the SGPRs read are known, not the signatures themselves.
//! - What the reserved block holds (s2, s3 and `s[8:9]` of geometry, s3 of mesh, `s[4:5]` of some vertex
//!   programs).

use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RootError {
    #[error("serialized root signature is truncated in its {0}")]
    Truncated(&'static str),
    #[error("serialized root signature has version {0}, which is not 1.0, 1.1 or 1.2")]
    Version(u32),
    #[error("root signature has an unknown {what} value {value}")]
    Unknown { what: &'static str, value: u32 },
    #[error("the DXBC container has no RTS0 part")]
    NoRootPart,
    #[error("the root signature's dwords do not fit the {stage:?} user data: {detail}")]
    NoRoom {
        stage: UserStage,
        detail: &'static str,
    },
    #[error(
        "root parameter {param} lands on user SGPR s{sgpr} in two stages that need different values"
    )]
    StageWindowsClash { param: u32, sgpr: u32 },
    #[error(
        "root parameter {param} is a descriptor table at {handle:#x}, above the 4 GiB the user data can hold"
    )]
    TableAboveFourGiB { param: u32, handle: u64 },
    #[error("root parameter {param} does not exist in this root signature")]
    NoSuchParameter { param: u32 },
    #[error("root parameter {param} is not of the kind the call sets")]
    WrongKind { param: u32 },
    #[error("root constants {first}..{end} do not fit the parameter's {count} values")]
    ConstantRange { first: u32, end: u32, count: u32 },
}

/// The flag bits the console sets above the PC's 16: measured as `0x03000000` on the BVH builder's signatures.
/// Their meaning is not measured; the console names one `FORCE_MEMORY_BASED_ABI`.
pub const CONSOLE_FLAG_MASK: u32 = 0xFF00_0000;
pub const FLAG_CBV_SRV_UAV_HEAP_DIRECTLY_INDEXED: u32 = 0x400;
pub const FLAG_SAMPLER_HEAP_DIRECTLY_INDEXED: u32 = 0x800;

/// The D3D12 cost limit of a root signature, in dwords.
pub const COST_LIMIT: u32 = 64;
pub const USER_SGPRS: u32 = 32;
/// Bytes a descriptor table takes in the spill array. Unmeasured, and the census cannot measure it: no program
/// of `shaders.rpf` reads a spill array. The console compiler's notes give 4 for a constant and 8 for a
/// descriptor; a table is taken to keep the 32-bit form it has inline.
pub const SPILL_TABLE_BYTES: u32 = 4;
pub const SPILL_VIEW_BYTES: u32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RootVersion {
    V1_0,
    V1_1,
    V1_2,
}

impl RootVersion {
    fn number(self) -> u32 {
        match self {
            RootVersion::V1_0 => 1,
            RootVersion::V1_1 => 2,
            RootVersion::V1_2 => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RangeType {
    Srv,
    Uav,
    Cbv,
    Sampler,
}

impl RangeType {
    fn from_raw(value: u32) -> Result<RangeType, RootError> {
        Ok(match value {
            0 => RangeType::Srv,
            1 => RangeType::Uav,
            2 => RangeType::Cbv,
            3 => RangeType::Sampler,
            value => {
                return Err(RootError::Unknown {
                    what: "descriptor range type",
                    value,
                });
            }
        })
    }

    fn raw(self) -> u32 {
        self as u32
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DescriptorRange {
    pub kind: RangeType,
    /// `u32::MAX` for an unbounded range.
    pub count: u32,
    pub base_register: u32,
    pub space: u32,
    /// Zero in a 1.0 signature.
    pub flags: u32,
    /// `0xFFFF_FFFF` appends the range after the one before it.
    pub offset: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Visibility {
    All,
    Vertex,
    Hull,
    Domain,
    Geometry,
    Pixel,
    Amplification,
    Mesh,
}

impl Visibility {
    fn from_raw(value: u32) -> Result<Visibility, RootError> {
        Ok(match value {
            0 => Visibility::All,
            1 => Visibility::Vertex,
            2 => Visibility::Hull,
            3 => Visibility::Domain,
            4 => Visibility::Geometry,
            5 => Visibility::Pixel,
            6 => Visibility::Amplification,
            7 => Visibility::Mesh,
            value => {
                return Err(RootError::Unknown {
                    what: "shader visibility",
                    value,
                });
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ParameterKind {
    Table(Vec<DescriptorRange>),
    Constants {
        register: u32,
        space: u32,
        values: u32,
    },
    Cbv {
        register: u32,
        space: u32,
        flags: u32,
    },
    Srv {
        register: u32,
        space: u32,
        flags: u32,
    },
    Uav {
        register: u32,
        space: u32,
        flags: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RootParameter {
    pub kind: ParameterKind,
    pub visibility: Visibility,
}

impl RootParameter {
    /// Dwords the parameter takes in the user SGPRs.
    pub fn inline_dwords(&self) -> u32 {
        match &self.kind {
            ParameterKind::Table(_) => 1,
            ParameterKind::Constants { values, .. } => *values,
            _ => 2,
        }
    }

    pub fn spill_bytes(&self) -> u32 {
        match &self.kind {
            ParameterKind::Table(_) => SPILL_TABLE_BYTES,
            ParameterKind::Constants { values, .. } => values * 4,
            _ => SPILL_VIEW_BYTES,
        }
    }

    /// Dwords the parameter costs against D3D12's 64-dword limit: a table 1, constants their count, a view 2.
    pub fn cost(&self) -> u32 {
        self.inline_dwords()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StaticSampler {
    pub filter: u32,
    pub address_u: u32,
    pub address_v: u32,
    pub address_w: u32,
    pub mip_lod_bias: f32,
    pub max_anisotropy: u32,
    pub comparison: u32,
    pub border_color: u32,
    pub min_lod: f32,
    pub max_lod: f32,
    pub register: u32,
    pub space: u32,
    pub visibility: u32,
    /// 1.2 only: UINT border colours and non-normalized coordinates.
    pub flags: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RootSignature {
    pub version: RootVersion,
    pub parameters: Vec<RootParameter>,
    pub static_samplers: Vec<StaticSampler>,
    /// As serialized, console bits included.
    pub flags: u32,
}

struct Reader<'a> {
    bytes: &'a [u8],
}

impl Reader<'_> {
    fn u32(&self, at: usize, what: &'static str) -> Result<u32, RootError> {
        self.bytes
            .get(at..at.checked_add(4).ok_or(RootError::Truncated(what))?)
            .map(|b| u32::from_le_bytes(b.try_into().expect("4 bytes")))
            .ok_or(RootError::Truncated(what))
    }
}

impl RootSignature {
    /// Parses a serialized root signature: the bare form `D3D12SerializeRootSignature` returns, or a DXBC
    /// container with an `RTS0` part.
    pub fn parse(blob: &[u8]) -> Result<RootSignature, RootError> {
        if blob.starts_with(b"DXBC") {
            let parts =
                shader_container::parts(blob).map_err(|_| RootError::Truncated("container"))?;
            let part = shader_container::find_part(&parts, b"RTS0").ok_or(RootError::NoRootPart)?;
            return Self::parse(part);
        }
        let r = Reader { bytes: blob };
        let version = match r.u32(0, "header")? {
            1 => RootVersion::V1_0,
            2 => RootVersion::V1_1,
            3 => RootVersion::V1_2,
            other => return Err(RootError::Version(other)),
        };
        let count = r.u32(4, "header")? as usize;
        let params_at = r.u32(8, "header")? as usize;
        let samplers = r.u32(12, "header")? as usize;
        let samplers_at = r.u32(16, "header")? as usize;
        let flags = r.u32(20, "header")?;
        let with_flags = version != RootVersion::V1_0;
        let mut parameters = Vec::new();
        for i in 0..count {
            let at = params_at + 12 * i;
            let kind = r.u32(at, "parameter table")?;
            let visibility = Visibility::from_raw(r.u32(at + 4, "parameter table")?)?;
            let payload = r.u32(at + 8, "parameter table")? as usize;
            let view = |r: &Reader<'_>| -> Result<(u32, u32, u32), RootError> {
                let flags = if with_flags {
                    r.u32(payload + 8, "root descriptor")?
                } else {
                    0
                };
                Ok((
                    r.u32(payload, "root descriptor")?,
                    r.u32(payload + 4, "root descriptor")?,
                    flags,
                ))
            };
            let kind = match kind {
                0 => {
                    let ranges = r.u32(payload, "descriptor table")? as usize;
                    let ranges_at = r.u32(payload + 4, "descriptor table")? as usize;
                    let stride = if with_flags { 24 } else { 20 };
                    let mut list = Vec::with_capacity(ranges.min(1024));
                    for k in 0..ranges {
                        let at = ranges_at + stride * k;
                        let word = |n: usize| r.u32(at + 4 * n, "descriptor range");
                        let (flags, offset) = if with_flags {
                            (word(4)?, word(5)?)
                        } else {
                            (0, word(4)?)
                        };
                        list.push(DescriptorRange {
                            kind: RangeType::from_raw(word(0)?)?,
                            count: word(1)?,
                            base_register: word(2)?,
                            space: word(3)?,
                            flags,
                            offset,
                        });
                    }
                    ParameterKind::Table(list)
                }
                1 => ParameterKind::Constants {
                    register: r.u32(payload, "root constants")?,
                    space: r.u32(payload + 4, "root constants")?,
                    values: r.u32(payload + 8, "root constants")?,
                },
                2 => {
                    let (register, space, flags) = view(&r)?;
                    ParameterKind::Cbv {
                        register,
                        space,
                        flags,
                    }
                }
                3 => {
                    let (register, space, flags) = view(&r)?;
                    ParameterKind::Srv {
                        register,
                        space,
                        flags,
                    }
                }
                4 => {
                    let (register, space, flags) = view(&r)?;
                    ParameterKind::Uav {
                        register,
                        space,
                        flags,
                    }
                }
                value => {
                    return Err(RootError::Unknown {
                        what: "root parameter type",
                        value,
                    });
                }
            };
            parameters.push(RootParameter { kind, visibility });
        }
        let stride = if version == RootVersion::V1_2 { 14 } else { 13 };
        let mut static_samplers = Vec::new();
        for i in 0..samplers {
            let at = samplers_at + 4 * stride * i;
            let word = |n: usize| r.u32(at + 4 * n, "static sampler");
            static_samplers.push(StaticSampler {
                filter: word(0)?,
                address_u: word(1)?,
                address_v: word(2)?,
                address_w: word(3)?,
                mip_lod_bias: f32::from_bits(word(4)?),
                max_anisotropy: word(5)?,
                comparison: word(6)?,
                border_color: word(7)?,
                min_lod: f32::from_bits(word(8)?),
                max_lod: f32::from_bits(word(9)?),
                register: word(10)?,
                space: word(11)?,
                visibility: word(12)?,
                flags: if stride == 14 { word(13)? } else { 0 },
            });
        }
        Ok(RootSignature {
            version,
            parameters,
            static_samplers,
            flags,
        })
    }

    /// The bare serialized form, in this signature's own version.
    pub fn serialize(&self) -> Vec<u8> {
        let with_flags = self.version != RootVersion::V1_0;
        let sampler_stride = if self.version == RootVersion::V1_2 {
            14
        } else {
            13
        };
        let mut head = Vec::new();
        let mut payloads = Vec::new();
        let params_at = 24u32;
        let payload_base = params_at + 12 * self.parameters.len() as u32;
        let mut tables = Vec::new();
        for parameter in &self.parameters {
            let at = payload_base + payloads.len() as u32;
            let (kind, words): (u32, Vec<u32>) = match &parameter.kind {
                ParameterKind::Table(ranges) => {
                    tables.push((payloads.len(), ranges));
                    (0, vec![ranges.len() as u32, 0])
                }
                ParameterKind::Constants {
                    register,
                    space,
                    values,
                } => (1, vec![*register, *space, *values]),
                ParameterKind::Cbv {
                    register,
                    space,
                    flags,
                } => (2, view_words(*register, *space, *flags, with_flags)),
                ParameterKind::Srv {
                    register,
                    space,
                    flags,
                } => (3, view_words(*register, *space, *flags, with_flags)),
                ParameterKind::Uav {
                    register,
                    space,
                    flags,
                } => (4, view_words(*register, *space, *flags, with_flags)),
            };
            head.extend_from_slice(&kind.to_le_bytes());
            head.extend_from_slice(&(parameter.visibility as u32).to_le_bytes());
            head.extend_from_slice(&at.to_le_bytes());
            for word in words {
                payloads.extend_from_slice(&word.to_le_bytes());
            }
        }
        for (at, ranges) in tables {
            let ranges_at = payload_base + payloads.len() as u32;
            payloads[at + 4..at + 8].copy_from_slice(&ranges_at.to_le_bytes());
            for range in ranges {
                let mut words = vec![
                    range.kind.raw(),
                    range.count,
                    range.base_register,
                    range.space,
                ];
                if with_flags {
                    words.push(range.flags);
                }
                words.push(range.offset);
                for word in words {
                    payloads.extend_from_slice(&word.to_le_bytes());
                }
            }
        }
        let samplers_at = payload_base + payloads.len() as u32;
        let mut out = Vec::new();
        for word in [
            self.version.number(),
            self.parameters.len() as u32,
            params_at,
            self.static_samplers.len() as u32,
            samplers_at,
            self.flags,
        ] {
            out.extend_from_slice(&word.to_le_bytes());
        }
        out.extend_from_slice(&head);
        out.extend_from_slice(&payloads);
        for s in &self.static_samplers {
            let mut words = vec![
                s.filter,
                s.address_u,
                s.address_v,
                s.address_w,
                s.mip_lod_bias.to_bits(),
                s.max_anisotropy,
                s.comparison,
                s.border_color,
                s.min_lod.to_bits(),
                s.max_lod.to_bits(),
                s.register,
                s.space,
                s.visibility,
            ];
            if sampler_stride == 14 {
                words.push(s.flags);
            }
            for word in words {
                out.extend_from_slice(&word.to_le_bytes());
            }
        }
        out
    }

    /// A hash of the serialized form, console bits included: equal for signatures that place user data alike
    /// and use the same flags.
    pub fn fingerprint(&self) -> [u8; 32] {
        Sha256::digest(self.serialize()).into()
    }

    /// The flags a PC runtime would accept.
    pub fn pc_flags(&self) -> u32 {
        self.flags & !CONSOLE_FLAG_MASK
    }

    pub fn console_flags(&self) -> u32 {
        self.flags & CONSOLE_FLAG_MASK
    }

    pub fn resource_heap_indexed(&self) -> bool {
        self.flags & FLAG_CBV_SRV_UAV_HEAP_DIRECTLY_INDEXED != 0
    }

    pub fn sampler_heap_indexed(&self) -> bool {
        self.flags & FLAG_SAMPLER_HEAP_DIRECTLY_INDEXED != 0
    }

    /// Dwords against D3D12's 64-dword limit.
    pub fn cost(&self) -> u32 {
        self.parameters.iter().map(RootParameter::cost).sum()
    }

    /// Places the parameters in the user SGPRs of one stage.
    pub fn layout(&self, stage: UserStage) -> Result<StageLayout, RootError> {
        let mut sgpr = stage.base();
        let mut places = Vec::with_capacity(self.parameters.len());
        let mut spill_bytes = 0u32;
        let mut spilled = false;
        for parameter in &self.parameters {
            let dwords = parameter.inline_dwords();
            if !spilled && sgpr + dwords <= USER_SGPRS {
                places.push(Place::Inline { sgpr, dwords });
                sgpr += dwords;
            } else {
                spilled = true;
                places.push(Place::Spill {
                    byte_offset: spill_bytes,
                });
                spill_bytes += parameter.spill_bytes();
            }
        }
        let mut take = |wanted: bool| -> Result<Option<u32>, RootError> {
            if !wanted {
                return Ok(None);
            }
            if sgpr >= USER_SGPRS {
                return Err(RootError::NoRoom {
                    stage,
                    detail: "no SGPR is left for a heap base",
                });
            }
            sgpr += 1;
            Ok(Some(sgpr - 1))
        };
        let resource_heap = take(self.resource_heap_indexed())?;
        let sampler_heap = take(self.sampler_heap_indexed())?;
        Ok(StageLayout {
            stage,
            places,
            resource_heap,
            sampler_heap,
            spill_bytes,
            end: sgpr,
        })
    }
}

fn view_words(register: u32, space: u32, flags: u32, with_flags: bool) -> Vec<u32> {
    let mut words = vec![register, space];
    if with_flags {
        words.push(flags);
    }
    words
}

/// The stages whose user data the console lays out, each with its own first parameter SGPR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UserStage {
    Compute,
    Vertex,
    Pixel,
    Geometry,
    Mesh,
    /// The vertex program that runs inside a geometry shader's wave (the first half of a merged shader).
    GeometryVertex,
}

impl UserStage {
    /// The first SGPR a parameter takes. Compute's s4 is measured against signatures and the census; the
    /// graphics bases against the census and the name hash (see the module docs): vertex and pixel s8, and
    /// geometry, mesh and the vertex half of a geometry shader s16.
    pub fn base(self) -> u32 {
        match self {
            UserStage::Compute => 4,
            UserStage::Vertex | UserStage::Pixel => 8,
            UserStage::Geometry | UserStage::Mesh | UserStage::GeometryVertex => 16,
        }
    }

    /// Every base is measured. Only compute's is also checked against serialized signatures.
    pub fn base_measured(self) -> bool {
        true
    }

    pub fn base_against_signatures(self) -> bool {
        self == UserStage::Compute
    }

    /// What is still unmeasured for the stage, for a pipeline to note.
    pub fn open_questions(self) -> &'static [&'static str] {
        match self {
            UserStage::Compute => &["the spill array's table size"],
            _ => &[
                "the spill pointer's SGPR for graphics stages",
                "the spill array's table size",
                "whether shader visibility removes parameters from a stage",
            ],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Inline { sgpr: u32, dwords: u32 },
    Spill { byte_offset: u32 },
}

/// Where each root parameter lands for one stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageLayout {
    pub stage: UserStage,
    pub places: Vec<Place>,
    /// The SGPR holding the 32-bit base of the CBV/SRV/UAV heap, right after the parameters (the sampler heap's
    /// follows), when the signature indexes it directly. Measured on the shaders' reads, not on a signature.
    pub resource_heap: Option<u32>,
    pub sampler_heap: Option<u32>,
    pub spill_bytes: u32,
    /// The first SGPR after the user data, which is what `COMPUTE_PGM_RSRC2.USER_SGPR` counts up to.
    pub end: u32,
}

impl StageLayout {
    /// The user SGPR count a program of this signature carries in its header. Every graphics stage carries the
    /// same number, counted as if its window started at s8; compute counts from s0.
    pub fn header_count(&self) -> u32 {
        match self.stage {
            UserStage::Compute | UserStage::Vertex | UserStage::Pixel => self.end,
            _ => self.end - 8,
        }
    }

    pub fn spills(&self) -> bool {
        self.spill_bytes > 0
    }
}

/// What a command list has set for one root parameter.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RootArgument {
    #[default]
    Unset,
    Constants(Vec<u32>),
    /// The console VA of a root CBV, SRV or UAV.
    View(u64),
    /// The GPU descriptor handle of a table.
    Table(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArgKind {
    Constants,
    View,
    Table,
}

/// The root arguments of one bind point, indexed like the signature's parameters.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RootArguments {
    args: Vec<(ArgKind, RootArgument)>,
}

impl RootArguments {
    /// All parameters unset; root constants start as zeros.
    pub fn new(signature: &RootSignature) -> RootArguments {
        RootArguments {
            args: signature
                .parameters
                .iter()
                .map(|p| match &p.kind {
                    ParameterKind::Constants { values, .. } => (
                        ArgKind::Constants,
                        RootArgument::Constants(vec![0; *values as usize]),
                    ),
                    ParameterKind::Table(_) => (ArgKind::Table, RootArgument::Unset),
                    _ => (ArgKind::View, RootArgument::Unset),
                })
                .collect(),
        }
    }

    pub fn get(&self, param: u32) -> Option<&RootArgument> {
        self.args.get(param as usize).map(|(_, a)| a)
    }

    fn slot(&mut self, param: u32, kind: ArgKind) -> Result<&mut RootArgument, RootError> {
        match self.args.get_mut(param as usize) {
            None => Err(RootError::NoSuchParameter { param }),
            Some((k, slot)) if *k == kind => Ok(slot),
            Some(_) => Err(RootError::WrongKind { param }),
        }
    }

    /// `SetRoot32BitConstants`: `values` land from `first` on.
    pub fn set_constants(
        &mut self,
        param: u32,
        first: u32,
        values: &[u32],
    ) -> Result<(), RootError> {
        let RootArgument::Constants(list) = self.slot(param, ArgKind::Constants)? else {
            return Err(RootError::WrongKind { param });
        };
        let end = first as u64 + values.len() as u64;
        if end > list.len() as u64 {
            return Err(RootError::ConstantRange {
                first,
                end: end as u32,
                count: list.len() as u32,
            });
        }
        list[first as usize..end as usize].copy_from_slice(values);
        Ok(())
    }

    pub fn set_view(&mut self, param: u32, va: u64) -> Result<(), RootError> {
        *self.slot(param, ArgKind::View)? = RootArgument::View(va);
        Ok(())
    }

    pub fn set_table(&mut self, param: u32, handle: u64) -> Result<(), RootError> {
        *self.slot(param, ArgKind::Table)? = RootArgument::Table(handle);
        Ok(())
    }
}

/// The 32-bit bases of the bound descriptor heaps, written where a signature indexes them directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HeapBases {
    pub resource: u32,
    pub sampler: u32,
}

/// The user data one draw or dispatch starts with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserData {
    /// `s0..s31`. When `spill` is not empty the replay stores its 40-bit address in words 0 and 1.
    pub sgprs: [u32; 32],
    /// The packed spill array: constants 4 bytes, root views 8, tables 4, no alignment.
    pub spill: Vec<u8>,
}

/// Fills the user data for the stages of one pipeline. Stages that need different values in one SGPR (a mesh and
/// a pixel shader with bases 16 and 8 and parameters that overlap) are refused.
pub fn pack(
    layouts: &[&StageLayout],
    arguments: &RootArguments,
    heaps: HeapBases,
) -> Result<UserData, RootError> {
    let mut sgprs = [0u32; 32];
    let mut owner: [Option<(u32, u32)>; 32] = [None; 32];
    let mut spill = Vec::new();
    let mut put = |param: u32, part: u32, sgpr: u32, value: u32| -> Result<(), RootError> {
        let slot = sgpr as usize;
        match owner[slot] {
            Some(who) if who != (param, part) => {
                return Err(RootError::StageWindowsClash { param, sgpr });
            }
            _ => {}
        }
        owner[slot] = Some((param, part));
        sgprs[slot] = value;
        Ok(())
    };
    let mut spill_done = false;
    for layout in layouts {
        for (index, place) in layout.places.iter().enumerate() {
            let param = index as u32;
            let argument = arguments.get(param);
            match *place {
                Place::Inline { sgpr, dwords } => {
                    for k in 0..dwords {
                        let value = argument_dword(argument, param, k)?;
                        put(param, k, sgpr + k, value)?;
                    }
                }
                Place::Spill { .. } if !spill_done => {
                    let bytes = match argument {
                        Some(RootArgument::Constants(list)) => {
                            list.iter().flat_map(|v| v.to_le_bytes()).collect()
                        }
                        Some(RootArgument::View(va)) => va.to_le_bytes().to_vec(),
                        Some(RootArgument::Table(handle)) => {
                            table_low(param, *handle)?.to_le_bytes().to_vec()
                        }
                        _ => vec![0; 8],
                    };
                    spill.extend_from_slice(&bytes);
                }
                Place::Spill { .. } => {}
            }
        }
        spill_done |= layout.spills();
        if let Some(sgpr) = layout.resource_heap {
            put(u32::MAX, 0, sgpr, heaps.resource)?;
        }
        if let Some(sgpr) = layout.sampler_heap {
            put(u32::MAX, 1, sgpr, heaps.sampler)?;
        }
    }
    Ok(UserData { sgprs, spill })
}

fn table_low(param: u32, handle: u64) -> Result<u32, RootError> {
    u32::try_from(handle).map_err(|_| RootError::TableAboveFourGiB { param, handle })
}

fn argument_dword(argument: Option<&RootArgument>, param: u32, k: u32) -> Result<u32, RootError> {
    Ok(match argument {
        Some(RootArgument::Constants(list)) => list.get(k as usize).copied().unwrap_or(0),
        Some(RootArgument::View(va)) => (va >> (32 * k)) as u32,
        Some(RootArgument::Table(handle)) => table_low(param, *handle)?,
        _ => 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(kind: RangeType, count: u32, base: u32) -> DescriptorRange {
        DescriptorRange {
            kind,
            count,
            base_register: base,
            space: 0,
            flags: 0,
            offset: 0xFFFF_FFFF,
        }
    }

    fn table(ranges: Vec<DescriptorRange>) -> RootParameter {
        RootParameter {
            kind: ParameterKind::Table(ranges),
            visibility: Visibility::All,
        }
    }

    fn cbv(register: u32) -> RootParameter {
        RootParameter {
            kind: ParameterKind::Cbv {
                register,
                space: 0,
                flags: 0,
            },
            visibility: Visibility::All,
        }
    }

    fn constants(values: u32) -> RootParameter {
        RootParameter {
            kind: ParameterKind::Constants {
                register: 0,
                space: 1,
                values,
            },
            visibility: Visibility::Pixel,
        }
    }

    fn sampler() -> StaticSampler {
        StaticSampler {
            filter: 0x15,
            address_u: 1,
            address_v: 2,
            address_w: 3,
            mip_lod_bias: 0.5,
            max_anisotropy: 16,
            comparison: 4,
            border_color: 1,
            min_lod: 0.0,
            max_lod: 12.0,
            register: 2,
            space: 0,
            visibility: 5,
            flags: 0,
        }
    }

    fn fidelity_shape(version: RootVersion) -> RootSignature {
        let mut parameters = vec![
            table(vec![
                range(RangeType::Srv, 16, 0),
                range(RangeType::Uav, 4, 0),
            ]),
            table(vec![range(RangeType::Cbv, 1, 0)]),
        ];
        parameters.extend((0..4).map(cbv));
        RootSignature {
            version,
            parameters,
            static_samplers: vec![sampler()],
            flags: 0,
        }
    }

    #[test]
    fn every_version_round_trips() {
        for version in [RootVersion::V1_0, RootVersion::V1_1, RootVersion::V1_2] {
            let mut signature = fidelity_shape(version);
            signature.parameters.push(constants(5));
            signature.flags = 0x0300_0001;
            if version == RootVersion::V1_2 {
                signature.static_samplers[0].flags = 3;
            }
            if version == RootVersion::V1_0 {
                // 1.0 has no flag words: the writer drops them and the reader reads zeros.
                for p in &mut signature.parameters {
                    if let ParameterKind::Table(ranges) = &mut p.kind {
                        ranges.iter_mut().for_each(|r| r.flags = 0);
                    }
                }
            }
            let blob = signature.serialize();
            assert_eq!(
                RootSignature::parse(&blob).unwrap(),
                signature,
                "{version:?}"
            );
        }
    }

    #[test]
    fn range_flags_come_before_the_offset() {
        let mut signature = fidelity_shape(RootVersion::V1_1);
        let ParameterKind::Table(ranges) = &mut signature.parameters[0].kind else {
            unreachable!()
        };
        ranges[0].flags = 8;
        ranges[0].offset = 7;
        let blob = signature.serialize();
        let words: Vec<u32> = blob
            .chunks(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        let at = words.windows(2).position(|w| w == [8, 7]);
        assert!(at.is_some(), "flags 8 then offset 7 in {words:x?}");
    }

    #[test]
    fn dxbc_container_with_rts0_parses() {
        let signature = fidelity_shape(RootVersion::V1_1);
        let bare = signature.serialize();
        let blob = shader_container::testutil::dxbc(&[(b"HASH", &[0u8; 4]), (b"RTS0", &bare)]);
        assert_eq!(RootSignature::parse(&blob).unwrap(), signature);
        let none = shader_container::testutil::dxbc(&[(b"HASH", &[0u8; 4])]);
        assert_eq!(RootSignature::parse(&none), Err(RootError::NoRootPart));
    }

    #[test]
    fn damaged_blobs_are_errors() {
        let blob = fidelity_shape(RootVersion::V1_1).serialize();
        assert!(matches!(
            RootSignature::parse(&blob[..10]),
            Err(RootError::Truncated(_))
        ));
        assert!(matches!(
            RootSignature::parse(&blob[..60]),
            Err(RootError::Truncated(_))
        ));
        let mut bad = blob.clone();
        bad[0] = 9;
        assert_eq!(RootSignature::parse(&bad), Err(RootError::Version(9)));
        let mut bad = blob;
        bad[24] = 17;
        assert!(matches!(
            RootSignature::parse(&bad),
            Err(RootError::Unknown { .. })
        ));
    }

    #[test]
    fn console_flag_bits_are_kept_apart() {
        let mut signature = fidelity_shape(RootVersion::V1_1);
        signature.flags = 0x0300_0400;
        let parsed = RootSignature::parse(&signature.serialize()).unwrap();
        assert_eq!(parsed.console_flags(), 0x0300_0000);
        assert_eq!(parsed.pc_flags(), 0x400);
        assert!(parsed.resource_heap_indexed());
        assert!(!parsed.sampler_heap_indexed());
        assert_ne!(
            parsed.fingerprint(),
            fidelity_shape(RootVersion::V1_1).fingerprint()
        );
    }

    #[test]
    fn compute_layout_matches_the_measured_fidelityfx_signatures() {
        // Two tables and four root CBVs launch with 14 user SGPRs, two tables and one CBV with 8.
        let big = fidelity_shape(RootVersion::V1_1);
        let layout = big.layout(UserStage::Compute).unwrap();
        assert_eq!(layout.end, 14);
        assert_eq!(layout.places[0], Place::Inline { sgpr: 4, dwords: 1 });
        assert_eq!(layout.places[1], Place::Inline { sgpr: 5, dwords: 1 });
        assert_eq!(layout.places[2], Place::Inline { sgpr: 6, dwords: 2 });
        assert_eq!(
            layout.places[5],
            Place::Inline {
                sgpr: 12,
                dwords: 2
            }
        );
        let mut small = big;
        small.parameters.truncate(3);
        assert_eq!(small.layout(UserStage::Compute).unwrap().end, 8);
    }

    #[test]
    fn what_does_not_fit_spills_with_everything_after_it() {
        let signature = RootSignature {
            version: RootVersion::V1_1,
            parameters: vec![constants(20), cbv(0), constants(7), cbv(1), table(vec![])],
            static_samplers: vec![],
            flags: 0,
        };
        let layout = signature.layout(UserStage::Compute).unwrap();
        assert_eq!(
            layout.places[0],
            Place::Inline {
                sgpr: 4,
                dwords: 20
            }
        );
        assert_eq!(
            layout.places[1],
            Place::Inline {
                sgpr: 24,
                dwords: 2
            }
        );
        // 26 + 7 > 32: the constants spill, and the CBV and table after them too although they would fit.
        assert_eq!(layout.places[2], Place::Spill { byte_offset: 0 });
        assert_eq!(layout.places[3], Place::Spill { byte_offset: 28 });
        assert_eq!(layout.places[4], Place::Spill { byte_offset: 36 });
        assert_eq!(layout.spill_bytes, 36 + SPILL_TABLE_BYTES);
        assert_eq!(layout.end, 26);
    }

    #[test]
    fn heap_bases_follow_the_parameters() {
        let mut signature = fidelity_shape(RootVersion::V1_1);
        signature.parameters.truncate(3);
        signature.flags =
            FLAG_CBV_SRV_UAV_HEAP_DIRECTLY_INDEXED | FLAG_SAMPLER_HEAP_DIRECTLY_INDEXED;
        let layout = signature.layout(UserStage::Pixel).unwrap();
        assert_eq!(
            (layout.resource_heap, layout.sampler_heap),
            (Some(12), Some(13))
        );
        let mut full = RootSignature {
            parameters: vec![constants(24)],
            ..signature
        };
        assert_eq!(
            full.layout(UserStage::Pixel).unwrap_err(),
            RootError::NoRoom {
                stage: UserStage::Pixel,
                detail: "no SGPR is left for a heap base"
            }
        );
        full.parameters = vec![constants(22)];
        assert!(full.layout(UserStage::Pixel).is_ok());
    }

    fn set_up(signature: &RootSignature) -> RootArguments {
        let mut args = RootArguments::new(signature);
        args.set_table(0, 0x1_0000).unwrap();
        args.set_table(1, 0x2_0000).unwrap();
        args.set_view(2, 0x1234_5678_9ABC_DEF0).unwrap();
        args
    }

    #[test]
    fn packing_puts_values_where_the_layout_says() {
        let mut signature = fidelity_shape(RootVersion::V1_1);
        signature.parameters.truncate(3);
        signature.parameters.push(constants(3));
        let layout = signature.layout(UserStage::Compute).unwrap();
        let mut args = set_up(&signature);
        args.set_constants(3, 1, &[7, 8]).unwrap();
        let data = pack(&[&layout], &args, HeapBases::default()).unwrap();
        assert_eq!(&data.sgprs[..4], &[0; 4]);
        assert_eq!(&data.sgprs[4..6], &[0x1_0000, 0x2_0000]);
        assert_eq!(&data.sgprs[6..8], &[0x9ABC_DEF0, 0x1234_5678]);
        assert_eq!(&data.sgprs[8..11], &[0, 7, 8]);
        assert!(data.spill.is_empty());
    }

    #[test]
    fn spilled_values_are_packed_unaligned() {
        let signature = RootSignature {
            version: RootVersion::V1_1,
            parameters: vec![constants(28), constants(3), cbv(0)],
            static_samplers: vec![],
            flags: 0,
        };
        let layout = signature.layout(UserStage::Compute).unwrap();
        let mut args = RootArguments::new(&signature);
        args.set_constants(1, 0, &[1, 2, 3]).unwrap();
        args.set_view(2, 0xAABB_CCDD_0011_2233).unwrap();
        let data = pack(&[&layout], &args, HeapBases::default()).unwrap();
        let mut expect = Vec::new();
        for v in [1u32, 2, 3] {
            expect.extend_from_slice(&v.to_le_bytes());
        }
        expect.extend_from_slice(&0xAABB_CCDD_0011_2233u64.to_le_bytes());
        assert_eq!(data.spill, expect);
    }

    #[test]
    fn mesh_and_pixel_windows_coexist_while_they_do_not_overlap() {
        let mut signature = fidelity_shape(RootVersion::V1_1);
        signature.parameters.truncate(3);
        let (mesh, pixel) = (
            signature.layout(UserStage::Mesh).unwrap(),
            signature.layout(UserStage::Pixel).unwrap(),
        );
        let args = set_up(&signature);
        let data = pack(&[&mesh, &pixel], &args, HeapBases::default()).unwrap();
        assert_eq!(&data.sgprs[8..10], &[0x1_0000, 0x2_0000]);
        assert_eq!(&data.sgprs[16..18], &[0x1_0000, 0x2_0000]);
        // Eight dwords of parameters at s8 run into the mesh window at s16 with different owners.
        let wide = RootSignature {
            parameters: vec![constants(10)],
            ..signature
        };
        let (mesh, pixel) = (
            wide.layout(UserStage::Mesh).unwrap(),
            wide.layout(UserStage::Pixel).unwrap(),
        );
        let clash = pack(
            &[&mesh, &pixel],
            &RootArguments::new(&wide),
            HeapBases::default(),
        );
        assert!(matches!(clash, Err(RootError::StageWindowsClash { .. })));
    }

    #[test]
    fn argument_errors_are_typed() {
        let signature = fidelity_shape(RootVersion::V1_1);
        let mut args = RootArguments::new(&signature);
        assert_eq!(
            args.set_view(9, 0),
            Err(RootError::NoSuchParameter { param: 9 })
        );
        assert_eq!(args.set_view(0, 0), Err(RootError::WrongKind { param: 0 }));
        assert_eq!(
            args.set_constants(2, 0, &[1]),
            Err(RootError::WrongKind { param: 2 })
        );
        let layout = signature.layout(UserStage::Compute).unwrap();
        args.set_table(0, 1 << 33).unwrap();
        assert_eq!(
            pack(&[&layout], &args, HeapBases::default()),
            Err(RootError::TableAboveFourGiB {
                param: 0,
                handle: 1 << 33
            })
        );
    }

    #[test]
    fn graphics_bases_are_the_measured_ones() {
        let bases = [
            (UserStage::Compute, 4),
            (UserStage::Vertex, 8),
            (UserStage::Pixel, 8),
            (UserStage::Geometry, 16),
            (UserStage::Mesh, 16),
            (UserStage::GeometryVertex, 16),
        ];
        for (stage, base) in bases {
            assert_eq!(stage.base(), base, "{stage:?}");
            assert!(stage.base_measured());
        }
        assert!(UserStage::Compute.base_against_signatures());
        assert!(!UserStage::Pixel.base_against_signatures());
        assert!(!UserStage::Pixel.open_questions().is_empty());
    }

    #[test]
    fn the_header_count_is_the_same_for_every_graphics_stage() {
        // Two tables and one CBV: four dwords, then both heap bases.
        let mut signature = fidelity_shape(RootVersion::V1_1);
        signature.parameters.truncate(3);
        signature.flags =
            FLAG_CBV_SRV_UAV_HEAP_DIRECTLY_INDEXED | FLAG_SAMPLER_HEAP_DIRECTLY_INDEXED;
        let counts: Vec<u32> = [
            UserStage::Vertex,
            UserStage::Pixel,
            UserStage::Geometry,
            UserStage::GeometryVertex,
            UserStage::Mesh,
        ]
        .iter()
        .map(|&s| signature.layout(s).unwrap().header_count())
        .collect();
        assert_eq!(counts, vec![14; 5]);
        let mesh = signature.layout(UserStage::Mesh).unwrap();
        assert_eq!(
            (mesh.end, mesh.resource_heap, mesh.sampler_heap),
            (22, Some(20), Some(21))
        );
        assert_eq!(
            signature.layout(UserStage::Compute).unwrap().header_count(),
            10
        );
    }

    #[test]
    fn a_geometry_shader_and_its_vertex_half_share_one_window() {
        let mut signature = fidelity_shape(RootVersion::V1_1);
        signature.parameters.truncate(3);
        let args = set_up(&signature);
        let (geometry, vertex) = (
            signature.layout(UserStage::Geometry).unwrap(),
            signature.layout(UserStage::GeometryVertex).unwrap(),
        );
        let data = pack(&[&vertex, &geometry], &args, HeapBases::default()).unwrap();
        assert_eq!(&data.sgprs[16..18], &[0x1_0000, 0x2_0000]);
        assert_eq!(&data.sgprs[8..16], &[0; 8]);
        let pixel = signature.layout(UserStage::Pixel).unwrap();
        assert!(pack(&[&geometry, &pixel], &args, HeapBases::default()).is_ok());
        let wide = RootSignature {
            parameters: vec![constants(10)],
            ..signature
        };
        let clash = pack(
            &[
                &wide.layout(UserStage::Geometry).unwrap(),
                &wide.layout(UserStage::Pixel).unwrap(),
            ],
            &RootArguments::new(&wide),
            HeapBases::default(),
        );
        assert!(matches!(clash, Err(RootError::StageWindowsClash { .. })));
    }
}
