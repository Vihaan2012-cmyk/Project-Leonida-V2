//! The resources a device has made: their GPU addresses, their memory and the lookup from an address back to
//! the resource.
//!
//! A buffer's GPU address is its game address (`heap_base + address` is what shaders compute), so a buffer is
//! host memory at that address bound into the address space. A committed buffer gets an address chosen here and
//! committed host memory at it, so the CPU pointer of a mapped upload buffer is its GPU address. A buffer
//! placed by the game (`CreatePlacedResourceX`) lives in memory the game already mapped. A texture is a
//! `VkImage`; its GPU address is only the key its descriptors carry, the game address of a placed texture or an
//! address from a range no memory is mapped at.
//!
//! What a range of game memory holds is told to the address space, which decides where to keep it. A texture
//! whose tiled data was detiled into its image (`upload_texture`) is **cold**: only the image is read, so the
//! window's device-local copy of its memory is released. Buffers, placed or committed, targets and unordered-access
//! textures are **hot**, and so is whatever a view over an address names (`mark_hot`, called from the layers that
//! create them). `refresh_written` finds the textures the title wrote again and makes their images anew.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::desc::{AllocationInfo, DescError, ResourceDesc};
use super::image::{GpuImage, ImageError};
use super::layout;
use super::va::{VaAllocator, commit_host, release_host};
use crate::descriptor::format::Kind;
use crate::descriptor::gfx10;
use crate::descriptor::vulkan::{ViewInfo, view_info};
use crate::memory::{AddressSpace, Handles, LOW_LIMIT, MemoryError, PAGE, SyncPolicy};

pub type ResourceId = u32;

/// Where a committed resource's memory lives, as the heap type says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeapKind {
    Default,
    Upload,
    Readback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Committed(HeapKind),
    /// In memory the game mapped at the address it named.
    Placed,
    /// Address space with no memory until tiles are mapped into it.
    Reserved,
}

