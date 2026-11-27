//! Per-frame cost of a mirrored window under game-like traffic. Every test here is `#[ignore]`d; run
//! `cargo test --release -p xbox-d3d12 --lib memory::bench -- --ignored --nocapture --test-threads=1`.
//!
//! `LEONIDA_BENCH_STATIC_MB` (default 2560), `LEONIDA_BENCH_DYNAMIC_MB` (comma list, default `64,256`),
//! `LEONIDA_BENCH_GPU_MB` (default 64), `LEONIDA_BENCH_RENDER_MS` (default 6) and `LEONIDA_BENCH_FRAMES`
//! (default 90) change the traffic.

use std::time::Instant;

use super::testing::*;
use super::*;

const BURN: &[u32] = include!("burn_shader.in");
const GROUPS: u32 = 4096;

fn env_mb(name: &str, default: &str) -> Vec<u64> {
    std::env::var(name)
        .unwrap_or_else(|_| default.to_string())
        .split(',')
        .filter_map(|v| v.trim().parse::<u64>().ok())
        .collect()
}

/// A compute dispatch standing in for a frame's rendering: it keeps the GPU busy for about `render_ms`.
struct Burner {
    module: vk::ShaderModule,
    layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    out: vk::Buffer,
    out_memory: vk::DeviceMemory,
    out_address: u64,
    iters: u32,
}

impl Burner {
    fn new(gpu: &TestGpu) -> Burner {
        let device = &gpu.handles.device;
        // SAFETY: bench-only Vulkan use; `finish` destroys everything.
        unsafe {
            let module = device
                .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(BURN), None)
                .unwrap();
            let range = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::COMPUTE)
                .size(16)];
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
            let size = u64::from(GROUPS) * 256 * 4;
            let out = device
                .create_buffer(
                    &vk::BufferCreateInfo::default().size(size).usage(
                        vk::BufferUsageFlags::STORAGE_BUFFER
                            | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
                    ),
                    None,
                )
                .unwrap();
            let needs = device.get_buffer_memory_requirements(out);
            let types = gpu
                .handles
                .instance
                .get_physical_device_memory_properties(gpu.handles.physical);
            let kind = (0..types.memory_type_count)
                .find(|i| needs.memory_type_bits & (1 << i) != 0)
                .unwrap();
            let mut flags = vk::MemoryAllocateFlagsInfo::default()
                .flags(vk::MemoryAllocateFlags::DEVICE_ADDRESS);
            let out_memory = device
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(needs.size)
                        .memory_type_index(kind)
                        .push_next(&mut flags),
                    None,
                )
                .unwrap();
            device.bind_buffer_memory(out, out_memory, 0).unwrap();
            let out_address = device
                .get_buffer_device_address(&vk::BufferDeviceAddressInfo::default().buffer(out));
            Burner {
                module,
                layout,
                pipeline,
                out,
                out_memory,
                out_address,
                iters: 1,
            }
        }
    }

    fn record(&self, device: &ash::Device, command: vk::CommandBuffer) {
        let mut push = Vec::new();
        push.extend_from_slice(&self.out_address.to_le_bytes());
        push.extend_from_slice(&self.iters.to_le_bytes());
        push.extend_from_slice(&0u32.to_le_bytes());
        // SAFETY: the command buffer is recording.
        unsafe {
            device.cmd_bind_pipeline(command, vk::PipelineBindPoint::COMPUTE, self.pipeline);
            device.cmd_push_constants(
                command,
                self.layout,
                vk::ShaderStageFlags::COMPUTE,
                0,
                &push,
            );
            device.cmd_dispatch(command, GROUPS, 1, 1);
        }
    }

    /// Picks the iteration count that makes one dispatch last about `ms` on this GPU.
    fn calibrate(&mut self, gpu: &TestGpu, ms: f64) {
        if ms <= 0.0 {
            self.iters = 0;
            return;
        }
        let mut iters = 64;
        for _ in 0..8 {
            self.iters = iters;
            let took = self.time(gpu);
            if took > ms * 0.9 && took < ms * 1.1 {
                return;
            }
            iters = ((f64::from(iters) * ms / took.max(0.01)) as u32).max(1);
        }
    }

    fn time(&self, gpu: &TestGpu) -> f64 {
        let rig = Rig::new(gpu, 1);
        let start = Instant::now();
        for _ in 0..3 {
            let command = rig.slots[0].command;
            rig.begin(gpu, 0);
            self.record(&gpu.handles.device, command);
            rig.submit(gpu, 0, &[], &[]);
            rig.wait(gpu, 0);
        }
        let ms = start.elapsed().as_secs_f64() * 1000.0 / 3.0;
        rig.finish(gpu);
        ms
    }

    fn finish(self, gpu: &TestGpu) {
        let device = &gpu.handles.device;
        // SAFETY: nothing is in flight.
        unsafe {
            device.destroy_buffer(self.out, None);
            device.free_memory(self.out_memory, None);
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_shader_module(self.module, None);
        }
    }
}

