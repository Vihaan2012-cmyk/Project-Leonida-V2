//! The COM objects the title sees. Layouts follow the public `dstorage.h`: the interface vtables, the request
//! and queue-description structures. Nothing here was measured on the console build; every object's slots past
//! the public ones are traps that note the call and fail with `E_NOTIMPL`.

use std::ffi::c_void;
use std::fs::File;
use std::os::windows::io::AsRawHandle;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
};

use super::engine::{
    Destination, E_INVALIDARG, E_NOTIMPL, GpuDestination, Item, Queue, QueueDesc, Request, S_OK,
    Source, StatusArray, win32_error,
};
use crate::trace::{image_offset, note};

const E_POINTER: i32 = 0x8000_4003u32 as i32;
const E_NOINTERFACE: i32 = 0x8000_4002u32 as i32;
const TRAP_SLOTS: usize = 32;

const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> [u8; 16] {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5],
        d4[6], d4[7],
    ]
}

const IID_UNKNOWN: [u8; 16] = guid(0, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_FACTORY: [u8; 16] = guid(
    0x6924ea0c,
    0xc3cd,
    0x4826,
    [0xb1, 0x0a, 0xf6, 0x4f, 0x4e, 0xd9, 0x27, 0xc1],
);
const IID_FILE: [u8; 16] = guid(
    0x5de95e7b,
    0x955a,
    0x4868,
    [0xa7, 0x3c, 0x24, 0x3b, 0x29, 0xf4, 0xb8, 0xda],
);
const IID_QUEUE: [u8; 16] = guid(
    0xcfdbd83f,
    0x9e06,
    0x4fda,
    [0x8e, 0xa5, 0x69, 0x04, 0x21, 0x37, 0xf4, 0x9b],
);
const IID_QUEUE1: [u8; 16] = guid(
    0xdd2f482c,
    0x5eff,
    0x41e8,
    [0x9c, 0x9e, 0xd2, 0x37, 0x4b, 0x27, 0x81, 0x28],
);
const IID_STATUS: [u8; 16] = guid(
    0x82397587,
    0x7cd5,
    0x453b,
    [0xa0, 0x2e, 0x31, 0x37, 0x9b, 0xd6, 0x46, 0x56],
);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u32)]
enum Kind {
    Factory,
    File,
    Queue,
    Status,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Factory => "IDStorageFactory",
            Kind::File => "IDStorageFile",
            Kind::Queue => "IDStorageQueue",
            Kind::Status => "IDStorageStatusArray",
        }
    }

    fn interfaces(self) -> &'static [[u8; 16]] {
        match self {
            Kind::Factory => &[IID_FACTORY],
            Kind::File => &[IID_FILE],
            Kind::Queue => &[IID_QUEUE, IID_QUEUE1],
            Kind::Status => &[IID_STATUS],
        }
    }
}

#[repr(C)]
struct Header {
    vtable: *const usize,
    refs: AtomicU32,
    kind: Kind,
}

#[repr(C)]
struct FactoryObject {
    header: Header,
}

#[repr(C)]
struct FileObject {
    header: Header,
    file: Arc<File>,
}

#[repr(C)]
struct QueueObject {
    header: Header,
    queue: Queue,
}

#[repr(C)]
struct StatusObject {
    header: Header,
    array: Arc<StatusArray>,
}

fn read_guid(riid: *const u8) -> Option<[u8; 16]> {
    if riid.is_null() {
        return None;
    }
    // SAFETY: COM passes a 16-byte IID.
    Some(unsafe { *(riid as *const [u8; 16]) })
}

fn guid_text(bytes: &[u8; 16]) -> String {
    let d1 = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let d2 = u16::from_le_bytes([bytes[4], bytes[5]]);
    let d3 = u16::from_le_bytes([bytes[6], bytes[7]]);
    let tail: String = bytes[8..].iter().map(|b| format!("{b:02x}")).collect();
    format!("{d1:08x}-{d2:04x}-{d3:04x}-{}-{}", &tail[..4], &tail[4..])
}

/// Notes an interface id the program asked for that is not one of the public ones.
fn check_requested(kind: Kind, riid: *const u8) {
    if let Some(id) = read_guid(riid)
        && !kind.interfaces().contains(&id)
        && id != IID_UNKNOWN
    {
        note(format!(
            "{} was requested as interface {}, which is not the public one; handed out anyway",
            kind.name(),
            guid_text(&id)
        ));
    }
}

