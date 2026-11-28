use std::time::Duration;

use super::testing::*;
use super::*;

const RUN: u64 = 16 * PAGE;

fn spilling(gpu: &TestGpu) -> Option<AddressSpace> {
    let mut space = AddressSpace::new(
        gpu.handles.clone(),
        Config {
            base: 0,
            size: Some(1 << 34),
            chunk: 1 << 30,
        },
    )
    .unwrap();
    if space.backing() != Backing::Mirrored || !space.spill_enabled() {
        eprintln!("{}: no spill tier: skipped", gpu.name());
        return None;
    }
    space.tune_release(PAGE, Duration::ZERO);
    Some(space)
}

/// Memory above 4 GiB, where runs may be released; below it are descriptor heaps.
fn region(write_watch: bool) -> Region {
    Region::new_in(RUN, write_watch, 5 << 30, 1 << 26, 28)
}

fn placements(space: &AddressSpace, region: &Region) -> Vec<Placement> {
    (0..16)
        .map(|page| {
            space
                .placement_at(region.address + page * PAGE)
                .expect("bound")
        })
        .collect()
}

fn local(h: &Handles) -> u64 {
    h.budget.counters().local[Class::Mirror as usize]
}

fn spilled(h: &Handles) -> u64 {
    h.budget.counters().spilled[Class::Mirror as usize]
}

#[test]
fn a_cold_island_leaves_device_local_memory_and_the_memory_around_it_stays_mirrored() {
    for gpu in &TestGpu::open_all() {
        let Some(mut space) = spilling(gpu) else {
            continue;
        };
        let h = gpu.handles.clone();
        let kernel = Kernel::new(gpu);
        let region = region(false);
        let at = region.address;
        space.map_host(at, RUN, SyncPolicy::Always).unwrap();
        assert_eq!(local(&h), RUN);

        region.write(0, 41);
        kernel.run(gpu, &mut space, &[(at, at + 12 * PAGE)], (at, 4));
        space.note_gpu_write(at + 12 * PAGE, 4);
        assert_eq!(
            region.read(12 * PAGE / 4),
            0,
            "still only on the GPU's side"
        );

        space.mark_cold(at + 4 * PAGE, 4 * PAGE);
        assert_eq!(space.release_cold().unwrap(), 4 * PAGE, "{}", gpu.name());
        assert_eq!(
            region.read(12 * PAGE / 4),
            42,
            "what the GPU wrote was copied back before the copy went"
        );
        space.reclaim();
        let want: Vec<Placement> = (0..16)
            .map(|page| {
                if (4..8).contains(&page) {
                    Placement::Spilled
                } else {
                    Placement::DeviceLocal
                }
            })
            .collect();
        assert_eq!(placements(&space, &region), want);
        assert_eq!((local(&h), spilled(&h)), (12 * PAGE, 4 * PAGE));
        assert_eq!(space.released_bytes(), 4 * PAGE);
        assert_eq!(h.budget.counters().released, 4 * PAGE);

        // The mirrored memory around the island was uploaded again, and the island is read and written in place.
        region.write(4 * PAGE / 4, 500);
        kernel.run(
            gpu,
            &mut space,
            &[
                (at + 12 * PAGE, at + 13 * PAGE),
                (at + 4 * PAGE, at + 5 * PAGE),
            ],
            (at + 13 * PAGE, 4),
        );
        assert_eq!(
            region.read(13 * PAGE / 4),
            43,
            "the live part after the move"
        );
        assert_eq!(
            region.read(5 * PAGE / 4),
            501,
            "the island, read from system memory"
        );

        space.unmap(at, RUN).unwrap();
        space.reclaim();
        assert_eq!(h.budget.counters().local_total(), 0);
        assert_eq!(h.budget.counters().spilled_total(), 0);
        kernel.finish(gpu);
    }
}

