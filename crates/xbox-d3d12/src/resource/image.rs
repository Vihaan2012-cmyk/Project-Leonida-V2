//! The Vulkan image behind a texture resource, and moving texels in and out of it.
//!
//! A texture is a `VkImage` the shaders reach through their T# (see the descriptor registry), never through
//! an address. Its data is in the console's tiled layout in game memory when the game wrote it there; the
//! layer detiles each subresource (`resource::layout`) and uploads it here.

use ash::vk;

use super::desc::{Dimension, ResourceDesc};
use crate::descriptor::format::{Format, Kind};
use crate::memory::Handles;

#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    #[error("Vulkan call failed: {0}")]
    Vulkan(#[from] vk::Result),
    #[error("{0:?} images with these usages are not supported by the GPU")]
    Unsupported(vk::Format),
    #[error("no device-local memory type fits the image")]
    NoMemory,
    #[error("subresource {mip}/{layer} is outside the image")]
    Subresource { mip: u32, layer: u32 },
    #[error("{0}")]
    Desc(#[from] super::desc::DescError),
}

pub struct GpuImage {
    pub image: vk::Image,
    pub memory: vk::DeviceMemory,
    pub format: vk::Format,
    pub extent: [u32; 3],
    pub mips: u32,
    pub layers: u32,
    pub samples: u32,
    pub aspect: vk::ImageAspectFlags,
}

fn usage_for(desc: &ResourceDesc, info: &Format) -> vk::ImageUsageFlags {
    let mut usage = vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST;
    if !desc.has_flag(super::desc::DENY_SHADER_RESOURCE) {
        usage |= vk::ImageUsageFlags::SAMPLED;
    }
    if desc.has_flag(super::desc::ALLOW_RENDER_TARGET) {
        usage |= vk::ImageUsageFlags::COLOR_ATTACHMENT;
    }
    if desc.has_flag(super::desc::ALLOW_DEPTH_STENCIL) || info.kind != Kind::Color {
        usage |= vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT;
    }
    if desc.has_flag(super::desc::ALLOW_UNORDERED_ACCESS) {
        usage |= vk::ImageUsageFlags::STORAGE;
    }
    usage
}

fn aspect_of(info: &Format, format: vk::Format) -> vk::ImageAspectFlags {
    match (info.kind, format) {
        (Kind::Color, _) => vk::ImageAspectFlags::COLOR,
        (_, vk::Format::D16_UNORM | vk::Format::D32_SFLOAT | vk::Format::X8_D24_UNORM_PACK32) => {
            vk::ImageAspectFlags::DEPTH
        }
        _ => vk::ImageAspectFlags::DEPTH | vk::ImageAspectFlags::STENCIL,
    }
}

/// The Vulkan format to store a depth-stencil texture in: the console's 24-bit depth falls back to 32-bit
/// floating point where the GPU has no D24.
fn storage_format(h: &Handles, info: &Format, usage: vk::ImageUsageFlags) -> vk::Format {
    if info.vulkan != vk::Format::D24_UNORM_S8_UINT {
        return info.vulkan;
    }
    // SAFETY: a pure query on the physical device the handles name.
    let supported = unsafe {
        h.instance
            .get_physical_device_image_format_properties(
                h.physical,
                info.vulkan,
                vk::ImageType::TYPE_2D,
                vk::ImageTiling::OPTIMAL,
                usage,
                vk::ImageCreateFlags::empty(),
            )
            .is_ok()
    };
    if supported {
        info.vulkan
    } else {
        vk::Format::D32_SFLOAT_S8_UINT
    }
}

impl GpuImage {
    pub fn create(h: &Handles, desc: &ResourceDesc) -> Result<GpuImage, ImageError> {
        desc.validate()?;
        let info = desc.format_info()?;
        let usage = usage_for(desc, info);
        let format = storage_format(h, info, usage);
        let layers = desc.array_size();
        let (ty, extent) = match desc.dimension {
            Dimension::Texture1D => (vk::ImageType::TYPE_1D, [desc.width as u32, 1, 1]),
            Dimension::Texture3D => (
                vk::ImageType::TYPE_3D,
                [desc.width as u32, desc.height, desc.depth()],
            ),
            _ => (vk::ImageType::TYPE_2D, [desc.width as u32, desc.height, 1]),
        };
        let mut flags = vk::ImageCreateFlags::empty();
        if info.typeless
            || info.vulkan != format
            || desc.has_flag(super::desc::ALLOW_UNORDERED_ACCESS)
        {
            flags |= vk::ImageCreateFlags::MUTABLE_FORMAT;
        }
        if desc.dimension == Dimension::Texture2D && layers >= 6 {
            flags |= vk::ImageCreateFlags::CUBE_COMPATIBLE;
        }
        let samples = vk::SampleCountFlags::from_raw(desc.sample_count.max(1));
        let create = vk::ImageCreateInfo::default()
            .flags(flags)
            .image_type(ty)
            .format(format)
            .extent(vk::Extent3D {
                width: extent[0],
                height: extent[1],
                depth: extent[2],
            })
            .mip_levels(desc.mips())
            .array_layers(layers)
            .samples(samples)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: fresh objects, destroyed by `GpuImage::destroy`; on failure the ones made are destroyed here.
        unsafe {
            let _ = h
                .instance
                .get_physical_device_image_format_properties(
                    h.physical,
                    format,
                    ty,
                    vk::ImageTiling::OPTIMAL,
                    usage,
                    flags,
                )
                .map_err(|_| ImageError::Unsupported(format))?;
            let image = h.device.create_image(&create, None)?;
            let needs = h.device.get_image_memory_requirements(image);
            let types = h.instance.get_physical_device_memory_properties(h.physical);
            let kind = (0..types.memory_type_count)
                .filter(|i| needs.memory_type_bits & (1 << i) != 0)
                .find(|i| {
                    types.memory_types[*i as usize]
                        .property_flags
                        .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
                })
                .or_else(|| {
                    (0..types.memory_type_count).find(|i| needs.memory_type_bits & (1 << i) != 0)
                });
            let Some(kind) = kind else {
                h.device.destroy_image(image, None);
                return Err(ImageError::NoMemory);
            };
            let memory = match h.device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(needs.size)
                    .memory_type_index(kind),
                None,
            ) {
                Ok(memory) => memory,
                Err(e) => {
                    h.device.destroy_image(image, None);
                    return Err(e.into());
                }
            };
            if let Err(e) = h.device.bind_image_memory(image, memory, 0) {
                h.device.destroy_image(image, None);
                h.device.free_memory(memory, None);
                return Err(e.into());
            }
            let made = GpuImage {
                image,
                memory,
                format,
                extent,
                mips: desc.mips(),
                layers,
                samples: desc.sample_count.max(1),
                aspect: aspect_of(info, format),
            };
            made.to_general(h)?;
            Ok(made)
        }
    }

    /// Every descriptor reads the image in `GENERAL`, so that is the layout it lives in.
    fn to_general(&self, h: &Handles) -> Result<(), ImageError> {
        self.submit(h, |device, command| {
            let barrier = [vk::ImageMemoryBarrier::default()
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::GENERAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(self.image)
                .subresource_range(self.range())
                .dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)];
            // SAFETY: recording into a command buffer `submit` made.
            unsafe {
                device.cmd_pipeline_barrier(
                    command,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &barrier,
                );
            }
        })
    }

    fn range(&self) -> vk::ImageSubresourceRange {
        vk::ImageSubresourceRange::default()
            .aspect_mask(self.aspect)
            .level_count(self.mips)
            .layer_count(self.layers)
    }

    /// Records into a one-time command buffer on the handles' queue, and waits.
    fn submit(
        &self,
        h: &Handles,
        record: impl FnOnce(&ash::Device, vk::CommandBuffer),
    ) -> Result<(), ImageError> {
        let device = &h.device;
        // SAFETY: the pool, command buffer and fence are made and destroyed here; the queue is used under its
        // lock and waited on.
        unsafe {
            let pool = device.create_command_pool(
                &vk::CommandPoolCreateInfo::default().queue_family_index(h.queue_family),
                None,
            )?;
            let result = (|| {
                let command = device.allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(pool)
                        .command_buffer_count(1),
                )?[0];
                device.begin_command_buffer(
                    command,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )?;
                record(device, command);
                device.end_command_buffer(command)?;
                let fence = device.create_fence(&vk::FenceCreateInfo::default(), None)?;
                let commands = [command];
                let submitted = {
                    let _guard = h.queue_lock.lock().unwrap_or_else(|e| e.into_inner());
                    device.queue_submit(
                        h.queue,
                        &[vk::SubmitInfo::default().command_buffers(&commands)],
                        fence,
                    )
                };
                let waited =
                    submitted.and_then(|()| device.wait_for_fences(&[fence], true, u64::MAX));
                device.destroy_fence(fence, None);
                waited
            })();
            device.destroy_command_pool(pool, None);
            result?;
        }
        Ok(())
    }

    fn check(&self, mip: u32, layer: u32) -> Result<(), ImageError> {
        if mip >= self.mips || layer >= self.layers {
            return Err(ImageError::Subresource { mip, layer });
        }
        Ok(())
    }

    fn mip_extent(&self, mip: u32) -> vk::Extent3D {
        vk::Extent3D {
            width: (self.extent[0] >> mip).max(1),
            height: (self.extent[1] >> mip).max(1),
            depth: (self.extent[2] >> mip).max(1),
        }
    }

    fn staging(
        h: &Handles,
        size: u64,
    ) -> Result<(vk::Buffer, vk::DeviceMemory, *mut u8), ImageError> {
        // SAFETY: a fresh host-visible buffer the caller destroys with `drop_staging`.
        unsafe {
            let buffer = h.device.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size)
                    .usage(vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::TRANSFER_DST),
                None,
            )?;
            let needs = h.device.get_buffer_memory_requirements(buffer);
            let types = h.instance.get_physical_device_memory_properties(h.physical);
            let want =
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
            let Some(kind) = (0..types.memory_type_count).find(|i| {
                needs.memory_type_bits & (1 << i) != 0
                    && types.memory_types[*i as usize]
                        .property_flags
                        .contains(want)
            }) else {
                h.device.destroy_buffer(buffer, None);
                return Err(ImageError::NoMemory);
            };
            let memory = h.device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(needs.size)
                    .memory_type_index(kind),
                None,
            )?;
            h.device.bind_buffer_memory(buffer, memory, 0)?;
            let mapped = h
                .device
                .map_memory(memory, 0, size, vk::MemoryMapFlags::empty())?
                as *mut u8;
            Ok((buffer, memory, mapped))
        }
    }

    fn drop_staging(h: &Handles, buffer: vk::Buffer, memory: vk::DeviceMemory) {
        // SAFETY: the copy that used them has completed.
        unsafe {
            h.device.destroy_buffer(buffer, None);
            h.device.free_memory(memory, None);
        }
    }

    /// Writes one subresource from tightly packed rows of blocks, and waits for the copy.
    pub fn upload(&self, h: &Handles, mip: u32, layer: u32, rows: &[u8]) -> Result<(), ImageError> {
        self.check(mip, layer)?;
        let (buffer, memory, mapped) = Self::staging(h, rows.len().max(1) as u64)?;
        // SAFETY: `mapped` is a mapping of at least `rows.len()` bytes.
        unsafe { std::ptr::copy_nonoverlapping(rows.as_ptr(), mapped, rows.len()) };
        let region = [vk::BufferImageCopy::default()
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(self.aspect)
                    .mip_level(mip)
                    .base_array_layer(layer)
                    .layer_count(1),
            )
            .image_extent(self.mip_extent(mip))];
        let result = self.submit(h, |device, command| {
            // SAFETY: recording into a command buffer `submit` made; the buffer and image are alive.
            unsafe {
                device.cmd_copy_buffer_to_image(
                    command,
                    buffer,
                    self.image,
                    vk::ImageLayout::GENERAL,
                    &region,
                );
            }
        });
        Self::drop_staging(h, buffer, memory);
        result
    }

    /// Reads one subresource back as tightly packed rows of blocks; `bytes` is how many.
    pub fn download(
        &self,
        h: &Handles,
        mip: u32,
        layer: u32,
        bytes: usize,
    ) -> Result<Vec<u8>, ImageError> {
        self.check(mip, layer)?;
        let (buffer, memory, mapped) = Self::staging(h, bytes.max(1) as u64)?;
        let region = [vk::BufferImageCopy::default()
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(self.aspect)
                    .mip_level(mip)
                    .base_array_layer(layer)
                    .layer_count(1),
            )
            .image_extent(self.mip_extent(mip))];
        let result = self.submit(h, |device, command| {
            // SAFETY: as in `upload`.
            unsafe {
                device.cmd_copy_image_to_buffer(
                    command,
                    self.image,
                    vk::ImageLayout::GENERAL,
                    buffer,
                    &region,
                );
            }
        });
        // SAFETY: the copy completed and `mapped` covers `bytes`.
        let data = result.map(|()| unsafe { std::slice::from_raw_parts(mapped, bytes).to_vec() });
        Self::drop_staging(h, buffer, memory);
        data
    }

    /// Releases the image and its memory once the GPU is done with it.
    pub fn destroy(self, h: &Handles) {
        // SAFETY: the caller promises no work still uses the image.
        unsafe {
            h.device.destroy_image(self.image, None);
            h.device.free_memory(self.memory, None);
        }
    }
}
