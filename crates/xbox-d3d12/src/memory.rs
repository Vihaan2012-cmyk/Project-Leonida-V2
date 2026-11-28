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
//! - **Mirrored**: NVIDIA's sparse buffers list only device-local memory, so the window is bound to device-local
//!   memory and the imported host buffer is copied to it before the GPU reads (`record_uploads`) and back after
//!   the GPU writes (`record_downloads`). Writes the CPU made are found by `GetWriteWatch`, by the caller's
//!   `mark_dirty`, or by copying the whole region. The `sync` module says how uploads, the transfer queue
//!   (`submit_uploads`) and the copies back (`note_gpu_write`, `HostAccess`, `read_back`) keep neither side stale.
//!
//! # The spill tier
//!
//! A mirrored window holds a second copy of every mapped run in device-local memory, and the title maps about
//! 7.1 GiB of GPU memory at boot (the executable's pools: 6236 MiB, 710 MiB and about 185 MiB of descriptor
//! heaps), more than a card with 8 GB can give. So a run is mirrored only while the mirror's share of the budget
//! (`Budget::mirror_room`) has room, and is otherwise **spilled**: the imported host memory is bound into the
//! window itself, as a direct window does, so the GPU reads and writes the title's own bytes over PCIe and nothing
//! is copied. The driver's memory requirements for a sparse buffer do not list that memory, but the RTX 4060's
//! driver (616.64) binds it and reads and writes it correctly (`memory::probe`, run by hand); a driver that does
//! not is found out at start by a self-test, and `AddressSpace::set_spill` (or `LEONIDA_VRAM_SPILL=0`) turns the
//! tier off. Without it a run past the budget is tried in device-local memory at a low priority and failing that
//! left unbound.
//!
//! Measured on the RTX 4060 (`memory::probe`, 1 GiB blocks of device-local memory bound into a sparse buffer, with
//! other programs holding about 4 GiB of the card): the driver never refused one up to 10 GiB and the data stayed
//! correct, but it places each allocation where room is at that moment and moves none later, whatever the
//! priorities (set at allocation, raised to 1.0 on a late block, or lowered with `vkSetDeviceMemoryPriorityEXT`
//! on early ones): blocks that did not fit were in system memory from the start, read at 4 to 6 GB/s against 45 to
//! 55 GB/s for the ones in VRAM. So where memory lands is decided here, by order and by the mirror's share, not left
//! to the driver. A spilled run reads at the same 5 GB/s.
//!
//! Runs below 4 GiB are never spilled: descriptor heaps must be there (`LOW_LIMIT`) and every shader reads them.
//!
//! `promote_spilled` mirrors spilled runs again as device-local memory frees, hot runs first. The textures are the
//! other half of it: the title's tiled texture data sits in the window, and the layer detiles it on the CPU into a
//! `VkImage` the shaders sample, so once that is done the window's copy of the texture's memory is dead weight
//! (the `cold` module). The resource layer says which ranges are texture data (`mark_cold`) and which are read or
//! written through the window (`mark_hot`: buffers, targets, views over addresses, GPU writes), and between
//! submissions `release_cold` splits the memory objects under cold islands into pieces of their own
//! (`spill_range`), spilled and pinned, and mirrors what lay around them again. A released range is the title's
//! own host memory bound into the window, so a write by the CPU needs no copy there; `take_cold_writes` tells the
//! resource layer which images to make again. Counters: `Counters::released`, `WindowClasses`.

use std::collections::{BTreeMap, HashMap};
use std::ffi::{CStr, c_void};
use std::sync::{Arc, Mutex};

use ash::ext::external_memory_host;
use ash::vk;

mod budget;
mod cold;
mod dirty;
mod spill;
mod sync;
mod transfer;

pub use budget::{Budget, Class, Counters, Grant, MemoryExtensions, Placement, Request};
pub use cold::{Split, WindowClasses};
use dirty::Tiles;
pub use sync::HostAccess;
pub use transfer::{TimelineWait, TransferQueue, pick_transfer_family};

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
    /// Timeline semaphores, which a transfer queue's uploads are ordered with.
    pub timeline_semaphores: bool,
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
                timeline_semaphores: v12.timeline_semaphore == vk::TRUE,
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
    /// The device-local budget every allocation of the layer goes through.
    pub budget: Arc<Budget>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// The first address the window covers. Zero by default, so descriptor heaps below 4 GiB are inside it.
    pub base: u64,
    /// The window's size; `None` takes the largest the GPU allows.
    pub size: Option<u64>,
    /// The largest memory object made for one mapped run; longer runs are split. Each is mirrored or spilled as a
    /// whole, so the smaller it is the more exactly the mirror fills its share of the budget.
    pub chunk: u64,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            base: 0,
            size: None,
            chunk: 256 << 20,
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
    /// Where the imported host memory is. It equals the window address except for runs seen at another address.
    host: u64,
    tiles: Tiles,
    /// The device-local memory of a mirrored run, as the budget granted it; for a spilled run, the record that
    /// counts it as system memory.
    grant: Option<Grant>,
    /// Moved to system memory by `spill_range`: `promote_spilled` leaves it there.
    pinned: bool,
}

