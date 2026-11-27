use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::*;
use crate::heap::{FLAG_SHADER_VISIBLE, TYPE_CBV_SRV_UAV, increment_size};

const SHADER_VISIBLE: u32 = FLAG_SHADER_VISIBLE;
/// What the title stores as the GPU side of a heap that is not shader visible.
const NO_GPU: u64 = 0xBADA_DD00_0000_0000;
const KIB64: u64 = 64 << 10;

/// The title's override list for allocator id 0x80 on its own thread: `(address, bytes)`, newest last.
#[derive(Default)]
struct TitleHooks {
    overrides: Mutex<Vec<(u64, u64)>>,
}

impl TitleHooks {
    /// The title's allocation hook: bucket by attribute bits 16 to 23, then each node's callback, newest
    /// first: it answers when `attributes & 0x3fff == 4` and the size is the one it was given, else it
    /// forwards to the next node, and the last node allocates the default way.
    fn hook(&self, bytes: u64, attributes: u64) -> Option<u64> {
        if (attributes >> 16) & 0xff != ALLOCATOR_ID {
            return None;
        }
        let list = self.overrides.lock().unwrap();
        list.iter()
            .rev()
            .find(|(_, size)| attributes & OBJECT_TYPE_MASK == OBJECT_TYPE && *size == bytes)
            .map(|(address, _)| *address)
    }
}

impl HeapAllocator for TitleHooks {
    fn allocate(&self, bytes: u64, attributes: u64) -> Option<Hooked> {
        self.hook(bytes, attributes).map(|address| Hooked {
            address,
            release: None,
        })
    }
}

/// Low memory that hands out addresses and touches nothing.
struct CountingLow {
    next: AtomicU64,
    limit: u64,
}

impl CountingLow {
    fn new() -> Arc<CountingLow> {
        Arc::new(CountingLow {
            next: AtomicU64::new(0x4000_0000),
            limit: LOW_LIMIT,
        })
    }
}

