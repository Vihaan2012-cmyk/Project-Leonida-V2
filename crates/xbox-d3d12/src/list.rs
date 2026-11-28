//! Command allocators and command lists.
//!
//! A list records into a `CommandStream` (`stream.rs`); `Close` freezes it and the queue replays it. The
//! console list object is wider than its vtable: titles write packets into an inline command buffer whose
//! write cursor and limits sit at `+0x18`, `+0x28` and `+0x30` and fall back to the vtable only when the
//! buffer is full. The state of this object starts with zeroed words there, so the buffer is always full and
//! every command arrives through the vtable.
//!
//! Slots follow `d3d12_slots.txt`: PC order to slot 26 (`CopyTextureRegion` is not at 16), four slots later
//! for the root-argument group, two later after slot 55. A slot with no entry traps.

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use smallvec::{SmallVec, smallvec};

use crate::com::{
    self, ComPtr, E_FAIL, E_INVALIDARG, E_NOINTERFACE, E_POINTER, Guid, Kind, S_OK, add_ref, iid,
    release, state,
};
use crate::device::DeviceCore;
use crate::stream::{
    Barrier, BarrierGroup, Box3, BufferBarrier, Command, CommandStream, CopyLocation, Footprint,
    GlobalBarrier, IndexBufferView, Rect, Rects, RootView, TextureBarrier, TextureCopy,
    VertexBufferView, Viewport,
};

pub const LIST_TYPE_DIRECT: u32 = 0;
pub const LIST_TYPE_BUNDLE: u32 = 1;
/// The console DMA list type is derived, not measured.
pub const LIST_TYPE_DMA: u32 = 6;

// integration

/// Upload memory that the GPU can address by the same number the CPU writes it at, for console slot 148.
/// The memory step's arena provides it; without one the list hands out host memory.
pub trait UploadMemory: Send + Sync {
    fn allocate(&self, size: usize, alignment: usize) -> Option<usize>;
}

/// No GPU-addressable upload memory.
#[derive(Default)]
pub struct NoUploadMemory;

impl UploadMemory for NoUploadMemory {
    fn allocate(&self, _size: usize, _alignment: usize) -> Option<usize> {
        None
    }
}

// Allocators

pub struct Allocator {
    device: ComPtr,
    pub list_type: u32,
    /// The list recording into this allocator, 0 when none.
    recording: AtomicUsize,
}

/// `CreateCommandAllocator(type, riid, out)`.
///
/// # Safety
/// `device` is the calling device; `riid` and `out` as COM passes them.
pub unsafe fn create_allocator(
    device: *mut c_void,
    core: &Arc<DeviceCore>,
    list_type: u32,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    if out.is_null() {
        return E_POINTER;
    }
    if list_type > 5 {
        return E_INVALIDARG;
    }
    // SAFETY: `device` is the device that called.
    let Some(device) = (unsafe { ComPtr::add_ref(device) }) else {
        return E_POINTER;
    };
    let object = com::new_object(
        Kind::CommandAllocator,
        allocator_vtable(core),
        Allocator {
            device,
            list_type,
            recording: AtomicUsize::new(0),
        },
    );
    // SAFETY: the creation reference is handed out.
    unsafe { com::hand_out(object as *mut c_void, riid, out) }
}

unsafe fn allocator_of<'a>(this: *mut c_void) -> Option<&'a Allocator> {
    if this.is_null() {
        return None;
    }
    // SAFETY: as the caller promises.
    unsafe { (com::kind_of(this)? == Kind::CommandAllocator).then(|| state::<Allocator>(this)) }
}

unsafe extern "win64" fn a_query_interface(
    this: *mut c_void,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    // SAFETY: slot 0, COM arguments.
    unsafe { com::query_interface_for(Kind::CommandAllocator, this, riid, out) }
}

unsafe extern "win64" fn a_get_device(
    this: *mut c_void,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    // SAFETY: `this` is an allocator; it holds its device alive.
    unsafe { com::query(state::<Allocator>(this).device.as_ptr(), riid, out) }
}

unsafe extern "win64" fn a_reset(this: *mut c_void) -> u32 {
    // SAFETY: an allocator of this layer.
    let allocator = unsafe { state::<Allocator>(this) };
    if allocator.recording.load(Ordering::Acquire) != 0 {
        return E_FAIL;
    }
    S_OK
}

fn allocator_vtable(core: &DeviceCore) -> *const usize {
    core.cached_vtable(
        Kind::CommandAllocator,
        32,
        &[
            (0, a_query_interface as *const () as usize),
            (1, add_ref as *const () as usize),
            (2, release as *const () as usize),
            (7, a_get_device as *const () as usize),
            (8, a_reset as *const () as usize),
        ],
    )
}

// Lists

#[derive(Default)]
struct ListState {
    open: bool,
    /// How many commands the list held when it was last closed: the next recording starts with room for them.
    expected: usize,
    commands: Vec<Command>,
    uploads: Vec<Box<[u8]>>,
    error: Option<u32>,
    closed: Option<Arc<CommandStream>>,
}

/// The inline command buffer words a console title reads and writes in the list object: zero.
const INLINE_WORDS: usize = 0x100;

