//! `XTaskQueue` and `XAsync`: the GDK's work queues and asynchronous call blocks, as `XThreadingImpl` serves them.
//!
//! A queue has a work port and a completion port. A port dispatches on a pool of threads, on one thread, on
//! the submitting thread (`Immediate`) or only when the program calls `XTaskQueueDispatch` (`Manual`). An
//! async call keeps its state behind the program's `XAsyncBlock`: a signature, the final status and a pointer
//! to the state, in the block's 32 `internal` bytes. A provider (a function the call's author wrote, or one
//! of ours) is called with `Begin`, `DoWork`, `GetResult`, `Cancel` and `Cleanup`.

use std::cell::UnsafeCell;
use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use super::{
    E_ABORT, E_INVALIDARG, E_PENDING, E_POINTER, E_UNEXPECTED, HR_INSUFFICIENT_BUFFER,
    HR_NO_TASK_QUEUE, S_OK,
};

const INFINITE: u32 = u32::MAX;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Manual,
    ThreadPool,
    Serialized,
    Immediate,
}

fn mode_of(value: u32) -> Option<Mode> {
    match value {
        0 => Some(Mode::Manual),
        1 => Some(Mode::ThreadPool),
        2 => Some(Mode::Serialized),
        3 => Some(Mode::Immediate),
        _ => None,
    }
}

struct Work {
    context: usize,
    callback: usize,
}

impl Work {
    fn run(&self, canceled: bool) {
        // SAFETY: `callback` is the function address the program submitted, with the callback's shape.
        let callback: unsafe extern "win64" fn(*mut c_void, u8) =
            unsafe { std::mem::transmute(self.callback) };
        // SAFETY: the program's own callback, called as it asked.
        unsafe { callback(self.context as *mut c_void, u8::from(canceled)) };
    }
}

#[derive(Default)]
struct PortState {
    ready: VecDeque<Work>,
    delayed: Vec<(Instant, Work)>,
    running: usize,
    idle: usize,
    threads: usize,
    terminated: bool,
}

impl PortState {
    fn promote(&mut self, now: Instant) {
        if self.delayed.is_empty() {
            return;
        }
        self.delayed.sort_by_key(|(due, _)| *due);
        while self.delayed.first().is_some_and(|(due, _)| *due <= now) {
            let (_, work) = self.delayed.remove(0);
            self.ready.push_back(work);
        }
    }

    fn next_due(&self) -> Option<Instant> {
        self.delayed.iter().map(|(due, _)| *due).min()
    }
}

pub struct Port {
    mode: Mode,
    state: Mutex<PortState>,
    wake: Condvar,
}

impl Port {
    fn new(mode: Mode) -> Arc<Port> {
        Arc::new(Port {
            mode,
            state: Mutex::new(PortState::default()),
            wake: Condvar::new(),
        })
    }

    fn max_threads(&self) -> usize {
        match self.mode {
            Mode::ThreadPool => {
                std::thread::available_parallelism().map_or(2, |n| n.get().clamp(2, 8))
            }
            Mode::Serialized | Mode::Immediate => 1,
            Mode::Manual => 0,
        }
    }

    fn submit(self: &Arc<Port>, work: Work, delay: Duration) -> i32 {
        let mut state = lock(&self.state);
        if state.terminated {
            return E_ABORT;
        }
        if self.mode == Mode::Immediate && delay.is_zero() {
            drop(state);
            work.run(false);
            return S_OK;
        }
        if delay.is_zero() {
            state.ready.push_back(work);
        } else {
            state.delayed.push((Instant::now() + delay, work));
        }
        if self.mode != Mode::Manual && state.idle == 0 && state.threads < self.max_threads() {
            state.threads += 1;
            let port = Arc::clone(self);
            let spawned = std::thread::Builder::new()
                .name("xtaskqueue".into())
                .spawn(move || port.worker());
            if spawned.is_err() {
                state.threads -= 1;
            }
        }
        self.wake.notify_one();
        S_OK
    }

    fn worker(self: Arc<Port>) {
        let mut state = lock(&self.state);
        loop {
            let now = Instant::now();
            state.promote(now);
            if let Some(work) = state.ready.pop_front() {
                state.running += 1;
                let canceled = state.terminated;
                drop(state);
                work.run(canceled);
                state = lock(&self.state);
                state.running -= 1;
                continue;
            }
            if Arc::strong_count(&self) == 1 || (state.terminated && state.delayed.is_empty()) {
                state.threads -= 1;
                return;
            }
            let wait = state.next_due().map_or(Duration::from_millis(500), |due| {
                due.saturating_duration_since(now)
                    .min(Duration::from_millis(500))
            });
            state.idle += 1;
            state = self
                .wake
                .wait_timeout(state, wait)
                .map(|(guard, _)| guard)
                .unwrap_or_else(|e| e.into_inner().0);
            state.idle -= 1;
        }
    }

