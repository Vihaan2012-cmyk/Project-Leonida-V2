//! The device methods of the resource, descriptor and pipeline steps, installed through
//! `Integration::device_methods`: PC slot order for the public chain, the measured console methods where they
//! differ (`d3d12_slots.txt`), struct returns with the buffer first.

use std::ffi::c_void;
use std::sync::Arc;

use content_filter::Verdict;

use super::indirect::read_layout;
use super::objects::{
    new_command_signature, new_pipeline, new_query_heap, new_resource, new_root_signature,
    resource_of, root_of,
};
use super::rawdesc::*;
use super::resapi::Placement;
use super::state::{Assembly, assembly_of};
use crate::com::{self, E_FAIL, E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER, S_OK};
use crate::descriptor::views::{SrvKind, ViewResource};
use crate::descriptor::{self, SLOT_BYTES};
use crate::heap;
use crate::pipeline::{ComputePsoDesc, GraphicsPsoDesc, PipelineError};
use crate::pso_stream::{Dialect, HostMemory, Memory, PipelineStream};
use crate::resource::desc::AllocationInfo;
use crate::resource::{HeapKind, layout};
use crate::root::RootSignature;

macro_rules! f {
    ($f:expr) => {
        $f as *const () as usize
    };
}

fn asm(this: *mut c_void) -> Option<Arc<Assembly>> {
    // SAFETY: `this` is the device that called.
    unsafe { assembly_of(this) }
}

fn pipeline_hresult(what: &str, error: PipelineError) -> u32 {
    let code = match error {
        PipelineError::Bytecode(_) | PipelineError::Invalid(_) | PipelineError::Format(_) => {
            E_INVALIDARG
        }
        _ => E_FAIL,
    };
    com::note(format!("{what} failed: {error}"));
    code
}

// Resources

#[allow(clippy::too_many_arguments)]
fn create_resource(
    this: *mut c_void,
    placement: Placement,
    desc: *const u8,
    state: u32,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    if out.is_null() || desc.is_null() {
        return E_POINTER;
    }
    let Some(asm) = asm(this) else {
        return E_FAIL;
    };
    // SAFETY: the title's `D3D12_RESOURCE_DESC`.
    let Some(desc) = (unsafe { read_resource_desc(desc) }) else {
        com::note_once("a resource description with an unknown dimension was refused");
        return E_INVALIDARG;
    };
    match asm.create_resource(placement, &desc, state) {
        Ok(id) => {
            let heap = match placement {
                Placement::Committed(heap) => heap,
                _ => HeapKind::Default,
            };
            new_resource(this, &asm, id, desc, heap, riid, out)
        }
        Err(e) => {
            com::note(format!("creating a resource failed: {e}"));
            match e {
                crate::resource::ResourceError::Desc(_) => E_INVALIDARG,
                _ => E_OUTOFMEMORY,
            }
        }
    }
}

/// Slots 27 and 76: `(heap properties, heap flags, desc, initial state, clear value, riid, out)`.
#[allow(clippy::too_many_arguments)]
unsafe extern "win64" fn d_create_committed_resource(
    this: *mut c_void,
    props: *const u8,
    _heap_flags: u32,
    desc: *const u8,
    state: u32,
    _clear: *const c_void,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    // SAFETY: the title's heap properties.
    let heap = unsafe { read_heap_kind(props) };
    create_resource(this, Placement::Committed(heap), desc, state, riid, out)
}

/// The resource state a `D3D12_BARRIER_LAYOUT` puts a resource in, for the methods that take a layout.
fn state_of_layout(layout: u32) -> u32 {
    match layout {
        1 => 0xAC3,
        2 => 0x4,
        3 => 0x8,
        4 => 0x10,
        5 => 0x20,
        6 => 0xC0,
        7 => 0x800,
        8 => 0x400,
        _ => 0,
    }
}

