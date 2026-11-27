//! Command queues and fences.
//!
//! A queue runs its operations in order on a worker of its own: `ExecuteCommandLists` hands the closed
//! lists to the `Executor`, `Signal` and `Wait` act on `Timeline`s (a fence's value), and anything the
//! present step needs ordered behind earlier work goes through `enqueue`. A fence is a `Timeline`; the
//! default one lives in host memory, and a Vulkan timeline semaphore implements the same trait.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::com::{
    self, E_FAIL, E_INVALIDARG, E_NOTIMPL, E_POINTER, Kind, S_OK, add_ref, release, state,
};
use crate::device::DeviceCore;
use crate::stream::CommandStream;

pub const TYPE_DIRECT: u32 = 0;
pub const TYPE_COMPUTE: u32 = 2;
pub const TYPE_COPY: u32 = 3;

// integration

/// A fence's value: what the CPU waits on and the GPU signals. `crates/gpu` gives the Vulkan one (a timeline
/// semaphore plus a thread that mirrors its counter into `cell`).
pub trait Timeline: Send + Sync {
    /// The completed value.
    fn value(&self) -> u64;
    /// Sets the value from the CPU.
    fn signal(&self, value: u64);
    /// Blocks until the value reaches `value`; false when `abort` was raised first.
    fn wait(&self, value: u64, abort: &AtomicBool) -> bool;
    /// Runs `f` once the value reaches `value`, at once when it has.
    fn on_reached(&self, value: u64, f: Box<dyn FnOnce() + Send>);
    /// The completed value in memory the title can read (console fence slot 12 hands out its address).
    fn cell(&self) -> &AtomicU64;
}

pub trait TimelineFactory: Send + Sync {
    fn create(&self, initial: u64) -> Arc<dyn Timeline>;
}

type Waiter = (u64, Box<dyn FnOnce() + Send>);

/// A fence in host memory.
#[derive(Default)]
pub struct HostTimeline {
    value: AtomicU64,
    waiters: Mutex<Vec<Waiter>>,
    changed: Condvar,
    gate: Mutex<()>,
}

impl HostTimeline {
    pub fn new(initial: u64) -> HostTimeline {
        HostTimeline {
            value: AtomicU64::new(initial),
            ..Default::default()
        }
    }
}

impl Timeline for HostTimeline {
    fn value(&self) -> u64 {
        self.value.load(Ordering::Acquire)
    }

    fn signal(&self, value: u64) {
        self.value.store(value, Ordering::Release);
        let ready: Vec<_> = {
            let mut waiters = self.waiters.lock().expect("no panic while held");
            let (ready, rest) = std::mem::take(&mut *waiters)
                .into_iter()
                .partition(|(v, _)| *v <= value);
            *waiters = rest;
            ready
        };
        drop(self.gate.lock());
        self.changed.notify_all();
        for (_, f) in ready {
            f();
        }
    }

    fn wait(&self, value: u64, abort: &AtomicBool) -> bool {
        let mut guard = self.gate.lock().expect("no panic while held");
        loop {
            if self.value() >= value {
                return true;
            }
            if abort.load(Ordering::Acquire) {
                return false;
            }
            guard = self
                .changed
                .wait_timeout(guard, Duration::from_millis(5))
                .expect("no panic while held")
                .0;
        }
    }

    fn on_reached(&self, value: u64, f: Box<dyn FnOnce() + Send>) {
        let mut waiters = self.waiters.lock().expect("no panic while held");
        if self.value() >= value {
            drop(waiters);
            f();
        } else {
            waiters.push((value, f));
        }
    }

    fn cell(&self) -> &AtomicU64 {
        &self.value
    }
}

#[derive(Default)]
pub struct HostTimelines;

impl TimelineFactory for HostTimelines {
    fn create(&self, initial: u64) -> Arc<dyn Timeline> {
        Arc::new(HostTimeline::new(initial))
    }
}

