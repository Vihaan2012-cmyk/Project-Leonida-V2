//! How much device-local memory the layer may use, and where an allocation goes when that is not enough.
//!
//! The console title expects up to about 10 GB of GPU-optimal memory; a PC GPU may have 8 GB. Everything the layer
//! puts in device-local memory is allocated through `Budget::allocate`, which knows the real budget
//! (`VK_EXT_memory_budget`, or a limit injected with `set_limit`) and what the layer has taken:
//!
//! - **Render targets, depth buffers and unordered-access images** (`Class::Target`) are tried in device-local
//!   memory whatever the budget says, at the highest priority. Priority does not make the driver move other memory
//!   out for them (measured on an RTX 4060, see `memory`): what keeps room for them is the mirror's cap below.
//! - **Textures** (`Class::Texture`) go to device-local memory while they fit, and to system memory the GPU reads
//!   over PCIe when they do not or when device-local allocation fails.
//! - **The mirror of the game's memory** (`Class::Mirror`) is what shaders read the game's memory through. It takes
//!   at most `mirror_room` of device-local memory, a share of the budget that leaves the rest to targets, textures
//!   and the layer's own buffers. `AddressSpace` places the runs beyond that in system memory instead (its spill
//!   tier: the title's own host memory bound into the window, which the GPU reads over PCIe and nothing copies).
//!   Where the spill tier is not available a run past the budget is still tried in device-local memory at a low
//!   priority, which with `VK_EXT_pageable_device_local_memory` lets the driver demote it to system memory first,
//!   and where even that fails, `AddressSpace` leaves the run unbound and retries it later rather than failing the
//!   game's allocation.
//!
//! Counters are kept for all of it, and the notes say when spilling starts and when the numbers move.

use std::ffi::CStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use ash::vk;

const MIB: u64 = 1 << 20;

/// The share of the device-local budget, in percent, the mirror of the game's memory may take: what is left holds
/// render targets, depth buffers, the detiled copies of textures and what the layer allocates outside the budget.
pub const MIRROR_SHARE: u64 = 55;

/// The memory extensions the layer uses when the GPU has them. Each must be enabled when the device is made, with
/// the features `memory_priority` and `pageable_device_local_memory` name.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemoryExtensions {
    /// `VK_EXT_memory_budget`: the driver's own budget and usage of each heap.
    pub budget: bool,
    /// `VK_EXT_memory_priority`: which allocations the driver keeps in device-local memory first.
    pub priority: bool,
    /// `VK_EXT_pageable_device_local_memory`: lets the driver move device-local allocations to system memory.
    pub pageable: bool,
}

impl MemoryExtensions {
    /// What `physical` offers.
    pub fn available(instance: &ash::Instance, physical: vk::PhysicalDevice) -> MemoryExtensions {
        // SAFETY: a plain query on a physical device of `instance`.
        let have = unsafe {
            instance
                .enumerate_device_extension_properties(physical)
                .unwrap_or_default()
        };
        let has = |name: &CStr| have.iter().any(|e| e.extension_name_as_c_str() == Ok(name));
        let mut priority = vk::PhysicalDeviceMemoryPriorityFeaturesEXT::default();
        let mut pageable = vk::PhysicalDevicePageableDeviceLocalMemoryFeaturesEXT::default();
        let (has_priority, has_pageable) = (
            has(ash::ext::memory_priority::NAME),
            has(ash::ext::pageable_device_local_memory::NAME),
        );
        let mut features = vk::PhysicalDeviceFeatures2::default();
        if has_priority {
            features = features.push_next(&mut priority);
        }
        if has_pageable {
            features = features.push_next(&mut pageable);
        }
        // SAFETY: a plain query on a physical device of `instance`; the structures outlive it.
        unsafe { instance.get_physical_device_features2(physical, &mut features) };
        let priority = has_priority && priority.memory_priority == vk::TRUE;
        MemoryExtensions {
            budget: has(ash::ext::memory_budget::NAME),
            priority,
            pageable: priority && has_pageable && pageable.pageable_device_local_memory == vk::TRUE,
        }
    }

