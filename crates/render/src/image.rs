use std::sync::{Arc, Mutex};

use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};

use crate::buffer::{Buffer, BufferDesc, MemoryLocation};
use crate::device::{Device, Queue, lock};
use crate::error::{RenderError, Result};
use crate::format::{Format, level_extent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    D1,
    D2,
    D3,
}

#[derive(Debug, Clone)]
pub struct ImageDesc<'a> {
    pub name: &'a str,
    pub kind: ImageKind,
    pub format: Format,
    /// Width, height and depth; unused dimensions are 1.
    pub extent: [u32; 3],
    pub mips: u32,
    /// Array layers (a cube map has six).
    pub layers: u32,
    pub samples: u32,
    /// `TRANSFER_SRC` and `TRANSFER_DST` are always added.
    pub usage: vk::ImageUsageFlags,
    /// Create with `CUBE_COMPATIBLE` (needs a 2D image of six layers or a multiple).
    pub cube: bool,
    /// Create with `MUTABLE_FORMAT` so views can reinterpret the texels in another number type of the same size.
    pub mutable_format: bool,
    /// Give the image its own memory block (render targets, large images).
    pub dedicated: bool,
}

impl<'a> ImageDesc<'a> {
    pub fn new_2d(
        name: &'a str,
        format: Format,
        width: u32,
        height: u32,
        usage: vk::ImageUsageFlags,
    ) -> Self {
        ImageDesc {
            name,
            kind: ImageKind::D2,
            format,
            extent: [width, height, 1],
            mips: 1,
            layers: 1,
            samples: 1,
            usage,
            cube: false,
            mutable_format: false,
            dedicated: false,
        }
    }

    pub fn new_3d(
        name: &'a str,
        format: Format,
        extent: [u32; 3],
        usage: vk::ImageUsageFlags,
    ) -> Self {
        ImageDesc {
            kind: ImageKind::D3,
            extent,
            ..ImageDesc::new_2d(name, format, 1, 1, usage)
        }
    }

    pub fn mips(mut self, mips: u32) -> Self {
        self.mips = mips;
        self
    }

    pub fn layers(mut self, layers: u32) -> Self {
        self.layers = layers;
        self
    }
}

enum Backing {
    Allocated(Mutex<Option<Allocation>>),
    Swapchain,
}

struct ImageInner {
    device: Device,
    handle: vk::Image,
    kind: ImageKind,
    format: Format,
    extent: [u32; 3],
    mips: u32,
    layers: u32,
    samples: u32,
    usage: vk::ImageUsageFlags,
    backing: Backing,
}

impl Drop for ImageInner {
    fn drop(&mut self) {
        if let Backing::Allocated(allocation) = &self.backing {
            // SAFETY: the image belongs to the device `self.device` keeps alive, and the last clone is gone, so
            // no command buffer or descriptor the stream tracks refers to it.
            unsafe {
                self.device.raw().destroy_image(self.handle, None);
            }
            if let Some(allocation) = lock(allocation).take()
                && let Some(allocator) = self.device.allocator().as_mut()
            {
                let _ = allocator.free(allocation);
            }
        }
    }
}

/// A Vulkan image with its memory. Clones share the image.
#[derive(Clone)]
pub struct Image {
    inner: Arc<ImageInner>,
}

pub(crate) fn image_type(kind: ImageKind) -> vk::ImageType {
    match kind {
        ImageKind::D1 => vk::ImageType::TYPE_1D,
        ImageKind::D2 => vk::ImageType::TYPE_2D,
        ImageKind::D3 => vk::ImageType::TYPE_3D,
    }
}

fn align_up(value: u64, align: u64) -> u64 {
    value.div_ceil(align) * align
}

