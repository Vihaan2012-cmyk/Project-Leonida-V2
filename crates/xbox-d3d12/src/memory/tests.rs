use super::testing::*;
use super::*;

fn cut(va: u64, len: u64, memory: u64) -> Cut {
    Cut { va, len, memory }
}

fn extent(len: u64, memory: u64, memory_offset: u64) -> Extent {
    Extent {
        len,
        memory,
        memory_offset,
    }
}

#[test]
fn extents_split_around_a_hole() {
    let mut map = Extents::default();
    map.insert(0x10000, extent(0x40000, 1, 0));
    let cuts = map.remove(0x20000, 0x10000);
    assert_eq!(cuts, vec![cut(0x20000, 0x10000, 1)]);
    assert_eq!(map.0[&0x10000], extent(0x10000, 1, 0));
    assert_eq!(map.0[&0x30000], extent(0x20000, 1, 0x20000));
    assert_eq!(map.0.len(), 2);
}

#[test]
fn extents_cut_several_and_ignore_gaps() {
    let mut map = Extents::default();
    for (i, va) in [0x10000u64, 0x20000, 0x40000].into_iter().enumerate() {
        map.insert(va, extent(0x10000, i as u64 + 1, 0));
    }
    let cuts = map.remove(0x8000, 0x30000);
    assert_eq!(
        cuts,
        vec![cut(0x10000, 0x10000, 1), cut(0x20000, 0x10000, 2)]
    );
    assert_eq!(map.0.keys().copied().collect::<Vec<_>>(), vec![0x40000]);
    assert!(map.remove(0x100000, 0x10000).is_empty());
}

#[test]
fn extents_cut_the_start_of_one_that_begins_before() {
    let mut map = Extents::default();
    map.insert(0x10000, extent(0x30000, 7, 0x5_0000));
    let cuts = map.remove(0x0, 0x20000);
    assert_eq!(cuts, vec![cut(0x10000, 0x10000, 7)]);
    assert_eq!(map.0[&0x20000], extent(0x20000, 7, 0x6_0000));
    let hit = map.overlapping(0x28000, 0x31000);
    assert_eq!(hit.len(), 1);
    assert_eq!((hit[0].2, hit[0].3), (0x28000, 0x31000));
}

#[test]
fn range_sets_merge_and_split() {
    let mut set = RangeSet::default();
    set.add(10, 20);
    set.add(30, 40);
    set.add(20, 30);
    assert_eq!(set.0.iter().collect::<Vec<_>>(), vec![(&10, &40)]);
    set.add(5, 12);
    set.add(50, 60);
    set.remove(15, 55);
    assert_eq!(
        set.0.iter().map(|(a, b)| (*a, *b)).collect::<Vec<_>>(),
        vec![(5, 15), (55, 60)]
    );
    assert_eq!(set.take().len(), 2);
    assert!(set.0.is_empty());
}

// SPIR-V of a compute shader (GLSL, buffer_reference) that stores `*(heap + src) + 1` at `heap + dst`, with
// `heap`, `src` and `dst` three 64-bit push constants.
const INCREMENT: &[u32] = include!("test_shader.in");

struct Kernel {
    module: vk::ShaderModule,
    layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    pool: vk::CommandPool,
}