unsafe extern "win64" fn query_interface(
    this: *mut Header,
    riid: *const u8,
    out: *mut *mut c_void,
) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: `this` is one of our objects.
    let kind = unsafe { (*this).kind };
    let Some(id) = read_guid(riid) else {
        return E_POINTER;
    };
    if id == IID_UNKNOWN || kind.interfaces().contains(&id) {
        // SAFETY: as above.
        unsafe { add_ref(this) };
        // SAFETY: `out` is writable, checked above.
        unsafe { *out = this.cast() };
        S_OK
    } else {
        note(format!(
            "{} asked for unknown interface {}",
            kind.name(),
            guid_text(&id)
        ));
        // SAFETY: `out` is writable, checked above.
        unsafe { *out = std::ptr::null_mut() };
        E_NOINTERFACE
    }
}

unsafe extern "win64" fn add_ref(this: *mut Header) -> u32 {
    // SAFETY: `this` is one of our objects.
    unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

unsafe extern "win64" fn release(this: *mut Header) -> u32 {
    // SAFETY: `this` is one of our objects.
    let left = unsafe { (*this).refs.fetch_sub(1, Ordering::AcqRel) } - 1;
    if left == 0 {
        // SAFETY: the last reference is gone; the object was made by `Box::into_raw` with the type its kind names.
        unsafe {
            match (*this).kind {
                Kind::Factory => {}
                Kind::File => drop(Box::from_raw(this.cast::<FileObject>())),
                Kind::Queue => drop(Box::from_raw(this.cast::<QueueObject>())),
                Kind::Status => drop(Box::from_raw(this.cast::<StatusObject>())),
            }
        }
    }
    left
}

unsafe extern "win64" fn trapped_method(this: *const Header, slot: usize, caller: usize) -> i32 {
    // SAFETY: `this` is one of our objects.
    let kind = unsafe { (*this).kind };
    note(format!(
        "{} slot {slot} called from {}; not implemented (the console build may add methods to the public ones)",
        kind.name(),
        image_offset(caller)
    ));
    E_NOTIMPL
}

fn new_object<T>(object: T) -> *mut c_void {
    Box::into_raw(Box::new(object)).cast()
}

fn header(kind: Kind) -> Header {
    Header {
        vtable: tables()[kind as usize] as *const usize,
        refs: AtomicU32::new(1),
        kind,
    }
}

// Factory.

unsafe fn hand_out(object: *mut c_void, out: *mut *mut c_void) -> i32 {
    if out.is_null() {
        // SAFETY: the object was just made with one reference.
        unsafe { release(object.cast()) };
        return E_POINTER;
    }
    // SAFETY: `out` is writable, checked above.
    unsafe { *out = object };
    S_OK
}

unsafe extern "win64" fn create_queue(
    _this: *mut Header,
    desc: *const u8,
    riid: *const u8,
    out: *mut *mut c_void,
) -> i32 {
    if desc.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    check_requested(Kind::Queue, riid);
    // SAFETY: `desc` is a `DSTORAGE_QUEUE_DESC`: source type (i32), capacity (u16), priority (i16), name, device.
    let description = unsafe {
        QueueDesc {
            source_type: std::ptr::read_unaligned(desc.cast::<i32>()),
            capacity: std::ptr::read_unaligned(desc.add(4).cast::<u16>()),
            priority: std::ptr::read_unaligned(desc.add(6).cast::<i16>()),
            name: std::ptr::read_unaligned(desc.add(8).cast::<usize>()),
            device: std::ptr::read_unaligned(desc.add(16).cast::<usize>()),
        }
    };
    let object = new_object(QueueObject {
        header: header(Kind::Queue),
        queue: Queue::new(description),
    });
    // SAFETY: `out` was checked.
    unsafe { hand_out(object, out) }
}

unsafe extern "win64" fn open_file(
    _this: *mut Header,
    path: *const u16,
    riid: *const u8,
    out: *mut *mut c_void,
) -> i32 {
    if path.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    check_requested(Kind::File, riid);
    // SAFETY: a terminated UTF-16 path.
    let length = (0..32767)
        .take_while(|i| unsafe { *path.add(*i) } != 0)
        .count();
    // SAFETY: `length` characters were just read.
    let text = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(path, length) });
    let location = match crate::files::read_location(&text) {
        Ok(location) => location,
        Err(code) => return win32_error(code),
    };
    let file = match File::open(&location) {
        Ok(file) => file,
        Err(e) => {
            return e
                .raw_os_error()
                .map_or(E_NOTIMPL, |c| win32_error(c as u32));
        }
    };
    let object = new_object(FileObject {
        header: header(Kind::File),
        file: Arc::new(file),
    });
    // SAFETY: `out` was checked.
    unsafe { hand_out(object, out) }
}