/// Slot 127, `CreateCommittedResource3` by its measured call (`0x193e1b4`, the display buffers made in device setup):
/// `(heap properties, heap flags, desc1, initial layout, clear value, protected session, castable format count,
/// castable formats, riid, out)`. `D3D12_RESOURCE_DESC1` starts with the `D3D12_RESOURCE_DESC` fields.
#[allow(clippy::too_many_arguments)]
unsafe extern "win64" fn d_create_committed_resource3(
    this: *mut c_void,
    props: *const u8,
    _heap_flags: u32,
    desc: *const u8,
    layout: u32,
    _clear: *const c_void,
    _protected: *const c_void,
    _castable_count: u32,
    _castable: *const u32,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    // SAFETY: the title's heap properties.
    let heap = unsafe { read_heap_kind(props) };
    create_resource(
        this,
        Placement::Committed(heap),
        desc,
        state_of_layout(layout),
        riid,
        out,
    )
}

/// Slot 30: `(desc, initial state, clear value, riid, out)`.
unsafe extern "win64" fn d_create_reserved_resource(
    this: *mut c_void,
    desc: *const u8,
    state: u32,
    _clear: *const c_void,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    create_resource(this, Placement::Reserved, desc, state, riid, out)
}

/// Slot 133, `CreatePlacedResourceX`: `(va, desc, state, clear, riid, out)`, where `state` is `0xFFFFFFFF`.
unsafe extern "win64" fn d_create_placed_resource_x(
    this: *mut c_void,
    va: u64,
    desc: *const u8,
    state: u32,
    _clear: *const c_void,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    create_resource(this, Placement::Placed(va), desc, state, riid, out)
}

/// Slot 25: a 16-byte struct, so `(buffer, this, visible mask, count, descs)`.
unsafe extern "win64" fn d_resource_allocation_info(
    buffer: *mut u64,
    _this: *mut c_void,
    _mask: u32,
    count: u32,
    descs: *const u8,
) -> *mut u64 {
    let (mut size, mut alignment) = (0u64, 0u64);
    for i in 0..count as usize {
        // SAFETY: `count` descriptions of 56 bytes each.
        let Some(desc) = (unsafe { read_resource_desc(descs.add(56 * i)) }) else {
            continue;
        };
        if let Ok(AllocationInfo {
            size: s,
            alignment: a,
        }) = desc.allocation_info()
        {
            size = size.next_multiple_of(a) + s;
            alignment = alignment.max(a);
        }
    }
    // SAFETY: `buffer` has room for 16 bytes.
    unsafe {
        *buffer = size;
        *buffer.add(1) = alignment.max(1);
    }
    buffer
}

/// Slot 108, `GetResourceAllocationInfo1`: slot 25's arguments and a sixth, an array of `count` 24-byte
/// `D3D12_RESOURCE_ALLOCATION_INFO1` (offset, alignment, size) that the title may leave null.
unsafe extern "win64" fn d_resource_allocation_info1(
    buffer: *mut u64,
    _this: *mut c_void,
    _mask: u32,
    count: u32,
    descs: *const u8,
    info1: *mut u64,
) -> *mut u64 {
    let (mut size, mut alignment) = (0u64, 0u64);
    for i in 0..count as usize {
        // SAFETY: `count` descriptions of 56 bytes each.
        let Some(desc) = (unsafe { read_resource_desc(descs.add(56 * i)) }) else {
            continue;
        };
        if let Ok(AllocationInfo {
            size: s,
            alignment: a,
        }) = desc.allocation_info()
        {
            let offset = size.next_multiple_of(a);
            if !info1.is_null() {
                // SAFETY: `count` entries of three words.
                unsafe {
                    *info1.add(3 * i) = offset;
                    *info1.add(3 * i + 1) = a;
                    *info1.add(3 * i + 2) = s;
                }
            }
            size = offset + s;
            alignment = alignment.max(a);
        }
    }
    // SAFETY: `buffer` has room for 16 bytes.
    unsafe {
        *buffer = size;
        *buffer.add(1) = alignment.max(1);
    }
    buffer
}

