//! The work behind the DirectStorage objects: requests, status arrays and queues with their worker threads.
//! Nothing here touches COM; `super::com` wraps it.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::os::windows::fs::FileExt;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Threading::{CreateEventW, ResetEvent, SetEvent};

use crate::trace::note;

pub const S_OK: i32 = 0;
pub const E_PENDING: i32 = 0x8000_000Au32 as i32;
pub const E_NOTIMPL: i32 = 0x8000_4001u32 as i32;
pub const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;
pub const E_FAIL: i32 = 0x8000_4005u32 as i32;

pub fn win32_error(code: u32) -> i32 {
    (0x8007_0000 | (code & 0xFFFF)) as i32
}

fn io_error(error: &std::io::Error) -> i32 {
    error
        .raw_os_error()
        .map_or(E_FAIL, |c| win32_error(c as u32))
}

/// Where a GPU request's bytes go, as the request describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuDestination {
    Buffer {
        resource: usize,
        offset: u64,
    },
    TextureRegion {
        resource: usize,
        subresource: u32,
        /// Left, top, front, right, bottom, back.
        region: [u32; 6],
    },
    MultipleSubresources {
        resource: usize,
        first_subresource: u32,
    },
    Tiles {
        resource: usize,
        /// X, Y, Z, subresource.
        coordinate: [u32; 4],
        /// Tile count, use-box flag, width, height and depth (the last two as stored in 16 bits each).
        size: [u32; 4],
    },
}

/// Decompressed bytes bound for a GPU resource.
pub struct GpuUpload<'a> {
    pub destination: &'a GpuDestination,
    pub data: &'a [u8],
}