#[derive(Debug, thiserror::Error)]
pub enum ResourceError {
    #[error("{0}")]
    Desc(#[from] DescError),
    #[error("{0}")]
    Memory(#[from] MemoryError),
    #[error("{0}")]
    Image(#[from] ImageError),
    #[error("no address space left for {0:#x} bytes")]
    OutOfAddresses(u64),
    #[error("could not commit {0:#x} bytes of host memory at {1:#x}")]
    HostMemory(u64, u64),
    #[error("{0:#x} is not 64 KiB aligned")]
    Unaligned(u64),
    #[error("{address:#x}..+{len:#x} overlaps another resource")]
    Overlap { address: u64, len: u64 },
    #[error("resource {0} does not exist")]
    Unknown(ResourceId),
    #[error("resource {0} has no texture")]
    NotATexture(ResourceId),
    #[error("resource {0} has no memory the game wrote its texels in")]
    NoGuestMemory(ResourceId),
    #[error("tiled data of {0:?} textures is not handled")]
    Format(Kind),
}

pub struct Resource {
    pub id: ResourceId,
    pub desc: ResourceDesc,
    pub origin: Origin,
    /// `GetGPUVirtualAddress`.
    pub address: u64,
    /// Bytes of address space it holds.
    pub size: u64,
    /// The game memory a placed texture's texels are in; the address for anything else.
    pub memory: u64,
    pub image: Option<GpuImage>,
    owns_host: bool,
    /// The address is a key from the image range even though the memory is the game's.
    keyed: bool,
}

impl Resource {
    /// The CPU pointer of a buffer whose memory is on the host at its GPU address.
    pub fn cpu_address(&self) -> Option<u64> {
        (self.desc.is_buffer() && self.origin != Origin::Reserved).then_some(self.address)
    }
}

/// Addresses images are keyed by in the T#s that name them; nothing is mapped there.
const KEY_BASE: u64 = 1 << 44;
const KEY_END: u64 = 1 << 45;
/// The first part of the window, under 4 GiB, kept for what must be there (descriptor heaps).
const LOW_SIZE: u64 = 256 << 20;

pub struct Resources {
    space: Arc<Mutex<AddressSpace>>,
    low: VaAllocator,
    general: VaAllocator,
    keys: VaAllocator,
    by_address: BTreeMap<u64, ResourceId>,
    items: HashMap<ResourceId, Resource>,
    next: ResourceId,
    /// Placed textures whose image was made from their memory, by the game address of that memory.
    detiled: Mutex<BTreeSet<(u64, ResourceId)>>,
}

impl Resources {
    pub fn new(space: AddressSpace) -> Resources {
        Resources::shared(Arc::new(Mutex::new(space)))
    }

    /// Resources over an address space that others (the `XMem*` shims, through `memory::hooks`) also use: every
    /// use here takes the same lock, briefly.
    pub fn shared(space: Arc<Mutex<AddressSpace>>) -> Resources {
        let (base, size) = space
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .window();
        let end = base + size;
        let start = base.max(PAGE);
        let low_end = (start + LOW_SIZE).min(LOW_LIMIT).min(end);
        Resources {
            space,
            low: VaAllocator::new(start, low_end),
            general: VaAllocator::new(low_end, end),
            keys: VaAllocator::new(KEY_BASE, KEY_END),
            by_address: BTreeMap::new(),
            items: HashMap::new(),
            next: 1,
            detiled: Mutex::new(BTreeSet::new()),
        }
    }

    pub fn space(&self) -> MutexGuard<'_, AddressSpace> {
        self.space.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn space_mut(&mut self) -> MutexGuard<'_, AddressSpace> {
        self.space()
    }

    fn handles(&self) -> Handles {
        self.space().handles().clone()
    }

    pub fn shared_space(&self) -> &Arc<Mutex<AddressSpace>> {
        &self.space
    }

    /// The value of the shaders' `heap_base` push constants.
    pub fn heap_base(&self) -> u64 {
        self.space().heap_base()
    }

    /// Committed host memory under 4 GiB, bound into the address space and zeroed, for descriptor heaps: nearly
    /// every shader truncates a descriptor table's address to 32 bits. The CPU pointer is the address.
    pub fn alloc_low(&mut self, len: u64, policy: SyncPolicy) -> Result<u64, ResourceError> {
        let len = len.next_multiple_of(PAGE);
        self.alloc_committed(len, true, policy, policy == SyncPolicy::WriteWatch)
    }

    pub fn free_low(&mut self, va: u64, len: u64) {
        let len = len.next_multiple_of(PAGE);
        let _ = self.space().unmap(va, len);
        release_host(va);
        self.free_va(va, len);
    }

    /// Tells the allocators the game uses `address..address + len` itself (memory it reserved and mapped), so no
    /// resource is given addresses there.
    pub fn note_game_range(&mut self, address: u64, len: u64) {
        let _ = self.low.reserve(address, len) || self.general.reserve(address, len);
    }

    /// Takes an address from `general` (or `low` when asked) where host memory can be committed: addresses the
    /// process already uses for something else are skipped and kept out of the allocator.
    fn alloc_committed(
        &mut self,
        len: u64,
        low: bool,
        policy: SyncPolicy,
        write_watch: bool,
    ) -> Result<u64, ResourceError> {
        for _ in 0..64 {
            let va = if low {
                self.low.alloc(len, PAGE)
            } else {
                self.general
                    .alloc(len, PAGE)
                    .or_else(|| self.low.alloc(len, PAGE))
            }
            .ok_or(ResourceError::OutOfAddresses(len))?;
            match self.commit(va, len, policy, write_watch) {
                Ok(()) => return Ok(va),
                Err(ResourceError::HostMemory(..)) => continue,
                Err(e) => {
                    self.free_va(va, len);
                    return Err(e);
                }
            }
        }
        Err(ResourceError::OutOfAddresses(len))
    }

    fn free_va(&mut self, va: u64, len: u64) {
        if va < self.low_end() {
            self.low.free(va, len);
        } else {
            self.general.free(va, len);
        }
    }

    fn low_end(&self) -> u64 {
        let (base, size) = self.space().window();
        (base.max(PAGE) + LOW_SIZE).min(LOW_LIMIT).min(base + size)
    }

    fn commit(
        &mut self,
        va: u64,
        len: u64,
        policy: SyncPolicy,
        write_watch: bool,
    ) -> Result<(), ResourceError> {
        if !commit_host(va, len, write_watch) {
            return Err(ResourceError::HostMemory(len, va));
        }
        let mut space = self.space();
        if let Err(e) = space.map_host(va, len, policy) {
            drop(space);
            release_host(va);
            return Err(e.into());
        }
        space.mark_hot(va, len);
        Ok(())
    }

    fn insert(
        &mut self,
        desc: ResourceDesc,
        origin: Origin,
        (address, memory, size): (u64, u64, u64),
        image: Option<GpuImage>,
        owns_host: bool,
    ) -> ResourceId {
        let id = self.next;
        self.next += 1;
        // Aliasing buffers may start at the same address; the first one keeps the entry.
        self.by_address.entry(address).or_insert(id);
        self.items.insert(
            id,
            Resource {
                id,
                desc,
                origin,
                address,
                size,
                memory,
                image,
                owns_host,
                keyed: false,
            },
        );
        id
    }

    /// `CreateCommittedResource`: an address and memory of its own.
    pub fn create_committed(
        &mut self,
        desc: &ResourceDesc,
        heap: HeapKind,
    ) -> Result<ResourceId, ResourceError> {
        let AllocationInfo { size, .. } = desc.allocation_info()?;
        if desc.is_buffer() {
            let (policy, watch) = match heap {
                HeapKind::Upload => (SyncPolicy::WriteWatch, true),
                _ => (SyncPolicy::Marked, false),
            };
            let va = self.alloc_committed(size, false, policy, watch)?;
            return Ok(self.insert(*desc, Origin::Committed(heap), (va, va, size), None, true));
        }
        let key = self
            .keys
            .alloc(size, PAGE)
            .ok_or(ResourceError::OutOfAddresses(size))?;
        match GpuImage::create(&self.handles(), desc) {
            Ok(image) => Ok(self.insert(
                *desc,
                Origin::Committed(heap),
                (key, key, size),
                Some(image),
                false,
            )),
            Err(e) => {
                self.keys.free(key, size);
                Err(e.into())
            }
        }
    }

    /// `CreatePlacedResourceX(va, desc, ...)`: the resource is the memory the game mapped at `va`. A buffer
    /// whose pages are not yet bound into the address space (the game maps them without telling this layer) gets
    /// them bound here.
    pub fn create_placed(
        &mut self,
        va: u64,
        desc: &ResourceDesc,
    ) -> Result<ResourceId, ResourceError> {
        let AllocationInfo { size, .. } = desc.allocation_info()?;
        if desc.is_buffer() {
            // D3D12 lets placed resources alias, and the title places buffers over memory other buffers use.
            self.note_game_range(va, size);
            // The console places buffers at any address in memory the title mapped; the window binds whole pages.
            let last = (va + size).next_multiple_of(PAGE);
            let mut at = va - va % PAGE;
            while at < last {
                if self.space().is_bound(at) {
                    at += PAGE;
                    continue;
                }
                let mut end = at + PAGE;
                while end < last && !self.space().is_bound(end) {
                    end += PAGE;
                }
                self.space().map_host(at, end - at, SyncPolicy::Always)?;
                at = end;
            }
            self.space().mark_hot(va, size);
            return Ok(self.insert(*desc, Origin::Placed, (va, va, size), None, false));
        }
        let image = GpuImage::create(&self.handles(), desc)?;
        if !desc.detiles_for_sampling() {
            self.space().mark_hot(va, size);
        }
        let (address, keyed) = if self.by_address.contains_key(&va) {
            let key = self
                .keys
                .alloc(size, PAGE)
                .ok_or(ResourceError::OutOfAddresses(size))?;
            (key, true)
        } else {
            (va, false)
        };
        let id = self.insert(
            *desc,
            Origin::Placed,
            (address, va, size),
            Some(image),
            false,
        );
        if let Some(r) = self.items.get_mut(&id) {
            r.keyed = keyed;
        }
        Ok(id)
    }

    /// `CreateReservedResource`: a buffer is address space with nothing behind it until `map_tiles`; a texture is
    /// fully backed.
    pub fn create_reserved(&mut self, desc: &ResourceDesc) -> Result<ResourceId, ResourceError> {
        let AllocationInfo { size, .. } = desc.allocation_info()?;
        if desc.is_buffer() {
            let va = self
                .general
                .alloc(size, PAGE)
                .or_else(|| self.low.alloc(size, PAGE))
                .ok_or(ResourceError::OutOfAddresses(size))?;
            return Ok(self.insert(*desc, Origin::Reserved, (va, va, size), None, false));
        }
        let key = self
            .keys
            .alloc(size, PAGE)
            .ok_or(ResourceError::OutOfAddresses(size))?;
        match GpuImage::create(&self.handles(), desc) {
            Ok(image) => Ok(self.insert(
                *desc,
                Origin::Reserved,
                (key, key, size),
                Some(image),
                false,
            )),
            Err(e) => {
                self.keys.free(key, size);
                Err(e.into())
            }
        }
    }

    /// `UpdateTileMappings` for a reserved buffer: `len` bytes at `offset` now show the host memory at `host`.
    pub fn map_tiles(
        &mut self,
        id: ResourceId,
        offset: u64,
        host: u64,
        len: u64,
    ) -> Result<(), ResourceError> {
        let r = self.items.get(&id).ok_or(ResourceError::Unknown(id))?;
        let va = r.address + offset;
        let mut space = self.space();
        space.map_host_at(va, host, len, SyncPolicy::Always)?;
        space.mark_hot(va, len);
        Ok(())
    }

    /// A tile range back to nothing.
    pub fn unmap_tiles(
        &mut self,
        id: ResourceId,
        offset: u64,
        len: u64,
    ) -> Result<(), ResourceError> {
        let r = self.items.get(&id).ok_or(ResourceError::Unknown(id))?;
        Ok(self.space().unmap(r.address + offset, len)?)
    }

    pub fn destroy(&mut self, id: ResourceId) -> Result<(), ResourceError> {
        let r = self.items.remove(&id).ok_or(ResourceError::Unknown(id))?;
        if self.by_address.get(&r.address) == Some(&id) {
            self.by_address.remove(&r.address);
            if let Some(alias) = self
                .items
                .values()
                .find(|other| other.address == r.address)
                .map(|other| other.id)
            {
                self.by_address.insert(r.address, alias);
            }
        }
        self.detiled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&(r.memory, id));
        let key = r.image.is_some() && (r.origin != Origin::Placed || r.keyed);
        if let Some(image) = r.image {
            image.destroy(&self.handles());
        } else if r.desc.is_buffer() {
            match r.origin {
                Origin::Committed(_) => {
                    let _ = self.space().unmap(r.address, r.size);
                    if r.owns_host {
                        release_host(r.address);
                    }
                    self.free_va(r.address, r.size);
                }
                Origin::Reserved => {
                    let _ = self.space().unmap(r.address, r.size);
                    self.free_va(r.address, r.size);
                }
                Origin::Placed => {}
            }
        }
        if key {
            self.keys.free(r.address, r.size);
        }
        Ok(())
    }

    pub fn get(&self, id: ResourceId) -> Option<&Resource> {
        self.items.get(&id)
    }

    /// The resource holding a GPU address, and the offset of the address in it.
    pub fn find(&self, address: u64) -> Option<(ResourceId, u64)> {
        let (&start, &id) = self.by_address.range(..=address).next_back()?;
        let r = &self.items[&id];
        (address < start + r.size).then_some((id, address - start))
    }

    /// The texture a registered image descriptor names, and the Vulkan view to make of it.
    pub fn view_for(&self, image: &gfx10::Image) -> Option<(&Resource, ViewInfo)> {
        let (id, _) = self.find(image.address)?;
        let r = self.items.get(&id)?;
        let gpu = r.image.as_ref()?;
        let format = r.desc.format_info().ok()?;
        let mut info = view_info(image, format)?;
        if format.kind != Kind::Color {
            info.format = gpu.format;
        }
        Some((r, info))
    }

    fn texture(&self, id: ResourceId) -> Result<&Resource, ResourceError> {
        let r = self.items.get(&id).ok_or(ResourceError::Unknown(id))?;
        if r.image.is_none() {
            return Err(ResourceError::NotATexture(id));
        }
        Ok(r)
    }

    /// Reads a placed texture's texels out of the game memory it sits in (console tiled, or row-major with the
    /// `GetCopyableFootprints` pitches) and writes them to the image. Block-compressed and uncompressed colour
    /// formats only.
    pub fn upload_texture(&self, id: ResourceId) -> Result<(), ResourceError> {
        let r = self.texture(id)?;
        let (desc, image) = (&r.desc, r.image.as_ref().expect("checked"));
        if r.origin != Origin::Placed {
            return Err(ResourceError::NoGuestMemory(id));
        }
        let info = desc.format_info()?;
        if info.kind != Kind::Color {
            return Err(ResourceError::Format(info.kind));
        }
        let sampled = desc.detiles_for_sampling();
        if sampled {
            self.space().rebase_writes(r.memory, r.size);
        }
        // SAFETY: a placed texture's memory is host memory the game mapped at its address, `size` bytes long.
        let memory = unsafe { std::slice::from_raw_parts(r.memory as *const u8, r.size as usize) };
        let handles = self.handles();
        if desc.is_tiled() {
            let array = desc.array_layout()?;
            for slice in 0..desc.array_size() {
                for mip in 0..desc.mips() {
                    let rows = array
                        .detile(slice, mip, memory)
                        .ok_or(ResourceError::NoGuestMemory(id))?;
                    image.upload(&handles, mip, slice, &rows)?;
                }
            }
        } else {
            let (footprints, _) = layout::row_major_footprints(
                desc.width as u32,
                desc.height,
                desc.depth(),
                desc.mips(),
                desc.array_size(),
                info.block,
                info.bytes,
            );
            for (index, f) in footprints.iter().enumerate() {
                let (slice, mip) = (index as u32 / desc.mips(), index as u32 % desc.mips());
                let mut rows =
                    Vec::with_capacity((f.row_bytes * u64::from(f.rows * f.depth)) as usize);
                for row in 0..u64::from(f.rows * f.depth) {
                    let at = (f.offset + row * f.row_pitch) as usize;
                    rows.extend_from_slice(&memory[at..at + f.row_bytes as usize]);
                }
                image.upload(&handles, mip, slice, &rows)?;
            }
        }
        if sampled {
            self.space().mark_cold(r.memory, r.size);
            self.detiled
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert((r.memory, id));
        }
        Ok(())
    }

    /// Makes the image of every detiled texture again whose memory the title wrote since it was made (the host's
    /// own writes reported to the address space, and the write watch). Returns how many it made.
    pub fn refresh_written(&self) -> usize {
        let written = self.space().take_cold_writes();
        if written.is_empty() {
            return 0;
        }
        let hit: Vec<ResourceId> = {
            let detiled = self.detiled.lock().unwrap_or_else(PoisonError::into_inner);
            detiled
                .iter()
                .filter(|(memory, id)| {
                    let size = self.items.get(id).map_or(0, |r| r.size);
                    written
                        .iter()
                        .any(|&(lo, hi)| *memory < hi && lo < memory + size)
                })
                .map(|&(_, id)| id)
                .collect()
        };
        hit.into_iter()
            .filter(|&id| self.upload_texture(id).is_ok())
            .count()
    }

    /// Gives the device-local copy of detiled textures' memory back (`AddressSpace::release_cold`). Call it between
    /// submissions, with the GPU idle.
    pub fn release_detiled(&self) -> u64 {
        self.space().release_cold().unwrap_or(0)
    }

    /// Says the GPU reads or writes `address..address + len` through the window: a view named it, a submission
    /// writes it. Not texture memory then.
    pub fn mark_hot(&self, address: u64, len: u64) {
        self.space().mark_hot(address, len);
    }

    /// The reverse: the image's texels written back into the game memory of a placed texture.
    pub fn download_texture(&self, id: ResourceId) -> Result<(), ResourceError> {
        let r = self.texture(id)?;
        let (desc, image) = (&r.desc, r.image.as_ref().expect("checked"));
        if r.origin != Origin::Placed {
            return Err(ResourceError::NoGuestMemory(id));
        }
        let info = desc.format_info()?;
        if info.kind != Kind::Color {
            return Err(ResourceError::Format(info.kind));
        }
        // SAFETY: as in `upload_texture`; the game does not write the texture while it is being read back.
        let memory =
            unsafe { std::slice::from_raw_parts_mut(r.memory as *mut u8, r.size as usize) };
        let handles = self.handles();
        if desc.is_tiled() {
            let array = desc.array_layout()?;
            for slice in 0..desc.array_size() {
                for mip in 0..desc.mips() {
                    let m = &array.slice.mips[mip as usize];
                    let bytes = (m.blocks_w * m.blocks_h * info.bytes) as usize;
                    let rows = image.download(&handles, mip, slice, bytes)?;
                    array
                        .tile(slice, mip, &rows, memory)
                        .ok_or(ResourceError::NoGuestMemory(id))?;
                }
            }
        } else {
            let (footprints, _) = layout::row_major_footprints(
                desc.width as u32,
                desc.height,
                desc.depth(),
                desc.mips(),
                desc.array_size(),
                info.block,
                info.bytes,
            );
            for (index, f) in footprints.iter().enumerate() {
                let (slice, mip) = (index as u32 / desc.mips(), index as u32 % desc.mips());
                let total = (f.row_bytes * u64::from(f.rows * f.depth)) as usize;
                let rows = image.download(&handles, mip, slice, total)?;
                for row in 0..u64::from(f.rows * f.depth) {
                    let at = (f.offset + row * f.row_pitch) as usize;
                    let from = (row * f.row_bytes) as usize;
                    memory[at..at + f.row_bytes as usize]
                        .copy_from_slice(&rows[from..from + f.row_bytes as usize]);
                }
            }
        }
        Ok(())
    }
}