/// Slot 38: `(desc, first, count, base offset, layouts, rows, row sizes, total)`.
#[allow(clippy::too_many_arguments)]
unsafe extern "win64" fn d_copyable_footprints(
    _this: *mut c_void,
    desc: *const u8,
    first: u32,
    count: u32,
    base: u64,
    layouts: *mut u8,
    rows: *mut u32,
    row_sizes: *mut u64,
    total: *mut u64,
) {
    // SAFETY: the title's description.
    let Some(desc) = (unsafe { read_resource_desc(desc) }) else {
        return;
    };
    let Ok(info) = desc.format_info().or_else(|e| {
        if desc.is_buffer() {
            crate::descriptor::format::format(crate::descriptor::format::dxgi::R8_UNORM).ok_or(e)
        } else {
            Err(e)
        }
    }) else {
        return;
    };
    let (footprints, _) = if desc.is_buffer() {
        (
            vec![layout::Footprint {
                offset: 0,
                width_blocks: desc.width as u32,
                rows: 1,
                depth: 1,
                row_bytes: desc.width,
                row_pitch: desc.width,
            }],
            desc.width,
        )
    } else {
        layout::row_major_footprints(
            desc.width as u32,
            desc.height,
            desc.depth(),
            desc.mips(),
            desc.array_size(),
            info.block,
            info.bytes,
        )
    };
    let Some(origin) = footprints.get(first as usize).map(|f| f.offset) else {
        return;
    };
    let mut end = 0;
    for k in 0..count as usize {
        let Some(f) = footprints.get(first as usize + k) else {
            break;
        };
        let mip = (first as usize + k) as u32 % desc.mips();
        // SAFETY: the title's arrays hold `count` entries; the placed footprint is 32 bytes.
        unsafe {
            if !layouts.is_null() {
                let p = layouts.add(32 * k);
                p.cast::<u64>().write_unaligned(base + (f.offset - origin));
                p.add(8).cast::<u32>().write_unaligned(desc.format);
                let (w, h) = if desc.is_buffer() {
                    (desc.width as u32, 1)
                } else {
                    (
                        (desc.width as u32 >> mip).max(1),
                        (desc.height >> mip).max(1),
                    )
                };
                p.add(12).cast::<u32>().write_unaligned(w);
                p.add(16).cast::<u32>().write_unaligned(h);
                p.add(20).cast::<u32>().write_unaligned(f.depth);
                p.add(24).cast::<u32>().write_unaligned(f.row_pitch as u32);
            }
            if !rows.is_null() {
                *rows.add(k) = f.rows;
            }
            if !row_sizes.is_null() {
                *row_sizes.add(k) = f.row_bytes;
            }
        }
        end = f.offset - origin + f.size();
    }
    if !total.is_null() {
        // SAFETY: an out pointer.
        unsafe { *total = end };
    }
}

unsafe extern "win64" fn d_residency(
    _this: *mut c_void,
    _count: u32,
    _objects: *const c_void,
) -> u32 {
    S_OK
}

// Descriptors

fn view_resource(asm: &Assembly, resource: *mut c_void) -> Option<ViewResource> {
    // SAFETY: the title's resource.
    let id = unsafe { resource_of(resource) }?.id;
    asm.resources().get(id).map(ViewResource::from)
}

unsafe extern "win64" fn d_create_cbv(this: *mut c_void, desc: *const u8, dest: u64) {
    let Some(asm) = asm(this) else { return };
    if desc.is_null() || dest == 0 {
        return;
    }
    // SAFETY: the title's constant buffer view description and a heap slot.
    unsafe {
        let m = Mem(desc);
        asm.resources().mark_hot(m.u64(0), u64::from(m.u32(8)));
        let slot = asm.descriptors().create_cbv(m.u64(0), m.u32(8));
        descriptor::write_slot(dest, &slot);
    }
}