#[repr(C)]
pub struct CommandList {
    inline: [u64; INLINE_WORDS],
    device: ComPtr,
    core: Arc<DeviceCore>,
    list_type: u32,
    allocator: Mutex<Option<ComPtr>>,
    extra_iids: Mutex<Vec<Guid>>,
    state: Mutex<ListState>,
}

impl Drop for CommandList {
    fn drop(&mut self) {
        self.release_allocator();
    }
}

impl CommandList {
    fn release_allocator(&self) {
        let Ok(mut slot) = self.allocator.lock() else {
            return;
        };
        if let Some(allocator) = slot.take() {
            // SAFETY: the pointer is an allocator of this layer, checked when it was taken.
            unsafe { state::<Allocator>(allocator.as_ptr()) }
                .recording
                .store(0, Ordering::Release);
        }
    }

    fn take_allocator(&self, this: usize, allocator: *mut c_void) -> u32 {
        // SAFETY: `allocator` is checked to be one of this layer's.
        let Some(a) = (unsafe { allocator_of(allocator) }) else {
            return E_INVALIDARG;
        };
        if a.list_type != self.list_type && self.list_type < 4 {
            return E_INVALIDARG;
        }
        if a.recording
            .compare_exchange(0, this, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return E_INVALIDARG;
        }
        // SAFETY: a counted reference to the allocator just checked.
        let reference = unsafe { ComPtr::add_ref(allocator) };
        if let Ok(mut slot) = self.allocator.lock() {
            *slot = reference;
        }
        S_OK
    }
}

/// The closed stream of a list, or `None` when it is open or not a list of this layer. Bundles named by
/// `ExecuteBundle` resolve through this too.
///
/// # Safety
/// `list` is null or a live COM object.
pub unsafe fn closed_stream(list: *mut c_void) -> Option<Arc<CommandStream>> {
    if list.is_null() {
        return None;
    }
    // SAFETY: as the caller promises.
    unsafe {
        if com::kind_of(list)? != Kind::CommandList {
            return None;
        }
        state::<CommandList>(list).state.lock().ok()?.closed.clone()
    }
}

/// Creates a list. `console` is slot 73 (`CreateCommandListX`), which accepts the console's own list IIDs
/// (not in the PC SDK, named by nobody yet: they are noted and answered) and the DMA type.
///
/// # Safety
/// `device` is the calling device; `allocator`, `pso`, `riid` and `out` as COM passes them.
#[allow(clippy::too_many_arguments)]
pub unsafe fn create_list(
    device: *mut c_void,
    core: &Arc<DeviceCore>,
    list_type: u32,
    allocator: *mut c_void,
    pso: *mut c_void,
    riid: *const u8,
    out: *mut *mut c_void,
    console: bool,
) -> u32 {
    if out.is_null() || riid.is_null() {
        return E_POINTER;
    }
    let max_type = if console { LIST_TYPE_DMA } else { 5 };
    if list_type > max_type {
        return E_INVALIDARG;
    }
    // SAFETY: COM passes a 16-byte IID.
    let wanted = unsafe { Guid::read(riid) };
    let mut extra = Vec::new();
    if !com::base_iids(Kind::CommandList).contains(&wanted) {
        if !console && wanted != iid::CONS_DMA_COMMAND_LIST {
            return E_NOINTERFACE;
        }
        com::note_once(format!(
            "CreateCommandList{} asked for interface {wanted} (list type {list_type}); answered as a command list",
            if console { "X" } else { "" }
        ));
        extra.push(wanted);
    }
    // SAFETY: `device` is the device that called.
    let Some(device_ref) = (unsafe { ComPtr::add_ref(device) }) else {
        return E_POINTER;
    };
    let object = com::new_object(
        Kind::CommandList,
        list_vtable(core),
        CommandList {
            inline: [0; INLINE_WORDS],
            device: device_ref,
            core: core.clone(),
            list_type,
            allocator: Mutex::new(None),
            extra_iids: Mutex::new(extra),
            state: Mutex::new(ListState {
                open: true,
                ..Default::default()
            }),
        },
    );
    // SAFETY: just allocated, alive for the call.
    let list = unsafe { &(*object).value };
    let result = list.take_allocator(object as usize, allocator);
    if result != S_OK {
        // SAFETY: the creation reference is given up.
        unsafe { release(object as *mut c_void) };
        return result;
    }
    if !pso.is_null() {
        // SAFETY: `pso` is the title's pipeline object.
        let pipeline = unsafe { ComPtr::add_ref(pso) };
        push(object as *mut c_void, Command::SetPipelineState(pipeline));
    }
    // SAFETY: the creation reference is handed out.
    unsafe { com::hand_out(object as *mut c_void, riid, out) }
}

fn push(this: *mut c_void, command: Command) {
    // SAFETY: only called from vtable methods of a list of this layer.
    let list = unsafe { state::<CommandList>(this) };
    let Ok(mut s) = list.state.lock() else {
        return;
    };
    if s.open {
        s.commands.push(command);
    } else {
        com::note_once("a call was recorded into a closed command list and dropped");
    }
}

fn unrecorded(this: *mut c_void, slot: usize, method: &'static str, args: [usize; 4]) {
    com::note_once(format!(
        "ID3D12GraphicsCommandList slot {slot} ({method}) recorded but not decoded"
    ));
    push(this, Command::Unrecorded { slot, method, args });
}

