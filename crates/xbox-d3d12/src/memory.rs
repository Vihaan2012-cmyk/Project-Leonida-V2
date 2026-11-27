//! The game memory the GPU reads: physical pages and host memory imported into Vulkan.
//!
//! The console's GPU addresses are the game's own virtual addresses, and translated shaders compute
//! `heap_base + address`. One sparse `VkBuffer` (the window) spans the address range the game's GPU-visible
//! memory lives in; `heap_base` is the buffer's device address minus the window's first address, so the sum lands
//! on the byte the game meant. Whatever the game maps there, a committed `XMemVirtualAlloc` region or the 64 KiB
//! page-file sections `XMemMapPhysicalPages` maps into a placeholder, is host memory at an address, and is
//! imported into Vulkan with `VK_EXT_external_memory_host`. Two backings serve it:
//!
//! - **Direct**: the imported memory itself is bound into the window with sparse binding, so the GPU and the CPU
//!   share the bytes. Drivers whose sparse buffers accept host-visible memory (Intel, probably AMD) take this.
//! - **Mirrored**: NVIDIA's sparse buffers accept only device-local memory, so the window is bound to device-local
//!   memory and the imported host buffer is copied to it before the GPU reads (`record_uploads`) and back after
//!   the GPU writes (`record_downloads`). Writes the CPU made are found by `GetWriteWatch`, by the caller's
//!   `mark_dirty`, or by copying the whole region.

use std::collections::{BTreeMap, HashMap};
use std::ffi::{CStr, c_void};
use std::sync::{Arc, Mutex};

use ash::ext::external_memory_host;
use ash::vk;

/// The unit the console maps physical memory in, and the granule sparse binding works at.
pub const PAGE: u64 = 64 * 1024;

/// Device extensions the address space needs on top of Vulkan 1.3.
pub const DEVICE_EXTENSIONS: [&CStr; 1] = [external_memory_host::NAME];

/// Descriptor-table pointers are truncated to 32 bits by nearly every translated shader, so descriptor heaps must
/// live below this address, inside the window.
pub const LOW_LIMIT: u64 = 1 << 32;

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("Vulkan call failed: {0}")]
    Vulkan(#[from] vk::Result),
    #[error("the GPU lacks {0}")]
    Unsupported(&'static str),
    #[error("range {va:#x}+{len:#x} is not a multiple of {granule:#x} bytes")]
    Unaligned { va: u64, len: u64, granule: u64 },
    #[error(
        "addresses {lo:#x}..{hi:#x} need a window of {needed:#x} bytes, the GPU allows {max:#x}: \
         GPU-visible memory must be reserved closer together"
    )]
    WindowTooLarge {
        lo: u64,
        hi: u64,
        needed: u64,
        max: u64,
    },
    #[error("no memory type serves both {what} ({device:#x}) and imported host memory ({host:#x})")]
    NoMemoryType {
        what: &'static str,
        device: u32,
        host: u32,
    },
    #[error("the GPU allows {0} memory allocations and the game's mappings need more")]
    TooManyAllocations(u32),
}

/// What the GPU offers the address space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Support {
    pub sparse_buffers: bool,
    pub host_import: bool,
    pub buffer_address: bool,
    pub shader_int64: bool,
    /// `minImportedHostPointerAlignment`.
    pub host_alignment: u64,
    /// The largest window: the smaller of `maxBufferSize` and the sparse address space.
    pub max_window: u64,
    pub max_allocation: u64,
    pub max_allocation_count: u32,
}

impl Support {
    pub fn query(instance: &ash::Instance, physical: vk::PhysicalDevice) -> Support {
        // SAFETY: `physical` came from `instance`; the property and feature structs outlive the calls.
        unsafe {
            let extensions = instance
                .enumerate_device_extension_properties(physical)
                .unwrap_or_default();
            let host_import = extensions
                .iter()
                .any(|e| e.extension_name_as_c_str() == Ok(external_memory_host::NAME));
            let mut host = vk::PhysicalDeviceExternalMemoryHostPropertiesEXT::default();
            let mut v11 = vk::PhysicalDeviceVulkan11Properties::default();
            let mut v13 = vk::PhysicalDeviceVulkan13Properties::default();
            let mut properties = vk::PhysicalDeviceProperties2::default()
                .push_next(&mut v11)
                .push_next(&mut v13);
            if host_import {
                properties = properties.push_next(&mut host);
            }
            instance.get_physical_device_properties2(physical, &mut properties);
            let limits = properties.properties.limits;
            let mut v12 = vk::PhysicalDeviceVulkan12Features::default();
            let mut features = vk::PhysicalDeviceFeatures2::default().push_next(&mut v12);
            instance.get_physical_device_features2(physical, &mut features);
            let base = features.features;
            Support {
                sparse_buffers: base.sparse_binding == vk::TRUE
                    && base.sparse_residency_buffer == vk::TRUE,
                host_import,
                buffer_address: v12.buffer_device_address == vk::TRUE,
                shader_int64: base.shader_int64 == vk::TRUE,
                host_alignment: host.min_imported_host_pointer_alignment.max(1),
                max_window: v13.max_buffer_size.min(limits.sparse_address_space_size),
                max_allocation: v11.max_memory_allocation_size,
                max_allocation_count: limits.max_memory_allocation_count,
            }
        }
    }

