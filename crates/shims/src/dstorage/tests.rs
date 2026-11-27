use std::ffi::c_void;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::*;
use crate::dstorage::{
    GpuDestination, GpuUpload, set_decompressor, set_fence_signal, set_gpu_copy,
};

static LOCK: Mutex<()> = Mutex::new(());

const E_NOTIMPL: i32 = 0x8000_4001u32 as i32;
const E_PENDING: i32 = 0x8000_000Au32 as i32;
const FILE_NOT_FOUND: i32 = 0x8007_0002u32 as i32;

type Create = unsafe extern "win64" fn(*mut c_void, *const u8, *const u8, *mut *mut c_void) -> i32;
type Open = unsafe extern "win64" fn(*mut c_void, *const u16, *const u8, *mut *mut c_void) -> i32;
type Status =
    unsafe extern "win64" fn(*mut c_void, u32, *const u8, *const u8, *mut *mut c_void) -> i32;

unsafe fn method<F: Copy>(object: *mut c_void, slot: usize) -> F {
    // SAFETY: every object of ours starts with a vtable pointer.
    unsafe {
        let vtable = *(object as *const *const usize);
        std::mem::transmute_copy(&*vtable.add(slot))
    }
}

struct Setup {
    _guard: std::sync::MutexGuard<'static, ()>,
    factory: *mut c_void,
    queue: *mut c_void,
    status: *mut c_void,
}

fn desc() -> [u8; 32] {
    let mut desc = [0u8; 32];
    desc[8..10].copy_from_slice(&64u16.to_le_bytes());
    desc[24] = 1;
    desc
}

fn setup() -> Setup {
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut factory = std::ptr::null_mut();
    // SAFETY: the shim under test, with a valid out pointer.
    unsafe {
        assert_eq!(get_factory(IID_FACTORY.as_ptr(), &mut factory), 0);
        let mut queue = std::ptr::null_mut();
        let create: Create = method(factory, 10);
        assert_eq!(
            create(factory, desc().as_ptr(), IID_QUEUE.as_ptr(), &mut queue),
            0
        );
        let mut status = std::ptr::null_mut();
        let make: Status = method(factory, 5);
        assert_eq!(
            make(
                factory,
                8,
                std::ptr::null(),
                IID_STATUS.as_ptr(),
                &mut status
            ),
            0
        );
        Setup {
            _guard: guard,
            factory,
            queue,
            status,
        }
    }
}

fn open_file(setup: &Setup, path: &Path) -> *mut c_void {
    let wide: Vec<u16> = path.to_string_lossy().encode_utf16().chain([0]).collect();
    let mut file = std::ptr::null_mut();
    // SAFETY: a terminated path and a valid out pointer.
    let result = unsafe {
        let open: Open = method(setup.factory, 4);
        open(setup.factory, wide.as_ptr(), IID_FILE.as_ptr(), &mut file)
    };
    assert_eq!(result, 0);
    file
}

struct Spec {
    file: *mut c_void,
    offset: u64,
    size: u32,
    memory_source: *const u8,
    compression: u8,
    destination: u8,
    target: usize,
    target_size: u32,
    extra: u64,
    uncompressed: u32,
    tag: u64,
}

impl Default for Spec {
    fn default() -> Spec {
        Spec {
            file: std::ptr::null_mut(),
            offset: 0,
            size: 0,
            memory_source: std::ptr::null(),
            compression: 0,
            destination: 0,
            target: 0,
            target_size: 0,
            extra: 0,
            uncompressed: 0,
            tag: 0,
        }
    }
}

