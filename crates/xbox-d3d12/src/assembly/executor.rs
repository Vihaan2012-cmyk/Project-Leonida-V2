//! `Executor`: closed command lists replayed on the Vulkan queue.
//!
//! One `ExecuteCommandLists` call is one submission: every list is converted to replay commands (`bridge`), the
//! descriptors they can reach are registered and the bindless set brought up to date, one command buffer is
//! recorded (host memory copied into a mirrored window first, what the GPU wrote copied back after) and the call
//! returns when the GPU has finished. Submissions run one at a time.

use std::ffi::c_void;
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};

use ash::vk;
use content_filter::{DrawDecision, ResourceId as FilterId};

use super::bridge::{self, BufferRef, Converted, Objects, Resolver};
use super::hooks;
use super::indirect::IndirectLayout;
use super::objects::{pipeline_of, resource_of, root_of, signature_of};
use super::pipelines::PendingPipeline;
use super::resapi::image_ref;
use super::state::Assembly;
use super::views::identity;
use crate::com::{self, ComPtr, Kind};
use crate::compile_queue::DrawPolicy;
use crate::descriptor::vulkan::ViewInfo;
use crate::descriptor::{self, gfx10};
use crate::execute::{
    Arena, ImageRef, ReplayEnv, Replayer, SpillAllocator, TargetView,
    barrier::{COPY_DEST, NON_PIXEL_SHADER_RESOURCE, PIXEL_SHADER_RESOURCE},
};
use crate::heap;
use crate::list;
use crate::memory::Backing;
use crate::queue::{Executor, QueueInfo, TileCall};
use crate::resource::{Origin, Resources};
use crate::root::{HeapBases, ParameterKind, RangeType, RootSignature};
use crate::stream::{self, CommandStream};

pub struct AssemblyExecutor {
    asm: Arc<Assembly>,
}

impl AssemblyExecutor {
    pub fn new(asm: Arc<Assembly>) -> AssemblyExecutor {
        AssemblyExecutor { asm }
    }
}

impl Executor for AssemblyExecutor {
    fn execute(&self, _queue: &QueueInfo, lists: &[Arc<CommandStream>]) {
        if let Err(e) = self.asm.execute(lists) {
            com::note_once(format!("a submission failed: {e}"));
        }
    }

    fn tile_mappings(&self, _queue: &QueueInfo, call: TileCall) {
        let what = match call {
            TileCall::Update { .. } => "UpdateTileMappings",
            TileCall::Copy { .. } => "CopyTileMappings",
        };
        com::note_once(format!(
            "{what} was called; tiles are heap memory this layer has no heaps for, so the mapping is not applied"
        ));
    }
}

/// How often a large descriptor heap is looked through again for descriptors the title wrote itself.
const HEAP_SCAN_INTERVAL: Duration = Duration::from_millis(100);
const SMALL_HEAP: u32 = 4096;
const UNBOUNDED_TABLE: u64 = 1 << 16;

/// What the conversion needs of the assembly, over the resource store locked for the submission.
struct Context<'a> {
    asm: &'a Assembly,
    resources: &'a Resources,
    filter: Option<hooks::FilterHandle>,
}

/// A heap's type and GPU start, through its own vtable: the heap object is the heap step's.
fn heap_words(ptr: *mut c_void) -> Option<([u32; 4], u64)> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: `ptr` is a COM object of this layer; slots 8 and 10 of a descriptor heap are `GetDesc` (buffer
    // first) and `GetGPUDescriptorHandleForHeapStart`.
    unsafe {
        if com::kind_of(ptr)? != Kind::DescriptorHeap {
            return None;
        }
        let vtable = *(ptr as *const *const usize);
        let get_desc: unsafe extern "win64" fn(*mut u32, *mut c_void) -> *mut u32 =
            std::mem::transmute(*vtable.add(8));
        let get_gpu: unsafe extern "win64" fn(*mut c_void) -> u64 =
            std::mem::transmute(*vtable.add(10));
        let mut desc = [0u32; 4];
        get_desc(desc.as_mut_ptr(), ptr);
        Some((desc, get_gpu(ptr)))
    }
}