unsafe extern "win64" fn create_status_array(
    _this: *mut Header,
    capacity: u32,
    _name: *const u8,
    riid: *const u8,
    out: *mut *mut c_void,
) -> i32 {
    if out.is_null() {
        return E_INVALIDARG;
    }
    check_requested(Kind::Status, riid);
    let object = new_object(StatusObject {
        header: header(Kind::Status),
        array: Arc::new(StatusArray::new(capacity)),
    });
    // SAFETY: `out` was checked.
    unsafe { hand_out(object, out) }
}

unsafe extern "win64" fn set_debug_flags(_this: *mut Header, _flags: u32) {}

unsafe extern "win64" fn set_staging_buffer_size(_this: *mut Header, _size: u32) -> i32 {
    S_OK
}

// File.

unsafe extern "win64" fn file_close(_this: *mut Header) {}

unsafe extern "win64" fn get_file_information(
    this: *mut FileObject,
    info: *mut BY_HANDLE_FILE_INFORMATION,
) -> i32 {
    if info.is_null() {
        return E_POINTER;
    }
    // SAFETY: `this` is a file object and `info` is the program's output record.
    let ok = unsafe { GetFileInformationByHandle((*this).file.as_raw_handle() as HANDLE, info) };
    if ok != 0 {
        S_OK
    } else {
        win32_error(std::io::Error::last_os_error().raw_os_error().unwrap_or(5) as u32)
    }
}

// Status array.

unsafe extern "win64" fn status_is_complete(this: *mut StatusObject, index: u32) -> i32 {
    // SAFETY: `this` is a status array object.
    i32::from(unsafe { (*this).array.is_complete(index) })
}

unsafe extern "win64" fn status_get_hresult(this: *mut StatusObject, index: u32) -> i32 {
    // SAFETY: `this` is a status array object.
    unsafe { (*this).array.result(index) }
}

// Queue.

fn word<T: Copy>(base: *const u8, offset: usize) -> T {
    // SAFETY: callers pass offsets inside the 96-byte `DSTORAGE_REQUEST` they were handed.
    unsafe { std::ptr::read_unaligned(base.add(offset).cast::<T>()) }
}

