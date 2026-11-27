use std::any::Any;

use ash::vk;

use crate::buffer::Buffer;
use crate::device::{Device, Queue, SignalSemaphore, Submission, WaitSemaphore};
use crate::error::Result;
use crate::image::Image;
use crate::pipeline::{ComputePipeline, DescriptorSet, PipelineLayout};

type Keep = Box<dyn Any + Send + Sync>;

struct Pending {
    value: u64,
    raw: vk::CommandBuffer,
    _keep: Vec<Keep>,
}

/// A command pool for one queue and the command buffers recorded from it. Recording hands out an owned
/// [`CommandBuffer`]; `submit` runs it. Resources a command buffer names stay alive until the GPU finishes it.
///
/// Dropping the stream waits for its outstanding work.
pub struct CommandStream {
    queue: Queue,
    pool: vk::CommandPool,
    free: Vec<vk::CommandBuffer>,
    pending: Vec<Pending>,
}

impl CommandStream {
    pub fn new(queue: &Queue) -> Result<CommandStream> {
        let device = queue.device();
        // SAFETY: plain creation on a live device; the pool is destroyed in `Drop`.
        let pool = unsafe {
            device.raw().create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(device.queue_family())
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )?
        };
        Ok(CommandStream {
            queue: queue.clone(),
            pool,
            free: Vec::new(),
            pending: Vec::new(),
        })
    }

    pub fn queue(&self) -> &Queue {
        &self.queue
    }

    /// Starts recording a one-time-submit command buffer.
    pub fn begin(&mut self) -> Result<CommandBuffer> {
        self.reclaim()?;
        let device = self.queue.device();
        // SAFETY: the pool belongs to this stream, which is used from one thread at a time (`&mut self`).
        let raw = unsafe {
            let raw = match self.free.pop() {
                Some(raw) => raw,
                None => device.raw().allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(self.pool)
                        .level(vk::CommandBufferLevel::PRIMARY)
                        .command_buffer_count(1),
                )?[0],
            };
            device.raw().begin_command_buffer(
                raw,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            raw
        };
        Ok(CommandBuffer {
            device: device.clone(),
            raw,
            keep: Vec::new(),
        })
    }

    /// Ends and submits `cb`, which then runs after `waits` and signals `signals` and the queue's timeline.
    pub fn submit(
        &mut self,
        cb: CommandBuffer,
        waits: &[WaitSemaphore],
        signals: &[SignalSemaphore],
    ) -> Result<Submission> {
        // SAFETY: `cb` was recorded from this stream's pool and is ended exactly once, here.
        unsafe { self.queue.device().raw().end_command_buffer(cb.raw)? };
        let submission = self.queue.submit(&[cb.raw], waits, signals)?;
        self.pending.push(Pending {
            value: submission.value(),
            raw: cb.raw,
            _keep: cb.keep,
        });
        Ok(submission)
    }

    /// Recycles the command buffers the GPU has finished and releases the resources they named.
    pub fn reclaim(&mut self) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let done = self.queue.completed()?;
        let device = self.queue.device().clone();
        let mut index = 0;
        while index < self.pending.len() {
            if self.pending[index].value <= done {
                let pending = self.pending.remove(index);
                // SAFETY: the GPU finished this command buffer (its timeline value was reached).
                unsafe {
                    device
                        .raw()
                        .reset_command_buffer(pending.raw, vk::CommandBufferResetFlags::empty())?;
                }
                self.free.push(pending.raw);
            } else {
                index += 1;
            }
        }
        Ok(())
    }

    /// Blocks until everything submitted through this stream has finished.
    pub fn wait_idle(&mut self) -> Result<()> {
        if let Some(last) = self.pending.iter().map(|p| p.value).max() {
            self.queue.wait(last, u64::MAX)?;
        }
        self.reclaim()
    }
}

impl Drop for CommandStream {
    fn drop(&mut self) {
        let _ = self.wait_idle();
        // SAFETY: everything submitted through the pool has finished; destroying it frees its buffers.
        unsafe {
            self.queue
                .device()
                .raw()
                .destroy_command_pool(self.pool, None);
        }
    }
}

/// Parameters for [`CommandBuffer::image_barrier`].
#[derive(Debug, Clone, Copy)]
pub struct ImageBarrier {
    pub src_stage: vk::PipelineStageFlags2,
    pub src_access: vk::AccessFlags2,
    pub dst_stage: vk::PipelineStageFlags2,
    pub dst_access: vk::AccessFlags2,
    pub old_layout: vk::ImageLayout,
    pub new_layout: vk::ImageLayout,
    /// `None`: every mip and layer of the image's full aspect.
    pub range: Option<vk::ImageSubresourceRange>,
}

