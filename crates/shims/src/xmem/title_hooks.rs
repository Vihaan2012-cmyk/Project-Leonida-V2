//! The title's allocation hooks.
//!
//! Measured statically in the 1.0.2.1 executable (RVAs below); nothing was run.
//!
//! - The executable exports one symbol, `__XMemAllocationHooks__` (table at 0x6844380, name string at
//!   0x6791a9b). It is not an import: the system's `xmem.dll` finds the table by looking the name up in the
//!   executable's export directory, and so does this module (`set_main_module` gives the image).
//! - The table is two pointers: the allocation hook (0x4deb420) and the free hook (0x4e38ec0). Both are
//!   `win64` functions. The allocation hook is `(size, attributes) -> address`, the free hook
//!   `(address, attributes)`.
//! - Both pick a bucket by attribute bits 16 to 23, walk that bucket's chain of nodes
//!   `{function, argument, thread tag, next}` and call the newest node that is for every thread (tag -1) or the
//!   calling thread, as `function(size or address, attributes, argument, next)`. All 256 buckets start at one
//!   static node whose function (0x4deb540, 0x4e38fa0) is a jump to `XMemAllocDefault` or `XMemFreeDefault`.
//!   The title pushes scoped overrides onto a bucket; a descriptor heap's is in bucket 0x80.
//!
//! So every `XMemAlloc` and `XMemFree` runs the hook, whatever the attributes, and the hook ends in the
//! `Default` functions unless an override answers. The `Default` functions therefore must not call the hook
//! again. An allocation hook that returns null has already tried `XMemAllocDefault`, so null is the answer; ours
//! is only used where the executable has no table or no pointer in it (tests, and any other title).
//!
//! The title itself calls `XMemAlloc` from three places (0x1d6be29, 0x2474572, 0x5b52570) and `XMemFree` from
//! three (0x24745f8, 0x43ae9a1, and a tail jump), with attributes 0xf0814000 (bucket 0x81), 0x3 and a variable
//! one. The console's D3D12 runtime reaches bucket 0x80 through the same entry point
//! (`xbox_d3d12::heap::console`).

use std::cell::Cell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

use xbox_d3d12::heap::{HeapAllocator, Hooked};

use super::imp;
use crate::loader::main_module;
use crate::trace::note;

/// The name of the executable's export that holds the table.
pub const EXPORT_NAME: &str = "__XMemAllocationHooks__";

type AllocHook = unsafe extern "win64" fn(usize, u64) -> *mut c_void;
type FreeHook = unsafe extern "win64" fn(*mut c_void, u64);

/// The two pointers of the title's table; either is `None` while it holds null.
#[derive(Clone, Copy)]
pub(super) struct TitleHooks {
    pub(super) alloc: Option<AllocHook>,
    pub(super) free: Option<FreeHook>,
}

/// The hooks of the loaded title, read from its table on every call because the title owns the table.
pub(super) fn current() -> Option<TitleHooks> {
    let base = main_module();
    if base == 0 {
        return None;
    }
    // SAFETY: `set_main_module` was given the base of a loaded image whose headers and sections stay mapped.
    let table = unsafe { export_address(base, EXPORT_NAME)? };
    // SAFETY: the table is two pointers inside the image, found through its export directory.
    let (alloc, free) = unsafe {
        (
            (table as *const usize).read_volatile(),
            (table as *const usize).add(1).read_volatile(),
        )
    };
    static NOTED: AtomicBool = AtomicBool::new(false);
    if !NOTED.swap(true, Ordering::Relaxed) {
        note(format!(
            "the title's XMem allocation hooks: table {table:#x}, allocation {alloc:#x}, free {free:#x}"
        ));
    }
    // SAFETY: a nonzero entry is the address of a function of the title with the hook's signature.
    unsafe {
        Some(TitleHooks {
            alloc: (alloc != 0).then(|| std::mem::transmute::<usize, AllocHook>(alloc)),
            free: (free != 0).then(|| std::mem::transmute::<usize, FreeHook>(free)),
        })
    }
}

thread_local! {
    static LAST_DEFAULT: Cell<usize> = const { Cell::new(0) };
}

/// Remembers what `XMemAllocDefault` just returned on this thread, so the caller of a hook can tell memory the
/// hook chain made from memory the title owns.
pub(super) fn note_default_allocation(address: usize) {
    LAST_DEFAULT.with(|last| last.set(address));
}

/// The descriptor heap memory of the console runtime: `XMemAlloc` through the title's allocation hook.
///
/// Memory the chain allocated itself (the title pushed no override for the request) is freed again and
/// reported as nothing, so the caller places the heap in memory the GPU can read; only an address the title
/// chose is handed back, and the title owns it.
pub struct TitleHeapAllocator;