#[test]
fn memory_a_buffer_or_the_gpu_uses_stays_mirrored_inside_a_cold_range() {
    for gpu in &TestGpu::open_all() {
        let Some(mut space) = spilling(gpu) else {
            continue;
        };
        let kernel = Kernel::new(gpu);
        let region = region(true);
        let at = region.address;
        space.map_host(at, RUN, SyncPolicy::WriteWatch).unwrap();
        space.mark_hot(at + 8 * PAGE, 4 * PAGE);
        space.mark_cold(at + 4 * PAGE, 12 * PAGE);
        assert_eq!(space.release_cold().unwrap(), 8 * PAGE);
        space.reclaim();

        let spilled_pages = |space: &AddressSpace| -> Vec<u64> {
            (0..16)
                .filter(|p| space.placement_at(at + p * PAGE) == Some(Placement::Spilled))
                .collect()
        };
        assert_eq!(spilled_pages(&space), vec![4, 5, 6, 7, 12, 13, 14, 15]);
        let classes = space.window_classes();
        assert_eq!(classes.cold.spilled, 8 * PAGE);
        assert_eq!(classes.hot.mirrored, 4 * PAGE);
        assert_eq!(classes.other.mirrored, 4 * PAGE);
        assert_eq!(
            classes.cold.mirrored + classes.hot.spilled + classes.other.spilled,
            0
        );

        // The GPU writes into the hot part and the CPU gets the result through the layer's own calls.
        region.write(8 * PAGE / 4, 6);
        space.mark_dirty(at + 8 * PAGE, 4);
        let jobs = [(at + 8 * PAGE, at + 9 * PAGE)];
        kernel.run_tracked(gpu, &mut space, &jobs);
        assert_eq!(region.read(9 * PAGE / 4), 7);
        let jobs = [(at + 9 * PAGE, at + 10 * PAGE)];
        kernel.run_tracked(gpu, &mut space, &jobs);
        assert_eq!(
            region.read(10 * PAGE / 4),
            8,
            "the hot part kept its mirror"
        );
        assert_eq!(spilled_pages(&space).len(), 8);

        space.unmap(at, RUN).unwrap();
        space.reclaim();
        kernel.finish(gpu);
    }
}

#[test]
fn a_hot_claim_over_released_memory_takes_it_back_into_device_local_memory_first() {
    for gpu in &TestGpu::open_all() {
        let Some(mut space) = spilling(gpu) else {
            continue;
        };
        let h = gpu.handles.clone();
        let kernel = Kernel::new(gpu);
        let region = region(false);
        let at = region.address;
        space.map_host(at, RUN, SyncPolicy::Always).unwrap();
        space.mark_cold(at, RUN);
        assert_eq!(space.release_cold().unwrap(), RUN);
        space.reclaim();
        assert_eq!(space.released_bytes(), RUN);
        assert_eq!((local(&h), spilled(&h)), (0, RUN));

        // A buffer is placed over pages 5 and 6 of what was texture memory.
        region.write(5 * PAGE / 4, 20);
        space.mark_hot(at + 5 * PAGE, 2 * PAGE);
        assert_eq!(space.release_cold().unwrap(), 0);
        space.reclaim();
        let device_local: Vec<u64> = (0..16)
            .filter(|p| space.placement_at(at + p * PAGE) == Some(Placement::DeviceLocal))
            .collect();
        assert_eq!(device_local, vec![5, 6]);
        assert_eq!(space.released_bytes(), 14 * PAGE);
        assert_eq!((local(&h), spilled(&h)), (2 * PAGE, 14 * PAGE));
        kernel.run(
            gpu,
            &mut space,
            &[(at + 5 * PAGE, at + 5 * PAGE + 8)],
            (at + 5 * PAGE, 16),
        );
        assert_eq!(region.read(5 * PAGE / 4 + 2), 21);

        space.unmap(at, RUN).unwrap();
        space.reclaim();
        assert_eq!(h.budget.counters().local_total(), 0);
        assert_eq!(h.budget.counters().spilled_total(), 0);
        kernel.finish(gpu);
    }
}