unsafe fn slice<'a, T>(p: *const T, n: u32) -> &'a [T] {
    if p.is_null() || n == 0 {
        &[]
    } else {
        // SAFETY: as the caller promises: `n` readable elements.
        unsafe { std::slice::from_raw_parts(p, n as usize) }
    }
}

unsafe fn com_ref(p: *mut c_void) -> Option<ComPtr> {
    // SAFETY: as the caller promises.
    unsafe { ComPtr::add_ref(p) }
}

unsafe fn rect_vec(p: *const Rect, n: u32) -> Rects {
    // SAFETY: as the caller promises.
    SmallVec::from_slice(unsafe { slice(p, n) })
}

unsafe extern "win64" fn l_query_interface(
    this: *mut c_void,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    // SAFETY: a list of this layer, COM arguments.
    unsafe {
        let list = state::<CommandList>(this);
        let extra = list
            .extra_iids
            .lock()
            .map(|e| e.clone())
            .unwrap_or_default();
        com::query_interface_with(this, riid, out, |g| {
            com::base_iids(Kind::CommandList).contains(g) || extra.contains(g)
        })
    }
}

unsafe extern "win64" fn l_get_device(
    this: *mut c_void,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    // SAFETY: a list; it holds its device alive.
    unsafe { com::query(state::<CommandList>(this).device.as_ptr(), riid, out) }
}

unsafe extern "win64" fn l_get_type(this: *mut c_void) -> u32 {
    // SAFETY: a list of this layer.
    unsafe { state::<CommandList>(this).list_type }
}

unsafe extern "win64" fn l_close(this: *mut c_void) -> u32 {
    // SAFETY: a list of this layer.
    let list = unsafe { state::<CommandList>(this) };
    let Ok(mut s) = list.state.lock() else {
        return E_FAIL;
    };
    if !s.open {
        return E_FAIL;
    }
    s.open = false;
    s.expected = s.commands.len();
    let stream = CommandStream {
        list_type: list.list_type,
        commands: std::mem::take(&mut s.commands),
        uploads: std::mem::take(&mut s.uploads),
    };
    s.closed = Some(Arc::new(stream));
    let error = s.error.take();
    drop(s);
    list.release_allocator();
    error.unwrap_or(S_OK)
}

unsafe extern "win64" fn l_reset(
    this: *mut c_void,
    allocator: *mut c_void,
    pso: *mut c_void,
) -> u32 {
    // SAFETY: a list of this layer.
    let list = unsafe { state::<CommandList>(this) };
    {
        let Ok(mut s) = list.state.lock() else {
            return E_FAIL;
        };
        if s.open {
            return E_FAIL;
        }
        let result = list.take_allocator(this as usize, allocator);
        if result != S_OK {
            return result;
        }
        *s = ListState {
            open: true,
            commands: Vec::with_capacity(s.expected),
            expected: s.expected,
            ..Default::default()
        };
    }
    if !pso.is_null() {
        // SAFETY: the title's pipeline object.
        push(this, Command::SetPipelineState(unsafe { com_ref(pso) }));
    }
    S_OK
}

unsafe extern "win64" fn l_clear_state(this: *mut c_void, pso: *mut c_void) {
    // SAFETY: the title's pipeline object or null.
    push(
        this,
        Command::ClearState {
            pipeline: unsafe { com_ref(pso) },
        },
    );
}

unsafe extern "win64" fn l_draw_instanced(this: *mut c_void, a: u32, b: u32, c: u32, d: u32) {
    push(
        this,
        Command::DrawInstanced {
            vertex_count: a,
            instance_count: b,
            start_vertex: c,
            start_instance: d,
        },
    );
}

unsafe extern "win64" fn l_draw_indexed_instanced(
    this: *mut c_void,
    a: u32,
    b: u32,
    c: u32,
    d: i32,
    e: u32,
) {
    push(
        this,
        Command::DrawIndexedInstanced {
            index_count: a,
            instance_count: b,
            start_index: c,
            base_vertex: d,
            start_instance: e,
        },
    );
}

unsafe extern "win64" fn l_dispatch(this: *mut c_void, x: u32, y: u32, z: u32) {
    push(
        this,
        Command::Dispatch {
            x,
            y,
            z,
            extra: None,
        },
    );
}

unsafe extern "win64" fn l_dispatch_x(this: *mut c_void, x: u32, y: u32, z: u32, extra: u32) {
    push(
        this,
        Command::Dispatch {
            x,
            y,
            z,
            extra: Some(extra),
        },
    );
}

unsafe extern "win64" fn l_copy_buffer_region(
    this: *mut c_void,
    dst: *mut c_void,
    dst_offset: u64,
    src: *mut c_void,
    src_offset: u64,
    bytes: u64,
) {
    // SAFETY: the title's resources.
    let (dst, src) = unsafe { (com_ref(dst), com_ref(src)) };
    if let (Some(dst), Some(src)) = (dst, src) {
        push(
            this,
            Command::CopyBufferRegion {
                dst,
                dst_offset,
                src,
                src_offset,
                bytes,
            },
        );
    }
}