fn request(spec: &Spec) -> [u8; REQUEST_SIZE] {
    let mut bytes = [0u8; REQUEST_SIZE];
    let memory = !spec.memory_source.is_null();
    let mut options = u64::from(spec.compression & 0xF);
    if memory {
        options |= OPTION_MEMORY_SOURCE;
    }
    if spec.destination == 1 {
        options |= OPTION_PAGE_DESTINATION;
    }
    bytes[0..8].copy_from_slice(&options.to_le_bytes());
    bytes[8..16].copy_from_slice(&(spec.target as u64).to_le_bytes());
    bytes[16..18].copy_from_slice(&(spec.extra as u16).to_le_bytes());
    let uncompressed = if spec.uncompressed != 0 {
        spec.uncompressed
    } else {
        spec.target_size
    };
    bytes[0x18..0x1C].copy_from_slice(&uncompressed.to_le_bytes());
    if memory {
        bytes[0x20..0x28].copy_from_slice(&(spec.memory_source as u64).to_le_bytes());
    } else {
        bytes[0x20..0x28].copy_from_slice(&(spec.file as u64).to_le_bytes());
        bytes[0x28..0x30].copy_from_slice(&spec.offset.to_le_bytes());
    }
    bytes[0x30..0x34].copy_from_slice(&spec.size.to_le_bytes());
    bytes[0x38..0x40].copy_from_slice(&spec.tag.to_le_bytes());
    bytes
}

fn enqueue(setup: &Setup, spec: &Spec) {
    let bytes = request(spec);
    // SAFETY: a request of the console's size.
    unsafe {
        let enqueue: unsafe extern "win64" fn(*mut c_void, *const u8) = method(setup.queue, 3);
        enqueue(setup.queue, bytes.as_ptr());
    }
}

fn mark_and_submit(setup: &Setup, index: u32) {
    // SAFETY: our own queue and status array.
    unsafe {
        let status: unsafe extern "win64" fn(*mut c_void, *mut c_void, u32) =
            method(setup.queue, 4);
        status(setup.queue, setup.status, index);
        let submit: unsafe extern "win64" fn(*mut c_void) = method(setup.queue, 6);
        submit(setup.queue);
    }
}

