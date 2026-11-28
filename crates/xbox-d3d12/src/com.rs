//! COM objects for the console interfaces: a common header, reference counting, `QueryInterface` against
//! the PC and console IIDs, vtables longer than the highest slot the title uses, and a trap for the slots
//! this layer does not implement (it notes them and fails; a slot is never null).
//!
//! Every object starts with `ComHeader`; the Rust state follows it. The console runtime passes struct
//! returns as `(buffer, this, ...)`, which each such method here reads that way.

use std::collections::HashSet;
use std::ffi::c_void;
use std::fmt;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::thunks;

pub type Hresult = u32;
pub const S_OK: Hresult = 0;
pub const S_FALSE: Hresult = 1;
pub const E_NOTIMPL: Hresult = 0x8000_4001;
pub const E_NOINTERFACE: Hresult = 0x8000_4002;
pub const E_POINTER: Hresult = 0x8000_4003;
pub const E_FAIL: Hresult = 0x8000_4005;
pub const E_UNEXPECTED: Hresult = 0x8000_FFFF;
pub const E_INVALIDARG: Hresult = 0x8007_0057;
pub const E_OUTOFMEMORY: Hresult = 0x8007_000E;
pub const DXGI_ERROR_NOT_FOUND: Hresult = 0x887A_0002;

/// A GUID in its in-memory layout.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Guid {
    pub d1: u32,
    pub d2: u16,
    pub d3: u16,
    pub d4: [u8; 8],
}

impl Guid {
    pub const fn new(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Guid {
        Guid { d1, d2, d3, d4 }
    }

    /// # Safety
    /// `p` points at 16 readable bytes.
    pub unsafe fn read(p: *const u8) -> Guid {
        // SAFETY: as the caller promises; unaligned because COM callers do not align IIDs.
        unsafe { std::ptr::read_unaligned(p as *const Guid) }
    }
}

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            self.d1,
            self.d2,
            self.d3,
            self.d4[0],
            self.d4[1],
            self.d4[2],
            self.d4[3],
            self.d4[4],
            self.d4[5],
            self.d4[6],
            self.d4[7]
        )
    }
}

/// The IIDs the layer answers, from the Windows SDK headers (the console runtime's D3D12 IIDs are the
/// same bytes) and the console-only DMA list IID reported by prior art.
pub mod iid {
    use super::Guid;