pub type GpuCopy = fn(&GpuUpload<'_>) -> i32;
pub type FenceSignal = fn(fence: usize, value: u64) -> i32;
pub type Decompress = fn(input: &[u8], output: &mut [u8]) -> Result<(), i32>;

static GPU_COPY: RwLock<Option<GpuCopy>> = RwLock::new(None);
static FENCE_SIGNAL: RwLock<Option<FenceSignal>> = RwLock::new(None);
static DECOMPRESSORS: Mutex<Option<HashMap<u8, Decompress>>> = Mutex::new(None);
static NOTED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

pub fn set_gpu_copy(copy: Option<GpuCopy>) {
    if let Ok(mut slot) = GPU_COPY.write() {
        *slot = copy;
    }
}

pub fn set_fence_signal(signal: Option<FenceSignal>) {
    if let Ok(mut slot) = FENCE_SIGNAL.write() {
        *slot = signal;
    }
}

pub fn set_decompressor(format: u8, decompress: Decompress) {
    if let Ok(mut slot) = DECOMPRESSORS.lock() {
        slot.get_or_insert_with(HashMap::new)
            .insert(format, decompress);
    }
}

fn note_once(text: String) {
    let fresh = NOTED
        .lock()
        .map(|mut seen| seen.get_or_insert_with(HashSet::new).insert(text.clone()))
        .unwrap_or(true);
    if fresh {
        note(text);
    }
}

/// `ID3D12Fence::Signal` is slot 10 of the fence's vtable.
fn signal_through_vtable(fence: usize, value: u64) -> i32 {
    if fence == 0 {
        return E_INVALIDARG;
    }
    // SAFETY: the program passed a fence object; `ID3D12Fence` is a COM interface whose first word is its
    // vtable, and slot 10 is `Signal(UINT64)`.
    unsafe {
        let vtable = *(fence as *const *const usize);
        let signal: unsafe extern "system" fn(usize, u64) -> i32 =
            std::mem::transmute(*vtable.add(10));
        signal(fence, value)
    }
}

pub struct Entry {
    done: AtomicBool,
    result: AtomicI32,
}

/// What the title polls to see how far the queue has got.
pub struct StatusArray {
    entries: Vec<Entry>,
}

impl StatusArray {
    pub fn new(capacity: u32) -> StatusArray {
        StatusArray {
            entries: (0..capacity)
                .map(|_| Entry {
                    done: AtomicBool::new(false),
                    result: AtomicI32::new(E_PENDING),
                })
                .collect(),
        }
    }

    pub fn is_complete(&self, index: u32) -> bool {
        self.entries
            .get(index as usize)
            .is_some_and(|e| e.done.load(Ordering::Acquire))
    }

    pub fn result(&self, index: u32) -> i32 {
        self.entries.get(index as usize).map_or(E_INVALIDARG, |e| {
            if e.done.load(Ordering::Acquire) {
                e.result.load(Ordering::Relaxed)
            } else {
                E_PENDING
            }
        })
    }

    fn complete(&self, index: u32, result: i32) {
        if let Some(entry) = self.entries.get(index as usize) {
            entry.result.store(result, Ordering::Relaxed);
            entry.done.store(true, Ordering::Release);
        }
    }

    pub fn reset(&self, index: u32) {
        if let Some(entry) = self.entries.get(index as usize) {
            entry.done.store(false, Ordering::Release);
            entry.result.store(E_PENDING, Ordering::Relaxed);
        }
    }
}

pub enum Source {
    File {
        file: Arc<File>,
        offset: u64,
        size: u32,
    },
    Memory {
        address: usize,
        size: u32,
    },
}

pub enum Destination {
    Memory { address: usize, size: u32 },
    Gpu(GpuDestination),
}

pub struct Request {
    pub source: Source,
    pub destination: Destination,
    pub compression: u8,
    pub uncompressed: u32,
    pub tag: u64,
    pub name: String,
    seq: u64,
}

impl Request {
    pub fn new(
        source: Source,
        destination: Destination,
        compression: u8,
        uncompressed: u32,
        tag: u64,
        name: String,
    ) -> Request {
        Request {
            source,
            destination,
            compression,
            uncompressed,
            tag,
            name,
            seq: 0,
        }
    }
}

pub enum Item {
    Request(Request),
    Status { array: Arc<StatusArray>, index: u32 },
    Signal { fence: usize, value: u64 },
    Event(usize),
}

const WHOLE_FILE: u32 = u32::MAX;

fn read_range(file: &File, offset: u64, buffer: &mut [u8]) -> Result<(), i32> {
    let mut done = 0;
    while done < buffer.len() {
        match file.seek_read(&mut buffer[done..], offset + done as u64) {
            Ok(0) => return Err(win32_error(38)),
            Ok(n) => done += n,
            Err(e) => return Err(io_error(&e)),
        }
    }
    Ok(())
}

fn file_length(file: &File, offset: u64, size: u32) -> Result<usize, i32> {
    if size != WHOLE_FILE {
        return Ok(size as usize);
    }
    let length = file.metadata().map_err(|e| io_error(&e))?.len();
    usize::try_from(length.saturating_sub(offset)).map_err(|_| E_INVALIDARG)
}

/// Reads or copies a request's source into a vector, or straight into `into` when given.
fn fetch(source: &Source, into: Option<&mut [u8]>) -> Result<Option<Vec<u8>>, i32> {
    match source {
        Source::File { file, offset, size } => {
            let length = file_length(file, *offset, *size)?;
            match into {
                Some(buffer) => {
                    let buffer = buffer.get_mut(..length).ok_or(E_INVALIDARG)?;
                    read_range(file, *offset, buffer)?;
                    Ok(None)
                }
                None => {
                    let mut data = vec![0u8; length];
                    read_range(file, *offset, &mut data)?;
                    Ok(Some(data))
                }
            }
        }
        Source::Memory { address, size } => {
            if *address == 0 {
                return Err(E_INVALIDARG);
            }
            // SAFETY: a memory request names `size` readable bytes at `address` until it completes.
            let bytes =
                unsafe { std::slice::from_raw_parts(*address as *const u8, *size as usize) };
            match into {
                Some(buffer) => {
                    buffer
                        .get_mut(..bytes.len())
                        .ok_or(E_INVALIDARG)?
                        .copy_from_slice(bytes);
                    Ok(None)
                }
                None => Ok(Some(bytes.to_vec())),
            }
        }
    }
}

fn decompress(request: &Request, input: &[u8]) -> Result<Vec<u8>, i32> {
    let function = DECOMPRESSORS
        .lock()
        .ok()
        .and_then(|d| d.as_ref()?.get(&request.compression).copied());
    let Some(function) = function else {
        note_once(format!(
            "DirectStorage compression format {} is not supported ({:?})",
            request.compression,
            if request.name.is_empty() {
                "unnamed request"
            } else {
                &request.name
            }
        ));
        return Err(E_NOTIMPL);
    };
    let mut output = vec![0u8; request.uncompressed as usize];
    function(input, &mut output)?;
    Ok(output)
}

/// Runs one request to completion; the result is an `HRESULT`.
pub fn execute(request: &Request) -> i32 {
    match run(request) {
        Ok(()) => S_OK,
        Err(code) => code,
    }
}

fn run(request: &Request) -> Result<(), i32> {
    match (&request.destination, request.compression) {
        (Destination::Memory { address, size }, 0) => {
            if *address == 0 {
                return Err(E_INVALIDARG);
            }
            // SAFETY: a memory destination names `size` writable bytes at `address` until the request completes.
            let buffer =
                unsafe { std::slice::from_raw_parts_mut(*address as *mut u8, *size as usize) };
            fetch(&request.source, Some(buffer)).map(|_| ())
        }
        (Destination::Memory { address, size }, _) => {
            if *address == 0 {
                return Err(E_INVALIDARG);
            }
            let input = fetch(&request.source, None)?.unwrap_or_default();
            let output = decompress(request, &input)?;
            // SAFETY: as above.
            let buffer =
                unsafe { std::slice::from_raw_parts_mut(*address as *mut u8, *size as usize) };
            buffer
                .get_mut(..output.len())
                .ok_or(E_INVALIDARG)?
                .copy_from_slice(&output);
            Ok(())
        }
        (Destination::Gpu(destination), compression) => {
            let input = fetch(&request.source, None)?.unwrap_or_default();
            let data = if compression == 0 {
                input
            } else {
                decompress(request, &input)?
            };
            let copy = GPU_COPY.read().ok().and_then(|c| *c);
            let Some(copy) = copy else {
                note_once(
                    "DirectStorage request for a GPU resource, and no copy callback is set"
                        .to_string(),
                );
                return Err(E_NOTIMPL);
            };
            match copy(&GpuUpload {
                destination,
                data: &data,
            }) {
                code if code < 0 => Err(code),
                _ => Ok(()),
            }
        }
    }
}

#[derive(Default)]
struct Failures {
    count: u32,
    first: i32,
}

/// What `RetrieveErrorRecord` reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorRecord {
    pub count: u32,
    pub first: i32,
}