/// A queue as the executor sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueueInfo {
    pub id: u32,
    pub queue_type: u32,
    pub priority: i32,
    pub flags: u32,
    pub node_mask: u32,
}

/// A `UpdateTileMappings` or `CopyTileMappings` call, copied.
#[derive(Debug)]
pub enum TileCall {
    Update {
        resource: com::ComPtr,
        heap: Option<com::ComPtr>,
        coordinates: Vec<[u32; 4]>,
        sizes: Vec<[u32; 4]>,
        range_flags: Vec<u32>,
        heap_range_starts: Vec<u32>,
        range_tile_counts: Vec<u32>,
        flags: u32,
    },
    Copy {
        dst: com::ComPtr,
        dst_coordinate: [u32; 4],
        src: com::ComPtr,
        src_coordinate: [u32; 4],
        size: [u32; 4],
        flags: u32,
    },
}

/// What replays a queue's lists: the execute step's `Executor` over the renderer.
pub trait Executor: Send + Sync {
    /// Runs the lists in order; returns when they have been submitted.
    fn execute(&self, queue: &QueueInfo, lists: &[Arc<CommandStream>]);

    fn tile_mappings(&self, queue: &QueueInfo, call: TileCall) {
        let _ = (queue, call);
        com::note_once("tile mappings were recorded but no executor applies them");
    }
}

/// Accepts the lists and keeps only a count. The default until the execute step is wired.
#[derive(Default)]
pub struct CountingExecutor {
    pub lists: AtomicU64,
    pub commands: AtomicU64,
}

impl Executor for CountingExecutor {
    fn execute(&self, _queue: &QueueInfo, lists: &[Arc<CommandStream>]) {
        self.lists.fetch_add(lists.len() as u64, Ordering::Relaxed);
        for list in lists {
            self.commands
                .fetch_add(list.commands.len() as u64, Ordering::Relaxed);
        }
        com::note_once("command lists were submitted but no executor replays them yet");
    }
}

enum Op {
    Execute(Vec<Arc<CommandStream>>),
    Signal(Arc<dyn Timeline>, u64),
    Wait(Arc<dyn Timeline>, u64),
    Tiles(Box<TileCall>),
    Custom(Box<dyn FnOnce() + Send>),
    Flush(Sender<()>),
    Shutdown,
}

pub struct Queue {
    device: com::ComPtr,
    pub info: QueueInfo,
    tx: Mutex<Sender<Op>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    abort: Arc<AtomicBool>,
    started: Instant,
}

fn run_worker(
    rx: Receiver<Op>,
    executor: Arc<dyn Executor>,
    info: QueueInfo,
    abort: Arc<AtomicBool>,
) {
    while let Ok(op) = rx.recv() {
        match op {
            Op::Execute(lists) => executor.execute(&info, &lists),
            Op::Signal(timeline, value) => timeline.signal(value),
            Op::Wait(timeline, value) => {
                if !timeline.wait(value, &abort) {
                    return;
                }
            }
            Op::Tiles(call) => executor.tile_mappings(&info, *call),
            Op::Custom(f) => f(),
            Op::Flush(done) => {
                let _ = done.send(());
            }
            Op::Shutdown => return,
        }
    }
}

impl Queue {
    fn send(&self, op: Op) {
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send(op);
        }
    }

    /// Waits until everything submitted so far has run.
    pub fn flush(&self) {
        let (tx, rx) = channel();
        self.send(Op::Flush(tx));
        let _ = rx.recv();
    }
}

impl Drop for Queue {
    fn drop(&mut self) {
        self.abort.store(true, Ordering::Release);
        self.send(Op::Shutdown);
        if let Some(worker) = self.worker.lock().ok().and_then(|mut w| w.take()) {
            let _ = worker.join();
        }
    }
}

static NEXT_QUEUE: AtomicU32 = AtomicU32::new(1);

