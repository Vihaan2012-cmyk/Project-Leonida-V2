use super::dirty::*;
use super::testing::*;
use super::*;

fn space_with(gpu: &TestGpu, transfer: Option<TransferQueue>) -> AddressSpace {
    AddressSpace::new_with_transfer(
        gpu.handles.clone(),
        Config {
            base: 0,
            size: Some(1 << 38),
            chunk: 1 << 30,
        },
        transfer,
    )
    .unwrap()
}

/// Runs `body` on each GPU, with a transfer queue when `queue` says so (skipped on GPUs without one).
fn each_gpu(queue: bool, body: impl Fn(&TestGpu, &mut AddressSpace, &Kernel)) {
    let gpus = TestGpu::open_all();
    if gpus.is_empty() {
        eprintln!("skipped: no GPU with sparse binding and host import");
    }
    for gpu in &gpus {
        let transfer = if queue {
            match gpu.dedicated.clone().or_else(|| gpu.sibling.clone()) {
                Some(t) => Some(t),
                None => {
                    eprintln!("{}: skipped, no transfer queue", gpu.name());
                    continue;
                }
            }
        } else {
            None
        };
        let mut space = space_with(gpu, transfer);
        eprintln!("{}: {:?}", gpu.name(), space.backing());
        let kernel = Kernel::new(gpu);
        body(gpu, &mut space, &kernel);
        kernel.finish(gpu);
    }
}

#[test]
fn runs_are_joined_cut_and_intersected() {
    let mut runs = vec![(30, 40), (10, 20), (20, 25), (24, 26), (50, 60)];
    coalesce(&mut runs);
    assert_eq!(runs, vec![(10, 26), (30, 40), (50, 60)]);
    let mut set = RangeSet::default();
    set.add(15, 18);
    set.add(35, 55);
    assert_eq!(
        subtract(&runs, &set),
        vec![(10, 15), (18, 26), (30, 35), (55, 60)]
    );
    assert_eq!(intersect(&runs, &set), vec![(15, 18), (35, 40), (50, 55)]);
    assert_eq!(set.pieces(16, 36), vec![(16, 18), (35, 36)]);
    assert!(set.pieces(18, 35).is_empty());
}

#[test]
fn write_watch_reports_partial_pages_and_joins_neighbours() {
    let region = Region::new(4 * PAGE, true);
    let mut tiles = Tiles::default();
    let mut scanner = Scanner::default();
    let mut scan = || {
        let mut out = Vec::new();
        assert!(scanner.scan(&mut tiles, region.address, region.len, 0, &mut out));
        coalesce(&mut out);
        out
    };
    scan();
    region.touch(4, 1);
    assert_eq!(scan(), vec![(region.address, region.address + 4096)]);
    assert!(scan().is_empty());
    region.touch(4096 * 3 + 100, 1);
    region.touch(4096 * 4, 1);
    region.touch(4096 * 5 + 4000, 1);
    let at = region.address;
    assert_eq!(scan(), vec![(at + 4096 * 3, at + 4096 * 6)]);
    region.touch(PAGE + 8, 1);
    region.touch(PAGE + 4096 * 15, 1);
    assert_eq!(
        scan(),
        vec![
            (at + PAGE, at + PAGE + 4096),
            (at + PAGE + 4096 * 15, at + PAGE + 4096 * 16)
        ]
    );
}

#[test]
fn an_unwatched_region_counts_as_written() {
    let region = Region::new(PAGE, false);
    let mut tiles = Tiles::default();
    let mut scanner = Scanner::default();
    let mut out = Vec::new();
    assert!(!scanner.scan(&mut tiles, region.address, region.len, 0, &mut out));
    assert!(out.is_empty());
}

#[test]
fn a_densely_written_tile_is_copied_whole_until_it_calms_down() {
    let len = 4 * TILE;
    let region = Region::new(len, true);
    let mut tiles = Tiles::default();
    let mut scanner = Scanner::default();
    let at = region.address;
    let mut scan = |tiles: &mut Tiles| {
        let mut out = Vec::new();
        assert!(scanner.scan(tiles, at, len, 0, &mut out));
        coalesce(&mut out);
        out
    };
    scan(&mut tiles);
    let dense = |frame: u32| {
        for offset in (TILE..2 * TILE).step_by(4096) {
            region.touch(offset, frame);
        }
    };
    dense(1);
    assert_eq!(scan(&mut tiles), vec![(at + TILE, at + 2 * TILE)]);
    assert_eq!(tiles.hot_tiles(), 0);
    dense(2);
    scan(&mut tiles);
    assert_eq!(tiles.hot_tiles(), 1);
    // A hot tile is copied whole even when nothing wrote it, and a write elsewhere is still found.
    region.touch(3 * TILE + 8, 5);
    assert_eq!(
        scan(&mut tiles),
        vec![
            (at + TILE, at + 2 * TILE),
            (at + 3 * TILE, at + 3 * TILE + 4096)
        ]
    );
    // Written pages in a hot tile are not reset, so they stay reported only through the whole-tile copy.
    for _ in 0..40 {
        scan(&mut tiles);
    }
    assert_eq!(tiles.hot_tiles(), 0);
    assert!(scan(&mut tiles).is_empty());
}

