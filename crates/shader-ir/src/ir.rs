use std::fmt;

pub use rdna::format::Round;

pub type Tmp = u32;
pub type BlockId = usize;

/// Interpolated parameters a pixel shader can read; each has four channels and a value per triangle vertex.
pub const ATTRIBUTES: usize = 32;

pub fn attribute_slot(attr: u8, channel: u8, vertex: u8) -> usize {
    (usize::from(attr) * 4 + usize::from(channel)) * 3 + usize::from(vertex)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reg {
    S(u8),
    V(u8),
    Exec,
    Vcc,
    Scc,
    M0,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Un {
    Not,
    Bfrev,
    Ffbl,
    Ffbh,
    Popcount,
    Popcount64,
    Not64,
    Lo,
    Hi,
    Zext,
    BoolToU32,
    CvtF32U32,
    CvtF32I32,
    CvtU32F32,
    CvtI32F32,
    FNeg,
    FAbs,
    FClamp01,
    FRcp,
    FRsq,
    FSqrt,
    FExp2,
    FLog2,
    FFloor,
    FCeil,
    FTrunc,
    FRndne,
    FFract,
    FSin,
    FCos,
    F16ToF32,
    F32ToF16(Round),
    FCanonical,
    /// A double, as its 64 bits, to the nearest single.
    CvtF32F64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bin {
    Add,
    Sub,
    Mul,
    MulHiU,
    And,
    Or,
    Xor,
    Shl,
    ShrU,
    ShrS,
    MinU,
    MaxU,
    MinS,
    MaxS,
    FAdd,
    FSub,
    FMul,
    FMin,
    FMax,
    Add64,
    /// Double-precision add on 64-bit patterns.
    DAdd,
    Sub64,
    And64,
    Or64,
    Xor64,
    Shl64,
    ShrU64,
    Pack64,
    LaneBit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tri {
    FFma,
    FMad,
    BfeU,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmp {
    Eq,
    Ne,
    LtU,
    LeU,
    GtU,
    GeU,
    LtS,
    LeS,
    GtS,
    GeS,
    Eq64,
    Ne64,
    FLt,
    FEq,
    FLe,
    FGt,
    FLg,
    FGe,
    FO,
    FU,
    FNge,
    FNlg,
    FNgt,
    FNle,
    FNeq,
    FNlt,
    False,
    True,
    Class,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AtomicOp {
    Swap,
    CompareSwap,
    Add,
    Sub,
    MinS,
    MinU,
    MaxS,
    MaxU,
    And,
    Or,
    Xor,
}

impl AtomicOp {
    /// The new word, from the old one, the operand and (for compare-swap) the expected value.
    pub fn apply(self, old: u32, value: u32, compare: u32) -> u32 {
        match self {
            AtomicOp::Swap => value,
            AtomicOp::CompareSwap => {
                if old == compare {
                    value
                } else {
                    old
                }
            }
            AtomicOp::Add => old.wrapping_add(value),
            AtomicOp::Sub => old.wrapping_sub(value),
            AtomicOp::MinS => (old as i32).min(value as i32) as u32,
            AtomicOp::MinU => old.min(value),
            AtomicOp::MaxS => (old as i32).max(value as i32) as u32,
            AtomicOp::MaxU => old.max(value),
            AtomicOp::And => old & value,
            AtomicOp::Or => old | value,
            AtomicOp::Xor => old ^ value,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Space {
    Lds,
    Memory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageDim {
    D1,
    D2,
    D3,
    Cube,
    D1Array,
    D2Array,
}

impl ImageDim {
    /// Address components before any LOD or mip: x, then y, then z, layer or cube face.
    pub fn coordinates(self) -> usize {
        match self {
            ImageDim::D1 => 1,
            ImageDim::D2 | ImageDim::D1Array => 2,
            ImageDim::D3 | ImageDim::Cube | ImageDim::D2Array => 3,
        }
    }

    /// Derivatives per screen direction for gradient sampling; a cube's are on its face.
    pub fn gradients(self) -> usize {
        match self {
            ImageDim::D1 | ImageDim::D1Array => 1,
            ImageDim::D2 | ImageDim::D2Array | ImageDim::Cube => 2,
            ImageDim::D3 => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageOp {
    Load,
    LoadMip,
    Sample,
    SampleBias,
    SampleLod,
    SampleLodZero,
    SampleGrad,
    Gather {
        channel: u8,
    },
    Size,
    QueryLod,
    Store,
    /// Updates one texel of a storage image for each active lane; `data` holds one word, or two for 64 bits.
    Atomic(AtomicOp),
}

impl ImageOp {
    /// Whether the level of detail comes from screen-space derivatives, which only pixel shaders have.
    pub fn implicit(self) -> bool {
        matches!(
            self,
            ImageOp::Sample | ImageOp::SampleBias | ImageOp::QueryLod
        )
    }
}

/// One result component of an image instruction. Every component of one instruction shares all other fields.
/// `lod` is the explicit level, bias or mip; `gradients` lists the horizontal derivatives, then the vertical
/// ones. With `d16`, each component is a half (or the low 16 bits of an integer) in its low bits.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub op: ImageOp,
    pub dim: ImageDim,
    pub resource: [Tmp; 8],
    pub sampler: Option<[Tmp; 4]>,
    pub coordinates: Vec<Tmp>,
    pub lod: Option<Tmp>,
    pub compare: Option<Tmp>,
    pub gradients: Vec<Tmp>,
    pub clamp: Option<Tmp>,
    pub offset: Option<Tmp>,
    pub data: Vec<Tmp>,
    pub unnormalized: bool,
    pub d16: bool,
    pub component: u8,
}

impl Image {
    pub fn operands(&self) -> Vec<Tmp> {
        self.resource
            .iter()
            .chain(self.sampler.iter().flatten())
            .chain(&self.coordinates)
            .chain(&self.lod)
            .chain(&self.compare)
            .chain(&self.gradients)
            .chain(&self.clamp)
            .chain(&self.offset)
            .chain(&self.data)
            .copied()
            .collect()
    }

    pub fn same_instruction(&self, other: &Image) -> bool {
        Image {
            component: other.component,
            ..self.clone()
        } == *other
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Const(u32),
    Const64(u64),
    Get(Reg),
    GetPair(u8),
    Un(Un, Tmp),
    Bin(Bin, Tmp, Tmp),
    Tri(Tri, Tmp, Tmp, Tmp),
    Cmp(Cmp, Tmp, Tmp),
    Select(Tmp, Tmp, Tmp),
    LaneId,
    CodeBase,
    Active,
    Ballot(Tmp),
    ReadLane(Tmp, Tmp),
    Shuffle(Tmp, Tmp),
    GetRelative(u8, Tmp),
    FirstActiveLane,
    LdsLoad(Tmp),
    Load {
        when: Option<Tmp>,
        address: Tmp,
    },
    ScalarLoad(Tmp),
    TypedLoad {
        format: Tmp,
        address: Tmp,
        in_range: Tmp,
        select: Option<Tmp>,
        component: u8,
    },
    Image(Box<Image>),
    Attribute {
        attr: u8,
        channel: u8,
        vertex: u8,
    },
    /// Updates one aligned word for each active lane, in lane order, and yields the word it found.
    Atomic {
        space: Space,
        op: AtomicOp,
        address: Tmp,
        value: Tmp,
        compare: Option<Tmp>,
        when: Option<Tmp>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Let(Tmp, Expr),
    Set(Reg, Tmp),
    SetPair(u8, Tmp),
    SetLane(u8, Tmp),
    /// Writes VGPR `base + index` in each active lane; nothing when that is past v255.
    SetRelative(u8, Tmp, Tmp),
    WriteLane(u8, Tmp, Tmp),
    LdsStore(Tmp, Tmp),
    /// `done` marks a pixel shader's last export, whose exec mask decides which pixels survive; `compressed`
    /// exports carry two registers of packed 16-bit pairs in the first two values.
    Export {
        target: u8,
        values: [Option<Tmp>; 4],
        done: bool,
        compressed: bool,
    },
    Message(u32, Tmp),
    FetchVertexInputs,
    /// `s_barrier`: every wave of the work group arrives before any goes on.
    Barrier,
    Store {
        when: Option<Tmp>,
        address: Tmp,
        value: Tmp,
    },
    /// Writes one element of the buffer format `format` at `address`, converting each channel's register value.
    TypedStore {
        format: Tmp,
        address: Tmp,
        in_range: Tmp,
        values: [Tmp; 4],
        d16: bool,
    },
    /// Writes a texel; `image.op` is `ImageOp::Store` and components without a value are written as zero.
    ImageStore {
        image: Box<Image>,
        values: [Option<Tmp>; 4],
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Terminator {
    Jump(BlockId),
    Branch(Tmp, BlockId, BlockId),
    End,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub pc: usize,
    pub stmts: Vec<Stmt>,
    pub terminator: Terminator,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub wave64: bool,
    pub blocks: Vec<Block>,
    pub temps: u32,
    /// Uses the hardware's ordered pixel shading: overlapping pixels must run one after another, in order.
    pub ordered: bool,
    /// A 64-lane program that reads or writes lanes 32-63 by number, so it needs those lanes even where the
    /// GPU runs 32 pixels or vertices together.
    pub upper_lanes: bool,
}

impl Expr {
    pub fn operands(&self) -> Vec<Tmp> {
        match self {
            Expr::Un(_, a) | Expr::Ballot(a) | Expr::LdsLoad(a) | Expr::ScalarLoad(a) => vec![*a],
            Expr::Bin(_, a, b)
            | Expr::Cmp(_, a, b)
            | Expr::ReadLane(a, b)
            | Expr::Shuffle(a, b) => vec![*a, *b],
            Expr::GetRelative(_, index) => vec![*index],
            Expr::Tri(_, a, b, c) | Expr::Select(a, b, c) => vec![*a, *b, *c],
            Expr::Load { when, address } => when.iter().copied().chain([*address]).collect(),
            Expr::TypedLoad {
                format,
                address,
                in_range,
                select,
                ..
            } => [*format, *address, *in_range]
                .into_iter()
                .chain(*select)
                .collect(),
            Expr::Image(image) => image.operands(),
            Expr::Atomic {
                address,
                value,
                compare,
                when,
                ..
            } => [*address, *value]
                .into_iter()
                .chain(*compare)
                .chain(*when)
                .collect(),
            _ => Vec::new(),
        }
    }

    pub fn per_lane(&self) -> bool {
        matches!(
            self,
            Expr::LaneId
                | Expr::Get(Reg::V(_))
                | Expr::Load { .. }
                | Expr::LdsLoad(_)
                | Expr::TypedLoad { .. }
                | Expr::Image(_)
                | Expr::Attribute { .. }
                | Expr::Atomic { .. }
                | Expr::Shuffle(..)
                | Expr::GetRelative(..)
        )
    }

    pub fn wave_wide(&self) -> bool {
        matches!(
            self,
            Expr::Ballot(_)
                | Expr::ReadLane(..)
                | Expr::FirstActiveLane
                | Expr::Active
                | Expr::ScalarLoad(_)
        )
    }
}

impl Function {
    pub fn varying(&self) -> Vec<bool> {
        let mut varying = vec![false; self.temps as usize];
        for block in &self.blocks {
            for stmt in &block.stmts {
                if let Stmt::Let(tmp, expr) = stmt {
                    varying[*tmp as usize] = expr.per_lane()
                        || (!expr.wave_wide()
                            && expr.operands().iter().any(|t| varying[*t as usize]));
                }
            }
        }
        varying
    }
}

impl fmt::Display for Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (id, block) in self.blocks.iter().enumerate() {
            writeln!(f, "b{id} (pc 0x{:x}):", block.pc)?;
            for stmt in &block.stmts {
                writeln!(f, "  {stmt:?}")?;
            }
            writeln!(f, "  {:?}", block.terminator)?;
        }
        Ok(())
    }
}