    /// The extension names to enable on the device.
    pub fn names(self) -> Vec<&'static CStr> {
        let mut names = Vec::new();
        if self.budget {
            names.push(ash::ext::memory_budget::NAME);
        }
        if self.priority {
            names.push(ash::ext::memory_priority::NAME);
        }
        if self.pageable {
            names.push(ash::ext::pageable_device_local_memory::NAME);
        }
        names
    }
}

/// What a device-local allocation is for, which decides what is kept in device-local memory first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Render targets, depth buffers and images shaders write: written every frame, never spilled while avoidable.
    Target,
    /// The device-local copy of the game's memory behind the sparse window.
    Mirror,
    /// Sampled images: read-mostly, and fine to read over PCIe.
    Texture,
}

impl Class {
    const COUNT: usize = 3;

    fn index(self) -> usize {
        self as usize
    }

    fn priority(self) -> f32 {
        match self {
            Class::Target => 1.0,
            Class::Mirror => 0.6,
            Class::Texture => 0.4,
        }
    }
}

/// Where an allocation ended up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    DeviceLocal,
    /// System memory the GPU reads over PCIe.
    Spilled,
}

/// What `Budget::allocate` needs to know.
#[derive(Debug, Clone, Copy)]
pub struct Request {
    pub size: u64,
    /// The memory types the resource accepts.
    pub type_bits: u32,
    pub class: Class,
    /// Allocate with `VK_MEMORY_ALLOCATE_DEVICE_ADDRESS`.
    pub device_address: bool,
}

/// A granted allocation. Give it back to `Budget::release` when the memory is freed.
#[derive(Debug, Clone, Copy)]
pub struct Grant {
    pub memory: vk::DeviceMemory,
    pub memory_type: u32,
    pub size: u64,
    pub class: Class,
    pub placement: Placement,
}

/// A reading of the counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counters {
    /// What the process may use of the device-local heaps now (the driver's budget, or the injected limit).
    pub budget: u64,
    /// What the process uses of them now: the driver's figure when it gives one, otherwise the layer's own.
    pub used: u64,
    /// The layer's own device-local bytes, by `Class` (`Target`, `Mirror`, `Texture`).
    pub local: [u64; Class::COUNT],
    /// The layer's bytes in system memory because device-local memory was short, by `Class`.
    pub spilled: [u64; Class::COUNT],
    /// The most `spilled` has held at once, summed over the classes.
    pub peak_spilled: u64,
    /// Device-local allocations the driver refused.
    pub refused: u64,
    /// Bytes of the game's memory left unbound for now because nothing could back them.
    pub deferred: u64,
}

impl Counters {
    pub fn local_total(&self) -> u64 {
        self.local.iter().sum()
    }

    pub fn spilled_total(&self) -> u64 {
        self.spilled.iter().sum()
    }

    /// The line the boot trace carries.
    pub fn line(&self) -> String {
        format!(
            "vram: device-local {} of {} MiB budget in use ({} MiB ours: {} targets, {} game memory, {} \
             textures); spilled to system memory {} MiB ({} targets, {} game memory, {} textures), \
             peak {} MiB; refused {}; left unbound {} MiB",
            self.used / MIB,
            self.budget / MIB,
            self.local_total() / MIB,
            self.local[0] / MIB,
            self.local[1] / MIB,
            self.local[2] / MIB,
            self.spilled_total() / MIB,
            self.spilled[0] / MIB,
            self.spilled[1] / MIB,
            self.spilled[2] / MIB,
            self.peak_spilled / MIB,
            self.refused,
            self.deferred / MIB,
        )
    }
}

type NoteFn = Box<dyn Fn(String) + Send + Sync>;