fn wait(setup: &Setup, index: u32) -> i32 {
    let started = Instant::now();
    // SAFETY: our own status array.
    unsafe {
        let complete: unsafe extern "win64" fn(*mut c_void, u32) -> i32 = method(setup.status, 3);
        let result: unsafe extern "win64" fn(*mut c_void, u32) -> i32 = method(setup.status, 4);
        while complete(setup.status, index) == 0 {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "request never finished"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        result(setup.status, index)
    }
}

fn temp_file(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
    let scratch = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-tmp");
    std::fs::create_dir_all(&scratch).expect("scratch");
    let dir = tempfile::Builder::new()
        .prefix("dstorage-")
        .tempdir_in(&scratch)
        .expect("temp dir");
    let path = dir
        .path()
        .canonicalize()
        .expect("canonical")
        .join("data.bin");
    std::fs::write(&path, bytes).expect("write");
    (dir, path)
}

fn pattern(length: usize) -> Vec<u8> {
    (0..length).map(|i| (i * 7 + i / 251) as u8).collect()
}

static SIGNALLED: AtomicU64 = AtomicU64::new(0);

fn record_signal(fence: usize, value: u64) -> i32 {
    assert_eq!(fence, 0x1234);
    SIGNALLED.store(value, Ordering::SeqCst);
    0
}

#[test]
fn a_file_request_reads_a_range_into_memory_and_signals() {
    let s = setup();
    let data = pattern(20_000);
    let (_dir, path) = temp_file(&data);
    let file = open_file(&s, &path);
    let mut buffer = vec![0u8; 5000];
    enqueue(
        &s,
        &Spec {
            file,
            offset: 1000,
            size: 5000,
            target: buffer.as_mut_ptr() as usize,
            target_size: 5000,
            ..Spec::default()
        },
    );
    set_fence_signal(Some(record_signal));
    // SAFETY: our own queue; the fence is only passed to the callback above.
    unsafe {
        let signal: unsafe extern "win64" fn(*mut c_void, *mut c_void, u64) = method(s.queue, 5);
        signal(s.queue, 0x1234 as *mut c_void, 42);
    }
    mark_and_submit(&s, 0);
    assert_eq!(wait(&s, 0), 0);
    assert_eq!(buffer, &data[1000..6000]);
    assert_eq!(
        SIGNALLED.load(Ordering::SeqCst),
        42,
        "a fence ahead of the status is signalled before it"
    );
    set_fence_signal(None);
}

#[test]
fn a_whole_file_request_with_size_minus_one() {
    let s = setup();
    let data = pattern(3000);
    let (_dir, path) = temp_file(&data);
    let file = open_file(&s, &path);
    let mut buffer = vec![0u8; 2500];
    enqueue(
        &s,
        &Spec {
            file,
            offset: 500,
            size: u32::MAX,
            target: buffer.as_mut_ptr() as usize,
            target_size: 2500,
            ..Spec::default()
        },
    );
    mark_and_submit(&s, 1);
    assert_eq!(wait(&s, 1), 0);
    assert_eq!(buffer, &data[500..]);
}

#[test]
fn memory_sources_and_decompressors() {
    let s = setup();
    let source = pattern(64);
    let mut plain = vec![0u8; 64];
    enqueue(
        &s,
        &Spec {
            memory_source: source.as_ptr(),
            size: 64,
            target: plain.as_mut_ptr() as usize,
            target_size: 64,
            ..Spec::default()
        },
    );
    mark_and_submit(&s, 2);
    assert_eq!(wait(&s, 2), 0);
    assert_eq!(plain, source);

    fn unxor(input: &[u8], output: &mut [u8]) -> Result<(), i32> {
        for (o, i) in output.iter_mut().zip(input) {
            *o = i ^ 0x5A;
        }
        Ok(())
    }
    set_decompressor(0xE, unxor);
    let packed: Vec<u8> = source.iter().map(|b| b ^ 0x5A).collect();
    let mut unpacked = vec![0u8; 64];
    enqueue(
        &s,
        &Spec {
            memory_source: packed.as_ptr(),
            size: 64,
            compression: 0xE,
            uncompressed: 64,
            target: unpacked.as_mut_ptr() as usize,
            target_size: 64,
            ..Spec::default()
        },
    );
    mark_and_submit(&s, 3);
    assert_eq!(wait(&s, 3), 0);
    assert_eq!(unpacked, source);
}

#[test]
fn an_unknown_compression_fails_the_status_and_the_queue_records_it() {
    let s = setup();
    let source = pattern(16);
    let mut out = vec![0u8; 16];
    enqueue(
        &s,
        &Spec {
            memory_source: source.as_ptr(),
            size: 16,
            compression: 0xC,
            uncompressed: 16,
            target: out.as_mut_ptr() as usize,
            target_size: 16,
            ..Spec::default()
        },
    );
    mark_and_submit(&s, 4);
    assert_eq!(wait(&s, 4), E_NOTIMPL);
    assert!(out.iter().all(|b| *b == 0));
    assert!(
        crate::trace::notes()
            .iter()
            .any(|n| n.contains("compression format 12"))
    );
    let mut record = [0u8; 16];
    // SAFETY: our own queue and a record large enough for the fields written.
    unsafe {
        let event: unsafe extern "win64" fn(*mut c_void) -> *mut c_void = method(s.queue, 9);
        assert!(!event(s.queue).is_null());
        let retrieve: unsafe extern "win64" fn(*mut c_void, *mut u8) = method(s.queue, 10);
        retrieve(s.queue, record.as_mut_ptr());
    }
    assert_eq!(u32::from_le_bytes(record[0..4].try_into().expect("4")), 1);
    assert_eq!(
        i32::from_le_bytes(record[4..8].try_into().expect("4")),
        E_NOTIMPL
    );
}

#[test]
fn gpu_destinations_go_through_the_callback() {
    static SEEN: Mutex<Option<(GpuDestination, Vec<u8>)>> = Mutex::new(None);
    fn copy(upload: &GpuUpload<'_>) -> i32 {
        *SEEN.lock().expect("lock") = Some((upload.destination.clone(), upload.data.to_vec()));
        0
    }
    let s = setup();
    let data = pattern(512);
    let (_dir, path) = temp_file(&data);
    let file = open_file(&s, &path);
    set_gpu_copy(Some(copy));
    enqueue(
        &s,
        &Spec {
            file,
            offset: 12,
            size: 100,
            destination: 1,
            target: 0xABC0,
            extra: 256,
            ..Spec::default()
        },
    );
    mark_and_submit(&s, 5);
    assert_eq!(wait(&s, 5), 0);
    let (destination, bytes) = SEEN.lock().expect("lock").take().expect("called");
    assert_eq!(
        destination,
        GpuDestination::Pages {
            table: 0xABC0,
            offset: 256
        }
    );
    assert_eq!(bytes, &data[12..112]);
    set_gpu_copy(None);
    enqueue(
        &s,
        &Spec {
            file,
            size: 4,
            destination: 1,
            target: 0xABC0,
            ..Spec::default()
        },
    );
    mark_and_submit(&s, 6);
    assert_eq!(wait(&s, 6), E_NOTIMPL);
}

#[test]
fn cancelled_requests_do_not_run_and_pending_status_reads_pending() {
    let s = setup();
    let source = pattern(8);
    let mut out = vec![0u8; 8];
    enqueue(
        &s,
        &Spec {
            memory_source: source.as_ptr(),
            size: 8,
            target: out.as_mut_ptr() as usize,
            target_size: 8,
            tag: 5,
            ..Spec::default()
        },
    );
    // SAFETY: our own queue and status array.
    unsafe {
        let cancel: unsafe extern "win64" fn(*mut c_void, u64, u64) = method(s.queue, 7);
        cancel(s.queue, 0xFF, 5);
        let result: unsafe extern "win64" fn(*mut c_void, u32) -> i32 = method(s.status, 4);
        assert_eq!(result(s.status, 7), E_PENDING);
    }
    mark_and_submit(&s, 7);
    assert_eq!(wait(&s, 7), 0);
    assert!(out.iter().all(|b| *b == 0));
}

#[test]
fn the_factory_reports_missing_files_and_traps_unknown_slots() {
    let s = setup();
    let missing: Vec<u16> = concat!(env!("CARGO_MANIFEST_DIR"), "/missing.bin")
        .encode_utf16()
        .chain([0])
        .collect();
    let mut file = std::ptr::null_mut();
    // SAFETY: a terminated path and out pointer; slot 12 is past the public methods.
    unsafe {
        let open: Open = method(s.factory, 4);
        assert_eq!(
            open(s.factory, missing.as_ptr(), IID_FILE.as_ptr(), &mut file),
            FILE_NOT_FOUND
        );
        let extra: unsafe extern "win64" fn(*mut c_void) -> i32 = method(s.factory, 12);
        assert_eq!(extra(s.factory), E_NOTIMPL);
        let mut other = std::ptr::null_mut();
        let query: unsafe extern "win64" fn(*mut c_void, *const u8, *mut *mut c_void) -> i32 =
            method(s.factory, 0);
        assert_eq!(query(s.factory, IID_UNKNOWN.as_ptr(), &mut other), 0);
        assert_eq!(other, s.factory);
        let mut info = [0xFFu8; 32];
        let query_info: unsafe extern "win64" fn(*mut c_void, *mut u8) = method(s.queue, 11);
        query_info(s.queue, info.as_mut_ptr());
        assert_eq!(u16::from_le_bytes([info[4], info[5]]), 64);
        assert_eq!(u32::from_le_bytes(info[24..28].try_into().expect("4")), 64);
    }
    assert!(
        crate::trace::notes()
            .iter()
            .any(|n| n.contains("IDStorageFactory slot 12"))
    );
}

#[test]
fn an_unexpected_interface_id_still_gets_a_factory() {
    let _s = setup();
    let unknown = [7u8; 16];
    let mut factory = std::ptr::null_mut();
    // SAFETY: valid pointers.
    assert_eq!(unsafe { get_factory(unknown.as_ptr(), &mut factory) }, 0);
    assert!(!factory.is_null());
}

#[test]
fn the_factory_slots_follow_the_executable_not_the_public_header() {
    let s = setup();
    // SAFETY: our own factory; slot 8 is `SetStagingBufferSize(UINT32)`, 3 and 6 are never called by the title.
    unsafe {
        let staging: unsafe extern "win64" fn(*mut c_void, u32) -> i32 = method(s.factory, 8);
        assert_eq!(staging(s.factory, 0x200_0000), 0);
        for slot in [3, 6, 7, 9] {
            let trapped: unsafe extern "win64" fn(*mut c_void) -> i32 = method(s.factory, slot);
            assert_eq!(trapped(s.factory), E_NOTIMPL, "slot {slot}");
        }
    }
}

#[test]
fn a_queue_description_reads_capacity_and_priority_where_the_console_puts_them() {
    let s = setup();
    let mut description = desc();
    description[8..10].copy_from_slice(&0x1234u16.to_le_bytes());
    description[10] = 2;
    let mut queue = std::ptr::null_mut();
    // SAFETY: our own factory and a console-sized description.
    unsafe {
        let create: Create = method(s.factory, 10);
        assert_eq!(
            create(
                s.factory,
                description.as_ptr(),
                IID_QUEUE.as_ptr(),
                &mut queue
            ),
            0
        );
        let mut info = [0u8; 32];
        let query: unsafe extern "win64" fn(*mut c_void, *mut u8) = method(queue, 11);
        query(queue, info.as_mut_ptr());
        assert_eq!(u16::from_le_bytes([info[4], info[5]]), 0x1234);
        assert_eq!(i16::from_le_bytes([info[6], info[7]]), 2);
    }
}

#[test]
fn deflate_requests_inflate_zlib_and_raw_streams() {
    use flate2::Compression;
    use flate2::write::{DeflateEncoder, ZlibEncoder};
    use std::io::Write;
    let s = setup();
    let data = pattern(40_000);
    let zlib = {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&data).expect("write");
        encoder.finish().expect("finish")
    };
    let raw = {
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&data).expect("write");
        encoder.finish().expect("finish")
    };
    for (index, stream) in [zlib, raw].iter().enumerate() {
        let mut out = vec![0u8; data.len()];
        enqueue(
            &s,
            &Spec {
                memory_source: stream.as_ptr(),
                size: stream.len() as u32,
                compression: 1,
                uncompressed: data.len() as u32,
                target: out.as_mut_ptr() as usize,
                target_size: data.len() as u32,
                ..Spec::default()
            },
        );
        mark_and_submit(&s, index as u32);
        assert_eq!(wait(&s, index as u32), 0);
        assert_eq!(out, data);
    }
}