    /// Runs one callback if one is ready, waiting up to `timeout_ms` for it.
    fn dispatch(&self, timeout_ms: u32) -> bool {
        if self.mode != Mode::Manual {
            return false;
        }
        let deadline = (timeout_ms != INFINITE)
            .then(|| Instant::now() + Duration::from_millis(u64::from(timeout_ms)));
        let mut state = lock(&self.state);
        loop {
            let now = Instant::now();
            state.promote(now);
            if let Some(work) = state.ready.pop_front() {
                state.running += 1;
                let canceled = state.terminated;
                drop(state);
                work.run(canceled);
                lock(&self.state).running -= 1;
                return true;
            }
            if deadline.is_some_and(|d| d <= now) {
                return false;
            }
            let mut wait = Duration::from_millis(100);
            if let Some(due) = state.next_due() {
                wait = wait.min(due.saturating_duration_since(now));
            }
            if let Some(d) = deadline {
                wait = wait.min(d.saturating_duration_since(now));
            }
            state = self
                .wake
                .wait_timeout(state, wait.max(Duration::from_millis(1)))
                .map(|(guard, _)| guard)
                .unwrap_or_else(|e| e.into_inner().0);
        }
    }

    fn is_idle(&self) -> bool {
        let state = lock(&self.state);
        state.ready.is_empty() && state.delayed.is_empty() && state.running == 0
    }

    fn terminate(&self) {
        lock(&self.state).terminated = true;
        self.wake.notify_all();
    }
}

#[derive(Default)]
struct QueueState {
    terminating: bool,
    finished: bool,
    suspended: usize,
    monitors: Vec<(u64, usize, usize)>,
    terminated_callbacks: Vec<(usize, usize)>,
}

pub struct Queue {
    work: Arc<Port>,
    completion: Arc<Port>,
    state: Mutex<QueueState>,
    done: Condvar,
}

static TOKENS: AtomicU64 = AtomicU64::new(1);

impl Queue {
    fn new(work: Arc<Port>, completion: Arc<Port>) -> Arc<Queue> {
        Arc::new(Queue {
            work,
            completion,
            state: Mutex::new(QueueState::default()),
            done: Condvar::new(),
        })
    }

    fn port(&self, which: u32) -> Option<&Arc<Port>> {
        match which {
            0 => Some(&self.work),
            1 => Some(&self.completion),
            _ => None,
        }
    }

    /// Submits a callback, as the program does; a terminating queue refuses it.
    fn submit(
        self: &Arc<Queue>,
        which: u32,
        context: usize,
        callback: usize,
        delay_ms: u32,
    ) -> i32 {
        if lock(&self.state).terminating {
            return E_ABORT;
        }
        self.submit_internal(which, context, callback, delay_ms)
    }

    /// Submits a callback of an async call, which a terminating queue still runs.
    fn submit_internal(
        self: &Arc<Queue>,
        which: u32,
        context: usize,
        callback: usize,
        delay_ms: u32,
    ) -> i32 {
        let Some(port) = self.port(which) else {
            return E_INVALIDARG;
        };
        let monitors: Vec<(usize, usize)> = lock(&self.state)
            .monitors
            .iter()
            .map(|(_, c, f)| (*c, *f))
            .collect();
        let handle = Arc::as_ptr(self) as usize;
        let result = port.submit(
            Work { context, callback },
            Duration::from_millis(u64::from(delay_ms)),
        );
        if result == S_OK {
            for (monitor_context, monitor) in monitors {
                // SAFETY: the monitor the program registered, with its documented shape.
                let monitor: unsafe extern "win64" fn(*mut c_void, *mut c_void, u32) =
                    unsafe { std::mem::transmute(monitor) };
                // SAFETY: the program's own callback.
                unsafe { monitor(monitor_context as *mut c_void, handle as *mut c_void, which) };
            }
        }
        result
    }

    /// An async call holds the queue open until it completes.
    fn suspend(&self) -> bool {
        let mut state = lock(&self.state);
        if state.finished {
            return false;
        }
        state.suspended += 1;
        true
    }

    fn resume(&self) {
        let mut state = lock(&self.state);
        state.suspended = state.suspended.saturating_sub(1);
    }

    fn terminate(self: &Arc<Queue>, wait: bool, context: usize, callback: usize) -> i32 {
        {
            let mut state = lock(&self.state);
            if state.terminating {
                return E_ABORT;
            }
            state.terminating = true;
            if callback != 0 {
                state.terminated_callbacks.push((context, callback));
            }
        }
        let queue = Arc::clone(self);
        let finisher = std::thread::Builder::new()
            .name("xtaskqueue-terminate".into())
            .spawn(move || queue.finish_termination());
        if finisher.is_err() {
            return E_ABORT;
        }
        if wait {
            let mut state = lock(&self.state);
            while !state.finished {
                state = self
                    .done
                    .wait_timeout(state, Duration::from_millis(50))
                    .map(|(guard, _)| guard)
                    .unwrap_or_else(|e| e.into_inner().0);
            }
        }
        S_OK
    }

