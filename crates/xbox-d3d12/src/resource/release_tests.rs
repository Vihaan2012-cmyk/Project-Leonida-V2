use std::time::Duration;

use super::desc::{ALLOW_RENDER_TARGET, ResourceDesc};
use super::store::*;
use crate::descriptor::format::dxgi;
use crate::memory::testing::{Kernel, Region, TestGpu};
use crate::memory::{AddressSpace, Backing, Config, PAGE, Placement, SyncPolicy};

/// A mirrored space with the spill tier, over small sizes: the whole region is one memory object.
fn each_spilling_gpu(body: impl Fn(&TestGpu, &mut Resources, &Kernel)) {
    let gpus = TestGpu::open_all();
    if gpus.is_empty() {
        eprintln!("skipped: no GPU with sparse binding and host import");
    }
    for gpu in &gpus {
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
            continue;
        }
        space.tune_release(PAGE, Duration::ZERO);
        let mut resources = Resources::new(space);
        let kernel = Kernel::new(gpu);
        body(gpu, &mut resources, &kernel);
        kernel.finish(gpu);
    }
}

fn texture() -> ResourceDesc {
    let mut desc = ResourceDesc::texture_2d(dxgi::R8G8B8A8_UNORM, 100, 60, 0);
    desc.depth_or_array_size = 2;
    desc
}

/// The texture's size rounded to pages, then room for a buffer of two pages after it.
fn placed_in_one_run(
    resources: &mut Resources,
    desc: &ResourceDesc,
    policy: SyncPolicy,
) -> (Region, u64) {
    let size = desc.allocation_info().unwrap().size.next_multiple_of(PAGE);
    let region = Region::new_in(
        size + 2 * PAGE,
        policy == SyncPolicy::WriteWatch,
        5 << 30,
        1 << 26,
        28,
    );
    resources
        .space_mut()
        .map_host(region.address, region.len, policy)
        .unwrap();
    (region, size)
}

/// Tiles texels derived from `seed` into the texture's memory and returns them, subresource by subresource.
fn write_texels(desc: &ResourceDesc, region: &Region, seed: u32) -> Vec<Vec<u8>> {
    let info = desc.allocation_info().unwrap();
    let array = desc.array_layout().unwrap();
    // SAFETY: the region is ours and at least `info.size` bytes long.
    let memory =
        unsafe { std::slice::from_raw_parts_mut(region.address as *mut u8, info.size as usize) };
    let mut written = Vec::new();
    for slice in 0..desc.array_size() {
        for mip in 0..desc.mips() {
            let m = &array.slice.mips[mip as usize];
            let rows: Vec<u8> = (0..m.blocks_w * m.blocks_h * 4)
                .map(|i| (i * 3 + mip * 17 + slice * 29 + seed * 71) as u8)
                .collect();
            array.tile(slice, mip, &rows, memory).unwrap();
            written.push(rows);
        }
    }
    written
}

fn assert_image(resources: &Resources, id: ResourceId, desc: &ResourceDesc, want: &[Vec<u8>]) {
    let image = resources.get(id).unwrap().image.as_ref().unwrap();
    let handles = resources.space().handles().clone();
    let mut index = 0;
    for slice in 0..desc.array_size() {
        for mip in 0..desc.mips() {
            let got = image
                .download(&handles, mip, slice, want[index].len())
                .unwrap();
            assert!(got == want[index], "slice {slice} mip {mip} differs");
            index += 1;
        }
    }
}

fn placement(resources: &Resources, va: u64) -> Placement {
    resources.space().placement_at(va).expect("bound")
}

