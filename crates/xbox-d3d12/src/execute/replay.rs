//! Plays a recorded command stream onto a Vulkan command buffer.
//!
//! State a command list sets (root arguments, pipeline, viewports, targets) is kept here and applied when a draw
//! or dispatch needs it: pipelines bind lazily, the push-constant block is rebuilt when a root argument or the
//! pipeline changed, and a dynamic-rendering scope opens at the first draw after the targets changed and closes
//! before anything that cannot run inside one.

use std::sync::Arc;

use ash::vk;

use super::barrier;
use super::command::*;
use super::{ReplayError, VkContext};
use crate::pipeline::state::{block_size, dxgi_to_vk, format_bytes, has_stencil, vk_topology};
use crate::pipeline::{Pipeline, PipelineKind};
use crate::root::{self, HeapBases, RootArguments, RootSignature};

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

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReplayReport {
    pub draws: u32,
    pub dispatches: u32,
    pub copies: u32,
    pub barriers: u32,
    /// What was approximated or skipped.
    pub notes: Vec<String>,
}

#[derive(Default)]
struct Bound {
    root: Option<Arc<RootSignature>>,
    args: RootArguments,
    pipeline: Option<Arc<Pipeline>>,
    handle: vk::Pipeline,
    push_dirty: bool,
    bindless: bool,
}

pub struct Replayer<'a> {
    ctx: &'a VkContext,
    env: ReplayEnv<'a>,
    cb: vk::CommandBuffer,
    report: ReplayReport,
    graphics: Bound,
    compute: Bound,
    heaps: HeapBases,
    viewports: Vec<Viewport>,
    scissors: Vec<Rect>,
    blend_factor: [f32; 4],
    stencil_ref: u32,
    topology: Option<u32>,
    vertex: Vec<Option<VertexBufferView>>,
    index: Option<IndexBufferView>,
    colors: Vec<TargetView>,
    depth: Option<TargetView>,
    rendering: bool,
    dynamic_dirty: bool,
}

