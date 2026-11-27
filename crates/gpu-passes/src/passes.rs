use ash::vk;

use crate::PassError;
use crate::swizzle::Job;

static SURFACE_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/surface.spv"));
static BC1_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/bc1.spv"));

/// What the caller's next use of the output needs: made visible after the dispatches.
#[derive(Debug, Clone, Copy)]
pub struct Barrier {
    pub stage: vk::PipelineStageFlags,
    pub access: vk::AccessFlags,
}

impl Barrier {
    /// The output is copied to an image next.
    pub const TRANSFER_READ: Barrier = Barrier {
        stage: vk::PipelineStageFlags::TRANSFER,
        access: vk::AccessFlags::TRANSFER_READ,
    };
    /// The output is read by another compute dispatch.
    pub const COMPUTE_READ: Barrier = Barrier {
        stage: vk::PipelineStageFlags::COMPUTE_SHADER,
        access: vk::AccessFlags::SHADER_READ,
    };
    /// The output is read back by the host once the submission's fence signals.
    pub const HOST_READ: Barrier = Barrier {
        stage: vk::PipelineStageFlags::HOST,
        access: vk::AccessFlags::HOST_READ,
    };
}

struct Kernel {
    set_layout: vk::DescriptorSetLayout,
    layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    bindings: u32,
}

impl Kernel {
    fn new(
        device: &ash::Device,
        spirv: &[u8],
        bindings: u32,
        push_bytes: u32,
    ) -> Result<Kernel, PassError> {
        let words: Vec<u32> = spirv
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        let entries: Vec<_> = (0..bindings)
            .map(|binding| {
                vk::DescriptorSetLayoutBinding::default()
                    .binding(binding)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::COMPUTE)
            })
            .collect();
        let ranges = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .size(push_bytes)];
        // SAFETY: every handle is created from `device` and owned by the returned kernel, which `Passes`
        // destroys; the SPIR-V is the build script's output, a whole number of words.
        unsafe {
            let set_layout = device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&entries),
                None,
            )?;
            let layouts = [set_layout];
            let layout = device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&layouts)
                    .push_constant_ranges(&ranges),
                None,
            )?;
            let module = device
                .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&words), None)?;
            let stage = vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::COMPUTE)
                .module(module)
                .name(c"main");
            let info = vk::ComputePipelineCreateInfo::default()
                .stage(stage)
                .layout(layout);
            let made = device.create_compute_pipelines(vk::PipelineCache::null(), &[info], None);
            device.destroy_shader_module(module, None);
            let pipeline = made.map_err(|(_, error)| error)?[0];
            Ok(Kernel {
                set_layout,
                layout,
                pipeline,
                bindings,
            })
        }
    }

    fn destroy(&self, device: &ash::Device) {
        // SAFETY: the handles were created from `device` and the caller guarantees no work uses them.
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_descriptor_set_layout(self.set_layout, None);
        }
    }
}

/// The two compute pipelines (tiling in both directions, BC1 rebuild) and the descriptor pool they share.
///
/// Drop it before the device is destroyed, and not while recorded work that uses it is still pending.
pub struct Passes {
    device: ash::Device,
    pool: vk::DescriptorPool,
    surface: Kernel,
    bc1: Kernel,
}

/// The four storage buffers of a BC1 rebuild.
#[derive(Debug, Clone, Copy)]
pub struct Bc1Buffers {
    pub lanes: vk::Buffer,
    /// [`crate::bc1::Bc1Tables::pack`].
    pub tables: vk::Buffer,
    pub out: vk::Buffer,
    /// Two words, zero before the dispatch.
    pub status: vk::Buffer,
}

/// One BC1 rebuild: `blocks` blocks from lanes packed by [`crate::bc1::Lanes::pack`].
#[derive(Debug, Clone, Copy)]
pub struct Bc1Job {
    pub blocks: u32,
    pub lane_stride: u32,
    /// Longest history chain a block may follow before it is counted as failed (`status[1]`).
    pub max_chain: u32,
    /// Byte offsets (multiples of 4) of the lanes within the lanes buffer and of the output within the output.
    pub lanes_offset: u64,
    pub out_offset: u64,
}

const GROUP: u32 = 64;
const MAX_GROUPS_X: u32 = 32768;

fn grid(items: u64) -> Result<[u32; 3], PassError> {
    let groups = items.div_ceil(GROUP as u64);
    let x = groups.clamp(1, MAX_GROUPS_X as u64);
    let y = groups.div_ceil(x).max(1);
    if y > 65535 {
        return Err(PassError::TooLarge);
    }
    Ok([x as u32, y as u32, 1])
}

