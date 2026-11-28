use super::desc::ResourceDesc;
use super::store::*;
use crate::descriptor::format::dxgi;
use crate::memory::testing::{Kernel, Region, TestGpu};
use crate::memory::{AddressSpace, Config, PAGE};

fn resources(gpu: &TestGpu) -> Resources {
    let space = AddressSpace::new(
        gpu.handles.clone(),
        Config {
            base: 0,
            size: Some(1 << 36),
            chunk: 1 << 30,
        },
    )
    .unwrap();
    Resources::new(space)
}

fn each_gpu(body: impl Fn(&TestGpu, &mut Resources, &Kernel)) {
    let gpus = TestGpu::open_all();
    if gpus.is_empty() {
        eprintln!("skipped: no GPU with sparse binding and host import");
    }
    for gpu in &gpus {
        let mut resources = resources(gpu);
        let kernel = Kernel::new(gpu);
        body(gpu, &mut resources, &kernel);
        kernel.finish(gpu);
    }
}

fn word(address: u64) -> *mut u32 {
    address as *mut u32
}

#[test]
fn committed_buffers_get_distinct_aligned_addresses_the_cpu_can_write() {
    each_gpu(|gpu, resources, kernel| {
        let upload = resources
            .create_committed(&ResourceDesc::buffer(100_000), HeapKind::Upload)
            .unwrap();
        let default = resources
            .create_committed(&ResourceDesc::buffer(4096), HeapKind::Default)
            .unwrap();
        let (a, b) = (
            resources.get(upload).unwrap().address,
            resources.get(default).unwrap().address,
        );
        assert!(a.is_multiple_of(PAGE) && b.is_multiple_of(PAGE) && a != b);
        assert_eq!(resources.get(upload).unwrap().size, 131072);
        assert_eq!(resources.find(a + 100_000), Some((upload, 100_000)));
        // SAFETY: committed host memory at the buffer's own address.
        unsafe { *word(a + 8) = 99 };
        kernel.run(
            gpu,
            &mut resources.space_mut(),
            &[(a + 8, b + 4)],
            (b, PAGE),
        );
        // SAFETY: as above.
        assert_eq!(unsafe { *word(b + 4) }, 100);
        resources.destroy(upload).unwrap();
        resources.destroy(default).unwrap();
        assert!(resources.find(a).is_none());
        let again = resources
            .create_committed(&ResourceDesc::buffer(100_000), HeapKind::Upload)
            .unwrap();
        assert_eq!(resources.get(again).unwrap().address, a);
    });
}

#[test]
fn a_buffer_placed_in_game_memory_is_that_memory() {
    each_gpu(|gpu, resources, kernel| {
        let region = Region::new(4 * PAGE, false);
        region.write(0, 7);
        let desc = ResourceDesc::buffer(3 * PAGE);
        let id = resources.create_placed(region.address, &desc).unwrap();
        assert_eq!(resources.get(id).unwrap().address, region.address);
        assert!(resources.space().is_bound(region.address + 2 * PAGE));
        assert!(!resources.space().is_bound(region.address + 3 * PAGE));
        kernel.run(
            gpu,
            &mut resources.space_mut(),
            &[(region.address, region.address + PAGE)],
            (region.address, 3 * PAGE),
        );
        assert_eq!(region.read(PAGE / 4), 8);
        assert!(matches!(
            resources.create_placed(region.address + PAGE, &ResourceDesc::buffer(PAGE)),
            Err(ResourceError::Overlap { .. })
        ));
        assert!(matches!(
            resources.create_placed(region.address + 4, &desc),
            Err(ResourceError::Overlap { .. })
        ));
        resources.destroy(id).unwrap();
        // The console places buffers at any address of mapped memory, as the title does at boot.
        let small = resources
            .create_placed(region.address + 0x840, &ResourceDesc::buffer(0x100))
            .unwrap();
        assert_eq!(
            resources.get(small).unwrap().address,
            region.address + 0x840
        );
        assert!(resources.space().is_bound(region.address));
        resources.destroy(small).unwrap();
        resources
            .space_mut()
            .unmap(region.address, 3 * PAGE)
            .unwrap();
    });
}

#[test]
fn a_placed_tiled_texture_is_read_from_game_memory_and_written_back() {
    each_gpu(|_gpu, resources, _kernel| {
        let mut desc = ResourceDesc::texture_2d(dxgi::R8G8B8A8_UNORM, 100, 60, 0);
        desc.depth_or_array_size = 2;
        let info = desc.allocation_info().unwrap();
        let region = Region::new(info.size, false);
        let array = desc.array_layout().unwrap();
        // SAFETY: the region is ours and `info.size` bytes long.
        let memory = unsafe {
            std::slice::from_raw_parts_mut(region.address as *mut u8, info.size as usize)
        };
        let mut written = Vec::new();
        for slice in 0..2u32 {
            for mip in 0..desc.mips() {
                let m = &array.slice.mips[mip as usize];
                let rows: Vec<u8> = (0..m.blocks_w * m.blocks_h * 4)
                    .map(|i| (i * 3 + mip * 17 + slice * 29) as u8)
                    .collect();
                array.tile(slice, mip, &rows, memory).unwrap();
                written.push(rows);
            }
        }
        let id = resources.create_placed(region.address, &desc).unwrap();
        assert_eq!(resources.get(id).unwrap().address, region.address);
        resources.upload_texture(id).unwrap();
        let image = resources.get(id).unwrap().image.as_ref().unwrap();
        let handles = &resources.space().handles().clone();
        let mut index = 0;
        for slice in 0..2u32 {
            for mip in 0..desc.mips() {
                let bytes = written[index].len();
                let got = image.download(handles, mip, slice, bytes).unwrap();
                assert!(
                    got == written[index],
                    "slice {slice} mip {mip}: {} bytes, first difference at {:?}",
                    bytes,
                    got.iter().zip(&written[index]).position(|(a, b)| a != b)
                );
                index += 1;
            }
        }
        memory.fill(0);
        resources.download_texture(id).unwrap();
        // SAFETY: a fresh view of the region, taken after the layer wrote through its own pointer.
        let back =
            unsafe { std::slice::from_raw_parts(region.address as *const u8, info.size as usize) };
        assert_eq!(
            array.detile(1, 2, back).unwrap(),
            written[desc.mips() as usize + 2]
        );
        resources.destroy(id).unwrap();
    });
}