impl Image {
    pub fn new(device: &Device, desc: &ImageDesc<'_>) -> Result<Image> {
        if desc.extent.contains(&0) || desc.mips == 0 || desc.layers == 0 {
            return Err(RenderError::Invalid(format!(
                "image {}: empty extent, mips or layers",
                desc.name
            )));
        }
        let mut flags = vk::ImageCreateFlags::empty();
        if desc.cube {
            flags |= vk::ImageCreateFlags::CUBE_COMPATIBLE;
        }
        if desc.mutable_format {
            flags |= vk::ImageCreateFlags::MUTABLE_FORMAT;
        }
        if desc.kind == ImageKind::D3
            && desc.usage.intersects(vk::ImageUsageFlags::COLOR_ATTACHMENT)
        {
            flags |= vk::ImageCreateFlags::TYPE_2D_ARRAY_COMPATIBLE;
        }
        let usage =
            desc.usage | vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST;
        let info = vk::ImageCreateInfo::default()
            .flags(flags)
            .image_type(image_type(desc.kind))
            .format(desc.format.vk())
            .extent(vk::Extent3D {
                width: desc.extent[0],
                height: desc.extent[1],
                depth: desc.extent[2],
            })
            .mip_levels(desc.mips)
            .array_layers(desc.layers)
            .samples(vk::SampleCountFlags::from_raw(desc.samples.max(1)))
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(usage)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let raw = device.raw();
        // SAFETY: the image is bound once to memory sized from its own requirements; it is destroyed on failure.
        unsafe {
            let handle = raw.create_image(&info, None)?;
            let requirements = raw.get_image_memory_requirements(handle);
            let allocation = device
                .allocator()
                .as_mut()
                .ok_or(RenderError::NoMemoryType)
                .and_then(|a| {
                    a.allocate(&AllocationCreateDesc {
                        name: desc.name,
                        requirements,
                        location: MemoryLocation::GpuOnly.alloc(),
                        linear: false,
                        allocation_scheme: if desc.dedicated {
                            AllocationScheme::DedicatedImage(handle)
                        } else {
                            AllocationScheme::GpuAllocatorManaged
                        },
                    })
                    .map_err(Into::into)
                });
            let allocation = match allocation {
                Ok(a) => a,
                Err(e) => {
                    raw.destroy_image(handle, None);
                    return Err(e);
                }
            };
            if let Err(e) = raw.bind_image_memory(handle, allocation.memory(), allocation.offset())
            {
                raw.destroy_image(handle, None);
                if let Some(a) = device.allocator().as_mut() {
                    let _ = a.free(allocation);
                }
                return Err(e.into());
            }
            Ok(Image {
                inner: Arc::new(ImageInner {
                    device: device.clone(),
                    handle,
                    kind: desc.kind,
                    format: desc.format,
                    extent: desc.extent,
                    mips: desc.mips,
                    layers: desc.layers,
                    samples: desc.samples.max(1),
                    usage,
                    backing: Backing::Allocated(Mutex::new(Some(allocation))),
                }),
            })
        }
    }

    /// Wraps a swapchain image; the swapchain owns and destroys it.
    pub(crate) fn from_swapchain(
        device: &Device,
        handle: vk::Image,
        format: Format,
        extent: [u32; 2],
        usage: vk::ImageUsageFlags,
    ) -> Image {
        Image {
            inner: Arc::new(ImageInner {
                device: device.clone(),
                handle,
                kind: ImageKind::D2,
                format,
                extent: [extent[0], extent[1], 1],
                mips: 1,
                layers: 1,
                samples: 1,
                usage,
                backing: Backing::Swapchain,
            }),
        }
    }

    pub fn handle(&self) -> vk::Image {
        self.inner.handle
    }

    pub fn device(&self) -> &Device {
        &self.inner.device
    }

    pub fn kind(&self) -> ImageKind {
        self.inner.kind
    }

    pub fn format(&self) -> Format {
        self.inner.format
    }

    pub fn extent(&self) -> [u32; 3] {
        self.inner.extent
    }

    pub fn mips(&self) -> u32 {
        self.inner.mips
    }