unsafe fn read_location(p: *const u8) -> Option<CopyLocation> {
    if p.is_null() {
        return None;
    }
    // SAFETY: a `D3D12_TEXTURE_COPY_LOCATION`, 48 bytes on x64.
    unsafe {
        let resource = com_ref(*(p as *const *mut c_void))?;
        let kind = *(p.add(8) as *const u32);
        if kind == 1 {
            let q = p.add(16);
            Some(CopyLocation::Placed {
                resource,
                footprint: Footprint {
                    offset: *(q as *const u64),
                    format: *(q.add(8) as *const u32),
                    width: *(q.add(12) as *const u32),
                    height: *(q.add(16) as *const u32),
                    depth: *(q.add(20) as *const u32),
                    row_pitch: *(q.add(24) as *const u32),
                },
            })
        } else {
            Some(CopyLocation::Subresource {
                resource,
                index: *(p.add(16) as *const u32),
            })
        }
    }
}

unsafe extern "win64" fn l_copy_texture_region(
    this: *mut c_void,
    dst: *const u8,
    x: u32,
    y: u32,
    z: u32,
    src: *const u8,
    src_box: *const Box3,
) {
    // SAFETY: the title's copy locations and optional box.
    unsafe {
        if let (Some(dst), Some(src)) = (read_location(dst), read_location(src)) {
            push(
                this,
                Command::CopyTextureRegion(Box::new(TextureCopy {
                    dst,
                    x,
                    y,
                    z,
                    src,
                    src_box: src_box.as_ref().copied(),
                })),
            );
        }
    }
}

unsafe extern "win64" fn l_copy_resource(this: *mut c_void, dst: *mut c_void, src: *mut c_void) {
    // SAFETY: the title's resources.
    let (dst, src) = unsafe { (com_ref(dst), com_ref(src)) };
    if let (Some(dst), Some(src)) = (dst, src) {
        push(this, Command::CopyResource { dst, src });
    }
}

unsafe extern "win64" fn l_copy_tiles(this: *mut c_void, a: usize, b: usize, c: usize, d: usize) {
    unrecorded(this, 18, "CopyTiles", [a, b, c, d]);
}

unsafe extern "win64" fn l_resolve_subresource(
    this: *mut c_void,
    dst: *mut c_void,
    dst_subresource: u32,
    src: *mut c_void,
    src_subresource: u32,
    format: u32,
) {
    // SAFETY: the title's resources.
    let (dst, src) = unsafe { (com_ref(dst), com_ref(src)) };
    if let (Some(dst), Some(src)) = (dst, src) {
        push(
            this,
            Command::ResolveSubresource {
                dst,
                dst_subresource,
                src,
                src_subresource,
                format,
            },
        );
    }
}

unsafe extern "win64" fn l_set_topology(this: *mut c_void, topology: u32) {
    push(this, Command::SetPrimitiveTopology(topology));
}

unsafe extern "win64" fn l_set_viewports(this: *mut c_void, n: u32, p: *const Viewport) {
    // SAFETY: `n` viewports.
    push(
        this,
        Command::SetViewports(SmallVec::from_slice(unsafe { slice(p, n) })),
    );
}

unsafe extern "win64" fn l_set_scissors(this: *mut c_void, n: u32, p: *const Rect) {
    // SAFETY: `n` rectangles.
    push(this, Command::SetScissorRects(unsafe { rect_vec(p, n) }));
}

unsafe extern "win64" fn l_set_blend_factor(this: *mut c_void, p: *const [f32; 4]) {
    // SAFETY: four floats or null.
    push(
        this,
        Command::SetBlendFactor(unsafe { p.as_ref() }.copied()),
    );
}

unsafe extern "win64" fn l_set_stencil_ref(this: *mut c_void, value: u32) {
    push(this, Command::SetStencilRef(value));
}

unsafe extern "win64" fn l_set_pipeline_state(this: *mut c_void, pso: *mut c_void) {
    // SAFETY: the title's pipeline object.
    push(this, Command::SetPipelineState(unsafe { com_ref(pso) }));
}

unsafe extern "win64" fn l_resource_barrier(this: *mut c_void, n: u32, p: *const u8) {
    if p.is_null() {
        return;
    }
    let mut barriers = Vec::with_capacity(n as usize);
    for i in 0..n as usize {
        // SAFETY: `n` `D3D12_RESOURCE_BARRIER`s of 32 bytes each.
        unsafe {
            let b = p.add(i * 32);
            let (kind, flags) = (*(b as *const u32), *(b.add(4) as *const u32));
            let pointer = |offset: usize| com_ref(*(b.add(offset) as *const *mut c_void));
            match kind {
                0 => {
                    if let Some(resource) = pointer(8) {
                        barriers.push(Barrier::Transition {
                            resource,
                            subresource: *(b.add(16) as *const u32),
                            before: *(b.add(20) as *const u32),
                            after: *(b.add(24) as *const u32),
                            flags,
                        });
                    }
                }
                1 => barriers.push(Barrier::Aliasing {
                    before: pointer(8),
                    after: pointer(16),
                }),
                2 => barriers.push(Barrier::Uav {
                    resource: pointer(8),
                }),
                other => com::note_once(format!("ResourceBarrier of unknown type {other}")),
            }
        }
    }
    push(this, Command::ResourceBarrier(barriers));
}