impl LowMemory for CountingLow {
    fn alloc_low(&self, bytes: u64) -> Option<LowBlock> {
        let len = bytes.next_multiple_of(KIB64);
        let address = self.next.fetch_add(len, Ordering::Relaxed);
        (address + len <= self.limit).then(|| LowBlock {
            address,
            keep: Box::new(()),
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Assertion {
    start: u64,
    expected: u64,
}

struct TitleHeap {
    start: u64,
    end: u64,
    gpu_minus_cpu: u64,
    _keep: Placed,
}

/// The title's heap routine (0x1cf3280) with the runtime behind `placement`: it sets the override up when
/// it has an address, lets the runtime place the heap, reads the start handle, and asserts that it is the
/// address it asked for.
fn title_creates_heap(
    hooks: &TitleHooks,
    placement: &ConsolePlacement,
    address: u64,
    count: u64,
    flags: u32,
) -> Result<TitleHeap, Assertion> {
    let increment = u64::from(increment_size(TYPE_CBV_SRV_UAV));
    let bytes = count * increment;
    if address != 0 {
        hooks.overrides.lock().unwrap().push((address, bytes));
    }
    let placed = placement.place(bytes as usize, flags & SHADER_VISIBLE != 0);
    if address != 0 {
        hooks.overrides.lock().unwrap().pop();
    }
    let placed = placed.expect("the runtime placed the heap");
    let start = placed.cpu;
    if address != 0 && start != address {
        return Err(Assertion {
            start,
            expected: address,
        });
    }
    let gpu_minus_cpu = if flags & SHADER_VISIBLE != 0 {
        placed.gpu.wrapping_sub(start)
    } else {
        NO_GPU
    };
    Ok(TitleHeap {
        start,
        end: start + bytes,
        gpu_minus_cpu,
        _keep: placed,
    })
}

/// The size heap 1 gets: three 64 KiB-rounded ranges, divided by the increment.
fn first_heap_count() -> u64 {
    let increment = u64::from(increment_size(TYPE_CBV_SRV_UAV));
    let a1 = (3128 * increment).next_multiple_of(KIB64);
    let a2 = (4000 * increment).next_multiple_of(KIB64);
    let a3 = 0x40000u64.next_multiple_of(KIB64) * 0x20;
    (a1 + a2 + a3) / increment
}

fn recording() -> (
    Arc<Mutex<Vec<PlacementNote>>>,
    impl Fn(&PlacementNote) + Send + Sync + 'static,
) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    (seen, move |n: &PlacementNote| {
        sink.lock().unwrap().push(n.clone())
    })
}

#[test]
fn the_titles_start_handle_assertion_holds_at_the_address_it_chose() {
    let hooks = Arc::new(TitleHooks::default());
    let placement = ConsolePlacement::new(hooks.clone(), CountingLow::new());
    let address = 0x2000_0000;
    let count = first_heap_count();

    let heap = title_creates_heap(&hooks, &placement, address, count, SHADER_VISIBLE).unwrap();

    assert_eq!(heap.start, address);
    assert_eq!(heap.end, address + count * 32);
    assert_eq!(heap.gpu_minus_cpu, 0);
}

#[test]
fn a_layer_chosen_start_trips_the_assertion_as_upstreams_boot_did() {
    let hooks = TitleHooks::default();
    let placement = ConsolePlacement::new(Arc::new(NoAllocator), CountingLow::new());
    let address = 0x2000_0000;

    let result = title_creates_heap(
        &hooks,
        &placement,
        address,
        first_heap_count(),
        SHADER_VISIBLE,
    );

    assert_eq!(
        result.err().unwrap(),
        Assertion {
            start: 0x4000_0000,
            expected: address
        }
    );
}

#[test]
fn the_hook_only_answers_the_size_the_title_registered() {
    let hooks = Arc::new(TitleHooks::default());
    let placement = ConsolePlacement::new(hooks.clone(), CountingLow::new());
    hooks.overrides.lock().unwrap().push((0x2000_0000, 0x10000));

    let same = placement.place(0x10000, true).unwrap();
    let other = placement.place(0x10020, true).unwrap();

    assert_eq!(same.cpu, 0x2000_0000);
    assert_ne!(other.cpu, 0x2000_0000);
}

#[test]
fn the_descriptor_heap_attributes_reach_the_titles_bucket_and_object_type() {
    let hooks = TitleHooks::default();
    hooks.overrides.lock().unwrap().push((0x2000_0000, 0x10000));

    assert_eq!(
        hooks.hook(0x10000, DESCRIPTOR_HEAP_ATTRIBUTES),
        Some(0x2000_0000)
    );
    assert_eq!(hooks.hook(0x10000, DESCRIPTOR_HEAP_ATTRIBUTES | 0x40), None);
    assert_eq!(hooks.hook(0x10000, 0x7f_0004), None);
    assert_eq!(
        hooks.hook(0x10000, DESCRIPTOR_HEAP_ATTRIBUTES | 0x1000),
        None
    );
}

#[test]
fn a_heap_without_an_address_is_placed_low_with_equal_handles() {
    let hooks = Arc::new(TitleHooks::default());
    let placement = ConsolePlacement::new(hooks.clone(), CountingLow::new());

    let heap = title_creates_heap(&hooks, &placement, 0, 0x80, SHADER_VISIBLE).unwrap();

    assert!(heap.end <= LOW_LIMIT);
    assert_eq!(heap.gpu_minus_cpu, 0);
}

#[test]
fn a_heap_that_is_not_shader_visible_has_no_gpu_handle_and_the_titles_sentinel() {
    let hooks = Arc::new(TitleHooks::default());
    let placement = ConsolePlacement::new(hooks.clone(), CountingLow::new());
    let address = 0x2800_0000;

    let heap = title_creates_heap(&hooks, &placement, address, 0x1000, 0).unwrap();
    let placed = placement.place(0x1000 * 32, false).unwrap();

    assert_eq!(heap.start, address);
    assert_eq!(heap.gpu_minus_cpu, NO_GPU);
    assert_eq!(placed.gpu, 0);
}

#[test]
fn two_heaps_in_low_memory_do_not_overlap() {
    let placement = ConsolePlacement::new(Arc::new(NoAllocator), CountingLow::new());

    let a = placement.place(0x2000, true).unwrap();
    let b = placement.place(0x2000, true).unwrap();

    assert!(a.cpu + 0x2000 <= b.cpu);
    assert_eq!(a.gpu, a.cpu);
    assert_eq!(b.gpu, b.cpu);
}

#[test]
fn a_title_range_above_4_gib_is_kept_and_reported() {
    let hooks = Arc::new(TitleHooks::default());
    let (seen, sink) = recording();
    let placement = ConsolePlacement::new(hooks.clone(), CountingLow::new()).with_notes(sink);
    let address = 0x1_2000_0000;

    let heap = title_creates_heap(&hooks, &placement, address, 0x800, SHADER_VISIBLE).unwrap();

    assert_eq!(heap.start, address);
    assert!(
        seen.lock()
            .unwrap()
            .contains(&PlacementNote::AboveLowLimit {
                address,
                bytes: 0x800 * 32
            })
    );
}

#[test]
fn a_range_above_4_gib_that_only_the_layer_holds_is_replaced_by_low_memory() {
    struct High {
        released: Arc<AtomicBool>,
    }
    impl HeapAllocator for High {
        fn allocate(&self, _bytes: u64, _attributes: u64) -> Option<Hooked> {
            let flag = self.released.clone();
            Some(Hooked {
                address: 0x1_0000_0000,
                release: Some(Box::new(move || flag.store(true, Ordering::SeqCst))),
            })
        }
    }
    let released = Arc::new(AtomicBool::new(false));
    let (seen, sink) = recording();
    let placement = ConsolePlacement::new(
        Arc::new(High {
            released: released.clone(),
        }),
        CountingLow::new(),
    )
    .with_notes(sink);

    let visible = placement.place(0x4000, true).unwrap();
    let hidden = placement.place(0x4000, false).unwrap();

    assert!(visible.cpu + 0x4000 <= LOW_LIMIT);
    assert_eq!(visible.gpu, visible.cpu);
    assert!(released.load(Ordering::SeqCst));
    assert!(
        seen.lock()
            .unwrap()
            .contains(&PlacementNote::MovedBelowLimit {
                from: 0x1_0000_0000,
                to: visible.cpu
            })
    );
    assert_eq!(hidden.cpu, 0x1_0000_0000);
    assert_eq!(hidden.gpu, 0);
}

#[test]
fn a_heap_releases_hooked_memory_when_it_dies() {
    struct Owned(Arc<AtomicBool>);
    impl HeapAllocator for Owned {
        fn allocate(&self, _bytes: u64, _attributes: u64) -> Option<Hooked> {
            let flag = self.0.clone();
            Some(Hooked {
                address: 0x2000_0000,
                release: Some(Box::new(move || flag.store(true, Ordering::SeqCst))),
            })
        }
    }
    let released = Arc::new(AtomicBool::new(false));
    let placement = ConsolePlacement::new(Arc::new(Owned(released.clone())), CountingLow::new());

    let placed = placement.place(0x1000, true).unwrap();
    assert!(!released.load(Ordering::SeqCst));
    drop(placed);

    assert!(released.load(Ordering::SeqCst));
}

#[test]
fn the_unmeasured_attributes_are_reported_once() {
    let hooks = Arc::new(TitleHooks::default());
    let (seen, sink) = recording();
    let placement = ConsolePlacement::new(hooks.clone(), CountingLow::new()).with_notes(sink);
    let mut kept = Vec::new();
    for _ in 0..3 {
        hooks.overrides.lock().unwrap().push((0x2000_0000, 0x1000));
        kept.push(placement.place(0x1000, false).unwrap());
    }

    let notes = seen.lock().unwrap();
    let unmeasured = notes
        .iter()
        .filter(|n| matches!(n, PlacementNote::UnmeasuredAttributes { .. }))
        .count();
    assert_eq!(unmeasured, 1);
}

#[test]
fn running_out_of_low_memory_refuses_the_heap_and_says_so() {
    let (seen, sink) = recording();
    let low = Arc::new(CountingLow {
        next: AtomicU64::new(LOW_LIMIT - KIB64),
        limit: LOW_LIMIT,
    });
    let placement = ConsolePlacement::new(Arc::new(NoAllocator), low).with_notes(sink);

    assert!(placement.place(2 * KIB64 as usize, true).is_none());
    assert_eq!(
        *seen.lock().unwrap(),
        [PlacementNote::OutOfLowMemory { bytes: 2 * KIB64 }]
    );
}