    pub fn check(&self) -> Result<(), MemoryError> {
        let missing = [
            (self.sparse_buffers, "sparse buffer binding with residency"),
            (self.host_import, "VK_EXT_external_memory_host"),
            (self.buffer_address, "buffer device addresses"),
            (self.shader_int64, "64-bit shader integers"),
        ];
        match missing.into_iter().find(|(ok, _)| !ok) {
            Some((_, what)) => Err(MemoryError::Unsupported(what)),
            None => Ok(()),
        }
    }
}

/// The Vulkan objects the address space works through. The device must have been created with the features of
/// `Support::check` and `DEVICE_EXTENSIONS`, and `queue` must come from a family with sparse binding.
#[derive(Clone)]
pub struct Handles {
    pub instance: ash::Instance,
    pub physical: vk::PhysicalDevice,
    pub device: ash::Device,
    pub queue: vk::Queue,
    /// The family `queue` belongs to.
    pub queue_family: u32,
    /// Held while binding, so the queue can also be submitted to by the code that shares it.
    pub queue_lock: Arc<Mutex<()>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// The first address the window covers. Zero by default, so descriptor heaps below 4 GiB are inside it.
    pub base: u64,
    /// The window's size; `None` takes the largest the GPU allows.
    pub size: Option<u64>,
    /// The largest memory object made for one mapped run; longer runs are split.
    pub chunk: u64,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            base: 0,
            size: None,
            chunk: 1 << 30,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backing {
    Direct,
    Mirrored,
}

/// How the writes the CPU makes to mirrored memory are found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncPolicy {
    /// Only what `mark_dirty` named.
    Marked,
    /// All of it, every time.
    Always,
    /// Windows' write watch: the region was reserved with `MEM_WRITE_WATCH`. A region it cannot report on counts
    /// as written.
    WriteWatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Extent {
    len: u64,
    memory: u64,
    memory_offset: u64,
}

/// A piece taken out of the bound ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cut {
    va: u64,
    len: u64,
    memory: u64,
}

/// Which memory object backs each bound address range.
#[derive(Default)]
struct Extents(BTreeMap<u64, Extent>);

impl Extents {
    fn insert(&mut self, va: u64, extent: Extent) {
        self.0.insert(va, extent);
    }

    /// The extents touching `lo..hi`, each with the part of it inside.
    fn overlapping(&self, lo: u64, hi: u64) -> Vec<(u64, Extent, u64, u64)> {
        let start = self
            .0
            .range(..lo)
            .next_back()
            .filter(|(s, e)| **s + e.len > lo)
            .map(|(s, _)| *s)
            .unwrap_or(lo);
        self.0
            .range(start..hi)
            .map(|(s, e)| (*s, *e, (*s).max(lo), (*s + e.len).min(hi)))
            .collect()
    }

    /// Unbinds `va..va + len`, keeping what lies outside it.
    fn remove(&mut self, va: u64, len: u64) -> Vec<Cut> {
        let end = va + len;
        let mut cuts = Vec::new();
        for (s, e, cut_from, cut_to) in self.overlapping(va, end) {
            self.0.remove(&s);
            if s < cut_from {
                self.0.insert(
                    s,
                    Extent {
                        len: cut_from - s,
                        ..e
                    },
                );
            }
            if cut_to < s + e.len {
                self.0.insert(
                    cut_to,
                    Extent {
                        len: s + e.len - cut_to,
                        memory: e.memory,
                        memory_offset: e.memory_offset + (cut_to - s),
                    },
                );
            }
            cuts.push(Cut {
                va: cut_from,
                len: cut_to - cut_from,
                memory: e.memory,
            });
        }
        cuts
    }
}

/// Disjoint half-open address ranges.
#[derive(Default, Debug, PartialEq, Eq)]
struct RangeSet(BTreeMap<u64, u64>);

impl RangeSet {
    fn add(&mut self, lo: u64, hi: u64) {
        if lo >= hi {
            return;
        }
        let (mut lo, mut hi) = (lo, hi);
        if let Some((&s, &e)) = self.0.range(..=lo).next_back()
            && e >= lo
        {
            lo = s;
            hi = hi.max(e);
        }
        let inside: Vec<u64> = self.0.range(lo..=hi).map(|(s, _)| *s).collect();
        for s in inside {
            hi = hi.max(self.0.remove(&s).expect("listed above"));
        }
        self.0.insert(lo, hi);
    }