#[test]
fn a_corrupt_deflate_stream_fails_the_request() {
    let s = setup();
    let junk = vec![0x78u8, 0x9C, 1, 2, 3, 4, 5, 6, 7, 8];
    let mut out = vec![0u8; 64];
    enqueue(
        &s,
        &Spec {
            memory_source: junk.as_ptr(),
            size: junk.len() as u32,
            compression: 1,
            uncompressed: 64,
            target: out.as_mut_ptr() as usize,
            target_size: 64,
            ..Spec::default()
        },
    );
    mark_and_submit(&s, 0);
    assert!(wait(&s, 0) < 0);
}

#[test]
fn many_requests_between_two_status_marks_all_finish() {
    let s = setup();
    let data = pattern(300_000);
    let (_dir, path) = temp_file(&data);
    let file = open_file(&s, &path);
    let mut buffers: Vec<Vec<u8>> = (0..24).map(|_| vec![0u8; 10_000]).collect();
    for (i, buffer) in buffers.iter_mut().enumerate() {
        enqueue(
            &s,
            &Spec {
                file,
                offset: (i * 10_000) as u64,
                size: 10_000,
                target: buffer.as_mut_ptr() as usize,
                target_size: 10_000,
                ..Spec::default()
            },
        );
    }
    mark_and_submit(&s, 0);
    assert_eq!(wait(&s, 0), 0);
    for (i, buffer) in buffers.iter().enumerate() {
        assert_eq!(
            buffer[..],
            data[i * 10_000..(i + 1) * 10_000],
            "request {i}"
        );
    }
}

