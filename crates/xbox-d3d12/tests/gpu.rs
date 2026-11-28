//! Pipelines and replay on a real GPU: a compute shader from a hand-assembled RDNA program through each bytecode
//! shape, and a draw. Every test skips without a Vulkan 1.3 GPU.
#![allow(unsafe_code)]

use std::sync::{Arc, Mutex};

use ash::vk;
use shader_container::testutil;
use shader_translate::FloatMode;
use smallvec::smallvec;
use xbox_d3d12::execute::{
    self, Arena, BindPoint, Command, ImageRef, ReplayEnv, SpillAllocator, TargetView, VkContext,
    barrier,
};
use xbox_d3d12::pipeline::shaders::{Link, ShaderSource, TranslatedStage};
use xbox_d3d12::pipeline::{
    Bytecode, CodeStore, ComputePsoDesc, DirectTranslation, GraphicsPsoDesc, Pipeline,
    PipelineCache, PipelineEnv, PipelineError, Slot67Parts, bindless_bindings,
};
use xbox_d3d12::root::{
    ParameterKind, RootParameter, RootSignature, RootVersion, UserData, UserStage, Visibility,
};

const ARENA_BYTES: u64 = 1 << 20;
const CODE_AT: u64 = 0x8000;

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

struct Setup {
    ctx: Arc<VkContext>,
    arena: Arc<TestArena>,
    env: PipelineEnv,
    bindless: vk::DescriptorSetLayout,
}

impl Drop for Setup {
    fn drop(&mut self) {
        // SAFETY: the environment holding pipelines built on this layout is dropped before the layout.
        unsafe {
            self.ctx
                .device
                .destroy_descriptor_set_layout(self.bindless, None)
        };
    }
}

