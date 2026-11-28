//! The console device: `D3D12XboxCreateDevice`'s object.
//!
//! Slots follow `d3d12_slots.txt`: PC order for the public chain, the measured console methods where they
//! differ (`CreateCommandQueueX` at 48, `CreateCommandListX` at 73, frame events at 78 to 81), struct
//! returns with the buffer first, and a trap for every slot nothing implements. Methods that belong to the
//! resource, descriptor and pipeline steps are installed through `Integration::device_methods`.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::com::{
    self, E_INVALIDARG, E_NOINTERFACE, E_POINTER, Guid, Kind, S_OK, add_ref, release, state,
};
use crate::heap::{self, FixedWindow, HeapInfo, HeapPlacement, HeapRegistry};
use crate::list::{self, NoUploadMemory, UploadMemory};
use crate::queue::{self, CountingExecutor, Executor, HostTimelines, TimelineFactory};

// integration

/// The console's frame-pacing calls, device slots 78 to 81. The present step implements them over its
/// presenter and the pacing code; the default has no display and completes at once.
pub trait FrameEvents: Send + Sync {
    /// `SetFrameIntervalX(output, interval_us, period, flags)`.
    fn set_interval(&self, output: usize, interval_us: u32, period: usize, flags: u32) -> u32;
    /// `ScheduleFrameEventX(type, offset_us, ancillary, flags)`.
    fn schedule(&self, event_type: u32, offset_us: u32, ancillary: usize, flags: u32) -> u32;
    /// `WaitFrameEventX(type, timeout_ms, ancillary, flags, token)`; writes the frame token.
    /// # Safety
    /// `token` is writable or null.
    unsafe fn wait(
        &self,
        event_type: u32,
        timeout_ms: u32,
        ancillary: usize,
        flags: u32,
        token: *mut u64,
    ) -> u32;
    /// `GetFrameStatisticsX(token, type_mask, count, statistics)`.
    /// # Safety
    /// `count` and `stats` are writable or null.
    unsafe fn statistics(
        &self,
        token: u64,
        type_mask: u32,
        count: *mut u32,
        stats: *mut c_void,
    ) -> u32;
}

/// Frames that never wait: the token counts up.
#[derive(Default)]
pub struct ImmediateFrames {
    token: AtomicU64,
}

impl FrameEvents for ImmediateFrames {
    fn set_interval(&self, _o: usize, _i: u32, _p: usize, _f: u32) -> u32 {
        S_OK
    }

    fn schedule(&self, _t: u32, _o: u32, _a: usize, _f: u32) -> u32 {
        S_OK
    }

    unsafe fn wait(&self, _t: u32, _timeout: u32, _a: usize, _f: u32, token: *mut u64) -> u32 {
        if !token.is_null() {
            // SAFETY: the title's out parameter.
            unsafe { *token = self.token.fetch_add(1, Ordering::Relaxed) + 1 };
        }
        S_OK
    }

    unsafe fn statistics(
        &self,
        _token: u64,
        _mask: u32,
        count: *mut u32,
        _stats: *mut c_void,
    ) -> u32 {
        if !count.is_null() {
            // SAFETY: the title's out parameter.
            unsafe { *count = 0 };
        }
        com::S_FALSE
    }
}

/// What the other steps plug in. `Default` is a device that runs with no renderer behind it.
pub struct Integration {
    /// Replays closed lists (execute step).
    pub executor: Arc<dyn Executor>,
    /// Fence values (a Vulkan timeline semaphore in the renderer).
    pub timelines: Arc<dyn TimelineFactory>,
    /// Where descriptor heaps lie.
    pub placement: Arc<dyn HeapPlacement>,
    /// Frame events (present step).
    pub frame_events: Arc<dyn FrameEvents>,
    /// GPU-addressable upload memory (memory step).
    pub upload: Arc<dyn UploadMemory>,
    /// Device methods `(slot, extern "win64" function)` that replace the default: resources, descriptors,
    /// pipelines and root signatures. They receive the device object as `this` and reach the shared state
    /// through `device_core`.
    pub device_methods: Vec<(usize, usize)>,
    /// Extra queue methods, such as `PresentX`, `SuspendX` and `ResumeX`.
    pub queue_methods: Vec<(usize, usize)>,
    /// Extra or replacement command list methods.
    pub list_methods: Vec<(usize, usize)>,
}

impl Default for Integration {
    fn default() -> Self {
        Integration {
            executor: Arc::new(CountingExecutor::default()),
            timelines: Arc::new(HostTimelines),
            placement: Arc::new(FixedWindow::default()),
            frame_events: Arc::new(ImmediateFrames::default()),
            upload: Arc::new(NoUploadMemory),
            device_methods: Vec::new(),
            queue_methods: Vec::new(),
            list_methods: Vec::new(),
        }
    }
}