#[test]
fn a_failed_request_in_a_group_gives_the_status_its_error() {
    let s = setup();
    let source = pattern(32);
    let mut good = vec![0u8; 32];
    let mut bad = vec![0u8; 32];
    for (compression, out) in [(0u8, &mut good), (0xC, &mut bad)] {
        enqueue(
            &s,
            &Spec {
                memory_source: source.as_ptr(),
                size: 32,
                compression,
                uncompressed: 32,
                target: out.as_mut_ptr() as usize,
                target_size: 32,
                ..Spec::default()
            },
        );
    }
    mark_and_submit(&s, 0);
    assert_eq!(wait(&s, 0), E_NOTIMPL);
    assert_eq!(good, source);
}

#[test]
fn page_destinations_split_over_consecutive_entries() {
    static WRITES: Mutex<Vec<(u64, usize, usize)>> = Mutex::new(Vec::new());
    fn write(page: u64, offset: usize, data: &[u8]) -> i32 {
        WRITES
            .lock()
            .expect("lock")
            .push((page, offset, data.len()));
        0
    }
    let s = setup();
    let data = pattern(150_000);
    let (_dir, path) = temp_file(&data);
    let file = open_file(&s, &path);
    let table: [u64; 4] = [7, 3, 9, 1];
    crate::dstorage::set_page_writer(Some(write));
    enqueue(
        &s,
        &Spec {
            file,
            size: 150_000,
            destination: 1,
            target: table.as_ptr() as usize,
            extra: 0x100,
            ..Spec::default()
        },
    );
    mark_and_submit(&s, 0);
    assert_eq!(wait(&s, 0), 0);
    crate::dstorage::set_page_writer(None);
    assert_eq!(
        *WRITES.lock().expect("lock"),
        vec![
            (7, 0x100, 0xFF00),
            (3, 0, 0x1_0000),
            (9, 0, 150_000 - 0xFF00 - 0x1_0000)
        ]
    );
}