#[test]
fn only_the_cpus_bytes_reach_a_page_that_also_holds_gpu_results() {
    each_gpu(false, |gpu, space, kernel| {
        let region = Region::new(PAGE, true);
        let at = region.address;
        space.map_host(at, PAGE, SyncPolicy::WriteWatch).unwrap();
        space.set_host_access(at, PAGE, HostAccess::OnDemand);
        region.write(0, 10);
        kernel.run_tracked(gpu, space, &[(at, at + 16)]);
        assert_eq!(
            space.stale_bytes(),
            if space.backing() == Backing::Mirrored {
                4
            } else {
                0
            }
        );
        // The CPU writes another word of the same 4 KiB page; the GPU's word 4 must survive the page copy.
        region.write(2, 20);
        kernel.run_tracked(gpu, space, &[(at + 8, at + 24)]);
        space.read_back(at, PAGE).unwrap();
        assert_eq!(region.read(4), 11);
        assert_eq!(region.read(6), 21);
        assert_eq!(space.stale_bytes(), 0);
    });
}

#[test]
fn marking_a_range_dirty_makes_the_cpu_win_over_unread_gpu_results() {
    each_gpu(false, |gpu, space, kernel| {
        let region = Region::new(PAGE, false);
        let at = region.address;
        space.map_host(at, PAGE, SyncPolicy::Marked).unwrap();
        space.set_host_access(at, PAGE, HostAccess::OnDemand);
        region.write(0, 1);
        kernel.run_tracked(gpu, space, &[(at, at + 16)]);
        region.write(4, 500);
        space.mark_dirty(at + 16, 4);
        kernel.run_tracked(gpu, space, &[(at + 16, at + 20)]);
        space.read_back(at, PAGE).unwrap();
        assert_eq!(region.read(5), 501);
        assert_eq!(space.stale_bytes(), 0);
    });
}

#[test]
fn eager_results_are_on_the_host_after_the_submission_and_on_demand_ones_after_read_back() {
    each_gpu(false, |gpu, space, kernel| {
        let region = Region::new(PAGE, false);
        let at = region.address;
        space.map_host(at, PAGE, SyncPolicy::Marked).unwrap();
        space.set_host_access(at + 4096, 4096, HostAccess::OnDemand);
        region.write(0, 7);
        region.write(1024, 70);
        kernel.run_tracked(gpu, space, &[(at, at + 8), (at + 4096, at + 4096 + 8)]);
        assert_eq!(region.read(2), 8);
        if space.backing() == Backing::Mirrored {
            assert_eq!(region.read(1026), 0);
            assert_eq!(space.stale_bytes(), 4);
        }
        assert_eq!(
            space.read_back(at + 4096, 4096).unwrap() > 0,
            space.backing() == Backing::Mirrored
        );
        assert_eq!(region.read(1026), 71);
        assert_eq!(space.read_back(at + 4096, 4096).unwrap(), 0);
    });
}

#[test]
fn a_transfer_queue_copies_what_the_cpu_wrote_and_the_gpu_waits_for_it() {
    each_gpu(true, |gpu, space, kernel| {
        let region = Region::new(8 * PAGE, true);
        let at = region.address;
        space
            .map_host(at, 8 * PAGE, SyncPolicy::WriteWatch)
            .unwrap();
        for round in 0..4u32 {
            for page in 0..8 {
                region.write(page * PAGE / 4, 100 * round + page as u32);
            }
            let (inline, waited) = kernel.run_tracked(
                gpu,
                space,
                &[
                    (at + 4 * PAGE, at + 4 * PAGE + 4),
                    (at + 7 * PAGE, at + 7 * PAGE + 8),
                ],
            );
            let _ = inline;
            assert_eq!(waited, space.backing() == Backing::Mirrored);
            assert_eq!(region.read(PAGE + 1), 100 * round + 4 + 1);
            assert_eq!(region.read(7 * PAGE / 4 + 2), 100 * round + 7 + 1);
        }
    });
}

