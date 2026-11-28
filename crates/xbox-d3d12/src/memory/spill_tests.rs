use super::testing::*;
use super::*;

const MIB: u64 = 1 << 20;
const RUN: u64 = 16 * PAGE;

fn space(gpu: &TestGpu, size: u64) -> AddressSpace {
    AddressSpace::new(
        gpu.handles.clone(),
        Config {
            base: 0,
            size: Some(size),
            chunk: 1 << 30,
        },
    )
    .unwrap()
}

/// A mirrored space with the spill tier, or `None` where the GPU does not need or cannot do it.
fn spilling(gpu: &TestGpu) -> Option<AddressSpace> {
    let space = space(gpu, 1 << 38);
    if space.backing() != Backing::Mirrored || !space.spill_enabled() {
        eprintln!(
            "{}: {:?}, spill tier {}: skipped",
            gpu.name(),
            space.backing(),
            space.spill_enabled()
        );
        return None;
    }
    Some(space)
}

/// Memory above 4 GiB, where runs may be spilled; below it they are descriptor heaps and always mirrored.
fn region(len: u64) -> Region {
    Region::new_in(len, false, 5 << 30, 1 << 26, 28)
}

fn mirror_local(h: &Handles) -> u64 {
    h.budget.counters().local[Class::Mirror as usize]
}

fn mirror_spilled(h: &Handles) -> u64 {
    h.budget.counters().spilled[Class::Mirror as usize]
}

/// The GPU adds one to the word at the run's start and stores it two pages on; both are read from the CPU side.
fn increment(gpu: &TestGpu, kernel: &Kernel, space: &mut AddressSpace, region: &Region) -> u32 {
    kernel.run(
        gpu,
        space,
        &[(region.address, region.address + 2 * PAGE)],
        (region.address, RUN),
    );
    region.read(2 * PAGE / 4)
}

#[test]
fn runs_past_the_mirror_limit_go_to_system_memory_and_both_sides_see_each_others_bytes() {
    for gpu in &TestGpu::open_all() {
        let Some(mut space) = spilling(gpu) else {
            continue;
        };
        let h = gpu.handles.clone();
        h.budget.set_mirror_limit(Some(2 * MIB));
        let kernel = Kernel::new(gpu);
        let regions: Vec<Region> = (0..4).map(|_| region(RUN)).collect();
        let policies = [
            SyncPolicy::Always,
            SyncPolicy::Marked,
            SyncPolicy::Always,
            SyncPolicy::Marked,
        ];
        for (region, policy) in regions.iter().zip(policies) {
            space.map_host(region.address, RUN, policy).unwrap();
        }
        assert_eq!(mirror_local(&h), 2 * MIB, "{}", gpu.name());
        assert_eq!(mirror_spilled(&h), 2 * MIB);
        assert_eq!(space.spilled_bytes(), 2 * MIB);

        for (n, region) in regions.iter().enumerate() {
            region.write(0, 41 + n as u32);
            if n < 2 {
                space.mark_dirty(region.address, 4);
            }
            let want = 42 + n as u32;
            assert_eq!(increment(gpu, &kernel, &mut space, region), want, "run {n}");
        }

        // The CPU rewrites each run with no word to the layer for the two spilled ones: what it wrote is what the
        // GPU reads, because there is only the one copy.
        for (n, region) in regions.iter().enumerate() {
            region.write(0, 1000 + n as u32);
            if n < 2 {
                space.mark_dirty(region.address, 4);
            }
            assert_eq!(
                increment(gpu, &kernel, &mut space, region),
                1001 + n as u32,
                "second round, run {n}"
            );
        }

        for region in &regions {
            space.unmap(region.address, RUN).unwrap();
        }
        space.reclaim();
        let counters = h.budget.counters();
        assert_eq!(counters.local_total() + counters.spilled_total(), 0);
        kernel.finish(gpu);
        h.budget.set_mirror_limit(None);
    }
}