    pub const UNKNOWN: Guid = Guid::new(0, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
    /// `IID_ID3D12ConsDmaCommandList`.
    pub const CONS_DMA_COMMAND_LIST: Guid = Guid::new(
        0x30bc77b9,
        0x8379,
        0x4b25,
        [0x95, 0xde, 0xb8, 0x58, 0x8c, 0x42, 0x27, 0x5d],
    );
    pub const OBJECT: Guid = Guid::new(
        0xc4fec28f,
        0x7966,
        0x4e95,
        [0x9f, 0x94, 0xf4, 0x31, 0xcb, 0x56, 0xc3, 0xb8],
    );
    pub const DEVICE_CHILD: Guid = Guid::new(
        0x905db94b,
        0xa00c,
        0x4140,
        [0x9d, 0xf5, 0x2b, 0x64, 0xca, 0x9e, 0xa3, 0x57],
    );
    pub const ROOT_SIGNATURE: Guid = Guid::new(
        0xc54a6b66,
        0x72df,
        0x4ee8,
        [0x8b, 0xe5, 0xa9, 0x46, 0xa1, 0x42, 0x92, 0x14],
    );
    pub const PAGEABLE: Guid = Guid::new(
        0x63ee58fb,
        0x1268,
        0x4835,
        [0x86, 0xda, 0xf0, 0x08, 0xce, 0x62, 0xf0, 0xd6],
    );
    pub const HEAP: Guid = Guid::new(
        0x6b3b2502,
        0x6e51,
        0x45b3,
        [0x90, 0xee, 0x98, 0x84, 0x26, 0x5e, 0x8d, 0xf3],
    );
    pub const RESOURCE: Guid = Guid::new(
        0x696442be,
        0xa72e,
        0x4059,
        [0xbc, 0x79, 0x5b, 0x5c, 0x98, 0x04, 0x0f, 0xad],
    );
    pub const COMMAND_ALLOCATOR: Guid = Guid::new(
        0x6102dee4,
        0xaf59,
        0x4b09,
        [0xb9, 0x99, 0xb4, 0x4d, 0x73, 0xf0, 0x9b, 0x24],
    );
    pub const FENCE: Guid = Guid::new(
        0x0a753dcf,
        0xc4d8,
        0x4b91,
        [0xad, 0xf6, 0xbe, 0x5a, 0x60, 0xd9, 0x5a, 0x76],
    );
    pub const FENCE1: Guid = Guid::new(
        0x433685fe,
        0xe22b,
        0x4ca0,
        [0xa8, 0xdb, 0xb5, 0xb4, 0xf4, 0xdd, 0x0e, 0x4a],
    );
    pub const PIPELINE_STATE: Guid = Guid::new(
        0x765a30f3,
        0xf624,
        0x4c6f,
        [0xa8, 0x28, 0xac, 0xe9, 0x48, 0x62, 0x24, 0x45],
    );
    pub const DESCRIPTOR_HEAP: Guid = Guid::new(
        0x8efb471d,
        0x616c,
        0x4f49,
        [0x90, 0xf7, 0x12, 0x7b, 0xb7, 0x63, 0xfa, 0x51],
    );
    pub const QUERY_HEAP: Guid = Guid::new(
        0x0d9658ae,
        0xed45,
        0x469e,
        [0xa6, 0x1d, 0x97, 0x0e, 0xc5, 0x83, 0xca, 0xb4],
    );
    pub const COMMAND_SIGNATURE: Guid = Guid::new(
        0xc36a797c,
        0xec80,
        0x4f0a,
        [0x89, 0x85, 0xa7, 0xb2, 0x47, 0x50, 0x82, 0xd1],
    );
    pub const COMMAND_LIST: Guid = Guid::new(
        0x7116d91c,
        0xe7e4,
        0x47ce,
        [0xb8, 0xc6, 0xec, 0x81, 0x68, 0xf4, 0x37, 0xe5],
    );
    pub const GRAPHICS_COMMAND_LIST: Guid = Guid::new(
        0x5b160d0f,
        0xac1b,
        0x4185,
        [0x8b, 0xa8, 0xb3, 0xae, 0x42, 0xa5, 0xa4, 0x55],
    );
    pub const GRAPHICS_COMMAND_LIST1: Guid = Guid::new(
        0x553103fb,
        0x1fe7,
        0x4557,
        [0xbb, 0x38, 0x94, 0x6d, 0x7d, 0x0e, 0x7c, 0xa7],
    );
    pub const GRAPHICS_COMMAND_LIST2: Guid = Guid::new(
        0x38c3e585,
        0xff17,
        0x412c,
        [0x91, 0x50, 0x4F, 0xC6, 0xF9, 0xD7, 0x2A, 0x28],
    );
    pub const COMMAND_QUEUE: Guid = Guid::new(
        0x0ec870a6,
        0x5d7e,
        0x4c22,
        [0x8c, 0xfc, 0x5b, 0xaa, 0xe0, 0x76, 0x16, 0xed],
    );
    pub const DEVICE: Guid = Guid::new(
        0x189819f1,
        0x1db6,
        0x4b57,
        [0xbe, 0x54, 0x18, 0x21, 0x33, 0x9b, 0x85, 0xf7],
    );
    pub const DEVICE1: Guid = Guid::new(
        0x77acce80,
        0x638e,
        0x4e65,
        [0x88, 0x95, 0xc1, 0xf2, 0x33, 0x86, 0x86, 0x3e],
    );
    pub const DEVICE2: Guid = Guid::new(
        0x30baa41e,
        0xb15b,
        0x475c,
        [0xa0, 0xbb, 0x1a, 0xf5, 0xc5, 0xb6, 0x43, 0x28],
    );
    pub const DEVICE3: Guid = Guid::new(
        0x81dadc15,
        0x2bad,
        0x4392,
        [0x93, 0xc5, 0x10, 0x13, 0x45, 0xc4, 0xaa, 0x98],
    );
    pub const DEVICE4: Guid = Guid::new(
        0xe865df17,
        0xa9ee,
        0x46f9,
        [0xa4, 0x63, 0x30, 0x98, 0x31, 0x5a, 0xa2, 0xe5],
    );
    pub const DEVICE5: Guid = Guid::new(
        0x8b4f173b,
        0x2fea,
        0x4b80,
        [0x8f, 0x58, 0x43, 0x07, 0x19, 0x1a, 0xb9, 0x5d],
    );
    pub const DEVICE6: Guid = Guid::new(
        0xc70b221b,
        0x40e4,
        0x4a17,
        [0x89, 0xaf, 0x02, 0x5a, 0x07, 0x27, 0xa6, 0xdc],
    );
    pub const DEVICE7: Guid = Guid::new(
        0x5c014b53,
        0x68a1,
        0x4b9b,
        [0x8b, 0xd1, 0xdd, 0x60, 0x46, 0xb9, 0x35, 0x8b],
    );
    pub const DEVICE8: Guid = Guid::new(
        0x9218e6bb,
        0xf944,
        0x4f7e,
        [0xA7, 0x5C, 0xB1, 0xB2, 0xC7, 0xB7, 0x01, 0xF3],
    );
    pub const GRAPHICS_COMMAND_LIST3: Guid = Guid::new(
        0x6fda83a7,
        0xb84c,
        0x4e38,
        [0x9A, 0xC8, 0xC7, 0xBD, 0x22, 0x01, 0x6B, 0x3D],
    );
    pub const GRAPHICS_COMMAND_LIST4: Guid = Guid::new(
        0x8754318e,
        0xd3a9,
        0x4541,
        [0x98, 0xcf, 0x64, 0x5b, 0x50, 0xdc, 0x48, 0x74],
    );
    pub const DEVICE9: Guid = Guid::new(
        0x4c80e962,
        0xf032,
        0x4f60,
        [0xbc, 0x9e, 0xeb, 0xc2, 0xcf, 0xa1, 0xd8, 0x3c],
    );
    pub const DEVICE10: Guid = Guid::new(
        0x517f8718,
        0xaa66,
        0x49f9,
        [0xb0, 0x2b, 0xa7, 0xab, 0x89, 0xc0, 0x60, 0x31],
    );
    pub const DEVICE11: Guid = Guid::new(
        0x5405c344,
        0xd457,
        0x444e,
        [0xb4, 0xdd, 0x23, 0x66, 0xe4, 0x5a, 0xee, 0x39],
    );
    pub const DEVICE12: Guid = Guid::new(
        0x5af5c532,
        0x4c91,
        0x4cd0,
        [0xb5, 0x41, 0x15, 0xa4, 0x05, 0x39, 0x5f, 0xc5],
    );
    pub const DEVICE13: Guid = Guid::new(
        0x14eecffc,
        0x4df8,
        0x40f7,
        [0xa1, 0x18, 0x5c, 0x81, 0x6f, 0x45, 0x69, 0x5e],
    );
    pub const DEVICE14: Guid = Guid::new(
        0x5f6e592d,
        0xd895,
        0x44c2,
        [0x8e, 0x4a, 0x88, 0xad, 0x49, 0x26, 0xd3, 0x23],
    );
    pub const GRAPHICS_COMMAND_LIST5: Guid = Guid::new(
        0x55050859,
        0x4024,
        0x474c,
        [0x87, 0xf5, 0x64, 0x72, 0xea, 0xee, 0x44, 0xea],
    );
    pub const GRAPHICS_COMMAND_LIST6: Guid = Guid::new(
        0xc3827890,
        0xe548,
        0x4cfa,
        [0x96, 0xcf, 0x56, 0x89, 0xa9, 0x37, 0x0f, 0x80],
    );
    pub const GRAPHICS_COMMAND_LIST7: Guid = Guid::new(
        0xdd171223,
        0x8b61,
        0x4769,
        [0x90, 0xe3, 0x16, 0x0c, 0xcd, 0xe4, 0xe2, 0xc1],
    );
    pub const GRAPHICS_COMMAND_LIST8: Guid = Guid::new(
        0xee936ef9,
        0x599d,
        0x4d28,
        [0x93, 0x8e, 0x23, 0xc4, 0xad, 0x05, 0xce, 0x51],
    );
    pub const GRAPHICS_COMMAND_LIST9: Guid = Guid::new(
        0x34ed2808,
        0xffe6,
        0x4c2b,
        [0xb1, 0x1a, 0xca, 0xbd, 0x2b, 0x0c, 0x59, 0xe1],
    );
    pub const GRAPHICS_COMMAND_LIST10: Guid = Guid::new(
        0x7013c015,
        0xd161,
        0x4b63,
        [0xa0, 0x8c, 0x23, 0x85, 0x52, 0xdd, 0x8a, 0xcc],
    );
}

/// What an object is; it picks the slot table and the interfaces `QueryInterface` answers.
#[repr(u32)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    Device = 1,
    CommandQueue,
    CommandAllocator,
    CommandList,
    Fence,
    DescriptorHeap,
    Heap,
    Resource,
    RootSignature,
    PipelineState,
    QueryHeap,
    CommandSignature,
    Other,
    Trap,
}