fn bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_ne_bytes()).collect()
}

impl Passes {
    /// `max_sets` bounds the dispatch groups recorded between two [`Passes::reset`] calls (one set per
    /// `record_*` call).
    pub fn new(device: &ash::Device, max_sets: u32) -> Result<Passes, PassError> {
        let surface = Kernel::new(device, SURFACE_SPV, 2, 64)?;
        let bc1 = match Kernel::new(device, BC1_SPV, 4, 32) {
            Ok(kernel) => kernel,
            Err(error) => {
                surface.destroy(device);
                return Err(error);
            }
        };
        let sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(max_sets.max(1) * 4)];
        // SAFETY: a fresh pool on `device`, destroyed in Drop.
        let pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(max_sets.max(1))
                    .pool_sizes(&sizes),
                None,
            )
        };
        match pool {
            Ok(pool) => Ok(Passes {
                device: device.clone(),
                pool,
                surface,
                bc1,
            }),
            Err(error) => {
                surface.destroy(device);
                bc1.destroy(device);
                Err(error.into())
            }
        }
    }

    /// Returns every descriptor set to the pool. Call once the work recorded since the last reset has finished.
    pub fn reset(&self) -> Result<(), PassError> {
        // SAFETY: the pool belongs to `self.device`; the caller guarantees no recorded work still uses its sets.
        unsafe {
            self.device
                .reset_descriptor_pool(self.pool, vk::DescriptorPoolResetFlags::empty())?
        };
        Ok(())
    }

    /// Allocates a set for `kernel` and points its bindings at `buffers` (whole buffers).
    unsafe fn bind(
        &self,
        cmd: vk::CommandBuffer,
        kernel: &Kernel,
        buffers: &[vk::Buffer],
    ) -> Result<(), PassError> {
        debug_assert_eq!(buffers.len() as u32, kernel.bindings);
        let layouts = [kernel.set_layout];
        // SAFETY: the set comes from this object's pool and layout; the writes cover bindings the layout declares;
        // the caller of the `record_*` function guarantees `cmd` is recording and the buffers are valid.
        unsafe {
            let set =
                self.device
                    .allocate_descriptor_sets(
                        &vk::DescriptorSetAllocateInfo::default()
                            .descriptor_pool(self.pool)
                            .set_layouts(&layouts),
                    )
                    .map_err(|error| match error {
                        vk::Result::ERROR_OUT_OF_POOL_MEMORY
                        | vk::Result::ERROR_FRAGMENTED_POOL => PassError::PoolExhausted,
                        other => PassError::Vulkan(other),
                    })?[0];
            let infos: Vec<[vk::DescriptorBufferInfo; 1]> = buffers
                .iter()
                .map(|b| {
                    [vk::DescriptorBufferInfo::default()
                        .buffer(*b)
                        .range(vk::WHOLE_SIZE)]
                })
                .collect();
            let writes: Vec<_> = infos
                .iter()
                .enumerate()
                .map(|(binding, info)| {
                    vk::WriteDescriptorSet::default()
                        .dst_set(set)
                        .dst_binding(binding as u32)
                        .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                        .buffer_info(info)
                })
                .collect();
            self.device.update_descriptor_sets(&writes, &[]);
            self.device
                .cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, kernel.pipeline);
            self.device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::COMPUTE,
                kernel.layout,
                0,
                &[set],
                &[],
            );
        }
        Ok(())
    }

    unsafe fn finish(&self, cmd: vk::CommandBuffer, after: Barrier) {
        let barrier = [vk::MemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::SHADER_WRITE)
            .dst_access_mask(after.access)];
        // SAFETY: `cmd` is recording (the caller's promise).
        unsafe {
            self.device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                after.stage,
                vk::DependencyFlags::empty(),
                &barrier,
                &[],
                &[],
            );
        }
    }

    unsafe fn surface(
        &self,
        cmd: vk::CommandBuffer,
        tiled: vk::Buffer,
        linear: vk::Buffer,
        jobs: &[Job],
        to_tiled: bool,
        after: Barrier,
    ) -> Result<(), PassError> {
        let mut plan = Vec::with_capacity(jobs.len());
        for job in jobs {
            job.validate()?;
            let eq = job.equation()?;
            let count = if to_tiled {
                job.tiled_bytes()? / 4
            } else {
                job.linear_bytes().div_ceil(4)
            };
            let pat = eq.pack();
            let push = [
                to_tiled as u32,
                (job.tiled_offset / 4) as u32,
                (job.linear_offset / 4) as u32,
                job.extent[0],
                job.extent[1],
                job.extent[2],
                job.origin[0],
                job.origin[1],
                job.tiles[0],
                job.tiles[1],
                eq.shape(),
                count as u32,
                pat[0],
                pat[1],
                pat[2],
                pat[3],
            ];
            plan.push((bytes(&push), grid(count)?));
        }
        // SAFETY: forwarded from the caller's contract.
        unsafe {
            self.bind(cmd, &self.surface, &[tiled, linear])?;
            for (push, [x, y, z]) in plan {
                self.device.cmd_push_constants(
                    cmd,
                    self.surface.layout,
                    vk::ShaderStageFlags::COMPUTE,
                    0,
                    &push,
                );
                self.device.cmd_dispatch(cmd, x, y, z);
            }
            self.finish(cmd, after);
        }
        Ok(())
    }

    /// Records the detile of `jobs`: each reads its mip from `tiled` and writes it, row-major and tight, into
    /// `linear` at the job's `linear_offset`. Jobs from [`crate::layout::Layout`] write disjoint ranges.
    ///
    /// # Safety
    /// `cmd` must be recording on this object's device; both buffers must be valid storage buffers, large
    /// enough for the jobs, and not be written by other work during the dispatches. Earlier writes to `tiled`
    /// must already be made visible to compute shaders.
    pub unsafe fn record_detile(
        &self,
        cmd: vk::CommandBuffer,
        tiled: vk::Buffer,
        linear: vk::Buffer,
        jobs: &[Job],
        after: Barrier,
    ) -> Result<(), PassError> {
        // SAFETY: forwarded from the caller's contract.
        unsafe { self.surface(cmd, tiled, linear, jobs, false, after) }
    }

    /// Records the reverse: each job reads `linear` and writes its mip's whole tiled region in `tiled`
    /// (see [`crate::swizzle::tile`] for exactly which words). Same contract as `record_detile`.
    ///
    /// # Safety
    /// As for [`Passes::record_detile`].
    pub unsafe fn record_tile(
        &self,
        cmd: vk::CommandBuffer,
        tiled: vk::Buffer,
        linear: vk::Buffer,
        jobs: &[Job],
        after: Barrier,
    ) -> Result<(), PassError> {
        // SAFETY: forwarded from the caller's contract.
        unsafe { self.surface(cmd, tiled, linear, jobs, true, after) }
    }

    /// Records the BC1 rebuild. `tables` holds [`crate::bc1::Bc1Tables::pack`]; `status` is two zeroed words
    /// the kernel adds to: blocks that hit an unrecovered history entry, blocks whose history chain was longer
    /// than `max_chain`. Blocks counted there are wrong and need the CPU path.
    ///
    /// # Safety
    /// As for [`Passes::record_detile`], for the four buffers; `status` must be zero-initialised.
    pub unsafe fn record_bc1(
        &self,
        cmd: vk::CommandBuffer,
        buffers: Bc1Buffers,
        job: Bc1Job,
        after: Barrier,
    ) -> Result<(), PassError> {
        let [x, y, z] = grid(job.blocks as u64)?;
        let push = bytes(&[
            job.blocks,
            job.lane_stride,
            job.max_chain,
            (job.lanes_offset / 4) as u32,
            (job.out_offset / 4) as u32,
            0,
            0,
            0,
        ]);
        // SAFETY: forwarded from the caller's contract.
        unsafe {
            self.bind(
                cmd,
                &self.bc1,
                &[buffers.lanes, buffers.tables, buffers.out, buffers.status],
            )?;
            self.device.cmd_push_constants(
                cmd,
                self.bc1.layout,
                vk::ShaderStageFlags::COMPUTE,
                0,
                &push,
            );
            self.device.cmd_dispatch(cmd, x, y, z);
            self.finish(cmd, after);
        }
        Ok(())
    }
}

impl Drop for Passes {
    fn drop(&mut self) {
        // SAFETY: the handles belong to `self.device`; the type's contract is that no recorded work uses them.
        unsafe {
            self.device.destroy_descriptor_pool(self.pool, None);
        }
        self.surface.destroy(&self.device);
        self.bc1.destroy(&self.device);
    }
}