unsafe extern "win64" fn d_create_srv(
    this: *mut c_void,
    resource: *mut c_void,
    desc: *const u8,
    dest: u64,
) {
    let Some(asm) = asm(this) else { return };
    if dest == 0 {
        return;
    }
    let Some(mut view) = view_resource(&asm, resource) else {
        // SAFETY: a heap slot.
        unsafe { descriptor::write_slot(dest, &crate::descriptor::views::NULL) };
        return;
    };
    // SAFETY: the title's description.
    let Some(mut srv) = (unsafe { read_srv(desc, &view) }) else {
        com::note_once(
            "a shader resource view of a kind this layer has no descriptor for was left empty",
        );
        return;
    };
    if let Some(filter) = super::hooks::content_filter()
        && filter.on_bind_texture(view.address) == Verdict::NeutralTexture
        && matches!(srv.kind, SrvKind::Texture2D { .. })
        && let Some(neutral) = asm.neutral_texture()
    {
        view = neutral;
        srv.format = crate::descriptor::format::dxgi::UNKNOWN;
        srv.kind = SrvKind::Texture2D {
            most_detailed_mip: 0,
            mip_levels: 1,
            plane_slice: 0,
            min_lod_clamp: 0.0,
        };
    }
    let made = asm.descriptors().create_srv(Some(&view), &srv);
    match made {
        Ok((slot, registration)) => {
            // SAFETY: a heap slot.
            unsafe { descriptor::write_slot(dest, &slot) };
            if let Some(r) = registration
                && r.new
            {
                asm.bindless.note_texture(r.index);
            }
            asm.refresh_placed_texture(resource);
        }
        Err(e) => {
            com::note_once(format!("a shader resource view was refused: {e}"));
        }
    }
}

unsafe extern "win64" fn d_create_uav(
    this: *mut c_void,
    resource: *mut c_void,
    counter: *mut c_void,
    desc: *const u8,
    dest: u64,
) {
    let Some(asm) = asm(this) else { return };
    if dest == 0 {
        return;
    }
    let Some(view) = view_resource(&asm, resource) else {
        // SAFETY: a heap slot.
        unsafe { descriptor::write_slot(dest, &crate::descriptor::views::NULL) };
        return;
    };
    let counter = view_resource(&asm, counter);
    // SAFETY: the title's description.
    let Some(uav) = (unsafe { read_uav(desc, &view) }) else {
        com::note_once(
            "an unordered access view of a kind this layer has no descriptor for was left empty",
        );
        return;
    };
    let made = asm
        .descriptors()
        .create_uav(Some(&view), counter.as_ref(), &uav);
    match made {
        Ok((slot, registration)) => {
            // SAFETY: a heap slot.
            unsafe { descriptor::write_slot(dest, &slot) };
            if let Some(r) = registration
                && r.new
            {
                asm.bindless.note_texture(r.index);
            }
        }
        Err(e) => com::note_once(format!("an unordered access view was refused: {e}")),
    }
}

fn target_view(this: *mut c_void, resource: *mut c_void, desc: *const u8, dest: u64, depth: bool) {
    let Some(asm) = asm(this) else { return };
    if dest == 0 {
        return;
    }
    let Some(view) = view_resource(&asm, resource) else {
        return;
    };
    // SAFETY: the title's description.
    let target = unsafe {
        if depth {
            read_dsv(desc, &view)
        } else {
            read_rtv(desc, &view)
        }
    };
    let Some(target) = target else {
        com::note_once(
            "a render target view of a kind this layer has no descriptor for was left empty",
        );
        return;
    };
    let made = if depth {
        asm.descriptors().create_dsv(&view, &target)
    } else {
        asm.descriptors().create_rtv(&view, &target)
    };
    match made {
        Ok((slot, registration)) => {
            // SAFETY: a heap slot.
            unsafe { descriptor::write_slot(dest, &slot) };
            if let Some(r) = registration
                && r.new
            {
                asm.bindless.note_texture(r.index);
            }
        }
        Err(e) => com::note_once(format!("a render target view was refused: {e}")),
    }
}

unsafe extern "win64" fn d_create_rtv(
    this: *mut c_void,
    resource: *mut c_void,
    desc: *const u8,
    dest: u64,
) {
    target_view(this, resource, desc, dest, false);
}

unsafe extern "win64" fn d_create_dsv(
    this: *mut c_void,
    resource: *mut c_void,
    desc: *const u8,
    dest: u64,
) {
    target_view(this, resource, desc, dest, true);
}