    fn remove(&mut self, lo: u64, hi: u64) {
        let hits: Vec<(u64, u64)> = self
            .0
            .iter()
            .filter(|(s, e)| **s < hi && **e > lo)
            .map(|(s, e)| (*s, *e))
            .collect();
        for (s, e) in hits {
            self.0.remove(&s);
            if s < lo {
                self.0.insert(s, lo);
            }
            if e > hi {
                self.0.insert(hi, e);
            }
        }
    }

    fn take(&mut self) -> Vec<(u64, u64)> {
        std::mem::take(&mut self.0).into_iter().collect()
    }
}

struct Window {
    base: u64,
    size: u64,
    buffer: vk::Buffer,
    address: u64,
    type_bits: u32,
}

/// The imported host memory of a mirrored run, as a buffer to copy from and to.
struct Mirror {
    buffer: vk::Buffer,
    import: vk::DeviceMemory,
}

struct Memory {
    /// The memory bound into the window: the imported host memory (direct) or device-local memory (mirrored).
    handle: vk::DeviceMemory,
    live: u64,
    mirror: Option<Mirror>,
    policy: SyncPolicy,
}

struct Bind {
    offset: u64,
    size: u64,
    memory: vk::DeviceMemory,
    memory_offset: u64,
}

/// The one address space translated shaders see, and what is bound into it.
pub struct AddressSpace {
    h: Handles,
    host: external_memory_host::Device,
    support: Support,
    config: Config,
    backing: Backing,
    types: vk::PhysicalDeviceMemoryProperties,
    window: Window,
    extents: Extents,
    memories: HashMap<u64, Memory>,
    next_memory: u64,
    dirty: RangeSet,
    pending: Vec<Bind>,
    batching: bool,
    retired_buffers: Vec<vk::Buffer>,
    retired_memory: Vec<vk::DeviceMemory>,
    generation: u64,
}

fn align_down(value: u64, to: u64) -> u64 {
    value / to * to
}

fn align_up(value: u64, to: u64) -> u64 {
    value.div_ceil(to) * to
}

const WINDOW_USAGE: vk::BufferUsageFlags = vk::BufferUsageFlags::from_raw(
    vk::BufferUsageFlags::TRANSFER_SRC.as_raw()
        | vk::BufferUsageFlags::TRANSFER_DST.as_raw()
        | vk::BufferUsageFlags::UNIFORM_BUFFER.as_raw()
        | vk::BufferUsageFlags::STORAGE_BUFFER.as_raw()
        | vk::BufferUsageFlags::INDEX_BUFFER.as_raw()
        | vk::BufferUsageFlags::VERTEX_BUFFER.as_raw()
        | vk::BufferUsageFlags::INDIRECT_BUFFER.as_raw()
        | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS.as_raw(),
);

impl AddressSpace {
    pub fn new(handles: Handles, config: Config) -> Result<AddressSpace, MemoryError> {
        let support = Support::query(&handles.instance, handles.physical);
        support.check()?;
        let size = config
            .size
            .unwrap_or(support.max_window)
            .min(support.max_window);
        let host = external_memory_host::Device::new(&handles.instance, &handles.device);
        let window = Self::make_window(&handles, config.base, size, false)?;
        // SAFETY: `physical` came from `instance`.
        let types = unsafe {
            handles
                .instance
                .get_physical_device_memory_properties(handles.physical)
        };
        let mut space = AddressSpace {
            h: handles,
            host,
            support,
            config,
            backing: Backing::Direct,
            types,
            window,
            extents: Extents::default(),
            memories: HashMap::new(),
            next_memory: 1,
            dirty: RangeSet::default(),
            pending: Vec::new(),
            batching: false,
            retired_buffers: Vec::new(),
            retired_memory: Vec::new(),
            generation: 0,
        };
        space.backing = space.choose_backing()?;
        if space.backing == Backing::Direct {
            let external = Self::make_window(&space.h, config.base, size, true)?;
            let host = space.probe_host_bits()?;
            if host & external.type_bits != 0 {
                let plain = std::mem::replace(&mut space.window, external);
                // SAFETY: never bound or used.
                unsafe { space.h.device.destroy_buffer(plain.buffer, None) };
            } else {
                // SAFETY: never bound or used.
                unsafe { space.h.device.destroy_buffer(external.buffer, None) };
                space.backing = Backing::Mirrored;
            }
        }
        Ok(space)
    }

