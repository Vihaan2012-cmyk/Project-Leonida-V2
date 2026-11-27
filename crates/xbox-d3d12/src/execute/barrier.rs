//! `D3D12_RESOURCE_STATES` as Vulkan layouts, stages and accesses, and resource barriers as synchronization2.

use ash::vk;

use super::command::{Barrier, ImageRef, Resource};

pub const VERTEX_AND_CONSTANT_BUFFER: u32 = 0x1;
pub const INDEX_BUFFER: u32 = 0x2;
pub const RENDER_TARGET: u32 = 0x4;
pub const UNORDERED_ACCESS: u32 = 0x8;
pub const DEPTH_WRITE: u32 = 0x10;
pub const DEPTH_READ: u32 = 0x20;
pub const NON_PIXEL_SHADER_RESOURCE: u32 = 0x40;
pub const PIXEL_SHADER_RESOURCE: u32 = 0x80;
pub const STREAM_OUT: u32 = 0x100;
pub const INDIRECT_ARGUMENT: u32 = 0x200;
pub const COPY_DEST: u32 = 0x400;
pub const COPY_SOURCE: u32 = 0x800;
pub const RESOLVE_DEST: u32 = 0x1000;
pub const RESOLVE_SOURCE: u32 = 0x2000;

const WRITES: u32 =
    RENDER_TARGET | UNORDERED_ACCESS | DEPTH_WRITE | STREAM_OUT | COPY_DEST | RESOLVE_DEST;
const SHADER_READS: u32 = NON_PIXEL_SHADER_RESOURCE | PIXEL_SHADER_RESOURCE;
const TRANSFER_READS: u32 = COPY_SOURCE | RESOLVE_SOURCE;

/// What a state means to Vulkan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateMap {
    pub layout: vk::ImageLayout,
    pub stage: vk::PipelineStageFlags2,
    pub access: vk::AccessFlags2,
}

fn shader_stages(state: u32) -> vk::PipelineStageFlags2 {
    use vk::PipelineStageFlags2 as S;
    let non_pixel = S::VERTEX_SHADER
        | S::GEOMETRY_SHADER
        | S::COMPUTE_SHADER
        | S::TASK_SHADER_EXT
        | S::MESH_SHADER_EXT;
    let mut stages = S::empty();
    if state
        & (NON_PIXEL_SHADER_RESOURCE | UNORDERED_ACCESS | VERTEX_AND_CONSTANT_BUFFER | STREAM_OUT)
        != 0
    {
        stages |= non_pixel;
    }
    if state & (PIXEL_SHADER_RESOURCE | UNORDERED_ACCESS | VERTEX_AND_CONSTANT_BUFFER) != 0 {
        stages |= S::FRAGMENT_SHADER;
    }
    stages
}

/// Maps the bits of a state; `depth` says the resource is a depth image, whose read-only layout differs.
pub fn map_state(state: u32, depth: bool) -> StateMap {
    use vk::AccessFlags2 as A;
    use vk::PipelineStageFlags2 as S;
    if state == 0 {
        return StateMap {
            layout: vk::ImageLayout::GENERAL,
            stage: S::ALL_COMMANDS,
            access: A::MEMORY_READ | A::MEMORY_WRITE,
        };
    }
    let mut stage = shader_stages(state);
    let mut access = A::empty();
    if state & VERTEX_AND_CONSTANT_BUFFER != 0 {
        stage |= S::VERTEX_ATTRIBUTE_INPUT;
        access |= A::UNIFORM_READ | A::VERTEX_ATTRIBUTE_READ;
    }
    if state & INDEX_BUFFER != 0 {
        stage |= S::INDEX_INPUT;
        access |= A::INDEX_READ;
    }
    if state & RENDER_TARGET != 0 {
        stage |= S::COLOR_ATTACHMENT_OUTPUT;
        access |= A::COLOR_ATTACHMENT_READ | A::COLOR_ATTACHMENT_WRITE;
    }
    if state & UNORDERED_ACCESS != 0 {
        access |= A::SHADER_STORAGE_READ | A::SHADER_STORAGE_WRITE;
    }
    if state & (DEPTH_WRITE | DEPTH_READ) != 0 {
        stage |= S::EARLY_FRAGMENT_TESTS | S::LATE_FRAGMENT_TESTS;
        access |= A::DEPTH_STENCIL_ATTACHMENT_READ;
        if state & DEPTH_WRITE != 0 {
            access |= A::DEPTH_STENCIL_ATTACHMENT_WRITE;
        }
    }
    if state & SHADER_READS != 0 {
        access |= A::SHADER_SAMPLED_READ | A::SHADER_STORAGE_READ | A::UNIFORM_READ;
    }
    if state & INDIRECT_ARGUMENT != 0 {
        stage |= S::DRAW_INDIRECT;
        access |= A::INDIRECT_COMMAND_READ;
    }
    if state & STREAM_OUT != 0 {
        access |= A::SHADER_STORAGE_WRITE;
    }
    if state & (COPY_DEST | RESOLVE_DEST) != 0 {
        stage |= S::ALL_TRANSFER;
        access |= A::TRANSFER_WRITE;
    }
    if state & TRANSFER_READS != 0 {
        stage |= S::ALL_TRANSFER;
        access |= A::TRANSFER_READ;
    }
    let layout = layout_of(state, depth);
    StateMap {
        layout,
        stage,
        access,
    }
}