fn setup(source: Arc<dyn ShaderSource>, cache: Option<&std::path::Path>) -> Option<Setup> {
    let ctx = match VkContext::standalone() {
        Ok(ctx) => Arc::new(ctx),
        Err(error) => {
            eprintln!("skipping: {error}");
            return None;
        }
    };
    if ctx.properties.api_version < vk::API_VERSION_1_3 {
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
    let cache = PipelineCache::open(&ctx, cache).unwrap();
    let mut env = PipelineEnv::new(ctx.clone(), bindless, cache, source, arena.clone()).unwrap();
    env.note = Arc::new(|text| eprintln!("note: {text}"));
    Some(Setup {
        ctx,
        arena,
        env,
        bindless,
    })
}

fn translation() -> Arc<dyn ShaderSource> {
    Arc::new(DirectTranslation::new(FloatMode::default()))
}

fn replay<'a>(setup: &'a Setup) -> ReplayEnv<'a> {
    ReplayEnv {
        arena: setup.arena.arena(),
        layout: setup.env.layout,
        bindless: None,
        spill: None,
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

fn uav_root() -> Arc<RootSignature> {
    root_with(vec![RootParameter {
        kind: ParameterKind::Uav {
            register: 0,
            space: 0,
            flags: 0,
        },
        visibility: Visibility::All,
    }])
}

/// Group g, thread t stores g * 1000 + t at the root UAV plus 256 g + 4 t.
const STORE_IDS: &str = "v_lshlrev_b32_e32 v1, 2, v0
    s_lshl_b32 s3, s6, 8
    v_add_nc_u32_e32 v1, s3, v1
    v_mad_u32_u24 v2, s6, 1000, v0
    global_store_dword v1, v2, s[4:5]
    s_endpgm";

/// A synthetic FSSM compute blob: wave32, RSRC2 with six user SGPRs and a group id, 64 threads.
fn fssm_compute(source: &str) -> Vec<u8> {
    let code = rdna::assemble(source, true).unwrap();
    let mut blob = vec![0u8; 0x228];
    blob[..4].copy_from_slice(b"FSSM");
    blob[4..6].copy_from_slice(&1u16.to_le_bytes());
    blob[0xC0..0xC4].copy_from_slice(&0x200u32.to_le_bytes());
    blob[0xD4..0xD8].copy_from_slice(&0x20u32.to_le_bytes());
    for (at, value) in [
        (0x138, (6u32 << 1) | (1 << 7)),
        (0x13C, 64),
        (0x140, 1),
        (0x144, 1),
    ] {
        blob[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    blob.extend_from_slice(&code);
    blob
}

fn check_ids(arena: &TestArena, at: u64, groups: u32) {
    let words = arena.words(at, (groups * 64) as usize);
    for group in 0..groups {
        for thread in 0..64 {
            assert_eq!(
                words[(group * 64 + thread) as usize],
                group * 1000 + thread,
                "group {group} thread {thread}"
            );
        }
    }
}

fn dispatch_ids(setup: &Setup, desc: &ComputePsoDesc, at: u64) {
    let pipeline = setup.env.create_compute(desc).unwrap();
    assert_eq!(pipeline.layouts[0].stage, UserStage::Compute);
    let root = pipeline.root.clone();
    let commands = [
        Command::SetRootSignature {
            bind: BindPoint::Compute,
            root,
        },
        Command::SetPipeline(pipeline),
        Command::SetRootView {
            bind: BindPoint::Compute,
            param: 0,
            va: at,
        },
        Command::Dispatch([4, 1, 1]),
    ];
    let report = execute::run(&setup.ctx, replay(setup), &commands).unwrap();
    assert_eq!(report.dispatches, 1);
    check_ids(&setup.arena, at, 4);
}

#[test]
fn a_compute_pipeline_from_an_fssm_container_runs_with_root_arguments() {
    let Some(setup) = setup(translation(), None) else {
        return;
    };
    let container = testutil::container(5, &[fssm_compute(STORE_IDS)]);
    let desc = ComputePsoDesc {
        root: Some(uav_root()),
        cs: Bytecode(container),
    };
    dispatch_ids(&setup, &desc, 0x1000);
}

#[test]
fn a_compute_pipeline_from_the_console_slot_67_parts_runs_the_same_program() {
    let Some(setup) = setup(translation(), None) else {
        return;
    };
    let mut program = vec![0u8; 36];
    program.extend_from_slice(&rdna::assemble(STORE_IDS, true).unwrap());
    let mut state = vec![0u8; 28];
    state[12..16].copy_from_slice(&(64u32 | 1 << 16).to_le_bytes());
    state[16..20].copy_from_slice(&(1u32 | 1 << 20).to_le_bytes());
    state[24..28].copy_from_slice(&((6u32 << 1) | (1 << 7)).to_le_bytes());
    let parts = Slot67Parts {
        dispatch_state: state,
        kind1: vec![0; 20],
        program,
        records: Vec::new(),
    };
    let desc = ComputePsoDesc::from_slot67(Some(uav_root()), &parts).unwrap();
    dispatch_ids(&setup, &desc, 0x2000);
    let bare = Slot67Parts {
        dispatch_state: Vec::new(),
        ..parts
    };
    assert!(matches!(
        ComputePsoDesc::from_slot67(None, &bare),
        Err(PipelineError::Unsupported(_))
    ));
}

#[test]
fn the_pipeline_cache_survives_a_run_and_blobs_name_their_pipeline() {
    let path = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("pipelines.cache");
    let _ = std::fs::remove_file(&path);
    let container = testutil::container(5, &[fssm_compute(STORE_IDS)]);
    let desc = ComputePsoDesc {
        root: Some(uav_root()),
        cs: Bytecode(container),
    };
    {
        let Some(first) = setup(translation(), Some(&path)) else {
            return;
        };
        assert!(!first.env.cache.loaded);
        let pipeline = first.env.create_compute(&desc).unwrap();
        let blob = first.env.cached_blob(&pipeline);
        assert!(first.env.cache.accepts_blob(&blob, &pipeline.key));
        let mut changed = blob.clone();
        changed[20] ^= 1;
        assert!(!first.env.cache.accepts_blob(&changed, &pipeline.key));
        first.env.cache.save().unwrap();
    }
    let second = setup(translation(), Some(&path)).unwrap();
    assert!(second.env.cache.loaded || std::fs::metadata(&path).map_or(0, |m| m.len()) == 0);
    second.env.create_compute(&desc).unwrap();
}

#[test]
fn shaders_of_the_wrong_stage_are_refused_and_spirv_comes_from_the_source() {
    let source = DirectTranslation::new(FloatMode::default());
    let container = testutil::container(0, &[fssm_compute(STORE_IDS)]);
    let error = source
        .translate(
            &container,
            shader_container::Stage::Compute,
            &Link::default(),
        )
        .unwrap_err();
    assert!(matches!(error, PipelineError::WrongStage { .. }), "{error}");
    let ok = testutil::container(5, &[fssm_compute(STORE_IDS)]);
    let stage = source
        .translate(&ok, shader_container::Stage::Compute, &Link::default())
        .unwrap();
    assert_eq!(stage.threads, Some([64, 1, 1]));
    assert_eq!(stage.user_sgprs, 6);
    assert!(!stage.uses_code_base);
    assert_eq!(stage.spirv[0], 0x0723_0203);
}

/// Serves prebuilt stages for graphics bytecode named by a one-byte tag.
struct Fixed(Vec<(u8, Arc<TranslatedStage>)>);

impl ShaderSource for Fixed {
    fn translate(
        &self,
        bytecode: &[u8],
        _stage: shader_container::Stage,
        _link: &Link,
    ) -> Result<Arc<TranslatedStage>, PipelineError> {
        self.0
            .iter()
            .find(|(tag, _)| bytecode == [*tag])
            .map(|(_, s)| s.clone())
            .ok_or_else(|| PipelineError::Bytecode("unknown test shader".into()))
    }
}

fn lifted(source: &str) -> shader_ir::Function {
    let code = rdna::assemble(source, true).unwrap_or_else(|e| panic!("{e:?}\n{source}"));
    shader_ir::lift(&code, false).unwrap()
}

fn stage(tag: u8, stage: shader_container::Stage, spirv: Vec<u32>) -> Arc<TranslatedStage> {
    stage_with_code(tag, stage, spirv, vec![0; 16], false)
}

fn stage_with_code(
    tag: u8,
    stage: shader_container::Stage,
    spirv: Vec<u32>,
    code: Vec<u8>,
    uses_code_base: bool,
) -> Arc<TranslatedStage> {
    Arc::new(TranslatedStage {
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
        uses_code_base,
        code,
        code_start: 0,
        hash: [tag; 32],
    })
}

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
    vec![
        (
            1,
            stage(
                1,
                shader_container::Stage::Vertex,
                shader_translate::vertex(&vertex, &vertex_options).unwrap(),
            ),
        ),
        (
            2,
            stage(
                2,
                shader_container::Stage::Pixel,
                shader_translate::pixel(&pixel, &pixel_options).unwrap(),
            ),
        ),
    ]
}

/// A program that takes its own address and loads the float stored right after its last instruction into s10:
/// `s_getpc_b64` returns the address of the instruction after it. Returns the program with the constant
/// appended, which is where a console shader keeps it.
fn program_with_constant(body: impl Fn(&str) -> String, constant: f32) -> Vec<u8> {
    let assemble = |offset: u32| {
        let source = format!(
            "s_getpc_b64 s[8:9]
             s_load_dword s10, s[8:9], 0x{offset:x}
             s_waitcnt lgkmcnt(0)
             {}",
            body("s10")
        );
        rdna::assemble(&source, true).unwrap_or_else(|e| {
            panic!(
                "{e:?}
{source}"
            )
        })
    };
    let length = assemble(0).len() as u32;
    let mut code = assemble(length - 4);
    assert_eq!(code.len() as u32, length);
    code.extend(constant.to_le_bytes());
    code
}

/// The triangle's vertex shader scaling its corners by a constant read through its own address, and a pixel
/// shader writing a constant read through its own address as green, each from a blob of its own.
fn code_base_stages() -> Vec<(u8, Arc<TranslatedStage>)> {
    let vertex_code = program_with_constant(
        |scale| {
            format!(
                "v_and_b32_e32 v1, 1, v0
                 v_lshrrev_b32_e32 v2, 1, v0
                 v_and_b32_e32 v2, 1, v2
                 v_cvt_f32_u32_e32 v1, v1
                 v_cvt_f32_u32_e32 v2, v2
                 v_mul_f32_e32 v1, {scale}, v1
                 v_mul_f32_e32 v2, {scale}, v2
                 v_add_f32_e32 v1, -1.0, v1
                 v_add_f32_e32 v2, -1.0, v2
                 v_mov_b32_e32 v3, 0
                 v_mov_b32_e32 v4, 1.0
                 exp pos0 v1, v2, v3, v4 done
                 s_endpgm"
            )
        },
        4.0,
    );
    let pixel_code = program_with_constant(
        |green| {
            format!(
                "v_mov_b32_e32 v4, 1.0
                 v_mov_b32_e32 v5, {green}
                 v_mov_b32_e32 v6, 0
                 v_mov_b32_e32 v7, 1.0
                 exp mrt0 v4, v5, v6, v7 done vm
                 s_endpgm"
            )
        },
        0.5,
    );
    let float = FloatMode::default();
    let vertex = shader_translate::vertex(
        &shader_ir::lift(&vertex_code[..vertex_code.len() - 4], false).unwrap(),
        &shader_translate::VertexOptions {
            user_sgprs: 0,
            inputs: Vec::new(),
            float,
        },
    )
    .unwrap();
    let pixel = shader_translate::pixel(
        &shader_ir::lift(&pixel_code[..pixel_code.len() - 4], false).unwrap(),
        &shader_translate::PixelOptions {
            user_sgprs: 0,
            inputs: 0x2,
            outputs: vec![shader_translate::PixelOutput {
                target: 0,
                number: shader_translate::images::Number::Float,
            }],
            depth: None,
            attribute_locations: Vec::new(),
            float,
        },
    )
    .unwrap();
    vec![
        (
            3,
            stage_with_code(
                3,
                shader_container::Stage::Vertex,
                vertex,
                vertex_code,
                true,
            ),
        ),
        (
            4,
            stage_with_code(4, shader_container::Stage::Pixel, pixel, pixel_code, true),
        ),
    ]
}

#[test]
fn vertex_and_pixel_stages_each_read_constants_after_their_own_code() {
    let Some(setup) = setup(Arc::new(Fixed(code_base_stages())), None) else {
        return;
    };
    let Some((pipeline, pixels)) = draw_triangle(&setup, 3, 4) else {
        return;
    };
    let [vertex, pixel] = &pipeline.stages[..] else {
        panic!("{:?}", pipeline.stages);
    };
    assert_ne!(vertex.code_va, pixel.code_va);
    let words = pipeline.push_words(
        &UserData {
            sgprs: [0; 32],
            spill: Vec::new(),
        },
        0,
    );
    let at =
        |index: u32| u64::from(words[index as usize]) | u64::from(words[index as usize + 1]) << 32;
    assert_eq!(at(shader_translate::PUSH_CODE), vertex.code_va);
    assert_eq!(at(shader_translate::PUSH_CODE_PIXEL), pixel.code_va);
    // The vertex shader's 4.0 covers the target and the pixel shader's 0.5 is green; either constant read
    // from the other blob would give 0.0 (nothing there) or clamp to 1.0.
    for (i, word) in pixels.iter().enumerate() {
        assert!(
            *word == 0xFF00_80FF || *word == 0xFF00_7FFF,
            "pixel {i}: {word:08x}"
        );
    }
}

/// Moves a fresh image out of `UNDEFINED`, the state the resource layer hands images over in.
fn to_general(ctx: &VkContext, image: vk::Image) {
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
        let b = [vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            .dst_access_mask(vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::GENERAL)
            .image(image)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .level_count(1)
                    .layer_count(1),
            )];
        d.cmd_pipeline_barrier2(cb, &vk::DependencyInfo::default().image_memory_barriers(&b));
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

const SIZE: u32 = 8;

/// Draws the full-target triangle of the `vertex` and `pixel` test shaders into a cleared 8x8 target and
/// returns the pipeline with the pixels read back.
fn draw_triangle(setup: &Setup, vertex: u8, pixel: u8) -> Option<(Arc<Pipeline>, Vec<u32>)> {
    if !setup.ctx.features.pixel_shaders {
        eprintln!("skipping: no translated pixel shaders on this GPU");
        return None;
    }
    let d = &setup.ctx.device;
    // SAFETY: handles are destroyed at the end of the test after the replay's fence wait.
    let (image, memory, view) = unsafe {
        let image = d
            .create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(vk::Format::R8G8B8A8_UNORM)
                    .extent(vk::Extent3D {
                        width: SIZE,
                        height: SIZE,
                        depth: 1,
                    })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .usage(
                        vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
                    ),
                None,
            )
            .unwrap();
        let needs = d.get_image_memory_requirements(image);
        let kind = setup
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
                    .format(vk::Format::R8G8B8A8_UNORM)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .level_count(1)
                            .layer_count(1),
                    ),
                None,
            )
            .unwrap();
        (image, memory, view)
    };
    to_general(&setup.ctx, image);
    let image_ref = ImageRef {
        image,
        format: vk::Format::R8G8B8A8_UNORM,
        aspect: vk::ImageAspectFlags::COLOR,
        extent: [SIZE, SIZE, 1],
        mips: 1,
        layers: 1,
    };
    let target = TargetView {
        view,
        format: vk::Format::R8G8B8A8_UNORM,
        extent: [SIZE, SIZE],
        samples: vk::SampleCountFlags::TYPE_1,
    };
    let root = root_with(Vec::new());
    let mut desc = GraphicsPsoDesc::new(root.clone());
    desc.vs = Bytecode(vec![vertex]);
    desc.ps = Bytecode(vec![pixel]);
    desc.render_targets = vec![28];
    desc.raster.cull = 1;
    desc.depth_stencil.depth_enable = false;
    let pipeline = match setup.env.create_graphics(&desc) {
        Ok(p) => p,
        Err(error) => panic!("{error}"),
    };
    let readback = 0x4000;
    let transition = |before, after| {
        Command::Barriers(vec![execute::Barrier::Transition {
            resource: execute::Resource::Image(image_ref),
            subresource: None,
            before,
            after,
        }])
    };
    let commands = [
        transition(0, barrier::RENDER_TARGET),
        Command::SetRootSignature {
            bind: BindPoint::Graphics,
            root,
        },
        Command::SetPipeline(pipeline.clone()),
        Command::SetRenderTargets {
            colors: smallvec![target],
            depth: None,
        },
        Command::ClearRenderTarget {
            view: target,
            color: [0.0; 4],
            rects: Default::default(),
        },
        Command::SetTopology(4),
        Command::Draw {
            vertices: 3,
            instances: 1,
            first_vertex: 0,
            first_instance: 0,
        },
        transition(barrier::RENDER_TARGET, barrier::COPY_SOURCE),
        Command::CopyImageToBuffer {
            dst: execute::BufferFootprint {
                va: readback,
                row_pitch: SIZE * 4,
                slice_rows: 0,
            },
            src: image_ref,
            sub: execute::Subresource { mip: 0, layer: 0 },
            region: execute::TexelBox {
                origin: [0; 3],
                size: [SIZE, SIZE, 1],
            },
        },
    ];
    let report = execute::run(&setup.ctx, replay(setup), &commands).unwrap();
    assert_eq!((report.draws, report.copies), (1, 1));
    let pixels = setup.arena.words(readback, (SIZE * SIZE) as usize);
    // SAFETY: the replay has finished with the image.
    unsafe {
        d.destroy_image_view(view, None);
        d.destroy_image(image, None);
        d.free_memory(memory, None);
    }
    Some((pipeline, pixels))
}