impl Kind {
    /// The interface name `d3d12_slots.txt` knows this kind by.
    pub fn interface(self) -> &'static str {
        match self {
            Kind::Device => "ID3D12Device",
            Kind::CommandQueue => "ID3D12CommandQueue",
            Kind::CommandAllocator => "ID3D12CommandAllocator",
            Kind::CommandList => "ID3D12GraphicsCommandList",
            Kind::Fence => "ID3D12Fence",
            Kind::DescriptorHeap => "ID3D12DescriptorHeap",
            Kind::Heap => "ID3D12Heap",
            Kind::Resource => "ID3D12Resource",
            Kind::RootSignature => "ID3D12RootSignature",
            Kind::PipelineState => "ID3D12PipelineState",
            Kind::QueryHeap => "ID3D12QueryHeap",
            Kind::CommandSignature => "ID3D12CommandSignature",
            Kind::Other => "object",
            Kind::Trap => "trap",
        }
    }

    fn from_u32(value: u32) -> Option<Kind> {
        const ALL: [Kind; 14] = [
            Kind::Device,
            Kind::CommandQueue,
            Kind::CommandAllocator,
            Kind::CommandList,
            Kind::Fence,
            Kind::DescriptorHeap,
            Kind::Heap,
            Kind::Resource,
            Kind::RootSignature,
            Kind::PipelineState,
            Kind::QueryHeap,
            Kind::CommandSignature,
            Kind::Other,
            Kind::Trap,
        ];
        ALL.into_iter().find(|k| *k as u32 == value)
    }
}