/// Slots 70 and 71, `CreateShaderResourceViewX` and `CreateUnorderedAccessViewX`: `(resource desc, view desc,
/// handle)`. The view description is the PC structure with the GPU address of the memory in the word at 0x28, so
/// no resource object is involved; the title makes these for memory it placed itself.
fn view_from_address(
    this: *mut c_void,
    resource_desc: *const u8,
    view_desc: *const u8,
    dest: u64,
    unordered: bool,
) {
    let Some(asm) = asm(this) else { return };
    if resource_desc.is_null() || view_desc.is_null() || dest == 0 {
        return;
    }
    // SAFETY: the title's resource description and its view description, 0x38 bytes.
    let (desc, address) = unsafe { (read_resource_desc(resource_desc), Mem(view_desc).u64(0x28)) };
    let Some(desc) = desc else {
        com::note_once(
            "a view over a resource description with an unknown dimension was left empty",
        );
        return;
    };
    let view = ViewResource { address, desc };
    if desc.is_buffer() {
        asm.resources().mark_hot(address, desc.width);
    }
    let made = if unordered {
        // SAFETY: as above.
        let Some(uav) = (unsafe { read_uav(view_desc, &view) }) else {
            com::note_once(
                "an unordered access view of a kind this layer has no descriptor for was left empty",
            );
            return;
        };
        asm.descriptors().create_uav(Some(&view), None, &uav)
    } else {
        // SAFETY: as above.
        let Some(srv) = (unsafe { read_srv(view_desc, &view) }) else {
            com::note_once(
                "a shader resource view of a kind this layer has no descriptor for was left empty",
            );
            return;
        };
        asm.descriptors().create_srv(Some(&view), &srv)
    };
    match made {
        Ok((slot, registration)) => {
            // SAFETY: a heap slot.
            unsafe { descriptor::write_slot(dest, &slot) };
            if let Some(r) = registration
                && r.new
            {
                asm.bindless.note_texture(r.index);
            }
        }
        Err(e) => com::note_once(format!("a view made from an address was refused: {e}")),
    }
}

unsafe extern "win64" fn d_create_srv_x(
    this: *mut c_void,
    resource_desc: *const u8,
    view_desc: *const u8,
    dest: u64,
) {
    view_from_address(this, resource_desc, view_desc, dest, false);
}

unsafe extern "win64" fn d_create_uav_x(
    this: *mut c_void,
    resource_desc: *const u8,
    view_desc: *const u8,
    dest: u64,
) {
    view_from_address(this, resource_desc, view_desc, dest, true);
}

/// Slot 61, `CreateSamplerX(desc, handle)`: the title's only sampler creation. The description is the console's
/// own 0x54-byte structure, which `read_console_sampler` documents.
unsafe extern "win64" fn d_create_sampler_x(this: *mut c_void, desc: *const u8, dest: u64) {
    let Some(asm) = asm(this) else { return };
    if desc.is_null() || dest == 0 {
        return;
    }
    // SAFETY: the title's sampler description and a heap slot.
    unsafe {
        let (words, registration) = asm
            .descriptors()
            .create_sampler_console(&read_console_sampler(desc));
        descriptor::write_sampler_slot(dest, &words);
        if registration.new {
            asm.bindless.note_sampler(registration.index);
        }
    }
}

unsafe extern "win64" fn d_create_sampler(this: *mut c_void, desc: *const u8, dest: u64) {
    let Some(asm) = asm(this) else { return };
    if desc.is_null() || dest == 0 {
        return;
    }
    // SAFETY: the title's sampler description and a heap slot.
    unsafe {
        let (words, registration) = asm.descriptors().create_sampler(&read_sampler(desc));
        descriptor::write_sampler_slot(dest, &words);
        if registration.new {
            asm.bindless.note_sampler(registration.index);
        }
    }
}

/// Slot 24: `(count, dest, source, heap type)`, which are CPU handles: host addresses of 32-byte slots.
unsafe extern "win64" fn d_copy_descriptors_simple(
    _this: *mut c_void,
    count: u32,
    dest: u64,
    source: u64,
    heap_type: u32,
) {
    let bytes = count as usize * heap::increment_size(heap_type) as usize;
    if bytes == 0 || dest == 0 || source == 0 {
        return;
    }
    // SAFETY: both ranges are `count` slots of heaps of this layer.
    unsafe {
        std::ptr::copy(
            source as usize as *const u8,
            dest as usize as *mut u8,
            bytes,
        )
    };
}