#[derive(Default)]
struct Noting {
    spill_started: bool,
    /// The spilled total at the last note.
    spilled_at: u64,
    /// The device-local total at the last note.
    local_at: u64,
    last: Option<std::time::Instant>,
    last_counters: Counters,
}

/// The device-local budget and the counters of what the layer took of it. Shared by everything that allocates.
pub struct Budget {
    instance: ash::Instance,
    physical: vk::PhysicalDevice,
    extensions: MemoryExtensions,
    types: vk::PhysicalDeviceMemoryProperties,
    local_heaps: u32,
    local_heap_size: u64,
    /// `u64::MAX` when the driver's budget is used.
    limit: AtomicU64,
    /// With a limit set, refuse device-local allocations past it as a GPU of that size would.
    enforced: AtomicBool,
    /// What the mirror may take of device-local memory; `u64::MAX` for `MIRROR_SHARE` of the budget.
    mirror_limit: AtomicU64,
    local: [AtomicU64; Class::COUNT],
    spilled: [AtomicU64; Class::COUNT],
    peak_spilled: AtomicU64,
    refused: AtomicU64,
    deferred: AtomicU64,
    noting: Mutex<Noting>,
    note: RwLock<Option<NoteFn>>,
}

impl Budget {
    pub fn new(
        instance: &ash::Instance,
        physical: vk::PhysicalDevice,
        extensions: MemoryExtensions,
    ) -> Arc<Budget> {
        // SAFETY: `physical` came from `instance`.
        let types = unsafe { instance.get_physical_device_memory_properties(physical) };
        let mut local_heaps = 0;
        let mut local_heap_size = 0;
        for i in 0..types.memory_heap_count {
            let heap = types.memory_heaps[i as usize];
            if heap.flags.contains(vk::MemoryHeapFlags::DEVICE_LOCAL) {
                local_heaps |= 1 << i;
                local_heap_size += heap.size;
            }
        }
        Arc::new(Budget {
            instance: instance.clone(),
            physical,
            extensions,
            types,
            local_heaps,
            local_heap_size,
            limit: AtomicU64::new(u64::MAX),
            enforced: AtomicBool::new(false),
            mirror_limit: AtomicU64::new(u64::MAX),
            local: Default::default(),
            spilled: Default::default(),
            peak_spilled: AtomicU64::new(0),
            refused: AtomicU64::new(0),
            deferred: AtomicU64::new(0),
            noting: Mutex::new(Noting::default()),
            note: RwLock::new(None),
        })
    }

    pub fn extensions(&self) -> MemoryExtensions {
        self.extensions
    }

    /// Where the notes go: the boot trace.
    pub fn set_note(&self, note: impl Fn(String) + Send + Sync + 'static) {
        *self.note.write().unwrap_or_else(PoisonError::into_inner) = Some(Box::new(note));
    }

    /// Replaces the driver's budget with `bytes` of device-local memory for the layer's own allocations (a test
    /// hook, and `LEONIDA_VRAM_LIMIT_MB`); `None` goes back to the driver's.
    pub fn set_limit(&self, bytes: Option<u64>) {
        self.limit
            .store(bytes.unwrap_or(u64::MAX), Ordering::Relaxed);
    }

    /// With a limit set, makes device-local allocations past it fail with `ERROR_OUT_OF_DEVICE_MEMORY`, as a GPU
    /// that small would refuse them (a test hook). Without it the limit only steers where textures go.
    pub fn set_enforced(&self, enforced: bool) {
        self.enforced.store(enforced, Ordering::Relaxed);
    }

    /// Replaces the share of the budget the mirror of the game's memory may take with `bytes` (a test hook, and
    /// `LEONIDA_VRAM_MIRROR_MB`); `None` goes back to `MIRROR_SHARE`.
    pub fn set_mirror_limit(&self, bytes: Option<u64>) {
        self.mirror_limit
            .store(bytes.unwrap_or(u64::MAX), Ordering::Relaxed);
    }