unsafe extern "win64" fn l_set_descriptor_heaps(
    this: *mut c_void,
    n: u32,
    heaps: *const *mut c_void,
) {
    // SAFETY: `n` heap pointers.
    let heaps = unsafe { slice(heaps, n) }
        .iter()
        .filter_map(|&h| unsafe { com_ref(h) })
        .collect();
    push(this, Command::SetDescriptorHeaps(heaps));
}

unsafe fn set_root_signature(this: *mut c_void, compute: bool, signature: *mut c_void) {
    // SAFETY: the title's root signature or null.
    push(
        this,
        Command::SetRootSignature {
            compute,
            signature: unsafe { com_ref(signature) },
        },
    );
}

unsafe extern "win64" fn l_set_compute_root_signature(this: *mut c_void, s: *mut c_void) {
    // SAFETY: as above.
    unsafe { set_root_signature(this, true, s) };
}

unsafe extern "win64" fn l_set_graphics_root_signature(this: *mut c_void, s: *mut c_void) {
    // SAFETY: as above.
    unsafe { set_root_signature(this, false, s) };
}

macro_rules! root_methods {
    ($table:ident, $constant:ident, $constants:ident, $view:ident, $compute:expr) => {
        unsafe extern "win64" fn $table(this: *mut c_void, index: u32, base: u64) {
            push(
                this,
                Command::SetRootDescriptorTable {
                    compute: $compute,
                    index,
                    base,
                },
            );
        }

        unsafe extern "win64" fn $constant(this: *mut c_void, index: u32, value: u32, offset: u32) {
            push(
                this,
                Command::SetRootConstants {
                    compute: $compute,
                    index,
                    dest_offset: offset,
                    values: smallvec![value],
                },
            );
        }

        unsafe extern "win64" fn $constants(
            this: *mut c_void,
            index: u32,
            count: u32,
            data: *const u32,
            offset: u32,
        ) {
            // SAFETY: `count` dwords.
            let values = SmallVec::from_slice(unsafe { slice(data, count) });
            push(
                this,
                Command::SetRootConstants {
                    compute: $compute,
                    index,
                    dest_offset: offset,
                    values,
                },
            );
        }

        fn $view(this: *mut c_void, view: RootView, index: u32, address: u64) {
            push(
                this,
                Command::SetRootView {
                    compute: $compute,
                    view,
                    index,
                    address,
                },
            );
        }
    };
}

root_methods!(
    l_compute_table,
    l_compute_constant,
    l_compute_constants,
    compute_view,
    true
);
root_methods!(
    l_graphics_table,
    l_graphics_constant,
    l_graphics_constants,
    graphics_view,
    false
);

macro_rules! view_slot {
    ($name:ident, $inner:ident, $view:expr) => {
        unsafe extern "win64" fn $name(this: *mut c_void, index: u32, address: u64) {
            $inner(this, $view, index, address);
        }
    };
}

view_slot!(l_compute_cbv, compute_view, RootView::ConstantBuffer);
view_slot!(l_compute_srv, compute_view, RootView::ShaderResource);
view_slot!(l_compute_uav, compute_view, RootView::UnorderedAccess);
view_slot!(l_graphics_cbv, graphics_view, RootView::ConstantBuffer);
view_slot!(l_graphics_srv, graphics_view, RootView::ShaderResource);
view_slot!(l_graphics_uav, graphics_view, RootView::UnorderedAccess);

unsafe extern "win64" fn l_set_index_buffer(this: *mut c_void, view: *const IndexBufferView) {
    // SAFETY: a view or null.
    push(
        this,
        Command::SetIndexBuffer(unsafe { view.as_ref() }.copied()),
    );
}

unsafe extern "win64" fn l_set_vertex_buffers(
    this: *mut c_void,
    start: u32,
    n: u32,
    views: *const VertexBufferView,
) {
    // SAFETY: `n` views.
    let views = SmallVec::from_slice(unsafe { slice(views, n) });
    push(this, Command::SetVertexBuffers { start, views });
}

unsafe extern "win64" fn l_set_stream_output(
    this: *mut c_void,
    start: u32,
    n: u32,
    views: *const [u64; 3],
) {
    // SAFETY: `n` 24-byte views.
    let views = unsafe { slice(views, n) }.to_vec();
    push(this, Command::SetStreamOutputTargets { start, views });
}

unsafe extern "win64" fn l_set_render_targets(
    this: *mut c_void,
    n: u32,
    rtvs: *const u64,
    single_range: u32,
    dsv: *const u64,
) {
    let single = single_range != 0;
    let take = if single { n.min(1) } else { n };
    // SAFETY: `take` handles, then an optional depth handle.
    let (rtvs, dsv) = unsafe {
        (
            SmallVec::from_slice(slice(rtvs, take)),
            dsv.as_ref().copied(),
        )
    };
    push(
        this,
        Command::SetRenderTargets {
            rtvs,
            single_range: single,
            count: n,
            dsv,
        },
    );
}

unsafe extern "win64" fn l_clear_dsv(
    this: *mut c_void,
    dsv: u64,
    flags: u32,
    depth: f32,
    stencil: u8,
    n: u32,
    rects: *const Rect,
) {
    // SAFETY: `n` rectangles.
    let rects = unsafe { rect_vec(rects, n) };
    push(
        this,
        Command::ClearDepthStencilView {
            dsv,
            flags,
            depth,
            stencil,
            rects,
        },
    );
}