#[test]
fn what_the_cpu_writes_where_the_gpu_wrote_is_copied_in_order_with_the_gpu() {
    each_gpu(true, |gpu, space, kernel| {
        let region = Region::new(2 * PAGE, true);
        let at = region.address;
        space
            .map_host(at, 2 * PAGE, SyncPolicy::WriteWatch)
            .unwrap();
        region.write(0, 1);
        kernel.run_tracked(gpu, space, &[(at, at + 8)]);
        assert_eq!(region.read(2), 2);
        // The GPU has written in this page before, so the CPU's new bytes are copied in the command buffer.
        region.write(1, 40);
        let (inline, waited) = kernel.run_tracked(gpu, space, &[(at + 4, at + 12)]);
        assert_eq!(region.read(2), 2);
        if space.backing() == Backing::Mirrored {
            assert!(inline > 0);
            assert!(waited);
        }
        assert_eq!(region.read(3), 41);
    });
}

#[test]
fn unmapping_forgets_stale_and_access_state() {
    each_gpu(false, |gpu, space, kernel| {
        let region = Region::new(2 * PAGE, false);
        let at = region.address;
        space.map_host(at, 2 * PAGE, SyncPolicy::Marked).unwrap();
        space.set_host_access(at, 2 * PAGE, HostAccess::OnDemand);
        kernel.run_tracked(gpu, space, &[(at, at + 8)]);
        space.unmap(at, PAGE).unwrap();
        assert_eq!(space.stale_bytes(), 0);
        space.unmap(at + PAGE, PAGE).unwrap();
        space.reclaim();
    });
}

#[test]
fn growing_the_window_keeps_what_the_gpu_wrote() {
    each_gpu(false, |gpu, space, kernel| {
        let region = Region::new(PAGE, false);
        let at = region.address;
        space.map_host(at, PAGE, SyncPolicy::Marked).unwrap();
        space.set_host_access(at, PAGE, HostAccess::OnDemand);
        region.write(0, 3);
        kernel.run_tracked(gpu, space, &[(at, at + 8)]);
        let (base, size) = space.window();
        let far = align_up(base + size, 1 << 30) + (1 << 30);
        if far - base > space.support().max_window {
            return;
        }
        space.ensure(far, far + PAGE).unwrap();
        kernel.run_tracked(gpu, space, &[(at + 8, at + 12)]);
        space.read_back(at, PAGE).unwrap();
        assert_eq!(region.read(2), 4);
        assert_eq!(region.read(3), 5);
    });
}

#[test]
fn never_read_memory_believes_the_cpu_only_through_mark_dirty() {
    each_gpu(false, |gpu, space, kernel| {
        let region = Region::new(PAGE, true);
        let at = region.address;
        space.map_host(at, PAGE, SyncPolicy::WriteWatch).unwrap();
        space.set_host_access(at, PAGE, HostAccess::Never);
        region.write(0, 1);
        kernel.run_tracked(gpu, space, &[(at, at + 8)]);
        space.read_back(at, PAGE).unwrap();
        assert_eq!(region.read(2), 2);
        // An unannounced write is not copied; announced, it is.
        region.write(0, 50);
        kernel.run_tracked(gpu, space, &[(at, at + 12)]);
        let mirrored = space.backing() == Backing::Mirrored;
        space.read_back(at, PAGE).unwrap();
        assert_eq!(region.read(3), if mirrored { 2 } else { 51 });
        region.write(0, 60);
        space.mark_dirty(at, 4);
        kernel.run_tracked(gpu, space, &[(at, at + 12)]);
        space.read_back(at, PAGE).unwrap();
        assert_eq!(region.read(3), 61);
    });
}

#[test]
fn write_watch_follows_host_memory_seen_at_another_address() {
    each_gpu(false, |gpu, space, kernel| {
        let region = Region::new(2 * PAGE, true);
        let alias = 0xC000_0000u64;
        space
            .map_host_at(alias, region.address, 2 * PAGE, SyncPolicy::WriteWatch)
            .unwrap();
        region.write(0, 20);
        let (first, _) = kernel.run_tracked(gpu, space, &[(alias, alias + PAGE + 4)]);
        assert_eq!(region.read(PAGE / 4 + 1), 21);
        region.write(PAGE / 4 + 8, 30);
        let (second, _) = kernel.run_tracked(gpu, space, &[(alias + PAGE + 32, alias + 8)]);
        assert_eq!(region.read(2), 31);
        if space.backing() == Backing::Mirrored {
            assert_eq!(first, 2 * PAGE);
            assert_eq!(second, 4096);
        }
        space.unmap(alias, 2 * PAGE).unwrap();
        space.reclaim();
    });
}