#[test]
fn a_row_major_texture_keeps_its_pitch() {
    each_gpu(|_gpu, resources, _kernel| {
        let mut desc = ResourceDesc::texture_2d(dxgi::R8G8B8A8_UNORM, 10, 6, 1);
        desc.layout = super::desc::Layout::RowMajor;
        let region = Region::new(desc.allocation_info().unwrap().size, false);
        for row in 0..6u64 {
            for x in 0..10u64 {
                region.write(row * 64 + x, (row * 100 + x) as u32);
            }
        }
        let id = resources.create_placed(region.address, &desc).unwrap();
        resources.upload_texture(id).unwrap();
        let image = resources.get(id).unwrap().image.as_ref().unwrap();
        let rows = image
            .download(resources.space().handles(), 0, 0, 240)
            .unwrap();
        let texel = |row: usize, x: usize| {
            u32::from_le_bytes(rows[(row * 10 + x) * 4..][..4].try_into().unwrap())
        };
        assert_eq!(texel(5, 9), 509);
        assert_eq!(texel(2, 0), 200);
        resources.destroy(id).unwrap();
    });
}

#[test]
fn committed_textures_are_keyed_by_an_address_nothing_is_mapped_at() {
    each_gpu(|_gpu, resources, _kernel| {
        let desc = ResourceDesc::texture_2d(dxgi::BC7_UNORM, 64, 64, 0);
        let a = resources
            .create_committed(&desc, HeapKind::Default)
            .unwrap();
        let b = resources
            .create_committed(&desc, HeapKind::Default)
            .unwrap();
        let (ka, kb) = (
            resources.get(a).unwrap().address,
            resources.get(b).unwrap().address,
        );
        assert!(ka >= 1 << 44 && kb >= ka + 65536 && ka.is_multiple_of(256));
        assert!(!resources.space().is_bound(ka));
        assert_eq!(resources.find(ka + 5), Some((a, 5)));
        assert!(matches!(
            resources.upload_texture(a),
            Err(ResourceError::NoGuestMemory(_))
        ));
        resources.destroy(a).unwrap();
        resources.destroy(b).unwrap();
        assert!(matches!(
            resources.destroy(a),
            Err(ResourceError::Unknown(_))
        ));
    });
}

#[test]
fn a_placed_texture_over_a_buffer_gets_a_key() {
    each_gpu(|_gpu, resources, _kernel| {
        let region = Region::new(65536, false);
        let buffer = resources
            .create_placed(region.address, &ResourceDesc::buffer(65536))
            .unwrap();
        let mut desc = ResourceDesc::texture_2d(dxgi::R8G8B8A8_UNORM, 32, 32, 1);
        desc.layout = super::desc::Layout::RowMajor;
        let texture = resources.create_placed(region.address, &desc).unwrap();
        assert_ne!(resources.get(texture).unwrap().address, region.address);
        assert_eq!(resources.get(texture).unwrap().memory, region.address);
        resources.destroy(texture).unwrap();
        resources.destroy(buffer).unwrap();
        resources.space_mut().unmap(region.address, 65536).unwrap();
    });
}

#[test]
fn a_reserved_buffer_shows_the_tiles_mapped_into_it() {
    each_gpu(|gpu, resources, kernel| {
        let id = resources
            .create_reserved(&ResourceDesc::buffer(8 * PAGE))
            .unwrap();
        let reserved = resources.get(id).unwrap().address;
        assert!(!resources.space().is_bound(reserved));
        let host = Region::new(2 * PAGE, false);
        host.write(0, 50);
        resources
            .map_tiles(id, 3 * PAGE, host.address, 2 * PAGE)
            .unwrap();
        assert!(resources.space().is_bound(reserved + 4 * PAGE));
        assert!(!resources.space().is_bound(reserved));
        kernel.run(
            gpu,
            &mut resources.space_mut(),
            &[(reserved + 3 * PAGE, reserved + 3 * PAGE + 4)],
            (reserved + 3 * PAGE, PAGE),
        );
        assert_eq!(host.read(1), 51);
        resources.unmap_tiles(id, 3 * PAGE, 2 * PAGE).unwrap();
        assert!(!resources.space().is_bound(reserved + 3 * PAGE));
        resources.destroy(id).unwrap();
    });
}

#[test]
fn descriptor_heap_memory_is_below_four_gib_and_bound() {
    each_gpu(|gpu, resources, kernel| {
        let heap = resources
            .alloc_low(100_000, crate::memory::SyncPolicy::Always)
            .unwrap();
        assert!(heap < 1 << 32 && heap.is_multiple_of(PAGE));
        assert!(resources.space().is_bound(heap));
        // SAFETY: committed host memory at `heap`, at least 100,000 bytes.
        unsafe { *word(heap) = 3 };
        kernel.run(
            gpu,
            &mut resources.space_mut(),
            &[(heap, heap + 4)],
            (heap, PAGE),
        );
        // SAFETY: as above.
        assert_eq!(unsafe { *word(heap + 4) }, 4);
        resources.free_low(heap, 100_000);
        assert!(!resources.space().is_bound(heap));
    });
}