/// State shared by a device and everything it creates.
pub struct DeviceCore {
    pub integration: Integration,
    pub heaps: HeapRegistry,
    pub removed_reason: AtomicU32,
    vtables: Mutex<HashMap<Kind, usize>>,
}

impl DeviceCore {
    pub fn new(integration: Integration) -> DeviceCore {
        DeviceCore {
            integration,
            heaps: HeapRegistry::default(),
            removed_reason: AtomicU32::new(0),
            vtables: Mutex::new(HashMap::new()),
        }
    }

    /// The vtable for a kind, built once per device.
    pub fn cached_vtable(&self, kind: Kind, len: usize, known: &[(usize, usize)]) -> *const usize {
        let mut tables = self.vtables.lock().expect("no panic while held");
        *tables
            .entry(kind)
            .or_insert_with(|| com::object_vtable(len, known) as usize) as *const usize
    }

    pub fn remember_heap(&self, info: HeapInfo) {
        self.heaps.insert(info);
    }

    pub fn forget_heap(&self, gpu: u64, _cpu: u64) {
        self.heaps.remove(gpu);
    }
}

pub struct Device {
    pub core: Arc<DeviceCore>,
}

impl Drop for Device {
    fn drop(&mut self) {
        crate::assembly::state::device_released(&self.core);
    }
}

/// The shared state of a device, for the methods of other steps.
///
/// # Safety
/// `this` is a device object made by `create_device`, alive for `'a`.
pub unsafe fn device_core<'a>(this: *mut c_void) -> &'a Arc<DeviceCore> {
    // SAFETY: as the caller promises.
    unsafe { &state::<Device>(this).core }
}

