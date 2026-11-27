//! What replaying a game-like frame costs on the CPU, and the state elisions that keep it low.
//!
//! The benchmark is `#[ignore]`d: `cargo test --release -p xbox-d3d12 --test replay_perf -- --ignored --nocapture`.
//! Every test skips without a Vulkan 1.3 GPU with translated pixel shaders.
#![allow(unsafe_code)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ash::vk;
use shader_container::testutil;
use shader_translate::FloatMode;
use xbox_d3d12::execute::barrier::{
    COPY_SOURCE, NON_PIXEL_SHADER_RESOURCE, PIXEL_SHADER_RESOURCE, RENDER_TARGET,
};
use xbox_d3d12::execute::{
    Arena, Barrier, BindPoint, Command, ImageRef, IndexBufferView, ReplayEnv, ReplayReport,
    Replayer, Resource, TargetView, VertexBufferView, Viewport, VkContext,
};
use xbox_d3d12::pipeline::shaders::{Link, ShaderSource, TranslatedStage};
use xbox_d3d12::pipeline::{
    Bytecode, CodeStore, ComputePsoDesc, DirectTranslation, GraphicsPsoDesc, InputElement,
    Pipeline, PipelineCache, PipelineEnv, PipelineError, PipelineKind, bindless_bindings,
};
use xbox_d3d12::root::{
    DescriptorRange, ParameterKind, RangeType, RootParameter, RootSignature, RootVersion,
    Visibility,
};

const ARENA_BYTES: u64 = 1 << 20;
const CODE_AT: u64 = 0x8000;
const SIZE: u32 = 64;
const FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;

struct TestArena {
    ctx: Arc<VkContext>,
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    ptr: usize,
    address: u64,
    cursor: Mutex<u64>,
}

impl TestArena {
    fn new(ctx: &Arc<VkContext>) -> TestArena {
        let usage = vk::BufferUsageFlags::STORAGE_BUFFER
            | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS
            | vk::BufferUsageFlags::TRANSFER_SRC
            | vk::BufferUsageFlags::TRANSFER_DST
            | vk::BufferUsageFlags::INDEX_BUFFER
            | vk::BufferUsageFlags::VERTEX_BUFFER
            | vk::BufferUsageFlags::INDIRECT_BUFFER;
        // SAFETY: fresh handles from valid create infos; the mapping covers the whole allocation and lives as
        // long as the arena.
        unsafe {
            let d = &ctx.device;
            let buffer = d
                .create_buffer(
                    &vk::BufferCreateInfo::default()
                        .size(ARENA_BYTES)
                        .usage(usage),
                    None,
                )
                .unwrap();
            let needs = d.get_buffer_memory_requirements(buffer);
            let kind = ctx
                .memory_type(
                    needs.memory_type_bits,
                    vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
                )
                .unwrap();
            let mut flags = vk::MemoryAllocateFlagsInfo::default()
                .flags(vk::MemoryAllocateFlags::DEVICE_ADDRESS);
            let memory = d
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(needs.size)
                        .memory_type_index(kind)
                        .push_next(&mut flags),
                    None,
                )
                .unwrap();
            d.bind_buffer_memory(buffer, memory, 0).unwrap();
            let ptr = d
                .map_memory(memory, 0, ARENA_BYTES, vk::MemoryMapFlags::empty())
                .unwrap() as usize;
            std::ptr::write_bytes(ptr as *mut u8, 0, ARENA_BYTES as usize);
            let address =
                d.get_buffer_device_address(&vk::BufferDeviceAddressInfo::default().buffer(buffer));
            TestArena {
                ctx: ctx.clone(),
                buffer,
                memory,
                ptr,
                address,
                cursor: Mutex::new(CODE_AT),
            }
        }
    }

    fn words(&self, va: u64, count: usize) -> Vec<u32> {
        // SAFETY: the range is inside the mapped, host-coherent arena and the GPU work using it has finished.
        unsafe { std::slice::from_raw_parts((self.ptr + va as usize) as *const u32, count) }
            .to_vec()
    }

    fn arena(&self) -> Arena {
        Arena {
            buffer: self.buffer,
            device_address: self.address,
        }
    }
}

impl CodeStore for TestArena {
    fn upload(&self, bytes: &[u8]) -> Result<u64, PipelineError> {
        let mut cursor = self.cursor.lock().unwrap();
        let at = (*cursor).next_multiple_of(256);
        // SAFETY: the copy stays inside the mapped arena.
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                (self.ptr + at as usize) as *mut u8,
                bytes.len(),
            );
        }
        *cursor = at + bytes.len() as u64;
        Ok(at)
    }
}

impl Drop for TestArena {
    fn drop(&mut self) {
        // SAFETY: nothing uses the arena once the test is done.
        unsafe {
            self.ctx.device.unmap_memory(self.memory);
            self.ctx.device.destroy_buffer(self.buffer, None);
            self.ctx.device.free_memory(self.memory, None);
        }
    }
}

/// Serves the tagged graphics stages, and everything else through the direct translation.
struct Source {
    fixed: Vec<(u8, Arc<TranslatedStage>)>,
    direct: DirectTranslation,
}

impl ShaderSource for Source {
    fn translate(
        &self,
        bytecode: &[u8],
        stage: shader_container::Stage,
        link: &Link,
    ) -> Result<Arc<TranslatedStage>, PipelineError> {
        match self.fixed.iter().find(|(tag, _)| bytecode == [*tag]) {
            Some((_, s)) => Ok(s.clone()),
            None => self.direct.translate(bytecode, stage, link),
        }
    }
}

fn lifted(source: &str) -> shader_ir::Function {
    let code = rdna::assemble(source, true).unwrap_or_else(|e| panic!("{e:?}\n{source}"));
    shader_ir::lift(&code, false).unwrap()
}

fn stage(tag: u8, stage: shader_container::Stage, spirv: Vec<u32>) -> TranslatedStage {
    TranslatedStage {
        stage,
        first_half: false,
        spirv,
        wave64: false,
        user_sgprs: 0,
        threads: None,
        interface: None,
        exports: Vec::new(),
        interpolants: Vec::new(),
        vertex_inputs: Vec::new(),
        outputs: vec![shader_translate::PixelOutput {
            target: 0,
            number: shader_translate::images::Number::Float,
        }],
        writes_depth: false,
        mesh: None,
        uses_code_base: false,
        code: vec![0; 16],
        code_start: 0,
        hash: [tag; 32],
    }
}