#[test]
fn a_draw_replays_through_a_graphics_pipeline_and_fills_the_target() {
    let Some(setup) = setup(Arc::new(Fixed(triangle_stages())), None) else {
        return;
    };
    let Some((_, pixels)) = draw_triangle(&setup, 1, 2) else {
        return;
    };
    for (i, word) in pixels.iter().enumerate() {
        // 0.5 in unorm8 is 127 or 128 depending on the driver's rounding.
        assert!(
            *word == 0xFF00_80FF || *word == 0xFF00_7FFF,
            "pixel {i}: {word:08x}"
        );
    }
}

fn run_commands<'a>(
    setup: &'a Setup,
    spill: Option<&'a mut dyn SpillAllocator>,
    commands: &[Command],
) {
    let env = ReplayEnv {
        spill,
        ..replay(setup)
    };
    execute::run(&setup.ctx, env, commands).unwrap();
}

fn constants_param(values: u32) -> RootParameter {
    RootParameter {
        kind: ParameterKind::Constants {
            register: 0,
            space: 0,
            values,
        },
        visibility: Visibility::All,
    }
}

fn compute_container(source: &str, user_sgprs: u32) -> Bytecode {
    let mut blob = fssm_compute(source);
    blob[0x138..0x13C].copy_from_slice(&(user_sgprs << 1).to_le_bytes());
    Bytecode(testutil::container(5, &[blob]))
}

