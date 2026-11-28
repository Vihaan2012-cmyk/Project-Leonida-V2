//! Measurements of what a GPU does when the game maps more memory than it has. Every test here is `#[ignore]`d;
//! run `cargo test -p xbox-d3d12 --lib memory::probe -- --ignored --nocapture --test-threads=1`.
//!
//! `LEONIDA_PROBE_GPU` (default `NVIDIA`) picks the device by name, `LEONIDA_PROBE_GIB` (default 10, at most 10)
//! the oversubscription, `LEONIDA_PROBE_PRIORITY` (default 0.1) the priority of each 1 GiB block and
//! `LEONIDA_PROBE_DEMOTE` set makes the probe lower the priority of all but the newest blocks to zero.
//! The oversubscription probe stops when free system memory drops under 16 GB.

use std::time::Instant;

use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

use super::testing::*;
use super::*;

const GIB: u64 = 1 << 30;
const STOP_BELOW: u64 = 16_000_000_000;

fn free_ram() -> u64 {
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..unsafe { std::mem::zeroed() }
    };
    // SAFETY: a plain query into a correctly sized structure.
    unsafe { GlobalMemoryStatusEx(&mut status) };
    status.ullAvailPhys
}

fn picked(gpus: &[TestGpu]) -> Vec<&TestGpu> {
    let want = std::env::var("LEONIDA_PROBE_GPU").unwrap_or_else(|_| "NVIDIA".into());
    gpus.iter().filter(|g| g.name().contains(&want)).collect()
}

fn local_type(types: &vk::PhysicalDeviceMemoryProperties, bits: u32) -> Option<u32> {
    (0..types.memory_type_count).find(|i| {
        let f = types.memory_types[*i as usize].property_flags;
        bits & (1 << i) != 0
            && f.contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
            && !f.contains(vk::MemoryPropertyFlags::HOST_VISIBLE)
    })
}

fn type_list(bits: u32) -> Vec<u32> {
    (0..32).filter(|i| bits & (1 << i) != 0).collect()
}

fn non_sparse_buffer(gpu: &TestGpu, size: u64, external: bool) -> vk::Buffer {
    let mut host = vk::ExternalMemoryBufferCreateInfo::default()
        .handle_types(vk::ExternalMemoryHandleTypeFlags::HOST_ALLOCATION_EXT);
    let mut info = vk::BufferCreateInfo::default().size(size).usage(
        vk::BufferUsageFlags::TRANSFER_SRC
            | vk::BufferUsageFlags::TRANSFER_DST
            | vk::BufferUsageFlags::STORAGE_BUFFER
            | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
    );
    if external {
        info = info.push_next(&mut host);
    }
    // SAFETY: a fresh buffer the probe destroys.
    unsafe { gpu.handles.device.create_buffer(&info, None).unwrap() }
}

#[test]
#[ignore = "measurement"]
fn probe_the_memory_types_each_buffer_kind_accepts() {
    for gpu in &TestGpu::open_all() {
        let h = &gpu.handles;
        let families = [gpu.family];
        let plain = AddressSpace::make_window(h, 0, 1 << 36, false, &families).unwrap();
        let external = AddressSpace::make_window(h, 0, 1 << 36, true, &families).unwrap();
        let space = AddressSpace::new(
            h.clone(),
            Config {
                base: 0,
                size: Some(1 << 38),
                chunk: 1 << 30,
            },
        )
        .unwrap();
        let region = Region::new(16 * PAGE, false);
        let imported = space.host_type_bits(region.address).unwrap();
        let loose = non_sparse_buffer(gpu, GIB, false);
        let loose_external = non_sparse_buffer(gpu, GIB, true);
        // SAFETY: buffers made above.
        let (loose_bits, external_bits) = unsafe {
            (
                h.device
                    .get_buffer_memory_requirements(loose)
                    .memory_type_bits,
                h.device
                    .get_buffer_memory_requirements(loose_external)
                    .memory_type_bits,
            )
        };
        eprintln!(
            "{}: backing {:?}\n  sparse window types {:?}, with host import {:?}\n  imported host memory types {:?}\n  \
             non-sparse device-address buffer types {:?}, with host import {:?}",
            gpu.name(),
            space.backing(),
            type_list(plain.type_bits),
            type_list(external.type_bits),
            type_list(imported),
            type_list(loose_bits),
            type_list(external_bits),
        );
        // SAFETY: nothing uses these.
        unsafe {
            h.device.destroy_buffer(plain.buffer, None);
            h.device.destroy_buffer(external.buffer, None);
            h.device.destroy_buffer(loose, None);
            h.device.destroy_buffer(loose_external, None);
        }
    }
}