/// A mapped run no device-local memory could back: the host memory is the game's and intact, the window has
/// nothing bound there, and `retry_deferred` binds it when memory is free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Deferred {
    len: u64,
    host: u64,
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
    sync: sync::SyncState,
    pending: Vec<Bind>,
    batching: bool,
    retired_buffers: Vec<vk::Buffer>,
    retired_memory: Vec<vk::DeviceMemory>,
    retired_grants: Vec<Grant>,
    deferred: BTreeMap<u64, Deferred>,
    /// Runs past the mirror's share of the budget go to system memory bound straight into the window.
    spill: bool,
    lenient: bool,
    reclaims: u32,
    /// `promote_spilled` stopped at its per-call limit with runs still waiting: the next `reclaim` goes on.
    promote_more: bool,
    /// What is known of how the GPU uses the game's memory: see `cold`.
    temp: cold::Temperature,
    generation: u64,
    transfer: Option<transfer::Transfer>,
    /// The families the window and the copy buffers are shared by: more than one when a transfer queue of another
    /// family is in use.
    families: Vec<u32>,
}

fn sharing(families: &[u32]) -> vk::SharingMode {
    if families.len() > 1 {
        vk::SharingMode::CONCURRENT
    } else {
        vk::SharingMode::EXCLUSIVE
    }
}