    /// Waits for the async calls to complete, then cancels what is left and tells the program.
    fn finish_termination(self: Arc<Queue>) {
        loop {
            if lock(&self.state).suspended == 0 {
                self.work.terminate();
                self.completion.terminate();
                if self.work.is_idle() && self.completion.is_idle() {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let callbacks = {
            let mut state = lock(&self.state);
            state.finished = true;
            std::mem::take(&mut state.terminated_callbacks)
        };
        for (context, callback) in callbacks {
            // SAFETY: the callback the program gave `XTaskQueueTerminate`, called once with its context.
            let callback: unsafe extern "win64" fn(*mut c_void) =
                unsafe { std::mem::transmute(callback) };
            // SAFETY: the program's own callback.
            unsafe { callback(context as *mut c_void) };
        }
        self.done.notify_all();
    }
}

fn queue_handle(queue: &Arc<Queue>) -> *mut c_void {
    Arc::into_raw(Arc::clone(queue)) as *mut c_void
}

/// # Safety
/// `handle` is null or came from [`queue_handle`] and is still open.
unsafe fn queue_from(handle: *mut c_void) -> Option<Arc<Queue>> {
    if handle.is_null() {
        return None;
    }
    // SAFETY: as the caller promises; the handle keeps its own count.
    unsafe {
        Arc::increment_strong_count(handle as *const Queue);
        Some(Arc::from_raw(handle as *const Queue))
    }
}

/// # Safety
/// `handle` is null or a port handle of a queue that is still open.
unsafe fn port_from(handle: *mut c_void) -> Option<Arc<Port>> {
    if handle.is_null() {
        return None;
    }
    // SAFETY: as the caller promises.
    unsafe {
        Arc::increment_strong_count(handle as *const Port);
        Some(Arc::from_raw(handle as *const Port))
    }
}

#[derive(Default)]
struct ProcessQueues {
    chosen: Option<Arc<Queue>>,
    unset: bool,
    default: Option<Arc<Queue>>,
}

static PROCESS: Mutex<Option<ProcessQueues>> = Mutex::new(None);

/// The process task queue: the one the program set, else a default queue on thread pools for both ports.
pub(super) fn process_queue() -> Option<Arc<Queue>> {
    let mut guard = lock(&PROCESS);
    let process = guard.get_or_insert_with(ProcessQueues::default);
    if let Some(chosen) = &process.chosen {
        return Some(Arc::clone(chosen));
    }
    if process.unset {
        return None;
    }
    Some(Arc::clone(process.default.get_or_insert_with(|| {
        Queue::new(Port::new(Mode::ThreadPool), Port::new(Mode::ThreadPool))
    })))
}

// ---- XAsync ----

#[repr(C)]
pub struct AsyncBlock {
    queue: *mut c_void,
    context: *mut c_void,
    callback: usize,
    internal: [u8; 32],
}

const SIGNATURE: u32 = 0x4e45_4c41;
const OP_BEGIN: u32 = 0;
const OP_DO_WORK: u32 = 1;
const OP_GET_RESULT: u32 = 2;
const OP_CANCEL: u32 = 3;
const OP_CLEANUP: u32 = 4;

#[repr(C)]
pub struct ProviderData {
    async_block: *mut AsyncBlock,
    buffer_size: usize,
    buffer: *mut c_void,
    context: *mut c_void,
}

type Provider = unsafe extern "win64" fn(u32, *const ProviderData) -> i32;

#[derive(Default)]
struct AsyncInner {
    status: i32,
    completed: bool,
    finished: bool,
}

struct AsyncState {
    provider: usize,
    data: UnsafeCell<ProviderData>,
    provider_block: UnsafeCell<AsyncBlock>,
    user_block: usize,
    identity: usize,
    callback: usize,
    queue: Arc<Queue>,
    inner: Mutex<AsyncInner>,
    changed: Condvar,
}

// SAFETY: the raw pointers are the program's own blocks and buffers, which it owns across threads as the GDK
// requires; the mutable parts are guarded by `inner` or only touched by one operation at a time.
unsafe impl Send for AsyncState {}
// SAFETY: as above.
unsafe impl Sync for AsyncState {}

impl AsyncState {
    fn call(&self, op: u32) -> i32 {
        // SAFETY: `provider` is the provider function given to `begin`; `data` is valid for the call.
        let provider: Provider = unsafe { std::mem::transmute(self.provider) };
        // SAFETY: the call the provider was written for.
        unsafe { provider(op, self.data.get()) }
    }

    fn block(&self) -> *mut AsyncBlock {
        self.provider_block.get()
    }
}

impl Drop for AsyncState {
    fn drop(&mut self) {
        self.call(OP_CLEANUP);
    }
}

static BLOCKS: Mutex<()> = Mutex::new(());

/// # Safety
/// `block` is a writable block.
unsafe fn read_internal(block: *mut AsyncBlock) -> (u32, i32, usize) {
    // SAFETY: as the caller promises; the bytes may be unaligned in a packed caller structure.
    unsafe {
        let base = std::ptr::addr_of_mut!((*block).internal) as *mut u8;
        (
            std::ptr::read_unaligned(base as *const u32),
            std::ptr::read_unaligned(base.add(4) as *const i32),
            std::ptr::read_unaligned(base.add(8) as *const usize),
        )
    }
}

/// # Safety
/// `block` is a writable block.
unsafe fn write_internal(block: *mut AsyncBlock, signature: u32, status: i32, state: usize) {
    // SAFETY: as the caller promises.
    unsafe {
        let base = std::ptr::addr_of_mut!((*block).internal) as *mut u8;
        std::ptr::write_unaligned(base as *mut u32, signature);
        std::ptr::write_unaligned(base.add(4) as *mut i32, status);
        std::ptr::write_unaligned(base.add(8) as *mut usize, state);
        std::ptr::write_unaligned(base.add(16) as *mut u64, 0);
        std::ptr::write_unaligned(base.add(24) as *mut u64, 0);
    }
}

/// # Safety
/// `block` is a writable block.
unsafe fn state_of(block: *mut AsyncBlock) -> Option<Arc<AsyncState>> {
    let _guard = lock(&BLOCKS);
    // SAFETY: as the caller promises.
    let (signature, _, state) = unsafe { read_internal(block) };
    if signature != SIGNATURE || state == 0 {
        return None;
    }
    // SAFETY: the block holds one count of the state until it is cleared, which needs `BLOCKS`.
    unsafe {
        Arc::increment_strong_count(state as *const AsyncState);
        Some(Arc::from_raw(state as *const AsyncState))
    }
}

/// Takes the state out of the program's block, keeping its signature and status, and gives it back.
fn extract(state: &Arc<AsyncState>, status: i32) -> Option<Arc<AsyncState>> {
    let _guard = lock(&BLOCKS);
    let user = state.user_block as *mut AsyncBlock;
    // SAFETY: the program's block stays valid until its call has completed and delivered its result.
    let (signature, _, held) = unsafe { read_internal(user) };
    if signature != SIGNATURE || held == 0 {
        return None;
    }
    // SAFETY: writes the cleared state into both blocks; the count the user block held comes back as an `Arc`.
    unsafe {
        write_internal(user, SIGNATURE, status, 0);
        write_internal(state.block(), SIGNATURE, status, 0);
        Some(Arc::from_raw(held as *const AsyncState))
    }
}

/// `XAsyncBegin`.
///
/// # Safety
/// `block` is a writable block the program keeps alive until the call completes.
pub(super) unsafe fn begin(
    block: *mut AsyncBlock,
    context: usize,
    identity: usize,
    provider: usize,
) -> i32 {
    if block.is_null() || provider == 0 {
        return E_INVALIDARG;
    }
    // SAFETY: as the caller promises.
    let (signature, _, held) = unsafe { read_internal(block) };
    if signature == SIGNATURE && held != 0 {
        return E_INVALIDARG;
    }
    // SAFETY: as the caller promises.
    let (queue_handle, user_context, callback) =
        unsafe { ((*block).queue, (*block).context, (*block).callback) };
    // SAFETY: a queue handle the program holds, or null.
    let queue = match unsafe { queue_from(queue_handle) } {
        Some(queue) => queue,
        None => match process_queue() {
            Some(queue) => queue,
            None => return HR_NO_TASK_QUEUE,
        },
    };
    if !queue.suspend() {
        return E_ABORT;
    }
    let state = Arc::new(AsyncState {
        provider,
        data: UnsafeCell::new(ProviderData {
            async_block: std::ptr::null_mut(),
            buffer_size: 0,
            buffer: std::ptr::null_mut(),
            context: context as *mut c_void,
        }),
        provider_block: UnsafeCell::new(AsyncBlock {
            queue: queue_handle,
            context: user_context,
            callback,
            internal: [0; 32],
        }),
        user_block: block as usize,
        identity,
        callback,
        queue: Arc::clone(&queue),
        inner: Mutex::new(AsyncInner {
            status: E_PENDING,
            ..AsyncInner::default()
        }),
        changed: Condvar::new(),
    });
    let held = Arc::into_raw(Arc::clone(&state)) as usize;
    {
        let _guard = lock(&BLOCKS);
        // SAFETY: both blocks are writable; the user block owns the count taken above.
        unsafe {
            (*state.data.get()).async_block = state.block();
            write_internal(block, SIGNATURE, E_PENDING, held);
            write_internal(state.block(), SIGNATURE, E_PENDING, held);
        }
    }
    let result = state.call(OP_BEGIN);
    if result < 0 {
        queue.resume();
        if let Some(back) = extract(&state, result) {
            drop(back);
        }
        return result;
    }
    S_OK
}

/// `XAsyncSchedule`.
///
/// # Safety
/// `block` is a block of a call that has begun, or garbage the signature rejects.
pub(super) unsafe fn schedule(block: *mut AsyncBlock, delay_ms: u32) -> i32 {
    // SAFETY: as the caller promises.
    let Some(state) = (unsafe { state_of(block) }) else {
        return E_INVALIDARG;
    };
    let context = Arc::into_raw(Arc::clone(&state)) as usize;
    let result =
        state
            .queue
            .submit_internal(0, context, work_callback as *const () as usize, delay_ms);
    if result != S_OK {
        // SAFETY: the count taken above for a callback that was not queued.
        drop(unsafe { Arc::from_raw(context as *const AsyncState) });
    }
    result
}

unsafe extern "win64" fn work_callback(context: *mut c_void, canceled: u8) {
    // SAFETY: the count `schedule` took for this callback.
    let state = unsafe { Arc::from_raw(context as *const AsyncState) };
    if lock(&state.inner).completed {
        return;
    }
    if canceled != 0 {
        state.call(OP_CANCEL);
        // SAFETY: the provider's own block copy, alive as long as the state.
        unsafe { complete(state.block(), E_ABORT, 0) };
        return;
    }
    let mut result = state.call(OP_DO_WORK);
    if result != E_PENDING {
        if result >= 0 {
            result = E_UNEXPECTED;
        }
        // SAFETY: as above.
        unsafe { complete(state.block(), result, 0) };
    }
}

/// `XAsyncComplete`.
///
/// # Safety
/// `block` is a block of a call that has begun, or garbage the signature rejects.
pub(super) unsafe fn complete(block: *mut AsyncBlock, result: i32, required: usize) {
    // SAFETY: as the caller promises.
    let Some(state) = (unsafe { state_of(block) }) else {
        return;
    };
    {
        let mut inner = lock(&state.inner);
        if inner.completed {
            return;
        }
        inner.completed = true;
        inner.status = result;
    }
    // SAFETY: the provider data is only changed by the call's own completion.
    unsafe { (*state.data.get()).buffer_size = required };
    {
        let _guard = lock(&BLOCKS);
        // SAFETY: the blocks stay valid until the call has delivered its result.
        unsafe {
            let (_, _, held) = read_internal(state.user_block as *mut AsyncBlock);
            write_internal(state.user_block as *mut AsyncBlock, SIGNATURE, result, held);
            write_internal(state.block(), SIGNATURE, result, held);
        }
    }
    let keep = if required == 0 || result < 0 {
        extract(&state, result)
    } else {
        None
    };
    let callback_context = Arc::into_raw(Arc::clone(&state)) as usize;
    drop(keep);
    if state.callback != 0 {
        let queued = state.queue.submit_internal(
            1,
            callback_context,
            completion_callback as *const () as usize,
            0,
        );
        if queued == S_OK {
            return;
        }
    }
    // SAFETY: the count taken above, when the callback did not take it over.
    let state = unsafe { Arc::from_raw(callback_context as *const AsyncState) };
    finish(&state);
}

fn finish(state: &Arc<AsyncState>) {
    lock(&state.inner).finished = true;
    state.changed.notify_all();
    state.queue.resume();
}

unsafe extern "win64" fn completion_callback(context: *mut c_void, _canceled: u8) {
    // SAFETY: the count `complete` took for this callback.
    let state = unsafe { Arc::from_raw(context as *const AsyncState) };
    let callback: unsafe extern "win64" fn(*mut AsyncBlock) =
        // SAFETY: the completion routine the program put in its block.
        unsafe { std::mem::transmute(state.callback) };
    // SAFETY: the program's own routine, handed its own block.
    unsafe { callback(state.user_block as *mut AsyncBlock) };
    finish(&state);
}

/// `XAsyncGetStatus`.
///
/// # Safety
/// `block` is a block the program has begun a call with.
pub(super) unsafe fn get_status(block: *mut AsyncBlock, wait: bool) -> i32 {
    if block.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: as the caller promises.
    let (signature, status, _) = unsafe { read_internal(block) };
    if signature != SIGNATURE {
        return E_INVALIDARG;
    }
    // SAFETY: as above.
    let Some(state) = (unsafe { state_of(block) }) else {
        return status;
    };
    let mut inner = lock(&state.inner);
    if wait {
        while !inner.finished {
            inner = state.changed.wait(inner).unwrap_or_else(|e| e.into_inner());
        }
    }
    inner.status
}

/// `XAsyncGetResultSize`.
///
/// # Safety
/// `block` as for [`get_status`]; `size` is writable.
pub(super) unsafe fn get_result_size(block: *mut AsyncBlock, size: *mut usize) -> i32 {
    if size.is_null() {
        return E_POINTER;
    }
    // SAFETY: as the caller promises.
    let status = unsafe { get_status(block, false) };
    // SAFETY: as the caller promises.
    let needed = unsafe { state_of(block) }.map_or(0, |s| {
        // SAFETY: only written once, at completion.
        unsafe { (*s.data.get()).buffer_size }
    });
    // SAFETY: `size` is writable.
    unsafe { *size = needed };
    status
}

/// `XAsyncGetResult`.
///
/// # Safety
/// `block` as for [`get_status`]; `buffer` is writable for `size` bytes or null; `used` is writable or null.
pub(super) unsafe fn get_result(
    block: *mut AsyncBlock,
    identity: usize,
    size: usize,
    buffer: *mut c_void,
    used: *mut usize,
) -> i32 {
    if !used.is_null() {
        // SAFETY: writable, checked.
        unsafe { *used = 0 };
    }
    // SAFETY: as the caller promises.
    let Some(state) = (unsafe { state_of(block) }) else {
        // SAFETY: as the caller promises.
        let (signature, status, _) = unsafe { read_internal(block) };
        return if signature == SIGNATURE {
            status
        } else {
            E_INVALIDARG
        };
    };
    let status = lock(&state.inner).status;
    if status == E_PENDING {
        return E_PENDING;
    }
    if status < 0 {
        drop(extract(&state, status));
        return status;
    }
    if state.identity != 0 && identity != 0 && state.identity != identity {
        return E_INVALIDARG;
    }
    // SAFETY: only written once, at completion.
    let needed = unsafe { (*state.data.get()).buffer_size };
    if needed == 0 {
        drop(extract(&state, status));
        return S_OK;
    }
    if buffer.is_null() || size < needed {
        return HR_INSUFFICIENT_BUFFER;
    }
    // SAFETY: the provider is handed the caller's buffer for the length of the call.
    unsafe {
        (*state.data.get()).buffer = buffer;
        (*state.data.get()).buffer_size = size;
    }
    let result = state.call(OP_GET_RESULT);
    // SAFETY: as above.
    let written = unsafe { (*state.data.get()).buffer_size };
    if !used.is_null() {
        // SAFETY: writable, checked.
        unsafe { *used = written.min(size) };
    }
    drop(extract(&state, status));
    result
}

/// `XAsyncCancel`.
///
/// # Safety
/// `block` as for [`get_status`].
pub(super) unsafe fn cancel(block: *mut AsyncBlock) {
    // SAFETY: as the caller promises.
    if let Some(state) = unsafe { state_of(block) }
        && !lock(&state.inner).completed
    {
        state.call(OP_CANCEL);
    }
}

unsafe extern "win64" fn run_provider(op: u32, data: *const ProviderData) -> i32 {
    // SAFETY: the provider data of a live call.
    let data = unsafe { &*data };
    match op {
        OP_BEGIN => {
            // SAFETY: the call's own block.
            unsafe { schedule(data.async_block, 0) }
        }
        OP_DO_WORK => {
            let work: unsafe extern "win64" fn(*mut AsyncBlock) -> i32 =
                // SAFETY: the work function given to `run`.
                unsafe { std::mem::transmute(data.context) };
            // SAFETY: the program's own work function, handed the call's block.
            let result = unsafe { work(data.async_block) };
            // SAFETY: the call's own block.
            unsafe { complete(data.async_block, result, 0) };
            S_OK
        }
        _ => S_OK,
    }
}

/// `XAsyncRun`.
///
/// # Safety
/// `block` is a block the program keeps alive; `work` is a function address.
pub(super) unsafe fn run(block: *mut AsyncBlock, work: usize) -> i32 {
    if work == 0 {
        return E_INVALIDARG;
    }
    // SAFETY: as the caller promises.
    unsafe {
        begin(
            block,
            work,
            run as *const () as usize,
            run_provider as *const () as usize,
        )
    }
}

/// A call's identity, from its label: the same text anywhere names the same call.
fn label_id(label: &str) -> usize {
    let hash = label.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    });
    (hash as usize) | 1
}

/// A call whose work runs once on the work port and leaves its result as bytes, for the runtime's own `*Async`
/// functions.
type OneShotWork = Box<dyn FnOnce() -> (i32, Vec<u8>) + Send>;

struct OneShot {
    work: Option<OneShotWork>,
    result: Vec<u8>,
}

unsafe extern "win64" fn one_shot_provider(op: u32, data: *const ProviderData) -> i32 {
    // SAFETY: the provider data of a live call.
    let data = unsafe { &*data };
    let shot = data.context as *mut OneShot;
    match op {
        OP_BEGIN => {
            // SAFETY: the call's own block.
            unsafe { schedule(data.async_block, 0) }
        }
        OP_DO_WORK => {
            // SAFETY: made by `begin_one_shot` and freed only at cleanup.
            let shot = unsafe { &mut *shot };
            let (result, bytes) = shot
                .work
                .take()
                .map_or((super::E_FAIL, Vec::new()), |work| work());
            let size = if result >= 0 { bytes.len() } else { 0 };
            shot.result = bytes;
            // SAFETY: the call's own block.
            unsafe { complete(data.async_block, result, size) };
            S_OK
        }
        OP_GET_RESULT => {
            // SAFETY: as above.
            let shot = unsafe { &*shot };
            let count = shot.result.len().min(data.buffer_size);
            if !data.buffer.is_null() {
                // SAFETY: the caller's buffer holds at least `buffer_size` bytes.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        shot.result.as_ptr(),
                        data.buffer as *mut u8,
                        count,
                    );
                }
            }
            S_OK
        }
        OP_CLEANUP => {
            // SAFETY: the box `begin_one_shot` made, freed exactly once.
            drop(unsafe { Box::from_raw(shot) });
            S_OK
        }
        _ => S_OK,
    }
}