/// The first words of every object. The vtable pointer comes first, as COM requires.
#[repr(C)]
pub struct ComHeader {
    pub vtable: *const usize,
    refs: AtomicU32,
    kind: u32,
    destroy: unsafe fn(*mut ComHeader),
}

/// An object: the header, then the Rust state at offset `0x18`.
#[repr(C)]
pub struct Object<T> {
    pub head: ComHeader,
    pub value: T,
}

/// Where the Rust state of an `Object` starts.
pub const STATE_OFFSET: usize = std::mem::size_of::<ComHeader>();

unsafe fn destroy_box<T>(head: *mut ComHeader) {
    // SAFETY: `head` is the start of a `Box<Object<T>>` made by `new_object`, released once.
    drop(unsafe { Box::from_raw(head as *mut Object<T>) });
}

/// Allocates an object with one reference.
pub fn new_object<T>(kind: Kind, vtable: *const usize, value: T) -> *mut Object<T> {
    Box::into_raw(Box::new(Object {
        head: ComHeader {
            vtable,
            refs: AtomicU32::new(1),
            kind: kind as u32,
            destroy: destroy_box::<T>,
        },
        value,
    }))
}

/// The state of an object of this layer.
///
/// # Safety
/// `this` is an `Object<T>` made by `new_object` and alive for `'a`.
pub unsafe fn state<'a, T>(this: *const c_void) -> &'a T {
    // SAFETY: as the caller promises.
    unsafe { &(*(this as *const Object<T>)).value }
}

/// The kind of an object made by this layer.
///
/// # Safety
/// `this` is an object made by `new_object`.
pub unsafe fn kind_of(this: *const c_void) -> Option<Kind> {
    // SAFETY: as the caller promises.
    Kind::from_u32(unsafe { (*(this as *const ComHeader)).kind })
}

/// `AddRef`.
///
/// # Safety
/// `this` is an object of this layer.
pub unsafe extern "win64" fn add_ref(this: *mut c_void) -> u32 {
    // SAFETY: as the caller promises.
    unsafe {
        (*(this as *const ComHeader))
            .refs
            .fetch_add(1, Ordering::AcqRel)
            + 1
    }
}

