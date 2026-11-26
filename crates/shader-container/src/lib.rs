//! Shader containers in `shaders.rpf`: carving records, DXBC parts, XDAT/XBCH unpacking and FSSM headers.

mod dxbc;
mod error;
#[cfg(any(test, feature = "test-support"))]
pub mod testutil;
mod xdat;

pub use dxbc::{Part, ShaderModel, Stage, find_part, parts, shader_model};
pub use error::ContainerError;
pub use xdat::{MAX_FSSM_BYTES, XBCH_TAG, inflate_xbch, unpack_all, xdat_entries};