/// Starts a call whose result is `work`'s bytes. `label` names the call: the result function must pass the
/// same one.
///
/// # Safety
/// `block` is a writable block the program keeps alive until the call completes.
pub(super) unsafe fn begin_one_shot(
    block: *mut AsyncBlock,
    label: &'static str,
    work: impl FnOnce() -> (i32, Vec<u8>) + Send + 'static,
) -> i32 {
    let shot = Box::into_raw(Box::new(OneShot {
        work: Some(Box::new(work)),
        result: Vec::new(),
    }));
    // SAFETY: as the caller promises.
    unsafe {
        begin(
            block,
            shot as usize,
            label_id(label),
            one_shot_provider as *const () as usize,
        )
    }
}

/// The result of a [`begin_one_shot`] call into `buffer`.
///
/// # Safety
/// As for [`get_result`].
pub(super) unsafe fn one_shot_result(
    block: *mut AsyncBlock,
    label: &'static str,
    size: usize,
    buffer: *mut c_void,
) -> i32 {
    // SAFETY: as the caller promises.
    unsafe { get_result(block, label_id(label), size, buffer, std::ptr::null_mut()) }
}

// ---- The methods of XThreadingImpl ----

type This = *mut c_void;

pub(super) unsafe extern "win64" fn m_async_get_status(
    _: This,
    block: *mut AsyncBlock,
    wait: u8,
) -> i32 {
    // SAFETY: the program's block.
    unsafe { get_status(block, wait != 0) }
}

