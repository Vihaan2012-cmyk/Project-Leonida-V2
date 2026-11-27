//! Descriptor heaps.
//!
//! A heap's CPU handle is the host address of its descriptor bytes (the title copies descriptors through it
//! with plain moves, so it must point at writable memory), and the handles come back from heap slots 9 and
//! 10 in RAX, as the console does. On the console one address serves the CPU and the GPU, so a
//! shader-visible heap's GPU handle is the same number as its CPU handle; a heap that is not shader
//! visible has GPU handle 0. Where a heap lies is a `HeapPlacement`: the default hands out addresses from a
//! fixed window in creation order, so the same boot gives the same handles every run, which is what a title
//! comparing the handle with an address it worked out needs. What the title compares against at the first
//! heap is not known; `placement` is where to change it.
//!
//! The descriptor writers (`Create*View`, `CopyDescriptors`) belong to `descriptor.rs`; they write at
//! `cpu_start + index * increment_size(type)`.

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};

pub mod console;
mod low;
mod window;

pub use console::{ConsolePlacement, HeapAllocator, Hooked, LowBlock, LowMemory, PlacementNote};
pub use low::ResourcesLow;
pub use window::FixedWindow;

use crate::com::{self, E_INVALIDARG, E_OUTOFMEMORY, E_POINTER, Kind, add_ref, release};
use crate::device::DeviceCore;

pub const TYPE_CBV_SRV_UAV: u32 = 0;
pub const TYPE_SAMPLER: u32 = 1;
pub const TYPE_RTV: u32 = 2;
pub const TYPE_DSV: u32 = 3;
pub const FLAG_SHADER_VISIBLE: u32 = 1;

/// The most descriptors one heap may hold.
pub const MAX_DESCRIPTORS: u32 = 4 << 20;

/// The handle increment of a heap type, in bytes.
///
/// The console's values were not measured. 32 bytes is the size of a GFX10 image descriptor (T#) and holds
/// a sampler (S#, 16 bytes) and the host-side RTV/DSV records; the shaders' own address arithmetic shows
/// the stride they expect (`d3d12_vulkan.md`, descriptor model).
pub fn increment_size(heap_type: u32) -> u32 {
    match heap_type {
        TYPE_CBV_SRV_UAV | TYPE_SAMPLER | TYPE_RTV | TYPE_DSV => 32,
        _ => 0,
    }
}

/// A heap as the other steps see it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeapInfo {
    pub heap_type: u32,
    pub count: u32,
    pub flags: u32,
    pub node_mask: u32,
    pub cpu_start: u64,
    pub gpu_start: u64,
    pub increment: u32,
    /// The heap's COM object, for `ComPtr::add_ref`; valid while the heap lives.
    pub object: usize,
}

impl HeapInfo {
    pub fn bytes(&self) -> u64 {
        u64::from(self.count) * u64::from(self.increment)
    }

    pub fn shader_visible(&self) -> bool {
        self.flags & FLAG_SHADER_VISIBLE != 0
    }

    /// The CPU address of descriptor `index`.
    pub fn cpu_at(&self, index: u32) -> u64 {
        self.cpu_start + u64::from(index) * u64::from(self.increment)
    }

    /// The GPU address of descriptor `index`.
    pub fn gpu_at(&self, index: u32) -> u64 {
        self.gpu_start + u64::from(index) * u64::from(self.increment)
    }
}

// integration

/// Memory for a heap's descriptor bytes. The memory step may replace the default to give shader-visible
/// heaps a range of the GPU arena (below 4 GiB, where the shaders' 32-bit table pointers reach).
pub trait HeapPlacement: Send + Sync {
    fn place(&self, bytes: usize, shader_visible: bool) -> Option<Placed>;
}

/// A placed heap: its handles and what keeps the memory alive.
pub struct Placed {
    /// The host address of the descriptor bytes: the CPU handle.
    pub cpu: u64,
    /// The console GPU address (0 when the heap is not shader visible).
    pub gpu: u64,
    pub keep: Box<dyn Send + Sync>,
}

pub struct DescriptorHeap {
    pub device: com::ComPtr,
    core: Arc<DeviceCore>,
    info: HeapInfo,
    _memory: Placed,
}

impl Drop for DescriptorHeap {
    fn drop(&mut self) {
        self.core
            .forget_heap(self.info.gpu_start, self.info.cpu_start);
    }
}

/// The heap behind a COM pointer, if it is a descriptor heap of this layer.
///
/// # Safety
/// `heap` is an object made by this layer (or another that starts with a `ComHeader`) and stays alive while
/// the result is used.
pub unsafe fn info_of(heap: *mut c_void) -> Option<HeapInfo> {
    // SAFETY: as the caller promises.
    unsafe {
        (com::kind_of(heap)? == Kind::DescriptorHeap)
            .then(|| com::state::<DescriptorHeap>(heap).info)
    }
}