#[test]
fn gpu_results_in_spilled_and_mirrored_runs_reach_the_cpu_through_the_layers_own_calls() {
    for gpu in &TestGpu::open_all() {
        let Some(mut space) = spilling(gpu) else {
            continue;
        };
        let h = gpu.handles.clone();
        h.budget.set_mirror_limit(Some(MIB));
        let kernel = Kernel::new(gpu);
        let local = Region::new_in(RUN, true, 5 << 30, 1 << 26, 28);
        let spilled = Region::new_in(RUN, true, 5 << 30, 1 << 26, 28);
        space
            .map_host(local.address, RUN, SyncPolicy::WriteWatch)
            .unwrap();
        space
            .map_host(spilled.address, RUN, SyncPolicy::WriteWatch)
            .unwrap();
        assert_eq!(space.spilled_bytes(), RUN);
        assert!(space.is_bound(spilled.address));
        for (n, region) in [&local, &spilled].into_iter().enumerate() {
            region.write(0, 6 + n as u32);
            let jobs = [(region.address, region.address + 2 * PAGE)];
            kernel.run_tracked(gpu, &mut space, &jobs);
            assert_eq!(region.read(2 * PAGE / 4), 7 + n as u32, "run {n}");
            // A second pass reads what the first wrote: the GPU's own results survive in both tiers.
            let jobs = [(region.address + 2 * PAGE, region.address + 3 * PAGE)];
            kernel.run_tracked(gpu, &mut space, &jobs);
            assert_eq!(region.read(3 * PAGE / 4), 8 + n as u32, "run {n}, chained");
        }
        space.unmap(local.address, RUN).unwrap();
        space.unmap(spilled.address, RUN).unwrap();
        space.reclaim();
        kernel.finish(gpu);
        h.budget.set_mirror_limit(None);
    }
}

#[test]
fn spilled_runs_are_mirrored_again_when_device_local_memory_frees() {
    for gpu in &TestGpu::open_all() {
        let Some(mut space) = spilling(gpu) else {
            continue;
        };
        let h = gpu.handles.clone();
        h.budget.set_mirror_limit(Some(2 * MIB));
        let kernel = Kernel::new(gpu);
        let regions: Vec<Region> = (0..4).map(|_| region(RUN)).collect();
        for region in &regions {
            space
                .map_host(region.address, RUN, SyncPolicy::Always)
                .unwrap();
        }
        assert_eq!(
            (mirror_local(&h), mirror_spilled(&h)),
            (2 * MIB, 2 * MIB),
            "{}",
            gpu.name()
        );
        let spilled: Vec<&Region> = regions
            .iter()
            .filter(|r| {
                space
                    .extents
                    .0
                    .get(&r.address)
                    .is_some_and(|e| space.memories[&e.memory].is_spilled())
            })
            .collect();
        assert_eq!(spilled.len(), 2);
        for region in &spilled {
            region.write(0, 500);
        }
        // Another run frees a megabyte of device-local memory; the next reclaim mirrors one spilled run.
        let gone = regions
            .iter()
            .find(|r| !spilled.iter().any(|s| s.address == r.address))
            .unwrap();
        space.unmap(gone.address, RUN).unwrap();
        space.reclaim();
        assert_eq!(
            (mirror_local(&h), mirror_spilled(&h)),
            (2 * MIB, MIB),
            "one run is promoted"
        );
        assert_eq!(space.spilled_bytes(), MIB);
        for region in &spilled {
            assert_eq!(increment(gpu, &kernel, &mut space, region), 501);
        }
        for region in regions.iter().filter(|r| r.address != gone.address) {
            space.unmap(region.address, RUN).unwrap();
        }
        space.reclaim();
        assert_eq!(h.budget.counters().local_total(), 0);
        assert_eq!(h.budget.counters().spilled_total(), 0);
        kernel.finish(gpu);
        h.budget.set_mirror_limit(None);
    }
}

#[test]
fn a_mirrored_run_moves_to_system_memory_keeping_what_the_gpu_wrote() {
    for gpu in &TestGpu::open_all() {
        let Some(mut space) = spilling(gpu) else {
            continue;
        };
        let h = gpu.handles.clone();
        let kernel = Kernel::new(gpu);
        let region = self::region(RUN);
        space
            .map_host(region.address, RUN, SyncPolicy::Always)
            .unwrap();
        assert_eq!((mirror_local(&h), mirror_spilled(&h)), (RUN, 0));
        region.write(0, 41);
        // The GPU writes its result into the window and the CPU has not been given it yet.
        kernel.run(
            gpu,
            &mut space,
            &[(region.address, region.address + 2 * PAGE)],
            (region.address, 4),
        );
        space.note_gpu_write(region.address + 2 * PAGE, 4);
        assert_eq!(region.read(2 * PAGE / 4), 0, "still only on the GPU's side");
        let moved = space.spill_range(region.address, RUN).unwrap();
        assert_eq!(moved, RUN);
        assert_eq!(
            region.read(2 * PAGE / 4),
            42,
            "{}: copied back before the device-local copy went",
            gpu.name()
        );
        space.reclaim();
        assert_eq!((mirror_local(&h), mirror_spilled(&h)), (0, RUN));
        assert_eq!(space.spill_range(region.address, RUN).unwrap(), 0);
        region.write(0, 100);
        assert_eq!(increment(gpu, &kernel, &mut space, &region), 101);
        space.unmap(region.address, RUN).unwrap();
        space.reclaim();
        assert_eq!(h.budget.counters().spilled_total(), 0);
        kernel.finish(gpu);
    }
}