/// Slot 23: ranges of CPU handles; sizes may be null for single descriptors.
#[allow(clippy::too_many_arguments)]
unsafe extern "win64" fn d_copy_descriptors(
    _this: *mut c_void,
    dest_ranges: u32,
    dest_starts: *const u64,
    dest_sizes: *const u32,
    source_ranges: u32,
    source_starts: *const u64,
    source_sizes: *const u32,
    heap_type: u32,
) {
    let step = u64::from(heap::increment_size(heap_type));
    let expand = |ranges: u32, starts: *const u64, sizes: *const u32| -> Vec<u64> {
        let mut out = Vec::new();
        for i in 0..ranges as usize {
            // SAFETY: `ranges` entries in each array.
            let (start, size) = unsafe {
                (
                    *starts.add(i),
                    if sizes.is_null() { 1 } else { *sizes.add(i) },
                )
            };
            out.extend((0..u64::from(size)).map(|k| start + k * step));
        }
        out
    };
    let (dest, source) = (
        expand(dest_ranges, dest_starts, dest_sizes),
        expand(source_ranges, source_starts, source_sizes),
    );
    for (to, from) in dest.iter().zip(&source) {
        // SAFETY: both are descriptor slots of heaps of this layer.
        unsafe {
            std::ptr::copy(
                *from as usize as *const u8,
                *to as usize as *mut u8,
                SLOT_BYTES as usize,
            )
        };
    }
}

// Root signatures and pipelines

/// Slot 16: `(node mask, blob, size, riid, out)`.
unsafe extern "win64" fn d_create_root_signature(
    this: *mut c_void,
    _node_mask: u32,
    blob: *const u8,
    size: usize,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    if blob.is_null() || out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the title's serialized signature.
    let bytes = unsafe { std::slice::from_raw_parts(blob, size) };
    match RootSignature::parse(bytes) {
        Ok(root) => new_root_signature(this, Arc::new(root), riid, out),
        Err(e) => {
            com::note(format!("a root signature was refused: {e}"));
            E_INVALIDARG
        }
    }
}

/// Hands a graphics description to the service and returns the pipeline object, whose build is still going.
fn create_graphics(
    this: *mut c_void,
    desc: GraphicsPsoDesc,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    let Some(asm) = asm(this) else {
        return E_FAIL;
    };
    if let Err(e) = desc.check() {
        return pipeline_hresult("a graphics pipeline", e);
    }
    let handle = asm.pipeline_service.create_graphics(desc);
    new_pipeline(this, handle, false, riid, out)
}

unsafe extern "win64" fn d_create_graphics_pipeline_state(
    this: *mut c_void,
    desc: *const u8,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    if desc.is_null() || out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the title's description and the root signature it names.
    unsafe {
        let Some(root) = root_of(pso_root(desc)) else {
            com::note_once("a graphics pipeline without a root signature was refused");
            return E_INVALIDARG;
        };
        create_graphics(this, read_graphics_pso(desc, root), riid, out)
    }
}

fn create_compute(
    this: *mut c_void,
    desc: ComputePsoDesc,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    let Some(asm) = asm(this) else {
        return E_FAIL;
    };
    let handle = asm.pipeline_service.create_compute(desc);
    new_pipeline(this, handle, true, riid, out)
}

/// Slot 11, and slot 62 with two words between: `D3D12_COMPUTE_PIPELINE_STATE_DESC`.
unsafe extern "win64" fn d_create_compute_pipeline_state(
    this: *mut c_void,
    desc: *const u8,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    if desc.is_null() || out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the title's description and the root signature it names.
    let desc = unsafe { read_compute_pso(desc, root_of(pso_root(desc))) };
    create_compute(this, desc, riid, out)
}

unsafe extern "win64" fn d_create_compute_pipeline_state_x(
    this: *mut c_void,
    desc: *const u8,
    _a: usize,
    _b: usize,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    // SAFETY: as slot 11.
    unsafe { d_create_compute_pipeline_state(this, desc, riid, out) }
}

/// Slot 86: `D3D12_PIPELINE_STATE_STREAM_DESC {size, pointer}`, then `(riid, out)`. The title's streams are the
/// console's numbering of the d3dx12 subobjects.
unsafe extern "win64" fn d_create_pipeline_state_stream(
    this: *mut c_void,
    desc: *const u8,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    if desc.is_null() || out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the title's two-word stream description.
    let (size, pointer) = unsafe { (Mem(desc).u64(0), Mem(desc).u64(8)) };
    if pointer == 0 || size == 0 || size > MAX_STREAM_BYTES {
        com::note_once("a pipeline state stream with no bytes was refused");
        return E_INVALIDARG;
    }
    // SAFETY: the title keeps `size` bytes at `pointer` alive for the call.
    let bytes = unsafe { std::slice::from_raw_parts(pointer as usize as *const u8, size as usize) };
    create_from_stream(this, bytes, &HostMemory, riid, out)
}

