//! How the passes run: a compute backend that takes SPIR-V and storage buffers.

use std::sync::Arc;

use crate::UpscaleError;

/// Runs one compute dispatch. Buffers are bound at set 0, bindings 0.. in order, and their contents are read back
/// after the dispatch. The renderer can implement this over its own persistent resources.
pub trait Compute: Send + Sync {
    fn dispatch(
        &self,
        spirv: &[u32],
        buffers: &mut [&mut [u32]],
        groups: [u32; 3],
    ) -> Result<(), UpscaleError>;
}

impl Compute for gpu::Gpu {
    fn dispatch(
        &self,
        spirv: &[u32],
        buffers: &mut [&mut [u32]],
        groups: [u32; 3],
    ) -> Result<(), UpscaleError> {
        gpu::Gpu::dispatch(self, spirv, buffers, groups, None).map_err(UpscaleError::Gpu)
    }
}

pub type SharedCompute = Arc<dyn Compute>;

/// Workgroup counts for an 8x8 pass over `width` x `height`.
pub(crate) fn groups_for(width: u32, height: u32) -> [u32; 3] {
    [width.div_ceil(8), height.div_ceil(8), 1]
}

pub(crate) fn words(values: &[f32]) -> Vec<u32> {
    values.iter().map(|v| v.to_bits()).collect()
}

pub(crate) fn floats(words: &[u32]) -> Vec<f32> {
    words.iter().map(|w| f32::from_bits(*w)).collect()
}
