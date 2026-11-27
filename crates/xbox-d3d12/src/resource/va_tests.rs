use super::va::*;

#[test]
fn allocations_are_aligned_and_do_not_overlap() {
    let mut va = VaAllocator::new(0x1_0000_0000, 0x1_1000_0000);
    let a = va.alloc(0x1234, 0x10000).unwrap();
    let b = va.alloc(0x10000, 0x10000).unwrap();
    let c = va.alloc(0x100, 0x100000).unwrap();
    assert_eq!(a, 0x1_0000_0000);
    assert!(b >= a + 0x1234 && b.is_multiple_of(0x10000));
    assert!(c.is_multiple_of(0x100000) && c >= b + 0x10000);
}

#[test]
fn freed_ranges_merge_back() {
    let mut va = VaAllocator::new(0x10000, 0x50000);
    let total = va.available();
    let a = va.alloc(0x10000, 0x10000).unwrap();
    let b = va.alloc(0x10000, 0x10000).unwrap();
    let c = va.alloc(0x10000, 0x10000).unwrap();
    va.free(b, 0x10000);
    va.free(a, 0x10000);
    va.free(c, 0x10000);
    assert_eq!(va.available(), total);
    assert_eq!(va.alloc(0x40000, 0x10000), Some(0x10000));
    assert_eq!(va.alloc(1, 1), None);
}

#[test]
fn a_range_the_game_chose_is_reserved_out_of_the_middle() {
    let mut va = VaAllocator::new(0x10000, 0x50000);
    assert!(va.reserve(0x20000, 0x10000));
    assert!(!va.reserve(0x28000, 0x10000));
    assert!(!va.reserve(0x60000, 0x10000));
    assert_eq!(va.alloc(0x10000, 0x10000), Some(0x10000));
    assert_eq!(va.alloc(0x10000, 0x10000), Some(0x30000));
    va.free(0x20000, 0x10000);
    assert_eq!(va.alloc(0x10000, 0x10000), Some(0x20000));
}

#[test]
fn memory_is_committed_at_the_address_asked_for() {
    let va = 0x1_8000_0000u64;
    if !commit_host(va, 0x20000, true) {
        return;
    }
    // SAFETY: the allocation was just made at this address.
    unsafe { *(va as *mut u32) = 5 };
    assert!(!commit_host(va, 0x10000, false));
    release_host(va);
}
