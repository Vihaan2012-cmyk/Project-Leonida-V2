//! Shader containers in `shaders.rpf`: carving records, DXBC parts, XDAT/XBCH unpacking and FSSM headers.

mod dxbc;
mod error;
#[cfg(any(test, feature = "test-support"))]
pub mod testutil;

pub use dxbc::{Part, ShaderModel, Stage, find_part, parts, shader_model};
pub use error::ContainerError;
