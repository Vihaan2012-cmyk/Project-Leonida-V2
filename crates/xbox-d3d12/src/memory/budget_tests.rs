use std::sync::Mutex;

use super::testing::*;
use super::*;
use crate::descriptor::format::dxgi;
use crate::resource::desc::{ALLOW_RENDER_TARGET, ResourceDesc};
use crate::resource::image::GpuImage;

const MIB: u64 = 1 << 20;

fn space(gpu: &TestGpu) -> AddressSpace {
    AddressSpace::new(
        gpu.handles.clone(),
        Config {
            base: 0,
            size: Some(1 << 38),
            chunk: 1 << 30,
        },
    )
    .unwrap()
}

fn capture(budget: &Budget) -> Arc<Mutex<Vec<String>>> {
    let notes = Arc::new(Mutex::new(Vec::new()));
    let sink = notes.clone();
    budget.set_note(move |text| sink.lock().unwrap().push(text));
    notes
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| (i as u8).wrapping_mul(7) ^ seed).collect()
}

/// A 1024x1024 RGBA8 texture: 4 MiB.
fn texture() -> ResourceDesc {
    ResourceDesc::texture_2d(dxgi::R8G8B8A8_UNORM, 1024, 1024, 1)
}

#[test]
fn textures_past_a_small_budget_spill_to_system_memory_and_keep_their_texels() {
    for gpu in &TestGpu::open_all() {
        let h = &gpu.handles;
        h.budget.set_limit(Some(10 * MIB));
        let notes = capture(&h.budget);
        let mut images = Vec::new();
        for n in 0..6u8 {
            let image = GpuImage::create(h, &texture()).unwrap();
            let data = pattern(1024 * 1024 * 4, n);
            image.upload(h, 0, 0, &data).unwrap();
            images.push((image, data));
        }
        let spilled: Vec<bool> = images.iter().map(|(i, _)| !i.device_local()).collect();
        let counters = h.budget.counters();
        eprintln!("{}: {spilled:?}\n{}", gpu.name(), counters.line());
        if h.budget.can_spill() {
            assert_eq!(
                spilled,
                [false, false, true, true, true, true],
                "{}",
                gpu.name()
            );
            assert!(counters.spilled[Class::Texture as usize] >= 16 * MIB);
            assert_eq!(counters.peak_spilled, counters.spilled_total());
            let said = notes.lock().unwrap().clone();
            let starts = said.iter().filter(|t| t.contains("spilling")).count();
            assert_eq!(starts, 1, "the start of spilling is noted once: {said:?}");
        } else {
            assert!(spilled.iter().all(|s| !s), "{}", gpu.name());
        }
        for (image, data) in &images {
            assert_eq!(
                &image.download(h, 0, 0, data.len()).unwrap(),
                data,
                "{}",
                gpu.name()
            );
        }
        for (image, _) in images {
            image.destroy(h);
        }
        let counters = h.budget.counters();
        assert_eq!(counters.local_total() + counters.spilled_total(), 0);
        h.budget.set_limit(None);
    }
}

#[test]
fn render_targets_and_depth_stay_device_local_when_the_budget_is_gone() {
    for gpu in &TestGpu::open_all() {
        let h = &gpu.handles;
        h.budget.set_limit(Some(4 * MIB));
        let filler: Vec<GpuImage> = (0..4)
            .map(|_| GpuImage::create(h, &texture()).unwrap())
            .collect();
        assert_eq!(h.budget.available(), 0);
        let mut target = texture();
        target.flags = ALLOW_RENDER_TARGET;
        let target = GpuImage::create(h, &target).unwrap();
        let depth =
            GpuImage::create(h, &ResourceDesc::texture_2d(dxgi::D32_FLOAT, 1024, 1024, 1)).unwrap();
        assert!(
            target.device_local() && depth.device_local(),
            "{}",
            gpu.name()
        );
        let data = pattern(1024 * 1024 * 4, 9);
        target.upload(h, 0, 0, &data).unwrap();
        assert_eq!(target.download(h, 0, 0, data.len()).unwrap(), data);
        let counters = h.budget.counters();
        assert!(counters.local[Class::Target as usize] >= 8 * MIB);
        if h.budget.can_spill() {
            assert!(filler.iter().any(|i| !i.device_local()));
        }
        for image in filler.into_iter().chain([target, depth]) {
            image.destroy(h);
        }
        h.budget.set_limit(None);
    }
}