fn layout_of(state: u32, depth: bool) -> vk::ImageLayout {
    use vk::ImageLayout as L;
    let writes = state & WRITES;
    if writes.count_ones() > 1 {
        return L::GENERAL;
    }
    if writes == UNORDERED_ACCESS || state & STREAM_OUT != 0 {
        return L::GENERAL;
    }
    if state & DEPTH_WRITE != 0 {
        return if state & !(DEPTH_WRITE | DEPTH_READ) == 0 {
            L::DEPTH_STENCIL_ATTACHMENT_OPTIMAL
        } else {
            L::GENERAL
        };
    }
    if state & RENDER_TARGET != 0 {
        return if state == RENDER_TARGET {
            L::COLOR_ATTACHMENT_OPTIMAL
        } else {
            L::GENERAL
        };
    }
    if state & (COPY_DEST | RESOLVE_DEST) != 0 {
        return if state & !(COPY_DEST | RESOLVE_DEST) == 0 {
            L::TRANSFER_DST_OPTIMAL
        } else {
            L::GENERAL
        };
    }
    let reads = state & !(VERTEX_AND_CONSTANT_BUFFER | INDEX_BUFFER | INDIRECT_ARGUMENT);
    if reads == 0 {
        return if depth {
            L::DEPTH_STENCIL_READ_ONLY_OPTIMAL
        } else {
            L::SHADER_READ_ONLY_OPTIMAL
        };
    }
    if reads & !TRANSFER_READS == 0 {
        return L::TRANSFER_SRC_OPTIMAL;
    }
    if reads & !SHADER_READS == 0 {
        return if depth {
            L::DEPTH_STENCIL_READ_ONLY_OPTIMAL
        } else {
            L::SHADER_READ_ONLY_OPTIMAL
        };
    }
    if depth && reads & !(DEPTH_READ | SHADER_READS) == 0 {
        return L::DEPTH_STENCIL_READ_ONLY_OPTIMAL;
    }
    L::READ_ONLY_OPTIMAL
}

fn range(image: &ImageRef, sub: Option<super::command::Subresource>) -> vk::ImageSubresourceRange {
    let (base_mip, mips, base_layer, layers) = match sub {
        Some(s) => (s.mip, 1, s.layer, 1),
        None => (0, vk::REMAINING_MIP_LEVELS, 0, vk::REMAINING_ARRAY_LAYERS),
    };
    vk::ImageSubresourceRange::default()
        .aspect_mask(image.aspect)
        .base_mip_level(base_mip)
        .level_count(mips)
        .base_array_layer(base_layer)
        .layer_count(layers)
}

/// What a list of barriers comes to: image transitions and one memory barrier for everything else.
#[derive(Debug, Default)]
pub struct Built {
    pub images: Vec<vk::ImageMemoryBarrier2<'static>>,
    pub memory: Option<vk::MemoryBarrier2<'static>>,
}

pub fn build(barriers: &[Barrier]) -> Built {
    let mut built = Built::default();
    let mut memory = vk::MemoryBarrier2::default();
    let mut any_memory = false;
    for barrier in barriers {
        match *barrier {
            Barrier::Transition {
                resource,
                subresource,
                before,
                after,
            } => match resource {
                Resource::Image(image) => {
                    if before == after {
                        continue;
                    }
                    let (src, dst) = (
                        map_state(before, image.is_depth()),
                        map_state(after, image.is_depth()),
                    );
                    built.images.push(
                        vk::ImageMemoryBarrier2::default()
                            .src_stage_mask(src.stage)
                            .src_access_mask(src.access)
                            .dst_stage_mask(dst.stage)
                            .dst_access_mask(dst.access)
                            .old_layout(src.layout)
                            .new_layout(dst.layout)
                            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                            .image(image.image)
                            .subresource_range(range(&image, subresource)),
                    );
                }
                Resource::Buffer => {
                    if before == after {
                        continue;
                    }
                    let (src, dst) = (map_state(before, false), map_state(after, false));
                    memory.src_stage_mask |= src.stage;
                    memory.src_access_mask |= src.access;
                    memory.dst_stage_mask |= dst.stage;
                    memory.dst_access_mask |= dst.access;
                    any_memory = true;
                }
            },
            Barrier::Uav | Barrier::Aliasing => {
                memory.src_stage_mask |= vk::PipelineStageFlags2::ALL_COMMANDS;
                memory.src_access_mask |= vk::AccessFlags2::MEMORY_WRITE;
                memory.dst_stage_mask |= vk::PipelineStageFlags2::ALL_COMMANDS;
                memory.dst_access_mask |=
                    vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE;
                any_memory = true;
            }
        }
    }
    built.memory = any_memory.then_some(memory);
    built
}