struct Staged {
    command: vk::CommandBuffer,
}

fn one_shot(
    gpu: &TestGpu,
    pool: vk::CommandPool,
    record: impl FnOnce(vk::CommandBuffer),
) -> Staged {
    let device = &gpu.handles.device;
    // SAFETY: test-only Vulkan use; the caller submits and frees.
    unsafe {
        let command = device
            .allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .command_buffer_count(1),
            )
            .unwrap()[0];
        device
            .begin_command_buffer(command, &vk::CommandBufferBeginInfo::default())
            .unwrap();
        record(command);
        device.end_command_buffer(command).unwrap();
        Staged { command }
    }
}

fn submit_and_wait(gpu: &TestGpu, pool: vk::CommandPool, staged: Staged) -> Result<(), vk::Result> {
    let device = &gpu.handles.device;
    // SAFETY: test-only Vulkan use.
    unsafe {
        let fence = device.create_fence(&vk::FenceCreateInfo::default(), None)?;
        let commands = [staged.command];
        let submitted = device.queue_submit(
            gpu.handles.queue,
            &[vk::SubmitInfo::default().command_buffers(&commands)],
            fence,
        );
        let waited = submitted.and_then(|()| device.wait_for_fences(&[fence], true, u64::MAX));
        device.destroy_fence(fence, None);
        device.free_command_buffers(pool, &commands);
        waited
    }
}

/// A non-sparse buffer over imported host memory is read and written by a shader through its device address,
/// and the copy bandwidth between it and device-local memory.
#[test]
#[ignore = "measurement"]
fn probe_a_device_address_buffer_over_imported_host_memory() {
    for gpu in &TestGpu::open_all() {
        let h = &gpu.handles;
        let space = AddressSpace::new(
            h.clone(),
            Config {
                base: 0,
                size: Some(1 << 38),
                chunk: 1 << 30,
            },
        )
        .unwrap();
        let len = 256 << 20;
        let region = Region::new(len, false);
        let bits = space.host_type_bits(region.address).unwrap();
        let buffer = non_sparse_buffer(gpu, len, true);
        // SAFETY: the region stays mapped until the end; everything made here is destroyed.
        unsafe {
            let needs = h.device.get_buffer_memory_requirements(buffer);
            let kinds = needs.memory_type_bits & bits;
            eprintln!(
                "{}: import types {:?} & buffer types {:?} = {:?}",
                gpu.name(),
                type_list(bits),
                type_list(needs.memory_type_bits),
                type_list(kinds)
            );
            if kinds == 0 {
                h.device.destroy_buffer(buffer, None);
                continue;
            }
            let kind = kinds.trailing_zeros();
            let mut import = vk::ImportMemoryHostPointerInfoEXT::default()
                .handle_type(vk::ExternalMemoryHandleTypeFlags::HOST_ALLOCATION_EXT)
                .host_pointer(region.address as usize as *mut c_void);
            let mut flags = vk::MemoryAllocateFlagsInfo::default()
                .flags(vk::MemoryAllocateFlags::DEVICE_ADDRESS);
            let memory = h
                .device
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(len)
                        .memory_type_index(kind)
                        .push_next(&mut import)
                        .push_next(&mut flags),
                    None,
                )
                .unwrap();
            h.device.bind_buffer_memory(buffer, memory, 0).unwrap();
            let base = h
                .device
                .get_buffer_device_address(&vk::BufferDeviceAddressInfo::default().buffer(buffer));
            region.write(0, 41);
            region.write(len / 8 / 4, 100);
            let kernel = Kernel::new(gpu);
            kernel.run_raw(gpu, &[(base, 0, 4096), (base, len / 8, len / 2)]);
            eprintln!(
                "  shader through the buffer's address: {} (want 42), {} (want 101)",
                region.read(1024),
                region.read(len / 2 / 4)
            );
            assert_eq!(region.read(1024), 42);
            assert_eq!(region.read(len / 2 / 4), 101);
            kernel.finish(gpu);

            // Copy bandwidth in both directions.
            let local = non_sparse_buffer(gpu, len, false);
            let needs = h.device.get_buffer_memory_requirements(local);
            let local_memory = h
                .device
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(needs.size)
                        .memory_type_index(
                            local_type(&space.types, needs.memory_type_bits).unwrap(),
                        ),
                    None,
                )
                .unwrap();
            h.device.bind_buffer_memory(local, local_memory, 0).unwrap();
            let pool = h
                .device
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default().queue_family_index(gpu.family),
                    None,
                )
                .unwrap();
            for (name, from, to) in [
                ("host to local", buffer, local),
                ("local to host", local, buffer),
            ] {
                let mut best = f64::MAX;
                for _ in 0..4 {
                    let start = Instant::now();
                    let staged = one_shot(gpu, pool, |c| {
                        h.device.cmd_copy_buffer(
                            c,
                            from,
                            to,
                            &[vk::BufferCopy::default().size(len)],
                        );
                    });
                    submit_and_wait(gpu, pool, staged).unwrap();
                    best = best.min(start.elapsed().as_secs_f64());
                }
                eprintln!("  copy {name}: {:.1} GB/s", len as f64 / best / 1e9);
            }
            h.device.destroy_command_pool(pool, None);
            h.device.destroy_buffer(local, None);
            h.device.free_memory(local_memory, None);
            h.device.destroy_buffer(buffer, None);
            h.device.free_memory(memory, None);
        }
    }
}