unsafe extern "win64" fn l_clear_rtv(
    this: *mut c_void,
    rtv: u64,
    color: *const [f32; 4],
    n: u32,
    rects: *const Rect,
) {
    // SAFETY: four floats and `n` rectangles.
    let (color, rects) = unsafe {
        (
            color.as_ref().copied().unwrap_or_default(),
            rect_vec(rects, n),
        )
    };
    push(this, Command::ClearRenderTargetView { rtv, color, rects });
}

unsafe extern "win64" fn l_clear_uav_uint(
    this: *mut c_void,
    gpu: u64,
    cpu: u64,
    resource: *mut c_void,
    values: *const [u32; 4],
    n: u32,
    rects: *const Rect,
) {
    // SAFETY: the title's resource, four values and `n` rectangles.
    unsafe {
        if let Some(resource) = com_ref(resource) {
            push(
                this,
                Command::ClearUnorderedAccessViewUint {
                    gpu,
                    cpu,
                    resource,
                    values: values.as_ref().copied().unwrap_or_default(),
                    rects: rect_vec(rects, n),
                },
            );
        }
    }
}

unsafe extern "win64" fn l_clear_uav_float(
    this: *mut c_void,
    gpu: u64,
    cpu: u64,
    resource: *mut c_void,
    values: *const [f32; 4],
    n: u32,
    rects: *const Rect,
) {
    // SAFETY: the title's resource, four values and `n` rectangles.
    unsafe {
        if let Some(resource) = com_ref(resource) {
            push(
                this,
                Command::ClearUnorderedAccessViewFloat {
                    gpu,
                    cpu,
                    resource,
                    values: values.as_ref().copied().unwrap_or_default(),
                    rects: rect_vec(rects, n),
                },
            );
        }
    }
}

unsafe extern "win64" fn l_discard_resource(
    this: *mut c_void,
    resource: *mut c_void,
    region: *const u8,
) {
    // SAFETY: the title's resource and an optional `D3D12_DISCARD_REGION` (24 bytes).
    unsafe {
        let Some(resource) = com_ref(resource) else {
            return;
        };
        let (rects, first, count) = if region.is_null() {
            (Rects::new(), 0, 0)
        } else {
            let n = *(region as *const u32);
            let p = *(region.add(8) as *const *const Rect);
            (
                rect_vec(p, n),
                *(region.add(16) as *const u32),
                *(region.add(20) as *const u32),
            )
        };
        push(
            this,
            Command::DiscardResource {
                resource,
                rects,
                first_subresource: first,
                subresources: count,
            },
        );
    }
}

/// Console slot 59: one GPU event marker, `(begin, blob, size)`.
unsafe extern "win64" fn l_gpu_event(this: *mut c_void, begin: u8, data: *const u8, size: u32) {
    let size = size.min(1 << 16);
    // SAFETY: `size` readable bytes, as the measured callers pass.
    let data = SmallVec::from_slice(unsafe { slice(data, size) });
    push(
        this,
        Command::GpuEvent {
            begin: begin != 0,
            data,
        },
    );
}

unsafe extern "win64" fn l_execute_indirect(
    this: *mut c_void,
    signature: *mut c_void,
    max_count: u32,
    arguments: *mut c_void,
    arguments_offset: u64,
    count: *mut c_void,
    count_offset: u64,
) {
    // SAFETY: the title's objects.
    unsafe {
        if let (Some(signature), Some(arguments)) = (com_ref(signature), com_ref(arguments)) {
            push(
                this,
                Command::ExecuteIndirect {
                    signature,
                    max_count,
                    arguments,
                    arguments_offset,
                    count: com_ref(count),
                    count_offset,
                },
            );
        }
    }
}

unsafe extern "win64" fn l_set_scratch_buffer(
    this: *mut c_void,
    kind: u32,
    address: u64,
    size: u64,
    value: u64,
) {
    push(
        this,
        Command::SetScratchBuffer {
            kind,
            address,
            size,
            value,
        },
    );
}

/// Slot 71, `(address, value, 0, 0)`: the title's two sites (rvas 0x04ed4670 at the start of each frame and 0x0248ffd0)
/// write a frame number or marker into an `u32` table of 3 frames by 18 words that it allocates itself (rva
/// 0x05cabb50); nothing in the executable reads the table back, so it is a GPU progress breadcrumb that no wait
/// depends on. It records nothing.
unsafe extern "win64" fn l_write_value(
    _this: *mut c_void,
    _address: u64,
    _value: u64,
    _c: u64,
    _d: u64,
) -> u32 {
    static NOTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !NOTED.swap(true, Ordering::Relaxed) {
        com::note(
            "ID3D12GraphicsCommandList slot 71 (address, value): a progress breadcrumb nothing reads, ignored",
        );
    }
    S_OK
}

/// Slot 73: no call site on a command list was found in the executable (the `(1, 0)` and `(address, ..)` shapes at
/// this offset belong to other interfaces), so no wait on a written value exists. It records nothing.
unsafe extern "win64" fn l_wait_value(
    _this: *mut c_void,
    _a: u64,
    _b: u64,
    _c: u64,
    _d: u64,
) -> u32 {
    unmeasured_call(73, "no call site measured");
    S_OK
}