/// Parses a `DSTORAGE_REQUEST`: options (source type bit 0, destination type bits 1-7, compression byte 1),
/// the source union at 8, the destination union at 32, the uncompressed size at 72, the tag at 80, the name at 88.
unsafe fn parse_request(base: *const u8) -> Result<Request, String> {
    let options: u64 = word(base, 0);
    let source_type = options & 1;
    let destination_type = (options >> 1) & 0x7F;
    let compression = ((options >> 8) & 0xFF) as u8;
    let source = if source_type == 0 {
        let file: usize = word(base, 8);
        if file == 0 {
            return Err("file request with no file".into());
        }
        let header = file as *const Header;
        // SAFETY: the program passes an `IDStorageFile` it got from our factory.
        if unsafe { (*header).kind } != Kind::File {
            return Err("file request names an object that is not one of our files".into());
        }
        // SAFETY: checked just above.
        let file = unsafe { (*(file as *const FileObject)).file.clone() };
        Source::File {
            file,
            offset: word(base, 16),
            size: word(base, 24),
        }
    } else {
        Source::Memory {
            address: word(base, 8),
            size: word(base, 16),
        }
    };
    let resource: usize = word(base, 32);
    let gpu = |destination| Destination::Gpu(destination);
    let destination = match destination_type {
        0 => Destination::Memory {
            address: word(base, 32),
            size: word(base, 40),
        },
        1 => gpu(GpuDestination::Buffer {
            resource,
            offset: word(base, 40),
        }),
        2 => {
            let mut region = [0u32; 6];
            for (i, slot) in region.iter_mut().enumerate() {
                *slot = word(base, 44 + 4 * i);
            }
            gpu(GpuDestination::TextureRegion {
                resource,
                subresource: word(base, 40),
                region,
            })
        }
        3 => gpu(GpuDestination::MultipleSubresources {
            resource,
            first_subresource: word(base, 40),
        }),
        4 => {
            let mut coordinate = [0u32; 4];
            let mut size = [0u32; 4];
            for i in 0..4 {
                coordinate[i] = word(base, 40 + 4 * i);
                size[i] = word(base, 56 + 4 * i);
            }
            gpu(GpuDestination::Tiles {
                resource,
                coordinate,
                size,
            })
        }
        other => return Err(format!("destination type {other}")),
    };
    let name_pointer: usize = word(base, 88);
    let name = if name_pointer == 0 {
        String::new()
    } else {
        // SAFETY: a request name is a terminated 8-bit string.
        let length = (0..256)
            .take_while(|i| unsafe { *(name_pointer as *const u8).add(*i) } != 0)
            .count();
        // SAFETY: `length` bytes were just read.
        String::from_utf8_lossy(unsafe {
            std::slice::from_raw_parts(name_pointer as *const u8, length)
        })
        .into_owned()
    };
    Ok(Request::new(
        source,
        destination,
        compression,
        word(base, 72),
        word(base, 80),
        name,
    ))
}

unsafe extern "win64" fn enqueue_request(this: *mut QueueObject, request: *const u8) {
    if request.is_null() {
        return;
    }
    // SAFETY: the program passes a `DSTORAGE_REQUEST` of `REQUEST_SIZE` bytes.
    match unsafe { parse_request(request) } {
        // SAFETY: `this` is a queue object.
        Ok(parsed) => unsafe { (*this).queue.enqueue(Item::Request(parsed)) },
        Err(reason) => note(format!("DirectStorage request refused: {reason}")),
    }
}

unsafe extern "win64" fn enqueue_status(this: *mut QueueObject, status: *mut Header, index: u32) {
    // SAFETY: `status` is an object of ours when its kind says so.
    if status.is_null() || unsafe { (*status).kind } != Kind::Status {
        note("DirectStorage EnqueueStatus with an object that is not a status array".to_string());
        return;
    }
    // SAFETY: checked above; `this` is a queue object.
    unsafe {
        let array = (*status.cast::<StatusObject>()).array.clone();
        array.reset(index);
        (*this).queue.enqueue(Item::Status { array, index });
    }
}

unsafe extern "win64" fn enqueue_signal(this: *mut QueueObject, fence: *mut c_void, value: u64) {
    // SAFETY: `this` is a queue object.
    unsafe {
        (*this).queue.enqueue(Item::Signal {
            fence: fence as usize,
            value,
        })
    };
}

unsafe extern "win64" fn enqueue_set_event(this: *mut QueueObject, event: HANDLE) {
    // SAFETY: `this` is a queue object.
    unsafe { (*this).queue.enqueue(Item::Event(event as usize)) };
}

unsafe extern "win64" fn queue_submit(this: *mut QueueObject) {
    // SAFETY: `this` is a queue object.
    unsafe { (*this).queue.submit() };
}

unsafe extern "win64" fn queue_cancel(this: *mut QueueObject, mask: u64, value: u64) {
    // SAFETY: `this` is a queue object.
    unsafe { (*this).queue.cancel(mask, value) };
}

unsafe extern "win64" fn queue_close(this: *mut QueueObject) {
    // SAFETY: `this` is a queue object.
    unsafe { (*this).queue.close() };
}

unsafe extern "win64" fn queue_error_event(this: *mut QueueObject) -> HANDLE {
    // SAFETY: `this` is a queue object.
    unsafe { (*this).queue.error_event() as HANDLE }
}