struct Block {
    memory: vk::DeviceMemory,
}

fn bind(
    gpu: &TestGpu,
    window: vk::Buffer,
    offset: u64,
    size: u64,
    memory: vk::DeviceMemory,
) -> Result<(), vk::Result> {
    let binds = [vk::SparseMemoryBind::default()
        .resource_offset(offset)
        .size(size)
        .memory(memory)];
    let buffer_binds = [vk::SparseBufferMemoryBindInfo::default()
        .buffer(window)
        .binds(&binds)];
    let info = [vk::BindSparseInfo::default().buffer_binds(&buffer_binds)];
    let device = &gpu.handles.device;
    // SAFETY: test-only Vulkan use; the fence is waited for and destroyed.
    unsafe {
        let fence = device.create_fence(&vk::FenceCreateInfo::default(), None)?;
        let submitted = device.queue_bind_sparse(gpu.handles.queue, &info, fence);
        let waited = submitted.and_then(|()| device.wait_for_fences(&[fence], true, u64::MAX));
        device.destroy_fence(fence, None);
        waited
    }
}

/// Allocates device-local blocks of 1 GiB, each bound into a sparse window and filled by the GPU, past what the
/// card has, checking every earlier block after each step.
#[test]
#[ignore = "measurement"]
fn probe_oversubscribing_device_local_memory() {
    let gibs: u64 = std::env::var("LEONIDA_PROBE_GIB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10)
        .min(10);
    let priority: f32 = std::env::var("LEONIDA_PROBE_PRIORITY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.1);
    let demote = std::env::var("LEONIDA_PROBE_DEMOTE").is_ok();
    let gpus = TestGpu::open_all();
    for gpu in picked(&gpus) {
        let h = &gpu.handles;
        let device = &h.device;
        let extensions = h.budget.extensions();
        eprintln!(
            "{}: budget {} priority {} pageable {}; block priority {priority}, demote {demote}",
            gpu.name(),
            extensions.budget,
            extensions.priority,
            extensions.pageable
        );
        let families = [gpu.family];
        let window = AddressSpace::make_window(h, 0, 16 * GIB, false, &families).unwrap();
        // SAFETY: the physical device came from this instance.
        let types = unsafe { h.instance.get_physical_device_memory_properties(h.physical) };
        let kind = local_type(&types, window.type_bits).expect("a device-local type");
        // SAFETY: test-only Vulkan use; everything made here is destroyed at the end.
        unsafe {
            let staging = device
                .create_buffer(
                    &vk::BufferCreateInfo::default()
                        .size(1 << 20)
                        .usage(vk::BufferUsageFlags::TRANSFER_DST),
                    None,
                )
                .unwrap();
            let needs = device.get_buffer_memory_requirements(staging);
            let host_kind = (0..types.memory_type_count)
                .find(|i| {
                    needs.memory_type_bits & (1 << i) != 0
                        && types.memory_types[*i as usize].property_flags.contains(
                            vk::MemoryPropertyFlags::HOST_VISIBLE
                                | vk::MemoryPropertyFlags::HOST_COHERENT,
                        )
                        && !types.memory_types[*i as usize]
                            .property_flags
                            .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
                })
                .unwrap();
            let staging_memory = device
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(needs.size)
                        .memory_type_index(host_kind),
                    None,
                )
                .unwrap();
            device
                .bind_buffer_memory(staging, staging_memory, 0)
                .unwrap();
            let mapped = device
                .map_memory(
                    staging_memory,
                    0,
                    vk::WHOLE_SIZE,
                    vk::MemoryMapFlags::empty(),
                )
                .unwrap() as *const u32;
            let pool = device
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default().queue_family_index(gpu.family),
                    None,
                )
                .unwrap();
            let pageable = ash::ext::pageable_device_local_memory::Device::new(&h.instance, device);
            let sink = non_sparse_buffer(gpu, 128 << 20, false);
            let sink_needs = device.get_buffer_memory_requirements(sink);
            let sink_memory = device
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(sink_needs.size)
                        .memory_type_index(
                            local_type(&types, sink_needs.memory_type_bits).unwrap(),
                        ),
                    None,
                )
                .unwrap();
            device.bind_buffer_memory(sink, sink_memory, 0).unwrap();
            let tag = |i: u64| 0xA500_0000 | i as u32;
            let mut blocks: Vec<Block> = Vec::new();
            let start_free = free_ram();
            eprintln!(
                "free system memory at the start: {} MB",
                start_free / 1_000_000
            );
            for i in 0..gibs {
                let free = free_ram();
                if free < STOP_BELOW {
                    eprintln!("stopping: free system memory is {} MB", free / 1_000_000);
                    break;
                }
                let t0 = Instant::now();
                let mut flags = vk::MemoryAllocateFlagsInfo::default()
                    .flags(vk::MemoryAllocateFlags::DEVICE_ADDRESS);
                let mut prio = vk::MemoryPriorityAllocateInfoEXT::default().priority(priority);
                let mut info = vk::MemoryAllocateInfo::default()
                    .allocation_size(GIB)
                    .memory_type_index(kind)
                    .push_next(&mut flags);
                if extensions.priority {
                    info = info.push_next(&mut prio);
                }
                let memory = match device.allocate_memory(&info, None) {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!(
                            "block {i}: allocation refused: {e:?}; {}",
                            h.budget.counters().line()
                        );
                        break;
                    }
                };
                let alloc_ms = t0.elapsed().as_secs_f64() * 1e3;
                if let Err(e) = bind(gpu, window.buffer, i * GIB, GIB, memory) {
                    eprintln!("block {i}: bind refused: {e:?}");
                    device.free_memory(memory, None);
                    break;
                }
                blocks.push(Block { memory });
                if demote && extensions.pageable {
                    for (n, b) in blocks.iter().enumerate() {
                        if n + 2 < blocks.len() {
                            (pageable.fp().set_device_memory_priority_ext)(
                                pageable.device(),
                                b.memory,
                                0.0,
                            );
                        }
                    }
                }
                let t1 = Instant::now();
                let staged = one_shot(gpu, pool, |c| {
                    device.cmd_fill_buffer(c, window.buffer, i * GIB, GIB, tag(i));
                });
                let filled = submit_and_wait(gpu, pool, staged);
                let fill_ms = t1.elapsed().as_secs_f64() * 1e3;
                if let Err(e) = filled {
                    eprintln!("block {i}: fill failed: {e:?}");
                    break;
                }
                let t2 = Instant::now();
                let mut wrong = 0;
                for (n, _) in blocks.iter().enumerate() {
                    let n = n as u64;
                    let offsets = [0, GIB / 2, GIB - 1024];
                    let staged = one_shot(gpu, pool, |c| {
                        let copies: Vec<vk::BufferCopy> = offsets
                            .iter()
                            .enumerate()
                            .map(|(k, o)| {
                                vk::BufferCopy::default()
                                    .src_offset(n * GIB + o)
                                    .dst_offset(k as u64 * 1024)
                                    .size(1024)
                            })
                            .collect();
                        device.cmd_copy_buffer(c, window.buffer, staging, &copies);
                    });
                    if submit_and_wait(gpu, pool, staged).is_err() {
                        wrong += 1000;
                        continue;
                    }
                    for w in 0..3 * 256 {
                        if *mapped.add(w) != tag(n) {
                            wrong += 1;
                        }
                    }
                }
                let verify_ms = t2.elapsed().as_secs_f64() * 1e3;
                let counters = h.budget.counters();
                eprintln!(
                    "block {i}: total {} GiB, allocate {alloc_ms:.0} ms, fill {fill_ms:.0} ms, verify all {verify_ms:.0} ms, \
                     wrong words {wrong}, heap usage {} of {} MiB, free system memory {} MB",
                    i + 1,
                    counters.used >> 20,
                    counters.budget >> 20,
                    free_ram() / 1_000_000
                );
                if wrong != 0 {
                    break;
                }
            }
            let speeds = |blocks: &[Block], label: &str| {
                let mut line = String::new();
                for n in 0..blocks.len() as u64 {
                    let mut best = f64::MAX;
                    for _ in 0..3 {
                        let t = Instant::now();
                        let staged = one_shot(gpu, pool, |c| {
                            device.cmd_copy_buffer(
                                c,
                                window.buffer,
                                sink,
                                &[vk::BufferCopy::default()
                                    .src_offset(n * GIB)
                                    .size(128 << 20)],
                            );
                        });
                        submit_and_wait(gpu, pool, staged).unwrap();
                        best = best.min(t.elapsed().as_secs_f64());
                    }
                    line += &format!(" {:.0}", f64::from(128u32 << 20) / best / 1e9);
                }
                eprintln!("read GB/s per block {label}:{line}");
            };
            speeds(&blocks, "after the allocations");
            if blocks.len() as u64 == gibs {
                let i = gibs;
                let mut flags = vk::MemoryAllocateFlagsInfo::default()
                    .flags(vk::MemoryAllocateFlags::DEVICE_ADDRESS);
                let mut prio = vk::MemoryPriorityAllocateInfoEXT::default().priority(1.0);
                let info = vk::MemoryAllocateInfo::default()
                    .allocation_size(GIB)
                    .memory_type_index(kind)
                    .push_next(&mut flags)
                    .push_next(&mut prio);
                match device.allocate_memory(&info, None) {
                    Ok(memory) => {
                        bind(gpu, window.buffer, i * GIB, GIB, memory).unwrap();
                        blocks.push(Block { memory });
                        let staged = one_shot(gpu, pool, |c| {
                            device.cmd_fill_buffer(c, window.buffer, i * GIB, GIB, tag(i));
                        });
                        let t = Instant::now();
                        submit_and_wait(gpu, pool, staged).unwrap();
                        eprintln!(
                            "an extra block at priority 1.0: fill {:.0} ms",
                            t.elapsed().as_secs_f64() * 1e3
                        );
                        speeds(&blocks, "after a priority 1.0 block");
                    }
                    Err(e) => eprintln!("an extra block at priority 1.0 was refused: {e:?}"),
                }
            }
            device.destroy_buffer(sink, None);
            device.free_memory(sink_memory, None);
            device.device_wait_idle().unwrap();
            device.destroy_command_pool(pool, None);
            device.unmap_memory(staging_memory);
            device.destroy_buffer(staging, None);
            device.free_memory(staging_memory, None);
            device.destroy_buffer(window.buffer, None);
            for b in blocks {
                device.free_memory(b.memory, None);
            }
            eprintln!(
                "free system memory at the end: {} MB (start {} MB)",
                free_ram() / 1_000_000,
                start_free / 1_000_000
            );
        }
    }
}