#[test]
fn a_texture_keeps_sampling_after_its_memory_leaves_device_local_memory() {
    each_spilling_gpu(|gpu, resources, kernel| {
        let desc = texture();
        let (region, size) = placed_in_one_run(resources, &desc, SyncPolicy::Always);
        let buffer = region.address + size;
        let buffer_id = resources
            .create_placed(buffer, &ResourceDesc::buffer(2 * PAGE))
            .unwrap();
        let want = write_texels(&desc, &region, 1);
        let id = resources.create_placed(region.address, &desc).unwrap();
        resources.upload_texture(id).unwrap();
        assert_eq!(placement(resources, region.address), Placement::DeviceLocal);

        assert_eq!(resources.release_detiled(), size, "{}", gpu.name());
        resources.space_mut().reclaim();
        assert_eq!(placement(resources, region.address), Placement::Spilled);
        assert_eq!(
            placement(resources, buffer),
            Placement::DeviceLocal,
            "the buffer behind it stays"
        );
        let counters = resources.space().handles().budget.counters();
        assert_eq!(counters.released, size);
        assert_image(resources, id, &desc, &want);

        // The window still shows the bytes, for whatever reads them: the GPU copies a texture word to the buffer.
        let word = region.read(1);
        kernel.run(
            gpu,
            &mut resources.space_mut(),
            &[(region.address + 4, buffer)],
            (buffer, 4),
        );
        assert_eq!(region.read(size / 4), word.wrapping_add(1));
        resources.destroy(id).unwrap();
        resources.destroy(buffer_id).unwrap();
        resources
            .space_mut()
            .unmap(region.address, region.len)
            .unwrap();
        resources.space_mut().reclaim();
    });
}

#[test]
fn a_write_the_host_reports_to_released_texture_memory_makes_the_image_again() {
    each_spilling_gpu(|_gpu, resources, _kernel| {
        let desc = texture();
        let (region, size) = placed_in_one_run(resources, &desc, SyncPolicy::Always);
        write_texels(&desc, &region, 1);
        let id = resources.create_placed(region.address, &desc).unwrap();
        resources.upload_texture(id).unwrap();
        resources.release_detiled();
        resources.space_mut().reclaim();
        assert_eq!(placement(resources, region.address), Placement::Spilled);
        assert_eq!(resources.refresh_written(), 0);

        // Streaming reuses the memory: new texels land in it and the host says so (a DirectStorage read).
        let fresh = write_texels(&desc, &region, 2);
        resources.space_mut().mark_dirty(region.address, size);
        assert_eq!(resources.refresh_written(), 1);
        assert_image(resources, id, &desc, &fresh);
        assert_eq!(resources.refresh_written(), 0, "reported once");
        assert_eq!(placement(resources, region.address), Placement::Spilled);
        resources.destroy(id).unwrap();
        resources
            .space_mut()
            .unmap(region.address, region.len)
            .unwrap();
        resources.space_mut().reclaim();
    });
}

#[test]
fn the_title_writing_released_texture_memory_is_found_by_the_write_watch() {
    each_spilling_gpu(|_gpu, resources, _kernel| {
        let desc = texture();
        let (region, _) = placed_in_one_run(resources, &desc, SyncPolicy::WriteWatch);
        let first = write_texels(&desc, &region, 1);
        let id = resources.create_placed(region.address, &desc).unwrap();
        resources.upload_texture(id).unwrap();
        assert_image(resources, id, &desc, &first);
        assert_eq!(resources.refresh_written(), 0, "the read is not news");
        resources.release_detiled();
        resources.space_mut().reclaim();
        assert_eq!(placement(resources, region.address), Placement::Spilled);
        assert_eq!(resources.refresh_written(), 0, "releasing is not a write");

        let fresh = write_texels(&desc, &region, 2);
        assert_eq!(resources.refresh_written(), 1);
        assert_image(resources, id, &desc, &fresh);
        assert_eq!(resources.refresh_written(), 0);
        resources.destroy(id).unwrap();
        resources
            .space_mut()
            .unmap(region.address, region.len)
            .unwrap();
        resources.space_mut().reclaim();
    });
}