/// `Release`; frees the object at zero.
///
/// # Safety
/// `this` is an object of this layer with a reference to give up.
pub unsafe extern "win64" fn release(this: *mut c_void) -> u32 {
    let head = this as *mut ComHeader;
    // SAFETY: as the caller promises.
    let left = unsafe { (*head).refs.fetch_sub(1, Ordering::AcqRel) }.saturating_sub(1);
    if left == 0 {
        // SAFETY: as the caller promises.
        let kind = unsafe { (*head).kind };
        if kind == Kind::Resource as u32 && keep_resources() {
            note_once(format!(
                "a resource reached zero references and is kept ({this:p})"
            ));
            return left;
        }
        // SAFETY: the last reference is gone, so nothing else uses the object.
        unsafe { ((*head).destroy)(head) };
    }
    left
}

/// `LEONIDA_KEEP_RESOURCES`: resources that reach zero references stay alive, to tell a lifetime problem from
/// another one.
fn keep_resources() -> bool {
    static KEEP: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *KEEP.get_or_init(|| std::env::var_os("LEONIDA_KEEP_RESOURCES").is_some())
}

/// A counted reference to a COM object, ours or another layer's: it calls `AddRef` and `Release` through
/// the object's own vtable.
pub struct ComPtr(NonNull<c_void>);

// SAFETY: COM objects of this layer are reference counted atomically and their state is `Sync`.
unsafe impl Send for ComPtr {}
// SAFETY: as above.
unsafe impl Sync for ComPtr {}

type RefFn = unsafe extern "win64" fn(*mut c_void) -> u32;

impl ComPtr {
    /// Takes a new reference to `p`; `None` for null.
    ///
    /// # Safety
    /// `p` is null or a live COM object.
    pub unsafe fn add_ref(p: *mut c_void) -> Option<ComPtr> {
        let ptr = NonNull::new(p)?;
        // SAFETY: a COM object's first word is its vtable, whose slot 1 is `AddRef`.
        unsafe {
            let f: RefFn = std::mem::transmute(*(*(p as *const *const usize)).add(1));
            f(p);
        }
        Some(ComPtr(ptr))
    }

    /// Adopts the reference the caller already holds.
    ///
    /// # Safety
    /// `p` is a live COM object whose reference the caller gives away.
    pub unsafe fn adopt(p: *mut c_void) -> Option<ComPtr> {
        NonNull::new(p).map(ComPtr)
    }

    pub fn as_ptr(&self) -> *mut c_void {
        self.0.as_ptr()
    }

    /// Gives the reference to the caller without releasing it.
    pub fn into_raw(self) -> *mut c_void {
        let p = self.0.as_ptr();
        std::mem::forget(self);
        p
    }
}

impl Clone for ComPtr {
    fn clone(&self) -> ComPtr {
        // SAFETY: `self` holds a live object.
        unsafe { ComPtr::add_ref(self.as_ptr()).expect("non-null") }
    }
}

impl Drop for ComPtr {
    fn drop(&mut self) {
        let p = self.0.as_ptr();
        // SAFETY: `self` holds a reference to a live object; slot 2 is `Release`.
        unsafe {
            let f: RefFn = std::mem::transmute(*(*(p as *const *const usize)).add(2));
            f(p);
        }
    }
}

impl fmt::Debug for ComPtr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ComPtr({:p})", self.0)
    }
}

// Trace

type TraceFn = fn(String);
type AddressFn = fn(usize) -> String;

static TRACE: OnceLock<TraceFn> = OnceLock::new();
static ADDRESS: OnceLock<AddressFn> = OnceLock::new();
static NOTES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static NOTED: Mutex<Option<HashSet<String>>> = Mutex::new(None);
const NOTE_LIMIT: usize = 4096;

/// Sends every note to `f` as well (the runtime's trace). Only the first call counts.
pub fn set_trace(f: TraceFn) {
    let _ = TRACE.set(f);
}

/// How a caller's address is written in notes (the runtime gives image offsets).
pub fn set_address_format(f: AddressFn) {
    let _ = ADDRESS.set(f);
}

/// Records a trace note.
pub fn note(text: impl Into<String>) {
    let text = text.into();
    if let Some(f) = TRACE.get() {
        f(text.clone());
    }
    if let Ok(mut notes) = NOTES.lock()
        && notes.len() < NOTE_LIMIT
    {
        notes.push(text);
    }
}

/// Records a note once per distinct text.
pub fn note_once(text: impl Into<String>) {
    let text = text.into();
    let first = NOTED
        .lock()
        .map(|mut seen| seen.get_or_insert_with(HashSet::new).insert(text.clone()))
        .unwrap_or(false);
    if first {
        note(text);
    }
}