impl Kernel {
    fn new(gpu: &TestGpu) -> Kernel {
        let device = &gpu.handles.device;
        // SAFETY: test-only Vulkan use; `finish` destroys everything.
        unsafe {
            let module = device
                .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(INCREMENT), None)
                .unwrap();
            let range = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::COMPUTE)
                .size(24)];
            let layout = device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&range),
                    None,
                )
                .unwrap();
            let stage = vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::COMPUTE)
                .module(module)
                .name(c"main");
            let pipeline = device
                .create_compute_pipelines(
                    vk::PipelineCache::null(),
                    &[vk::ComputePipelineCreateInfo::default()
                        .stage(stage)
                        .layout(layout)],
                    None,
                )
                .unwrap()[0];
            let pool = device
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default().queue_family_index(gpu.family),
                    None,
                )
                .unwrap();
            Kernel {
                module,
                layout,
                pipeline,
                pool,
            }
        }
    }

    /// One submission: uploads, then each `(src, dst)` increment, then downloads of `download`. Returns the
    /// bytes the uploads copied.
    fn run(
        &self,
        gpu: &TestGpu,
        space: &mut AddressSpace,
        jobs: &[(u64, u64)],
        download: (u64, u64),
    ) -> u64 {
        let device = &gpu.handles.device;
        // SAFETY: test-only Vulkan use; the command buffer completes before returning.
        unsafe {
            let command = device
                .allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(self.pool)
                        .command_buffer_count(1),
                )
                .unwrap()[0];
            device
                .begin_command_buffer(command, &vk::CommandBufferBeginInfo::default())
                .unwrap();
            let uploaded = space.record_uploads(command);
            device.cmd_bind_pipeline(command, vk::PipelineBindPoint::COMPUTE, self.pipeline);
            for &(src, dst) in jobs {
                let mut push = Vec::new();
                for word in [space.heap_base(), src, dst] {
                    push.extend_from_slice(&word.to_le_bytes());
                }
                device.cmd_push_constants(
                    command,
                    self.layout,
                    vk::ShaderStageFlags::COMPUTE,
                    0,
                    &push,
                );
                device.cmd_dispatch(command, 1, 1, 1);
                let barrier = [vk::MemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                    .dst_access_mask(vk::AccessFlags::SHADER_READ)];
                device.cmd_pipeline_barrier(
                    command,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::DependencyFlags::empty(),
                    &barrier,
                    &[],
                    &[],
                );
            }
            space.record_downloads(command, download.0, download.1);
            device.end_command_buffer(command).unwrap();
            let fence = device
                .create_fence(&vk::FenceCreateInfo::default(), None)
                .unwrap();
            let commands = [command];
            device
                .queue_submit(
                    gpu.handles.queue,
                    &[vk::SubmitInfo::default().command_buffers(&commands)],
                    fence,
                )
                .unwrap();
            device.wait_for_fences(&[fence], true, u64::MAX).unwrap();
            device.destroy_fence(fence, None);
            device.free_command_buffers(self.pool, &commands);
            uploaded
        }
    }

    fn finish(self, gpu: &TestGpu) {
        let device = &gpu.handles.device;
        // SAFETY: nothing is in flight.
        unsafe {
            device.destroy_command_pool(self.pool, None);
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_shader_module(self.module, None);
        }
    }
}

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

fn each_gpu(body: impl Fn(&TestGpu, &mut AddressSpace, &Kernel)) {
    let gpus = TestGpu::open_all();
    if gpus.is_empty() {
        eprintln!("skipped: no GPU with sparse binding and host import");
    }
    for gpu in &gpus {
        let mut space = space(gpu);
        eprintln!("{}: {:?}", gpu.name(), space.backing());
        let kernel = Kernel::new(gpu);
        body(gpu, &mut space, &kernel);
        kernel.finish(gpu);
    }
}

#[test]
fn a_shader_reads_and_writes_host_memory_at_its_own_address() {
    each_gpu(|gpu, space, kernel| {
        let region = Region::new(4 * PAGE, false);
        space
            .map_host(region.address, 4 * PAGE, SyncPolicy::Always)
            .unwrap();
        assert!(space.is_bound(region.address));
        assert!(!space.is_bound(region.address + 4 * PAGE));
        region.write(0, 41);
        region.write(2 * PAGE / 4, 1000);
        let whole = (region.address, 4 * PAGE);
        kernel.run(
            gpu,
            space,
            &[(region.address, region.address + 2 * PAGE + 8)],
            whole,
        );
        assert_eq!(region.read((2 * PAGE + 8) / 4), 42);
        kernel.run(
            gpu,
            space,
            &[(region.address + 2 * PAGE, region.address + 4)],
            whole,
        );
        assert_eq!(region.read(1), 1001);
        space.unmap(region.address, 4 * PAGE).unwrap();
        space.reclaim();
        assert_eq!(space.memory_objects(), 0);
    });
}

#[test]
fn unmapping_one_page_leaves_its_neighbours_working() {
    each_gpu(|gpu, space, kernel| {
        let region = Region::new(3 * PAGE, false);
        space
            .map_host(region.address, 3 * PAGE, SyncPolicy::Always)
            .unwrap();
        space.unmap(region.address + PAGE, PAGE).unwrap();
        assert!(space.is_bound(region.address));
        assert!(!space.is_bound(region.address + PAGE));
        assert!(space.is_bound(region.address + 2 * PAGE));
        region.write(2 * PAGE / 4, 6);
        kernel.run(
            gpu,
            space,
            &[(region.address + 2 * PAGE, region.address)],
            (region.address, PAGE),
        );
        assert_eq!(region.read(0), 7);
        space.reclaim();
        assert_eq!(space.memory_objects(), 1);
    });
}