/// A full-screen triangle whose vertex stage declares two vertex inputs it does not read, so pipelines have two
/// vertex slots to bind.
fn triangle_stages() -> Vec<(u8, Arc<TranslatedStage>)> {
    let vertex = lifted(
        "v_and_b32_e32 v1, 1, v0
         v_lshrrev_b32_e32 v2, 1, v0
         v_and_b32_e32 v2, 1, v2
         v_cvt_f32_u32_e32 v1, v1
         v_cvt_f32_u32_e32 v2, v2
         v_mul_f32_e32 v1, 4.0, v1
         v_mul_f32_e32 v2, 4.0, v2
         v_add_f32_e32 v1, -1.0, v1
         v_add_f32_e32 v2, -1.0, v2
         v_mov_b32_e32 v3, 0
         v_mov_b32_e32 v4, 1.0
         exp pos0 v1, v2, v3, v4 done
         s_endpgm",
    );
    let vertex_options = shader_translate::VertexOptions {
        user_sgprs: 0,
        inputs: Vec::new(),
        float: FloatMode::default(),
    };
    let pixel = lifted(
        "v_mov_b32_e32 v4, 1.0
         v_mov_b32_e32 v5, 0.5
         v_mov_b32_e32 v6, 0
         v_mov_b32_e32 v7, 1.0
         exp mrt0 v4, v5, v6, v7 done vm
         s_endpgm",
    );
    let pixel_options = shader_translate::PixelOptions {
        user_sgprs: 0,
        inputs: 0x2,
        outputs: vec![shader_translate::PixelOutput {
            target: 0,
            number: shader_translate::images::Number::Float,
        }],
        depth: None,
        attribute_locations: Vec::new(),
        float: FloatMode::default(),
    };
    let element = |semantic: &str, register: u32| shader_container::SignatureElement {
        stream: 0,
        semantic: semantic.into(),
        semantic_index: 0,
        system_value: 0,
        component_type: 3,
        register,
        mask: 0xF,
        rw_mask: 0xF,
        min_precision: 0,
    };
    let mut vs = stage(
        1,
        shader_container::Stage::Vertex,
        shader_translate::vertex(&vertex, &vertex_options).unwrap(),
    );
    vs.vertex_inputs = (0..2)
        .map(|register| shader_translate::VertexInput {
            register,
            number: shader_translate::images::Number::Float,
        })
        .collect();
    vs.interface = Some(shader_container::Interface {
        inputs: vec![element("POSITION", 0), element("TEXCOORD", 1)],
        outputs: Vec::new(),
        pipeline: None,
    });
    let ps = stage(
        2,
        shader_container::Stage::Pixel,
        shader_translate::pixel(&pixel, &pixel_options).unwrap(),
    );
    vec![(1, Arc::new(vs)), (2, Arc::new(ps))]
}

fn fssm_compute(source: &str, user_sgprs: u32) -> Bytecode {
    let code = rdna::assemble(source, true).unwrap();
    let mut blob = vec![0u8; 0x228];
    blob[..4].copy_from_slice(b"FSSM");
    blob[4..6].copy_from_slice(&1u16.to_le_bytes());
    blob[0xC0..0xC4].copy_from_slice(&0x200u32.to_le_bytes());
    blob[0xD4..0xD8].copy_from_slice(&0x20u32.to_le_bytes());
    for (at, value) in [
        (0x138, (user_sgprs << 1) | (1 << 7)),
        (0x13C, 64),
        (0x140, 1),
        (0x144, 1),
    ] {
        blob[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    blob.extend_from_slice(&code);
    Bytecode(testutil::container(5, &[blob]))
}

struct Target {
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
    image_ref: ImageRef,
    target: TargetView,
}

struct World {
    graphics: Vec<Arc<Pipeline>>,
    compute: Vec<Arc<Pipeline>>,
    graphics_root: Arc<RootSignature>,
    compute_root: Arc<RootSignature>,
    targets: Vec<Target>,
    arena: Arc<TestArena>,
    env: PipelineEnv,
    bindless: vk::DescriptorSetLayout,
    ctx: Arc<VkContext>,
}

impl Drop for World {
    fn drop(&mut self) {
        // SAFETY: the pipelines and the device layer's objects that use these handles are gone or idle.
        unsafe {
            let d = &self.ctx.device;
            let _ = d.device_wait_idle();
            for t in self.targets.drain(..) {
                d.destroy_image_view(t.view, None);
                d.destroy_image(t.image, None);
                d.free_memory(t.memory, None);
            }
        }
        self.graphics.clear();
        self.compute.clear();
        // SAFETY: the environment holding pipelines built on this layout is dropped with the world, after this.
        unsafe {
            self.ctx
                .device
                .destroy_descriptor_set_layout(self.bindless, None)
        };
    }
}

fn constants(values: u32) -> RootParameter {
    RootParameter {
        kind: ParameterKind::Constants {
            register: 0,
            space: 0,
            values,
        },
        visibility: Visibility::All,
    }
}

fn view_param() -> RootParameter {
    RootParameter {
        kind: ParameterKind::Cbv {
            register: 1,
            space: 0,
            flags: 0,
        },
        visibility: Visibility::All,
    }
}

fn table_param() -> RootParameter {
    RootParameter {
        kind: ParameterKind::Table(vec![DescriptorRange {
            kind: RangeType::Srv,
            count: 8,
            base_register: 0,
            space: 0,
            flags: 0,
            offset: 0xFFFF_FFFF,
        }]),
        visibility: Visibility::All,
    }
}

fn root_with(parameters: Vec<RootParameter>) -> Arc<RootSignature> {
    Arc::new(RootSignature {
        version: RootVersion::V1_1,
        parameters,
        static_samplers: Vec::new(),
        flags: 0,
    })
}

fn input(semantic: &str, slot: u32) -> InputElement {
    InputElement {
        semantic: semantic.into(),
        semantic_index: 0,
        format: 6,
        slot,
        offset: 0xFFFF_FFFF,
        class: 0,
        step_rate: 0,
    }
}

impl World {
    fn new(graphics_pipelines: usize) -> Option<World> {
        let ctx = match VkContext::standalone() {
            Ok(ctx) => Arc::new(ctx),
            Err(error) => {
                eprintln!("skipping: {error}");
                return None;
            }
        };
        if ctx.properties.api_version < vk::API_VERSION_1_3 || !ctx.features.pixel_shaders {
            eprintln!("skipping: no translated pixel shaders on this GPU");
            return None;
        }
        let arena = Arc::new(TestArena::new(&ctx));
        // SAFETY: a plain create call on a live device.
        let bindless = unsafe {
            let bindings = bindless_bindings(4);
            ctx.device
                .create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                    None,
                )
                .unwrap()
        };
        let cache = PipelineCache::open(&ctx, None).unwrap();
        let source = Arc::new(Source {
            fixed: triangle_stages(),
            direct: DirectTranslation::new(FloatMode::default()),
        });
        let env = PipelineEnv::new(ctx.clone(), bindless, cache, source, arena.clone()).unwrap();
        let graphics_root = root_with(vec![constants(4), table_param(), view_param()]);
        let compute_root = root_with(vec![constants(2), view_param()]);
        let graphics = (0..graphics_pipelines)
            .map(|i| {
                let mut desc = GraphicsPsoDesc::new(graphics_root.clone());
                desc.vs = Bytecode(vec![1]);
                desc.ps = Bytecode(vec![2]);
                desc.render_targets = vec![28];
                desc.input_layout = vec![input("POSITION", 0), input("TEXCOORD", 1)];
                desc.raster.cull = 1 + (i as u32 % 3);
                desc.raster.depth_bias = i as i32;
                desc.depth_stencil.depth_enable = false;
                env.create_graphics(&desc).unwrap()
            })
            .collect();
        let compute = (0..2)
            .map(|i| {
                let source = format!(
                    "s_add_u32 s10, s4, s5
                     v_mov_b32_e32 v1, s10
                     v_mov_b32_e32 v2, {i}
                     global_store_dword v2, v1, s[6:7]
                     s_endpgm"
                );
                env.create_compute(&ComputePsoDesc {
                    root: Some(compute_root.clone()),
                    cs: fssm_compute(&source, 8),
                })
                .unwrap()
            })
            .collect();
        let mut world = World {
            graphics,
            compute,
            graphics_root,
            compute_root,
            targets: Vec::new(),
            arena,
            env,
            bindless,
            ctx,
        };
        for _ in 0..6 {
            let target = world.target();
            world.targets.push(target);
        }
        let images: Vec<vk::Image> = world.targets.iter().map(|t| t.image).collect();
        to_general(&world.ctx, &images);
        Some(world)
    }

    fn target(&self) -> Target {
        let d = &self.ctx.device;
        // SAFETY: handles are destroyed in `World::drop` after the replays' fence waits.
        unsafe {
            let image = d
                .create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(FORMAT)
                        .extent(vk::Extent3D {
                            width: SIZE,
                            height: SIZE,
                            depth: 1,
                        })
                        .mip_levels(1)
                        .array_layers(1)
                        .samples(vk::SampleCountFlags::TYPE_1)
                        .usage(
                            vk::ImageUsageFlags::COLOR_ATTACHMENT
                                | vk::ImageUsageFlags::TRANSFER_SRC,
                        ),
                    None,
                )
                .unwrap();
            let needs = d.get_image_memory_requirements(image);
            let kind = self
                .ctx
                .memory_type(
                    needs.memory_type_bits,
                    vk::MemoryPropertyFlags::DEVICE_LOCAL,
                )
                .unwrap();
            let memory = d
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(needs.size)
                        .memory_type_index(kind),
                    None,
                )
                .unwrap();
            d.bind_image_memory(image, memory, 0).unwrap();
            let view = d
                .create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(image)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(FORMAT)
                        .subresource_range(
                            vk::ImageSubresourceRange::default()
                                .aspect_mask(vk::ImageAspectFlags::COLOR)
                                .level_count(1)
                                .layer_count(1),
                        ),
                    None,
                )
                .unwrap();
            Target {
                image,
                memory,
                view,
                image_ref: ImageRef {
                    image,
                    format: FORMAT,
                    aspect: vk::ImageAspectFlags::COLOR,
                    extent: [SIZE, SIZE, 1],
                    mips: 1,
                    layers: 1,
                },
                target: TargetView {
                    view,
                    format: FORMAT,
                    extent: [SIZE, SIZE],
                    samples: vk::SampleCountFlags::TYPE_1,
                },
            }
        }
    }

    fn env(&self) -> ReplayEnv<'_> {
        ReplayEnv {
            arena: self.arena.arena(),
            layout: self.env.layout,
            bindless: None,
            spill: None,
        }
    }
}