/// The notes recorded in this process.
pub fn notes() -> Vec<String> {
    NOTES.lock().map(|n| n.clone()).unwrap_or_default()
}

pub fn address(caller: usize) -> String {
    match ADDRESS.get() {
        Some(f) => f(caller),
        None => format!("{caller:#x}"),
    }
}

// Slot names

const SLOT_TABLE: &str = include_str!("../../shims/src/d3d12_slots.txt");

/// How sure a slot's meaning is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Basis {
    /// Seen in the executable or in a shipped client.
    Measured,
    /// The PC header's order, assumed.
    Pc,
    /// Inferred from the measured slots around it.
    Inferred,
}

/// The name and basis of a method, if the slot table knows it.
pub fn slot_name(interface: &str, slot: usize) -> Option<(&'static str, Basis)> {
    SLOT_TABLE
        .lines()
        .filter(|l| !l.starts_with('#'))
        .find_map(|line| {
            let mut w = line.split_whitespace();
            let (i, s, m, b) = (w.next()?, w.next()?, w.next()?, w.next()?);
            if i != interface || s.parse::<usize>().ok()? != slot {
                return None;
            }
            let basis = match b {
                "measured" => Basis::Measured,
                "inferred" => Basis::Inferred,
                _ => Basis::Pc,
            };
            Some((m, basis))
        })
}

/// Called by the machine-code entry of an unimplemented slot: notes it once and fails with `E_NOTIMPL`.
///
/// # Safety
/// `this` is an object of this layer (or any pointer, if its header cannot be read: then the kind is
/// unknown).
pub unsafe extern "win64" fn unknown_slot(
    this: *mut c_void,
    slot: usize,
    caller: usize,
    a1: usize,
    a2: usize,
    a3: usize,
) -> u32 {
    // SAFETY: every object handed out by this layer starts with a `ComHeader`.
    let kind = unsafe { kind_of(this) };
    let interface = kind.map_or("unknown object", Kind::interface);
    let name = match slot_name(interface, slot) {
        Some((n, Basis::Measured)) => format!("{n}, measured"),
        Some((n, Basis::Inferred)) => format!("{n}, inferred"),
        Some((n, Basis::Pc)) => format!("{n}, assumed from the PC order"),
        None => "unknown".to_string(),
    };
    note_once(format!(
        "{interface} slot {slot} ({name}) called from {} with {a1:#x} {a2:#x} {a3:#x}; not implemented",
        address(caller)
    ));
    E_NOTIMPL
}

/// Builds a vtable of `len` slots: each `(slot, function)` in `known`, a trap in the rest. The table is
/// leaked: objects point at it for the life of the process.
pub fn build_vtable(len: usize, known: &[(usize, usize)]) -> *const usize {
    assert!(len <= thunks::SLOTS);
    let mut table: Vec<usize> = thunks::UNKNOWN[..len].iter().map(|f| *f as usize).collect();
    for &(slot, function) in known {
        assert!(slot < len, "slot {slot} is past the vtable");
        table[slot] = function;
    }
    Box::leak(table.into_boxed_slice()).as_ptr()
}

/// The shared `QueryInterface`: answers `riid` when `accepts` does, handing out `this` again.
///
/// # Safety
/// `this` is an object of this layer; `riid` and `out` are what COM passes.
pub unsafe fn query_interface_with(
    this: *mut c_void,
    riid: *const u8,
    out: *mut *mut c_void,
    accepts: impl Fn(&Guid) -> bool,
) -> u32 {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: `out` is writable, checked above.
    unsafe { *out = std::ptr::null_mut() };
    if riid.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: COM passes a 16-byte IID.
    let wanted = unsafe { Guid::read(riid) };
    if accepts(&wanted) {
        // SAFETY: `this` is a live object; the new reference is for the caller.
        unsafe {
            add_ref(this);
            *out = this;
        }
        S_OK
    } else {
        // SAFETY: as above.
        let interface = unsafe { kind_of(this) }.map_or("object", Kind::interface);
        note_once(format!("{interface} asked for interface {wanted}; refused"));
        E_NOINTERFACE
    }
}

