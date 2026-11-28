//! The COM objects of the device that have no module of their own: resources, root signatures, pipeline states,
//! command signatures, query heaps and the blobs the runtime hands back. Each holds its device alive and the
//! state behind it (a resource id, a built pipeline); the methods are the PC order for the public chain.

#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::ffi::c_void;
use std::sync::{Arc, OnceLock};

use super::indirect::IndirectLayout;
use super::pipelines::{Created, Handle, PendingPipeline};
use super::state::Assembly;
use crate::com::{
    self, ComPtr, E_INVALIDARG, E_POINTER, Guid, Kind, S_OK, add_ref, hand_out, kind_of,
    new_object, query_interface_for, release, state,
};
use crate::device::{DeviceCore, device_core};
use crate::resource::desc::{Dimension, Layout, ResourceDesc};
use crate::resource::{HeapKind, Origin, ResourceId};
use crate::root::RootSignature;

pub struct ResourceObject {
    device: ComPtr,
    asm: Arc<Assembly>,
    pub id: ResourceId,
    pub desc: ResourceDesc,
    pub heap: HeapKind,
}

impl Drop for ResourceObject {
    fn drop(&mut self) {
        self.asm.release_resource(self.id);
    }
}

pub struct RootObject {
    device: ComPtr,
    pub root: Arc<RootSignature>,
}

pub struct PipelineObject {
    device: ComPtr,
    /// The pipeline, which may still be building.
    pub pending: Arc<Created>,
}

pub struct SignatureObject {
    device: ComPtr,
    pub layout: IndirectLayout,
}

pub struct QueryHeapObject {
    device: ComPtr,
    pub query_type: u32,
    pub count: u32,
}

pub struct BlobObject {
    data: Vec<u8>,
}

macro_rules! f {
    ($f:expr) => {
        $f as *const () as usize
    };
}

fn device_ref(device: *mut c_void) -> Option<ComPtr> {
    // SAFETY: `device` is the device object that called.
    unsafe { ComPtr::add_ref(device) }
}

unsafe extern "win64" fn get_device_of(
    device: &ComPtr,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    // SAFETY: the object holds its device alive.
    unsafe { com::query(device.as_ptr(), riid, out) }
}

// Resources

pub fn new_resource(
    device: *mut c_void,
    asm: &Arc<Assembly>,
    id: ResourceId,
    desc: ResourceDesc,
    heap: HeapKind,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    let Some(device_ref) = device_ref(device) else {
        return E_POINTER;
    };
    // SAFETY: `device` is a device of this layer.
    let core = unsafe { device_core(device) };
    let object = new_object(
        Kind::Resource,
        resource_vtable(core),
        ResourceObject {
            device: device_ref,
            asm: asm.clone(),
            id,
            desc,
            heap,
        },
    );
    // SAFETY: the creation reference is handed out.
    unsafe { hand_out(object as *mut c_void, riid, out) }
}

/// The resource id behind a COM pointer of this layer.
///
/// # Safety
/// `p` is null or a live COM object.
pub unsafe fn resource_of<'a>(p: *mut c_void) -> Option<&'a ResourceObject> {
    if p.is_null() {
        return None;
    }
    // SAFETY: as the caller promises.
    unsafe { (kind_of(p)? == Kind::Resource).then(|| state::<ResourceObject>(p)) }
}

unsafe extern "win64" fn r_query_interface(
    this: *mut c_void,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    // SAFETY: slot 0, COM arguments.
    unsafe { query_interface_for(Kind::Resource, this, riid, out) }
}

unsafe extern "win64" fn r_get_device(
    this: *mut c_void,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    // SAFETY: a resource of this layer.
    unsafe { get_device_of(&state::<ResourceObject>(this).device, riid, out) }
}

/// `Map(subresource, read_range, data)`: a buffer's memory is host memory at its address.
unsafe extern "win64" fn r_map(
    this: *mut c_void,
    _subresource: u32,
    _read_range: *const u64,
    data: *mut *mut c_void,
) -> u32 {
    // SAFETY: a resource of this layer.
    let object = unsafe { state::<ResourceObject>(this) };
    let resources = object.asm.resources();
    let Some(r) = resources.get(object.id) else {
        return E_INVALIDARG;
    };
    let address = match (r.cpu_address(), r.origin) {
        (Some(a), _) => a,
        (None, Origin::Placed) => r.memory,
        _ => {
            com::note_once("Map of a texture whose memory is not on the host");
            return com::E_FAIL;
        }
    };
    if !data.is_null() {
        // SAFETY: an out pointer.
        unsafe { *data = address as usize as *mut c_void };
    }
    S_OK
}