    /// How much more device-local memory the mirror of the game's memory may take: what its cap leaves, and no
    /// more than `available`. A run longer than this goes to system memory.
    pub fn mirror_room(&self) -> u64 {
        let taken = self.local[Class::Mirror.index()].load(Ordering::Relaxed);
        self.mirror_cap()
            .saturating_sub(taken)
            .min(self.available())
    }

    /// The most device-local memory the mirror of the game's memory may hold.
    pub fn mirror_cap(&self) -> u64 {
        match self.mirror_limit.load(Ordering::Relaxed) {
            u64::MAX => self.heap_state().0 / 100 * MIRROR_SHARE,
            bytes => bytes,
        }
    }

    /// Counts `size` bytes of `class` placed in system memory without any device memory being allocated for them
    /// (the spill tier of the game's memory). Give the grant to `release` when the memory is unmapped.
    pub(super) fn record_spill(&self, class: Class, size: u64) -> Grant {
        let grant = Grant {
            memory: vk::DeviceMemory::null(),
            memory_type: u32::MAX,
            size,
            class,
            placement: Placement::Spilled,
        };
        self.account(&grant);
        grant
    }

    /// Whether the GPU has memory outside its device-local heaps to spill to.
    pub fn can_spill(&self) -> bool {
        (0..self.types.memory_type_count).any(|i| !self.is_local(i))
    }

