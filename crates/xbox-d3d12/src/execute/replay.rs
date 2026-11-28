//! Plays a recorded command stream onto a Vulkan command buffer.
//!
//! State a command list sets (root arguments, pipeline, viewports, targets) is kept here and applied when a draw
//! or dispatch needs it, and only where it differs from what the command buffer already holds:
//! - pipelines bind lazily, and the push-constant range is rewritten only in the words that changed;
//! - viewports, scissors, blend factor, stencil reference, topology, vertex and index buffers are set once per
//!   change, not once per draw;
//! - a dynamic-rendering scope opens at the first draw after the targets changed, takes clears that came right
//!   after the targets as its load operations, and closes only before something that cannot run inside one;
//! - barriers of consecutive commands are recorded in one call.

mod draw;
mod plan;
mod rendering;
mod util;

use std::sync::Arc;

use ash::vk;

use super::barrier;
use super::command::*;
use super::state::{PushShadow, VertexBinder};
use super::{ReplayError, VkContext};
use crate::pipeline::state::dxgi_to_vk;
use crate::pipeline::{Pipeline, PipelineKind};
use crate::root::{HeapBases, RootArgument, RootArguments, RootSignature};
use plan::Plan;
use util::{buffer_image_region, extent3, layers_of, offset3};

/// The arena: one buffer whose offsets are console VAs, and its device address (the heap base shaders add).
#[derive(Debug, Clone, Copy)]
pub struct Arena {
    pub buffer: vk::Buffer,
    pub device_address: u64,
}

/// Somewhere to put a root signature's spill array for the length of the submission.
pub trait SpillAllocator {
    /// Copies `bytes` into the arena and returns the console VA of the copy.
    fn store(&mut self, bytes: &[u8]) -> Option<u64>;
}

pub struct ReplayEnv<'a> {
    pub arena: Arena,
    /// The pipeline layout all pipelines share (`PipelineEnv::layout`).
    pub layout: vk::PipelineLayout,
    /// The image descriptor set, bound as set 1.
    pub bindless: Option<vk::DescriptorSet>,
    pub spill: Option<&'a mut dyn SpillAllocator>,
}

/// How many Vulkan calls of each kind the replay made, for the ones the replay tries to keep rare.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReplayStats {
    pub pipeline_binds: u32,
    pub push_calls: u32,
    pub push_words: u32,
    pub viewport_sets: u32,
    pub scissor_sets: u32,
    pub blend_sets: u32,
    pub stencil_sets: u32,
    pub topology_sets: u32,
    pub vertex_binds: u32,
    pub index_binds: u32,
    pub rendering_scopes: u32,
    pub barrier_calls: u32,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReplayReport {
    pub draws: u32,
    pub dispatches: u32,
    pub copies: u32,
    pub barriers: u32,
    pub stats: ReplayStats,
    /// What was approximated or skipped.
    pub notes: Vec<String>,
}

pub(crate) const PUSH_WORDS: usize = shader_translate::PUSH_WORDS as usize;

struct Bound {
    root: Option<Arc<RootSignature>>,
    args: RootArguments,
    pipeline: Option<Arc<Pipeline>>,
    handle: vk::Pipeline,
    /// The push block these arguments and this pipeline make; stale when `block_valid` is false.
    block: [u32; PUSH_WORDS],
    block_valid: bool,
    /// The block changed since it was last compared with what the command buffer holds.
    block_changed: bool,
    /// Where the arguments land for the bound pipeline, when a change can be patched into the block.
    plan: Plan,
    plan_ok: bool,
    bindless: bool,
}

impl Bound {
    fn new() -> Bound {
        Bound {
            root: None,
            args: RootArguments::default(),
            pipeline: None,
            handle: vk::Pipeline::null(),
            block: [0; PUSH_WORDS],
            block_valid: false,
            block_changed: false,
            plan: Plan::default(),
            plan_ok: false,
            bindless: false,
        }
    }

    /// An argument changed: patch it into the block, or have the block packed again.
    fn argument_changed(&mut self, param: u32) -> Result<(), ReplayError> {
        if !self.block_valid {
            return Ok(());
        }
        if self.plan_ok {
            self.plan
                .patch(&mut self.block[..32], param, self.args.get(param))?;
            self.block_changed = true;
        } else {
            self.block_valid = false;
        }
        Ok(())
    }
}