/// The pointer for `D3D12XboxCreateDevice(adapter, params, riid, out)`. `params` is the 24-word block the
/// executable passes; its meaning is mostly unmeasured, so it is only noted.
///
/// # Safety
/// `params` is null or 24 readable words; `riid` and `out` as COM passes them.
pub unsafe fn create_device(
    params: *const u32,
    riid: *const u8,
    out: *mut *mut c_void,
    integration: Integration,
) -> u32 {
    if out.is_null() {
        return E_POINTER;
    }
    let words = if params.is_null() {
        "none".to_string()
    } else {
        // SAFETY: as the caller promises.
        unsafe { std::slice::from_raw_parts(params, 24) }
            .iter()
            .map(|w| format!("{w:08x}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let wanted = if riid.is_null() {
        "null".to_string()
    } else {
        // SAFETY: COM passes a 16-byte IID.
        unsafe { Guid::read(riid) }.to_string()
    };
    com::note(format!(
        "D3D12XboxCreateDevice asked for {wanted} with parameters {words}"
    ));
    let core = Arc::new(DeviceCore::new(integration));
    let object = com::new_object(Kind::Device, device_vtable(&core), Device { core });
    // SAFETY: just allocated; the reference goes to the caller, `riid` decides what it sees.
    unsafe {
        // An IID the layer does not know is the console's own device interface: the device itself answers.
        if riid.is_null() || !com::is_known_iid(&Guid::read(riid)) {
            *out = object as *mut c_void;
            return S_OK;
        }
        let result = d_query_interface(object as *mut c_void, riid, out);
        release(object as *mut c_void);
        result
    }
}

unsafe extern "win64" fn d_query_interface(
    this: *mut c_void,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    if out.is_null() {
        return E_POINTER;
    }
    if riid.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: COM passes a 16-byte IID; `this` is a device.
    unsafe {
        let wanted = Guid::read(riid);
        if com::base_iids(Kind::Device).contains(&wanted) {
            return com::query_interface_for(Kind::Device, this, riid, out);
        }
        if com::is_known_iid(&wanted) {
            *out = std::ptr::null_mut();
            com::note_once(format!(
                "ID3D12Device asked for interface {wanted}; refused"
            ));
            return E_NOINTERFACE;
        }
        com::note(format!(
            "ID3D12Device asked for interface {wanted}; answered by a trap"
        ));
        let label: &'static str = Box::leak(format!("ID3D12Device as {wanted}").into_boxed_str());
        *out = com::trap_object(label);
    }
    S_OK
}

unsafe extern "win64" fn d_get_node_count(_this: *mut c_void) -> u32 {
    1
}

unsafe extern "win64" fn d_create_command_queue(
    this: *mut c_void,
    desc: *const u32,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    // SAFETY: a device; COM arguments.
    unsafe { queue::create_queue(this, device_core(this), desc, riid, out) }
}

unsafe extern "win64" fn d_create_command_allocator(
    this: *mut c_void,
    list_type: u32,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    // SAFETY: a device; COM arguments.
    unsafe { list::create_allocator(this, device_core(this), list_type, riid, out) }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "win64" fn d_create_command_list(
    this: *mut c_void,
    node_mask: u32,
    list_type: u32,
    allocator: *mut c_void,
    pso: *mut c_void,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    if node_mask > 1 {
        return E_INVALIDARG;
    }
    // SAFETY: a device; COM arguments.
    unsafe {
        list::create_list(
            this,
            device_core(this),
            list_type,
            allocator,
            pso,
            riid,
            out,
            false,
        )
    }
}

/// Slot 73: `(desc, allocator, pso, riid, out)`; `desc` is 16 bytes and its first word is the list type.
unsafe extern "win64" fn d_create_command_list_x(
    this: *mut c_void,
    desc: *const u32,
    allocator: *mut c_void,
    pso: *mut c_void,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    if desc.is_null() {
        return E_POINTER;
    }
    // SAFETY: a device; `desc` has 16 readable bytes; COM arguments.
    unsafe {
        let list_type = *desc;
        list::create_list(
            this,
            device_core(this),
            list_type,
            allocator,
            pso,
            riid,
            out,
            true,
        )
    }
}

/// Slot 48: `(desc, riid, out)`; `desc` starts with the PC `D3D12_COMMAND_QUEUE_DESC` and carries console
/// words after it.
unsafe extern "win64" fn d_create_command_queue_x(
    this: *mut c_void,
    desc: *const u32,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    // SAFETY: as `d_create_command_queue`.
    unsafe { queue::create_queue(this, device_core(this), desc, riid, out) }
}

const FEATURE_OPTIONS: u32 = 0;
const FEATURE_ARCHITECTURE: u32 = 1;
const FEATURE_SHADER_MODEL: u32 = 7;
const FEATURE_GPU_VIRTUAL_ADDRESS_SUPPORT: u32 = 6;

/// Answers the features the title asks first. `D3D12_OPTIONS` is asked with 0x40 bytes where the PC
/// structure has 0x3C: the tail is zeroed. Other features fail with `E_INVALIDARG` and a note.
unsafe extern "win64" fn d_check_feature_support(
    _this: *mut c_void,
    feature: u32,
    data: *mut u8,
    size: u32,
) -> u32 {
    if data.is_null() {
        return E_INVALIDARG;
    }
    let fill = |words: &[u32]| -> u32 {
        if size < (words.len() * 4) as u32 {
            return E_INVALIDARG;
        }
        // SAFETY: `size` writable bytes, checked against the words written.
        unsafe {
            std::ptr::write_bytes(data, 0, size as usize);
            std::ptr::copy_nonoverlapping(words.as_ptr() as *const u8, data, words.len() * 4);
        }
        S_OK
    };
    match feature {
        // double ops, logic op, min precision, tiled tier 3, binding tier 3, PS stencil ref, typed UAV loads,
        // ROVs, conservative raster tier 3, 40 VA bits, standard swizzle, cross node 0, cross adapter, VP/RT
        // index, resource heap tier 2
        FEATURE_OPTIONS if size == 0x3C || size == 0x40 => {
            fill(&[1, 1, 0, 3, 3, 1, 1, 1, 3, 40, 0, 0, 0, 1, 2])
        }
        // node 0, not tile based, UMA, cache coherent UMA
        FEATURE_ARCHITECTURE => {
            // SAFETY: the structure's first word is the node index the title passes in; it is kept.
            let node = unsafe { *(data as *const u32) };
            fill(&[node, 0, 1, 1])
        }
        FEATURE_SHADER_MODEL => {
            // SAFETY: `HighestShaderModel` is an in/out word.
            let asked = unsafe { *(data as *const u32) };
            fill(&[asked.min(0x66)])
        }
        FEATURE_GPU_VIRTUAL_ADDRESS_SUPPORT => fill(&[40, 44]),
        other => {
            com::note_once(format!(
                "CheckFeatureSupport feature {other} with {size} bytes is not answered"
            ));
            E_INVALIDARG
        }
    }
}

unsafe extern "win64" fn d_create_descriptor_heap(
    this: *mut c_void,
    desc: *const u32,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    // SAFETY: a device; COM arguments.
    unsafe { heap::create(this, device_core(this), desc, riid, out) }
}

unsafe extern "win64" fn d_descriptor_increment(_this: *mut c_void, heap_type: u32) -> u32 {
    heap::increment_size(heap_type)
}

/// Slot 26: a 20-byte `D3D12_HEAP_PROPERTIES`, returned as `(buffer, this, node_mask, heap_type)`.
unsafe extern "win64" fn d_custom_heap_properties(
    buffer: *mut u32,
    _this: *mut c_void,
    node_mask: u32,
    heap_type: u32,
) -> *mut u32 {
    // The properties the standard heap types stand for on a unified-memory machine: CPU page property
    // (1 not available, 2 write combine, 3 write back) and memory pool (1 system, 2 video).
    let (page, pool) = match heap_type {
        2 => (2, 1),
        3 => (3, 1),
        _ => (1, 2),
    };
    // SAFETY: `buffer` has room for 20 bytes.
    unsafe {
        *buffer = 4;
        *buffer.add(1) = page;
        *buffer.add(2) = pool;
        *buffer.add(3) = node_mask.max(1);
        *buffer.add(4) = node_mask.max(1);
    }
    buffer
}

unsafe extern "win64" fn d_create_fence(
    this: *mut c_void,
    initial: u64,
    flags: u32,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    // SAFETY: a device; COM arguments.
    unsafe { queue::create_fence(this, device_core(this), initial, flags, riid, out) }
}

unsafe extern "win64" fn d_removed_reason(this: *mut c_void) -> u32 {
    // SAFETY: a device of this layer.
    unsafe { device_core(this).removed_reason.load(Ordering::Acquire) }
}

unsafe extern "win64" fn d_set_stable_power_state(_this: *mut c_void, _enable: i32) -> u32 {
    S_OK
}

/// The adapter's LUID, 8 bytes: in RAX, as the console returns structs that small.
unsafe extern "win64" fn d_adapter_luid(_this: *mut c_void) -> u64 {
    0
}

unsafe extern "win64" fn d_set_frame_interval(
    this: *mut c_void,
    output: usize,
    interval_us: u32,
    period: usize,
    flags: u32,
) -> u32 {
    // SAFETY: a device of this layer.
    unsafe {
        device_core(this)
            .integration
            .frame_events
            .set_interval(output, interval_us, period, flags)
    }
}

unsafe extern "win64" fn d_schedule_frame_event(
    this: *mut c_void,
    event_type: u32,
    offset_us: u32,
    ancillary: usize,
    flags: u32,
) -> u32 {
    // SAFETY: a device of this layer.
    unsafe {
        device_core(this)
            .integration
            .frame_events
            .schedule(event_type, offset_us, ancillary, flags)
    }
}

unsafe extern "win64" fn d_wait_frame_event(
    this: *mut c_void,
    event_type: u32,
    timeout_ms: u32,
    ancillary: usize,
    flags: u32,
    token: *mut u64,
) -> u32 {
    // SAFETY: a device of this layer.
    unsafe {
        device_core(this)
            .integration
            .frame_events
            .wait(event_type, timeout_ms, ancillary, flags, token)
    }
}

unsafe extern "win64" fn d_frame_statistics(
    this: *mut c_void,
    token: u64,
    type_mask: u32,
    count: *mut u32,
    stats: *mut c_void,
) -> u32 {
    // SAFETY: a device of this layer.
    unsafe {
        device_core(this)
            .integration
            .frame_events
            .statistics(token, type_mask, count, stats)
    }
}

fn device_vtable(core: &DeviceCore) -> *const usize {
    macro_rules! f {
        ($f:expr) => {
            $f as *const () as usize
        };
    }
    let mut known = vec![
        (0, f!(d_query_interface)),
        (1, f!(add_ref)),
        (2, f!(release)),
        (7, f!(d_get_node_count)),
        (8, f!(d_create_command_queue)),
        (9, f!(d_create_command_allocator)),
        (12, f!(d_create_command_list)),
        (13, f!(d_check_feature_support)),
        (14, f!(d_create_descriptor_heap)),
        (15, f!(d_descriptor_increment)),
        (26, f!(d_custom_heap_properties)),
        (36, f!(d_create_fence)),
        (37, f!(d_removed_reason)),
        (40, f!(d_set_stable_power_state)),
        (43, f!(d_adapter_luid)),
        (48, f!(d_create_command_queue_x)),
        (73, f!(d_create_command_list_x)),
        (78, f!(d_set_frame_interval)),
        (79, f!(d_schedule_frame_event)),
        (80, f!(d_wait_frame_event)),
        (81, f!(d_frame_statistics)),
    ];
    known.extend_from_slice(&core.integration.device_methods);
    core.cached_vtable(Kind::Device, 256, &known)
}