#[test]
fn root_constants_an_indirect_dispatch_fills_and_copies_run_in_order() {
    let Some(setup) = setup(translation(), None) else {
        return;
    };
    // Two constants at s4 and s5, a root UAV at s6: one thread stores their sum.
    let root = root_with(vec![
        constants_param(2),
        RootParameter {
            kind: ParameterKind::Uav {
                register: 0,
                space: 0,
                flags: 0,
            },
            visibility: Visibility::All,
        },
    ]);
    let desc = ComputePsoDesc {
        root: Some(root.clone()),
        cs: compute_container(
            "s_add_u32 s10, s4, s5
             v_mov_b32_e32 v1, s10
             v_mov_b32_e32 v2, 0
             global_store_dword v2, v1, s[6:7]
             s_endpgm",
            8,
        ),
    };
    let pipeline = setup.env.create_compute(&desc).unwrap();
    let (args, out, copy) = (0x3000u64, 0x3100u64, 0x3200u64);
    // The indirect arguments: one group of 64 threads, written by a fill and a copy of the constants below.
    let commands = [
        Command::FillBuffer {
            va: args,
            bytes: 12,
            value: 1,
        },
        Command::FillBuffer {
            va: out,
            bytes: 16,
            value: 0xDEAD_BEEF,
        },
        Command::Barriers(vec![execute::Barrier::Uav]),
        Command::SetRootSignature {
            bind: BindPoint::Compute,
            root,
        },
        Command::SetPipeline(pipeline),
        Command::SetRootConstants {
            bind: BindPoint::Compute,
            param: 0,
            first: 0,
            values: smallvec![40, 2],
        },
        Command::SetRootView {
            bind: BindPoint::Compute,
            param: 1,
            va: out,
        },
        Command::ExecuteIndirect {
            kind: execute::IndirectKind::Dispatch,
            max_count: 1,
            args,
            stride: 12,
            count: None,
        },
        Command::Barriers(vec![execute::Barrier::Uav]),
        Command::CopyBuffer {
            dst: copy,
            src: out,
            bytes: 16,
        },
    ];
    run_commands(&setup, None, &commands);
    assert_eq!(
        setup.arena.words(out, 4),
        [42, 0xDEAD_BEEF, 0xDEAD_BEEF, 0xDEAD_BEEF]
    );
    assert_eq!(
        setup.arena.words(copy, 4),
        [42, 0xDEAD_BEEF, 0xDEAD_BEEF, 0xDEAD_BEEF]
    );
}