/// Creates a queue from the 16-byte `D3D12_COMMAND_QUEUE_DESC` prefix (the console's `desc` carries
/// further words after it; they are not read).
///
/// # Safety
/// `device` is the calling device; `desc` is 16 readable bytes or null; `riid` and `out` as COM passes them.
pub unsafe fn create_queue(
    device: *mut c_void,
    core: &Arc<DeviceCore>,
    desc: *const u32,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    if out.is_null() || desc.is_null() {
        return E_POINTER;
    }
    // SAFETY: 16 readable bytes, as the caller promises.
    let words = unsafe { std::slice::from_raw_parts(desc, 4) };
    let (queue_type, priority, flags, node_mask) = (words[0], words[1] as i32, words[2], words[3]);
    if !matches!(queue_type, TYPE_DIRECT | TYPE_COMPUTE | TYPE_COPY) {
        com::note_once(format!("CreateCommandQueue refused type {queue_type}"));
        return E_INVALIDARG;
    }
    let info = QueueInfo {
        id: NEXT_QUEUE.fetch_add(1, Ordering::Relaxed),
        queue_type,
        priority,
        flags,
        node_mask,
    };
    let (tx, rx) = channel();
    let abort = Arc::new(AtomicBool::new(false));
    let executor = core.integration.executor.clone();
    let worker_abort = abort.clone();
    let worker = std::thread::Builder::new()
        .name(format!("d3d12-queue-{}", info.id))
        .spawn(move || run_worker(rx, executor, info, worker_abort));
    let Ok(worker) = worker else {
        return E_FAIL;
    };
    // SAFETY: `device` is the device that called.
    let Some(device_ref) = (unsafe { com::ComPtr::add_ref(device) }) else {
        return E_POINTER;
    };
    let object = com::new_object(
        Kind::CommandQueue,
        queue_vtable(core),
        Queue {
            device: device_ref,
            info,
            tx: Mutex::new(tx),
            worker: Mutex::new(Some(worker)),
            abort,
            started: Instant::now(),
        },
    );
    // SAFETY: the creation reference is handed out.
    unsafe { com::hand_out(object as *mut c_void, riid, out) }
}

/// The queue state behind a COM pointer.
///
/// # Safety
/// `this` is a command queue of this layer, alive for `'a`.
pub unsafe fn queue_of<'a>(this: *mut c_void) -> Option<&'a Queue> {
    // SAFETY: as the caller promises.
    unsafe { (com::kind_of(this)? == Kind::CommandQueue).then(|| state::<Queue>(this)) }
}

/// Runs `f` on the queue's worker after everything submitted before it (what `PresentX` needs to follow the
/// frame's work). Returns false when `this` is not a queue.
///
/// # Safety
/// `this` is a command queue of this layer.
pub unsafe fn enqueue(this: *mut c_void, f: Box<dyn FnOnce() + Send>) -> bool {
    // SAFETY: as the caller promises.
    match unsafe { queue_of(this) } {
        Some(queue) => {
            queue.send(Op::Custom(f));
            true
        }
        None => false,
    }
}

unsafe extern "win64" fn q_query_interface(
    this: *mut c_void,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    // SAFETY: slot 0, COM arguments.
    unsafe { com::query_interface_for(Kind::CommandQueue, this, riid, out) }
}