struct Shared {
    cancels: Mutex<Vec<(u64, u64, u64)>>,
    failures: Mutex<Failures>,
    event: AtomicUsize,
    inflight: AtomicUsize,
}

impl Shared {
    fn fail(&self, code: i32) {
        if let Ok(mut failures) = self.failures.lock() {
            if failures.count == 0 {
                failures.first = code;
            }
            failures.count += 1;
        }
        let event = self.event.load(Ordering::Acquire);
        if event != 0 {
            // SAFETY: an event this queue created.
            unsafe { SetEvent(event as HANDLE) };
        }
    }

    fn cancelled(&self, request: &Request) -> bool {
        self.cancels.lock().is_ok_and(|c| {
            c.iter()
                .any(|(mask, value, upto)| request.seq < *upto && request.tag & mask == *value)
        })
    }
}

pub struct QueueDesc {
    pub source_type: i32,
    pub capacity: u16,
    pub priority: i16,
    pub name: usize,
    pub device: usize,
}

pub struct Queue {
    pub desc: QueueDesc,
    shared: Arc<Shared>,
    pending: Mutex<Vec<Item>>,
    sequence: AtomicU64,
    sender: Mutex<Option<Sender<Vec<Item>>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl Queue {
    pub fn new(desc: QueueDesc) -> Queue {
        let shared = Arc::new(Shared {
            cancels: Mutex::new(Vec::new()),
            failures: Mutex::new(Failures::default()),
            event: AtomicUsize::new(0),
            inflight: AtomicUsize::new(0),
        });
        let (sender, receiver) = channel();
        let worker_shared = shared.clone();
        let worker = std::thread::Builder::new()
            .name("dstorage queue".into())
            .spawn(move || work(&worker_shared, receiver))
            .ok();
        Queue {
            desc,
            shared,
            pending: Mutex::new(Vec::new()),
            sequence: AtomicU64::new(1),
            sender: Mutex::new(Some(sender)),
            worker: Mutex::new(worker),
        }
    }

    pub fn enqueue(&self, mut item: Item) {
        if self.sender.lock().is_ok_and(|s| s.is_none()) {
            return;
        }
        if let Item::Request(request) = &mut item {
            request.seq = self.sequence.fetch_add(1, Ordering::Relaxed);
        }
        self.shared.inflight.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut pending) = self.pending.lock() {
            pending.push(item);
        }
    }