pub(super) unsafe extern "win64" fn m_async_get_result_size(
    _: This,
    block: *mut AsyncBlock,
    size: *mut usize,
) -> i32 {
    // SAFETY: the program's block and pointer.
    unsafe { get_result_size(block, size) }
}

pub(super) unsafe extern "win64" fn m_async_cancel(_: This, block: *mut AsyncBlock) {
    // SAFETY: the program's block.
    unsafe { cancel(block) }
}

pub(super) unsafe extern "win64" fn m_async_run(
    _: This,
    block: *mut AsyncBlock,
    work: usize,
) -> i32 {
    // SAFETY: the program's block and work function.
    unsafe { run(block, work) }
}

pub(super) unsafe extern "win64" fn m_async_begin(
    _: This,
    block: *mut AsyncBlock,
    context: usize,
    identity: usize,
    _identity_name: usize,
    provider: usize,
) -> i32 {
    // SAFETY: the program's block and provider.
    unsafe { begin(block, context, identity, provider) }
}

pub(super) unsafe extern "win64" fn m_async_schedule(
    _: This,
    block: *mut AsyncBlock,
    delay_ms: u32,
) -> i32 {
    // SAFETY: the program's block.
    unsafe { schedule(block, delay_ms) }
}

pub(super) unsafe extern "win64" fn m_async_complete(
    _: This,
    block: *mut AsyncBlock,
    result: i32,
    required: usize,
) {
    // SAFETY: the program's block.
    unsafe { complete(block, result, required) }
}