    fn say(&self, text: String) {
        if let Some(note) = self
            .note
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            note(text);
        }
    }

    fn is_local(&self, memory_type: u32) -> bool {
        self.local_heaps & (1 << self.types.memory_types[memory_type as usize].heap_index) != 0
    }

    fn ours(&self, counters: &[AtomicU64; Class::COUNT]) -> [u64; Class::COUNT] {
        [
            counters[0].load(Ordering::Relaxed),
            counters[1].load(Ordering::Relaxed),
            counters[2].load(Ordering::Relaxed),
        ]
    }

    /// The device-local budget and what is used of it.
    fn heap_state(&self) -> (u64, u64) {
        let ours: u64 = self.ours(&self.local).iter().sum();
        let limit = self.limit.load(Ordering::Relaxed);
        if limit != u64::MAX {
            return (limit, ours);
        }
        if self.extensions.budget {
            let mut budget = vk::PhysicalDeviceMemoryBudgetPropertiesEXT::default();
            let mut properties =
                vk::PhysicalDeviceMemoryProperties2::default().push_next(&mut budget);
            // SAFETY: a plain query on a physical device of `instance`, with the extension enabled on its device.
            unsafe {
                self.instance
                    .get_physical_device_memory_properties2(self.physical, &mut properties);
            }
            let mut total = (0, 0);
            for i in 0..self.types.memory_heap_count as usize {
                if self.local_heaps & (1 << i) != 0 {
                    total.0 += budget.heap_budget[i];
                    total.1 += budget.heap_usage[i];
                }
            }
            if total.0 > 0 {
                return total;
            }
        }
        (self.local_heap_size / 8 * 7, ours)
    }

    /// Room left in device-local memory for new allocations, after leaving the driver some for what the layer
    /// does not allocate through the budget (swap chains, pipelines, upscaler buffers).
    pub fn available(&self) -> u64 {
        let (budget, used) = self.heap_state();
        let reserve = if self.limit.load(Ordering::Relaxed) == u64::MAX {
            (budget / 32).max(64 * MIB)
        } else {
            0
        };
        budget.saturating_sub(used).saturating_sub(reserve)
    }

    pub fn counters(&self) -> Counters {
        let (budget, used) = self.heap_state();
        Counters {
            budget,
            used,
            local: self.ours(&self.local),
            spilled: self.ours(&self.spilled),
            peak_spilled: self.peak_spilled.load(Ordering::Relaxed),
            refused: self.refused.load(Ordering::Relaxed),
            deferred: self.deferred.load(Ordering::Relaxed),
        }
    }

    /// Records bytes of the game's memory left unbound (positive) or bound again (negative).
    pub(super) fn defer(&self, bytes: i64) {
        if bytes >= 0 {
            self.deferred.fetch_add(bytes as u64, Ordering::Relaxed);
        } else {
            self.deferred
                .fetch_sub(bytes.unsigned_abs(), Ordering::Relaxed);
        }
    }

    pub(super) fn note_unbound(&self, va: u64, len: u64) {
        self.say(format!(
            "vram: no device-local memory for {len:#x} bytes of game memory at {va:#x}: left unbound for \
             now and retried when memory is freed; the GPU sees zeros there meanwhile"
        ));
        self.note_counters();
    }

    /// The best memory type for `bits` in device-local memory, or in system memory.
    fn pick(&self, bits: u32, local: bool) -> Option<u32> {
        let candidates = (0..self.types.memory_type_count)
            .filter(|i| bits & (1 << i) != 0 && self.is_local(*i) == local);
        let flags = |i: u32| self.types.memory_types[i as usize].property_flags;
        if local {
            candidates.min_by_key(|i| {
                (
                    flags(*i).contains(vk::MemoryPropertyFlags::HOST_VISIBLE),
                    *i,
                )
            })
        } else {
            candidates.min_by_key(|i| (flags(*i).as_raw().count_ones(), *i))
        }
    }

    /// Allocates for `request` in the order its class calls for. See the module documentation.
    ///
    /// # Safety
    /// `device` is the device `Budget::new` was made for.
    pub unsafe fn allocate(
        &self,
        device: &ash::Device,
        request: &Request,
    ) -> Result<Grant, vk::Result> {
        let local = self.pick(request.type_bits, true);
        let system = self.pick(request.type_bits, false);
        let over = request.size > self.available();
        let order: [(Option<u32>, Placement); 2] = match request.class {
            Class::Texture if over => [
                (system, Placement::Spilled),
                (local, Placement::DeviceLocal),
            ],
            _ => [
                (local, Placement::DeviceLocal),
                (system, Placement::Spilled),
            ],
        };
        let mut last = vk::Result::ERROR_OUT_OF_DEVICE_MEMORY;
        for (kind, placement) in order {
            let Some(kind) = kind else { continue };
            let priority = match (placement, request.class, over) {
                (Placement::DeviceLocal, Class::Mirror, true) => 0.1,
                _ => request.class.priority(),
            };
            let refused = placement == Placement::DeviceLocal
                && self.enforced.load(Ordering::Relaxed)
                && self.limit.load(Ordering::Relaxed) != u64::MAX
                && self.heap_state().1 + request.size > self.limit.load(Ordering::Relaxed);
            // SAFETY: as the caller promises.
            let made = if refused {
                Err(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY)
            } else {
                unsafe { self.allocate_in(device, request, kind, priority) }
            };
            match made {
                Ok(memory) => {
                    let grant = Grant {
                        memory,
                        memory_type: kind,
                        size: request.size,
                        class: request.class,
                        placement,
                    };
                    self.account(&grant);
                    return Ok(grant);
                }
                Err(e) => {
                    if placement == Placement::DeviceLocal {
                        self.refused.fetch_add(1, Ordering::Relaxed);
                    }
                    last = e;
                }
            }
        }
        Err(last)
    }

    unsafe fn allocate_in(
        &self,
        device: &ash::Device,
        request: &Request,
        kind: u32,
        priority: f32,
    ) -> Result<vk::DeviceMemory, vk::Result> {
        let mut flags =
            vk::MemoryAllocateFlagsInfo::default().flags(vk::MemoryAllocateFlags::DEVICE_ADDRESS);
        let mut prioritised = vk::MemoryPriorityAllocateInfoEXT::default().priority(priority);
        let mut info = vk::MemoryAllocateInfo::default()
            .allocation_size(request.size)
            .memory_type_index(kind);
        if request.device_address {
            info = info.push_next(&mut flags);
        }
        if self.extensions.priority && self.is_local(kind) {
            info = info.push_next(&mut prioritised);
        }
        // SAFETY: as the caller promises.
        unsafe { device.allocate_memory(&info, None) }
    }

    fn account(&self, grant: &Grant) {
        let i = grant.class.index();
        match grant.placement {
            Placement::DeviceLocal => {
                self.local[i].fetch_add(grant.size, Ordering::Relaxed);
            }
            Placement::Spilled => {
                self.spilled[i].fetch_add(grant.size, Ordering::Relaxed);
                let total: u64 = self.ours(&self.spilled).iter().sum();
                self.peak_spilled.fetch_max(total, Ordering::Relaxed);
            }
        }
        self.note_change(grant);
    }

    /// Takes `grant` off the counters. Call it when its memory is freed.
    pub fn release(&self, grant: &Grant) {
        let counters = match grant.placement {
            Placement::DeviceLocal => &self.local,
            Placement::Spilled => &self.spilled,
        };
        counters[grant.class.index()].fetch_sub(grant.size, Ordering::Relaxed);
    }

    fn note_change(&self, grant: &Grant) {
        let spilled: u64 = self.ours(&self.spilled).iter().sum();
        let local: u64 = self.ours(&self.local).iter().sum();
        let (first, moved) = {
            let mut noting = self.noting.lock().unwrap_or_else(PoisonError::into_inner);
            let first = grant.placement == Placement::Spilled && !noting.spill_started;
            noting.spill_started |= first;
            let moved = spilled.abs_diff(noting.spilled_at) >= 256 * MIB
                || local.abs_diff(noting.local_at) >= 512 * MIB;
            if first || moved {
                noting.spilled_at = spilled;
                noting.local_at = local;
            }
            (first, moved)
        };
        if first {
            self.say(format!(
                "vram: device-local memory is short, spilling {} MiB {:?} to system memory the GPU reads over PCIe",
                grant.size / MIB,
                grant.class
            ));
        }
        if first || moved {
            self.say(self.counters().line());
        }
    }

    fn note_counters(&self) {
        self.say(self.counters().line());
    }

    /// A fresh note of the counters, for the trace to end on.
    pub fn report(&self) {
        self.note_counters();
    }

    /// Called once a frame: notes the counters when 30 seconds have passed and they moved by 64 MiB or more.
    pub fn tick(&self) {
        let now = std::time::Instant::now();
        {
            let mut noting = self.noting.lock().unwrap_or_else(PoisonError::into_inner);
            if noting
                .last
                .is_some_and(|t| now.duration_since(t).as_secs() < 30)
            {
                return;
            }
            let c = self.counters();
            let l = noting.last_counters;
            let moved = c.local_total().abs_diff(l.local_total())
                + c.spilled_total().abs_diff(l.spilled_total())
                + c.deferred.abs_diff(l.deferred)
                + c.used.abs_diff(l.used);
            noting.last = Some(now);
            if moved < 64 * MIB {
                return;
            }
            noting.last_counters = c;
        }
        self.note_counters();
    }

    /// Notes what the budget is made of, once the notes have somewhere to go.
    pub fn announce(&self) {
        let limit = self.limit.load(Ordering::Relaxed);
        self.say(format!(
            "vram: device-local heaps {} MiB, VK_EXT_memory_budget {}, VK_EXT_memory_priority {}, VK_EXT_pageable_device_local_memory {}{}",
            self.local_heap_size / MIB,
            if self.extensions.budget { "on" } else { "off" },
            if self.extensions.priority { "on" } else { "off" },
            if self.extensions.pageable { "on" } else { "off" },
            if limit == u64::MAX {
                String::new()
            } else {
                format!(", limited to {} MiB by LEONIDA_VRAM_LIMIT_MB", limit / MIB)
            }
        ));
        self.say(format!(
            "vram: the game's memory may take {} MiB of device-local memory, the rest of it is read from system              memory over PCIe",
            self.mirror_cap() / MIB
        ));
        self.note_counters();
    }
}