    pub fn layers(&self) -> u32 {
        self.inner.layers
    }

    pub fn samples(&self) -> u32 {
        self.inner.samples
    }

    pub fn usage(&self) -> vk::ImageUsageFlags {
        self.inner.usage
    }

    /// Every mip level and layer, of `aspect`.
    pub fn full_range(&self, aspect: vk::ImageAspectFlags) -> vk::ImageSubresourceRange {
        vk::ImageSubresourceRange {
            aspect_mask: aspect,
            base_mip_level: 0,
            level_count: self.inner.mips,
            base_array_layer: 0,
            layer_count: self.inner.layers,
        }
    }

    /// The copy regions and total size for tightly packed data covering every layer and mip, layer-major with
    /// the mips of one layer together (the order DDS files use). Each subresource starts on a 16-byte boundary.
    pub fn packed_layout(&self) -> (Vec<vk::BufferImageCopy>, u64) {
        let inner = &self.inner;
        let aspect = inner.format.sampled_aspect();
        let mut regions = Vec::new();
        let mut offset = 0u64;
        for layer in 0..inner.layers {
            for mip in 0..inner.mips {
                let [w, h, d] = level_extent(inner.extent, mip);
                regions.push(vk::BufferImageCopy {
                    buffer_offset: offset,
                    buffer_row_length: 0,
                    buffer_image_height: 0,
                    image_subresource: vk::ImageSubresourceLayers {
                        aspect_mask: aspect,
                        mip_level: mip,
                        base_array_layer: layer,
                        layer_count: 1,
                    },
                    image_offset: vk::Offset3D::default(),
                    image_extent: vk::Extent3D {
                        width: w,
                        height: h,
                        depth: d,
                    },
                });
                offset = align_up(offset + inner.format.level_bytes(inner.extent, mip), 16);
            }
        }
        (regions, offset)
    }

    /// Uploads `data` laid out as `packed_layout` describes, and leaves the image in `final_layout`. Blocks until
    /// the copy is done. For tools and setup; frames use a [`crate::StagingRing`].
    pub fn upload_packed(
        &self,
        queue: &Queue,
        data: &[u8],
        final_layout: vk::ImageLayout,
    ) -> Result<()> {
        let (regions, _) = self.packed_layout();
        if (data.len() as u64) < self.packed_data_end(&regions) {
            return Err(RenderError::Invalid(
                "image data is shorter than its mips and layers".into(),
            ));
        }
        let staging = Buffer::new(
            queue.device(),
            &BufferDesc {
                name: "image upload",
                size: data.len() as u64,
                usage: vk::BufferUsageFlags::empty(),
                location: MemoryLocation::Upload,
            },
        )?;
        staging.write(0, data)?;
        queue.immediate(|cb| {
            cb.transition(
                self,
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            );
            cb.copy_buffer_to_image(
                &staging,
                self,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &regions,
            );
            cb.transition(self, vk::ImageLayout::TRANSFER_DST_OPTIMAL, final_layout);
            Ok(())
        })
    }

    fn packed_data_end(&self, regions: &[vk::BufferImageCopy]) -> u64 {
        regions
            .last()
            .map(|r| {
                r.buffer_offset
                    + self
                        .inner
                        .format
                        .level_bytes(self.inner.extent, r.image_subresource.mip_level)
            })
            .unwrap_or(0)
    }

    /// Reads every layer and mip back in `packed_layout` order. The image must be in `layout`; it is left there.
    pub fn read_back(&self, queue: &Queue, layout: vk::ImageLayout) -> Result<Vec<u8>> {
        let (regions, total) = self.packed_layout();
        let staging = Buffer::new(
            queue.device(),
            &BufferDesc {
                name: "image readback",
                size: total,
                usage: vk::BufferUsageFlags::empty(),
                location: MemoryLocation::Readback,
            },
        )?;
        queue.immediate(|cb| {
            cb.transition(self, layout, vk::ImageLayout::TRANSFER_SRC_OPTIMAL);
            cb.copy_image_to_buffer(
                self,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                &staging,
                &regions,
            );
            cb.transition(self, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, layout);
            Ok(())
        })?;
        staging.read_vec(0, self.packed_data_end(&regions) as usize)
    }