impl Context<'_> {
    fn target(&self, handle: u64, depth: bool) -> Option<TargetView> {
        // SAFETY: a CPU descriptor handle of a heap this layer made is 32 readable bytes.
        let words = unsafe { descriptor::read_slot(handle) };
        let image = gfx10::decode_image(&words)?;
        let (resource, info) = self.resources.view_for(&image)?;
        let gpu = resource.image.as_ref()?;
        let info = ViewInfo { mips: 1, ..info };
        let aspect = if depth {
            gpu.aspect
        } else {
            vk::ImageAspectFlags::COLOR
        };
        let view = self
            .asm
            .views
            .get(gpu.image, &info, aspect, identity())
            .ok()?;
        Some(TargetView {
            view,
            format: info.format,
            extent: [
                (gpu.extent[0] >> info.base_mip).max(1),
                (gpu.extent[1] >> info.base_mip).max(1),
            ],
            samples: vk::SampleCountFlags::from_raw(gpu.samples),
        })
    }

    /// Registers the descriptors in `count` slots at `start`, which lie in a heap of this device.
    fn scan(&self, start: u64, count: u64, samplers: bool) {
        if count == 0 {
            return;
        }
        // SAFETY: the slots lie inside a descriptor heap whose bytes are committed host memory (checked by the
        // caller against the heap's extent).
        let words =
            unsafe { std::slice::from_raw_parts(start as usize as *const u32, count as usize * 8) };
        let mut descriptors = self.asm.descriptors();
        if samplers {
            for (_, registration) in descriptors.scan_samplers(words, 8) {
                if registration.new {
                    self.asm.bindless.note_sampler(registration.index);
                }
            }
        } else {
            for (_, registration) in descriptors.scan_textures(words, 8) {
                if registration.new {
                    self.asm.bindless.note_texture(registration.index);
                }
            }
        }
    }
}

/// The objects of a device, which need nothing locked.
struct Live;

impl Objects for Live {
    fn pipeline(&self, object: &ComPtr) -> Option<Arc<dyn PendingPipeline>> {
        // SAFETY: a COM object the title named in a list.
        unsafe { pipeline_of(object.as_ptr()) }
    }

    fn command_signature(&self, object: &ComPtr) -> Option<IndirectLayout> {
        // SAFETY: as `pipeline`.
        unsafe { signature_of(object.as_ptr()) }
    }

    fn bundle(&self, object: &ComPtr) -> Option<Arc<CommandStream>> {
        // SAFETY: as `pipeline`.
        unsafe { list::closed_stream(object.as_ptr()) }
    }
}

impl Objects for Context<'_> {
    fn pipeline(&self, object: &ComPtr) -> Option<Arc<dyn PendingPipeline>> {
        Live.pipeline(object)
    }

    fn command_signature(&self, object: &ComPtr) -> Option<IndirectLayout> {
        Live.command_signature(object)
    }

    fn bundle(&self, object: &ComPtr) -> Option<Arc<CommandStream>> {
        Live.bundle(object)
    }
}