/// Dynamic state the list set that the command buffer does not hold yet.
#[derive(Default)]
struct DynamicDirty {
    viewport: bool,
    scissor: bool,
    blend: bool,
    stencil: bool,
}

impl DynamicDirty {
    fn all() -> DynamicDirty {
        DynamicDirty {
            viewport: true,
            scissor: true,
            blend: true,
            stencil: true,
        }
    }

    fn any(&self) -> bool {
        self.viewport || self.scissor || self.blend || self.stencil
    }
}

/// Clears that came right after the targets were set, which become load operations of the scope that opens.
#[derive(Default)]
struct PendingClears {
    colors: [Option<[f32; 4]>; 8],
    depth: Option<f32>,
    stencil: Option<u8>,
    any: bool,
}

pub struct Replayer<'a> {
    ctx: &'a VkContext,
    env: ReplayEnv<'a>,
    cb: vk::CommandBuffer,
    report: ReplayReport,
    graphics: Bound,
    compute: Bound,
    /// Push constants are one range for both bind points; this is what the command buffer holds, and which bind
    /// point's block it was last made equal to.
    pushed: PushShadow<PUSH_WORDS>,
    pushed_by: Option<BindPoint>,
    heaps: HeapBases,
    viewports: Viewports,
    scissors: Rects,
    blend_factor: [f32; 4],
    stencil_ref: u32,
    topology: Option<u32>,
    dirty: DynamicDirty,
    /// The topology the command buffer holds; `None` when the bound pipeline does not take it dynamically.
    topology_applied: Option<vk::PrimitiveTopology>,
    vertex: VertexBinder,
    index: Option<IndexBufferView>,
    index_applied: Option<(u64, vk::IndexType)>,
    colors: ColorTargets,
    depth: Option<TargetView>,
    rendering: bool,
    clears: PendingClears,
    barriers: barrier::Batch,
}