/// Console slot 146: the enhanced `Barrier(num_groups, groups)`.
unsafe extern "win64" fn l_barrier(this: *mut c_void, n: u32, groups: *const u8) {
    if groups.is_null() {
        return;
    }
    let mut out = Vec::new();
    for i in 0..n as usize {
        // SAFETY: `n` `D3D12_BARRIER_GROUP`s of 16 bytes: type, count, pointer.
        unsafe {
            let g = groups.add(i * 16);
            let (kind, count) = (*(g as *const u32), *(g.add(4) as *const u32) as usize);
            let data = *(g.add(8) as *const *const u8);
            if data.is_null() {
                continue;
            }
            let u = |p: *const u8, o: usize| *(p.add(o) as *const u32);
            match kind {
                0 => out.push(BarrierGroup::Global(
                    (0..count)
                        .map(|j| {
                            let p = data.add(j * 16);
                            GlobalBarrier {
                                sync_before: u(p, 0),
                                sync_after: u(p, 4),
                                access_before: u(p, 8),
                                access_after: u(p, 12),
                            }
                        })
                        .collect(),
                )),
                1 => out.push(BarrierGroup::Texture(
                    (0..count)
                        .map(|j| {
                            let p = data.add(j * 64);
                            TextureBarrier {
                                sync_before: u(p, 0),
                                sync_after: u(p, 4),
                                access_before: u(p, 8),
                                access_after: u(p, 12),
                                layout_before: u(p, 16),
                                layout_after: u(p, 20),
                                resource: com_ref(*(p.add(24) as *const *mut c_void)),
                                subresources: [
                                    u(p, 32),
                                    u(p, 36),
                                    u(p, 40),
                                    u(p, 44),
                                    u(p, 48),
                                    u(p, 52),
                                ],
                                flags: u(p, 56),
                            }
                        })
                        .collect(),
                )),
                2 => out.push(BarrierGroup::Buffer(
                    (0..count)
                        .map(|j| {
                            let p = data.add(j * 40);
                            BufferBarrier {
                                sync_before: u(p, 0),
                                sync_after: u(p, 4),
                                access_before: u(p, 8),
                                access_after: u(p, 12),
                                resource: com_ref(*(p.add(16) as *const *mut c_void)),
                                offset: *(p.add(24) as *const u64),
                                size: *(p.add(32) as *const u64),
                            }
                        })
                        .collect(),
                )),
                other => com::note_once(format!("Barrier group of unknown type {other}")),
            }
        }
    }
    push(this, Command::EnhancedBarrier(out));
}

/// Console slot 148: `(size, alignment)` returns memory the CPU writes and the GPU reads at the same address.
unsafe extern "win64" fn l_allocate_upload(
    this: *mut c_void,
    size: usize,
    alignment: usize,
) -> usize {
    // SAFETY: a list of this layer.
    let list = unsafe { state::<CommandList>(this) };
    let alignment = alignment.max(16).next_power_of_two();
    if let Some(address) = list.core.integration.upload.allocate(size, alignment) {
        return address;
    }
    com::note_once("upload memory for slot 148 is host memory: the GPU cannot address it yet");
    let mut block = vec![0u8; size + alignment].into_boxed_slice();
    let base = block.as_mut_ptr() as usize;
    let address = base.next_multiple_of(alignment);
    if let Ok(mut s) = list.state.lock() {
        s.uploads.push(block);
    }
    address
}

/// Slots the executable calls whose meaning is not measured: they record nothing and say so once.
fn unmeasured_call(slot: usize, shape: &str) {
    com::note_once(format!(
        "ID3D12GraphicsCommandList slot {slot} ({shape}): meaning not measured, ignored"
    ));
}

/// Slot 29, `(ptr, u32, u32)`, seen at one site.
unsafe extern "win64" fn l_unmeasured_29(
    _this: *mut c_void,
    _p: *const c_void,
    _a: u32,
    _b: u32,
) -> u32 {
    unmeasured_call(29, "ptr, u32, u32");
    S_OK
}

/// Slot 30, `(ptr, u32, first, count, ptr, byte offset)`, seen at one site.
unsafe extern "win64" fn l_unmeasured_30(
    _this: *mut c_void,
    _p: *const c_void,
    _a: u32,
    _first: u32,
    _count: u32,
    _q: *const c_void,
    _offset: u32,
) -> u32 {
    unmeasured_call(30, "ptr, u32, first, count, ptr, byte offset");
    S_OK
}

/// Slot 131, `(a, b, c, mask)`: called once per list just before `Close` at boot.
unsafe extern "win64" fn l_unmeasured_131(
    _this: *mut c_void,
    _a: u64,
    _b: u64,
    _c: u64,
    _mask: u64,
) -> u32 {
    unmeasured_call(131, "a, b, c, mask");
    S_OK
}

/// Slot 132, `(a, b, c, mask)`: called once per list just before `Close` at boot.
unsafe extern "win64" fn l_unmeasured_132(
    _this: *mut c_void,
    _a: u64,
    _b: u64,
    _c: u64,
    _mask: u64,
) -> u32 {
    unmeasured_call(132, "a, b, c, mask");
    S_OK
}