#[test]
fn a_page_destination_lands_in_physical_pages_every_view_sees() {
    fn shim<T: Copy>(name: &str) -> T {
        let address = crate::builtin()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("{name} registered"))
            .address;
        // SAFETY: `T` is the function pointer type matching the shim's measured shape.
        unsafe { std::mem::transmute_copy(&address) }
    }
    type Reserve = unsafe extern "win64" fn(*mut c_void, usize, u32, u64, u32) -> *mut c_void;
    type Allocate = unsafe extern "win64" fn(usize, *mut usize, *mut usize) -> i32;
    type Map = unsafe extern "win64" fn(*mut c_void, usize, *const usize) -> i32;
    const PAGE: usize = 0x1_0000;
    let s = setup();
    let data = pattern(150_000);
    let (_dir, path) = temp_file(&data);
    let file = open_file(&s, &path);
    let reserve: Reserve = shim("XMemVirtualAlloc");
    let allocate: Allocate = shim("XMemAllocatePhysicalPages");
    let map: Map = shim("XMemMapPhysicalPages");
    // SAFETY: the shapes the executable uses; every pointer is valid.
    let (cpu, graphics, table) = unsafe {
        let cpu = reserve(std::ptr::null_mut(), 3 * PAGE, 0x2040_2000, 0x23, 0x2004);
        let graphics = reserve(std::ptr::null_mut(), 3 * PAGE, 0x2040_2000, 0x23, 0x2004);
        let mut count = 3usize;
        let mut pages = [0usize; 3];
        assert_eq!(allocate(0x280, &mut count, pages.as_mut_ptr()), 1);
        assert_eq!(map(cpu, 3, pages.as_ptr()), 1);
        assert_eq!(map(graphics, 3, pages.as_ptr()), 1);
        // The title's table lists the pages in the order the data fills them, not the order they were mapped.
        let table: [u64; 3] = [pages[2] as u64, pages[0] as u64, pages[1] as u64];
        (cpu as usize, graphics as usize, table)
    };
    crate::dstorage::set_page_writer(Some(crate::xmem::write_physical_page));
    enqueue(
        &s,
        &Spec {
            file,
            size: 150_000,
            destination: 1,
            target: table.as_ptr() as usize,
            extra: 0x100,
            ..Spec::default()
        },
    );
    mark_and_submit(&s, 0);
    let result = wait(&s, 0);
    crate::dstorage::set_page_writer(None);
    assert_eq!(result, 0);
    let first = PAGE - 0x100;
    for base in [cpu, graphics] {
        // SAFETY: three mapped pages of a reservation of ours.
        let view = unsafe { std::slice::from_raw_parts(base as *const u8, 3 * PAGE) };
        assert_eq!(&view[2 * PAGE + 0x100..], &data[..first], "first page");
        assert_eq!(&view[..PAGE], &data[first..first + PAGE], "second page");
        assert_eq!(
            &view[PAGE..PAGE + (150_000 - first - PAGE)],
            &data[first + PAGE..],
            "third page"
        );
    }
}