    /// The memory types imported host memory can have, asked of a probe allocation.
    fn probe_host_bits(&self) -> Result<u32, MemoryError> {
        let layout = std::alloc::Layout::from_size_align(PAGE as usize, PAGE as usize)
            .expect("a valid layout");
        // SAFETY: a probe allocation only handed to the driver to ask its memory types, then freed.
        unsafe {
            let probe = std::alloc::alloc(layout);
            if probe.is_null() {
                return Ok(0);
            }
            let bits = self.host_type_bits(probe as u64);
            std::alloc::dealloc(probe, layout);
            bits
        }
    }

    /// Direct when a sparse buffer accepts the memory types imported host memory has.
    fn choose_backing(&self) -> Result<Backing, MemoryError> {
        Ok(if self.probe_host_bits()? & self.window.type_bits != 0 {
            Backing::Direct
        } else {
            Backing::Mirrored
        })
    }

    fn host_type_bits(&self, va: u64) -> Result<u32, MemoryError> {
        let mut host = vk::MemoryHostPointerPropertiesEXT::default();
        // SAFETY: only reads the pointer's page properties.
        unsafe {
            (self.host.fp().get_memory_host_pointer_properties_ext)(
                self.host.device(),
                vk::ExternalMemoryHandleTypeFlags::HOST_ALLOCATION_EXT,
                va as usize as *mut c_void,
                &mut host,
            )
            .result()?;
        }
        Ok(host.memory_type_bits)
    }