/// A command buffer being recorded. Every command that names a resource keeps it alive until the GPU is done.
pub struct CommandBuffer {
    device: Device,
    raw: vk::CommandBuffer,
    keep: Vec<Keep>,
}

impl CommandBuffer {
    pub fn device(&self) -> &Device {
        &self.device
    }

    /// The `VkCommandBuffer`, for calls this wrapper lacks. Pair with [`CommandBuffer::keep_alive`].
    pub fn raw(&self) -> vk::CommandBuffer {
        self.raw
    }

    /// Keeps `object` alive until the GPU finishes this command buffer.
    pub fn keep_alive<T: Any + Send + Sync>(&mut self, object: T) {
        self.keep.push(Box::new(object));
    }

    pub fn copy_buffer(&mut self, src: &Buffer, dst: &Buffer, regions: &[vk::BufferCopy]) {
        // SAFETY: recording into this command buffer; the buffers are kept alive below.
        unsafe {
            self.device
                .raw()
                .cmd_copy_buffer(self.raw, src.handle(), dst.handle(), regions);
        }
        self.keep.push(Box::new(src.clone()));
        self.keep.push(Box::new(dst.clone()));
    }

    pub fn copy_buffer_to_image(
        &mut self,
        src: &Buffer,
        dst: &Image,
        layout: vk::ImageLayout,
        regions: &[vk::BufferImageCopy],
    ) {
        // SAFETY: recording into this command buffer; the resources are kept alive below.
        unsafe {
            self.device.raw().cmd_copy_buffer_to_image(
                self.raw,
                src.handle(),
                dst.handle(),
                layout,
                regions,
            );
        }
        self.keep.push(Box::new(src.clone()));
        self.keep.push(Box::new(dst.clone()));
    }

    pub fn copy_image_to_buffer(
        &mut self,
        src: &Image,
        layout: vk::ImageLayout,
        dst: &Buffer,
        regions: &[vk::BufferImageCopy],
    ) {
        // SAFETY: recording into this command buffer; the resources are kept alive below.
        unsafe {
            self.device.raw().cmd_copy_image_to_buffer(
                self.raw,
                src.handle(),
                layout,
                dst.handle(),
                regions,
            );
        }
        self.keep.push(Box::new(src.clone()));
        self.keep.push(Box::new(dst.clone()));
    }

    pub fn copy_image(
        &mut self,
        src: &Image,
        src_layout: vk::ImageLayout,
        dst: &Image,
        dst_layout: vk::ImageLayout,
        regions: &[vk::ImageCopy],
    ) {
        // SAFETY: recording into this command buffer; the resources are kept alive below.
        unsafe {
            self.device.raw().cmd_copy_image(
                self.raw,
                src.handle(),
                src_layout,
                dst.handle(),
                dst_layout,
                regions,
            );
        }
        self.keep.push(Box::new(src.clone()));
        self.keep.push(Box::new(dst.clone()));
    }

    pub fn fill_buffer(&mut self, buffer: &Buffer, offset: u64, size: u64, value: u32) {
        // SAFETY: recording into this command buffer; the buffer is kept alive below.
        unsafe {
            self.device
                .raw()
                .cmd_fill_buffer(self.raw, buffer.handle(), offset, size, value);
        }
        self.keep.push(Box::new(buffer.clone()));
    }

    pub fn clear_color_image(
        &mut self,
        image: &Image,
        layout: vk::ImageLayout,
        color: vk::ClearColorValue,
        ranges: &[vk::ImageSubresourceRange],
    ) {
        // SAFETY: recording into this command buffer; the image is kept alive below.
        unsafe {
            self.device.raw().cmd_clear_color_image(
                self.raw,
                image.handle(),
                layout,
                &color,
                ranges,
            );
        }
        self.keep.push(Box::new(image.clone()));
    }

    pub fn memory_barrier(
        &mut self,
        src_stage: vk::PipelineStageFlags2,
        src_access: vk::AccessFlags2,
        dst_stage: vk::PipelineStageFlags2,
        dst_access: vk::AccessFlags2,
    ) {
        let barrier = [vk::MemoryBarrier2::default()
            .src_stage_mask(src_stage)
            .src_access_mask(src_access)
            .dst_stage_mask(dst_stage)
            .dst_access_mask(dst_access)];
        let info = vk::DependencyInfo::default().memory_barriers(&barrier);
        // SAFETY: recording into this command buffer.
        unsafe { self.device.raw().cmd_pipeline_barrier2(self.raw, &info) };
    }

