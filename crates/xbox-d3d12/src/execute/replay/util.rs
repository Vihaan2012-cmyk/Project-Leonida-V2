//! Small pure helpers the replay shares: attachment infos, rectangles and buffer-image regions.

use ash::vk;

use super::super::command::*;
use crate::pipeline::state::{block_size, format_bytes};

pub(super) fn attachment(
    view: vk::ImageView,
    layout: vk::ImageLayout,
    clear: Option<vk::ClearValue>,
) -> vk::RenderingAttachmentInfo<'static> {
    let info = vk::RenderingAttachmentInfo::default()
        .image_view(view)
        .image_layout(layout)
        .store_op(vk::AttachmentStoreOp::STORE);
    match clear {
        Some(value) => info.load_op(vk::AttachmentLoadOp::CLEAR).clear_value(value),
        None => info.load_op(vk::AttachmentLoadOp::LOAD),
    }
}

/// A D3D viewport as a Vulkan one with the y axis flipped, so clip space keeps D3D's orientation.
pub(super) fn flip(v: Viewport) -> vk::Viewport {
    vk::Viewport {
        x: v.x,
        y: v.y + v.height,
        width: v.width,
        height: -v.height,
        min_depth: v.min_depth,
        max_depth: v.max_depth,
    }
}

pub(super) fn rect2d(r: Rect) -> vk::Rect2D {
    vk::Rect2D {
        offset: vk::Offset2D {
            x: r.left,
            y: r.top,
        },
        extent: vk::Extent2D {
            width: (r.right - r.left).max(0) as u32,
            height: (r.bottom - r.top).max(0) as u32,
        },
    }
}

pub(super) fn offset3(o: [u32; 3]) -> vk::Offset3D {
    vk::Offset3D {
        x: o[0] as i32,
        y: o[1] as i32,
        z: o[2] as i32,
    }
}

pub(super) fn extent3(e: [u32; 3]) -> vk::Extent3D {
    vk::Extent3D {
        width: e[0],
        height: e[1],
        depth: e[2],
    }
}

pub(super) fn copy_aspect(image: &ImageRef) -> vk::ImageAspectFlags {
    if image.aspect.contains(vk::ImageAspectFlags::DEPTH) {
        vk::ImageAspectFlags::DEPTH
    } else {
        image.aspect
    }
}

pub(super) fn layers_of(image: &ImageRef, sub: Subresource) -> vk::ImageSubresourceLayers {
    vk::ImageSubresourceLayers::default()
        .aspect_mask(copy_aspect(image))
        .mip_level(sub.mip)
        .base_array_layer(sub.layer)
        .layer_count(1)
}

pub(super) fn buffer_image_region(
    image: &ImageRef,
    sub: Subresource,
    origin: [u32; 3],
    size: [u32; 3],
    buffer: &BufferFootprint,
) -> vk::BufferImageCopy {
    let block = block_size(image.format);
    let row_texels = buffer.row_pitch / format_bytes(image.format).max(1) * block;
    vk::BufferImageCopy::default()
        .buffer_offset(buffer.va)
        .buffer_row_length(row_texels)
        .buffer_image_height(if buffer.slice_rows == 0 {
            0
        } else {
            buffer.slice_rows * block
        })
        .image_subresource(layers_of(image, sub))
        .image_offset(offset3(origin))
        .image_extent(extent3(size))
}