/// Moves fresh images out of `UNDEFINED`, the state the resource layer hands images over in.
fn to_general(ctx: &VkContext, images: &[vk::Image]) {
    // SAFETY: handles are made and destroyed here, after the fence wait.
    unsafe {
        let d = &ctx.device;
        let pool = d
            .create_command_pool(
                &vk::CommandPoolCreateInfo::default().queue_family_index(ctx.family),
                None,
            )
            .unwrap();
        let cb = d
            .allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .command_buffer_count(1),
            )
            .unwrap()[0];
        d.begin_command_buffer(cb, &vk::CommandBufferBeginInfo::default())
            .unwrap();
        let barriers: Vec<_> = images
            .iter()
            .map(|image| {
                vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                    .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                    .dst_access_mask(vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE)
                    .old_layout(vk::ImageLayout::UNDEFINED)
                    .new_layout(vk::ImageLayout::GENERAL)
                    .image(*image)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .level_count(1)
                            .layer_count(1),
                    )
            })
            .collect();
        d.cmd_pipeline_barrier2(
            cb,
            &vk::DependencyInfo::default().image_memory_barriers(&barriers),
        );
        d.end_command_buffer(cb).unwrap();
        let buffers = [cb];
        let fence = d
            .create_fence(&vk::FenceCreateInfo::default(), None)
            .unwrap();
        d.queue_submit(
            ctx.queue,
            &[vk::SubmitInfo::default().command_buffers(&buffers)],
            fence,
        )
        .unwrap();
        d.wait_for_fences(&[fence], true, u64::MAX).unwrap();
        d.destroy_fence(fence, None);
        d.destroy_command_pool(pool, None);
    }
}

/// A pool and a command buffer to record into over and over.
struct Recorder {
    null: VkContext,
    ctx: Arc<VkContext>,
    pool: vk::CommandPool,
    cb: vk::CommandBuffer,
}

impl Recorder {
    fn new(ctx: &Arc<VkContext>) -> Recorder {
        let d = &ctx.device;
        // SAFETY: plain create calls on a live device; destroyed in Drop.
        unsafe {
            let pool = d
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default().queue_family_index(ctx.family),
                    None,
                )
                .unwrap();
            let cb = d
                .allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(pool)
                        .command_buffer_count(1),
                )
                .unwrap()[0];
            Recorder {
                null: null_context(ctx),
                ctx: ctx.clone(),
                pool,
                cb,
            }
        }
    }

    /// Records `commands` and returns the time it took, the report, and leaves the buffer ended.
    fn record(&self, world: &World, commands: &[Command]) -> (Duration, ReplayReport) {
        let d = &self.ctx.device;
        // SAFETY: the pool is reset only when no submission uses its buffer.
        unsafe {
            d.reset_command_pool(self.pool, vk::CommandPoolResetFlags::empty())
                .unwrap();
            d.begin_command_buffer(
                self.cb,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .unwrap();
        }
        let start = Instant::now();
        let report = Replayer::new(&self.ctx, world.env(), self.cb)
            .record(commands)
            .unwrap();
        // SAFETY: the buffer is recording.
        unsafe { d.end_command_buffer(self.cb).unwrap() };
        (start.elapsed(), report)
    }

    fn submit(&self) {
        let d = &self.ctx.device;
        // SAFETY: the buffer is ended; the fence wait finishes the work before anything is freed.
        unsafe {
            let fence = d
                .create_fence(&vk::FenceCreateInfo::default(), None)
                .unwrap();
            let buffers = [self.cb];
            d.queue_submit(
                self.ctx.queue,
                &[vk::SubmitInfo::default().command_buffers(&buffers)],
                fence,
            )
            .unwrap();
            d.wait_for_fences(&[fence], true, u64::MAX).unwrap();
            d.destroy_fence(fence, None);
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        // SAFETY: nothing uses the pool any more.
        unsafe { self.ctx.device.destroy_command_pool(self.pool, None) };
    }
}

struct Lcg(u64);

impl Lcg {
    fn next(&mut self, below: u32) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as u32) % below
    }
}