#[test]
fn a_buffer_the_gpu_writes_stays_mirrored_beside_released_textures() {
    each_spilling_gpu(|gpu, resources, kernel| {
        let desc = texture();
        let (region, size) = placed_in_one_run(resources, &desc, SyncPolicy::WriteWatch);
        let buffer = region.address + size;
        let buffer_id = resources
            .create_placed(buffer, &ResourceDesc::buffer(2 * PAGE))
            .unwrap();
        write_texels(&desc, &region, 1);
        let id = resources.create_placed(region.address, &desc).unwrap();
        resources.upload_texture(id).unwrap();
        assert_eq!(resources.release_detiled(), size);
        resources.space_mut().reclaim();

        region.write(size / 4, 40);
        resources.space_mut().mark_dirty(buffer, 4);
        let jobs = [(buffer, buffer + PAGE)];
        kernel.run_tracked(gpu, &mut resources.space_mut(), &jobs);
        assert_eq!(region.read((size + PAGE) / 4), 41);
        let jobs = [(buffer + PAGE, buffer + PAGE + 8)];
        kernel.run_tracked(gpu, &mut resources.space_mut(), &jobs);
        assert_eq!(region.read((size + PAGE) / 4 + 2), 42);
        assert_eq!(placement(resources, buffer), Placement::DeviceLocal);
        assert_eq!(placement(resources, buffer + PAGE), Placement::DeviceLocal);
        let classes = resources.space().window_classes();
        assert_eq!(classes.cold.spilled, size);
        assert_eq!(classes.hot.mirrored, 2 * PAGE);
        assert_eq!(classes.hot.spilled, 0);
        resources.destroy(id).unwrap();
        resources.destroy(buffer_id).unwrap();
        resources
            .space_mut()
            .unmap(region.address, region.len)
            .unwrap();
        resources.space_mut().reclaim();
    });
}

#[test]
fn a_buffer_placed_over_texture_memory_after_the_texture_went_takes_it_back() {
    each_spilling_gpu(|_gpu, resources, _kernel| {
        let desc = texture();
        let (region, size) = placed_in_one_run(resources, &desc, SyncPolicy::Always);
        write_texels(&desc, &region, 1);
        let id = resources.create_placed(region.address, &desc).unwrap();
        resources.upload_texture(id).unwrap();
        resources.release_detiled();
        resources.space_mut().reclaim();
        resources.destroy(id).unwrap();
        assert_eq!(placement(resources, region.address), Placement::Spilled);

        resources
            .create_placed(region.address, &ResourceDesc::buffer(PAGE))
            .unwrap();
        resources.release_detiled();
        resources.space_mut().reclaim();
        assert_eq!(placement(resources, region.address), Placement::DeviceLocal);
        assert_eq!(
            placement(resources, region.address + PAGE),
            Placement::Spilled,
            "the rest of the old texture stays out"
        );
        assert_eq!(resources.space().released_bytes(), size - PAGE);
        resources
            .space_mut()
            .unmap(region.address, region.len)
            .unwrap();
        resources.space_mut().reclaim();
    });
}

#[test]
fn render_targets_and_written_textures_are_never_cold() {
    each_spilling_gpu(|_gpu, resources, _kernel| {
        let mut desc = texture();
        desc.flags = ALLOW_RENDER_TARGET;
        let (region, _) = placed_in_one_run(resources, &desc, SyncPolicy::Always);
        write_texels(&desc, &region, 1);
        let id = resources.create_placed(region.address, &desc).unwrap();
        resources.upload_texture(id).unwrap();
        assert_eq!(resources.release_detiled(), 0);
        assert_eq!(placement(resources, region.address), Placement::DeviceLocal);
        let classes = resources.space().window_classes();
        assert_eq!(classes.cold.mirrored + classes.cold.spilled, 0);
        assert!(classes.hot.mirrored >= PAGE);
        resources.destroy(id).unwrap();
        resources
            .space_mut()
            .unmap(region.address, region.len)
            .unwrap();
        resources.space_mut().reclaim();
    });
}
