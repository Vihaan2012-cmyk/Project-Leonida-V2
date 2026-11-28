//! What one submission costs through the real executor on the assembled device: a game-shaped frame recorded
//! through the list's vtable, then converted, registered, recorded into Vulkan and run, with the time of each phase
//! of `Assembly::execute`.
//!
//! The benchmark is `#[ignore]`d: `cargo test --release -p xbox-d3d12 --test executor_perf -- --ignored --nocapture`.
//! The tests that are not benchmarks check the caches the executor keeps. Every test skips without a GPU that can
//! host the layer.
#![cfg(windows)]
#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use shader_container::testutil;
use shader_translate::FloatMode;
use xbox_d3d12::assembly::state::ExecTimings;
use xbox_d3d12::assembly::{Assembly, AssemblyConfig, create_assembled_device_on};
use xbox_d3d12::com::{Guid, iid};
use xbox_d3d12::pipeline::shaders::{Link, ShaderSource, TranslatedStage};
use xbox_d3d12::pipeline::{DirectTranslation, PipelineError};
use xbox_d3d12::root::{
    DescriptorRange, ParameterKind, RangeType, RootParameter, RootSignature, RootVersion,
    Visibility,
};

struct Counting;

static ALLOCS: AtomicU64 = AtomicU64::new(0);

// SAFETY: every call is forwarded to the system allocator unchanged.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: as the caller promises.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: as the caller promises.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: as the caller promises.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

type Ptr = *mut c_void;

const SIZE: u32 = 128;
const FORMAT: u32 = 28;
const TARGETS: usize = 6;

static ONE_GPU_TEST_AT_A_TIME: Mutex<()> = Mutex::new(());

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

/// A full-screen triangle whose vertex stage declares two vertex inputs it does not read.
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

fn fssm_compute(source: &str, user_sgprs: u32) -> Vec<u8> {
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
    testutil::container(5, &[blob])
}

unsafe fn slot<F: Copy>(object: Ptr, index: usize) -> F {
    // SAFETY: the object's first word is its vtable; the caller names the right signature.
    unsafe {
        let address = *(*(object as *const *const usize)).add(index);
        std::mem::transmute_copy::<usize, F>(&address)
    }
}

fn guid(g: &Guid) -> [u8; 16] {
    // SAFETY: `Guid` is 16 bytes of plain data.
    unsafe { std::mem::transmute::<Guid, [u8; 16]>(*g) }
}

fn buffer_desc(bytes: u64, flags: u32) -> [u8; 56] {
    let mut d = [0u8; 56];
    let mut put = |at: usize, v: &[u8]| d[at..at + v.len()].copy_from_slice(v);
    put(0, &1u32.to_le_bytes());
    put(16, &bytes.to_le_bytes());
    put(24, &1u32.to_le_bytes());
    put(28, &(1u32 | 1 << 16).to_le_bytes());
    put(36, &1u32.to_le_bytes());
    put(44, &1u32.to_le_bytes());
    put(48, &flags.to_le_bytes());
    d
}

