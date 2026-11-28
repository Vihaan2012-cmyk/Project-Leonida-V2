//! Pipeline creation off the title's thread: the device's `PipelineEnv` behind a `PipelineService`.
//!
//! Device slots 10, 11, 62 and 86 hand the description to the service and return the pipeline object at once; the
//! object holds the [`PipelineHandle`]. The command-list conversion asks the handle when a draw or dispatch needs
//! the pipeline and either waits for the build or drops the draw, by the draw policy.

use std::path::PathBuf;
use std::sync::Arc;

use crate::com;
use crate::compile_queue::{
    BytecodeStore, DirBytecode, DrawPolicy, PipelineBuilder, PipelineHandle, PipelineService,
    PsoKey, Resolved,
};
use crate::pipeline::{ComputePsoDesc, GraphicsPsoDesc, Pipeline, PipelineError};

/// What the service builds with: the device's `PipelineEnv`, or something that wraps it.
pub type SharedBuilder = Arc<dyn PipelineBuilder<Built = Arc<Pipeline>>>;

/// [`SharedBuilder`] as a builder in its own right, which notes each failure when it happens: the title has
/// already been told the pipeline exists.
pub struct DeviceBuilder(pub SharedBuilder);

impl PipelineBuilder for DeviceBuilder {
    type Built = Arc<Pipeline>;

    fn graphics(&self, desc: &GraphicsPsoDesc) -> Result<Arc<Pipeline>, PipelineError> {
        self.0.graphics(desc).inspect_err(|e| {
            com::note(format!("a graphics pipeline failed: {e}"));
        })
    }

    fn compute(&self, desc: &ComputePsoDesc) -> Result<Arc<Pipeline>, PipelineError> {
        self.0.compute(desc).inspect_err(|e| {
            com::note(format!("a compute pipeline failed: {e}"));
        })
    }

    fn cached_blob(&self, key: &PsoKey) -> Vec<u8> {
        self.0.cached_blob(key)
    }

    fn save_cache(&self) -> Result<(), PipelineError> {
        self.0.save_cache()
    }
}

pub type Service = PipelineService<DeviceBuilder>;
pub type Handle = PipelineHandle<DeviceBuilder>;

/// Where `pipelines-seen` is kept and what holds the shader bytes its records refer to.
#[derive(Clone)]
pub struct PipelinesSeen {
    pub path: PathBuf,
    pub bytecode: Arc<dyn BytecodeStore>,
}

impl std::fmt::Debug for PipelinesSeen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PipelinesSeen")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl PipelinesSeen {
    /// The log at `path`, with the shaders kept in a folder beside it named `<path>.shaders`.
    pub fn beside(path: impl Into<PathBuf>) -> PipelinesSeen {
        let path = path.into();
        let mut folder = path.clone().into_os_string();
        folder.push(".shaders");
        PipelinesSeen {
            bytecode: Arc::new(DirBytecode::new(folder)),
            path,
        }
    }
}

/// What the replay of a command list needs of a pipeline the title set.
pub trait PendingPipeline: Send + Sync {
    /// A compute pipeline, bound for dispatches.
    fn compute(&self) -> bool;
    /// The built pipeline, by the draw policy: waits for the build or says it is not ready.
    fn resolve(&self) -> Resolved<Arc<Pipeline>>;
    /// The built pipeline if it is, without waiting whatever the policy; a draw that finds it not ready is counted
    /// as skipped.
    fn poll(&self) -> Resolved<Arc<Pipeline>>;
}

/// A pipeline the title created: its handle and the bind point it is for.
pub struct Created {
    pub handle: Handle,
    pub compute: bool,
}

impl PendingPipeline for Created {
    fn compute(&self) -> bool {
        self.compute
    }

    fn resolve(&self) -> Resolved<Arc<Pipeline>> {
        self.handle.resolve()
    }

    fn poll(&self) -> Resolved<Arc<Pipeline>> {
        self.handle.resolve_with(DrawPolicy::Skip)
    }
}

/// How the device starts its service.
pub fn start(
    seen: Option<&PipelinesSeen>,
    workers: Option<usize>,
    policy: DrawPolicy,
    builder: SharedBuilder,
) -> Service {
    let mut config = crate::compile_queue::ServiceConfig {
        draw_policy: policy,
        seen: seen.map(|s| (s.path.clone(), s.bytecode.clone())),
        ..Default::default()
    };
    let from_env = std::env::var("LEONIDA_PIPELINE_WORKERS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok());
    if let Some(workers) = from_env.or(workers) {
        config.workers = workers.max(1);
    }
    PipelineService::start(config, move || Ok(DeviceBuilder(builder)))
}