#[derive(Clone, Copy)]
struct Shape {
    passes: usize,
    draws_per_pass: usize,
}

const VERTEX_AT: u64 = 0x10000;
const INDEX_AT: u64 = 0x40000;

fn viewport() -> Viewport {
    Viewport {
        x: 0.0,
        y: 0.0,
        width: SIZE as f32,
        height: SIZE as f32,
        min_depth: 0.0,
        max_depth: 1.0,
    }
}

/// A frame shaped like the game's: render passes of draws with root constants every few draws, a descriptor
/// table every few more, a pipeline about every 20, mesh buffers changing as meshes do, and redundant sets the
/// way a D3D12 title issues them; barriers between passes; a few dispatches.
fn game_frame(world: &World, shape: Shape) -> Vec<Command> {
    let mut rng = Lcg(7);
    let mut out = Vec::new();
    let ts = &world.targets;
    out.push(Command::SetDescriptorHeaps(Default::default()));
    for pass in 0..shape.passes {
        let transitions: Vec<Barrier> = (0..12)
            .map(|k| {
                let t = &ts[(pass + k) % ts.len()];
                let (before, after) = if k % 3 == 0 {
                    (RENDER_TARGET, PIXEL_SHADER_RESOURCE)
                } else if k % 3 == 1 {
                    (PIXEL_SHADER_RESOURCE, RENDER_TARGET)
                } else {
                    (RENDER_TARGET, COPY_SOURCE)
                };
                Barrier::Transition {
                    resource: Resource::Image(t.image_ref),
                    subresource: None,
                    before,
                    after,
                }
            })
            .chain([Barrier::Transition {
                resource: Resource::Buffer,
                subresource: None,
                before: NON_PIXEL_SHADER_RESOURCE,
                after: PIXEL_SHADER_RESOURCE,
            }])
            .collect();
        out.push(Command::Barriers(transitions));
        let t = &ts[pass % ts.len()];
        out.push(Command::SetRenderTargets {
            colors: vec![t.target],
            depth: None,
        });
        out.push(Command::ClearRenderTarget {
            view: t.target,
            color: [0.0; 4],
            rects: Vec::new(),
        });
        out.push(Command::SetViewports(vec![viewport()]));
        out.push(Command::SetScissors(vec![xbox_d3d12::execute::Rect {
            left: 0,
            top: 0,
            right: SIZE as i32,
            bottom: SIZE as i32,
        }]));
        out.push(Command::SetRootSignature {
            bind: BindPoint::Graphics,
            root: world.graphics_root.clone(),
        });
        out.push(Command::SetTopology(4));
        for d in 0..shape.draws_per_pass {
            if d % 20 == 0 {
                let p = world.graphics[rng.next(world.graphics.len() as u32) as usize].clone();
                out.push(Command::SetPipeline(p));
                out.push(Command::SetRootSignature {
                    bind: BindPoint::Graphics,
                    root: world.graphics_root.clone(),
                });
            }
            if d % 3 == 0 {
                out.push(Command::SetRootConstants {
                    bind: BindPoint::Graphics,
                    param: 0,
                    first: 0,
                    values: vec![rng.next(1000), rng.next(1000), 3, 4],
                });
            }
            if d % 4 == 0 {
                out.push(Command::SetRootTable {
                    bind: BindPoint::Graphics,
                    param: 1,
                    handle: 0x1000 + u64::from(rng.next(64)) * 32,
                });
            }
            if d % 7 == 0 {
                out.push(Command::SetRootView {
                    bind: BindPoint::Graphics,
                    param: 2,
                    va: 0x2000 + u64::from(rng.next(256)) * 256,
                });
            }
            if d % 5 == 0 {
                out.push(Command::SetTopology(4));
            }
            let indexed = d % 8 != 7;
            if d % 2 == 0 {
                let mesh = u64::from(rng.next(64)) * 256;
                out.push(Command::SetVertexBuffers {
                    start: 0,
                    views: vec![
                        VertexBufferView {
                            va: VERTEX_AT + mesh,
                            size: 1024,
                            stride: 32,
                        },
                        VertexBufferView {
                            va: VERTEX_AT + 0x4000 + mesh,
                            size: 512,
                            stride: 16,
                        },
                    ],
                });
                out.push(Command::SetIndexBuffer(Some(IndexBufferView {
                    va: INDEX_AT + mesh,
                    size: 512,
                    format: 57,
                })));
            }
            if indexed {
                out.push(Command::DrawIndexed {
                    indices: 36,
                    instances: 1,
                    first_index: 0,
                    base_vertex: 0,
                    first_instance: 0,
                });
            } else {
                out.push(Command::Draw {
                    vertices: 3,
                    instances: 1,
                    first_vertex: 0,
                    first_instance: 0,
                });
            }
        }
        if pass % 5 == 4 {
            out.push(Command::SetRootSignature {
                bind: BindPoint::Compute,
                root: world.compute_root.clone(),
            });
            out.push(Command::SetPipeline(world.compute[pass % 2].clone()));
            for g in 0..2 {
                out.push(Command::SetRootConstants {
                    bind: BindPoint::Compute,
                    param: 0,
                    first: 0,
                    values: vec![g, 2],
                });
                out.push(Command::SetRootView {
                    bind: BindPoint::Compute,
                    param: 1,
                    va: 0x3000,
                });
                out.push(Command::Dispatch([4, 1, 1]));
            }
        }
    }
    out
}

fn median(times: &mut [Duration]) -> Duration {
    times.sort();
    times[times.len() / 2]
}