impl Resolver for Context<'_> {
    fn buffer(&self, resource: &ComPtr) -> Option<BufferRef> {
        // SAFETY: a COM object the title named in a list.
        let id = unsafe { resource_of(resource.as_ptr()) }?.id;
        let r = self.resources.get(id)?;
        r.desc.is_buffer().then_some(BufferRef {
            address: r.address,
            size: r.desc.width,
        })
    }

    fn image(&self, resource: &ComPtr) -> Option<ImageRef> {
        // SAFETY: as `buffer`.
        let id = unsafe { resource_of(resource.as_ptr()) }?.id;
        image_ref(self.resources.get(id)?)
    }

    fn buffer_containing(&self, address: u64) -> Option<BufferRef> {
        let (id, _) = self.resources.find(address)?;
        let r = self.resources.get(id)?;
        r.desc.is_buffer().then_some(BufferRef {
            address: r.address,
            size: r.desc.width,
        })
    }

    fn root_signature(&self, object: &ComPtr) -> Option<Arc<RootSignature>> {
        // SAFETY: as `buffer`.
        unsafe { root_of(object.as_ptr()) }
    }

    fn heap_bases(&self, heaps: &[ComPtr]) -> HeapBases {
        let mut bases = HeapBases::default();
        for heap in heaps {
            let Some((desc, gpu)) = heap_words(heap.as_ptr()) else {
                continue;
            };
            match desc[0] {
                heap::TYPE_CBV_SRV_UAV => bases.resource = gpu as u32,
                heap::TYPE_SAMPLER => bases.sampler = gpu as u32,
                _ => continue,
            }
            if desc[2] & heap::FLAG_SHADER_VISIBLE != 0 && gpu != 0 {
                self.scan_heap(desc[0] == heap::TYPE_SAMPLER, gpu, desc[1]);
            }
        }
        bases
    }

    fn color_target(&self, handle: u64) -> Option<TargetView> {
        self.target(handle, false)
    }

    fn depth_target(&self, handle: u64) -> Option<TargetView> {
        self.target(handle, true)
    }

    fn table_bound(&self, root: &RootSignature, param: u32, handle: u64) {
        let Some(core) = self.asm.core.get() else {
            return;
        };
        let Some(ParameterKind::Table(ranges)) =
            root.parameters.get(param as usize).map(|p| &p.kind)
        else {
            return;
        };
        let Some(info) = core.heaps.find(handle) else {
            return;
        };
        let end = info.gpu_start + info.bytes();
        let step = u64::from(info.increment);
        let mut next = handle;
        for range in ranges {
            let start = if range.offset == 0xFFFF_FFFF {
                next
            } else {
                handle + u64::from(range.offset) * step
            };
            let room = end.saturating_sub(start) / step;
            let count = if range.count == u32::MAX {
                room.min(UNBOUNDED_TABLE)
            } else {
                u64::from(range.count).min(room)
            };
            next = start + count * step;
            match range.kind {
                RangeType::Srv | RangeType::Uav => self.scan(start, count, false),
                RangeType::Sampler => self.scan(start, count, true),
                RangeType::Cbv => {}
            }
        }
    }

    fn filters_draws(&self) -> bool {
        self.filter.is_some()
    }

    fn skip_draw(&self, buffers: &[u64]) -> bool {
        self.filter
            .as_ref()
            .is_some_and(|f| f.on_draw(buffers) == DrawDecision::Skip)
    }
}

impl Context<'_> {
    /// Looks through a whole shader-visible heap for descriptors the title wrote itself, for root signatures that
    /// index the heap directly. A large heap is only looked through again after `HEAP_SCAN_INTERVAL`.
    fn scan_heap(&self, samplers: bool, gpu: u64, count: u32) {
        let now = Instant::now();
        {
            let mut scans = self
                .asm
                .scans
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(last) = scans.get(&gpu)
                && count > SMALL_HEAP
                && now.duration_since(*last) < HEAP_SCAN_INTERVAL
            {
                return;
            }
            scans.insert(gpu, now);
        }
        self.scan(gpu, u64::from(count), samplers);
    }
}

/// Merges address ranges that touch or overlap.
fn merge_ranges(mut ranges: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    ranges.retain(|r| r.1 > 0);
    ranges.sort_unstable();
    let mut out: Vec<(u64, u64)> = Vec::new();
    for (start, len) in ranges {
        match out.last_mut() {
            Some(last) if start <= last.0 + last.1 => last.1 = last.1.max(start + len - last.0),
            _ => out.push((start, len)),
        }
    }
    out
}

