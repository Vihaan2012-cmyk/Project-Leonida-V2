//! Shader containers in `shaders.rpf`: carving records, DXBC parts, XDAT/XBCH unpacking and FSSM headers.

mod carve;
mod dxbc;
mod error;
mod fssm;
mod link;
pub mod scan;
mod signature;
#[cfg(any(test, feature = "test-support"))]
pub mod testutil;
mod xdat;

pub use carve::{CarveStats, CarvedShader, carve};
pub use dxbc::{Part, ShaderModel, Stage, find_part, parts, shader_model};
pub use error::ContainerError;
pub use fssm::{CodeRange, ENDPGM, Fssm, Interpolant, ParameterExport, PixelSetup, WaveSize};
pub use link::{pixel_attribute_sources, register_exports};
pub use scan::{ScanResult, ScanSummary, ShaderInfo};
pub use signature::{
    GeometryInfo, Interface, MeshInfo, PipelineInfo, SignatureElement, interface, pipeline_info,
    signature,
};
pub use xdat::{MAX_FSSM_BYTES, XBCH_TAG, inflate_xbch, unpack_all, xdat_entries};