#[test]
fn spilled_runs_keep_working_when_the_window_is_rebuilt() {
    for gpu in &TestGpu::open_all() {
        let mut space = space(gpu, 1 << 33);
        if space.backing() != Backing::Mirrored || !space.spill_enabled() {
            continue;
        }
        let h = gpu.handles.clone();
        h.budget.set_mirror_limit(Some(0));
        let kernel = Kernel::new(gpu);
        let region = self::region(RUN);
        space
            .map_host(region.address, RUN, SyncPolicy::Marked)
            .unwrap();
        assert_eq!(space.spilled_bytes(), RUN);
        region.write(0, 7);
        let generation = space.generation();
        space.ensure(0, 3 << 33).unwrap();
        assert!(space.generation() > generation, "the window was rebuilt");
        assert_eq!(increment(gpu, &kernel, &mut space, &region), 8);
        space.unmap(region.address, RUN).unwrap();
        space.reclaim();
        kernel.finish(gpu);
        h.budget.set_mirror_limit(None);
    }
}

#[test]
fn runs_below_4_gib_are_never_spilled() {
    for gpu in &TestGpu::open_all() {
        let Some(mut space) = spilling(gpu) else {
            continue;
        };
        let h = gpu.handles.clone();
        h.budget.set_mirror_limit(Some(0));
        let low = Region::new(RUN, false);
        assert!(low.address + RUN <= LOW_LIMIT);
        let high = region(RUN);
        space
            .map_host(low.address, RUN, SyncPolicy::Always)
            .unwrap();
        space
            .map_host(high.address, RUN, SyncPolicy::Always)
            .unwrap();
        assert_eq!((mirror_local(&h), mirror_spilled(&h)), (RUN, RUN));
        space.unmap(low.address, RUN).unwrap();
        space.unmap(high.address, RUN).unwrap();
        space.reclaim();
        h.budget.set_mirror_limit(None);
    }
}

#[test]
fn with_the_tier_off_runs_past_the_limit_stay_in_device_local_memory() {
    for gpu in &TestGpu::open_all() {
        let mut space = space(gpu, 1 << 38);
        if space.backing() != Backing::Mirrored {
            continue;
        }
        space.set_spill(false);
        assert!(!space.spill_enabled());
        let h = gpu.handles.clone();
        h.budget.set_mirror_limit(Some(0));
        let region = self::region(RUN);
        space
            .map_host(region.address, RUN, SyncPolicy::Always)
            .unwrap();
        assert_eq!((mirror_local(&h), mirror_spilled(&h)), (RUN, 0));
        assert_eq!(space.spill_range(region.address, RUN).unwrap(), 0);
        space.unmap(region.address, RUN).unwrap();
        space.reclaim();
        h.budget.set_mirror_limit(None);
    }
}

#[test]
fn the_budget_says_how_much_of_it_the_mirror_may_take() {
    for gpu in &TestGpu::open_all() {
        let budget = &gpu.handles.budget;
        budget.set_limit(Some(1000 * MIB));
        assert_eq!(budget.mirror_cap(), 550 * MIB);
        assert_eq!(budget.mirror_room(), 550 * MIB);
        budget.set_mirror_limit(Some(100 * MIB));
        assert_eq!(budget.mirror_room(), 100 * MIB);
        budget.set_mirror_limit(Some(5000 * MIB));
        assert_eq!(budget.mirror_room(), 1000 * MIB, "no more than is free");
        budget.set_mirror_limit(None);
        budget.set_limit(None);
    }
}