#[test]
fn islands_under_the_minimum_stay_and_unmapping_forgets_the_hints() {
    for gpu in &TestGpu::open_all() {
        let Some(mut space) = spilling(gpu) else {
            continue;
        };
        space.tune_release(3 * PAGE, Duration::ZERO);
        let region = region(false);
        let at = region.address;
        space.map_host(at, RUN, SyncPolicy::Always).unwrap();
        space.mark_cold(at + 2 * PAGE, 2 * PAGE);
        assert_eq!(
            space.release_cold().unwrap(),
            0,
            "two pages are under three"
        );
        space.mark_cold(at + 4 * PAGE, 2 * PAGE);
        assert_eq!(
            space.release_cold().unwrap(),
            4 * PAGE,
            "four pages in a row are not"
        );
        space.mark_cold(at + 8 * PAGE, 4 * PAGE);
        space.unmap(at, RUN).unwrap();
        assert_eq!(space.release_cold().unwrap(), 0);
        assert_eq!(space.window_classes(), WindowClasses::default());
        space.reclaim();
    }
}

#[test]
fn the_cpus_writes_to_cold_memory_are_reported_whether_or_not_it_was_released() {
    for gpu in &TestGpu::open_all() {
        let Some(mut space) = spilling(gpu) else {
            continue;
        };
        let kernel = Kernel::new(gpu);
        let region = region(true);
        let at = region.address;
        space.map_host(at, RUN, SyncPolicy::WriteWatch).unwrap();
        kernel.run(gpu, &mut space, &[], (at, 4));
        space.mark_cold(at + 4 * PAGE, 8 * PAGE);
        assert!(space.take_cold_writes().is_empty());

        // The host's own write (a streaming read landing in the pages) is named by its caller.
        space.mark_dirty(at + 6 * PAGE, 4096);
        space.mark_dirty(at + 14 * PAGE, 4096);
        assert_eq!(
            space.take_cold_writes(),
            vec![(at + 6 * PAGE, at + 6 * PAGE + 4096)]
        );

        // Mirrored, the write watch sees the title's write at the next upload.
        region.touch(5 * PAGE + 4096, 9);
        kernel.run(gpu, &mut space, &[], (at, 4));
        assert_eq!(
            space.take_cold_writes(),
            vec![(at + 5 * PAGE + 4096, at + 5 * PAGE + 8192)]
        );

        // Released, nothing uploads, and the system is asked about the memory itself.
        assert_eq!(space.release_cold().unwrap(), 8 * PAGE);
        space.reclaim();
        let first = space.take_cold_writes();
        assert!(first.is_empty(), "{first:x?}");
        region.touch(9 * PAGE, 3);
        region.touch(9 * PAGE + 4096, 3);
        region.touch(13 * PAGE, 3);
        assert_eq!(
            space.take_cold_writes(),
            vec![(at + 9 * PAGE, at + 9 * PAGE + 8192)]
        );
        assert!(space.take_cold_writes().is_empty(), "reported once");

        space.unmap(at, RUN).unwrap();
        space.reclaim();
        kernel.finish(gpu);
    }
}

#[test]
fn without_the_spill_tier_nothing_is_released() {
    for gpu in &TestGpu::open_all() {
        let mut space = AddressSpace::new(
            gpu.handles.clone(),
            Config {
                base: 0,
                size: Some(1 << 34),
                chunk: 1 << 30,
            },
        )
        .unwrap();
        if space.backing() != Backing::Mirrored {
            continue;
        }
        space.set_spill(false);
        space.tune_release(PAGE, Duration::ZERO);
        let region = region(false);
        space
            .map_host(region.address, RUN, SyncPolicy::Always)
            .unwrap();
        space.mark_cold(region.address, RUN);
        assert_eq!(space.release_cold().unwrap(), 0);
        assert_eq!(space.spill_range(region.address, RUN).unwrap(), 0);
        space.unmap(region.address, RUN).unwrap();
        space.reclaim();
    }
}