pub(super) unsafe extern "win64" fn m_async_get_result(
    _: This,
    block: *mut AsyncBlock,
    identity: usize,
    size: usize,
    buffer: *mut c_void,
    used: *mut usize,
) -> i32 {
    // SAFETY: the program's block and buffers.
    unsafe { get_result(block, identity, size, buffer, used) }
}

pub(super) unsafe extern "win64" fn m_queue_create(
    _: This,
    work: u32,
    completion: u32,
    out: *mut *mut c_void,
) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    let (Some(work), Some(completion)) = (mode_of(work), mode_of(completion)) else {
        return E_INVALIDARG;
    };
    let queue = Queue::new(Port::new(work), Port::new(completion));
    // SAFETY: `out` is writable, checked above.
    unsafe { *out = queue_handle(&queue) };
    S_OK
}

pub(super) unsafe extern "win64" fn m_queue_create_composite(
    _: This,
    work: *mut c_void,
    completion: *mut c_void,
    out: *mut *mut c_void,
) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: port handles of queues the program holds.
    let (Some(work), Some(completion)) =
        (unsafe { port_from(work) }, unsafe { port_from(completion) })
    else {
        return E_INVALIDARG;
    };
    let queue = Queue::new(work, completion);
    // SAFETY: `out` is writable, checked above.
    unsafe { *out = queue_handle(&queue) };
    S_OK
}