impl<'a> Replayer<'a> {
    pub fn new(ctx: &'a VkContext, env: ReplayEnv<'a>, cb: vk::CommandBuffer) -> Replayer<'a> {
        Replayer {
            ctx,
            env,
            cb,
            report: ReplayReport::default(),
            graphics: Bound::new(),
            compute: Bound::new(),
            pushed: PushShadow::default(),
            pushed_by: None,
            heaps: HeapBases::default(),
            viewports: Viewports::new(),
            scissors: Rects::new(),
            blend_factor: [1.0; 4],
            stencil_ref: 0,
            topology: None,
            dirty: DynamicDirty::all(),
            topology_applied: None,
            vertex: VertexBinder::default(),
            index: None,
            index_applied: None,
            colors: ColorTargets::new(),
            depth: None,
            rendering: false,
            clears: PendingClears::default(),
            barriers: barrier::Batch::default(),
        }
    }

    fn note(&mut self, text: impl Into<String>) {
        let text = text.into();
        if !self.report.notes.contains(&text) {
            self.report.notes.push(text);
        }
    }

    /// Records every command into the command buffer, which must be recording and outside a rendering scope.
    /// Returns what was done.
    pub fn record(mut self, commands: &[Command]) -> Result<ReplayReport, ReplayError> {
        for command in commands {
            if !command.is_lazy() {
                self.flush_barriers();
            }
            self.command(command)?;
        }
        self.flush_barriers();
        self.end_rendering();
        Ok(self.report)
    }

    fn flush_barriers(&mut self) {
        // SAFETY: `cb` is recording and outside a rendering scope: a scope never opens with barriers gathered.
        if unsafe { self.barriers.flush(&self.ctx.device, self.cb) } {
            self.report.stats.barrier_calls += 1;
        }
    }

    fn bound(&mut self, bind: BindPoint) -> &mut Bound {
        match bind {
            BindPoint::Graphics => &mut self.graphics,
            BindPoint::Compute => &mut self.compute,
        }
    }

    fn command(&mut self, command: &Command) -> Result<(), ReplayError> {
        let d = &self.ctx.device;
        let cb = self.cb;
        match command {
            Command::SetRootSignature { bind, root } => {
                let bound = self.bound(*bind);
                if !bound.root.as_ref().is_some_and(|r| Arc::ptr_eq(r, root)) {
                    bound.args = RootArguments::new(root);
                    bound.root = Some(root.clone());
                    bound.block_valid = false;
                }
            }
            Command::SetPipeline(pipeline) => {
                let bind = if pipeline.kind == PipelineKind::Compute {
                    BindPoint::Compute
                } else {
                    BindPoint::Graphics
                };
                let bound = self.bound(bind);
                if !bound
                    .pipeline
                    .as_ref()
                    .is_some_and(|p| Arc::ptr_eq(p, pipeline))
                {
                    bound.pipeline = Some(pipeline.clone());
                    bound.block_valid = false;
                }
            }
            Command::SetRootConstants {
                bind,
                param,
                first,
                values,
            } => {
                let bound = self.bound(*bind);
                let end = *first as usize + values.len();
                let same = matches!(
                    bound.args.get(*param),
                    Some(RootArgument::Constants(list))
                        if list.get(*first as usize..end) == Some(values.as_slice())
                );
                if !same {
                    bound.args.set_constants(*param, *first, values)?;
                    bound.argument_changed(*param)?;
                }
            }
            Command::SetRootView { bind, param, va } => {
                let bound = self.bound(*bind);
                if bound.args.get(*param) != Some(&RootArgument::View(*va)) {
                    bound.args.set_view(*param, *va)?;
                    bound.argument_changed(*param)?;
                }
            }
            Command::SetRootTable {
                bind,
                param,
                handle,
            } => {
                let bound = self.bound(*bind);
                if bound.args.get(*param) != Some(&RootArgument::Table(*handle)) {
                    bound.args.set_table(*param, *handle)?;
                    bound.argument_changed(*param)?;
                }
            }
            Command::SetDescriptorHeaps(bases) => {
                if self.heaps != *bases {
                    self.heaps = *bases;
                    self.graphics.block_valid = false;
                    self.compute.block_valid = false;
                }
            }
            Command::SetViewports(list) => {
                if self.viewports != *list {
                    self.viewports.clear();
                    self.viewports.extend_from_slice(list);
                    self.dirty.viewport = true;
                }
            }
            Command::SetScissors(list) => {
                if self.scissors != *list {
                    self.scissors.clear();
                    self.scissors.extend_from_slice(list);
                    self.dirty.scissor = true;
                }
            }
            Command::SetBlendFactor(factor) => {
                if self.blend_factor != *factor {
                    self.blend_factor = *factor;
                    self.dirty.blend = true;
                }
            }
            Command::SetStencilRef(value) => {
                if self.stencil_ref != *value {
                    self.stencil_ref = *value;
                    self.dirty.stencil = true;
                }
            }
            Command::SetTopology(value) => self.topology = Some(*value),
            Command::SetVertexBuffers { start, views } => {
                if self.vertex.set(*start, views) {
                    self.note("vertex buffers past slot 31 are ignored");
                }
            }
            Command::SetIndexBuffer(view) => self.index = *view,
            Command::SetRenderTargets { colors, depth } => {
                if self.colors != *colors || self.depth != *depth {
                    self.end_rendering();
                    self.colors.clear();
                    self.colors.extend_from_slice(colors);
                    self.depth = *depth;
                    self.dirty.viewport |= self.viewports.is_empty();
                    self.dirty.scissor |= self.scissors.is_empty();
                }
            }
            Command::ClearRenderTarget { view, color, rects } => {
                self.clear_target(*view, Some(*color), None, rects)
            }
            Command::ClearDepthStencil {
                view,
                depth,
                stencil,
                rects,
            } => self.clear_target(*view, None, Some((*depth, *stencil)), rects),
            Command::FillBuffer { va, bytes, value } => {
                self.end_rendering();
                // SAFETY: `cb` is recording and the arena buffer outlives the list.
                unsafe { d.cmd_fill_buffer(cb, self.env.arena.buffer, *va, *bytes, *value) };
                self.report.copies += 1;
            }
            Command::ClearImage {
                image,
                color,
                float,
            } => {
                self.end_rendering();
                let value = if *float {
                    vk::ClearColorValue {
                        float32: color.map(f32::from_bits),
                    }
                } else {
                    vk::ClearColorValue { uint32: *color }
                };
                let range = vk::ImageSubresourceRange::default()
                    .aspect_mask(image.aspect)
                    .level_count(vk::REMAINING_MIP_LEVELS)
                    .layer_count(vk::REMAINING_ARRAY_LAYERS);
                // SAFETY: `cb` is recording; the image is in the GENERAL layout by the barrier before.
                unsafe {
                    d.cmd_clear_color_image(
                        cb,
                        image.image,
                        vk::ImageLayout::GENERAL,
                        &value,
                        &[range],
                    )
                };
                self.report.copies += 1;
            }
            Command::Draw {
                vertices,
                instances,
                first_vertex,
                first_instance,
            } => {
                self.prepare_draw(false)?;
                // SAFETY: state for the draw was applied by `prepare_draw`.
                unsafe { d.cmd_draw(cb, *vertices, *instances, *first_vertex, *first_instance) };
                self.report.draws += 1;
            }
            Command::DrawIndexed {
                indices,
                instances,
                first_index,
                base_vertex,
                first_instance,
            } => {
                self.prepare_draw(true)?;
                // SAFETY: as above.
                unsafe {
                    d.cmd_draw_indexed(
                        cb,
                        *indices,
                        *instances,
                        *first_index,
                        *base_vertex,
                        *first_instance,
                    )
                };
                self.report.draws += 1;
            }
            Command::Dispatch(groups) => {
                self.prepare_dispatch()?;
                // SAFETY: the compute pipeline and its push block are bound.
                unsafe { d.cmd_dispatch(cb, groups[0], groups[1], groups[2]) };
                self.report.dispatches += 1;
            }
            Command::DispatchMesh(groups) => {
                let Some(mesh) = &self.ctx.mesh else {
                    self.note("DispatchMesh on a GPU without mesh shaders");
                    return Ok(());
                };
                self.prepare_draw(false)?;
                // SAFETY: a mesh pipeline is bound and rendering is open.
                unsafe { mesh.cmd_draw_mesh_tasks(cb, groups[0], groups[1], groups[2]) };
                self.report.draws += 1;
            }
            Command::ExecuteIndirect {
                kind,
                max_count,
                args,
                stride,
                count,
            } => self.indirect(*kind, *max_count, *args, *stride, *count)?,
            Command::CopyBuffer { dst, src, bytes } => {
                self.end_rendering();
                let region = vk::BufferCopy::default()
                    .src_offset(*src)
                    .dst_offset(*dst)
                    .size(*bytes);
                let buffer = self.env.arena.buffer;
                // SAFETY: `cb` is recording and the regions are in the arena.
                unsafe { d.cmd_copy_buffer(cb, buffer, buffer, &[region]) };
                self.report.copies += 1;
            }
            Command::CopyBufferToImage {
                dst,
                sub,
                at,
                src,
                size,
            } => {
                self.end_rendering();
                let region = buffer_image_region(dst, *sub, *at, *size, src);
                // SAFETY: the image is in TRANSFER_DST_OPTIMAL by the barrier before.
                unsafe {
                    d.cmd_copy_buffer_to_image(
                        cb,
                        self.env.arena.buffer,
                        dst.image,
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        &[region],
                    )
                };
                self.report.copies += 1;
            }
            Command::CopyImageToBuffer {
                dst,
                src,
                sub,
                region: boxed,
            } => {
                self.end_rendering();
                let region = buffer_image_region(src, *sub, boxed.origin, boxed.size, dst);
                // SAFETY: the image is in TRANSFER_SRC_OPTIMAL by the barrier before.
                unsafe {
                    d.cmd_copy_image_to_buffer(
                        cb,
                        src.image,
                        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                        self.env.arena.buffer,
                        &[region],
                    )
                };
                self.report.copies += 1;
            }
            Command::CopyImage {
                dst,
                dst_sub,
                at,
                src,
                src_sub,
                region,
            } => {
                self.end_rendering();
                let copy = vk::ImageCopy::default()
                    .src_subresource(layers_of(src, *src_sub))
                    .src_offset(offset3(region.origin))
                    .dst_subresource(layers_of(dst, *dst_sub))
                    .dst_offset(offset3(*at))
                    .extent(extent3(region.size));
                // SAFETY: layouts are TRANSFER_SRC/DST by the barriers before.
                unsafe {
                    d.cmd_copy_image(
                        cb,
                        src.image,
                        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                        dst.image,
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        &[copy],
                    )
                };
                self.report.copies += 1;
            }
            Command::ResolveImage {
                dst,
                dst_sub,
                src,
                src_sub,
            } => {
                self.end_rendering();
                let resolve = vk::ImageResolve::default()
                    .src_subresource(layers_of(src, *src_sub))
                    .dst_subresource(layers_of(dst, *dst_sub))
                    .extent(extent3(dst.mip_extent(dst_sub.mip)));
                // SAFETY: layouts are TRANSFER_SRC/DST by the barriers before.
                unsafe {
                    d.cmd_resolve_image(
                        cb,
                        src.image,
                        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                        dst.image,
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        &[resolve],
                    )
                };
                self.report.copies += 1;
            }
            Command::Barriers(list) => {
                self.end_rendering();
                if self.barriers.conflicts(list) {
                    self.flush_barriers();
                }
                self.barriers.add(list);
                self.report.barriers += list.len() as u32;
            }
            Command::BeginQuery { pool, kind, index } => {
                self.end_rendering();
                // SAFETY: `cb` is recording and outside a rendering scope for the reset.
                unsafe {
                    d.cmd_reset_query_pool(cb, *pool, *index, 1);
                    let flags = if *kind == QueryKind::Occlusion {
                        vk::QueryControlFlags::PRECISE
                    } else {
                        vk::QueryControlFlags::empty()
                    };
                    d.cmd_begin_query(cb, *pool, *index, flags);
                }
            }
            Command::EndQuery { pool, kind, index } => {
                self.end_rendering();
                // SAFETY: `cb` is recording.
                unsafe {
                    if *kind == QueryKind::Timestamp {
                        d.cmd_reset_query_pool(cb, *pool, *index, 1);
                        d.cmd_write_timestamp2(
                            cb,
                            vk::PipelineStageFlags2::ALL_COMMANDS,
                            *pool,
                            *index,
                        );
                    } else {
                        d.cmd_end_query(cb, *pool, *index);
                    }
                }
            }
            Command::ResolveQueries {
                pool,
                first,
                count,
                dst,
            } => {
                self.end_rendering();
                // SAFETY: `cb` is recording; the destination is in the arena.
                unsafe {
                    d.cmd_copy_query_pool_results(
                        cb,
                        *pool,
                        *first,
                        *count,
                        self.env.arena.buffer,
                        *dst,
                        8,
                        vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WAIT,
                    )
                };
                self.report.copies += 1;
            }
            Command::Marker(_) => {}
        }
        Ok(())
    }
}