#[test]
#[ignore = "benchmark: run with --release --ignored --nocapture"]
fn replaying_a_game_frame_costs_microseconds_per_draw() {
    let Some(world) = World::new(8) else { return };
    let shape = Shape {
        passes: 40,
        draws_per_pass: 250,
    };
    let commands = game_frame(&world, shape);
    let recorder = Recorder::new(&world.ctx);
    for _ in 0..5 {
        recorder.record(&world, &commands);
    }
    let mut times = Vec::new();
    let mut report = ReplayReport::default();
    for _ in 0..200 {
        let (t, r) = recorder.record(&world, &commands);
        times.push(t);
        report = r;
    }
    let min = *times.iter().min().unwrap();
    let med = median(&mut times);
    let draws = report.draws as f64;
    let clone_start = Instant::now();
    let copy = commands.clone();
    let clone_time = clone_start.elapsed();
    drop(copy);
    println!(
        "commands {}  draws {}  dispatches {}  barriers {}",
        commands.len(),
        report.draws,
        report.dispatches,
        report.barriers
    );
    println!(
        "record: min {:.2} ms ({:.3} us/draw)  median {:.2} ms ({:.3} us/draw)",
        min.as_secs_f64() * 1e3,
        min.as_secs_f64() * 1e6 / draws,
        med.as_secs_f64() * 1e3,
        med.as_secs_f64() * 1e6 / draws
    );
    println!(
        "cloning the command vector (allocations of building it): {:.2} ms ({:.3} us/draw)",
        clone_time.as_secs_f64() * 1e3,
        clone_time.as_secs_f64() * 1e6 / draws
    );
    println!("{}", floor(&world, shape, &report.stats));
    println!("calls: {:?}", report.stats);
    let variants: [(&str, Keep); 4] = [
        ("without barriers", |c| !matches!(c, Command::Barriers(_))),
        ("without root arguments", |c| {
            !matches!(
                c,
                Command::SetRootConstants { .. }
                    | Command::SetRootTable { .. }
                    | Command::SetRootView { .. }
            )
        }),
        (
            "without pipeline changes",
            |c| !matches!(c, Command::SetPipeline(p) if p.kind != PipelineKind::Compute),
        ),
        ("without clears", |c| {
            !matches!(c, Command::ClearRenderTarget { .. })
        }),
    ];
    for (name, keep) in variants {
        let mut kept: Vec<Command> = commands.iter().filter(|c| keep(c)).cloned().collect();
        if name == "without pipeline changes" {
            kept.insert(0, Command::SetPipeline(world.graphics[0].clone()));
        }
        print_variant(&world, &recorder, name, &kept);
    }
    let mut bare = vec![
        Command::SetRootSignature {
            bind: BindPoint::Graphics,
            root: world.graphics_root.clone(),
        },
        Command::SetPipeline(world.graphics[0].clone()),
        Command::SetRenderTargets {
            colors: vec![world.targets[0].target],
            depth: None,
        },
        Command::SetIndexBuffer(Some(IndexBufferView {
            va: INDEX_AT,
            size: 512,
            format: 57,
        })),
        Command::SetVertexBuffers {
            start: 0,
            views: vec![
                VertexBufferView {
                    va: VERTEX_AT,
                    size: 1024,
                    stride: 32,
                };
                2
            ],
        },
    ];
    bare.extend(
        commands
            .iter()
            .filter(|c| matches!(c, Command::DrawIndexed { .. }))
            .cloned(),
    );
    print_variant(&world, &recorder, "draws only", &bare);
    let null = null_context(&world.ctx);
    let (per_draw, frame) = cpu_only(&world, &null, &commands, 200);
    println!("replay's own work (null device): {frame:.2} ms ({per_draw:.3} us/draw)");
    println!("notes: {:?}", report.notes);
    recorder.submit();
}

extern "system" fn nothing() {}

/// A context whose every Vulkan function does nothing, so recording through it costs only the replay's own work.
fn null_context(real: &VkContext) -> VkContext {
    // SAFETY: each loaded function is an empty `extern "system"` function, called with arguments it ignores; the
    // context is not owned, so dropping it destroys nothing.
    let device = unsafe {
        ash::Device::load_with(|_| nothing as *const std::ffi::c_void, vk::Device::null())
    };
    let features = xbox_d3d12::execute::Features {
        mesh_shaders: false,
        ..real.features
    };
    VkContext::from_parts(
        real.instance.clone(),
        real.physical,
        device,
        vk::Queue::null(),
        real.family,
        features,
    )
}

/// The median of `runs` replays through the null context, in microseconds per draw, and the frame in ms.
fn cpu_only(world: &World, null: &VkContext, commands: &[Command], runs: usize) -> (f64, f64) {
    let draws = commands.iter().filter(|c| c.is_work()).count().max(1);
    let mut times: Vec<Duration> = (0..runs)
        .map(|_| {
            let start = Instant::now();
            Replayer::new(null, world.env(), vk::CommandBuffer::null())
                .record(commands)
                .unwrap();
            start.elapsed()
        })
        .collect();
    let med = median(&mut times);
    (
        med.as_secs_f64() * 1e6 / draws as f64,
        med.as_secs_f64() * 1e3,
    )
}

fn print_variant(world: &World, recorder: &Recorder, name: &str, commands: &[Command]) {
    let draws = commands.iter().filter(|c| c.is_work()).count().max(1);
    let mut times: Vec<Duration> = (0..100)
        .map(|_| recorder.record(world, commands).0)
        .collect();
    let med = median(&mut times);
    let (own, own_frame) = cpu_only(world, &recorder.null, commands, 100);
    println!(
        "  {name}: median {:.2} ms ({:.3} us/draw), own work {own_frame:.2} ms ({own:.3} us/draw)",
        med.as_secs_f64() * 1e3,
        med.as_secs_f64() * 1e6 / draws as f64
    );
}