pub(super) unsafe extern "win64" fn m_queue_get_port(
    _: This,
    queue: *mut c_void,
    which: u32,
    out: *mut *mut c_void,
) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: a queue handle the program holds.
    let Some(queue) = (unsafe { queue_from(queue) }) else {
        return E_INVALIDARG;
    };
    let Some(port) = queue.port(which) else {
        return E_INVALIDARG;
    };
    // SAFETY: `out` is writable, checked above; the port lives as long as the queue.
    unsafe { *out = Arc::as_ptr(port) as *mut c_void };
    S_OK
}

pub(super) unsafe extern "win64" fn m_queue_duplicate(
    _: This,
    queue: *mut c_void,
    out: *mut *mut c_void,
) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: a queue handle the program holds.
    let Some(queue) = (unsafe { queue_from(queue) }) else {
        return E_INVALIDARG;
    };
    // SAFETY: `out` is writable, checked above.
    unsafe { *out = queue_handle(&queue) };
    S_OK
}

pub(super) unsafe extern "win64" fn m_queue_dispatch(
    _: This,
    queue: *mut c_void,
    which: u32,
    timeout_ms: u32,
) -> u8 {
    // SAFETY: a queue handle the program holds.
    let Some(queue) = (unsafe { queue_from(queue) }) else {
        return 0;
    };
    let port = queue.port(which).map(Arc::clone);
    u8::from(port.is_some_and(|p| p.dispatch(timeout_ms)))
}

pub(super) unsafe extern "win64" fn m_queue_close(_: This, queue: *mut c_void) {
    if !queue.is_null() {
        // SAFETY: the count `queue_handle` gave this handle.
        drop(unsafe { Arc::from_raw(queue as *const Queue) });
    }
}

pub(super) unsafe extern "win64" fn m_queue_submit(
    _: This,
    queue: *mut c_void,
    which: u32,
    context: usize,
    callback: usize,
) -> i32 {
    // SAFETY: a queue handle the program holds.
    let Some(queue) = (unsafe { queue_from(queue) }) else {
        return E_INVALIDARG;
    };
    if callback == 0 {
        return E_INVALIDARG;
    }
    queue.submit(which, context, callback, 0)
}

pub(super) unsafe extern "win64" fn m_queue_submit_delayed(
    _: This,
    queue: *mut c_void,
    which: u32,
    delay_ms: u32,
    context: usize,
    callback: usize,
) -> i32 {
    // SAFETY: a queue handle the program holds.
    let Some(queue) = (unsafe { queue_from(queue) }) else {
        return E_INVALIDARG;
    };
    if callback == 0 {
        return E_INVALIDARG;
    }
    queue.submit(which, context, callback, delay_ms)
}

