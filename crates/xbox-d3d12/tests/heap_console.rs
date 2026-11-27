//! The title's first descriptor heap, driven through the device and heap vtables with `ConsolePlacement`
//! behind the device: the call shape of the routine at RVA 0x1cf3280.
#![cfg(windows)]
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use xbox_d3d12::com::{Guid, iid};
use xbox_d3d12::device::{Integration, create_device};
use xbox_d3d12::heap::console::{ALLOCATOR_ID, OBJECT_TYPE, OBJECT_TYPE_MASK};
use xbox_d3d12::heap::{
    ConsolePlacement, HeapAllocator, Hooked, LowBlock, LowMemory, PlacementNote,
};

type Ptr = *mut c_void;
type CreateHeapFn = unsafe extern "win64" fn(Ptr, *const u32, *const u8, *mut Ptr) -> u32;
type IncrementFn = unsafe extern "win64" fn(Ptr, u32) -> u32;
type StartFn = unsafe extern "win64" fn(Ptr) -> u64;

const NO_GPU: u64 = 0xBADA_DD00_0000_0000;

unsafe fn slot<F: Copy>(object: Ptr, index: usize) -> F {
    // SAFETY: the object's first word is its vtable; the caller names the right signature.
    unsafe {
        let address = *(*(object as *const *const usize)).add(index);
        std::mem::transmute_copy::<usize, F>(&address)
    }
}

fn guid_bytes(g: &Guid) -> [u8; 16] {
    // SAFETY: `Guid` is 16 bytes of plain data.
    unsafe { std::mem::transmute::<Guid, [u8; 16]>(*g) }
}

/// The title's override list for allocator 0x80, as its hook reads it.
#[derive(Default)]
struct TitleHooks {
    overrides: Mutex<Vec<(u64, u64)>>,
}

impl HeapAllocator for TitleHooks {
    fn allocate(&self, bytes: u64, attributes: u64) -> Option<Hooked> {
        if (attributes >> 16) & 0xff != ALLOCATOR_ID || attributes & OBJECT_TYPE_MASK != OBJECT_TYPE
        {
            return None;
        }
        let list = self.overrides.lock().unwrap();
        list.iter()
            .rev()
            .find(|(_, size)| *size == bytes)
            .map(|(address, _)| Hooked {
                address: *address,
                release: None,
            })
    }
}

struct Low(AtomicU64);

impl LowMemory for Low {
    fn alloc_low(&self, bytes: u64) -> Option<LowBlock> {
        let address = self
            .0
            .fetch_add(bytes.next_multiple_of(65536), Ordering::Relaxed);
        Some(LowBlock {
            address,
            keep: Box::new(()),
        })
    }
}

struct Title {
    device: Ptr,
    hooks: Arc<TitleHooks>,
    notes: Arc<Mutex<Vec<PlacementNote>>>,
}

impl Title {
    fn new() -> Title {
        let hooks = Arc::new(TitleHooks::default());
        let notes = Arc::new(Mutex::new(Vec::new()));
        let sink = notes.clone();
        let placement =
            ConsolePlacement::new(hooks.clone(), Arc::new(Low(AtomicU64::new(0x4000_0000))))
                .with_notes(move |n| sink.lock().unwrap().push(n.clone()));
        let integration = Integration {
            placement: Arc::new(placement),
            ..Default::default()
        };
        let params = [0x1234u32; 24];
        let unknown_iid = [7u8; 16];
        let mut device = std::ptr::null_mut();
        // SAFETY: valid pointers for each argument.
        let result = unsafe {
            create_device(
                params.as_ptr(),
                unknown_iid.as_ptr(),
                &mut device,
                integration,
            )
        };
        assert_eq!(result, 0);
        Title {
            device,
            hooks,
            notes,
        }
    }

    /// The routine at 0x1cf3280: returns the heap's slot 9 result and, as the title keeps it, slot 10 minus
    /// slot 9 (or the constant for a heap that is not shader visible). `None` is the title's breakpoint.
    fn create_heap(
        &self,
        heap_type: u32,
        address: u64,
        count: u32,
        flags: u32,
    ) -> Option<(u64, u64)> {
        // SAFETY: device slots 14 and 15 and heap slots 9 and 10, with their console call shapes.
        unsafe {
            let increment: IncrementFn = slot(self.device, 15);
            let bytes = u64::from(count) * u64::from(increment(self.device, heap_type));
            if address != 0 {
                self.hooks.overrides.lock().unwrap().push((address, bytes));
            }
            let desc = [heap_type, count, flags, 1];
            let heap_iid = guid_bytes(&iid::DESCRIPTOR_HEAP);
            let mut heap = std::ptr::null_mut();
            let create: CreateHeapFn = slot(self.device, 14);
            let result = create(self.device, desc.as_ptr(), heap_iid.as_ptr(), &mut heap);
            if address != 0 {
                self.hooks.overrides.lock().unwrap().pop();
            }
            assert_eq!(result, 0);
            let cpu_start: StartFn = slot(heap, 9);
            let start = cpu_start(heap);
            if address != 0 && start != address {
                return None;
            }
            let gpu = if flags & 1 != 0 {
                let gpu_start: StartFn = slot(heap, 10);
                gpu_start(heap).wrapping_sub(start)
            } else {
                NO_GPU
            };
            Some((start, gpu))
        }
    }
}

#[test]
fn the_first_heap_starts_where_the_title_asked_and_the_assertion_holds() {
    let title = Title::new();
    let address = 0x2000_0000;
    let count = (3 * 65536 + 5 * 65536) / 32;

    let (start, gpu_minus_cpu) = title.create_heap(0, address, count, 1).expect("assertion");

    assert_eq!(start, address);
    assert_eq!(gpu_minus_cpu, 0);
}

#[test]
fn the_other_four_heaps_need_no_address_and_the_gpu_side_follows_the_flag() {
    let title = Title::new();

    let sampler_visible = title.create_heap(1, 0, 0x80, 1).unwrap();
    let sampler_hidden = title.create_heap(1, 0, 0x80, 0).unwrap();
    let rtv = title.create_heap(2, 0, 0x1000, 0).unwrap();
    let dsv = title.create_heap(3, 0, 0x800, 0).unwrap();

    assert_eq!(sampler_visible.1, 0);
    assert!(sampler_visible.0 + 0x80 * 32 <= 1 << 32);
    for hidden in [sampler_hidden, rtv, dsv] {
        assert_eq!(hidden.1, NO_GPU);
    }
    assert!(sampler_hidden.0 >= sampler_visible.0 + 0x80 * 32);
}

#[test]
fn a_title_range_above_4_gib_is_reported_for_a_shader_visible_heap() {
    let title = Title::new();

    let (start, _) = title.create_heap(0, 0x1_2000_0000, 4096, 1).unwrap();

    assert_eq!(start, 0x1_2000_0000);
    assert!(
        title
            .notes
            .lock()
            .unwrap()
            .iter()
            .any(|n| matches!(n, PlacementNote::AboveLowLimit { .. }))
    );
}