/// What the driver costs for the same draws with nothing but the draw calls and the minimum state: the floor.
fn floor(world: &World, shape: Shape, stats: &xbox_d3d12::execute::ReplayStats) -> String {
    let recorder = Recorder::new(&world.ctx);
    let d = &world.ctx.device;
    let draws = shape.passes * shape.draws_per_pass;
    let pipeline = world.graphics[0].clone();
    let t = &world.targets[0];
    let mut times = Vec::new();
    for _ in 0..40 {
        // SAFETY: the pool is idle; commands are recorded into its one buffer and never submitted.
        unsafe {
            d.reset_command_pool(recorder.pool, vk::CommandPoolResetFlags::empty())
                .unwrap();
            d.begin_command_buffer(
                recorder.cb,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .unwrap();
            let start = Instant::now();
            let attachment = [vk::RenderingAttachmentInfo::default()
                .image_view(t.view)
                .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .load_op(vk::AttachmentLoadOp::LOAD)
                .store_op(vk::AttachmentStoreOp::STORE)];
            d.cmd_begin_rendering(
                recorder.cb,
                &vk::RenderingInfo::default()
                    .render_area(vk::Rect2D {
                        offset: vk::Offset2D::default(),
                        extent: vk::Extent2D {
                            width: SIZE,
                            height: SIZE,
                        },
                    })
                    .layer_count(1)
                    .color_attachments(&attachment),
            );
            d.cmd_bind_pipeline(
                recorder.cb,
                vk::PipelineBindPoint::GRAPHICS,
                pipeline.handle,
            );
            let words = [7u32; 4];
            let bytes = std::slice::from_raw_parts(words.as_ptr().cast::<u8>(), 8);
            let buffers = [world.arena.buffer; 2];
            let offsets = [0x10000u64, 0x14000];
            let sizes = [1024u64, 512];
            let strides = [32u64, 16];
            let total = draws as u64;
            let due = |count: u32, i: u64| {
                u64::from(count) * (i + 1) / total > u64::from(count) * i / total
            };
            for i in 0..total {
                if due(stats.pipeline_binds, i) {
                    d.cmd_bind_pipeline(
                        recorder.cb,
                        vk::PipelineBindPoint::GRAPHICS,
                        pipeline.handle,
                    );
                }
                if due(stats.push_calls, i) {
                    d.cmd_push_constants(
                        recorder.cb,
                        world.env.layout,
                        vk::ShaderStageFlags::ALL,
                        0,
                        bytes,
                    );
                }
                if due(stats.vertex_binds, i) {
                    d.cmd_bind_vertex_buffers2(
                        recorder.cb,
                        0,
                        &buffers,
                        &offsets,
                        Some(&sizes),
                        Some(&strides),
                    );
                }
                if due(stats.index_binds, i) {
                    d.cmd_bind_index_buffer(
                        recorder.cb,
                        world.arena.buffer,
                        0x40000,
                        vk::IndexType::UINT16,
                    );
                }
                d.cmd_draw_indexed(recorder.cb, 36, 1, 0, 0, 0);
            }
            d.cmd_end_rendering(recorder.cb);
            d.end_command_buffer(recorder.cb).unwrap();
            times.push(start.elapsed());
        }
    }
    let min = *times.iter().min().unwrap();
    format!(
        "floor (the same calls, nothing else): min {:.2} ms ({:.3} us/draw)",
        min.as_secs_f64() * 1e3,
        min.as_secs_f64() * 1e6 / draws as f64
    )
}

const READBACK: u64 = 0x60000;
const OUT_A: u64 = 0x61000;
const OUT_B: u64 = 0x62000;

fn views(va: u64) -> Vec<VertexBufferView> {
    vec![
        VertexBufferView {
            va,
            size: 1024,
            stride: 32,
        },
        VertexBufferView {
            va: va + 0x4000,
            size: 512,
            stride: 16,
        },
    ]
}

/// The state a graphics draw needs, with the first pipeline, the first target and the root bound.
fn draw_setup(world: &World) -> Vec<Command> {
    vec![
        Command::Barriers(vec![Barrier::Transition {
            resource: Resource::Image(world.targets[0].image_ref),
            subresource: None,
            before: 0,
            after: RENDER_TARGET,
        }]),
        Command::SetRootSignature {
            bind: BindPoint::Graphics,
            root: world.graphics_root.clone(),
        },
        Command::SetPipeline(world.graphics[0].clone()),
        Command::SetRenderTargets {
            colors: vec![world.targets[0].target],
            depth: None,
        },
        Command::SetViewports(vec![viewport()]),
        Command::SetTopology(4),
        Command::SetVertexBuffers {
            start: 0,
            views: views(VERTEX_AT),
        },
        Command::SetIndexBuffer(Some(IndexBufferView {
            va: INDEX_AT,
            size: 512,
            format: 57,
        })),
        Command::SetRootConstants {
            bind: BindPoint::Graphics,
            param: 0,
            first: 0,
            values: vec![1, 2, 3, 4],
        },
    ]
}

fn draw() -> Command {
    Command::Draw {
        vertices: 3,
        instances: 1,
        first_vertex: 0,
        first_instance: 0,
    }
}

fn indexed() -> Command {
    Command::DrawIndexed {
        indices: 3,
        instances: 1,
        first_index: 0,
        base_vertex: 0,
        first_instance: 0,
    }
}

fn replay(world: &World, commands: &[Command]) -> ReplayReport {
    xbox_d3d12::execute::run(&world.ctx, world.env(), commands).unwrap()
}

fn readback(world: &World, target: usize) -> Vec<Command> {
    vec![
        Command::Barriers(vec![Barrier::Transition {
            resource: Resource::Image(world.targets[target].image_ref),
            subresource: None,
            before: RENDER_TARGET,
            after: COPY_SOURCE,
        }]),
        Command::CopyImageToBuffer {
            dst: xbox_d3d12::execute::BufferFootprint {
                va: READBACK,
                row_pitch: SIZE * 4,
                slice_rows: 0,
            },
            src: world.targets[target].image_ref,
            sub: xbox_d3d12::execute::Subresource { mip: 0, layer: 0 },
            region: xbox_d3d12::execute::TexelBox {
                origin: [0; 3],
                size: [SIZE, SIZE, 1],
            },
        },
    ]
}

fn all_pixels(world: &World, want: &[u32]) {
    for (i, word) in world
        .arena
        .words(READBACK, (SIZE * SIZE) as usize)
        .iter()
        .enumerate()
    {
        assert!(want.contains(word), "pixel {i}: {word:08x}");
    }
}

const TRIANGLE: [u32; 2] = [0xFF00_80FF, 0xFF00_7FFF];

#[test]
fn state_set_again_to_the_same_value_records_nothing() {
    let Some(world) = World::new(2) else { return };
    let setup = draw_setup(&world);
    let mut commands = setup.clone();
    commands.push(draw());
    commands.push(indexed());
    commands.extend(setup[1..].to_vec());
    commands.push(draw());
    commands.push(indexed());
    commands.extend(setup[1..].to_vec());
    commands.push(indexed());
    let stats = replay(&world, &commands).stats;
    assert_eq!(stats.pipeline_binds, 1);
    assert_eq!(stats.push_calls, 1);
    assert_eq!(stats.vertex_binds, 1);
    assert_eq!(stats.index_binds, 1);
    assert_eq!(stats.topology_sets, 1);
    assert_eq!(stats.viewport_sets, 1);
    assert_eq!(stats.scissor_sets, 1);
    assert_eq!(stats.blend_sets, 1);
    assert_eq!(stats.stencil_sets, 1);
    assert_eq!(stats.rendering_scopes, 1);
}

type Keep = fn(&Command) -> bool;

type Counter = fn(&xbox_d3d12::execute::ReplayStats) -> u32;

#[test]
fn every_state_that_changes_is_recorded_again() {
    let Some(world) = World::new(2) else { return };
    let setup = draw_setup(&world);
    let second_slot = {
        let v = views(VERTEX_AT)[1];
        VertexBufferView { va: v.va + 64, ..v }
    };
    let steps: Vec<(&str, Command, Counter)> = vec![
        (
            "pipeline",
            Command::SetPipeline(world.graphics[1].clone()),
            |s| s.pipeline_binds,
        ),
        (
            "root constants",
            Command::SetRootConstants {
                bind: BindPoint::Graphics,
                param: 0,
                first: 2,
                values: vec![9],
            },
            |s| s.push_calls,
        ),
        (
            "root table",
            Command::SetRootTable {
                bind: BindPoint::Graphics,
                param: 1,
                handle: 0x40,
            },
            |s| s.push_calls,
        ),
        (
            "root view",
            Command::SetRootView {
                bind: BindPoint::Graphics,
                param: 2,
                va: 0x4000,
            },
            |s| s.push_calls,
        ),
        (
            "second vertex buffer",
            Command::SetVertexBuffers {
                start: 1,
                views: vec![second_slot],
            },
            |s| s.vertex_binds,
        ),
        (
            "vertex stride",
            Command::SetVertexBuffers {
                start: 0,
                views: vec![VertexBufferView {
                    va: VERTEX_AT,
                    size: 1024,
                    stride: 48,
                }],
            },
            |s| s.vertex_binds,
        ),
        (
            "index buffer",
            Command::SetIndexBuffer(Some(IndexBufferView {
                va: INDEX_AT + 64,
                size: 512,
                format: 57,
            })),
            |s| s.index_binds,
        ),
        (
            "index format",
            Command::SetIndexBuffer(Some(IndexBufferView {
                va: INDEX_AT,
                size: 512,
                format: 42,
            })),
            |s| s.index_binds,
        ),
        ("topology", Command::SetTopology(5), |s| s.topology_sets),
        (
            "viewport",
            Command::SetViewports(vec![Viewport {
                width: 32.0,
                ..viewport()
            }]),
            |s| s.viewport_sets,
        ),
        (
            "scissor",
            Command::SetScissors(vec![xbox_d3d12::execute::Rect {
                left: 0,
                top: 0,
                right: 8,
                bottom: 8,
            }]),
            |s| s.scissor_sets,
        ),
        ("blend factor", Command::SetBlendFactor([0.5; 4]), |s| {
            s.blend_sets
        }),
        ("stencil reference", Command::SetStencilRef(3), |s| {
            s.stencil_sets
        }),
        (
            "render targets",
            Command::SetRenderTargets {
                colors: vec![world.targets[1].target],
                depth: None,
            },
            |s| s.rendering_scopes,
        ),
    ];
    for (name, change, counter) in steps {
        let mut commands = setup.clone();
        commands.push(indexed());
        let before = counter(&replay(&world, &commands).stats);
        commands.push(change);
        commands.push(indexed());
        let after = counter(&replay(&world, &commands).stats);
        assert_eq!(after, before + 1, "a changed {name} must be recorded once");
    }
}

#[test]
fn a_pipeline_that_comes_back_binds_again_and_only_changed_push_words_are_pushed() {
    let Some(world) = World::new(2) else { return };
    let mut commands = draw_setup(&world);
    commands.extend([
        indexed(),
        Command::SetPipeline(world.graphics[1].clone()),
        indexed(),
        Command::SetPipeline(world.graphics[0].clone()),
        Command::SetRootConstants {
            bind: BindPoint::Graphics,
            param: 0,
            first: 3,
            values: vec![77],
        },
        indexed(),
    ]);
    let stats = replay(&world, &commands).stats;
    assert_eq!(stats.pipeline_binds, 3);
    assert_eq!(stats.push_calls, 2);
    assert_eq!(stats.push_words, 40 + 1, "{stats:?}");
}

#[test]
fn a_dispatch_between_draws_does_not_leave_its_push_constants_for_the_next_draw_or_dispatch() {
    let Some(world) = World::new(2) else { return };
    let mut commands = draw_setup(&world);
    commands.extend([
        Command::SetRootSignature {
            bind: BindPoint::Compute,
            root: world.compute_root.clone(),
        },
        Command::SetPipeline(world.compute[0].clone()),
        Command::SetRootConstants {
            bind: BindPoint::Compute,
            param: 0,
            first: 0,
            values: vec![40, 2],
        },
        Command::SetRootView {
            bind: BindPoint::Compute,
            param: 1,
            va: OUT_A,
        },
        Command::Dispatch([1, 1, 1]),
        draw(),
        Command::SetRootView {
            bind: BindPoint::Compute,
            param: 1,
            va: OUT_B,
        },
        Command::Dispatch([1, 1, 1]),
        draw(),
    ]);
    let stats = replay(&world, &commands).stats;
    assert_eq!(world.arena.words(OUT_A, 1), [42]);
    assert_eq!(world.arena.words(OUT_B, 1), [42]);
    assert_eq!(stats.push_calls, 4, "{stats:?}");
}

#[test]
fn clears_after_the_targets_become_load_operations_and_stay_ordered_before_other_work() {
    let Some(world) = World::new(2) else { return };
    let clear = |color| Command::ClearRenderTarget {
        view: world.targets[0].target,
        color,
        rects: Vec::new(),
    };
    let mut commands = draw_setup(&world);
    commands.push(clear([1.0, 0.0, 0.0, 1.0]));
    commands.push(clear([0.0, 0.0, 1.0, 1.0]));
    commands.push(draw());
    commands.extend(readback(&world, 0));
    let report = replay(&world, &commands);
    assert_eq!(report.stats.rendering_scopes, 1);
    all_pixels(&world, &TRIANGLE);

    let mut commands = draw_setup(&world);
    commands.push(clear([1.0, 0.0, 0.0, 1.0]));
    commands.extend(readback(&world, 0));
    let report = replay(&world, &commands);
    assert_eq!(report.stats.rendering_scopes, 1);
    all_pixels(&world, &[0xFF00_00FF]);

    let mut commands = draw_setup(&world);
    commands.push(draw());
    commands.push(clear([0.0, 1.0, 0.0, 1.0]));
    commands.extend(readback(&world, 0));
    replay(&world, &commands);
    all_pixels(&world, &[0xFF00_FF00]);
}

fn replay_with_graphics(world: &World, tail: Vec<Command>) -> ReplayReport {
    let mut commands = vec![
        Command::SetRootSignature {
            bind: BindPoint::Graphics,
            root: world.graphics_root.clone(),
        },
        Command::SetPipeline(world.graphics[0].clone()),
        Command::SetVertexBuffers {
            start: 0,
            views: views(VERTEX_AT),
        },
    ];
    commands.extend(tail);
    replay(world, &commands)
}

#[test]
fn barriers_of_consecutive_commands_are_one_call_unless_they_touch_one_image_or_a_draw_sits_between()
 {
    let Some(world) = World::new(2) else { return };
    let transition = |i: usize, before, after| {
        Command::Barriers(vec![Barrier::Transition {
            resource: Resource::Image(world.targets[i].image_ref),
            subresource: None,
            before,
            after,
        }])
    };
    let mid = vec![
        transition(0, 0, RENDER_TARGET),
        Command::SetRootConstants {
            bind: BindPoint::Graphics,
            param: 0,
            first: 0,
            values: vec![1],
        },
        transition(1, 0, RENDER_TARGET),
    ];
    assert_eq!(replay_with_graphics(&world, mid).stats.barrier_calls, 1);
    let same = vec![
        transition(0, 0, RENDER_TARGET),
        transition(0, RENDER_TARGET, PIXEL_SHADER_RESOURCE),
    ];
    let report = replay_with_graphics(&world, same);
    assert_eq!(report.stats.barrier_calls, 2);
    assert_eq!(report.barriers, 2);
    let split = vec![
        transition(0, 0, RENDER_TARGET),
        Command::SetRenderTargets {
            colors: vec![world.targets[0].target],
            depth: None,
        },
        draw(),
        transition(1, 0, RENDER_TARGET),
    ];
    assert_eq!(replay_with_graphics(&world, split).stats.barrier_calls, 2);
}

#[test]
fn a_game_frame_replays_with_far_fewer_calls_than_draws() {
    let Some(world) = World::new(8) else { return };
    let commands = game_frame(
        &world,
        Shape {
            passes: 4,
            draws_per_pass: 100,
        },
    );
    let report = replay(&world, &commands);
    assert_eq!(report.draws, 400);
    assert!(report.stats.vertex_binds < 400, "{:?}", report.stats);
    assert!(report.stats.index_binds < 400, "{:?}", report.stats);
    assert!(report.stats.push_calls < 400, "{:?}", report.stats);
    assert_eq!(report.stats.rendering_scopes, 4);
}

fn submit_in_order(world: &World, recorded: &[xbox_d3d12::execute::parallel::Recorded]) {
    let buffers: Vec<vk::CommandBuffer> = recorded.iter().map(|r| r.buffer).collect();
    let d = &world.ctx.device;
    // SAFETY: the buffers are ended; the fence wait finishes the work before anything is freed.
    unsafe {
        let fence = d
            .create_fence(&vk::FenceCreateInfo::default(), None)
            .unwrap();
        d.queue_submit(
            world.ctx.queue,
            &[vk::SubmitInfo::default().command_buffers(&buffers)],
            fence,
        )
        .unwrap();
        d.wait_for_fences(&[fence], true, u64::MAX).unwrap();
        d.destroy_fence(fence, None);
    }
}

/// A list that writes `value` to `OUT_A` and to its own word, fenced by UAV barriers on both sides.
fn writer_list(world: &World, index: u32, value: u32) -> Vec<Command> {
    vec![
        Command::Barriers(vec![Barrier::Uav]),
        Command::SetRootSignature {
            bind: BindPoint::Compute,
            root: world.compute_root.clone(),
        },
        Command::SetPipeline(world.compute[0].clone()),
        Command::SetRootConstants {
            bind: BindPoint::Compute,
            param: 0,
            first: 0,
            values: vec![value, 0],
        },
        Command::SetRootView {
            bind: BindPoint::Compute,
            param: 1,
            va: OUT_A,
        },
        Command::Dispatch([1, 1, 1]),
        Command::Barriers(vec![Barrier::Uav]),
        Command::SetRootView {
            bind: BindPoint::Compute,
            param: 1,
            va: OUT_B + 4 * u64::from(index),
        },
        Command::Dispatch([1, 1, 1]),
        Command::Barriers(vec![Barrier::Uav]),
    ]
}

#[test]
fn lists_recorded_on_several_threads_run_in_submission_order() {
    let Some(world) = World::new(2) else { return };
    let lists: Vec<Vec<Command>> = (0..8).map(|i| writer_list(&world, i, 100 + i)).collect();
    let refs: Vec<&[Command]> = lists.iter().map(|l| l.as_slice()).collect();
    let mut pools = xbox_d3d12::execute::parallel::RecordPools::new(&world.ctx, 4).unwrap();
    for _ in 0..2 {
        let recorded =
            xbox_d3d12::execute::parallel::record_lists(&world.ctx, &mut pools, &refs, &|_| {
                world.env()
            })
            .unwrap();
        assert_eq!(recorded.len(), 8);
        assert!(recorded.iter().all(|r| r.report.dispatches == 2));
        submit_in_order(&world, &recorded);
        assert_eq!(world.arena.words(OUT_A, 1), [107]);
        assert_eq!(
            world.arena.words(OUT_B, 8),
            [100, 101, 102, 103, 104, 105, 106, 107]
        );
        // SAFETY: the submission above finished.
        unsafe { pools.reset().unwrap() };
        // SAFETY: the arena is host-coherent and idle.
        unsafe { std::ptr::write_bytes((world.arena.ptr + OUT_A as usize) as *mut u8, 0, 0x2000) };
    }
}

#[test]
fn a_list_that_fails_to_record_fails_the_batch_and_a_single_worker_records_in_order() {
    let Some(world) = World::new(2) else { return };
    let good = writer_list(&world, 0, 5);
    let bad = vec![Command::Dispatch([1, 1, 1])];
    let refs: Vec<&[Command]> = vec![&good, &bad, &good];
    let mut pools = xbox_d3d12::execute::parallel::RecordPools::new(&world.ctx, 3).unwrap();
    let error = xbox_d3d12::execute::parallel::record_lists(&world.ctx, &mut pools, &refs, &|_| {
        world.env()
    })
    .unwrap_err();
    assert!(matches!(
        error,
        xbox_d3d12::execute::ReplayError::NoPipeline(_)
    ));
    // SAFETY: nothing was submitted.
    unsafe { pools.reset().unwrap() };
    let mut single = xbox_d3d12::execute::parallel::RecordPools::new(&world.ctx, 1).unwrap();
    let refs: Vec<&[Command]> = vec![&good, &good];
    let recorded =
        xbox_d3d12::execute::parallel::record_lists(&world.ctx, &mut single, &refs, &|_| {
            world.env()
        })
        .unwrap();
    assert_eq!(recorded.len(), 2);
    assert_ne!(recorded[0].buffer, recorded[1].buffer);
}

#[test]
#[ignore = "benchmark: run with --release --ignored --nocapture"]
fn recording_eight_lists_in_parallel_beats_recording_them_in_turn() {
    let Some(world) = World::new(8) else { return };
    let lists: Vec<Vec<Command>> = (0..8)
        .map(|_| {
            game_frame(
                &world,
                Shape {
                    passes: 5,
                    draws_per_pass: 250,
                },
            )
        })
        .collect();
    let refs: Vec<&[Command]> = lists.iter().map(|l| l.as_slice()).collect();
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    for workers in [1, 2, 4, 8] {
        let mut pools =
            xbox_d3d12::execute::parallel::RecordPools::new(&world.ctx, workers).unwrap();
        let mut times = Vec::new();
        for _ in 0..30 {
            // SAFETY: nothing is submitted, so no buffer is pending.
            unsafe { pools.reset().unwrap() };
            let start = Instant::now();
            xbox_d3d12::execute::parallel::record_lists(&world.ctx, &mut pools, &refs, &|_| {
                world.env()
            })
            .unwrap();
            times.push(start.elapsed());
        }
        let med = median(&mut times);
        println!(
            "8 lists, {workers} worker(s) on {threads} hardware threads: median {:.2} ms for {} draws",
            med.as_secs_f64() * 1e3,
            8 * 5 * 250
        );
    }
}