/// Binds memory of each type the sparse window does not list into it anyway, to see what the driver does.
#[test]
#[ignore = "measurement"]
fn probe_binding_other_memory_types_into_the_sparse_window() {
    let gpus = TestGpu::open_all();
    for gpu in picked(&gpus) {
        let h = &gpu.handles;
        let device = &h.device;
        let families = [gpu.family];
        let window = AddressSpace::make_window(h, 0, 1 << 36, false, &families).unwrap();
        // SAFETY: the physical device came from this instance.
        let types = unsafe { h.instance.get_physical_device_memory_properties(h.physical) };
        let len = 4 * 1024 * 1024u64;
        // SAFETY: test-only Vulkan use; everything made here is destroyed.
        unsafe {
            let pool = device
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default().queue_family_index(gpu.family),
                    None,
                )
                .unwrap();
            for kind in 0..types.memory_type_count {
                let flags = types.memory_types[kind as usize].property_flags;
                let memory = match device.allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(len)
                        .memory_type_index(kind),
                    None,
                ) {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!("type {kind} {flags:?}: allocation {e:?}");
                        continue;
                    }
                };
                let bound = bind(gpu, window.buffer, 0, len, memory);
                let mut note = format!("type {kind} {flags:?}: bind {bound:?}");
                if bound.is_ok() {
                    let staged = one_shot(gpu, pool, |c| {
                        device.cmd_fill_buffer(c, window.buffer, 0, len, 0xC0DE0000 | kind);
                    });
                    note += &format!(", fill {:?}", submit_and_wait(gpu, pool, staged));
                    if flags.contains(vk::MemoryPropertyFlags::HOST_VISIBLE) {
                        let p = device
                            .map_memory(memory, 0, len, vk::MemoryMapFlags::empty())
                            .unwrap() as *const u32;
                        note += &format!(", CPU reads {:#x}", *p.add(1000));
                        device.unmap_memory(memory);
                    }
                    let _ = bind(gpu, window.buffer, 0, len, vk::DeviceMemory::null());
                }
                eprintln!("{note}");
                device.free_memory(memory, None);
            }
            device.destroy_command_pool(pool, None);
            device.destroy_buffer(window.buffer, None);
        }
    }
}