/// Records `commands` into a one-shot command buffer, submits it and waits: for tests and tools. The device
/// layer's queue does its own submission.
pub fn run<'a>(
    ctx: &'a VkContext,
    env: ReplayEnv<'a>,
    commands: &[Command],
) -> Result<ReplayReport, ReplayError> {
    let d = &ctx.device;
    // SAFETY: every handle is created from `d` and destroyed before returning, after the fence wait.
    unsafe {
        let pool = d.create_command_pool(
            &vk::CommandPoolCreateInfo::default().queue_family_index(ctx.family),
            None,
        )?;
        let result = (|| {
            let cb = d.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .command_buffer_count(1),
            )?[0];
            d.begin_command_buffer(
                cb,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            let report = Replayer::new(ctx, env, cb).record(commands)?;
            d.end_command_buffer(cb)?;
            let fence = d.create_fence(&vk::FenceCreateInfo::default(), None)?;
            let buffers = [cb];
            let submit = [vk::SubmitInfo::default().command_buffers(&buffers)];
            let waited = d
                .queue_submit(ctx.queue, &submit, fence)
                .and_then(|()| d.wait_for_fences(&[fence], true, u64::MAX));
            d.destroy_fence(fence, None);
            waited?;
            Ok(report)
        })();
        d.destroy_command_pool(pool, None);
        result
    }
}

/// The DXGI format of an index buffer view as Vulkan's index type, for callers that build views.
pub fn index_type(format: u32) -> Option<vk::IndexType> {
    match dxgi_to_vk(format)? {
        vk::Format::R16_UINT => Some(vk::IndexType::UINT16),
        vk::Format::R32_UINT => Some(vk::IndexType::UINT32),
        _ => None,
    }
}