fn texture_desc(width: u32, height: u32, format: u32, flags: u32) -> [u8; 56] {
    let mut d = [0u8; 56];
    let mut put = |at: usize, v: &[u8]| d[at..at + v.len()].copy_from_slice(v);
    put(0, &3u32.to_le_bytes());
    put(16, &u64::from(width).to_le_bytes());
    put(24, &height.to_le_bytes());
    put(28, &(1u32 | 1 << 16).to_le_bytes());
    put(32, &format.to_le_bytes());
    put(36, &1u32.to_le_bytes());
    put(44, &0x101u32.to_le_bytes());
    put(48, &flags.to_le_bytes());
    d
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Viewport([f32; 6]);

#[repr(C)]
#[derive(Clone, Copy)]
struct VertexView {
    location: u64,
    size: u32,
    stride: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct IndexView {
    location: u64,
    size: u32,
    format: u32,
}

#[repr(C, align(8))]
#[derive(Clone, Copy)]
struct RawBarrier([u8; 32]);

fn transition(resource: Ptr, before: u32, after: u32) -> RawBarrier {
    let mut b = [0u8; 32];
    b[8..16].copy_from_slice(&(resource as u64).to_le_bytes());
    b[16..20].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    b[20..24].copy_from_slice(&before.to_le_bytes());
    b[24..28].copy_from_slice(&after.to_le_bytes());
    RawBarrier(b)
}

const RENDER_TARGET: u32 = 0x4;
const PIXEL_SHADER_RESOURCE: u32 = 0x80;

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

/// Everything a frame names: the objects of the device the title made at load time.
struct World {
    asm: Arc<Assembly>,
    device: Ptr,
    list: Ptr,
    allocator: Ptr,
    targets: Vec<Ptr>,
    rtvs: Vec<u64>,
    srv_heap: Ptr,
    srv_start: u64,
    rtv_heap: Ptr,
    graphics_root: Ptr,
    compute_root: Ptr,
    graphics: Vec<Ptr>,
    compute: Vec<Ptr>,
    geometry: u64,
    constants: u64,
    output: u64,
    _guard: MutexGuard<'static, ()>,
}

fn create_resource(device: Ptr, props: u32, desc: &[u8; 56], state: u32) -> Ptr {
    let heap = [props, 0, 0, 1, 1];
    let mut out = std::ptr::null_mut();
    // SAFETY: device slot 27 with the PC argument order.
    unsafe {
        let create: unsafe extern "win64" fn(
            Ptr,
            *const u32,
            u32,
            *const u8,
            u32,
            *const c_void,
            *const u8,
            *mut Ptr,
        ) -> u32 = slot(device, 27);
        assert_eq!(
            create(
                device,
                heap.as_ptr(),
                0,
                desc.as_ptr(),
                state,
                std::ptr::null(),
                guid(&iid::RESOURCE).as_ptr(),
                &mut out
            ),
            0
        );
    }
    out
}

fn gpu_address(resource: Ptr) -> u64 {
    // SAFETY: resource slot 11.
    unsafe {
        let f: unsafe extern "win64" fn(Ptr) -> u64 = slot(resource, 11);
        f(resource)
    }
}

fn root_signature(device: Ptr, parameters: Vec<RootParameter>) -> Ptr {
    let bytes = RootSignature {
        version: RootVersion::V1_1,
        parameters,
        static_samplers: Vec::new(),
        flags: 0,
    }
    .serialize();
    let mut root = std::ptr::null_mut();
    // SAFETY: device slot 16.
    unsafe {
        let create: unsafe extern "win64" fn(
            Ptr,
            u32,
            *const u8,
            usize,
            *const u8,
            *mut Ptr,
        ) -> u32 = slot(device, 16);
        assert_eq!(
            create(
                device,
                0,
                bytes.as_ptr(),
                bytes.len(),
                guid(&iid::ROOT_SIGNATURE).as_ptr(),
                &mut root
            ),
            0
        );
    }
    root
}

fn heap(device: Ptr, kind: u32, count: u32, visible: bool) -> (Ptr, u64, u64) {
    let desc = [kind, count, u32::from(visible), 1];
    let mut heap = std::ptr::null_mut();
    // SAFETY: device slot 14, heap slots 9 and 10.
    unsafe {
        let create: unsafe extern "win64" fn(Ptr, *const u32, *const u8, *mut Ptr) -> u32 =
            slot(device, 14);
        assert_eq!(
            create(
                device,
                desc.as_ptr(),
                guid(&iid::DESCRIPTOR_HEAP).as_ptr(),
                &mut heap
            ),
            0
        );
        let cpu: unsafe extern "win64" fn(Ptr) -> u64 = slot(heap, 9);
        let gpu: unsafe extern "win64" fn(Ptr) -> u64 = slot(heap, 10);
        (heap, cpu(heap), if visible { gpu(heap) } else { 0 })
    }
}

fn graphics_pso(device: Ptr, root: Ptr, variant: u32) -> Ptr {
    static POSITION: &[u8] = b"POSITION\0";
    static TEXCOORD: &[u8] = b"TEXCOORD\0";
    static VS: [u8; 1] = [1];
    static PS: [u8; 1] = [2];
    let mut elements = [0u8; 64];
    for (i, (name, slot)) in [(POSITION, 0u32), (TEXCOORD, 1)].into_iter().enumerate() {
        let at = i * 32;
        elements[at..at + 8].copy_from_slice(&(name.as_ptr() as u64).to_le_bytes());
        elements[at + 12..at + 16].copy_from_slice(&6u32.to_le_bytes());
        elements[at + 16..at + 20].copy_from_slice(&slot.to_le_bytes());
        elements[at + 20..at + 24].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    }
    let mut d = vec![0u8; 656];
    let mut put = |at: usize, v: &[u8]| d[at..at + v.len()].copy_from_slice(v);
    put(0, &(root as u64).to_le_bytes());
    put(8, &(VS.as_ptr() as u64).to_le_bytes());
    put(16, &1u64.to_le_bytes());
    put(24, &(PS.as_ptr() as u64).to_le_bytes());
    put(32, &1u64.to_le_bytes());
    put(448, &u32::MAX.to_le_bytes());
    put(452, &3u32.to_le_bytes());
    put(456, &(1 + variant % 3).to_le_bytes());
    put(464, &variant.to_le_bytes());
    put(476, &1u32.to_le_bytes());
    put(552, &(elements.as_ptr() as u64).to_le_bytes());
    put(560, &2u32.to_le_bytes());
    put(572, &3u32.to_le_bytes());
    put(576, &1u32.to_le_bytes());
    put(580, &FORMAT.to_le_bytes());
    put(616, &1u32.to_le_bytes());
    let mut pipeline = std::ptr::null_mut();
    // SAFETY: device slot 10; the description's pointers outlive the call.
    unsafe {
        let create: unsafe extern "win64" fn(Ptr, *const u8, *const u8, *mut Ptr) -> u32 =
            slot(device, 10);
        assert_eq!(
            create(
                device,
                d.as_ptr(),
                guid(&iid::PIPELINE_STATE).as_ptr(),
                &mut pipeline
            ),
            0
        );
    }
    pipeline
}

fn compute_pso(device: Ptr, root: Ptr, variant: u32) -> Ptr {
    let source = format!(
        "s_add_u32 s10, s4, s5
         v_mov_b32_e32 v1, s10
         v_mov_b32_e32 v2, {variant}
         global_store_dword v2, v1, s[6:7]
         s_endpgm"
    );
    let container = fssm_compute(&source, 8);
    let mut desc = [0u8; 56];
    desc[..8].copy_from_slice(&(root as u64).to_le_bytes());
    desc[8..16].copy_from_slice(&(container.as_ptr() as u64).to_le_bytes());
    desc[16..24].copy_from_slice(&(container.len() as u64).to_le_bytes());
    let mut pipeline = std::ptr::null_mut();
    // SAFETY: device slot 11.
    unsafe {
        let create: unsafe extern "win64" fn(Ptr, *const u8, *const u8, *mut Ptr) -> u32 =
            slot(device, 11);
        assert_eq!(
            create(
                device,
                desc.as_ptr(),
                guid(&iid::PIPELINE_STATE).as_ptr(),
                &mut pipeline
            ),
            0
        );
    }
    pipeline
}

fn world() -> Option<World> {
    let guard = ONE_GPU_TEST_AT_A_TIME
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    static ASSEMBLY: OnceLock<Option<Arc<Assembly>>> = OnceLock::new();
    let asm = ASSEMBLY
        .get_or_init(|| {
            let source = Arc::new(Source {
                fixed: triangle_stages(),
                direct: DirectTranslation::new(FloatMode::default()),
            });
            match Assembly::build_with_source(&AssemblyConfig::offscreen(64, 64), source, |env| env)
            {
                Ok(asm) => Some(asm),
                Err(e) => {
                    eprintln!("skipped: {e}");
                    None
                }
            }
        })
        .clone()?;
    let params = [0u32; 24];
    let mut device = std::ptr::null_mut();
    // SAFETY: valid pointers for each argument.
    let result = unsafe {
        create_assembled_device_on(&asm, params.as_ptr(), [7u8; 16].as_ptr(), &mut device)
    };
    assert_eq!(result, 0);
    let (mut allocator, mut list) = (std::ptr::null_mut(), std::ptr::null_mut());
    // SAFETY: device slots 9 and 73.
    unsafe {
        let create_allocator: unsafe extern "win64" fn(Ptr, u32, *const u8, *mut Ptr) -> u32 =
            slot(device, 9);
        assert_eq!(
            create_allocator(
                device,
                0,
                guid(&iid::COMMAND_ALLOCATOR).as_ptr(),
                &mut allocator
            ),
            0
        );
        let create_list: unsafe extern "win64" fn(
            Ptr,
            *const u32,
            Ptr,
            Ptr,
            *const u8,
            *mut Ptr,
        ) -> u32 = slot(device, 73);
        let desc = [0u32; 4];
        assert_eq!(
            create_list(
                device,
                desc.as_ptr(),
                allocator,
                std::ptr::null_mut(),
                guid(&iid::GRAPHICS_COMMAND_LIST).as_ptr(),
                &mut list
            ),
            0
        );
    }
    let targets: Vec<Ptr> = (0..TARGETS)
        .map(|_| create_resource(device, 1, &texture_desc(SIZE, SIZE, FORMAT, 1), 0))
        .collect();
    let (rtv_heap, rtv_cpu, _) = heap(device, 2, TARGETS as u32, false);
    let (srv_heap, srv_cpu, srv_start) = heap(device, 0, 64, true);
    let mut rtvs = Vec::new();
    // SAFETY: device slots 18 and 20 write 32 bytes at handles inside the heaps above.
    unsafe {
        let create_rtv: unsafe extern "win64" fn(Ptr, Ptr, *const u8, u64) = slot(device, 20);
        let create_srv: unsafe extern "win64" fn(Ptr, Ptr, *const u8, u64) = slot(device, 18);
        for (i, t) in targets.iter().enumerate() {
            let handle = rtv_cpu + 32 * i as u64;
            create_rtv(device, *t, std::ptr::null(), handle);
            rtvs.push(handle);
        }
        for i in 0..64u64 {
            create_srv(
                device,
                targets[i as usize % TARGETS],
                std::ptr::null(),
                srv_cpu + 32 * i,
            );
        }
    }
    let graphics_root = root_signature(
        device,
        vec![
            RootParameter {
                kind: ParameterKind::Constants {
                    register: 0,
                    space: 0,
                    values: 4,
                },
                visibility: Visibility::All,
            },
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
            },
            RootParameter {
                kind: ParameterKind::Cbv {
                    register: 1,
                    space: 0,
                    flags: 0,
                },
                visibility: Visibility::All,
            },
        ],
    );
    let compute_root = root_signature(
        device,
        vec![
            RootParameter {
                kind: ParameterKind::Constants {
                    register: 0,
                    space: 0,
                    values: 2,
                },
                visibility: Visibility::All,
            },
            RootParameter {
                kind: ParameterKind::Uav {
                    register: 0,
                    space: 0,
                    flags: 0,
                },
                visibility: Visibility::All,
            },
        ],
    );
    let graphics: Vec<Ptr> = (0..8)
        .map(|i| graphics_pso(device, graphics_root, i))
        .collect();
    let compute: Vec<Ptr> = (0..2)
        .map(|i| compute_pso(device, compute_root, i))
        .collect();
    assert!(
        asm.pipeline_service
            .wait_idle(std::time::Duration::from_secs(120))
    );
    let geometry = gpu_address(create_resource(device, 1, &buffer_desc(1 << 20, 0), 0));
    let constants = gpu_address(create_resource(device, 2, &buffer_desc(1 << 18, 0), 0));
    let output = gpu_address(create_resource(device, 1, &buffer_desc(1 << 16, 4), 0));
    Some(World {
        asm,
        device,
        list,
        allocator,
        targets,
        rtvs,
        srv_heap,
        srv_start,
        rtv_heap,
        graphics_root,
        compute_root,
        graphics,
        compute,
        geometry,
        constants,
        output,
        _guard: guard,
    })
}

#[derive(Clone, Copy)]
struct Shape {
    passes: usize,
    draws_per_pass: usize,
}

impl World {
    /// The calls of a frame shaped like the game's: a few render passes, each opened by a barrier call, its
    /// target and viewport, then draws with root constants every few draws, a descriptor table every few more, a
    /// pipeline about every 20, mesh buffers changing as meshes do and the redundant sets a D3D12 title issues; a
    /// few dispatches that write a buffer. The title also rewrites a part of its constant upload buffer.
    fn record(&self, shape: Shape) {
        let list = self.list;
        let mut rng = Lcg(7);
        // SAFETY: the constant buffer is committed host memory of 256 KiB at its address.
        unsafe {
            let words =
                std::slice::from_raw_parts_mut(self.constants as usize as *mut u32, 1 << 14);
            for (i, w) in words.iter_mut().enumerate() {
                *w = rng.next(1000) + i as u32;
            }
        }
        // SAFETY: list slots with the signatures of the PC interface.
        unsafe {
            let barrier: unsafe extern "win64" fn(Ptr, u32, *const RawBarrier) = slot(list, 26);
            let set_targets: unsafe extern "win64" fn(Ptr, u32, *const u64, u32, *const u64) =
                slot(list, 50);
            let clear: unsafe extern "win64" fn(Ptr, u64, *const [f32; 4], u32, *const c_void) =
                slot(list, 52);
            let viewports: unsafe extern "win64" fn(Ptr, u32, *const Viewport) = slot(list, 21);
            let scissors: unsafe extern "win64" fn(Ptr, u32, *const [i32; 4]) = slot(list, 22);
            let topology: unsafe extern "win64" fn(Ptr, u32) = slot(list, 20);
            let constants: unsafe extern "win64" fn(Ptr, u32, u32, *const u32, u32) =
                slot(list, 40);
            let table: unsafe extern "win64" fn(Ptr, u32, u64) = slot(list, 36);
            let cbv: unsafe extern "win64" fn(Ptr, u32, u64) = slot(list, 42);
            let vertex: unsafe extern "win64" fn(Ptr, u32, u32, *const VertexView) = slot(list, 48);
            let index: unsafe extern "win64" fn(Ptr, *const IndexView) = slot(list, 47);
            let draw_indexed: unsafe extern "win64" fn(Ptr, u32, u32, u32, i32, u32) =
                slot(list, 13);
            let draw: unsafe extern "win64" fn(Ptr, u32, u32, u32, u32) = slot(list, 12);
            let dispatch: unsafe extern "win64" fn(Ptr, u32, u32, u32) = slot(list, 14);
            let heaps: unsafe extern "win64" fn(Ptr, u32, *const Ptr) = slot(list, 32);
            let graphics_root: unsafe extern "win64" fn(Ptr, Ptr) = slot(list, 34);
            let compute_root: unsafe extern "win64" fn(Ptr, Ptr) = slot(list, 33);
            let set_pipeline: unsafe extern "win64" fn(Ptr, Ptr) = slot(list, 25);
            let compute_constants: unsafe extern "win64" fn(Ptr, u32, u32, *const u32, u32) =
                slot(list, 39);
            let compute_uav: unsafe extern "win64" fn(Ptr, u32, u64) = slot(list, 45);
            heaps(list, 1, &self.srv_heap);
            for pass in 0..shape.passes {
                let target = pass % TARGETS;
                let mut transitions = vec![transition(
                    self.targets[target],
                    if pass < TARGETS {
                        0
                    } else {
                        PIXEL_SHADER_RESOURCE
                    },
                    RENDER_TARGET,
                )];
                if pass > 0 {
                    transitions.push(transition(
                        self.targets[(pass - 1) % TARGETS],
                        RENDER_TARGET,
                        PIXEL_SHADER_RESOURCE,
                    ));
                }
                barrier(list, transitions.len() as u32, transitions.as_ptr());
                set_targets(list, 1, &self.rtvs[target], 0, std::ptr::null());
                clear(list, self.rtvs[target], &[0.0; 4], 0, std::ptr::null());
                viewports(
                    list,
                    1,
                    &Viewport([0.0, 0.0, SIZE as f32, SIZE as f32, 0.0, 1.0]),
                );
                scissors(list, 1, &[0, 0, SIZE as i32, SIZE as i32]);
                graphics_root(list, self.graphics_root);
                topology(list, 4);
                for d in 0..shape.draws_per_pass {
                    if d % 20 == 0 {
                        set_pipeline(list, self.graphics[rng.next(8) as usize]);
                    }
                    if d % 3 == 0 {
                        let values = [rng.next(1000), rng.next(1000), 3, 4];
                        constants(list, 0, 4, values.as_ptr(), 0);
                    }
                    if d % 4 == 0 {
                        table(list, 1, self.srv_start + u64::from(rng.next(56)) * 32);
                    }
                    if d % 7 == 0 {
                        cbv(list, 2, self.constants + u64::from(rng.next(256)) * 256);
                    }
                    if d % 5 == 0 {
                        topology(list, 4);
                    }
                    if d % 2 == 0 {
                        let mesh = u64::from(rng.next(64)) * 256;
                        let views = [
                            VertexView {
                                location: self.geometry + mesh,
                                size: 1024,
                                stride: 32,
                            },
                            VertexView {
                                location: self.geometry + 0x4000 + mesh,
                                size: 512,
                                stride: 16,
                            },
                        ];
                        vertex(list, 0, 2, views.as_ptr());
                        index(
                            list,
                            &IndexView {
                                location: self.geometry + 0x40000 + mesh,
                                size: 512,
                                format: 57,
                            },
                        );
                    }
                    if d % 8 != 7 {
                        draw_indexed(list, 36, 1, 0, 0, 0);
                    } else {
                        draw(list, 3, 1, 0, 0);
                    }
                }
                if pass % 5 == 4 {
                    compute_root(list, self.compute_root);
                    set_pipeline(list, self.compute[pass % 2]);
                    for g in 0..2u32 {
                        compute_constants(list, 0, 2, [g, 2].as_ptr(), 0);
                        compute_uav(list, 1, self.output + 256 * u64::from(g));
                        dispatch(list, 4, 1, 1);
                    }
                }
            }
            let close: unsafe extern "win64" fn(Ptr) -> u32 = slot(list, 9);
            assert_eq!(close(list), 0);
        }
    }

    fn reset(&self) {
        // SAFETY: list slot 10.
        unsafe {
            let reset: unsafe extern "win64" fn(Ptr, Ptr, Ptr) -> u32 = slot(self.list, 10);
            assert_eq!(reset(self.list, self.allocator, std::ptr::null_mut()), 0);
        }
    }

    fn stream(&self) -> Arc<xbox_d3d12::stream::CommandStream> {
        // SAFETY: the list is one of this layer's, closed.
        unsafe { xbox_d3d12::list::closed_stream(self.list) }.expect("a closed list")
    }

    fn timings(&self) -> ExecTimings {
        self.asm
            .exec
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .timings
            .take()
    }
}

fn median(times: &mut [Duration]) -> Duration {
    times.sort();
    times[times.len() / 2]
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn run_frames(w: &World, shape: Shape, rounds: usize) {
    let draws = (shape.passes * shape.draws_per_pass) as f64;
    let (mut record, mut convert_freed) = (Vec::new(), Vec::new());
    let mut phases: Vec<[Duration; 9]> = Vec::new();
    let mut counts = [0u64; 2];
    w.timings();
    for round in 0..rounds {
        if round > 0 {
            w.reset();
        }
        let a0 = ALLOCS.load(Ordering::Relaxed);
        let t = Instant::now();
        w.record(shape);
        let recorded = t.elapsed();
        let stream = w.stream();
        let a1 = ALLOCS.load(Ordering::Relaxed);
        let t = Instant::now();
        w.asm.execute(&[stream]).expect("the frame runs");
        let executed = t.elapsed();
        let a2 = ALLOCS.load(Ordering::Relaxed);
        let t = w.timings();
        if round >= 10 {
            record.push(recorded);
            convert_freed.push(executed);
            counts = [a1 - a0, a2 - a1];
            phases.push([
                t.prepare,
                t.convert,
                t.bindless,
                t.memory,
                t.record,
                t.submit_wait,
                t.finish,
                t.total,
                executed.saturating_sub(t.total),
            ]);
        }
    }
    println!(
        "frame of {} draws in {} passes",
        shape.passes * shape.draws_per_pass,
        shape.passes
    );
    println!(
        "  list recording through the vtable: median {:.3} ms, {} allocations",
        ms(median(&mut record)),
        counts[0]
    );
    println!(
        "  Assembly::execute: median {:.3} ms ({:.4} us/draw), {} allocations",
        ms(median(&mut convert_freed)),
        ms(median(&mut convert_freed)) * 1e3 / draws,
        counts[1]
    );
    let names = [
        "prepare (pipeline wait, placed textures, written textures)",
        "convert (stream to replay commands)",
        "bindless update",
        "memory (write ranges, dirty hot marking)",
        "record (replay into the command buffer, uploads, downloads)",
        "submit and wait (end, queue submit, GPU, fence)",
        "finish (release detiled, reclaim, budget)",
        "total as the phases add up",
        "outside the phases (stream drop, locks)",
    ];
    for (i, name) in names.iter().enumerate() {
        let mut v: Vec<Duration> = phases.iter().map(|p| p[i]).collect();
        let med = median(&mut v);
        println!(
            "    {name}: median {:.3} ms, min {:.3} ms",
            ms(med),
            ms(v[0])
        );
    }
    let stats = w.asm.exec.lock().unwrap_or_else(|e| e.into_inner());
    drop(stats);
    println!("  notes: {:?}", xbox_d3d12::com::notes());
}

#[test]
#[ignore = "benchmark: run with --release --ignored --nocapture"]
fn a_game_frame_through_the_real_executor() {
    let Some(w) = world() else { return };
    println!(
        "address space backing: {:?}",
        w.asm.resources().space().backing()
    );
    run_frames(
        &w,
        Shape {
            passes: 20,
            draws_per_pass: 500,
        },
        70,
    );
}

#[test]
fn the_frame_shape_executes_without_notes_of_failure() {
    let Some(w) = world() else { return };
    w.record(Shape {
        passes: 7,
        draws_per_pass: 40,
    });
    let stream = w.stream();
    w.asm.execute(&[stream]).expect("the frame runs");
    let t = w.timings();
    assert_eq!(t.submissions, 1);
    let bad: Vec<String> = xbox_d3d12::com::notes()
        .into_iter()
        .filter(|n| n.contains("could not be replayed") || n.contains("failed"))
        .collect();
    assert!(bad.is_empty(), "{bad:?}");
    let _ = (w.rtv_heap, w.device);
}