const MAX_STREAM_BYTES: u64 = 0x10000;

fn create_from_stream(
    this: *mut c_void,
    bytes: &[u8],
    memory: &dyn Memory,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    let stream = match PipelineStream::parse(bytes, memory, Dialect::Console) {
        Ok(stream) => stream,
        Err(e) => {
            com::note(format!("a pipeline state stream was refused: {e}"));
            return E_INVALIDARG;
        }
    };
    if let Err(e) = stream.validate() {
        com::note(format!("a pipeline state stream was refused: {e}"));
        return E_INVALIDARG;
    }
    for kind in stream.unsupported() {
        com::note_once(format!(
            "a pipeline state stream asks for {kind}, which is not honoured"
        ));
    }
    // SAFETY: the title's root signature object, or any pointer, which `root_of` checks by kind.
    let root = stream
        .root_signature
        .and_then(|p| unsafe { root_of(p as usize as *mut c_void) });
    if stream.is_compute() {
        return match stream.compute(root) {
            Ok(desc) => create_compute(this, desc, riid, out),
            Err(e) => pipeline_hresult("a compute pipeline stream", e.into()),
        };
    }
    let Some(root) = root else {
        com::note_once("a graphics pipeline stream without a root signature was refused");
        return E_INVALIDARG;
    };
    match stream.graphics(root) {
        Ok(desc) => create_graphics(this, desc, riid, out),
        Err(e) => pipeline_hresult("a graphics pipeline stream", e.into()),
    }
}

/// Slots 55, 57 and 67 build pipelines from the title's own shader cache. They fail at once: the title treats
/// the failure as a miss (only device-removed codes end its run) and creates the pipeline through slot 86 or 62.
macro_rules! cache_path {
    ($name:ident, $text:literal) => {
        unsafe extern "win64" fn $name(
            _this: *mut c_void,
            _desc: *const u8,
            _riid: *const u8,
            _out: *mut *mut c_void,
        ) -> u32 {
            com::note_once($text);
            E_NOTIMPL
        }
    };
}

cache_path!(
    d_create_pipeline_from_cache_55,
    "device slot 55 (cache pipeline creation) is declined; the title falls back to slots 86 and 62"
);
cache_path!(
    d_create_graphics_from_cache,
    "device slot 57 (console graphics pipeline from the cache) is declined; the title falls back to slot 86"
);
cache_path!(
    d_create_compute_from_cache,
    "device slot 67 (console compute pipeline from the cache) is declined; the title falls back to slot 62"
);

/// Slot 49, `(0x11f, 1)`: called once after the device is created, only when the reported OS version passes the
/// title's gate; its result is not read. What feature 0x11f is, is not measured.
unsafe extern "win64" fn d_set_feature_49(_this: *mut c_void, feature: u32, value: u32) -> u32 {
    com::note_once(format!(
        "device slot 49 ({feature:#x}, {value}) is accepted and ignored"
    ));
    S_OK
}

/// Slot 50, `(callback, 0, callback)`: the title registers a GPU crash callback during its crash-handler set-up.
/// No GPU fault is ever reported to it.
unsafe extern "win64" fn d_register_crash_callback_50(
    _this: *mut c_void,
    _callback: *const c_void,
    _context: *const c_void,
    _second: *const c_void,
) -> u32 {
    com::note_once(
        "device slot 50 (GPU crash callback registration) is accepted; no fault is reported to it",
    );
    S_OK
}

/// Slot 74, `(address, pages, &out)`: the title registers the range of 64 KiB pages it reserved before mapping
/// it with queue slot 31, and keeps the word written to `out` without reading it again. Accepted; the word is 1.
unsafe extern "win64" fn d_register_virtual_range_74(
    _this: *mut c_void,
    _address: u64,
    _pages: u32,
    out: *mut u64,
) -> u32 {
    com::note_once(
        "device slot 74 (virtual range registration) is accepted; the handle it returns is 1",
    );
    if !out.is_null() {
        // SAFETY: an out pointer.
        unsafe { *out = 1 };
    }
    S_OK
}