    pub fn create_view(&self, desc: &ViewDesc) -> Result<ImageView> {
        let inner = &self.inner;
        let format = desc.format.unwrap_or(inner.format);
        let aspect = desc.aspect.unwrap_or_else(|| format.sampled_aspect());
        let info = vk::ImageViewCreateInfo::default()
            .image(inner.handle)
            .view_type(desc.view_type)
            .format(format.vk())
            .components(desc.swizzle)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: aspect,
                base_mip_level: desc.base_mip,
                level_count: desc.mips.unwrap_or(inner.mips - desc.base_mip),
                base_array_layer: desc.base_layer,
                layer_count: desc.layers.unwrap_or(inner.layers - desc.base_layer),
            });
        // SAFETY: the image is valid and the view keeps a clone of it.
        let handle = unsafe { inner.device.raw().create_image_view(&info, None)? };
        Ok(ImageView {
            image: self.clone(),
            handle,
            format,
            view_type: desc.view_type,
        })
    }

    /// A view of everything: 2D, 2D array, cube-compatible or 3D as the image's shape suggests.
    pub fn default_view(&self) -> Result<ImageView> {
        self.create_view(&ViewDesc::of(self))
    }
}

/// What part of an image a view shows and how.
#[derive(Debug, Clone, Copy)]
pub struct ViewDesc {
    pub view_type: vk::ImageViewType,
    /// Reinterpret the texels in another format (needs `ImageDesc::mutable_format` unless it is the same one).
    pub format: Option<Format>,
    /// Default: the format's sampled aspect (depth only for depth-stencil).
    pub aspect: Option<vk::ImageAspectFlags>,
    pub base_mip: u32,
    /// `None`: every level from `base_mip`.
    pub mips: Option<u32>,
    pub base_layer: u32,
    /// `None`: every layer from `base_layer`.
    pub layers: Option<u32>,
    pub swizzle: vk::ComponentMapping,
}

impl ViewDesc {
    pub fn of(image: &Image) -> ViewDesc {
        let view_type = match (image.kind(), image.layers()) {
            (ImageKind::D1, 1) => vk::ImageViewType::TYPE_1D,
            (ImageKind::D1, _) => vk::ImageViewType::TYPE_1D_ARRAY,
            (ImageKind::D2, 1) => vk::ImageViewType::TYPE_2D,
            (ImageKind::D2, _) => vk::ImageViewType::TYPE_2D_ARRAY,
            (ImageKind::D3, _) => vk::ImageViewType::TYPE_3D,
        };
        ViewDesc::new(view_type)
    }

    pub fn new(view_type: vk::ImageViewType) -> ViewDesc {
        ViewDesc {
            view_type,
            format: None,
            aspect: None,
            base_mip: 0,
            mips: None,
            base_layer: 0,
            layers: None,
            swizzle: vk::ComponentMapping::default(),
        }
    }
}

/// A `VkImageView` that keeps its image alive.
pub struct ImageView {
    image: Image,
    handle: vk::ImageView,
    format: Format,
    view_type: vk::ImageViewType,
}

impl ImageView {
    pub fn handle(&self) -> vk::ImageView {
        self.handle
    }

    pub fn image(&self) -> &Image {
        &self.image
    }

    pub fn format(&self) -> Format {
        self.format
    }

    pub fn view_type(&self) -> vk::ImageViewType {
        self.view_type
    }
}

impl Drop for ImageView {
    fn drop(&mut self) {
        // SAFETY: the view was created from this device and the owner guarantees the GPU is done with it.
        unsafe {
            self.image
                .device()
                .raw()
                .destroy_image_view(self.handle, None);
        }
    }
}
