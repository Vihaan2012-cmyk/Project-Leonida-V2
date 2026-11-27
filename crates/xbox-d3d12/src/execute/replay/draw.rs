//! Making a draw or dispatch legal: the pipeline, the push block, dynamic state and buffers, each bound only when
//! it differs from what the command buffer holds.

use ash::vk;

use super::util::{flip, rect2d};
use super::{Bound, PUSH_WORDS, Replayer};
use crate::execute::ReplayError;
use crate::execute::command::*;
use crate::pipeline::state::{default_topology, vk_topology};
use crate::pipeline::{Pipeline, PipelineKind};
use crate::root::{self, RootArguments, StageLayout};

/// Stages a pipeline has user data for: vertex, pixel, geometry or mesh, plus the compute one.
const MAX_STAGES: usize = 8;

impl Replayer<'_> {
    /// Makes a draw legal: scope open, pipeline bound, push block and dynamic state current, buffers bound.
    pub(super) fn prepare_draw(&mut self, indexed: bool) -> Result<(), ReplayError> {
        let Some(pipeline) = self.graphics.pipeline.take() else {
            return Err(ReplayError::NoPipeline("graphics"));
        };
        let result = self.draw_state(&pipeline, indexed);
        self.graphics.pipeline = Some(pipeline);
        result
    }

    pub(super) fn prepare_dispatch(&mut self) -> Result<(), ReplayError> {
        let Some(pipeline) = self.compute.pipeline.take() else {
            return Err(ReplayError::NoPipeline("compute"));
        };
        self.end_rendering();
        let result = self.bind(BindPoint::Compute, &pipeline);
        self.compute.pipeline = Some(pipeline);
        result
    }

    fn draw_state(&mut self, pipeline: &Pipeline, indexed: bool) -> Result<(), ReplayError> {
        self.begin_rendering();
        if pipeline.render_targets.len() != self.colors.len() {
            self.note("a draw with a different number of render targets than its pipeline has");
        }
        self.bind(BindPoint::Graphics, pipeline)?;
        let mesh = pipeline.kind == PipelineKind::Mesh;
        if self.dirty.any() {
            self.apply_dynamic();
        }
        if mesh {
            return Ok(());
        }
        let d = &self.ctx.device;
        let cb = self.cb;
        let topology = self
            .topology
            .and_then(vk_topology)
            .unwrap_or_else(|| default_topology(pipeline.topology, pipeline.has_geometry));
        if self.topology_applied != Some(topology) {
            // SAFETY: the pipeline declares topology dynamic.
            unsafe { d.cmd_set_primitive_topology(cb, topology) };
            self.topology_applied = Some(topology);
            self.report.stats.topology_sets += 1;
        }
        if self.vertex.any_unset() {
            let unset: Vec<u32> = self.vertex.unset().collect();
            for slot in unset {
                self.note(format!("a draw with vertex buffer slot {slot} unbound"));
            }
        }
        let mut from = 0;
        while self.vertex.any_pending()
            && let Some((first, views)) = self.vertex.next_run(from)
        {
            let count = views.len();
            let buffers = [self.env.arena.buffer; 32];
            let mut offsets = [0u64; 32];
            let mut sizes = [0u64; 32];
            let mut strides = [0u64; 32];
            for (k, view) in views.iter().enumerate() {
                let view = (*view).expect("a run holds set slots");
                offsets[k] = view.va;
                sizes[k] = u64::from(view.size).max(1);
                strides[k] = u64::from(view.stride);
            }
            // SAFETY: the stride is dynamic state of this pipeline; the ranges are in the arena.
            unsafe {
                d.cmd_bind_vertex_buffers2(
                    cb,
                    first,
                    &buffers[..count],
                    &offsets[..count],
                    Some(&sizes[..count]),
                    Some(&strides[..count]),
                )
            };
            self.vertex.applied(first, count);
            self.report.stats.vertex_binds += 1;
            from = first + count as u32;
        }
        if indexed {
            let Some(view) = self.index else {
                return Err(ReplayError::NoIndexBuffer);
            };
            let ty = if view.format == 57 {
                vk::IndexType::UINT16
            } else {
                vk::IndexType::UINT32
            };
            if self.index_applied != Some((view.va, ty)) {
                // SAFETY: the index range is in the arena.
                unsafe { d.cmd_bind_index_buffer(cb, self.env.arena.buffer, view.va, ty) };
                self.index_applied = Some((view.va, ty));
                self.report.stats.index_binds += 1;
            }
        }
        Ok(())
    }

    /// Sets the dynamic state the list changed since the last draw.
    fn apply_dynamic(&mut self) {
        let (d, cb) = (&self.ctx.device, self.cb);
        let area = self.render_area(self.colors.iter().copied().chain(self.depth));
        if self.dirty.viewport {
            let mut list = [vk::Viewport::default(); 16];
            let count = self.viewports.len().min(16);
            for (slot, v) in list.iter_mut().zip(&self.viewports) {
                *slot = flip(*v);
            }
            let count = if count == 0 {
                list[0] = flip(Viewport {
                    x: 0.0,
                    y: 0.0,
                    width: area.extent.width as f32,
                    height: area.extent.height as f32,
                    min_depth: 0.0,
                    max_depth: 1.0,
                });
                1
            } else {
                count
            };
            // SAFETY: `cb` is recording; the pipeline declares this state dynamic.
            unsafe { d.cmd_set_viewport_with_count(cb, &list[..count]) };
            self.report.stats.viewport_sets += 1;
        }
        if self.dirty.scissor {
            let mut list = [vk::Rect2D::default(); 16];
            let count = self.scissors.len().min(16);
            for (slot, r) in list.iter_mut().zip(&self.scissors) {
                *slot = rect2d(*r);
            }
            let count = if count == 0 {
                list[0] = area;
                1
            } else {
                count
            };
            // SAFETY: as above.
            unsafe { d.cmd_set_scissor_with_count(cb, &list[..count]) };
            self.report.stats.scissor_sets += 1;
        }
        if self.dirty.blend {
            // SAFETY: as above.
            unsafe { d.cmd_set_blend_constants(cb, &self.blend_factor) };
            self.report.stats.blend_sets += 1;
        }
        if self.dirty.stencil {
            // SAFETY: as above.
            unsafe {
                d.cmd_set_stencil_reference(
                    cb,
                    vk::StencilFaceFlags::FRONT_AND_BACK,
                    self.stencil_ref,
                )
            };
            self.report.stats.stencil_sets += 1;
        }
        self.dirty = Default::default();
    }

    /// Binds the pipeline and the image set, and brings the push constants current with the root arguments.
    fn bind(&mut self, bind: BindPoint, pipeline: &Pipeline) -> Result<(), ReplayError> {
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
            self.report.stats.pipeline_binds += 1;
            if bind == BindPoint::Graphics {
                let mut mask = 0u32;
                for slot in &pipeline.vertex_slots {
                    if slot.slot < 32 {
                        mask |= 1 << slot.slot;
                    }
                }
                self.vertex.pipeline_bound(mask);
                if pipeline.kind == PipelineKind::Mesh {
                    self.topology_applied = None;
                }
            }
        }
        if !bound.bindless
            && let Some(set) = self.env.bindless
        {
            // SAFETY: the set matches set 1 of the shared layout.
            unsafe { d.cmd_bind_descriptor_sets(cb, point, layout, 1, &[set], &[]) };
            bound.bindless = true;
        }
        if !bound.block_valid {
            let spill = &mut self.env.spill;
            let mut store = |bytes: &[u8]| spill.as_mut().and_then(|s| s.store(bytes));
            rebuild_block(
                bound,
                pipeline,
                self.heaps,
                self.env.arena.device_address,
                &mut store,
            )?;
        }
        if bound.block_changed || self.pushed_by != Some(bind) {
            bound.block_changed = false;
            self.pushed_by = Some(bind);
        } else {
            return Ok(());
        }
        if let Some(range) = self.pushed.update(&bound.block) {
            let words = &bound.block[range.clone()];
            // SAFETY: the layout's one range covers every stage and these words; they are plain integers read
            // as their own bytes.
            unsafe {
                let bytes =
                    std::slice::from_raw_parts(words.as_ptr().cast::<u8>(), words.len() * 4);
                d.cmd_push_constants(
                    cb,
                    layout,
                    vk::ShaderStageFlags::ALL,
                    (range.start * 4) as u32,
                    bytes,
                )
            };
            self.report.stats.push_calls += 1;
            self.report.stats.push_words += range.len() as u32;
        }
        Ok(())
    }

    pub(super) fn indirect(
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

/// Packs the root arguments for the pipeline's stages into the push block: the user data, the heap base and the
/// code base. A spilling signature has its array stored first.
fn rebuild_block(
    bound: &mut Bound,
    pipeline: &Pipeline,
    heaps: root::HeapBases,
    heap_base: u64,
    store: &mut dyn FnMut(&[u8]) -> Option<u64>,
) -> Result<(), ReplayError> {
    if bound.root.is_none() {
        bound.args = RootArguments::new(&pipeline.root);
    }
    let mut user = match pipeline.layouts.as_slice() {
        [] => {
            bound.plan_ok = bound.plan.build(&[]);
            root::pack(&[], &bound.args, heaps)?
        }
        [first, rest @ ..] => {
            let mut refs: [&StageLayout; MAX_STAGES] = [first; MAX_STAGES];
            let count = (1 + rest.len()).min(MAX_STAGES);
            for (slot, layout) in refs.iter_mut().zip(&pipeline.layouts) {
                *slot = layout;
            }
            bound.plan_ok = bound.plan.build(&refs[..count]);
            root::pack(&refs[..count], &bound.args, heaps)?
        }
    };
    if !user.spill.is_empty() {
        let va = store(&user.spill).ok_or(ReplayError::NoSpillSpace)?;
        user.sgprs[0] = va as u32;
        user.sgprs[1] = (va >> 32) as u32 & 0xFF;
    }
    let words: [u32; PUSH_WORDS] = pipeline.push_words(&user, heap_base);
    bound.block = words;
    bound.block_valid = true;
    bound.block_changed = true;
    Ok(())
}