/// Slots the game's per-draw and per-dispatch state flushes call whose meaning is not measured (an executable
/// census: 70 with two floats, depth-bounds shaped; 75 with a pipeline flag byte; 142 with a u64 kept next to the
/// scratch buffer; 28 beside 29 and 30 with the same event-table arguments; 74 as `(address, 7, value, 0)`; 86 as
/// `(a, a, 0x10)`; 101 as `(2, 0 or 1)` around a group of clears; 129 as `(a, 1, 1)`; 62, 72, 126, 127 and 128 at
/// single sites). They record nothing; the note is made once without allocating on later calls, since some run
/// for every draw.
macro_rules! ignored_slot {
    ($name:ident, $slot:literal) => {
        unsafe extern "win64" fn $name(
            _this: *mut c_void,
            _a: u64,
            _b: u64,
            _c: u64,
            _d: u64,
        ) -> u32 {
            static NOTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !NOTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                unmeasured_call($slot, "arguments not measured");
            }
            S_OK
        }
    };
}

ignored_slot!(l_ignored_28, 28);
ignored_slot!(l_ignored_62, 62);
ignored_slot!(l_ignored_70, 70);
ignored_slot!(l_ignored_72, 72);
ignored_slot!(l_ignored_74, 74);
ignored_slot!(l_ignored_75, 75);
ignored_slot!(l_ignored_86, 86);
ignored_slot!(l_ignored_101, 101);
ignored_slot!(l_ignored_126, 126);
ignored_slot!(l_ignored_127, 127);
ignored_slot!(l_ignored_128, 128);
ignored_slot!(l_ignored_129, 129);
ignored_slot!(l_ignored_142, 142);

fn list_vtable(core: &DeviceCore) -> *const usize {
    macro_rules! f {
        ($f:expr) => {
            $f as *const () as usize
        };
    }
    let mut known = vec![
        (0, f!(l_query_interface)),
        (1, f!(add_ref)),
        (2, f!(release)),
        (7, f!(l_get_device)),
        (8, f!(l_get_type)),
        (9, f!(l_close)),
        (10, f!(l_reset)),
        (11, f!(l_clear_state)),
        (12, f!(l_draw_instanced)),
        (13, f!(l_draw_indexed_instanced)),
        (14, f!(l_dispatch)),
        (15, f!(l_copy_buffer_region)),
        (17, f!(l_copy_resource)),
        (18, f!(l_copy_tiles)),
        (19, f!(l_resolve_subresource)),
        (20, f!(l_set_topology)),
        (21, f!(l_set_viewports)),
        (22, f!(l_set_scissors)),
        (23, f!(l_set_blend_factor)),
        (24, f!(l_set_stencil_ref)),
        (25, f!(l_set_pipeline_state)),
        (26, f!(l_resource_barrier)),
        (28, f!(l_ignored_28)),
        (29, f!(l_unmeasured_29)),
        (30, f!(l_unmeasured_30)),
        (32, f!(l_set_descriptor_heaps)),
        (33, f!(l_set_compute_root_signature)),
        (34, f!(l_set_graphics_root_signature)),
        (35, f!(l_compute_table)),
        (36, f!(l_graphics_table)),
        (37, f!(l_compute_constant)),
        (38, f!(l_graphics_constant)),
        (39, f!(l_compute_constants)),
        (40, f!(l_graphics_constants)),
        (41, f!(l_compute_cbv)),
        (42, f!(l_graphics_cbv)),
        (43, f!(l_compute_srv)),
        (44, f!(l_graphics_srv)),
        (45, f!(l_compute_uav)),
        (46, f!(l_graphics_uav)),
        (47, f!(l_set_index_buffer)),
        (48, f!(l_set_vertex_buffers)),
        (49, f!(l_set_stream_output)),
        (50, f!(l_set_render_targets)),
        (51, f!(l_clear_dsv)),
        (52, f!(l_clear_rtv)),
        (53, f!(l_clear_uav_uint)),
        (54, f!(l_clear_uav_float)),
        (55, f!(l_discard_resource)),
        (59, f!(l_gpu_event)),
        (61, f!(l_execute_indirect)),
        (62, f!(l_ignored_62)),
        (70, f!(l_ignored_70)),
        (71, f!(l_write_value)),
        (72, f!(l_ignored_72)),
        (73, f!(l_wait_value)),
        (74, f!(l_ignored_74)),
        (75, f!(l_ignored_75)),
        (80, f!(l_dispatch_x)),
        (85, f!(l_copy_texture_region)),
        (86, f!(l_ignored_86)),
        (101, f!(l_ignored_101)),
        (126, f!(l_ignored_126)),
        (127, f!(l_ignored_127)),
        (128, f!(l_ignored_128)),
        (129, f!(l_ignored_129)),
        (131, f!(l_unmeasured_131)),
        (132, f!(l_unmeasured_132)),
        (141, f!(l_set_scratch_buffer)),
        (142, f!(l_ignored_142)),
        (146, f!(l_barrier)),
        (148, f!(l_allocate_upload)),
    ];
    known.extend_from_slice(&core.integration.list_methods);
    core.cached_vtable(Kind::CommandList, 192, &known)
}