/// `DSTORAGE_ERROR_RECORD`: failure count, then the first failure's `HRESULT` and command type. The rest of the
/// record (the failing request's file name and description) is left as the program had it.
unsafe extern "win64" fn queue_retrieve_error(this: *mut QueueObject, record: *mut u8) {
    if record.is_null() {
        return;
    }
    // SAFETY: `this` is a queue object; `record` is the program's error record, at least 12 bytes.
    unsafe {
        let error = (*this).queue.take_error();
        std::ptr::write_unaligned(record.cast::<u32>(), error.count);
        std::ptr::write_unaligned(record.add(4).cast::<i32>(), error.first);
        std::ptr::write_unaligned(record.add(8).cast::<u32>(), u32::from(error.count > 0));
    }
}

/// `DSTORAGE_QUEUE_INFO`: the queue description (24 bytes), empty slots, requests until automatic submit.
unsafe extern "win64" fn queue_query(this: *mut QueueObject, info: *mut u8) {
    if info.is_null() {
        return;
    }
    // SAFETY: `this` is a queue object; `info` is the program's 32-byte record.
    unsafe {
        let queue = &(*this).queue;
        std::ptr::write_unaligned(info.cast::<i32>(), queue.desc.source_type);
        std::ptr::write_unaligned(info.add(4).cast::<u16>(), queue.desc.capacity);
        std::ptr::write_unaligned(info.add(6).cast::<i16>(), queue.desc.priority);
        std::ptr::write_unaligned(info.add(8).cast::<usize>(), queue.desc.name);
        std::ptr::write_unaligned(info.add(16).cast::<usize>(), queue.desc.device);
        std::ptr::write_unaligned(info.add(24).cast::<u32>(), queue.empty_slots());
        std::ptr::write_unaligned(info.add(28).cast::<u32>(), 0);
    }
}

// Tables.

static TABLES: OnceLock<[usize; 4]> = OnceLock::new();

fn tables() -> [usize; 4] {
    *TABLES.get_or_init(|| {
        let mut code = crate::code::Code::default();
        let common = [
            query_interface as *const () as usize,
            add_ref as *const () as usize,
            release as *const () as usize,
        ];
        let methods: [Vec<usize>; 4] = [
            vec![
                create_queue as *const () as usize,
                open_file as *const () as usize,
                create_status_array as *const () as usize,
                set_debug_flags as *const () as usize,
                set_staging_buffer_size as *const () as usize,
            ],
            vec![
                file_close as *const () as usize,
                get_file_information as *const () as usize,
            ],
            vec![
                enqueue_request as *const () as usize,
                enqueue_status as *const () as usize,
                enqueue_signal as *const () as usize,
                queue_submit as *const () as usize,
                queue_cancel as *const () as usize,
                queue_close as *const () as usize,
                queue_error_event as *const () as usize,
                queue_retrieve_error as *const () as usize,
                queue_query as *const () as usize,
                enqueue_set_event as *const () as usize,
            ],
            vec![
                status_is_complete as *const () as usize,
                status_get_hresult as *const () as usize,
            ],
        ];
        let mut addresses = [0usize; 4];
        for (kind, methods) in methods.iter().enumerate() {
            let mut entries: Vec<usize> = common.to_vec();
            entries.extend(methods);
            for slot in entries.len()..TRAP_SLOTS {
                entries.push(
                    code.method_trap(slot, trapped_method as *const () as usize)
                        .unwrap_or(0),
                );
            }
            addresses[kind] = Box::leak(entries.into_boxed_slice()).as_ptr() as usize;
        }
        if code.seal().is_err() {
            return [0; 4];
        }
        std::mem::forget(code);
        addresses
    })
}

static FACTORY: OnceLock<usize> = OnceLock::new();

/// `DStorageGetFactory(REFIID, void**)`. The factory is one object for the whole process, and a missing or
/// unexpected interface id still gets it, because the title treats a null factory as fatal.
pub unsafe extern "win64" fn get_factory(riid: *const u8, out: *mut *mut c_void) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    if tables()[0] == 0 {
        return E_NOTIMPL;
    }
    check_requested(Kind::Factory, riid);
    let factory = *FACTORY.get_or_init(|| {
        new_object(FactoryObject {
            header: header(Kind::Factory),
        }) as usize
    });
    // SAFETY: the factory is never freed; `out` is writable, checked above.
    unsafe {
        add_ref(factory as *mut Header);
        *out = factory as *mut c_void;
    }
    S_OK
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
