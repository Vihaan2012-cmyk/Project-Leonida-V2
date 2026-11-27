//! Translated shaders, kept in a `redb` database so each game shader is translated once.

mod error;
mod record;
pub mod translate;

pub use error::StoreError;
pub use record::{
    Exactness, Options, PipelineOutcome, Record, Role, SCHEMA_VERSION, stage_kind,
    translator_version,
};
pub use translate::Built;