    pub fn image_barrier(&mut self, image: &Image, barrier: &ImageBarrier) {
        let range = barrier
            .range
            .unwrap_or_else(|| image.full_range(image.format().aspect()));
        let barriers = [vk::ImageMemoryBarrier2::default()
            .src_stage_mask(barrier.src_stage)
            .src_access_mask(barrier.src_access)
            .dst_stage_mask(barrier.dst_stage)
            .dst_access_mask(barrier.dst_access)
            .old_layout(barrier.old_layout)
            .new_layout(barrier.new_layout)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image.handle())
            .subresource_range(range)];
        let info = vk::DependencyInfo::default().image_memory_barriers(&barriers);
        // SAFETY: recording into this command buffer; the image is kept alive below.
        unsafe { self.device.raw().cmd_pipeline_barrier2(self.raw, &info) };
        self.keep.push(Box::new(image.clone()));
    }

    /// A full barrier moving the whole image between layouts: everything before finishes, everything after waits.
    /// Simple and correct; hot paths use `image_barrier` with narrower stages.
    pub fn transition(&mut self, image: &Image, old: vk::ImageLayout, new: vk::ImageLayout) {
        self.image_barrier(
            image,
            &ImageBarrier {
                src_stage: vk::PipelineStageFlags2::ALL_COMMANDS,
                src_access: vk::AccessFlags2::MEMORY_WRITE,
                dst_stage: vk::PipelineStageFlags2::ALL_COMMANDS,
                dst_access: vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE,
                old_layout: old,
                new_layout: new,
                range: None,
            },
        );
    }

    pub fn bind_compute_pipeline(&mut self, pipeline: &ComputePipeline) {
        // SAFETY: recording into this command buffer; the pipeline is kept alive below.
        unsafe {
            self.device.raw().cmd_bind_pipeline(
                self.raw,
                vk::PipelineBindPoint::COMPUTE,
                pipeline.handle(),
            );
        }
        self.keep.push(Box::new(pipeline.clone()));
    }

    pub fn bind_descriptor_set(
        &mut self,
        bind_point: vk::PipelineBindPoint,
        layout: &PipelineLayout,
        set_index: u32,
        set: &DescriptorSet,
    ) {
        // SAFETY: recording into this command buffer; the set and layout are kept alive below.
        unsafe {
            self.device.raw().cmd_bind_descriptor_sets(
                self.raw,
                bind_point,
                layout.handle(),
                set_index,
                &[set.handle()],
                &[],
            );
        }
        self.keep.push(Box::new(set.clone()));
        self.keep.push(Box::new(layout.clone()));
    }

    /// Binds a raw descriptor set (the bindless set, whose owner outlives the work).
    pub fn bind_raw_descriptor_set(
        &mut self,
        bind_point: vk::PipelineBindPoint,
        layout: &PipelineLayout,
        set_index: u32,
        set: vk::DescriptorSet,
    ) {
        // SAFETY: recording into this command buffer; the layout is kept alive below.
        unsafe {
            self.device.raw().cmd_bind_descriptor_sets(
                self.raw,
                bind_point,
                layout.handle(),
                set_index,
                &[set],
                &[],
            );
        }
        self.keep.push(Box::new(layout.clone()));
    }

    pub fn push_constants(&mut self, layout: &PipelineLayout, offset: u32, data: &[u8]) {
        // SAFETY: recording into this command buffer.
        unsafe {
            self.device.raw().cmd_push_constants(
                self.raw,
                layout.handle(),
                vk::ShaderStageFlags::ALL,
                offset,
                data,
            );
        }
    }

    pub fn dispatch(&mut self, x: u32, y: u32, z: u32) {
        // SAFETY: recording into this command buffer.
        unsafe { self.device.raw().cmd_dispatch(self.raw, x, y, z) };
    }

    pub fn begin_rendering(&mut self, info: &vk::RenderingInfo<'_>) {
        // SAFETY: recording into this command buffer; the caller keeps the attachments alive via `keep_alive`.
        unsafe { self.device.raw().cmd_begin_rendering(self.raw, info) };
    }

    pub fn end_rendering(&mut self) {
        // SAFETY: recording into this command buffer.
        unsafe { self.device.raw().cmd_end_rendering(self.raw) };
    }
}
