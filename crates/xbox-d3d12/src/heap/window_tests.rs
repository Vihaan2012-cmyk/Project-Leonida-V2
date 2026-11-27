use super::*;
use crate::memory::LOW_LIMIT;

#[cfg(windows)]
#[test]
fn heaps_come_out_in_creation_order_under_4_gib_with_equal_handles() {
    let window = FixedWindow::default();

    let a = window.place(1000 * 32, true).unwrap();
    let b = window.place(1000 * 32, true).unwrap();

    assert!(a.cpu < b.cpu);
    assert!(b.cpu + 1000 * 32 <= LOW_LIMIT);
    assert_eq!(a.cpu % (64 << 10), 0);
    assert_eq!((a.gpu, b.gpu), (a.cpu, b.cpu));
}

#[test]
fn a_heap_that_is_not_shader_visible_has_gpu_handle_zero() {
    let placed = FixedWindow::default().place(16 * 32, false).unwrap();

    assert_eq!(placed.gpu, 0);
    assert_ne!(placed.cpu, 0);
}

#[cfg(windows)]
#[test]
fn the_heap_memory_is_writable_to_its_last_byte() {
    let bytes = 3000 * 32;
    let placed = FixedWindow::default().place(bytes, true).unwrap();

    // SAFETY: `place` committed `bytes` readable and writable bytes at the CPU handle, owned by `placed`.
    let memory = unsafe { std::slice::from_raw_parts_mut(placed.cpu as *mut u8, bytes) };
    memory.fill(0xA5);
    memory[bytes - 1] = 0x5A;

    assert_eq!(memory[0], 0xA5);
    assert_eq!(memory[bytes - 1], 0x5A);
}

#[cfg(windows)]
#[test]
fn low_memory_blocks_do_not_overlap_the_heaps() {
    let window = FixedWindow::default();

    let heap = window.place(0x4000, true).unwrap();
    let block = window.alloc_low(0x4000).unwrap();

    assert!(heap.cpu + 0x4000 <= block.address || block.address + 0x4000 <= heap.cpu);
    assert!(block.address + 0x4000 <= LOW_LIMIT);
}

#[cfg(windows)]
#[test]
fn a_console_placement_over_the_window_places_unhooked_heaps_low() {
    let window = Arc::new(FixedWindow::default());
    let placement = ConsolePlacement::new(Arc::new(console::NoAllocator), window);

    let placed = placement.place(0x80 * 32, true).unwrap();

    assert!(placed.cpu + 0x80 * 32 <= LOW_LIMIT);
    assert_eq!(placed.gpu, placed.cpu);
}