/// Records the barriers; the command buffer must not be inside a rendering scope.
///
/// # Safety
/// `cb` is a command buffer of `device` in the recording state.
pub unsafe fn record(device: &ash::Device, cb: vk::CommandBuffer, barriers: &[Barrier]) {
    let built = build(barriers);
    if built.images.is_empty() && built.memory.is_none() {
        return;
    }
    let memory: Vec<_> = built.memory.into_iter().collect();
    let info = vk::DependencyInfo::default()
        .memory_barriers(&memory)
        .image_memory_barriers(&built.images);
    // SAFETY: the caller guarantees `cb` is recording; the info borrows locals alive for the call.
    unsafe { device.cmd_pipeline_barrier2(cb, &info) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use vk::ImageLayout as L;

    #[test]
    fn single_states_pick_their_layouts() {
        assert_eq!(
            map_state(RENDER_TARGET, false).layout,
            L::COLOR_ATTACHMENT_OPTIMAL
        );
        assert_eq!(
            map_state(DEPTH_WRITE, true).layout,
            L::DEPTH_STENCIL_ATTACHMENT_OPTIMAL
        );
        assert_eq!(
            map_state(DEPTH_READ, true).layout,
            L::DEPTH_STENCIL_READ_ONLY_OPTIMAL
        );
        assert_eq!(map_state(UNORDERED_ACCESS, false).layout, L::GENERAL);
        assert_eq!(map_state(COPY_DEST, false).layout, L::TRANSFER_DST_OPTIMAL);
        assert_eq!(
            map_state(COPY_SOURCE, false).layout,
            L::TRANSFER_SRC_OPTIMAL
        );
        assert_eq!(
            map_state(PIXEL_SHADER_RESOURCE, false).layout,
            L::SHADER_READ_ONLY_OPTIMAL
        );
        assert_eq!(
            map_state(PIXEL_SHADER_RESOURCE, true).layout,
            L::DEPTH_STENCIL_READ_ONLY_OPTIMAL
        );
        assert_eq!(map_state(0, false).layout, L::GENERAL);
    }

    #[test]
    fn mixed_states_fall_back_to_a_layout_that_allows_both() {
        let generic_read = VERTEX_AND_CONSTANT_BUFFER
            | INDEX_BUFFER
            | NON_PIXEL_SHADER_RESOURCE
            | PIXEL_SHADER_RESOURCE
            | INDIRECT_ARGUMENT
            | COPY_SOURCE;
        assert_eq!(map_state(generic_read, false).layout, L::READ_ONLY_OPTIMAL);
        assert_eq!(
            map_state(RENDER_TARGET | PIXEL_SHADER_RESOURCE, false).layout,
            L::GENERAL
        );
        assert_eq!(
            map_state(COPY_DEST | UNORDERED_ACCESS, false).layout,
            L::GENERAL
        );
    }

    #[test]
    fn accesses_follow_the_bits() {
        let map = map_state(INDEX_BUFFER | INDIRECT_ARGUMENT, false);
        assert!(
            map.access
                .contains(vk::AccessFlags2::INDEX_READ | vk::AccessFlags2::INDIRECT_COMMAND_READ)
        );
        assert!(map.stage.contains(
            vk::PipelineStageFlags2::INDEX_INPUT | vk::PipelineStageFlags2::DRAW_INDIRECT
        ));
        let write = map_state(UNORDERED_ACCESS, false);
        assert!(
            write
                .access
                .contains(vk::AccessFlags2::SHADER_STORAGE_WRITE)
        );
        assert!(
            write
                .stage
                .contains(vk::PipelineStageFlags2::COMPUTE_SHADER)
        );
    }

    fn image() -> ImageRef {
        ImageRef {
            image: vk::Image::null(),
            format: vk::Format::R8G8B8A8_UNORM,
            aspect: vk::ImageAspectFlags::COLOR,
            extent: [4, 4, 1],
            mips: 3,
            layers: 1,
        }
    }

    #[test]
    fn transitions_build_image_barriers_and_other_barriers_one_memory_barrier() {
        let built = build(&[
            Barrier::Transition {
                resource: Resource::Image(image()),
                subresource: None,
                before: RENDER_TARGET,
                after: PIXEL_SHADER_RESOURCE,
            },
            Barrier::Transition {
                resource: Resource::Image(image()),
                subresource: None,
                before: RENDER_TARGET,
                after: RENDER_TARGET,
            },
            Barrier::Transition {
                resource: Resource::Buffer,
                subresource: None,
                before: COPY_DEST,
                after: NON_PIXEL_SHADER_RESOURCE,
            },
            Barrier::Uav,
        ]);
        assert_eq!(built.images.len(), 1);
        assert_eq!(built.images[0].old_layout, L::COLOR_ATTACHMENT_OPTIMAL);
        assert_eq!(built.images[0].new_layout, L::SHADER_READ_ONLY_OPTIMAL);
        assert_eq!(
            built.images[0].subresource_range.level_count,
            vk::REMAINING_MIP_LEVELS
        );
        let memory = built.memory.unwrap();
        assert!(
            memory
                .src_access_mask
                .contains(vk::AccessFlags2::TRANSFER_WRITE | vk::AccessFlags2::MEMORY_WRITE)
        );
        assert!(build(&[]).memory.is_none());
    }
}