/// `CreateDescriptorHeap(desc, riid, out)`: `desc` is `D3D12_DESCRIPTOR_HEAP_DESC`, 16 bytes.
///
/// # Safety
/// `desc` is readable for 16 bytes or null; `riid` and `out` as COM passes them.
pub unsafe fn create(
    device: *mut c_void,
    core: &Arc<DeviceCore>,
    desc: *const u32,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    if out.is_null() || desc.is_null() {
        return E_POINTER;
    }
    // SAFETY: `desc` is 16 readable bytes, as the caller promises.
    let words = unsafe { std::slice::from_raw_parts(desc, 4) };
    let (heap_type, count, flags, node_mask) = (words[0], words[1], words[2], words[3]);
    if heap_type > TYPE_DSV || count == 0 || count > MAX_DESCRIPTORS || flags > FLAG_SHADER_VISIBLE
    {
        com::note_once(format!(
            "CreateDescriptorHeap refused: type {heap_type}, {count} descriptors, flags {flags:#x}"
        ));
        return E_INVALIDARG;
    }
    let shader_visible = flags & FLAG_SHADER_VISIBLE != 0;
    if shader_visible && heap_type >= TYPE_RTV {
        return E_INVALIDARG;
    }
    let increment = increment_size(heap_type);
    let bytes = count as usize * increment as usize;
    let Some(placed) = core.integration.placement.place(bytes, shader_visible) else {
        return E_OUTOFMEMORY;
    };
    let info = HeapInfo {
        heap_type,
        count,
        flags,
        node_mask,
        cpu_start: placed.cpu,
        gpu_start: placed.gpu,
        increment,
        object: 0,
    };
    // SAFETY: `device` is the device object that called.
    let Some(device_ref) = (unsafe { com::ComPtr::add_ref(device) }) else {
        return E_POINTER;
    };
    let object = com::new_object(
        Kind::DescriptorHeap,
        vtable(),
        DescriptorHeap {
            device: device_ref,
            core: core.clone(),
            info,
            _memory: placed,
        },
    );
    // SAFETY: just allocated, unshared.
    unsafe {
        let heap = &mut (*object).value;
        heap.info.object = object as usize;
        core.remember_heap(heap.info);
    }
    com::note_once(format!(
        "descriptor heap: type {heap_type}, {count} descriptors, flags {flags:#x}, cpu {:#x}, gpu {:#x}",
        info.cpu_start, info.gpu_start
    ));
    // SAFETY: `object` has the creation reference; `riid` and `out` as COM passes them.
    unsafe { com::hand_out(object as *mut c_void, riid, out) }
}

/// The heaps a device knows, by GPU address, for resolving a descriptor table handle.
#[derive(Default)]
pub struct HeapRegistry {
    by_gpu: Mutex<BTreeMap<u64, HeapInfo>>,
}

impl HeapRegistry {
    pub fn insert(&self, info: HeapInfo) {
        if info.gpu_start != 0
            && let Ok(mut map) = self.by_gpu.lock()
        {
            map.insert(info.gpu_start, info);
        }
    }

    pub fn remove(&self, gpu: u64) {
        if let Ok(mut map) = self.by_gpu.lock() {
            map.remove(&gpu);
        }
    }

    /// The shader-visible heap that contains the GPU address.
    pub fn find(&self, gpu: u64) -> Option<HeapInfo> {
        let map = self.by_gpu.lock().ok()?;
        let (_, info) = map.range(..=gpu).next_back()?;
        (gpu < info.gpu_start + info.bytes()).then_some(*info)
    }
}

unsafe extern "win64" fn query_interface(
    this: *mut c_void,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    // SAFETY: slot 0 of a descriptor heap, COM arguments.
    unsafe { com::query_interface_for(Kind::DescriptorHeap, this, riid, out) }
}

unsafe extern "win64" fn get_device(
    this: *mut c_void,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    // SAFETY: `this` is a descriptor heap of this layer; it holds its device alive.
    unsafe {
        com::query(
            com::state::<DescriptorHeap>(this).device.as_ptr(),
            riid,
            out,
        )
    }
}

/// `GetDesc`, a 16-byte struct: the console takes the buffer first, `(buffer, this)`.
unsafe extern "win64" fn get_desc(buffer: *mut u32, this: *mut c_void) -> *mut u32 {
    // SAFETY: `this` is a descriptor heap; `buffer` has room for 16 bytes.
    unsafe {
        let info = com::state::<DescriptorHeap>(this).info;
        *buffer = info.heap_type;
        *buffer.add(1) = info.count;
        *buffer.add(2) = info.flags;
        *buffer.add(3) = info.node_mask;
    }
    buffer
}

/// An 8-byte struct comes back in RAX on the console, not through a buffer.
unsafe extern "win64" fn get_cpu_start(this: *mut c_void) -> u64 {
    // SAFETY: `this` is a descriptor heap of this layer.
    unsafe { com::state::<DescriptorHeap>(this).info.cpu_start }
}

unsafe extern "win64" fn get_gpu_start(this: *mut c_void) -> u64 {
    // SAFETY: `this` is a descriptor heap of this layer.
    unsafe { com::state::<DescriptorHeap>(this).info.gpu_start }
}

fn vtable() -> *const usize {
    static TABLE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let table = *TABLE.get_or_init(|| {
        com::object_vtable(
            32,
            &[
                (0, query_interface as *const () as usize),
                (1, add_ref as *const () as usize),
                (2, release as *const () as usize),
                (7, get_device as *const () as usize),
                (8, get_desc as *const () as usize),
                (9, get_cpu_start as *const () as usize),
                (10, get_gpu_start as *const () as usize),
            ],
        ) as usize
    });
    table as *const usize
}

#[cfg(test)]
#[path = "heap/handles_tests.rs"]
mod handles_tests;

#[cfg(test)]
#[path = "heap/window_tests.rs"]
mod window_tests;