/// `Unmap(subresource, written_range)`: what the CPU wrote reaches a mirrored window.
unsafe extern "win64" fn r_unmap(this: *mut c_void, _subresource: u32, written: *const u64) {
    // SAFETY: a resource of this layer; the range is two words or null.
    unsafe {
        let object = state::<ResourceObject>(this);
        let (begin, end) = if written.is_null() {
            (0, object.desc.width)
        } else {
            (*written, *written.add(1))
        };
        if end <= begin {
            return;
        }
        let resources = object.asm.resources();
        if let Some(r) = resources.get(object.id)
            && let Some(address) = r.cpu_address()
        {
            resources.space().mark_dirty(address + begin, end - begin);
        }
    }
}

pub fn layout_raw(layout: Layout) -> u32 {
    match layout {
        Layout::Unknown => 0,
        Layout::RowMajor => 1,
        Layout::Undefined64K => 2,
        Layout::Standard64K => 3,
        Layout::ConsoleTiled => 0x101,
    }
}

pub fn dimension_raw(dimension: Dimension) -> u32 {
    match dimension {
        Dimension::Buffer => 1,
        Dimension::Texture1D => 2,
        Dimension::Texture2D => 3,
        Dimension::Texture3D => 4,
    }
}

/// Writes a `D3D12_RESOURCE_DESC` back out, 56 bytes.
///
/// # Safety
/// `out` has room for 56 bytes.
pub unsafe fn write_resource_desc(out: *mut u8, d: &ResourceDesc) {
    // SAFETY: as the caller promises.
    unsafe {
        let w32 = |at: usize, v: u32| out.add(at).cast::<u32>().write_unaligned(v);
        let w64 = |at: usize, v: u64| out.add(at).cast::<u64>().write_unaligned(v);
        w32(0, dimension_raw(d.dimension));
        w32(4, 0);
        w64(8, d.alignment);
        w64(16, d.width);
        w32(24, d.height);
        w32(28, (d.depth_or_array_size & 0xFFFF) | (d.mip_levels << 16));
        w32(32, d.format);
        w32(36, d.sample_count);
        w32(40, d.sample_quality);
        w32(44, layout_raw(d.layout));
        w32(48, d.flags);
        w32(52, 0);
    }
}

/// `GetDesc`: a 56-byte struct, buffer first on the console.
unsafe extern "win64" fn r_get_desc(buffer: *mut u8, this: *mut c_void) -> *mut u8 {
    // SAFETY: a resource of this layer; `buffer` has room for 56 bytes.
    unsafe { write_resource_desc(buffer, &state::<ResourceObject>(this).desc) };
    buffer
}

unsafe extern "win64" fn r_gpu_address(this: *mut c_void) -> u64 {
    // SAFETY: a resource of this layer.
    let object = unsafe { state::<ResourceObject>(this) };
    object
        .asm
        .resources()
        .get(object.id)
        .map_or(0, |r| r.address)
}

unsafe extern "win64" fn r_heap_properties(
    this: *mut c_void,
    props: *mut u32,
    flags: *mut u32,
) -> u32 {
    // SAFETY: a resource of this layer; both out pointers are optional.
    unsafe {
        let (kind, page, pool) = match state::<ResourceObject>(this).heap {
            HeapKind::Default => (1, 1, 2),
            HeapKind::Upload => (2, 1, 1),
            HeapKind::Readback => (3, 1, 1),
        };
        if !props.is_null() {
            for (i, v) in [kind, page, pool, 1, 1].into_iter().enumerate() {
                *props.add(i) = v;
            }
        }
        if !flags.is_null() {
            *flags = 0;
        }
    }
    S_OK
}

fn resource_vtable(core: &DeviceCore) -> *const usize {
    core.cached_vtable(
        Kind::Resource,
        32,
        &[
            (0, f!(r_query_interface)),
            (1, f!(add_ref)),
            (2, f!(release)),
            (7, f!(r_get_device)),
            (8, f!(r_map)),
            (9, f!(r_unmap)),
            (10, f!(r_get_desc)),
            (11, f!(r_gpu_address)),
            (14, f!(r_heap_properties)),
        ],
    )
}

// Root signatures

pub fn new_root_signature(
    device: *mut c_void,
    root: Arc<RootSignature>,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    let Some(device_ref) = device_ref(device) else {
        return E_POINTER;
    };
    // SAFETY: `device` is a device of this layer.
    let core = unsafe { device_core(device) };
    let object = new_object(
        Kind::RootSignature,
        simple_vtable(
            core,
            Kind::RootSignature,
            f!(rs_query_interface),
            f!(rs_get_device),
        ),
        RootObject {
            device: device_ref,
            root,
        },
    );
    // SAFETY: the creation reference is handed out.
    unsafe { hand_out(object as *mut c_void, riid, out) }
}