/// Imported host memory bound straight into the sparse window: written by the GPU through the window's address
/// and by the CPU, both seen on the other side with no copy.
#[test]
#[ignore = "measurement"]
fn probe_binding_imported_host_memory_into_the_sparse_window() {
    let gpus = TestGpu::open_all();
    for gpu in picked(&gpus) {
        let h = &gpu.handles;
        let device = &h.device;
        let families = [gpu.family];
        let space = AddressSpace::new(
            h.clone(),
            Config {
                base: 0,
                size: Some(1 << 38),
                chunk: 1 << 30,
            },
        )
        .unwrap();
        let external = std::env::var("LEONIDA_PROBE_PLAIN").is_err();
        let window = AddressSpace::make_window(h, 0, 1 << 36, external, &families).unwrap();
        let len = 64 * PAGE;
        let region = Region::new(len, false);
        // SAFETY: the region stays mapped until the end; everything made here is destroyed.
        unsafe {
            let bits = space.host_type_bits(region.address).unwrap();
            eprintln!(
                "{}: window types {:?}, imported types {:?}",
                gpu.name(),
                type_list(window.type_bits),
                type_list(bits)
            );
            let (memory, _) = space
                .import_memory(
                    region.address,
                    len,
                    true,
                    vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_CACHED,
                )
                .unwrap();
            let bound = bind(gpu, window.buffer, 8 * PAGE, len, memory);
            eprintln!(
                "bind imported memory at window offset 0x{:x}: {bound:?}",
                8 * PAGE
            );
            if bound.is_ok() {
                let address = device.get_buffer_device_address(
                    &vk::BufferDeviceAddressInfo::default().buffer(window.buffer),
                );
                region.write(0, 41);
                let kernel = Kernel::new(gpu);
                kernel.run_raw(gpu, &[(address, 8 * PAGE, 8 * PAGE + 4096)]);
                eprintln!(
                    "  GPU read what the CPU wrote and wrote back: {} (want 42)",
                    region.read(1024)
                );
                assert_eq!(region.read(1024), 42);
                region.write(2048, 6);
                kernel.run_raw(gpu, &[(address, 8 * PAGE + 8192, 8 * PAGE + 12288)]);
                eprintln!("  second round: {} (want 7)", region.read(3072));
                assert_eq!(region.read(3072), 7);
                kernel.finish(gpu);
                let _ = bind(gpu, window.buffer, 8 * PAGE, len, vk::DeviceMemory::null());
            }
            device.free_memory(memory, None);
            device.destroy_buffer(window.buffer, None);
        }
    }
}