    pub fn submit(&self) {
        let batch = self
            .pending
            .lock()
            .map(|mut p| std::mem::take(&mut *p))
            .unwrap_or_default();
        if batch.is_empty() {
            return;
        }
        let count = batch.len();
        let sent = self
            .sender
            .lock()
            .ok()
            .and_then(|s| s.as_ref().map(|s| s.send(batch).is_ok()))
            .unwrap_or(false);
        if !sent {
            self.shared.inflight.fetch_sub(count, Ordering::Relaxed);
        }
    }

    pub fn cancel(&self, mask: u64, value: u64) {
        let upto = self.sequence.load(Ordering::Relaxed);
        if let Ok(mut cancels) = self.shared.cancels.lock() {
            cancels.push((mask, value, upto));
        }
    }

    /// Stops taking requests; submitted work still finishes. Requests never submitted are dropped.
    pub fn close(&self) {
        let dropped = self
            .pending
            .lock()
            .map(|mut p| std::mem::take(&mut *p).len())
            .unwrap_or(0);
        self.shared.inflight.fetch_sub(dropped, Ordering::Relaxed);
        if let Ok(mut sender) = self.sender.lock() {
            sender.take();
        }
    }

    pub fn empty_slots(&self) -> u32 {
        let capacity = if self.desc.capacity == 0 {
            0x2000
        } else {
            self.desc.capacity as usize
        };
        capacity.saturating_sub(self.shared.inflight.load(Ordering::Relaxed)) as u32
    }

    /// The manual-reset event set at the first failure.
    pub fn error_event(&self) -> usize {
        let existing = self.shared.event.load(Ordering::Acquire);
        if existing != 0 {
            return existing;
        }
        // SAFETY: a new unnamed manual-reset event.
        let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) } as usize;
        match self
            .shared
            .event
            .compare_exchange(0, event, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => {
                if self.shared.failures.lock().is_ok_and(|f| f.count > 0) {
                    // SAFETY: the event just made.
                    unsafe { SetEvent(event as HANDLE) };
                }
                event
            }
            Err(winner) => {
                // SAFETY: an event nobody else saw.
                unsafe { windows_sys::Win32::Foundation::CloseHandle(event as HANDLE) };
                winner
            }
        }
    }

    /// The failures since the last call, which also clears them and resets the event.
    pub fn take_error(&self) -> ErrorRecord {
        let record = self
            .shared
            .failures
            .lock()
            .map(|mut f| {
                let record = ErrorRecord {
                    count: f.count,
                    first: f.first,
                };
                *f = Failures::default();
                record
            })
            .unwrap_or(ErrorRecord { count: 0, first: 0 });
        let event = self.shared.event.load(Ordering::Acquire);
        if event != 0 {
            // SAFETY: an event this queue created.
            unsafe { ResetEvent(event as HANDLE) };
        }
        record
    }
}

impl Drop for Queue {
    fn drop(&mut self) {
        self.close();
        if let Ok(mut worker) = self.worker.lock()
            && let Some(worker) = worker.take()
        {
            let _ = worker.join();
        }
        let event = self.shared.event.swap(0, Ordering::AcqRel);
        if event != 0 {
            // SAFETY: the event this queue created.
            unsafe { windows_sys::Win32::Foundation::CloseHandle(event as HANDLE) };
        }
    }
}

fn work(shared: &Shared, receiver: Receiver<Vec<Item>>) {
    let mut since_status = S_OK;
    for batch in receiver {
        for item in batch {
            match item {
                Item::Request(request) => {
                    if !shared.cancelled(&request) {
                        let result = execute(&request);
                        if result < 0 {
                            note_once(format!(
                                "DirectStorage request {:?} failed with {:#010x}",
                                request.name, result as u32
                            ));
                            shared.fail(result);
                            if since_status == S_OK {
                                since_status = result;
                            }
                        }
                    }
                }
                Item::Status { array, index } => {
                    array.complete(index, since_status);
                    since_status = S_OK;
                }
                Item::Signal { fence, value } => {
                    let signal = FENCE_SIGNAL.read().ok().and_then(|s| *s);
                    let result = match signal {
                        Some(signal) => signal(fence, value),
                        None => signal_through_vtable(fence, value),
                    };
                    if result < 0 {
                        shared.fail(result);
                    }
                }
                Item::Event(handle) => {
                    // SAFETY: an event handle the program passed to `EnqueueSetEvent`.
                    unsafe { SetEvent(handle as HANDLE) };
                }
            }
            shared.inflight.fetch_sub(1, Ordering::Relaxed);
        }
    }
}