impl<'a> Replayer<'a> {
    pub fn new(ctx: &'a VkContext, env: ReplayEnv<'a>, cb: vk::CommandBuffer) -> Replayer<'a> {
        Replayer {
            ctx,
            env,
            cb,
            report: ReplayReport::default(),
            graphics: Bound::default(),
            compute: Bound::default(),
            heaps: HeapBases::default(),
            viewports: Vec::new(),
            scissors: Vec::new(),
            blend_factor: [1.0; 4],
            stencil_ref: 0,
            topology: None,
            vertex: Vec::new(),
            index: None,
            colors: Vec::new(),
            depth: None,
            rendering: false,
            dynamic_dirty: true,
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
            self.command(command)?;
        }
        self.end_rendering();
        Ok(self.report)
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
                }
                bound.push_dirty = true;
            }
            Command::SetPipeline(pipeline) => {
                let bind = if pipeline.kind == PipelineKind::Compute {
                    BindPoint::Compute
                } else {
                    BindPoint::Graphics
                };
                let bound = self.bound(bind);
                bound.pipeline = Some(pipeline.clone());
                bound.push_dirty = true;
                self.dynamic_dirty = true;
            }
            Command::SetRootConstants {
                bind,
                param,
                first,
                values,
            } => {
                let bound = self.bound(*bind);
                bound.args.set_constants(*param, *first, values)?;
                bound.push_dirty = true;
            }
            Command::SetRootView { bind, param, va } => {
                let bound = self.bound(*bind);
                bound.args.set_view(*param, *va)?;
                bound.push_dirty = true;
            }
            Command::SetRootTable {
                bind,
                param,
                handle,
            } => {
                let bound = self.bound(*bind);
                bound.args.set_table(*param, *handle)?;
                bound.push_dirty = true;
            }
            Command::SetDescriptorHeaps(bases) => {
                self.heaps = *bases;
                self.graphics.push_dirty = true;
                self.compute.push_dirty = true;
            }
            Command::SetViewports(list) => {
                self.viewports = list.clone();
                self.dynamic_dirty = true;
            }
            Command::SetScissors(list) => {
                self.scissors = list.clone();
                self.dynamic_dirty = true;
            }
            Command::SetBlendFactor(factor) => {
                self.blend_factor = *factor;
                self.dynamic_dirty = true;
            }
            Command::SetStencilRef(value) => {
                self.stencil_ref = *value;
                self.dynamic_dirty = true;
            }
            Command::SetTopology(value) => {
                self.topology = Some(*value);
                self.dynamic_dirty = true;
            }
            Command::SetVertexBuffers { start, views } => {
                let end = *start as usize + views.len();
                if self.vertex.len() < end {
                    self.vertex.resize(end, None);
                }
                for (k, view) in views.iter().enumerate() {
                    self.vertex[*start as usize + k] = Some(*view);
                }
            }
            Command::SetIndexBuffer(view) => self.index = *view,
            Command::SetRenderTargets { colors, depth } => {
                self.end_rendering();
                self.colors = colors.clone();
                self.depth = *depth;
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
                // SAFETY: `cb` is recording and outside a rendering scope.
                unsafe { barrier::record(d, cb, list) };
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

    fn end_rendering(&mut self) {
        if self.rendering {
            // SAFETY: a rendering scope is open on `cb`.
            unsafe { self.ctx.device.cmd_end_rendering(self.cb) };
            self.rendering = false;
        }
    }

    fn render_area(&self, views: &[TargetView]) -> vk::Rect2D {
        let (w, h) = views.iter().fold((u32::MAX, u32::MAX), |(w, h), v| {
            (w.min(v.extent[0]), h.min(v.extent[1]))
        });
        vk::Rect2D {
            offset: vk::Offset2D::default(),
            extent: vk::Extent2D {
                width: if w == u32::MAX { 1 } else { w },
                height: if h == u32::MAX { 1 } else { h },
            },
        }
    }

    /// Opens a rendering scope on the bound targets.
    fn begin_rendering(&mut self) {
        if self.rendering {
            return;
        }
        let all: Vec<TargetView> = self.colors.iter().copied().chain(self.depth).collect();
        let colors: Vec<_> = self
            .colors
            .iter()
            .map(|v| attachment(v.view, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL, None))
            .collect();
        let depth = self.depth.map(|v| {
            attachment(
                v.view,
                vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
                None,
            )
        });
        let stencil = self.depth.filter(|v| has_stencil(v.format)).map(|v| {
            attachment(
                v.view,
                vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
                None,
            )
        });
        self.open_rendering(&all, &colors, depth.as_ref(), stencil.as_ref());
    }

    fn open_rendering(
        &mut self,
        all: &[TargetView],
        colors: &[vk::RenderingAttachmentInfo<'_>],
        depth: Option<&vk::RenderingAttachmentInfo<'_>>,
        stencil: Option<&vk::RenderingAttachmentInfo<'_>>,
    ) {
        let mut info = vk::RenderingInfo::default()
            .render_area(self.render_area(all))
            .layer_count(1)
            .color_attachments(colors);
        if let Some(d) = depth {
            info = info.depth_attachment(d);
        }
        if let Some(s) = stencil {
            info = info.stencil_attachment(s);
        }
        // SAFETY: `cb` is recording outside a rendering scope; the views outlive the list.
        unsafe { self.ctx.device.cmd_begin_rendering(self.cb, &info) };
        self.rendering = true;
    }

    fn clear_target(
        &mut self,
        view: TargetView,
        color: Option<[f32; 4]>,
        ds: Option<(Option<f32>, Option<u8>)>,
        rects: &[Rect],
    ) {
        let is_bound = self.colors.iter().any(|v| v.view == view.view)
            || self.depth.is_some_and(|v| v.view == view.view);
        let was_open = self.rendering;
        if !(was_open && is_bound) {
            self.end_rendering();
            let (colors, depth, stencil);
            let ready = |v: vk::ImageView, layout, clear| attachment(v, layout, clear);
            if let Some(c) = color {
                let value = vk::ClearValue {
                    color: vk::ClearColorValue { float32: c },
                };
                colors = vec![ready(
                    view.view,
                    vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                    rects.is_empty().then_some(value),
                )];
                depth = None;
                stencil = None;
            } else {
                let (d, s) = ds.unwrap_or((None, None));
                colors = Vec::new();
                let value = |d: f32, s: u8| vk::ClearValue {
                    depth_stencil: vk::ClearDepthStencilValue {
                        depth: d,
                        stencil: u32::from(s),
                    },
                };
                let layout = vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL;
                depth = Some(ready(
                    view.view,
                    layout,
                    (rects.is_empty() && d.is_some()).then(|| value(d.unwrap_or(0.0), 0)),
                ));
                stencil = has_stencil(view.format).then(|| {
                    ready(
                        view.view,
                        layout,
                        (rects.is_empty() && s.is_some()).then(|| value(0.0, s.unwrap_or(0))),
                    )
                });
            }
            self.open_rendering(&[view], &colors, depth.as_ref(), stencil.as_ref());
            if rects.is_empty() {
                self.end_rendering();
                return;
            }
        }
        let area = vk::Rect2D {
            offset: vk::Offset2D::default(),
            extent: vk::Extent2D {
                width: view.extent[0],
                height: view.extent[1],
            },
        };
        let regions: Vec<vk::ClearRect> = if rects.is_empty() {
            vec![vk::ClearRect {
                rect: area,
                base_array_layer: 0,
                layer_count: 1,
            }]
        } else {
            rects
                .iter()
                .map(|r| vk::ClearRect {
                    rect: rect2d(*r),
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .collect()
        };
        let mut attachments = Vec::new();
        if let Some(c) = color {
            let index = self
                .colors
                .iter()
                .position(|v| v.view == view.view)
                .unwrap_or(0) as u32;
            attachments.push(vk::ClearAttachment {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                color_attachment: if was_open && is_bound { index } else { 0 },
                clear_value: vk::ClearValue {
                    color: vk::ClearColorValue { float32: c },
                },
            });
        } else if let Some((d, s)) = ds {
            let mut aspect = vk::ImageAspectFlags::empty();
            if d.is_some() {
                aspect |= vk::ImageAspectFlags::DEPTH;
            }
            if s.is_some() && has_stencil(view.format) {
                aspect |= vk::ImageAspectFlags::STENCIL;
            }
            attachments.push(vk::ClearAttachment {
                aspect_mask: aspect,
                color_attachment: 0,
                clear_value: vk::ClearValue {
                    depth_stencil: vk::ClearDepthStencilValue {
                        depth: d.unwrap_or(0.0),
                        stencil: u32::from(s.unwrap_or(0)),
                    },
                },
            });
        }
        // SAFETY: a rendering scope with the view is open on `cb`.
        unsafe {
            self.ctx
                .device
                .cmd_clear_attachments(self.cb, &attachments, &regions)
        };
        if !(was_open && is_bound) {
            self.end_rendering();
        }
    }

    /// Makes a draw legal: scope open, pipeline bound, push block and dynamic state current, buffers bound.
    fn prepare_draw(&mut self, indexed: bool) -> Result<(), ReplayError> {
        let Some(pipeline) = self.graphics.pipeline.clone() else {
            return Err(ReplayError::NoPipeline("graphics"));
        };
        self.begin_rendering();
        if pipeline.render_targets.len() != self.colors.len() {
            self.note("a draw with a different number of render targets than its pipeline has");
        }
        self.bind(BindPoint::Graphics, &pipeline)?;
        let d = &self.ctx.device;
        let cb = self.cb;
        let mesh = pipeline.kind == PipelineKind::Mesh;
        if self.dynamic_dirty {
            let area = self.render_area(
                &self
                    .colors
                    .iter()
                    .copied()
                    .chain(self.depth)
                    .collect::<Vec<_>>(),
            );
            let viewports: Vec<vk::Viewport> = if self.viewports.is_empty() {
                vec![flip(Viewport {
                    x: 0.0,
                    y: 0.0,
                    width: area.extent.width as f32,
                    height: area.extent.height as f32,
                    min_depth: 0.0,
                    max_depth: 1.0,
                })]
            } else {
                self.viewports.iter().copied().map(flip).collect()
            };
            let scissors: Vec<vk::Rect2D> = if self.scissors.is_empty() {
                vec![area]
            } else {
                self.scissors.iter().copied().map(rect2d).collect()
            };
            // SAFETY: `cb` is recording; the pipeline declares these states dynamic.
            unsafe {
                d.cmd_set_viewport_with_count(cb, &viewports);
                d.cmd_set_scissor_with_count(cb, &scissors);
                d.cmd_set_blend_constants(cb, &self.blend_factor);
                d.cmd_set_stencil_reference(
                    cb,
                    vk::StencilFaceFlags::FRONT_AND_BACK,
                    self.stencil_ref,
                );
            }
            self.dynamic_dirty = false;
        }
        if !mesh {
            let topology = self.topology.and_then(vk_topology).unwrap_or_else(|| {
                crate::pipeline::state::default_topology(pipeline.topology, pipeline.has_geometry)
            });
            // SAFETY: the pipeline declares topology dynamic.
            unsafe { d.cmd_set_primitive_topology(cb, topology) };
            for slot in &pipeline.vertex_slots {
                match self.vertex.get(slot.slot as usize).copied().flatten() {
                    Some(view) => {
                        let size = u64::from(view.size);
                        // SAFETY: the stride is dynamic state of this pipeline; the range is in the arena.
                        unsafe {
                            d.cmd_bind_vertex_buffers2(
                                cb,
                                slot.slot,
                                &[self.env.arena.buffer],
                                &[view.va],
                                Some(&[size.max(1)]),
                                Some(&[u64::from(view.stride)]),
                            )
                        };
                    }
                    None => self.note(format!(
                        "a draw with vertex buffer slot {} unbound",
                        slot.slot
                    )),
                }
            }
            if indexed {
                match self.index {
                    Some(view) => {
                        let ty = if view.format == 57 {
                            vk::IndexType::UINT16
                        } else {
                            vk::IndexType::UINT32
                        };
                        // SAFETY: the index range is in the arena.
                        unsafe { d.cmd_bind_index_buffer(cb, self.env.arena.buffer, view.va, ty) };
                    }
                    None => return Err(ReplayError::NoIndexBuffer),
                }
            }
        }
        Ok(())
    }

    fn prepare_dispatch(&mut self) -> Result<(), ReplayError> {
        let Some(pipeline) = self.compute.pipeline.clone() else {
            return Err(ReplayError::NoPipeline("compute"));
        };
        self.end_rendering();
        self.bind(BindPoint::Compute, &pipeline)
    }

    /// Binds the pipeline, the image set and a push block current with the root arguments.
    fn bind(&mut self, bind: BindPoint, pipeline: &Arc<Pipeline>) -> Result<(), ReplayError> {
        let point = pipeline.bind_point();
        let (d, cb, layout) = (&self.ctx.device, self.cb, self.env.layout);
        let bound = match bind {
            BindPoint::Graphics => &mut self.graphics,
            BindPoint::Compute => &mut self.compute,
        };
        if bound.handle != pipeline.handle {
            // SAFETY: `cb` is recording and the pipeline outlives the list.
            unsafe { d.cmd_bind_pipeline(cb, point, pipeline.handle) };
            bound.handle = pipeline.handle;
            bound.push_dirty = true;
        }
        if !bound.bindless
            && let Some(set) = self.env.bindless
        {
            // SAFETY: the set matches set 1 of the shared layout.
            unsafe { d.cmd_bind_descriptor_sets(cb, point, layout, 1, &[set], &[]) };
            bound.bindless = true;
        }
        if !bound.push_dirty {
            return Ok(());
        }
        if bound.root.is_none() {
            bound.args = RootArguments::new(&pipeline.root);
        }
        let layouts = pipeline.stage_layouts();
        let mut user = root::pack(&layouts, &bound.args, self.heaps)?;
        if !user.spill.is_empty() {
            let va = self
                .env
                .spill
                .as_mut()
                .and_then(|s| s.store(&user.spill))
                .ok_or(ReplayError::NoSpillSpace)?;
            user.sgprs[0] = va as u32;
            user.sgprs[1] = (va >> 32) as u32 & 0xFF;
        }
        let words = pipeline.push_words(&user, self.env.arena.device_address);
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        // SAFETY: the layout's one range covers every stage and these bytes.
        unsafe { d.cmd_push_constants(cb, layout, vk::ShaderStageFlags::ALL, 0, &bytes) };
        bound.push_dirty = false;
        Ok(())
    }

    fn indirect(
        &mut self,
        kind: IndirectKind,
        max: u32,
        args: u64,
        stride: u32,
        count: Option<u64>,
    ) -> Result<(), ReplayError> {
        let width = match kind {
            IndirectKind::Draw => 16,
            IndirectKind::DrawIndexed => 20,
            IndirectKind::Dispatch | IndirectKind::DispatchMesh => 12,
        };
        let stride = if stride == 0 { width } else { stride };
        match kind {
            IndirectKind::Dispatch => self.prepare_dispatch()?,
            IndirectKind::DispatchMesh => {
                if self.ctx.mesh.is_none() {
                    self.note("DispatchMesh on a GPU without mesh shaders");
                    return Ok(());
                }
                self.prepare_draw(false)?;
            }
            IndirectKind::Draw => self.prepare_draw(false)?,
            IndirectKind::DrawIndexed => self.prepare_draw(true)?,
        }
        let (d, cb, buffer) = (&self.ctx.device, self.cb, self.env.arena.buffer);
        let multi = self.ctx.features.multi_draw_indirect;
        // SAFETY: the pipeline, state and arena ranges are as the draw or dispatch needs.
        unsafe {
            match (kind, count) {
                (IndirectKind::Draw, Some(c)) => {
                    d.cmd_draw_indirect_count(cb, buffer, args, buffer, c, max, stride)
                }
                (IndirectKind::DrawIndexed, Some(c)) => {
                    d.cmd_draw_indexed_indirect_count(cb, buffer, args, buffer, c, max, stride)
                }
                (IndirectKind::DispatchMesh, Some(c)) => {
                    if let Some(mesh) = &self.ctx.mesh {
                        mesh.cmd_draw_mesh_tasks_indirect_count(
                            cb, buffer, args, buffer, c, max, stride,
                        )
                    }
                }
                (IndirectKind::DispatchMesh, None) => {
                    if let Some(mesh) = &self.ctx.mesh {
                        mesh.cmd_draw_mesh_tasks_indirect(cb, buffer, args, max, stride)
                    }
                }
                (IndirectKind::Draw, None) if multi || max <= 1 => {
                    d.cmd_draw_indirect(cb, buffer, args, max, stride)
                }
                (IndirectKind::DrawIndexed, None) if multi || max <= 1 => {
                    d.cmd_draw_indexed_indirect(cb, buffer, args, max, stride)
                }
                (IndirectKind::Draw, None) => {
                    for i in 0..max {
                        d.cmd_draw_indirect(cb, buffer, args + u64::from(i * stride), 1, stride)
                    }
                }
                (IndirectKind::DrawIndexed, None) => {
                    for i in 0..max {
                        d.cmd_draw_indexed_indirect(
                            cb,
                            buffer,
                            args + u64::from(i * stride),
                            1,
                            stride,
                        )
                    }
                }
                (IndirectKind::Dispatch, _) => {
                    for i in 0..max {
                        d.cmd_dispatch_indirect(cb, buffer, args + u64::from(i * stride))
                    }
                }
            }
        }
        if count.is_some() && kind == IndirectKind::Dispatch {
            self.note("an indirect dispatch with a count buffer runs every record");
        }
        match kind {
            IndirectKind::Dispatch => self.report.dispatches += max,
            _ => self.report.draws += max,
        }
        Ok(())
    }
}

fn attachment(
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
fn flip(v: Viewport) -> vk::Viewport {
    vk::Viewport {
        x: v.x,
        y: v.y + v.height,
        width: v.width,
        height: -v.height,
        min_depth: v.min_depth,
        max_depth: v.max_depth,
    }
}

fn rect2d(r: Rect) -> vk::Rect2D {
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

fn offset3(o: [u32; 3]) -> vk::Offset3D {
    vk::Offset3D {
        x: o[0] as i32,
        y: o[1] as i32,
        z: o[2] as i32,
    }
}

fn extent3(e: [u32; 3]) -> vk::Extent3D {
    vk::Extent3D {
        width: e[0],
        height: e[1],
        depth: e[2],
    }
}

fn copy_aspect(image: &ImageRef) -> vk::ImageAspectFlags {
    if image.aspect.contains(vk::ImageAspectFlags::DEPTH) {
        vk::ImageAspectFlags::DEPTH
    } else {
        image.aspect
    }
}

fn layers_of(image: &ImageRef, sub: Subresource) -> vk::ImageSubresourceLayers {
    vk::ImageSubresourceLayers::default()
        .aspect_mask(copy_aspect(image))
        .mip_level(sub.mip)
        .base_array_layer(sub.layer)
        .layer_count(1)
}

fn buffer_image_region(
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