/// The IIDs of a kind's own interface chain, always answered.
pub fn base_iids(kind: Kind) -> &'static [Guid] {
    match kind {
        Kind::Device => &[
            iid::UNKNOWN,
            iid::OBJECT,
            iid::DEVICE,
            iid::DEVICE1,
            iid::DEVICE2,
            iid::DEVICE3,
            iid::DEVICE4,
            iid::DEVICE5,
            iid::DEVICE6,
            iid::DEVICE7,
            iid::DEVICE8,
            iid::DEVICE9,
            iid::DEVICE10,
            iid::DEVICE11,
        ],
        Kind::CommandQueue => &[
            iid::UNKNOWN,
            iid::OBJECT,
            iid::DEVICE_CHILD,
            iid::PAGEABLE,
            iid::COMMAND_QUEUE,
        ],
        Kind::CommandAllocator => &[
            iid::UNKNOWN,
            iid::OBJECT,
            iid::DEVICE_CHILD,
            iid::PAGEABLE,
            iid::COMMAND_ALLOCATOR,
        ],
        Kind::CommandList => &[
            iid::UNKNOWN,
            iid::OBJECT,
            iid::DEVICE_CHILD,
            iid::COMMAND_LIST,
            iid::GRAPHICS_COMMAND_LIST,
            iid::GRAPHICS_COMMAND_LIST1,
            iid::GRAPHICS_COMMAND_LIST2,
            iid::GRAPHICS_COMMAND_LIST3,
            iid::GRAPHICS_COMMAND_LIST4,
            iid::GRAPHICS_COMMAND_LIST5,
            iid::GRAPHICS_COMMAND_LIST6,
            iid::GRAPHICS_COMMAND_LIST7,
            iid::GRAPHICS_COMMAND_LIST8,
        ],
        Kind::Fence => &[
            iid::UNKNOWN,
            iid::OBJECT,
            iid::DEVICE_CHILD,
            iid::PAGEABLE,
            iid::FENCE,
            iid::FENCE1,
        ],
        Kind::DescriptorHeap => &[
            iid::UNKNOWN,
            iid::OBJECT,
            iid::DEVICE_CHILD,
            iid::PAGEABLE,
            iid::DESCRIPTOR_HEAP,
        ],
        Kind::Resource => &[
            iid::UNKNOWN,
            iid::OBJECT,
            iid::DEVICE_CHILD,
            iid::PAGEABLE,
            iid::RESOURCE,
        ],
        Kind::Heap => &[
            iid::UNKNOWN,
            iid::OBJECT,
            iid::DEVICE_CHILD,
            iid::PAGEABLE,
            iid::HEAP,
        ],
        Kind::RootSignature => &[
            iid::UNKNOWN,
            iid::OBJECT,
            iid::DEVICE_CHILD,
            iid::ROOT_SIGNATURE,
        ],
        Kind::PipelineState => &[
            iid::UNKNOWN,
            iid::OBJECT,
            iid::DEVICE_CHILD,
            iid::PAGEABLE,
            iid::PIPELINE_STATE,
        ],
        Kind::QueryHeap => &[
            iid::UNKNOWN,
            iid::OBJECT,
            iid::DEVICE_CHILD,
            iid::PAGEABLE,
            iid::QUERY_HEAP,
        ],
        Kind::CommandSignature => &[
            iid::UNKNOWN,
            iid::OBJECT,
            iid::DEVICE_CHILD,
            iid::PAGEABLE,
            iid::COMMAND_SIGNATURE,
        ],
        _ => &[iid::UNKNOWN],
    }
}

/// A `QueryInterface` for objects whose answers are `base_iids(kind)` and nothing else.
///
/// # Safety
/// As `query_interface_with`.
pub unsafe fn query_interface_for(
    kind: Kind,
    this: *mut c_void,
    riid: *const u8,
    out: *mut *mut c_void,
) -> u32 {
    // SAFETY: as the caller promises.
    unsafe { query_interface_with(this, riid, out, |g| base_iids(kind).contains(g)) }
}

/// Whether an IID is one of the PC SDK's D3D12 interfaces this layer knows by name (so a refusal is a
/// refusal of a known interface, not an unknown console one).
pub fn is_known_iid(g: &Guid) -> bool {
    [
        iid::UNKNOWN,
        iid::OBJECT,
        iid::DEVICE_CHILD,
        iid::PAGEABLE,
        iid::COMMAND_QUEUE,
        iid::COMMAND_ALLOCATOR,
        iid::COMMAND_LIST,
        iid::FENCE,
        iid::FENCE1,
        iid::DESCRIPTOR_HEAP,
        iid::HEAP,
        iid::RESOURCE,
        iid::ROOT_SIGNATURE,
        iid::PIPELINE_STATE,
        iid::QUERY_HEAP,
        iid::COMMAND_SIGNATURE,
        iid::DEVICE12,
        iid::DEVICE13,
        iid::DEVICE14,
        iid::GRAPHICS_COMMAND_LIST9,
        iid::GRAPHICS_COMMAND_LIST10,
    ]
    .contains(g)
        || base_iids(Kind::Device).contains(g)
        || base_iids(Kind::CommandList).contains(g)
}