struct Spill {
    arena: Arc<TestArena>,
    next: u64,
}

impl SpillAllocator for Spill {
    fn store(&mut self, bytes: &[u8]) -> Option<u64> {
        let at = self.next;
        self.next += 256;
        // SAFETY: the copy stays inside the mapped arena.
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                (self.arena.ptr + at as usize) as *mut u8,
                bytes.len(),
            );
        }
        Some(at)
    }
}

#[test]
fn a_root_argument_that_spills_reaches_the_shader_through_the_pointer_in_s0() {
    let Some(setup) = setup(translation(), None) else {
        return;
    };
    // 27 constants fill s4 to s30, so the UAV spills; the shader reads its address from the array at s[0:1].
    let root = root_with(vec![constants_param(27), {
        let mut uav = uav_root().parameters[0].clone();
        uav.visibility = Visibility::All;
        uav
    }]);
    let desc = ComputePsoDesc {
        root: Some(root.clone()),
        cs: compute_container(
            "s_load_dwordx2 s[40:41], s[0:1], 0x0
             s_waitcnt lgkmcnt(0)
             v_mov_b32_e32 v1, s4
             v_mov_b32_e32 v2, 0
             global_store_dword v2, v1, s[40:41]
             s_endpgm",
            31,
        ),
    };
    let pipeline = setup.env.create_compute(&desc).unwrap();
    assert!(pipeline.layouts[0].spills());
    let out = 0x5000;
    let commands = [
        Command::SetRootSignature {
            bind: BindPoint::Compute,
            root,
        },
        Command::SetPipeline(pipeline),
        Command::SetRootConstants {
            bind: BindPoint::Compute,
            param: 0,
            first: 0,
            values: smallvec![77],
        },
        Command::SetRootView {
            bind: BindPoint::Compute,
            param: 1,
            va: out,
        },
        Command::Dispatch([1, 1, 1]),
    ];
    let mut spill = Spill {
        arena: setup.arena.clone(),
        next: 0x6000,
    };
    run_commands(&setup, Some(&mut spill), &commands);
    assert_eq!(setup.arena.words(out, 1), [77]);
    // Without anywhere to put the array the replay says so rather than running with a bad pointer.
    let env = replay(&setup);
    let error = execute::run(&setup.ctx, env, &commands).unwrap_err();
    assert!(matches!(error, execute::ReplayError::NoSpillSpace));
}