/// Slot 101, `CreateStateObject(desc, riid, out)`: a ray tracing pipeline made from subobject lists. The title
/// takes a failure as "no ray tracing" and goes on without it.
unsafe extern "win64" fn d_create_state_object(
    _this: *mut c_void,
    _desc: *const u8,
    _riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    com::note_once(
        "device slot 101 (ray tracing state object) is declined; the title goes on without ray tracing",
    );
    if !out.is_null() {
        // SAFETY: an out pointer.
        unsafe { *out = std::ptr::null_mut() };
    }
    super::DXGI_ERROR_UNSUPPORTED
}

/// Slots 119 and 132 are called only from the title's crash handler (an unhandled exception filter and the GPU
/// crash callback it registers through slot 50): a crash-context append `(buffer, size, 0)` and a dump request
/// `(3)`. Nothing is recorded.
unsafe extern "win64" fn d_crash_handler_call(
    _this: *mut c_void,
    _a: usize,
    _b: usize,
    _c: usize,
) -> u32 {
    com::note_once("a device slot only the title's crash handler calls (119 or 132) is accepted");
    S_OK
}

/// Slot 39: `D3D12_QUERY_HEAP_DESC`, `(type, count, node mask)`.
unsafe extern "win64" fn d_create_query_heap(
    this: *mut c_void,
    desc: *const u32,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    if desc.is_null() || out.is_null() {
        return E_POINTER;
    }
    // SAFETY: a 12-byte description.
    let (kind, count) = unsafe { (*desc, *desc.add(1)) };
    new_query_heap(this, kind, count, riid, out)
}

/// Slot 41: `(desc, root signature, riid, out)`.
unsafe extern "win64" fn d_create_command_signature(
    this: *mut c_void,
    desc: *const u8,
    _root: *mut c_void,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    if desc.is_null() || out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the title's `D3D12_COMMAND_SIGNATURE_DESC`.
    let layout = unsafe { read_layout(desc) };
    new_command_signature(this, layout, riid, out)
}

/// The methods `Integration::device_methods` takes.
pub fn table() -> Vec<(usize, usize)> {
    vec![
        (10, f!(d_create_graphics_pipeline_state)),
        (11, f!(d_create_compute_pipeline_state)),
        (16, f!(d_create_root_signature)),
        (17, f!(d_create_cbv)),
        (18, f!(d_create_srv)),
        (19, f!(d_create_uav)),
        (20, f!(d_create_rtv)),
        (21, f!(d_create_dsv)),
        (22, f!(d_create_sampler)),
        (23, f!(d_copy_descriptors)),
        (24, f!(d_copy_descriptors_simple)),
        (25, f!(d_resource_allocation_info)),
        (27, f!(d_create_committed_resource)),
        (30, f!(d_create_reserved_resource)),
        (34, f!(d_residency)),
        (35, f!(d_residency)),
        (38, f!(d_copyable_footprints)),
        (39, f!(d_create_query_heap)),
        (41, f!(d_create_command_signature)),
        (49, f!(d_set_feature_49)),
        (50, f!(d_register_crash_callback_50)),
        (55, f!(d_create_pipeline_from_cache_55)),
        (57, f!(d_create_graphics_from_cache)),
        (61, f!(d_create_sampler_x)),
        (62, f!(d_create_compute_pipeline_state_x)),
        (67, f!(d_create_compute_from_cache)),
        (70, f!(d_create_srv_x)),
        (71, f!(d_create_uav_x)),
        (74, f!(d_register_virtual_range_74)),
        (76, f!(d_create_committed_resource)),
        (86, f!(d_create_pipeline_state_stream)),
        (101, f!(d_create_state_object)),
        (108, f!(d_resource_allocation_info1)),
        (119, f!(d_crash_handler_call)),
        (127, f!(d_create_committed_resource3)),
        (132, f!(d_crash_handler_call)),
        (133, f!(d_create_placed_resource_x)),
    ]
}
