//! Translated shaders, kept in a `redb` database so each game shader is translated once.
//!
//! The runtime asks [`Store::program`] with a program's FSSM bytes and gets the stored or freshly translated
//! program; `leonida pipelines` fills the store ahead of time.

mod error;
mod program;
mod record;
mod runtime;
mod store;
pub mod translate;

pub use error::StoreError;
pub use program::{
    AttributeLocations, Failure, Key, Source, Translated, container_programs, key_hex, key_of,
    reads_own_address, record_of, translate_built, translate_linked, translate_program,
    translate_with_facts, uses_code_base,
};
pub use record::{
    Exactness, Options, PipelineOutcome, Record, Role, SCHEMA_VERSION, stage_kind,
    translator_version,
};
pub use runtime::{SourceStats, StoreSource};
pub use store::{Program, Store};
pub use translate::Built;