/// The allocator for `xbox_d3d12::heap::ConsolePlacement::new`.
pub fn title_heap_allocator() -> std::sync::Arc<TitleHeapAllocator> {
    std::sync::Arc::new(TitleHeapAllocator)
}

impl HeapAllocator for TitleHeapAllocator {
    fn allocate(&self, bytes: u64, attributes: u64) -> Option<Hooked> {
        let hook = current()?.alloc?;
        note_default_allocation(0);
        // SAFETY: a hook of the loaded title, called as the console's `XMemAlloc` calls it.
        let address = unsafe { hook(usize::try_from(bytes).ok()?, attributes) } as usize;
        if address == 0 {
            return None;
        }
        if LAST_DEFAULT.with(Cell::get) == address {
            imp::free_default(address as *mut c_void, attributes);
            return None;
        }
        Some(Hooked {
            address: address as u64,
            release: None,
        })
    }
}

/// The address of a named export of the PE image mapped at `base`, if it has one that is not a forwarder.
///
/// # Safety
/// `base` is the start of a mapped PE image (headers and sections at their RVAs) that stays mapped.
pub(super) unsafe fn export_address(base: usize, wanted: &str) -> Option<usize> {
    let image = Image { base, size: 0 };
    // SAFETY: the DOS and PE headers lie in the first page of any image.
    let (lfanew, magic) = unsafe {
        if image.read::<u16>(0)? != 0x5A4D {
            return None;
        }
        let lfanew = image.read::<u32>(0x3C)?;
        if !(0x40..=0x800).contains(&lfanew) || image.read::<u32>(lfanew)? != 0x0000_4550 {
            return None;
        }
        (lfanew, image.read::<u16>(lfanew + 24)?)
    };
    if magic != 0x20B {
        return None;
    }
    let optional = lfanew + 24;
    // SAFETY: inside the optional header of a PE32+ image.
    let (size, directory, directory_size) = unsafe {
        (
            image.read::<u32>(optional + 56)?,
            image.read::<u32>(optional + 112)?,
            image.read::<u32>(optional + 116)?,
        )
    };
    let image = Image {
        base,
        size: size as usize,
    };
    if directory == 0 || directory_size < 40 {
        return None;
    }
    // SAFETY: every read is bounds-checked against the image's size.
    unsafe {
        let function_count = image.read::<u32>(directory + 20)?;
        let name_count = image.read::<u32>(directory + 24)?;
        let functions = image.read::<u32>(directory + 28)?;
        let names = image.read::<u32>(directory + 32)?;
        let ordinals = image.read::<u32>(directory + 36)?;
        for index in 0..name_count.min(1 << 16) {
            let name = image.read::<u32>(names.checked_add(4 * index)?)?;
            if !image.text_is(name, wanted) {
                continue;
            }
            let ordinal = u32::from(image.read::<u16>(ordinals.checked_add(2 * index)?)?);
            if ordinal >= function_count {
                return None;
            }
            let target = image.read::<u32>(functions.checked_add(4 * ordinal)?)?;
            let forwarder = target >= directory && target - directory < directory_size;
            return (target != 0 && !forwarder).then(|| base + target as usize);
        }
    }
    None
}

/// A mapped image, with reads checked against its size (0 while the size is not known: the first page only).
struct Image {
    base: usize,
    size: usize,
}

impl Image {
    fn within(&self, rva: u32, len: usize) -> bool {
        let limit = if self.size == 0 { 0x1000 } else { self.size };
        (rva as usize)
            .checked_add(len)
            .is_some_and(|end| end <= limit)
    }

    /// # Safety
    /// The image is mapped at `base` for at least the checked range.
    unsafe fn read<T: Copy>(&self, rva: u32) -> Option<T> {
        self.within(rva, size_of::<T>()).then(|| {
            // SAFETY: inside the image, as checked.
            unsafe { ((self.base + rva as usize) as *const T).read_unaligned() }
        })
    }

    /// Whether the NUL-terminated text at `rva` is exactly `wanted`.
    ///
    /// # Safety
    /// As `read`.
    unsafe fn text_is(&self, rva: u32, wanted: &str) -> bool {
        let wanted = wanted.as_bytes();
        if !self.within(rva, wanted.len() + 1) {
            return false;
        }
        // SAFETY: inside the image, as checked.
        let text = unsafe {
            std::slice::from_raw_parts((self.base + rva as usize) as *const u8, wanted.len() + 1)
        };
        text[..wanted.len()] == *wanted && text[wanted.len()] == 0
    }
}
