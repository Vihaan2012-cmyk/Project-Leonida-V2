use super::*;
use crate::descriptor::SLOT_BYTES;

fn info(heap_type: u32, count: u32, flags: u32, cpu: u64, gpu: u64) -> HeapInfo {
    HeapInfo {
        heap_type,
        count,
        flags,
        node_mask: 1,
        cpu_start: cpu,
        gpu_start: gpu,
        increment: increment_size(heap_type),
        object: 0,
    }
}

#[test]
fn every_heap_type_steps_by_the_descriptor_slot_and_the_rest_by_zero() {
    for heap_type in [TYPE_CBV_SRV_UAV, TYPE_SAMPLER, TYPE_RTV, TYPE_DSV] {
        assert_eq!(u64::from(increment_size(heap_type)), SLOT_BYTES);
    }
    assert_eq!(increment_size(4), 0);
    assert_eq!(increment_size(u32::MAX), 0);
}

#[test]
fn the_increment_divides_the_64_kib_ranges_the_title_sizes_heaps_in() {
    assert_eq!((64u32 << 10) % increment_size(TYPE_CBV_SRV_UAV), 0);
}

#[test]
fn descriptor_addresses_are_the_start_plus_whole_increments() {
    let heap = info(
        TYPE_CBV_SRV_UAV,
        1000,
        FLAG_SHADER_VISIBLE,
        0x2000_0000,
        0x2000_0000,
    );

    assert_eq!(heap.cpu_at(0), 0x2000_0000);
    assert_eq!(heap.cpu_at(1), 0x2000_0020);
    assert_eq!(heap.cpu_at(999), 0x2000_0000 + 999 * 32);
    assert_eq!(heap.gpu_at(7), heap.cpu_at(7));
    assert_eq!(heap.bytes(), 32_000);
}

#[test]
fn a_heap_that_is_not_shader_visible_reports_it_and_keeps_gpu_zero_based() {
    let hidden = info(TYPE_RTV, 16, 0, 0x3000_0000, 0);
    let visible = info(
        TYPE_SAMPLER,
        16,
        FLAG_SHADER_VISIBLE,
        0x3100_0000,
        0x3100_0000,
    );

    assert!(!hidden.shader_visible());
    assert!(visible.shader_visible());
    assert_eq!(hidden.gpu_at(2), 64);
}

#[test]
fn the_registry_finds_the_visible_heap_that_holds_a_gpu_address() {
    let registry = HeapRegistry::default();
    let a = info(
        TYPE_CBV_SRV_UAV,
        100,
        FLAG_SHADER_VISIBLE,
        0x2000_0000,
        0x2000_0000,
    );
    let b = info(
        TYPE_SAMPLER,
        50,
        FLAG_SHADER_VISIBLE,
        0x2100_0000,
        0x2100_0000,
    );
    registry.insert(a);
    registry.insert(b);
    registry.insert(info(TYPE_RTV, 8, 0, 0x2200_0000, 0));

    assert_eq!(registry.find(0x2000_0000), Some(a));
    assert_eq!(registry.find(a.gpu_at(99) + 31), Some(a));
    assert_eq!(registry.find(a.gpu_at(100)), None);
    assert_eq!(registry.find(0x2100_0020), Some(b));
    assert_eq!(registry.find(0x1fff_ffff), None);
    assert_eq!(registry.find(0), None);

    registry.remove(a.gpu_start);
    assert_eq!(registry.find(0x2000_0000), None);
    assert_eq!(registry.find(0x2100_0000), Some(b));
}