fn families_for(families: &[u32]) -> &[u32] {
    if families.len() > 1 { families } else { &[] }
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
        Self::new_with_transfer(handles, config, None)
    }

    /// Like `new`, with a queue of its own for copying uploads (`submit_uploads`): when it is a family other
    /// than `handles.queue_family`, the window is shared between the two. See `pick_transfer_family` and
    /// `Support::timeline_semaphores` for what the device needs.
    pub fn new_with_transfer(
        handles: Handles,
        config: Config,
        transfer: Option<TransferQueue>,
    ) -> Result<AddressSpace, MemoryError> {
        let support = Support::query(&handles.instance, handles.physical);
        support.check()?;
        if transfer.is_some() && !support.timeline_semaphores {
            return Err(MemoryError::Unsupported("timeline semaphores"));
        }
        let mut families = vec![handles.queue_family];
        if let Some(t) = &transfer
            && t.family != handles.queue_family
        {
            families.push(t.family);
        }
        let size = config
            .size
            .unwrap_or(support.max_window)
            .min(support.max_window);
        let host = external_memory_host::Device::new(&handles.instance, &handles.device);
        let window = Self::make_window(&handles, config.base, size, false, &families)?;
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
            sync: sync::SyncState::default(),
            pending: Vec::new(),
            batching: false,
            retired_buffers: Vec::new(),
            retired_memory: Vec::new(),
            retired_grants: Vec::new(),
            deferred: BTreeMap::new(),
            spill: false,
            lenient: false,
            reclaims: 0,
            promote_more: false,
            temp: cold::Temperature::default(),
            generation: 0,
            transfer: None,
            families,
        };
        space.backing = space.choose_backing()?;
        if space.backing == Backing::Direct {
            let external = Self::make_window(&space.h, config.base, size, true, &space.families)?;
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
        if let Some(queue) = transfer
            && space.backing == Backing::Mirrored
        {
            space.transfer = Some(transfer::Transfer::new(&space.h.device, queue)?);
        }
        if space.backing == Backing::Mirrored
            && std::env::var("LEONIDA_VRAM_SPILL").as_deref() != Ok("0")
        {
            space.spill = space.spill_works();
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
        families: &[u32],
    ) -> Result<Window, MemoryError> {
        let mut host = vk::ExternalMemoryBufferCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::HOST_ALLOCATION_EXT);
        let mut info = vk::BufferCreateInfo::default()
            .flags(vk::BufferCreateFlags::SPARSE_BINDING | vk::BufferCreateFlags::SPARSE_RESIDENCY)
            .size(size)
            .usage(WINDOW_USAGE)
            .sharing_mode(sharing(families))
            .queue_family_indices(families_for(families));
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
        let window = Self::make_window(
            &self.h,
            new_base,
            needed,
            self.backing == Backing::Direct,
            &self.families,
        )?;
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
        self.commit()
    }

    /// Binds the host memory at `va..va + len`, which must stay mapped at that address until `unmap`, into the
    /// window at the same address. A committed region of `XMemVirtualAlloc`, or a run of pages
    /// `XMemMapPhysicalPages` mapped; anything bound there before is replaced. `policy` matters when mirrored.
    pub fn map_host(&mut self, va: u64, len: u64, policy: SyncPolicy) -> Result<(), MemoryError> {
        self.map_host_at(va, va, len, policy)
    }

    /// Like `map_host`, for host memory at `host` that the GPU sees at `va`: a reserved resource's tiles are
    /// mapped this way. Write watch follows the host memory, wherever the GPU sees it.
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
            match self.import(va + done, host + done, part, policy) {
                Ok(()) => {}
                Err(MemoryError::Vulkan(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY))
                    if self.lenient && self.backing == Backing::Mirrored =>
                {
                    self.defer_run(va + done, host + done, part, policy);
                }
                Err(e) => return Err(e),
            }
            done += part;
        }
        if self.backing == Backing::Mirrored {
            self.sync.dirty.add(va, va + len);
        }
        self.finish()
    }

    /// Runs `body`, in which a mirrored run that no device-local memory can back is left unbound
    /// (`deferred_bytes`) and bound by `retry_deferred` once memory is free, rather than failing the mapping. For
    /// the game's own memory: it asked for an allocation the console would have given it, and the GPU reads zeros
    /// from the run meanwhile.
    pub fn lenient<R>(&mut self, body: impl FnOnce(&mut AddressSpace) -> R) -> R {
        let was = std::mem::replace(&mut self.lenient, true);
        let result = body(self);
        self.lenient = was;
        result
    }

    fn defer_run(&mut self, va: u64, host: u64, len: u64, policy: SyncPolicy) {
        self.deferred.insert(va, Deferred { len, host, policy });
        self.h.budget.defer(len as i64);
        self.h.budget.note_unbound(va, len);
    }

    /// The bytes of the game's memory left unbound for want of device-local memory.
    pub fn deferred_bytes(&self) -> u64 {
        self.deferred.values().map(|d| d.len).sum()
    }

    /// Forgets what is deferred inside `va..va + len`, keeping the rest of each run.
    fn forget_deferred(&mut self, va: u64, len: u64) {
        let end = va + len;
        let hit: Vec<u64> = self
            .deferred
            .range(..end)
            .filter(|(s, d)| **s + d.len > va)
            .map(|(s, _)| *s)
            .collect();
        for start in hit {
            let run = self.deferred.remove(&start).expect("found above");
            let cut_from = start.max(va);
            let cut_to = (start + run.len).min(end);
            self.h.budget.defer(-((cut_to - cut_from) as i64));
            if start < cut_from {
                self.deferred.insert(
                    start,
                    Deferred {
                        len: cut_from - start,
                        ..run
                    },
                );
            }
            if cut_to < start + run.len {
                self.deferred.insert(
                    cut_to,
                    Deferred {
                        len: start + run.len - cut_to,
                        host: run.host + (cut_to - start),
                        ..run
                    },
                );
            }
        }
    }

    /// Tries again to back the runs that were deferred, in address order, until one is refused. Returns the
    /// bytes bound now.
    pub fn retry_deferred(&mut self) -> Result<u64, MemoryError> {
        if self.deferred.is_empty() {
            return Ok(0);
        }
        let runs: Vec<(u64, Deferred)> = self.deferred.iter().map(|(k, v)| (*k, *v)).collect();
        let mut bound = 0;
        let mut result = Ok(());
        for (va, run) in runs {
            match self.import(va, run.host, run.len, run.policy) {
                Ok(()) => {
                    self.deferred.remove(&va);
                    self.h.budget.defer(-(run.len as i64));
                    self.sync.dirty.add(va, va + run.len);
                    bound += run.len;
                }
                Err(MemoryError::Vulkan(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY)) => break,
                Err(e) => {
                    result = Err(e);
                    break;
                }
            }
        }
        let committed = self.commit();
        result?;
        committed?;
        Ok(bound)
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
        preferred: vk::MemoryPropertyFlags,
    ) -> Result<(vk::DeviceMemory, u32), MemoryError> {
        let bits = self.host_type_bits(va)?;
        let mut import = vk::ImportMemoryHostPointerInfoEXT::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::HOST_ALLOCATION_EXT)
            .host_pointer(va as usize as *mut c_void);
        let mut flags =
            vk::MemoryAllocateFlagsInfo::default().flags(vk::MemoryAllocateFlags::DEVICE_ADDRESS);
        let kind = self.pick_type(bits, preferred, vk::MemoryPropertyFlags::empty());
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
        let (handle, mirror, grant) = match self.backing {
            Backing::Direct => {
                // SAFETY: the caller of `map_host` keeps the memory mapped until `unmap`.
                let (memory, bits) = unsafe {
                    self.import_memory(host, len, true, vk::MemoryPropertyFlags::HOST_VISIBLE)?
                };
                if bits & self.window.type_bits == 0 {
                    // SAFETY: just made, never bound.
                    unsafe { self.h.device.free_memory(memory, None) };
                    return Err(MemoryError::NoMemoryType {
                        what: "the window",
                        device: self.window.type_bits,
                        host: bits,
                    });
                }
                (memory, None, None)
            }
            Backing::Mirrored => {
                let hot = va + len <= LOW_LIMIT;
                let made = if self.spill && !hot && len > self.h.budget.mirror_room() {
                    self.import_spilled(host, len)
                } else {
                    match self.import_mirrored(host, len) {
                        Err(MemoryError::Vulkan(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY))
                            if self.spill =>
                        {
                            self.import_spilled(host, len)
                        }
                        other => other,
                    }
                };
                let (handle, mirror, grant) = made?;
                (handle, mirror, Some(grant))
            }
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
                host,
                tiles: Tiles::default(),
                grant,
                pinned: false,
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
    ) -> Result<(vk::DeviceMemory, Option<Mirror>, Grant), MemoryError> {
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
                    .sharing_mode(sharing(&self.families))
                    .queue_family_indices(families_for(&self.families))
                    .push_next(&mut host),
                None,
            )?;
            let needs = device.get_buffer_memory_requirements(buffer);
            let (import, bits) =
                match self.import_memory(va, len, false, vk::MemoryPropertyFlags::HOST_VISIBLE) {
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
            let local = self.h.budget.allocate(
                device,
                &Request {
                    size: len,
                    type_bits: self.window.type_bits,
                    class: Class::Mirror,
                    device_address: true,
                },
            );
            match local {
                Ok(grant) => Ok((grant.memory, Some(Mirror { buffer, import }), grant)),
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
        self.forget_deferred(va, len);
        self.sync.forget(va, va + len);
        self.temp.forget(va, va + len);
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
                    self.retired_grants.extend(memory.grant);
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
        let freed = self
            .retired_grants
            .iter()
            .any(|g| g.placement == Placement::DeviceLocal);
        for grant in self.retired_grants.drain(..) {
            self.h.budget.release(&grant);
        }
        self.reclaims = self.reclaims.wrapping_add(1);
        let now = freed || self.promote_more || self.reclaims.is_multiple_of(64);
        if !self.deferred.is_empty() && now {
            let _ = self.retry_deferred();
        }
        if self.spill && now && self.h.budget.counters().spilled[Class::Mirror as usize] > 0 {
            let _ = self.promote_spilled();
        }
    }
}

impl Drop for AddressSpace {
    fn drop(&mut self) {
        self.forget_deferred(0, u64::MAX / 2);
        let _ = self.commit();
        // SAFETY: the owner drops the address space only once the GPU is idle.
        unsafe {
            let _ = self.h.device.device_wait_idle();
            if let Some(transfer) = self.transfer.take() {
                transfer.destroy(&self.h.device);
            }
            self.sync.destroy(&self.h.device);
            self.h.device.destroy_buffer(self.window.buffer, None);
            for memory in self.memories.drain().map(|(_, m)| m) {
                self.h.device.free_memory(memory.handle, None);
                self.retired_grants.extend(memory.grant);
                if let Some(mirror) = memory.mirror {
                    self.h.device.destroy_buffer(mirror.buffer, None);
                    self.h.device.free_memory(mirror.import, None);
                }
            }
        }
        self.reclaim();
    }
}

#[cfg(test)]
mod bench;
#[cfg(test)]
mod budget_tests;
#[cfg(test)]
mod cold_tests;
pub mod hooks;
#[cfg(test)]
mod probe;
#[cfg(test)]
mod spill_tests;
#[cfg(test)]
mod sync_tests;
#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;
