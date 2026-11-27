//! Pipeline state objects built from the console shader bytecode through the translator.

pub mod cache;
pub mod shaders;
pub mod state;

use ash::vk;

use crate::root::RootError;

pub use cache::PipelineCache;
pub use shaders::{DirectTranslation, Link, ShaderSource, TranslatedStage};
pub use state::{
    BlendState, DepthStencilState, InputElement, RasterizerState, RenderTargetBlend, StencilFace,
    TopologyType, dxgi_to_vk, format_bytes,
};

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("the shader bytecode is not usable: {0}")]
    Bytecode(String),
    #[error("a {found} shader where a {wanted} shader is needed")]
    WrongStage { wanted: String, found: String },
    #[error("the shader could not be translated: {0}")]
    Translate(String),
    #[error("not supported: {0}")]
    Unsupported(&'static str),
    #[error("the pipeline description is invalid: {0}")]
    Invalid(&'static str),
    #[error("this GPU lacks {0}")]
    NoFeature(&'static str),
    #[error("DXGI format {0} is not one a pipeline can use here")]
    Format(u32),
    #[error("{0}")]
    CodeBase(&'static str),
    #[error("root signature: {0}")]
    Root(#[from] RootError),
    #[error("Vulkan: {0}")]
    Vulkan(#[from] vk::Result),
    #[error("pipeline cache: {0}")]
    Cache(String),
    #[error("shader code upload failed: {0}")]
    Upload(String),
}