unsafe extern "win64" fn q_get_device(
    this: *mut c_void,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    // SAFETY: `this` is a queue; it holds its device alive.
    unsafe { com::query(state::<Queue>(this).device.as_ptr(), riid, out) }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "win64" fn q_update_tile_mappings(
    this: *mut c_void,
    resource: *mut c_void,
    regions: u32,
    coordinates: *const [u32; 4],
    sizes: *const [u32; 4],
    heap: *mut c_void,
    ranges: u32,
    range_flags: *const u32,
    heap_range_starts: *const u32,
    range_tile_counts: *const u32,
    flags: u32,
) {
    // SAFETY: a queue of this layer; the arrays are `regions` and `ranges` long as D3D12 requires.
    unsafe {
        let queue = state::<Queue>(this);
        let Some(resource) = com::ComPtr::add_ref(resource) else {
            return;
        };
        let array = |p: *const u32, n: u32, default: u32| -> Vec<u32> {
            if p.is_null() {
                vec![default; n as usize]
            } else {
                std::slice::from_raw_parts(p, n as usize).to_vec()
            }
        };
        let blocks = |p: *const [u32; 4], n: u32| -> Vec<[u32; 4]> {
            if p.is_null() {
                Vec::new()
            } else {
                std::slice::from_raw_parts(p, n as usize).to_vec()
            }
        };
        let call = TileCall::Update {
            resource,
            heap: com::ComPtr::add_ref(heap),
            coordinates: blocks(coordinates, regions),
            sizes: blocks(sizes, regions),
            range_flags: array(range_flags, ranges, 0),
            heap_range_starts: array(heap_range_starts, ranges, 0),
            range_tile_counts: array(range_tile_counts, ranges, 1),
            flags,
        };
        queue.send(Op::Tiles(Box::new(call)));
    }
}

unsafe extern "win64" fn q_copy_tile_mappings(
    this: *mut c_void,
    dst: *mut c_void,
    dst_coordinate: *const [u32; 4],
    src: *mut c_void,
    src_coordinate: *const [u32; 4],
    size: *const [u32; 4],
    flags: u32,
) {
    // SAFETY: a queue of this layer; the structs are 16 bytes each.
    unsafe {
        let queue = state::<Queue>(this);
        if dst_coordinate.is_null() || src_coordinate.is_null() || size.is_null() {
            return;
        }
        let (Some(dst), Some(src)) = (com::ComPtr::add_ref(dst), com::ComPtr::add_ref(src)) else {
            return;
        };
        queue.send(Op::Tiles(Box::new(TileCall::Copy {
            dst,
            dst_coordinate: *dst_coordinate,
            src,
            src_coordinate: *src_coordinate,
            size: *size,
            flags,
        })));
    }
}

unsafe extern "win64" fn q_execute_command_lists(
    this: *mut c_void,
    count: u32,
    lists: *const *mut c_void,
) {
    if lists.is_null() {
        return;
    }
    // SAFETY: a queue of this layer; `lists` has `count` entries.
    unsafe {
        let queue = state::<Queue>(this);
        let mut streams = Vec::with_capacity(count as usize);
        for &list in std::slice::from_raw_parts(lists, count as usize) {
            match crate::list::closed_stream(list) {
                Some(stream) => streams.push(stream),
                None => com::note_once(
                    "ExecuteCommandLists was given a list that is open or not a command list; skipped",
                ),
            }
        }
        if !streams.is_empty() {
            queue.send(Op::Execute(streams));
        }
    }
}

/// Console slot 35: `(count, lists, token, flags)`. The token comes from `WaitFrameEventX`; the flags (3 for a
/// batch, 0 for a single list) are not measured beyond that, and the lists run in order either way.
unsafe extern "win64" fn q_execute_command_lists_x(
    this: *mut c_void,
    count: u32,
    lists: *const *mut c_void,
    _token: u64,
    _flags: u32,
) -> u32 {
    // SAFETY: the same call as the plain slot.
    unsafe { q_execute_command_lists(this, count, lists) };
    S_OK
}

/// Console slot 31: `(count, ranges, handle, dword, 0)`, called right after `XMemMapPhysicalPagesScatter`. What it
/// does is not measured, so it succeeds and says so once.
unsafe extern "win64" fn q_map_tiles_x(
    _this: *mut c_void,
    count: u32,
    _ranges: *const c_void,
    _handle: *const c_void,
    _value: u32,
    _zero: u32,
) -> u32 {
    com::note_once(format!(
        "ID3D12CommandQueue slot 31 (tile mapping, {count} ranges): meaning not measured, ignored"
    ));
    S_OK
}

unsafe extern "win64" fn q_marker(
    _this: *mut c_void,
    _metadata: u32,
    _data: *const c_void,
    _size: u32,
) {
}

unsafe extern "win64" fn q_end_event(_this: *mut c_void) {}

unsafe extern "win64" fn q_signal(this: *mut c_void, fence: *mut c_void, value: u64) -> u32 {
    // SAFETY: a queue of this layer; `fence` is checked.
    unsafe {
        let Some(fence) = fence_of(fence) else {
            return E_INVALIDARG;
        };
        state::<Queue>(this).send(Op::Signal(fence.timeline.clone(), value));
    }
    S_OK
}

unsafe extern "win64" fn q_wait(this: *mut c_void, fence: *mut c_void, value: u64) -> u32 {
    // SAFETY: a queue of this layer; `fence` is checked.
    unsafe {
        let Some(fence) = fence_of(fence) else {
            return E_INVALIDARG;
        };
        state::<Queue>(this).send(Op::Wait(fence.timeline.clone(), value));
    }
    S_OK
}

/// GPU timestamps tick in nanoseconds here; the console's rate was not measured.
const TIMESTAMP_FREQUENCY: u64 = 1_000_000_000;

unsafe extern "win64" fn q_timestamp_frequency(_this: *mut c_void, out: *mut u64) -> u32 {
    if out.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: `out` is writable.
    unsafe { *out = TIMESTAMP_FREQUENCY };
    S_OK
}

unsafe extern "win64" fn q_clock_calibration(
    this: *mut c_void,
    gpu: *mut u64,
    cpu: *mut u64,
) -> u32 {
    if gpu.is_null() || cpu.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: a queue of this layer; outputs are writable.
    unsafe {
        let ticks = state::<Queue>(this).started.elapsed().as_nanos() as u64;
        *gpu = ticks;
        *cpu = ticks;
    }
    S_OK
}

/// `GetDesc`: a 16-byte struct, buffer first on the console.
unsafe extern "win64" fn q_get_desc(buffer: *mut u32, this: *mut c_void) -> *mut u32 {
    // SAFETY: a queue of this layer; `buffer` has room for 16 bytes.
    unsafe {
        let info = state::<Queue>(this).info;
        *buffer = info.queue_type;
        *buffer.add(1) = info.priority as u32;
        *buffer.add(2) = info.flags;
        *buffer.add(3) = info.node_mask;
    }
    buffer
}

fn queue_vtable(core: &DeviceCore) -> *const usize {
    let mut known = vec![
        (0, q_query_interface as *const () as usize),
        (1, add_ref as *const () as usize),
        (2, release as *const () as usize),
        (7, q_get_device as *const () as usize),
        (8, q_update_tile_mappings as *const () as usize),
        (9, q_copy_tile_mappings as *const () as usize),
        (10, q_execute_command_lists as *const () as usize),
        (11, q_marker as *const () as usize),
        (12, q_marker as *const () as usize),
        (13, q_end_event as *const () as usize),
        (16, q_signal as *const () as usize),
        (17, q_wait as *const () as usize),
        (18, q_timestamp_frequency as *const () as usize),
        (19, q_clock_calibration as *const () as usize),
        (20, q_get_desc as *const () as usize),
        (31, q_map_tiles_x as *const () as usize),
        (35, q_execute_command_lists_x as *const () as usize),
    ];
    known.extend_from_slice(&core.integration.queue_methods);
    core.cached_vtable(Kind::CommandQueue, 64, &known)
}

// Fences

pub struct Fence {
    device: com::ComPtr,
    pub timeline: Arc<dyn Timeline>,
    pub flags: u32,
}

/// The fence behind a COM pointer.
///
/// # Safety
/// `this` is null or an object of this layer, alive for `'a`.
pub unsafe fn fence_of<'a>(this: *mut c_void) -> Option<&'a Fence> {
    if this.is_null() {
        return None;
    }
    // SAFETY: as the caller promises.
    unsafe { (com::kind_of(this)? == Kind::Fence).then(|| state::<Fence>(this)) }
}

/// `CreateFence(initial, flags, riid, out)`.
///
/// # Safety
/// `device` is the calling device; `riid` and `out` as COM passes them.
pub unsafe fn create_fence(
    device: *mut c_void,
    core: &Arc<DeviceCore>,
    initial: u64,
    flags: u32,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: `device` is the device that called.
    let Some(device_ref) = (unsafe { com::ComPtr::add_ref(device) }) else {
        return E_POINTER;
    };
    let object = com::new_object(
        Kind::Fence,
        fence_vtable(core),
        Fence {
            device: device_ref,
            timeline: core.integration.timelines.create(initial),
            flags,
        },
    );
    // SAFETY: the creation reference is handed out.
    unsafe { com::hand_out(object as *mut c_void, riid, out) }
}

unsafe extern "win64" fn f_query_interface(
    this: *mut c_void,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    // SAFETY: slot 0, COM arguments.
    unsafe { com::query_interface_for(Kind::Fence, this, riid, out) }
}

unsafe extern "win64" fn f_get_device(
    this: *mut c_void,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    // SAFETY: `this` is a fence; it holds its device alive.
    unsafe { com::query(state::<Fence>(this).device.as_ptr(), riid, out) }
}

unsafe extern "win64" fn f_completed_value(this: *mut c_void) -> u64 {
    // SAFETY: a fence of this layer.
    unsafe { state::<Fence>(this).timeline.value() }
}

#[cfg(windows)]
fn set_event(handle: usize) {
    use windows_sys::Win32::System::Threading::SetEvent;
    // SAFETY: the title's event handle, as D3D12 sets it from a thread of its own.
    unsafe { SetEvent(handle as *mut c_void) };
}

#[cfg(not(windows))]
fn set_event(_handle: usize) {}

unsafe extern "win64" fn f_set_event_on_completion(
    this: *mut c_void,
    value: u64,
    event: *mut c_void,
) -> u32 {
    // SAFETY: a fence of this layer.
    let fence = unsafe { state::<Fence>(this) };
    if event.is_null() {
        let never = AtomicBool::new(false);
        fence.timeline.wait(value, &never);
    } else {
        let handle = event as usize;
        fence
            .timeline
            .on_reached(value, Box::new(move || set_event(handle)));
    }
    S_OK
}

unsafe extern "win64" fn f_signal(this: *mut c_void, value: u64) -> u32 {
    // SAFETY: a fence of this layer.
    unsafe { state::<Fence>(this).timeline.signal(value) };
    S_OK
}

unsafe extern "win64" fn f_creation_flags(this: *mut c_void) -> u32 {
    // SAFETY: a fence of this layer.
    unsafe { state::<Fence>(this).flags }
}

/// Console slot 12: the address of the completed value, which the title reads directly.
unsafe extern "win64" fn f_completed_value_address(this: *mut c_void) -> *const u64 {
    // SAFETY: a fence of this layer; the cell lives as long as the fence.
    unsafe { state::<Fence>(this).timeline.cell().as_ptr() as *const u64 }
}

fn fence_vtable(core: &DeviceCore) -> *const usize {
    core.cached_vtable(
        Kind::Fence,
        32,
        &[
            (0, f_query_interface as *const () as usize),
            (1, add_ref as *const () as usize),
            (2, release as *const () as usize),
            (7, f_get_device as *const () as usize),
            (8, f_completed_value as *const () as usize),
            (9, f_set_event_on_completion as *const () as usize),
            (10, f_signal as *const () as usize),
            (11, f_creation_flags as *const () as usize),
            (12, f_completed_value_address as *const () as usize),
        ],
    )
}

/// What `E_NOTIMPL` is for in this file: methods the queue does not have are the trap's.
#[allow(dead_code)]
const _: u32 = E_NOTIMPL;
