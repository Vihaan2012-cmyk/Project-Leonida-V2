use ash::vk;

use crate::{FirstStage, Gpu, GpuError, Scratch, Target, TextureFormat};

/// The kind of one set-1 binding in a pipeline check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingKind {
    Buffer,
    Samplers,
    Textures,
    StorageTextures,
}

/// A pipeline to build in a check: a compute shader, or a draw's stages and attachment formats.
pub enum Check<'a> {
    Compute {
        spirv: &'a [u32],
        subgroup_size: Option<u32>,
    },
    Draw {
        first: FirstStage<'a>,
        fragment: &'a [u32],
        targets: &'a [TextureFormat],
        depth: Option<TextureFormat>,
    },
}

impl Gpu {
    /// Builds the pipeline a shader would run in without running it: the driver's compile is the check and, with
    /// validation on, the layers check the SPIR-V. Set 1 holds `bindings` as (binding, kind, count). Nothing is
    /// submitted, so checks may run on several threads at once.
    pub fn check(
        &self,
        work: Check<'_>,
        bindings: &[(u32, BindingKind, u32)],
        push_bytes: u32,
    ) -> Result<(), GpuError> {
        let mut scratch = Scratch::new(&self.device);
        scratch.wait = false;
        let stages = vk::ShaderStageFlags::ALL;
        // SAFETY: every handle is created from `self.device` and recorded in `scratch`, which destroys them.
        unsafe {
            let device = &self.device;
            scratch.set_layout = device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default(),
                None,
            )?;
            let list: Vec<_> = bindings
                .iter()
                .map(|(binding, kind, count)| {
                    let kind = match kind {
                        BindingKind::Buffer => vk::DescriptorType::STORAGE_BUFFER,
                        BindingKind::Samplers => vk::DescriptorType::SAMPLER,
                        BindingKind::Textures => vk::DescriptorType::SAMPLED_IMAGE,
                        BindingKind::StorageTextures => vk::DescriptorType::STORAGE_IMAGE,
                    };
                    vk::DescriptorSetLayoutBinding::default()
                        .binding(*binding)
                        .descriptor_type(kind)
                        .descriptor_count(*count)
                        .stage_flags(stages)
                })
                .collect();
            scratch.extra_layout = device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&list),
                None,
            )?;
            let set_layouts = [scratch.set_layout, scratch.extra_layout];
            let ranges = [vk::PushConstantRange::default()
                .stage_flags(stages)
                .size(push_bytes)];
            scratch.pipeline_layout = device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&set_layouts)
                    .push_constant_ranges(&ranges),
                None,
            )?;
            match work {
                Check::Compute {
                    spirv,
                    subgroup_size,
                } => self.compute_pipeline(&mut scratch, spirv, subgroup_size)?,
                Check::Draw {
                    first,
                    fragment,
                    targets,
                    depth,
                } => {
                    let blank = |format: TextureFormat| Target {
                        format,
                        clear: [0; 4],
                        data: Vec::new(),
                    };
                    let targets: Vec<Target> = targets.iter().map(|f| blank(*f)).collect();
                    let mut depth = depth.map(blank);
                    self.graphics_pipeline(
                        &mut scratch,
                        &first,
                        fragment,
                        [1, 1],
                        &targets,
                        &depth.as_mut(),
                    )?;
                }
            }
        }
        Ok(())
    }
}
