//! Where graphics and compute shaders read their root arguments: which SGPRs a program reads before writing
//! them, which it uses as pointers, descriptor bases or code-base arithmetic, and what its container header
//! says about its user SGPR count.

mod analysis;
mod pairs;
mod program;
mod summary;

pub use analysis::{Analysis, analyze};
pub use pairs::{ShiftCount, StageHashes, StagePair, Techniques, technique};
pub use program::{
    Program, RegSet, SFI0_RESOURCE_HEAP, SFI0_SAMPLER_HEAP, Selection, Use, census_archive,
    census_blob, census_container,
};
pub use summary::{Census, SgprRow, StageSummary, group_name};