impl Assembly {
    /// Records `record` into a command buffer, submits it on the main queue and waits for it.
    pub(super) fn run_commands<R>(
        &self,
        pool: vk::CommandPool,
        fence: vk::Fence,
        record: impl FnOnce(vk::CommandBuffer) -> R,
    ) -> Result<R, vk::Result> {
        let d = &self.gpu.ctx.device;
        // SAFETY: the command buffer comes from a pool of this device, is recorded once, submitted once and freed
        // after the fence says the GPU is done.
        unsafe {
            let cb = d.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .command_buffer_count(1),
            )?[0];
            let result = (|| {
                d.begin_command_buffer(
                    cb,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )?;
                let value = record(cb);
                d.end_command_buffer(cb)?;
                let buffers = [cb];
                let submit = [vk::SubmitInfo::default().command_buffers(&buffers)];
                {
                    let queue = self
                        .gpu
                        .vulkan
                        .queue
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);
                    d.queue_submit(*queue, &submit, fence)?;
                }
                d.wait_for_fences(&[fence], true, u64::MAX)?;
                d.reset_fences(&[fence])?;
                Ok(value)
            })();
            d.free_command_buffers(pool, &[cb]);
            result
        }
    }

    /// Texels the title wrote into placed texture memory reach the image before the GPU reads it: at each
    /// transition out of a copy destination or an undefined state.
    fn refresh_placed_textures(&self, resources: &Resources, lists: &[Arc<CommandStream>]) {
        for list in lists {
            for command in &list.commands {
                let stream::Command::ResourceBarrier(barriers) = command else {
                    continue;
                };
                for barrier in barriers {
                    let stream::Barrier::Transition {
                        resource,
                        before,
                        after,
                        ..
                    } = barrier
                    else {
                        continue;
                    };
                    let reads = after & (PIXEL_SHADER_RESOURCE | NON_PIXEL_SHADER_RESOURCE) != 0;
                    let written = before & COPY_DEST != 0 || *before == 0xFFFF_FFFF;
                    if !(reads && written) {
                        continue;
                    }
                    // SAFETY: a COM object the title named.
                    let Some(object) = (unsafe { resource_of(resource.as_ptr()) }) else {
                        continue;
                    };
                    if resources
                        .get(object.id)
                        .is_some_and(|r| r.origin == Origin::Placed && r.image.is_some())
                        && let Err(e) = resources.upload_texture(object.id)
                    {
                        com::note_once(format!("placed texture texels were not read: {e}"));
                    }
                }
            }
        }
    }

    /// Tells the content filter of the bytes copied into resources.
    fn filter_uploads(
        &self,
        filter: &hooks::FilterHandle,
        resources: &Resources,
        lists: &[Arc<CommandStream>],
    ) {
        const LIMIT: u64 = 256 << 20;
        for list in lists {
            for command in &list.commands {
                match command {
                    stream::Command::CopyBufferRegion {
                        dst,
                        dst_offset,
                        src,
                        src_offset,
                        bytes,
                    } => {
                        // SAFETY: COM objects the title named.
                        let (d, s) =
                            unsafe { (resource_of(dst.as_ptr()), resource_of(src.as_ptr())) };
                        let (Some(d), Some(s)) = (d, s) else { continue };
                        let Some(from) = resources.get(s.id).and_then(|r| r.cpu_address()) else {
                            continue;
                        };
                        if *bytes > LIMIT {
                            continue;
                        }
                        // SAFETY: the source is host memory at its address, `bytes` long inside the resource.
                        let data = unsafe {
                            std::slice::from_raw_parts(
                                (from + src_offset) as usize as *const u8,
                                *bytes as usize,
                            )
                        };
                        filter.on_upload(FilterId(u64::from(d.id)), *dst_offset, data);
                    }
                    stream::Command::CopyTextureRegion(copy) => {
                        let (
                            stream::CopyLocation::Subresource { resource: dst, .. },
                            stream::CopyLocation::Placed {
                                resource: src,
                                footprint,
                            },
                        ) = (&copy.dst, &copy.src)
                        else {
                            continue;
                        };
                        // SAFETY: COM objects the title named.
                        let (d, s) =
                            unsafe { (resource_of(dst.as_ptr()), resource_of(src.as_ptr())) };
                        let (Some(d), Some(s)) = (d, s) else { continue };
                        let Some(from) = resources.get(s.id).and_then(|r| r.cpu_address()) else {
                            continue;
                        };
                        let len = u64::from(footprint.row_pitch)
                            * u64::from(footprint.height)
                            * u64::from(footprint.depth.max(1));
                        if len > LIMIT || len == 0 {
                            continue;
                        }
                        // SAFETY: the footprint lies inside the upload buffer's host memory.
                        let data = unsafe {
                            std::slice::from_raw_parts(
                                (from + footprint.offset) as usize as *const u8,
                                len as usize,
                            )
                        };
                        filter.on_upload(FilterId(u64::from(d.id)), 0, data);
                    }
                    _ => {}
                }
            }
        }
    }

    /// Under the wait policy, waits for the build of every pipeline the lists draw or dispatch with. It runs before
    /// anything is locked: the builds put shader code in memory the resource store hands out.
    fn wait_for_pipelines(&self, lists: &[Arc<CommandStream>]) {
        if self.pipeline_service.draw_policy() != DrawPolicy::Wait {
            return;
        }
        let mut seen: Vec<Arc<dyn PendingPipeline>> = Vec::new();
        for list in lists {
            for pipeline in bridge::used_pipelines(list, &Live) {
                if !seen
                    .iter()
                    .any(|s| std::ptr::addr_eq(Arc::as_ptr(s), Arc::as_ptr(&pipeline)))
                {
                    let _ = pipeline.resolve();
                    seen.push(pipeline);
                }
            }
        }
    }

    pub fn execute(&self, lists: &[Arc<CommandStream>]) -> Result<(), vk::Result> {
        self.wait_for_pipelines(lists);
        let mut exec = self.exec.lock().unwrap_or_else(PoisonError::into_inner);
        exec.spill.reset();
        let filter = hooks::content_filter();
        let resources = self.resources();
        self.refresh_placed_textures(&resources, lists);
        if let Some(filter) = &filter {
            self.filter_uploads(filter, &resources, lists);
        }
        let context = Context {
            asm: self,
            resources: &resources,
            filter,
        };
        let converted: Vec<Converted> = lists
            .iter()
            .map(|list| bridge::convert(list, &context))
            .collect();
        drop(context);
        {
            let mut descriptors = self.descriptors();
            self.bindless
                .update(&mut descriptors, &resources, &self.views)?;
        }
        let writes = merge_ranges(
            converted
                .iter()
                .flat_map(|c| c.writes.iter().copied())
                .collect(),
        );
        let (pool, fence) = (exec.pool, exec.fence);
        let spill: &mut dyn SpillAllocator = &mut exec.spill;
        let mut space = resources.space();
        let backing = space.backing();
        let layout = self.pipelines.layout;
        let set = self.bindless.set;
        let arena = Arena {
            buffer: space.buffer(),
            device_address: space.heap_base(),
        };
        let mut report = Vec::new();
        let result = self.run_commands(pool, fence, |cb| {
            space.record_uploads(cb);
            for list in &converted {
                let env = ReplayEnv {
                    arena,
                    layout,
                    bindless: Some(set),
                    spill: Some(&mut *spill),
                };
                match Replayer::new(&self.gpu.ctx, env, cb).record(&list.commands) {
                    Ok(r) => report.extend(r.notes),
                    Err(e) => com::note_once(format!("a list could not be replayed: {e}")),
                }
            }
            if backing == Backing::Mirrored {
                for (start, len) in &writes {
                    space.record_downloads(cb, *start, *len);
                }
            }
        });
        drop(space);
        drop(resources);
        for note in report {
            com::note_once(note);
        }
        result?;
        self.resources().space().reclaim();
        self.gpu.handles.budget.tick();
        drop(converted);
        Ok(())
    }
}