static WAITERS: Mutex<Option<HashMap<u64, Arc<AtomicBool>>>> = Mutex::new(None);

pub(super) unsafe extern "win64" fn m_queue_register_waiter(
    _: This,
    queue: *mut c_void,
    which: u32,
    handle: usize,
    context: usize,
    callback: usize,
    token: *mut u64,
) -> i32 {
    if token.is_null() {
        return E_POINTER;
    }
    // SAFETY: a queue handle the program holds.
    let Some(queue) = (unsafe { queue_from(queue) }) else {
        return E_INVALIDARG;
    };
    if handle == 0 || callback == 0 || queue.port(which).is_none() {
        return E_INVALIDARG;
    }
    let id = TOKENS.fetch_add(1, Ordering::Relaxed);
    let stop = Arc::new(AtomicBool::new(false));
    lock(&WAITERS)
        .get_or_insert_with(HashMap::new)
        .insert(id, Arc::clone(&stop));
    let spawned = std::thread::Builder::new()
        .name("xtaskqueue-waiter".into())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                // SAFETY: waits on a handle the program registered and keeps open.
                let signaled = unsafe {
                    windows_sys::Win32::System::Threading::WaitForSingleObject(
                        handle as *mut c_void,
                        20,
                    )
                } == 0;
                if signaled {
                    queue.submit(which, context, callback, 0);
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        });
    if spawned.is_err() {
        return E_ABORT;
    }
    // SAFETY: `token` is writable, checked above.
    unsafe { *token = id };
    S_OK
}

pub(super) unsafe extern "win64" fn m_queue_unregister_waiter(
    _: This,
    _queue: *mut c_void,
    token: u64,
) {
    if let Some(stop) = lock(&WAITERS)
        .get_or_insert_with(HashMap::new)
        .remove(&token)
    {
        stop.store(true, Ordering::Relaxed);
    }
}

pub(super) unsafe extern "win64" fn m_queue_terminate(
    _: This,
    queue: *mut c_void,
    wait: u8,
    context: usize,
    callback: usize,
) -> i32 {
    // SAFETY: a queue handle the program holds.
    let Some(queue) = (unsafe { queue_from(queue) }) else {
        return E_INVALIDARG;
    };
    queue.terminate(wait != 0, context, callback)
}

pub(super) unsafe extern "win64" fn m_queue_register_monitor(
    _: This,
    queue: *mut c_void,
    context: usize,
    callback: usize,
    token: *mut u64,
) -> i32 {
    if token.is_null() {
        return E_POINTER;
    }
    // SAFETY: a queue handle the program holds.
    let Some(queue) = (unsafe { queue_from(queue) }) else {
        return E_INVALIDARG;
    };
    if callback == 0 {
        return E_INVALIDARG;
    }
    let id = TOKENS.fetch_add(1, Ordering::Relaxed);
    lock(&queue.state).monitors.push((id, context, callback));
    // SAFETY: `token` is writable, checked above.
    unsafe { *token = id };
    S_OK
}

pub(super) unsafe extern "win64" fn m_queue_unregister_monitor(
    _: This,
    queue: *mut c_void,
    token: u64,
) {
    // SAFETY: a queue handle the program holds.
    if let Some(queue) = unsafe { queue_from(queue) } {
        lock(&queue.state)
            .monitors
            .retain(|(id, _, _)| *id != token);
    }
}

pub(super) unsafe extern "win64" fn m_queue_get_process(_: This, out: *mut *mut c_void) -> u8 {
    if out.is_null() {
        return 0;
    }
    let queue = process_queue();
    // SAFETY: `out` is writable, checked above.
    unsafe { *out = queue.as_ref().map_or(std::ptr::null_mut(), queue_handle) };
    u8::from(queue.is_some())
}

pub(super) unsafe extern "win64" fn m_queue_set_process(_: This, queue: *mut c_void) {
    // SAFETY: a queue handle the program holds, or null.
    let chosen = unsafe { queue_from(queue) };
    let mut guard = lock(&PROCESS);
    let process = guard.get_or_insert_with(ProcessQueues::default);
    process.unset = chosen.is_none();
    process.chosen = chosen;
}

/// Submits a callback to a queue's port, as `XTaskQueueSubmitCallback` does; a null queue is the process queue.
///
/// # Safety
/// `queue` is null or a queue handle the program holds.
pub(super) unsafe fn submit(
    queue: *mut c_void,
    which: u32,
    context: usize,
    callback: usize,
) -> i32 {
    // SAFETY: as the caller promises.
    let queue = unsafe { queue_from(queue) }.or_else(process_queue);
    match queue {
        Some(queue) => queue.submit(which, context, callback, 0),
        None => HR_NO_TASK_QUEUE,
    }
}

/// The result of any call: its status, dropping the call's state. For results nothing is read from.
///
/// # Safety
/// As for [`get_result`].
pub(super) unsafe fn finish_call(block: *mut AsyncBlock) -> i32 {
    // SAFETY: as the caller promises.
    unsafe { get_result(block, 0, 0, std::ptr::null_mut(), std::ptr::null_mut()) }
}

static OFFLINE_ONCE: OnceLock<()> = OnceLock::new();

/// Called when the runtime starts, so the process queue exists before the program asks.
pub(super) fn prepare() {
    OFFLINE_ONCE.get_or_init(|| {
        let _ = process_queue();
    });
}