#[test]
fn mirrored_game_memory_is_counted_and_works_past_the_budget() {
    for gpu in &TestGpu::open_all() {
        let mut space = space(gpu);
        if space.backing() != Backing::Mirrored {
            eprintln!(
                "{}: direct, nothing is allocated in device memory",
                gpu.name()
            );
            continue;
        }
        space.set_spill(false);
        let h = gpu.handles.clone();
        h.budget.set_limit(Some(2 * MIB));
        let kernel = Kernel::new(gpu);
        let regions: Vec<Region> = (0..4).map(|_| Region::new(16 * PAGE, false)).collect();
        for region in &regions {
            space
                .map_host(region.address, 16 * PAGE, SyncPolicy::Always)
                .unwrap();
        }
        let counters = h.budget.counters();
        assert_eq!(counters.local[Class::Mirror as usize], 4 * 16 * PAGE);
        assert!(counters.local_total() > h.budget.counters().budget);
        for region in &regions {
            region.write(0, 41);
            kernel.run(
                gpu,
                &mut space,
                &[(region.address, region.address + 2 * PAGE)],
                (region.address, 16 * PAGE),
            );
            assert_eq!(region.read(2 * PAGE / 4), 42);
        }
        for region in &regions {
            space.unmap(region.address, 16 * PAGE).unwrap();
        }
        space.reclaim();
        assert_eq!(h.budget.counters().local_total(), 0);
        kernel.finish(gpu);
        h.budget.set_limit(None);
    }
}

#[test]
fn a_run_the_gpu_refuses_is_left_unbound_then_bound_when_memory_is_freed() {
    for gpu in &TestGpu::open_all() {
        let mut space = space(gpu);
        if space.backing() != Backing::Mirrored {
            continue;
        }
        space.set_spill(false);
        let h = gpu.handles.clone();
        let notes = capture(&h.budget);
        h.budget.set_limit(Some(2 * MIB));
        h.budget.set_enforced(true);
        let kernel = Kernel::new(gpu);
        let regions: Vec<Region> = (0..4).map(|_| Region::new(16 * PAGE, false)).collect();
        for (n, region) in regions.iter().take(2).enumerate() {
            space
                .map_host(region.address, 16 * PAGE, SyncPolicy::Always)
                .unwrap_or_else(|e| panic!("run {n}: {e}"));
        }
        let strict = space.map_host(regions[2].address, 16 * PAGE, SyncPolicy::Always);
        assert!(
            matches!(
                strict,
                Err(MemoryError::Vulkan(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY))
            ),
            "{strict:?}"
        );
        assert_eq!(space.deferred_bytes(), 0);
        for region in &regions[2..] {
            space
                .lenient(|s| s.map_host(region.address, 16 * PAGE, SyncPolicy::Always))
                .unwrap();
        }
        assert_eq!(space.deferred_bytes(), 2 * 16 * PAGE);
        assert_eq!(h.budget.counters().deferred, 2 * 16 * PAGE);
        assert!(space.is_bound(regions[1].address));
        assert!(!space.is_bound(regions[2].address) && !space.is_bound(regions[3].address));
        let said = notes.lock().unwrap().clone();
        assert!(said.iter().any(|t| t.contains("left unbound")), "{said:?}");

        // The game frees one backed run and one that never was: the other deferred run gets its memory.
        regions[2].write(0, 41);
        space.unmap(regions[3].address, 16 * PAGE).unwrap();
        assert_eq!(space.deferred_bytes(), 16 * PAGE);
        space.unmap(regions[0].address, 16 * PAGE).unwrap();
        space.reclaim();
        assert_eq!(
            space.deferred_bytes(),
            0,
            "the freed memory backs the deferred run"
        );
        assert_eq!(h.budget.counters().deferred, 0);
        assert!(space.is_bound(regions[2].address));
        kernel.run(
            gpu,
            &mut space,
            &[(regions[2].address, regions[2].address + 2 * PAGE)],
            (regions[2].address, 16 * PAGE),
        );
        assert_eq!(regions[2].read(2 * PAGE / 4), 42);
        space.unmap(regions[1].address, 16 * PAGE).unwrap();
        space.unmap(regions[2].address, 16 * PAGE).unwrap();
        space.reclaim();
        assert_eq!(h.budget.counters().local_total(), 0);
        kernel.finish(gpu);
        h.budget.set_enforced(false);
        h.budget.set_limit(None);
    }
}

#[test]
fn counters_name_every_figure() {
    let counters = Counters {
        budget: 7168 * MIB,
        used: 6000 * MIB,
        local: [100 * MIB, 200 * MIB, 300 * MIB],
        spilled: [0, MIB, 5 * MIB],
        peak_spilled: 9 * MIB,
        refused: 2,
        deferred: 3 * MIB,
    };
    assert_eq!(counters.local_total(), 600 * MIB);
    assert_eq!(counters.spilled_total(), 6 * MIB);
    let line = counters.line();
    for part in [
        "6000 of 7168 MiB budget",
        "600 MiB ours",
        "spilled to system memory 6 MiB",
        "peak 9 MiB",
        "refused 2",
        "left unbound 3 MiB",
    ] {
        assert!(line.contains(part), "{part} in {line}");
    }
}

#[test]
fn extension_names_follow_what_is_available() {
    let all = MemoryExtensions {
        budget: true,
        priority: true,
        pageable: true,
    };
    assert_eq!(all.names().len(), 3);
    assert!(MemoryExtensions::default().names().is_empty());
    let priority_only = MemoryExtensions {
        priority: true,
        ..MemoryExtensions::default()
    };
    assert_eq!(priority_only.names(), vec![ash::ext::memory_priority::NAME]);
}