struct Slot {
    command: vk::CommandBuffer,
    fence: vk::Fence,
    busy: std::cell::Cell<bool>,
}

/// Command buffers and fences for a few frames in flight on the main queue.
struct Rig {
    pool: vk::CommandPool,
    slots: Vec<Slot>,
}

impl Rig {
    fn new(gpu: &TestGpu, depth: usize) -> Rig {
        let device = &gpu.handles.device;
        // SAFETY: bench-only Vulkan use; `finish` destroys everything.
        unsafe {
            let pool = device
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default()
                        .queue_family_index(gpu.family)
                        .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                    None,
                )
                .unwrap();
            let commands = device
                .allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(pool)
                        .command_buffer_count(depth as u32),
                )
                .unwrap();
            let slots = commands
                .into_iter()
                .map(|command| Slot {
                    command,
                    fence: device
                        .create_fence(&vk::FenceCreateInfo::default(), None)
                        .unwrap(),
                    busy: std::cell::Cell::new(false),
                })
                .collect();
            Rig { pool, slots }
        }
    }

    /// Waits until slot `i` is free and starts recording into it.
    fn begin(&self, gpu: &TestGpu, i: usize) {
        self.wait(gpu, i);
        let device = &gpu.handles.device;
        // SAFETY: the slot's last submission has finished.
        unsafe {
            device
                .reset_command_buffer(self.slots[i].command, vk::CommandBufferResetFlags::empty())
                .unwrap();
            device
                .begin_command_buffer(
                    self.slots[i].command,
                    &vk::CommandBufferBeginInfo::default(),
                )
                .unwrap();
        }
    }

    fn wait(&self, gpu: &TestGpu, i: usize) {
        let slot = &self.slots[i];
        if !slot.busy.get() {
            return;
        }
        let device = &gpu.handles.device;
        // SAFETY: the fence belongs to this slot.
        unsafe {
            device
                .wait_for_fences(&[slot.fence], true, u64::MAX)
                .unwrap();
            device.reset_fences(&[slot.fence]).unwrap();
        }
        slot.busy.set(false);
    }

    /// Ends and submits slot `i`, after the semaphores in `waits` (binary, at all-commands).
    fn submit(&self, gpu: &TestGpu, i: usize, waits: &[vk::Semaphore], signals: &[vk::Semaphore]) {
        let device = &gpu.handles.device;
        let slot = &self.slots[i];
        let commands = [slot.command];
        let stages = vec![vk::PipelineStageFlags::ALL_COMMANDS; waits.len()];
        // SAFETY: the command buffer is recording and complete.
        unsafe {
            device.end_command_buffer(slot.command).unwrap();
            device
                .queue_submit(
                    gpu.handles.queue,
                    &[vk::SubmitInfo::default()
                        .command_buffers(&commands)
                        .wait_semaphores(waits)
                        .wait_dst_stage_mask(&stages)
                        .signal_semaphores(signals)],
                    slot.fence,
                )
                .unwrap();
        }
        slot.busy.set(true);
    }

    fn idle(&self, gpu: &TestGpu) {
        for i in 0..self.slots.len() {
            self.wait(gpu, i);
        }
    }

    fn finish(self, gpu: &TestGpu) {
        let device = &gpu.handles.device;
        // SAFETY: nothing is in flight once `idle` returned.
        unsafe {
            for slot in &self.slots {
                device.destroy_fence(slot.fence, None);
            }
            device.destroy_command_pool(self.pool, None);
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Traffic {
    static_mb: u64,
    dynamic_mb: u64,
    gpu_mb: u64,
    frames: usize,
    /// The CPU reads the GPU-written buffer every this many frames (`0`: never).
    read_every: usize,
}

#[derive(Default, Debug)]
struct Report {
    load_ms: f64,
    find_ms: f64,
    frame_ms: f64,
    uploaded_mb: f64,
    downloaded_mb: f64,
}

/// What each frame calls to get its uploads and downloads recorded.
trait Strategy {
    fn name(&self) -> String;
    /// Records the frame's uploads into `command`, returns the bytes copied.
    fn uploads(&mut self, space: &mut AddressSpace, command: vk::CommandBuffer) -> u64;
    /// Records the downloads of the GPU-written buffer at `va..va + len`; `read` says the CPU reads it this frame.
    fn downloads(
        &mut self,
        space: &mut AddressSpace,
        command: vk::CommandBuffer,
        va: u64,
        len: u64,
        read: bool,
    ) -> u64;
}

struct Naive {
    /// Downloads the GPU-written buffer after every frame, as a caller with no knowledge of reads must.
    every_frame: bool,
}

impl Strategy for Naive {
    fn name(&self) -> String {
        format!(
            "record_uploads, downloads {}",
            if self.every_frame {
                "every frame"
            } else {
                "when read"
            }
        )
    }

    fn uploads(&mut self, space: &mut AddressSpace, command: vk::CommandBuffer) -> u64 {
        space.record_uploads(command)
    }

    fn downloads(
        &mut self,
        space: &mut AddressSpace,
        command: vk::CommandBuffer,
        va: u64,
        len: u64,
        read: bool,
    ) -> u64 {
        if self.every_frame || read {
            space.record_downloads(command, va, len)
        } else {
            0
        }
    }
}

fn run_traffic(
    gpu: &TestGpu,
    space: &mut AddressSpace,
    burner: &Burner,
    traffic: Traffic,
    policy: SyncPolicy,
    strategy: &mut dyn Strategy,
) -> Report {
    let watch = policy == SyncPolicy::WriteWatch;
    let mb = 1u64 << 20;
    let high = 4u64 << 30;
    let statics = Region::new_in(traffic.static_mb * mb, watch, high, 8 << 30, 20);
    let dynamics = Region::new_in(
        traffic.dynamic_mb * mb,
        watch,
        high + (160 << 30),
        8 << 30,
        8,
    );
    let written = Region::new_in(traffic.gpu_mb * mb, watch, high + (96 << 30), 8 << 30, 8);
    let device = &gpu.handles.device;
    let mut report = Report::default();
    let rig = Rig::new(gpu, 2);

    space.begin_batch();
    for region in [&statics, &dynamics, &written] {
        space.map_host(region.address, region.len, policy).unwrap();
    }
    space.end_batch().unwrap();

    let touch_all = |region: &Region, frame: u32| {
        for offset in (0..region.len).step_by(4096) {
            region.touch(offset, frame);
        }
    };
    touch_all(&statics, 1);

    let (window_buffer, written_offset) = space.buffer_at(written.address);
    let mut find = 0.0;
    let mut up = 0u64;
    let mut down = 0u64;
    let mut measured = 0usize;
    let mut started = Instant::now();
    let warm = 3;
    for frame in 0..traffic.frames + warm {
        if frame == warm {
            rig.idle(gpu);
            started = Instant::now();
        }
        let slot = frame % 2;
        touch_all(&dynamics, frame as u32 + 2);
        let begun = Instant::now();
        rig.begin(gpu, slot);
        let command = rig.slots[slot].command;
        let before = Instant::now();
        let uploaded = strategy.uploads(space, command);
        let found = before.elapsed().as_secs_f64() * 1000.0;
        burner.record(device, command);
        // SAFETY: recording; the range is inside the window.
        unsafe {
            device.cmd_fill_buffer(
                command,
                window_buffer,
                written_offset,
                written.len,
                frame as u32,
            );
        }
        let read = traffic.read_every > 0 && frame % traffic.read_every == 0;
        let downloaded = strategy.downloads(space, command, written.address, written.len, read);
        rig.submit(gpu, slot, &[], &[]);
        if frame == 0 {
            rig.idle(gpu);
            report.load_ms = begun.elapsed().as_secs_f64() * 1000.0;
            report.uploaded_mb = uploaded as f64 / mb as f64;
        }
        if frame >= warm {
            find += found;
            up += uploaded;
            down += downloaded;
            measured += 1;
        }
    }
    rig.idle(gpu);
    report.frame_ms = started.elapsed().as_secs_f64() * 1000.0 / measured as f64;
    report.find_ms = find / measured as f64;
    report.uploaded_mb = up as f64 / measured as f64 / mb as f64;
    report.downloaded_mb = down as f64 / measured as f64 / mb as f64;
    rig.finish(gpu);
    for region in [&statics, &dynamics, &written] {
        space.unmap(region.address, region.len).unwrap();
    }
    space.reclaim();
    report
}

fn print_header(gpu: &TestGpu, space: &AddressSpace) {
    eprintln!("== {} ({:?})", gpu.name(), space.backing());
    eprintln!(
        "{:<44} {:>6} {:>5} {:>9} {:>9} {:>9} {:>8} {:>8}",
        "strategy / traffic", "dyn", "ms", "load ms", "find ms", "frame ms", "up MB", "down MB"
    );
}

fn table(
    gpu: &TestGpu,
    make: &dyn Fn(&TestGpu) -> AddressSpace,
    strategies: &mut [Box<dyn Strategy>],
    policies: &[SyncPolicy],
    read_every: usize,
) {
    let statics = env_mb("LEONIDA_BENCH_STATIC_MB", "2560")[0];
    let gpu_mb = env_mb("LEONIDA_BENCH_GPU_MB", "64")[0];
    let render = env_mb("LEONIDA_BENCH_RENDER_MS", "6")[0] as f64;
    let frames = env_mb("LEONIDA_BENCH_FRAMES", "90")[0] as usize;
    let mut burner = Burner::new(gpu);
    burner.calibrate(gpu, render);
    for &policy in policies {
        for dynamic_mb in env_mb("LEONIDA_BENCH_DYNAMIC_MB", "64,256") {
            for strategy in strategies.iter_mut() {
                let mut space = make(gpu);
                if space.backing() == Backing::Direct {
                    eprintln!("skipped: the window is direct on this GPU");
                    drop(space);
                    burner.finish(gpu);
                    return;
                }
                let traffic = Traffic {
                    static_mb: statics,
                    dynamic_mb,
                    gpu_mb,
                    frames,
                    read_every,
                };
                let r = run_traffic(gpu, &mut space, &burner, traffic, policy, strategy.as_mut());
                eprintln!(
                    "{:<44} {:>6} {:>5} {:>9.1} {:>9.3} {:>9.3} {:>8.1} {:>8.1}",
                    format!("{:?}: {}", policy, strategy.name()),
                    dynamic_mb,
                    render,
                    r.load_ms,
                    r.find_ms,
                    r.frame_ms,
                    r.uploaded_mb,
                    r.downloaded_mb
                );
            }
        }
    }
    burner.finish(gpu);
}

fn big_space(gpu: &TestGpu) -> AddressSpace {
    AddressSpace::new(
        gpu.handles.clone(),
        Config {
            base: 0,
            size: Some(1 << 39),
            chunk: 1 << 30,
        },
    )
    .unwrap()
}

#[test]
#[ignore = "benchmark"]
fn bench_the_sync_of_a_mirrored_window() {
    for gpu in &TestGpu::open_all() {
        let probe = big_space(gpu);
        print_header(gpu, &probe);
        drop(probe);
        let mut strategies: Vec<Box<dyn Strategy>> = vec![
            Box::new(Naive { every_frame: true }),
            Box::new(Naive { every_frame: false }),
        ];
        table(
            gpu,
            &big_space,
            &mut strategies,
            &[SyncPolicy::WriteWatch],
            30,
        );
    }
}