/// The title's pools are runs of 64 KiB page-file sections placed one after the other: a spilled run over such
/// views is what the GPU reads in the game.
#[test]
fn a_spilled_run_over_many_file_section_views_works() {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFileEx,
        PAGE_READWRITE, SEC_COMMIT, UnmapViewOfFile,
    };
    const VIEWS: u64 = 16;
    for gpu in &TestGpu::open_all() {
        let Some(mut space) = spilling(gpu) else {
            continue;
        };
        let h = gpu.handles.clone();
        h.budget.set_mirror_limit(Some(0));
        let kernel = Kernel::new(gpu);
        // SAFETY: sections mapped one after the other at addresses inside the window; every view and handle is
        // released at the end.
        unsafe {
            let mut sections = Vec::new();
            let mut base = (5u64 << 30) + (1 << 28);
            let views: Vec<u64> = 'search: loop {
                let mut mapped: Vec<u64> = Vec::new();
                for n in 0..VIEWS {
                    let section = CreateFileMappingW(
                        INVALID_HANDLE_VALUE,
                        std::ptr::null(),
                        PAGE_READWRITE | SEC_COMMIT,
                        0,
                        PAGE as u32,
                        std::ptr::null(),
                    );
                    assert!(!section.is_null());
                    let view = MapViewOfFileEx(
                        section,
                        FILE_MAP_ALL_ACCESS,
                        0,
                        0,
                        PAGE as usize,
                        (base + n * PAGE) as usize as *const c_void,
                    );
                    if view.Value.is_null() {
                        CloseHandle(section);
                        for (view, section) in mapped.drain(..).zip(sections.drain(..)) {
                            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                                Value: view as usize as *mut c_void,
                            });
                            CloseHandle(section);
                        }
                        base += 1 << 28;
                        continue 'search;
                    }
                    sections.push(section);
                    mapped.push(view.Value as u64);
                }
                break mapped;
            };
            let start = views[0];
            space
                .map_host(start, VIEWS * PAGE, SyncPolicy::Marked)
                .unwrap();
            assert_eq!(space.spilled_bytes(), VIEWS * PAGE);
            for n in 0..VIEWS {
                *((start + n * PAGE) as *mut u32) = 100 + n as u32;
            }
            for n in 0..VIEWS {
                let at = start + n * PAGE;
                kernel.run(gpu, &mut space, &[(at, at + 8)], (at, PAGE));
                assert_eq!(*((at + 8) as *const u32), 101 + n as u32, "view {n}");
            }
            space.unmap(start, VIEWS * PAGE).unwrap();
            space.reclaim();
            for view in views {
                UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: view as usize as *mut c_void,
                });
            }
            for section in sections {
                CloseHandle(section);
            }
        }
        kernel.finish(gpu);
        h.budget.set_mirror_limit(None);
    }
}

#[test]
fn a_mirror_past_a_budget_that_shrank_moves_to_system_memory_keeping_what_the_gpu_wrote() {
    for gpu in &TestGpu::open_all() {
        let Some(mut space) = spilling(gpu) else {
            continue;
        };
        let h = gpu.handles.clone();
        h.budget.set_mirror_limit(Some(8 * MIB));
        let kernel = Kernel::new(gpu);
        let regions: Vec<Region> = (0..4)
            .map(|_| Region::new_in(RUN, true, 5 << 30, 1 << 26, 28))
            .collect();
        for region in &regions {
            space
                .map_host(region.address, RUN, SyncPolicy::WriteWatch)
                .unwrap();
        }
        assert_eq!(mirror_local(&h), 4 * RUN, "{}", gpu.name());
        assert_eq!(space.spilled_bytes(), 0);
        for (n, region) in regions.iter().enumerate() {
            region.write(0, 10 + n as u32);
            let jobs = [(region.address, region.address + 2 * PAGE)];
            kernel.run_tracked(gpu, &mut space, &jobs);
        }

        h.budget.set_mirror_limit(Some(2 * RUN));
        let freed = space.fit_budget().unwrap();
        assert!(freed >= 2 * RUN, "{}: freed {freed}", gpu.name());
        space.reclaim();
        assert!(mirror_local(&h) <= 2 * RUN, "{}", gpu.name());
        assert!(space.spilled_bytes() >= 2 * RUN);
        assert_eq!(space.fit_budget().unwrap(), 0, "within the cap now");
        for (n, region) in regions.iter().enumerate() {
            assert_eq!(
                region.read(2 * PAGE / 4),
                11 + n as u32,
                "run {n} kept the GPU's result"
            );
        }

        for (n, region) in regions.iter().enumerate() {
            region.write(0, 100 + n as u32);
            let jobs = [(region.address, region.address + 2 * PAGE)];
            kernel.run_tracked(gpu, &mut space, &jobs);
            space.read_back(region.address, RUN).unwrap();
            assert_eq!(
                region.read(2 * PAGE / 4),
                101 + n as u32,
                "run {n} after the move"
            );
        }

        for region in &regions {
            space.unmap(region.address, RUN).unwrap();
        }
        space.reclaim();
        kernel.finish(gpu);
        h.budget.set_mirror_limit(None);
    }
}