/// Writes a counted object pointer into an out parameter, answering for `riid`: the object's own
/// `QueryInterface` decides, then the caller's reference is the one it made.
///
/// # Safety
/// `object` is a live COM object of this layer with a reference to give; `out` is writable or null.
pub unsafe fn hand_out(object: *mut c_void, riid: *const u8, out: *mut *mut c_void) -> u32 {
    if out.is_null() {
        // SAFETY: the reference is the caller's to give up.
        unsafe { release(object) };
        return E_POINTER;
    }
    // SAFETY: slot 0 of any COM object is `QueryInterface`.
    let result = unsafe {
        let q: unsafe extern "win64" fn(*mut c_void, *const u8, *mut *mut c_void) -> u32 =
            std::mem::transmute(*(*(object as *const *const usize)).add(0));
        q(object, riid, out)
    };
    // SAFETY: the creation reference is no longer needed.
    unsafe { release(object) };
    result
}

/// A trap object for an interface this layer does not know: every slot notes and fails.
pub fn trap_object(label: &'static str) -> *mut c_void {
    static TABLE: OnceLock<usize> = OnceLock::new();
    let table = *TABLE.get_or_init(|| {
        build_vtable(
            thunks::SLOTS,
            &[
                (0, trap_qi as *const () as usize),
                (1, add_ref as *const () as usize),
                (2, release as *const () as usize),
            ],
        ) as usize
    });
    new_object(Kind::Trap, table as *const usize, label) as *mut c_void
}

unsafe extern "win64" fn trap_qi(this: *mut c_void, riid: *const u8, out: *mut *mut c_void) -> u32 {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: a trap object, made by `trap_object`.
    let label: &'static str = unsafe { state::<&'static str>(this) };
    let wanted = if riid.is_null() {
        "null".to_string()
    } else {
        // SAFETY: COM passes a 16-byte IID.
        unsafe { Guid::read(riid) }.to_string()
    };
    note(format!("{label} asked for interface {wanted}"));
    let leaked: &'static str = Box::leak(format!("{label} as {wanted}").into_boxed_str());
    // SAFETY: `out` is writable, checked above.
    unsafe { *out = trap_object(leaked) };
    S_OK
}

/// Calls `QueryInterface` on any COM object.
///
/// # Safety
/// `object` is a live COM object; `riid` and `out` as COM passes them.
pub unsafe fn query(object: *mut c_void, riid: *const u8, out: *mut *mut c_void) -> u32 {
    // SAFETY: slot 0 of a COM object is `QueryInterface`.
    unsafe {
        let q: unsafe extern "win64" fn(*mut c_void, *const u8, *mut *mut c_void) -> u32 =
            std::mem::transmute(*(*(object as *const *const usize)).add(0));
        q(object, riid, out)
    }
}

unsafe extern "win64" fn get_private_data(
    _this: *mut c_void,
    _guid: *const u8,
    size: *mut u32,
    _data: *mut c_void,
) -> u32 {
    if !size.is_null() {
        // SAFETY: `size` is writable when not null.
        unsafe { *size = 0 };
    }
    DXGI_ERROR_NOT_FOUND
}

unsafe extern "win64" fn set_private_data(
    _this: *mut c_void,
    _guid: *const u8,
    _size: u32,
    _data: *const c_void,
) -> u32 {
    S_OK
}

unsafe extern "win64" fn set_name(_this: *mut c_void, _name: *const u16) -> u32 {
    S_OK
}

/// A vtable of `len` slots with the `ID3D12Object` methods (3 to 6; private data is accepted and not
/// stored) and `known`; the rest traps.
pub fn object_vtable(len: usize, known: &[(usize, usize)]) -> *const usize {
    let mut all = vec![
        (3, get_private_data as *const () as usize),
        (4, set_private_data as *const () as usize),
        (5, set_private_data as *const () as usize),
        (6, set_name as *const () as usize),
    ];
    all.extend_from_slice(known);
    build_vtable(len, &all)
}