    /// `external` marks the buffer as able to take imported host memory, which a direct window needs.
    fn make_window(
        h: &Handles,
        base: u64,
        size: u64,
        external: bool,
    ) -> Result<Window, MemoryError> {
        let mut host = vk::ExternalMemoryBufferCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::HOST_ALLOCATION_EXT);
        let mut info = vk::BufferCreateInfo::default()
            .flags(vk::BufferCreateFlags::SPARSE_BINDING | vk::BufferCreateFlags::SPARSE_RESIDENCY)
            .size(size)
            .usage(WINDOW_USAGE)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        if external {
            info = info.push_next(&mut host);
        }
        // SAFETY: a fresh sparse buffer, destroyed by `AddressSpace::drop` or `reclaim`.
        unsafe {
            let buffer = h.device.create_buffer(&info, None)?;
            let needs = h.device.get_buffer_memory_requirements(buffer);
            if needs.alignment > PAGE {
                h.device.destroy_buffer(buffer, None);
                return Err(MemoryError::Unsupported(
                    "sparse blocks of 64 KiB or smaller",
                ));
            }
            let address = h
                .device
                .get_buffer_device_address(&vk::BufferDeviceAddressInfo::default().buffer(buffer));
            Ok(Window {
                base,
                size,
                buffer,
                address,
                type_bits: needs.memory_type_bits,
            })
        }
    }

    pub fn handles(&self) -> &Handles {
        &self.h
    }

    pub fn support(&self) -> &Support {
        &self.support
    }

    pub fn backing(&self) -> Backing {
        self.backing
    }

    /// The value for the shaders' push constants (`PUSH_HEAP`, two words, low first): adding a game address to it
    /// gives the device address of the same byte. It changes only when the window is rebuilt (`generation`).
    pub fn heap_base(&self) -> u64 {
        self.window.address.wrapping_sub(self.window.base)
    }

    pub fn heap_base_words(&self) -> [u32; 2] {
        let base = self.heap_base();
        [base as u32, (base >> 32) as u32]
    }

    /// The device address of the byte at game address `va`.
    pub fn device_address(&self, va: u64) -> u64 {
        self.heap_base().wrapping_add(va)
    }

    /// The window as a buffer and an offset, for commands that take one (vertex, index, indirect, copies).
    pub fn buffer_at(&self, va: u64) -> (vk::Buffer, u64) {
        (self.window.buffer, va.wrapping_sub(self.window.base))
    }

    pub fn buffer(&self) -> vk::Buffer {
        self.window.buffer
    }

    /// The first address and the length the window covers. GPU-visible memory must be placed inside it.
    pub fn window(&self) -> (u64, u64) {
        (self.window.base, self.window.size)
    }

    /// Grows by one each time the window is rebuilt, which changes `heap_base` and the buffer.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether anything is bound at `va`.
    pub fn is_bound(&self, va: u64) -> bool {
        !self.extents.overlapping(va, va + 1).is_empty()
    }

    pub fn memory_objects(&self) -> usize {
        self.memories.len()
    }

    fn check_aligned(&self, va: u64, len: u64) -> Result<(), MemoryError> {
        let granule = PAGE.max(self.support.host_alignment);
        if len == 0 || !va.is_multiple_of(granule) || !len.is_multiple_of(granule) {
            return Err(MemoryError::Unaligned { va, len, granule });
        }
        Ok(())
    }

    /// Makes sure `lo..hi` is inside the window, rebuilding a larger one (and rebinding everything) if not.
    pub fn ensure(&mut self, lo: u64, hi: u64) -> Result<(), MemoryError> {
        let (base, size) = self.window();
        if lo >= base && hi <= base + size {
            return Ok(());
        }
        self.commit()?;
        let new_base = align_down(lo.min(base), PAGE);
        let new_end = align_up(hi.max(base + size), 1 << 30);
        let needed = new_end - new_base;
        if needed > self.support.max_window {
            return Err(MemoryError::WindowTooLarge {
                lo,
                hi,
                needed,
                max: self.support.max_window,
            });
        }
        let window = Self::make_window(&self.h, new_base, needed, self.backing == Backing::Direct)?;
        let old = std::mem::replace(&mut self.window, window);
        self.retired_buffers.push(old.buffer);
        self.generation += 1;
        self.pending = self
            .extents
            .0
            .iter()
            .map(|(va, e)| Bind {
                offset: va - new_base,
                size: e.len,
                memory: self.memories[&e.memory].handle,
                memory_offset: e.memory_offset,
            })
            .collect();
        self.commit()?;
        if self.backing == Backing::Mirrored {
            for (va, e) in &self.extents.0 {
                self.dirty.add(*va, *va + e.len);
            }
        }
        Ok(())
    }

    /// Binds the host memory at `va..va + len`, which must stay mapped at that address until `unmap`, into the
    /// window at the same address. A committed region of `XMemVirtualAlloc`, or a run of pages
    /// `XMemMapPhysicalPages` mapped; anything bound there before is replaced. `policy` matters when mirrored.
    pub fn map_host(&mut self, va: u64, len: u64, policy: SyncPolicy) -> Result<(), MemoryError> {
        self.map_host_at(va, va, len, policy)
    }

    /// Like `map_host`, for host memory at `host` that the GPU sees at `va`: a reserved resource's tiles are
    /// mapped this way. Write watch only follows memory that is at its own address, so an aliased run is copied
    /// whole each time instead.
    pub fn map_host_at(
        &mut self,
        va: u64,
        host: u64,
        len: u64,
        policy: SyncPolicy,
    ) -> Result<(), MemoryError> {
        self.check_aligned(va, len)?;
        self.check_aligned(host, len)?;
        self.ensure(va, va + len)?;
        self.unmap_range(va, len);
        let policy = if host != va && policy == SyncPolicy::WriteWatch {
            SyncPolicy::Always
        } else {
            policy
        };
        let chunk = self.config.chunk.min(self.support.max_allocation);
        let chunk = align_down(chunk, PAGE.max(self.support.host_alignment)).max(PAGE);
        let count = len.div_ceil(chunk);
        if (self.memories.len() as u64 + count) > u64::from(self.support.max_allocation_count) {
            return Err(MemoryError::TooManyAllocations(
                self.support.max_allocation_count,
            ));
        }
        let mut done = 0;
        while done < len {
            let part = chunk.min(len - done);
            self.import(va + done, host + done, part, policy)?;
            done += part;
        }
        if self.backing == Backing::Mirrored {
            self.dirty.add(va, va + len);
        }
        self.finish()
    }

    /// `XMemMapPhysicalPages`: `count` pages, now mapped at `va`.
    pub fn map_pages(
        &mut self,
        va: u64,
        count: u64,
        policy: SyncPolicy,
    ) -> Result<(), MemoryError> {
        self.map_host(va, count * PAGE, policy)
    }

    /// `XMemMapPhysicalPagesScatter`: one page at each address; neighbours are imported together.
    pub fn map_pages_scatter(
        &mut self,
        addresses: &[u64],
        policy: SyncPolicy,
    ) -> Result<(), MemoryError> {
        let mut sorted = addresses.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        let was = std::mem::replace(&mut self.batching, true);
        let mut result = Ok(());
        let mut run = 0;
        while run < sorted.len() && result.is_ok() {
            let mut end = run + 1;
            while end < sorted.len() && sorted[end] == sorted[end - 1] + PAGE {
                end += 1;
            }
            result = self.map_host(sorted[run], (end - run) as u64 * PAGE, policy);
            run = end;
        }
        self.batching = was;
        result?;
        self.finish()
    }

    fn pick_type(
        &self,
        bits: u32,
        preferred: vk::MemoryPropertyFlags,
        avoid: vk::MemoryPropertyFlags,
    ) -> u32 {
        let flags = |i: u32| self.types.memory_types[i as usize].property_flags;
        (0..self.types.memory_type_count)
            .filter(|i| bits & (1 << i) != 0)
            .find(|i| flags(*i).contains(preferred) && !flags(*i).intersects(avoid))
            .or_else(|| (0..self.types.memory_type_count).find(|i| bits & (1 << i) != 0))
            .unwrap_or(0)
    }

    /// Imports `va..va + len` as device memory.
    ///
    /// # Safety
    /// The host memory stays mapped at `va` until the memory is freed.
    unsafe fn import_memory(
        &self,
        va: u64,
        len: u64,
        device_address: bool,
    ) -> Result<(vk::DeviceMemory, u32), MemoryError> {
        let bits = self.host_type_bits(va)?;
        let mut import = vk::ImportMemoryHostPointerInfoEXT::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::HOST_ALLOCATION_EXT)
            .host_pointer(va as usize as *mut c_void);
        let mut flags =
            vk::MemoryAllocateFlagsInfo::default().flags(vk::MemoryAllocateFlags::DEVICE_ADDRESS);
        let kind = self.pick_type(
            bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE,
            vk::MemoryPropertyFlags::empty(),
        );
        let mut info = vk::MemoryAllocateInfo::default()
            .allocation_size(len)
            .memory_type_index(kind)
            .push_next(&mut import);
        if device_address {
            info = info.push_next(&mut flags);
        }
        // SAFETY: as the caller promises.
        let memory = unsafe { self.h.device.allocate_memory(&info, None)? };
        Ok((memory, bits))
    }

    fn import(
        &mut self,
        va: u64,
        host: u64,
        len: u64,
        policy: SyncPolicy,
    ) -> Result<(), MemoryError> {
        let (handle, mirror) = match self.backing {
            Backing::Direct => {
                // SAFETY: the caller of `map_host` keeps the memory mapped until `unmap`.
                let (memory, bits) = unsafe { self.import_memory(host, len, true)? };
                if bits & self.window.type_bits == 0 {
                    // SAFETY: just made, never bound.
                    unsafe { self.h.device.free_memory(memory, None) };
                    return Err(MemoryError::NoMemoryType {
                        what: "the window",
                        device: self.window.type_bits,
                        host: bits,
                    });
                }
                (memory, None)
            }
            Backing::Mirrored => self.import_mirrored(host, len)?,
        };
        let id = self.next_memory;
        self.next_memory += 1;
        self.memories.insert(
            id,
            Memory {
                handle,
                live: len,
                mirror,
                policy,
            },
        );
        self.extents.insert(
            va,
            Extent {
                len,
                memory: id,
                memory_offset: 0,
            },
        );
        self.pending.push(Bind {
            offset: va - self.window.base,
            size: len,
            memory: handle,
            memory_offset: 0,
        });
        Ok(())
    }

    fn import_mirrored(
        &self,
        va: u64,
        len: u64,
    ) -> Result<(vk::DeviceMemory, Option<Mirror>), MemoryError> {
        let device = &self.h.device;
        // SAFETY: fresh objects; on failure each one made so far is destroyed. The host memory stays mapped, as
        // `map_host` requires.
        unsafe {
            let mut host = vk::ExternalMemoryBufferCreateInfo::default()
                .handle_types(vk::ExternalMemoryHandleTypeFlags::HOST_ALLOCATION_EXT);
            let buffer = device.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(len)
                    .usage(vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::TRANSFER_DST)
                    .push_next(&mut host),
                None,
            )?;
            let needs = device.get_buffer_memory_requirements(buffer);
            let (import, bits) = match self.import_memory(va, len, false) {
                Ok(made) => made,
                Err(e) => {
                    device.destroy_buffer(buffer, None);
                    return Err(e);
                }
            };
            if needs.memory_type_bits & bits == 0 {
                device.destroy_buffer(buffer, None);
                device.free_memory(import, None);
                return Err(MemoryError::NoMemoryType {
                    what: "a copy buffer",
                    device: needs.memory_type_bits,
                    host: bits,
                });
            }
            if let Err(e) = device.bind_buffer_memory(buffer, import, 0) {
                device.destroy_buffer(buffer, None);
                device.free_memory(import, None);
                return Err(e.into());
            }
            let kind = self.pick_type(
                self.window.type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
                vk::MemoryPropertyFlags::HOST_VISIBLE,
            );
            let mut flags = vk::MemoryAllocateFlagsInfo::default()
                .flags(vk::MemoryAllocateFlags::DEVICE_ADDRESS);
            let local = device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(len)
                    .memory_type_index(kind)
                    .push_next(&mut flags),
                None,
            );
            match local {
                Ok(local) => Ok((local, Some(Mirror { buffer, import }))),
                Err(e) => {
                    device.destroy_buffer(buffer, None);
                    device.free_memory(import, None);
                    Err(e.into())
                }
            }
        }
    }

    /// Unbinds `va..va + len` (`XMemMapPhysicalPages` with no pages, `VirtualFree`). Call it before the host memory
    /// is unmapped or freed. The memory objects are freed by `reclaim`.
    pub fn unmap(&mut self, va: u64, len: u64) -> Result<(), MemoryError> {
        self.check_aligned(va, len)?;
        self.unmap_range(va, len);
        self.finish()
    }

    fn unmap_range(&mut self, va: u64, len: u64) {
        self.dirty.remove(va, va + len);
        for cut in self.extents.remove(va, len) {
            self.pending.push(Bind {
                offset: cut.va - self.window.base,
                size: cut.len,
                memory: vk::DeviceMemory::null(),
                memory_offset: 0,
            });
            if let Some(memory) = self.memories.get_mut(&cut.memory) {
                memory.live -= cut.len;
                if memory.live == 0 {
                    let memory = self.memories.remove(&cut.memory).expect("found above");
                    self.retired_memory.push(memory.handle);
                    if let Some(mirror) = memory.mirror {
                        self.retired_buffers.push(mirror.buffer);
                        self.retired_memory.push(mirror.import);
                    }
                }
            }
        }
    }

    /// Several mapping calls become one queue submission between `begin_batch` and `end_batch`.
    pub fn begin_batch(&mut self) {
        self.batching = true;
    }

    pub fn end_batch(&mut self) -> Result<(), MemoryError> {
        self.batching = false;
        self.commit()
    }

    fn finish(&mut self) -> Result<(), MemoryError> {
        if self.batching { Ok(()) } else { self.commit() }
    }

    /// Submits the pending binds and waits for them.
    pub fn commit(&mut self) -> Result<(), MemoryError> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let binds: Vec<vk::SparseMemoryBind> = self
            .pending
            .drain(..)
            .map(|b| {
                vk::SparseMemoryBind::default()
                    .resource_offset(b.offset)
                    .size(b.size)
                    .memory(b.memory)
                    .memory_offset(b.memory_offset)
            })
            .collect();
        let buffer_binds = [vk::SparseBufferMemoryBindInfo::default()
            .buffer(self.window.buffer)
            .binds(&binds)];
        let info = [vk::BindSparseInfo::default().buffer_binds(&buffer_binds)];
        let device = &self.h.device;
        // SAFETY: the queue is used under its lock; the fence is waited on and destroyed here.
        unsafe {
            let fence = device.create_fence(&vk::FenceCreateInfo::default(), None)?;
            let submitted = {
                let _guard = self.h.queue_lock.lock().unwrap_or_else(|e| e.into_inner());
                device.queue_bind_sparse(self.h.queue, &info, fence)
            };
            let waited = submitted.and_then(|()| device.wait_for_fences(&[fence], true, u64::MAX));
            device.destroy_fence(fence, None);
            waited?;
        }
        Ok(())
    }

    /// Tells a mirrored window the CPU wrote `va..va + len`.
    pub fn mark_dirty(&mut self, va: u64, len: u64) {
        if self.backing == Backing::Mirrored {
            self.dirty.add(va, va + len);
        }
    }

    /// Copies to the window what the CPU wrote since the last call, so the commands recorded after these see it.
    /// Nothing is recorded when the window is direct. Returns the bytes copied.
    pub fn record_uploads(&mut self, command: vk::CommandBuffer) -> u64 {
        if self.backing == Backing::Direct {
            return 0;
        }
        let by_policy = |policy: SyncPolicy| -> Vec<(u64, u64)> {
            self.extents
                .0
                .iter()
                .filter(|(_, e)| self.memories[&e.memory].policy == policy)
                .map(|(va, e)| (*va, *va + e.len))
                .collect()
        };
        for (lo, hi) in by_policy(SyncPolicy::WriteWatch) {
            match write_watch(lo, hi - lo) {
                Some(pages) => {
                    for page in pages {
                        self.dirty.add(page, page + WATCH_PAGE);
                    }
                }
                None => self.dirty.add(lo, hi),
            }
        }
        for (lo, hi) in by_policy(SyncPolicy::Always) {
            self.dirty.add(lo, hi);
        }
        let wanted = self.dirty.take();
        let bytes = self.record_copies(command, &wanted, true);
        if bytes > 0 {
            self.barrier(
                command,
                vk::AccessFlags::TRANSFER_WRITE,
                vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::ALL_COMMANDS,
            );
        }
        bytes
    }

    /// Copies what the GPU wrote in `va..va + len` back to the host memory. The CPU may read it once the commands
    /// have finished. Nothing is recorded when the window is direct. Returns the bytes copied.
    pub fn record_downloads(&mut self, command: vk::CommandBuffer, va: u64, len: u64) -> u64 {
        if self.backing == Backing::Direct {
            return 0;
        }
        self.barrier(
            command,
            vk::AccessFlags::MEMORY_WRITE,
            vk::AccessFlags::TRANSFER_READ,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::PipelineStageFlags::TRANSFER,
        );
        let bytes = self.record_copies(command, &[(va, va + len)], false);
        self.barrier(
            command,
            vk::AccessFlags::TRANSFER_WRITE,
            vk::AccessFlags::HOST_READ,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::HOST,
        );
        bytes
    }

    fn barrier(
        &self,
        command: vk::CommandBuffer,
        from: vk::AccessFlags,
        to: vk::AccessFlags,
        src: vk::PipelineStageFlags,
        dst: vk::PipelineStageFlags,
    ) {
        let barrier = [vk::MemoryBarrier::default()
            .src_access_mask(from)
            .dst_access_mask(to)];
        // SAFETY: the command buffer is in the recording state, as the caller promises.
        unsafe {
            self.h.device.cmd_pipeline_barrier(
                command,
                src,
                dst,
                vk::DependencyFlags::empty(),
                &barrier,
                &[],
                &[],
            );
        }
    }

    fn record_copies(
        &self,
        command: vk::CommandBuffer,
        ranges: &[(u64, u64)],
        upload: bool,
    ) -> u64 {
        let mut by_buffer: Vec<(vk::Buffer, Vec<vk::BufferCopy>)> = Vec::new();
        let mut bytes = 0;
        for &(lo, hi) in ranges {
            for (start, extent, from, to) in self.extents.overlapping(lo, hi) {
                let Some(mirror) = self
                    .memories
                    .get(&extent.memory)
                    .and_then(|m| m.mirror.as_ref())
                else {
                    continue;
                };
                let host_offset = extent.memory_offset + (from - start);
                let window_offset = from - self.window.base;
                let (src, dst) = if upload {
                    (host_offset, window_offset)
                } else {
                    (window_offset, host_offset)
                };
                let copy = vk::BufferCopy::default()
                    .src_offset(src)
                    .dst_offset(dst)
                    .size(to - from);
                bytes += to - from;
                match by_buffer.iter_mut().find(|(b, _)| *b == mirror.buffer) {
                    Some((_, copies)) => copies.push(copy),
                    None => by_buffer.push((mirror.buffer, vec![copy])),
                }
            }
        }
        for (host, copies) in by_buffer {
            let (src, dst) = if upload {
                (host, self.window.buffer)
            } else {
                (self.window.buffer, host)
            };
            // SAFETY: the command buffer is recording; both buffers are alive and the offsets are in range.
            unsafe { self.h.device.cmd_copy_buffer(command, src, dst, &copies) };
        }
        bytes
    }

    /// Frees what unbinding and rebuilding retired. Call it when the GPU is done with all work submitted before
    /// the unmapping, for instance after a frame's fence.
    pub fn reclaim(&mut self) {
        // SAFETY: the caller promises the GPU no longer uses the retired objects.
        unsafe {
            for buffer in self.retired_buffers.drain(..) {
                self.h.device.destroy_buffer(buffer, None);
            }
            for memory in self.retired_memory.drain(..) {
                self.h.device.free_memory(memory, None);
            }
        }
    }
}