fn simple_vtable(core: &DeviceCore, kind: Kind, qi: usize, get_device: usize) -> *const usize {
    core.cached_vtable(
        kind,
        32,
        &[(0, qi), (1, f!(add_ref)), (2, f!(release)), (7, get_device)],
    )
}

unsafe extern "win64" fn rs_query_interface(
    this: *mut c_void,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    // SAFETY: slot 0, COM arguments.
    unsafe { query_interface_for(Kind::RootSignature, this, riid, out) }
}

unsafe extern "win64" fn rs_get_device(
    this: *mut c_void,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    // SAFETY: a root signature of this layer.
    unsafe { get_device_of(&state::<RootObject>(this).device, riid, out) }
}

/// The root signature behind a COM pointer.
///
/// # Safety
/// `p` is null or a live COM object.
pub unsafe fn root_of(p: *mut c_void) -> Option<Arc<RootSignature>> {
    if p.is_null() {
        return None;
    }
    // SAFETY: as the caller promises.
    unsafe { (kind_of(p)? == Kind::RootSignature).then(|| state::<RootObject>(p).root.clone()) }
}

// Pipeline states

pub fn new_pipeline(
    device: *mut c_void,
    handle: Handle,
    compute: bool,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    let Some(device_ref) = device_ref(device) else {
        return E_POINTER;
    };
    // SAFETY: `device` is a device of this layer.
    let core = unsafe { device_core(device) };
    let vtable = core.cached_vtable(
        Kind::PipelineState,
        32,
        &[
            (0, f!(ps_query_interface)),
            (1, f!(add_ref)),
            (2, f!(release)),
            (7, f!(ps_get_device)),
            (8, f!(ps_cached_blob)),
            (11, f!(ps_scratch_blob)),
        ],
    );
    let object = new_object(
        Kind::PipelineState,
        vtable,
        PipelineObject {
            device: device_ref,
            pending: Arc::new(Created { handle, compute }),
        },
    );
    // SAFETY: the creation reference is handed out.
    unsafe { hand_out(object as *mut c_void, riid, out) }
}

unsafe extern "win64" fn ps_query_interface(
    this: *mut c_void,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    // SAFETY: slot 0, COM arguments.
    unsafe { query_interface_for(Kind::PipelineState, this, riid, out) }
}

unsafe extern "win64" fn ps_get_device(
    this: *mut c_void,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    // SAFETY: a pipeline state of this layer.
    unsafe { get_device_of(&state::<PipelineObject>(this).device, riid, out) }
}

unsafe extern "win64" fn ps_cached_blob(this: *mut c_void, out: *mut *mut c_void) -> u32 {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: a pipeline state of this layer.
    let object = unsafe { state::<PipelineObject>(this) };
    let Some(data) = object.pending.handle.cached_blob() else {
        com::note_once("GetCachedBlob was asked before the pipeline environment could be made");
        return com::E_FAIL;
    };
    // SAFETY: `out` is writable.
    unsafe { *out = new_blob(data) };
    S_OK
}

/// Console slot 11, `(0, 0, &blob)`: the compute scratch size the title multiplies by 1024. A translated module
/// keeps its private data in SPIR-V, so it is zero (the title then binds no scratch buffer).
unsafe extern "win64" fn ps_scratch_blob(
    _this: *mut c_void,
    _a: usize,
    _b: usize,
    out: *mut *mut c_void,
) -> u32 {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: `out` is writable.
    unsafe { *out = new_blob(vec![0; 4]) };
    S_OK
}

/// The pipeline behind a COM pointer, which may still be building.
///
/// # Safety
/// `p` is null or a live COM object.
pub unsafe fn pipeline_of(p: *mut c_void) -> Option<Arc<dyn PendingPipeline>> {
    if p.is_null() {
        return None;
    }
    // SAFETY: as the caller promises.
    unsafe {
        (kind_of(p)? == Kind::PipelineState).then(|| {
            let pending: Arc<dyn PendingPipeline> = state::<PipelineObject>(p).pending.clone();
            pending
        })
    }
}

/// The service handle of a pipeline state object of this layer.
///
/// # Safety
/// `p` is null or a live COM object.
pub unsafe fn pipeline_handle_of(p: *mut c_void) -> Option<Handle> {
    if p.is_null() {
        return None;
    }
    // SAFETY: as the caller promises.
    unsafe {
        (kind_of(p)? == Kind::PipelineState)
            .then(|| state::<PipelineObject>(p).pending.handle.clone())
    }
}

// Command signatures and query heaps