/// A few GiB of the game's memory spilled (and, for comparison, mirrored): correct at the ends of every chunk, and
/// how fast the GPU reads each. `LEONIDA_PROBE_SPILL_GIB` (default 3) sets the size.
#[test]
#[ignore = "measurement"]
fn probe_a_large_spilled_window() {
    let gib: u64 = std::env::var("LEONIDA_PROBE_SPILL_GIB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3)
        .min(6);
    let gpus = TestGpu::open_all();
    for gpu in picked(&gpus) {
        let h = &gpu.handles;
        let device = &h.device;
        let mut space = AddressSpace::new(
            h.clone(),
            Config {
                base: 0,
                size: Some(1 << 38),
                chunk: 256 << 20,
            },
        )
        .unwrap();
        if free_ram() < STOP_BELOW + 2 * gib * GIB {
            eprintln!(
                "skipped: free system memory is {} MB",
                free_ram() / 1_000_000
            );
            continue;
        }
        let kernel = Kernel::new(gpu);
        // SAFETY: the physical device came from this instance.
        let types = unsafe { h.instance.get_physical_device_memory_properties(h.physical) };
        // SAFETY: test-only Vulkan use; everything made here is destroyed.
        unsafe {
            let sink = non_sparse_buffer(gpu, 256 << 20, false);
            let needs = device.get_buffer_memory_requirements(sink);
            let sink_memory = device
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(needs.size)
                        .memory_type_index(local_type(&types, needs.memory_type_bits).unwrap()),
                    None,
                )
                .unwrap();
            device.bind_buffer_memory(sink, sink_memory, 0).unwrap();
            let pool = device
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default().queue_family_index(gpu.family),
                    None,
                )
                .unwrap();
            for (name, limit) in [("spilled", 0), ("mirrored", u64::MAX / 4)] {
                h.budget.set_mirror_limit(Some(limit));
                let region = Region::new_in(gib * GIB, false, 8 << 30, 1 << 32, 6);
                for n in 0..gib * 4 {
                    region.write(n * (64 << 20) / 4, 1000 + n as u32);
                }
                let t = Instant::now();
                space
                    .map_host(region.address, gib * GIB, SyncPolicy::Always)
                    .unwrap();
                let counters = h.budget.counters();
                eprintln!(
                    "{name}: mapped {gib} GiB in {:.0} ms: mirror local {} MiB, spilled {} MiB; {}",
                    t.elapsed().as_secs_f64() * 1e3,
                    counters.local[Class::Mirror as usize] >> 20,
                    counters.spilled[Class::Mirror as usize] >> 20,
                    counters.line()
                );
                let jobs: Vec<(u64, u64)> = (0..gib * 4)
                    .map(|n| {
                        let at = region.address + n * (64 << 20);
                        (at, at + 4096)
                    })
                    .collect();
                kernel.run_tracked(gpu, &mut space, &jobs);
                for n in 0..gib * 4 {
                    assert_eq!(
                        region.read(n * (64 << 20) / 4 + 1024),
                        1001 + n as u32,
                        "{name} {n}"
                    );
                }
                let (buffer, offset) = space.buffer_at(region.address);
                let mut best = f64::MAX;
                for _ in 0..3 {
                    let t = Instant::now();
                    let staged = one_shot(gpu, pool, |c| {
                        device.cmd_copy_buffer(
                            c,
                            buffer,
                            sink,
                            &[vk::BufferCopy::default()
                                .src_offset(offset + GIB / 2)
                                .size(256 << 20)],
                        );
                    });
                    submit_and_wait(gpu, pool, staged).unwrap();
                    best = best.min(t.elapsed().as_secs_f64());
                }
                eprintln!(
                    "{name}: GPU copy out of the window {:.1} GB/s",
                    (256u64 << 20) as f64 / best / 1e9
                );
                space.unmap(region.address, gib * GIB).unwrap();
                space.reclaim();
            }
            h.budget.set_mirror_limit(None);
            device.destroy_command_pool(pool, None);
            device.destroy_buffer(sink, None);
            device.free_memory(sink_memory, None);
        }
        kernel.finish(gpu);
    }
}