impl Drop for AddressSpace {
    fn drop(&mut self) {
        let _ = self.commit();
        // SAFETY: the owner drops the address space only once the GPU is idle.
        unsafe {
            let _ = self.h.device.device_wait_idle();
            self.h.device.destroy_buffer(self.window.buffer, None);
            for memory in self.memories.drain().map(|(_, m)| m) {
                self.h.device.free_memory(memory.handle, None);
                if let Some(mirror) = memory.mirror {
                    self.h.device.destroy_buffer(mirror.buffer, None);
                    self.h.device.free_memory(mirror.import, None);
                }
            }
        }
        self.reclaim();
    }
}

const WATCH_PAGE: u64 = 4096;

/// `WRITE_WATCH_FLAG_RESET`, which windows-sys does not carry.
#[cfg(windows)]
const WRITE_WATCH_FLAG_RESET: u32 = 1;

/// The pages written since the last call, for a region reserved with `MEM_WRITE_WATCH`; `None` where the system
/// cannot say.
#[cfg(windows)]
fn write_watch(va: u64, len: u64) -> Option<Vec<u64>> {
    use windows_sys::Win32::System::Memory::GetWriteWatch;
    let mut addresses = vec![std::ptr::null_mut::<c_void>(); (len / WATCH_PAGE) as usize];
    let mut count = addresses.len();
    let mut granularity = 0u32;
    // SAFETY: `addresses` has room for `count` entries; the call only reads the system's write-watch table.
    let status = unsafe {
        GetWriteWatch(
            WRITE_WATCH_FLAG_RESET,
            va as usize as *const c_void,
            len as usize,
            addresses.as_mut_ptr(),
            &mut count,
            &mut granularity,
        )
    };
    (status == 0).then(|| addresses[..count].iter().map(|a| *a as u64).collect())
}

#[cfg(not(windows))]
fn write_watch(_va: u64, _len: u64) -> Option<Vec<u64>> {
    None
}

#[cfg(test)]
mod bench;
#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;