pub fn new_command_signature(
    device: *mut c_void,
    layout: IndirectLayout,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    let Some(device_ref) = device_ref(device) else {
        return E_POINTER;
    };
    // SAFETY: `device` is a device of this layer.
    let core = unsafe { device_core(device) };
    let object = new_object(
        Kind::CommandSignature,
        simple_vtable(
            core,
            Kind::CommandSignature,
            f!(cs_query_interface),
            f!(cs_get_device),
        ),
        SignatureObject {
            device: device_ref,
            layout,
        },
    );
    // SAFETY: the creation reference is handed out.
    unsafe { hand_out(object as *mut c_void, riid, out) }
}

unsafe extern "win64" fn cs_query_interface(
    this: *mut c_void,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    // SAFETY: slot 0, COM arguments.
    unsafe { query_interface_for(Kind::CommandSignature, this, riid, out) }
}

unsafe extern "win64" fn cs_get_device(
    this: *mut c_void,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    // SAFETY: a command signature of this layer.
    unsafe { get_device_of(&state::<SignatureObject>(this).device, riid, out) }
}

/// The layout behind a COM pointer.
///
/// # Safety
/// `p` is null or a live COM object.
pub unsafe fn signature_of(p: *mut c_void) -> Option<IndirectLayout> {
    if p.is_null() {
        return None;
    }
    // SAFETY: as the caller promises.
    unsafe { (kind_of(p)? == Kind::CommandSignature).then(|| state::<SignatureObject>(p).layout) }
}

pub fn new_query_heap(
    device: *mut c_void,
    query_type: u32,
    count: u32,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    let Some(device_ref) = device_ref(device) else {
        return E_POINTER;
    };
    // SAFETY: `device` is a device of this layer.
    let core = unsafe { device_core(device) };
    let object = new_object(
        Kind::QueryHeap,
        simple_vtable(
            core,
            Kind::QueryHeap,
            f!(qh_query_interface),
            f!(qh_get_device),
        ),
        QueryHeapObject {
            device: device_ref,
            query_type,
            count,
        },
    );
    // SAFETY: the creation reference is handed out.
    unsafe { hand_out(object as *mut c_void, riid, out) }
}

unsafe extern "win64" fn qh_query_interface(
    this: *mut c_void,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    // SAFETY: slot 0, COM arguments.
    unsafe { query_interface_for(Kind::QueryHeap, this, riid, out) }
}

unsafe extern "win64" fn qh_get_device(
    this: *mut c_void,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    // SAFETY: a query heap of this layer.
    unsafe { get_device_of(&state::<QueryHeapObject>(this).device, riid, out) }
}

// Blobs

const BLOB_IID: Guid = Guid::new(
    0x8BA5_FB08,
    0x5195,
    0x40E2,
    [0xAC, 0x58, 0x0D, 0x98, 0x9C, 0x3A, 0x01, 0x02],
);

/// An `ID3DBlob` (`IUnknown`, then `GetBufferPointer` and `GetBufferSize`) holding `data`; the caller owns the
/// one reference.
pub fn new_blob(data: Vec<u8>) -> *mut c_void {
    static TABLE: OnceLock<usize> = OnceLock::new();
    let table = *TABLE.get_or_init(|| {
        com::build_vtable(
            8,
            &[
                (0, f!(blob_query_interface)),
                (1, f!(add_ref)),
                (2, f!(release)),
                (3, f!(blob_pointer)),
                (4, f!(blob_size)),
            ],
        ) as usize
    });
    new_object(Kind::Other, table as *const usize, BlobObject { data }) as *mut c_void
}

unsafe extern "win64" fn blob_query_interface(
    this: *mut c_void,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    if out.is_null() || riid.is_null() {
        return E_POINTER;
    }
    // SAFETY: COM passes a 16-byte IID.
    let wanted = unsafe { Guid::read(riid) };
    if wanted == com::iid::UNKNOWN || wanted == BLOB_IID {
        // SAFETY: `this` is a live blob; `out` is writable.
        unsafe {
            add_ref(this);
            *out = this;
        }
        S_OK
    } else {
        // SAFETY: `out` is writable.
        unsafe { *out = std::ptr::null_mut() };
        com::E_NOINTERFACE
    }
}

unsafe extern "win64" fn blob_pointer(this: *mut c_void) -> *const u8 {
    // SAFETY: a blob made by `new_blob`.
    unsafe { state::<BlobObject>(this).data.as_ptr() }
}

unsafe extern "win64" fn blob_size(this: *mut c_void) -> usize {
    // SAFETY: a blob made by `new_blob`.
    unsafe { state::<BlobObject>(this).data.len() }
}
