mod interp;
mod ir;
mod lift;
mod verify;

pub use interp::{BLOCK_LIMIT, Machine, RunError, run};
pub use ir::{Bin, Block, BlockId, Cmp, Expr, Function, Reg, Stmt, Terminator, Tmp, Tri, Un};
pub use lift::{LiftError, lift};
pub use verify::{Ty, VerifyError};
