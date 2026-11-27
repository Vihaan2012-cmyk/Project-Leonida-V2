mod interp;
mod ir;
mod lift;
mod verify;

pub use interp::{BLOCK_LIMIT, EXPORT_WORDS, Machine, RunError, export_slot, run};
pub use ir::{
    ATTRIBUTES, Bin, Block, BlockId, Cmp, Expr, Function, Image, ImageDim, ImageOp, Reg, Round,
    Stmt, Terminator, Tmp, Tri, Un, attribute_slot,
};
pub use lift::{Coverage, LiftError, coverage, export_target, lift};
pub use verify::{Ty, VerifyError};