#[test]
fn marked_pages_are_the_only_ones_copied_to_a_mirrored_window() {
    each_gpu(|gpu, space, kernel| {
        let region = Region::new(4 * PAGE, false);
        space
            .map_host(region.address, 4 * PAGE, SyncPolicy::Marked)
            .unwrap();
        region.write(0, 5);
        let done = (region.address + 4, 4);
        let job = [(region.address, region.address + 4)];
        let first = kernel.run(gpu, space, &job, done);
        assert_eq!(region.read(1), 6);
        // Written behind the layer's back and not marked: a mirrored window keeps the old value.
        region.write(0, 100);
        let second = kernel.run(gpu, space, &job, done);
        if space.backing() == Backing::Mirrored {
            assert_eq!(first, 4 * PAGE);
            assert_eq!(second, 0);
            assert_eq!(region.read(1), 6);
            space.mark_dirty(region.address, 4);
            let third = kernel.run(gpu, space, &job, done);
            assert_eq!(third, 4);
        }
        assert_eq!(region.read(1), 101);
    });
}

#[test]
fn write_watch_finds_the_pages_the_cpu_wrote() {
    each_gpu(|gpu, space, kernel| {
        let region = Region::new(2 * PAGE, true);
        space
            .map_host(region.address, 2 * PAGE, SyncPolicy::WriteWatch)
            .unwrap();
        let done = (region.address, PAGE);
        kernel.run(gpu, space, &[(region.address, region.address + 4)], done);
        region.write(PAGE / 4 + 3, 77);
        let copied = kernel.run(
            gpu,
            space,
            &[(region.address + PAGE + 12, region.address + 8)],
            done,
        );
        if space.backing() == Backing::Mirrored {
            assert_eq!(copied, 4096);
        }
        assert_eq!(region.read(2), 78);
    });
}

#[test]
fn the_window_grows_to_cover_a_late_mapping_and_keeps_old_bindings() {
    for gpu in &TestGpu::open_all() {
        let kernel = Kernel::new(gpu);
        let a = Region::new(PAGE, false);
        let b = Region::new(PAGE, false);
        let (low, high) = if a.address < b.address {
            (a, b)
        } else {
            (b, a)
        };
        let start = align_up(high.address + PAGE, 1 << 30);
        let mut space = AddressSpace::new(
            gpu.handles.clone(),
            Config {
                base: start,
                size: Some(1 << 30),
                chunk: 1 << 30,
            },
        )
        .unwrap();
        let generation = space.generation();
        let needed = align_up(start + (1 << 30) - align_down(low.address, PAGE), 1 << 30);
        if needed > space.support().max_window {
            let error = space.map_host(low.address, PAGE, SyncPolicy::Always);
            assert!(matches!(error, Err(MemoryError::WindowTooLarge { .. })));
            kernel.finish(gpu);
            continue;
        }
        space
            .map_host(high.address, PAGE, SyncPolicy::Always)
            .unwrap();
        space
            .map_host(low.address, PAGE, SyncPolicy::Always)
            .unwrap();
        assert!(space.generation() > generation);
        let (base, size) = space.window();
        assert!(base <= low.address && high.address + PAGE <= base + size);
        high.write(0, 9);
        kernel.run(
            gpu,
            &mut space,
            &[(high.address, low.address + 4)],
            (low.address, PAGE),
        );
        assert_eq!(low.read(1), 10);
        kernel.finish(gpu);
    }
}

#[test]
fn pages_mapped_from_file_sections_are_seen_at_their_own_address() {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFileEx,
        PAGE_READWRITE, SEC_COMMIT, UnmapViewOfFile,
    };
    each_gpu(|gpu, space, kernel| {
        // SAFETY: two sections mapped at addresses inside the window; every view and handle is released at
        // the end.
        unsafe {
            let mut sections = Vec::new();
            let mut views = Vec::new();
            let mut hint = (5u64 << 29) + (1 << 26);
            for _ in 0..2 {
                let section = CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    std::ptr::null(),
                    PAGE_READWRITE | SEC_COMMIT,
                    0,
                    PAGE as u32,
                    std::ptr::null(),
                );
                assert!(!section.is_null());
                let view = loop {
                    let view = MapViewOfFileEx(
                        section,
                        FILE_MAP_ALL_ACCESS,
                        0,
                        0,
                        PAGE as usize,
                        hint as usize as *const c_void,
                    );
                    hint += 1 << 26;
                    if !view.Value.is_null() {
                        break view.Value;
                    }
                };
                sections.push(section);
                views.push(view as u64);
            }
            space.map_pages_scatter(&views, SyncPolicy::Always).unwrap();
            for &view in &views {
                *(view as *mut u32) = 100;
                kernel.run(gpu, space, &[(view, view + 8)], (view, PAGE));
                assert_eq!(*((view + 8) as *const u32), 101);
            }
            space.unmap(views[0], PAGE).unwrap();
            assert!(!space.is_bound(views[0]));
            assert!(space.is_bound(views[1]));
            space.unmap(views[1], PAGE).unwrap();
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
    });
}
