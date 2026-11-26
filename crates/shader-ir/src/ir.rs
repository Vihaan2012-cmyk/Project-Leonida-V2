use std::fmt;

pub type Tmp = u32;
pub type BlockId = usize;

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
    Active,
    Ballot(Tmp),
    ReadLane(Tmp, Tmp),
    FirstActiveLane,
    LdsLoad(Tmp),
    Load { when: Option<Tmp>, address: Tmp },
    ScalarLoad(Tmp),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Let(Tmp, Expr),
    Set(Reg, Tmp),
    SetPair(u8, Tmp),
    SetLane(u8, Tmp),
    WriteLane(u8, Tmp, Tmp),
    LdsStore(Tmp, Tmp),
    Store {
        when: Option<Tmp>,
        address: Tmp,
        value: Tmp,
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
}

impl Expr {
    pub fn operands(&self) -> Vec<Tmp> {
        match self {
            Expr::Un(_, a) | Expr::Ballot(a) | Expr::LdsLoad(a) | Expr::ScalarLoad(a) => vec![*a],
            Expr::Bin(_, a, b) | Expr::Cmp(_, a, b) | Expr::ReadLane(a, b) => vec![*a, *b],
            Expr::Tri(_, a, b, c) | Expr::Select(a, b, c) => vec![*a, *b, *c],
            Expr::Load { when, address } => when.iter().copied().chain([*address]).collect(),
            _ => Vec::new(),
        }
    }

    pub fn per_lane(&self) -> bool {
        matches!(
            self,
            Expr::LaneId | Expr::Get(Reg::V(_)) | Expr::Load { .. } | Expr::LdsLoad(_)
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
