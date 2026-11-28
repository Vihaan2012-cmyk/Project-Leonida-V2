//! Pipelines made through the assembled device's slots 62 and 86 are built on worker threads: the slot returns
//! at once, a draw waits for the build or is dropped by the draw policy, and a second start finds the pipelines
//! the first one logged already built. The builder is held shut by a gate where a test needs a slow build.
//! Every test skips, with a message, when the machine has no GPU that can host the layer.
#![cfg(windows)]
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use shader_container::testutil;
use xbox_d3d12::assembly::objects::pipeline_handle_of;
use xbox_d3d12::assembly::pipelines::{Handle, PipelinesSeen, SharedBuilder};
use xbox_d3d12::assembly::{Assembly, AssemblyConfig, create_assembled_device_on};
use xbox_d3d12::com::{Guid, iid};
use xbox_d3d12::compile_queue::{DrawPolicy, PipelineBuilder, PsoKey};
use xbox_d3d12::pipeline::{ComputePsoDesc, GraphicsPsoDesc, Pipeline, PipelineEnv, PipelineError};
use xbox_d3d12::root::{ParameterKind, RootParameter, RootSignature, RootVersion, Visibility};

type Ptr = *mut c_void;

const LONG: Duration = Duration::from_secs(60);

/// Builds are held here until a test opens the gate; `started` counts the builds that reached it.
struct Gate {
    open: Mutex<bool>,
    changed: Condvar,
    started: AtomicUsize,
}

static GATE: Gate = Gate {
    open: Mutex::new(true),
    changed: Condvar::new(),
    started: AtomicUsize::new(0),
};

impl Gate {
    fn set(&self, open: bool) {
        *self.open.lock().unwrap_or_else(|e| e.into_inner()) = open;
        self.changed.notify_all();
    }

    fn pass(&self) {
        self.started.fetch_add(1, Ordering::SeqCst);
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        while !*open {
            open = self.changed.wait(open).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn wait_for_starts(&self, count: usize) {
        let from = Instant::now();
        while self.started.load(Ordering::SeqCst) < count {
            assert!(from.elapsed() < LONG, "no build reached the gate");
            std::thread::yield_now();
        }
    }
}

/// Closes the gate for a test and opens it again however the test ends.
struct Closed;

impl Closed {
    fn new() -> Closed {
        GATE.set(false);
        Closed
    }
}

impl Drop for Closed {
    fn drop(&mut self) {
        GATE.set(true);
    }
}

/// The real environment behind the gate.
struct Slow(Arc<PipelineEnv>);

impl PipelineBuilder for Slow {
    type Built = Arc<Pipeline>;

    fn graphics(&self, desc: &GraphicsPsoDesc) -> Result<Arc<Pipeline>, PipelineError> {
        GATE.pass();
        PipelineBuilder::graphics(&*self.0, desc)
    }

    fn compute(&self, desc: &ComputePsoDesc) -> Result<Arc<Pipeline>, PipelineError> {
        GATE.pass();
        PipelineBuilder::compute(&*self.0, desc)
    }

    fn cached_blob(&self, key: &PsoKey) -> Vec<u8> {
        PipelineBuilder::cached_blob(&*self.0, key)
    }

    fn save_cache(&self) -> Result<(), PipelineError> {
        PipelineBuilder::save_cache(&*self.0)
    }
}

fn scratch(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-tmp/pipeline-device")
        .join(name)
}

/// The paths the first and the second start share.
fn seen() -> PipelinesSeen {
    PipelinesSeen::beside(scratch("seen").join("pipelines-seen"))
}

fn cache_file() -> PathBuf {
    scratch("seen").join("pipelines.cache")
}

fn config() -> AssemblyConfig {
    AssemblyConfig {
        pipeline_cache: Some(cache_file()),
        pipelines_seen: Some(seen()),
        pipeline_workers: Some(2),
        ..AssemblyConfig::offscreen(256, 256)
    }
}

struct Started {
    asm: Arc<Assembly>,
    device: usize,
}

fn device_on(asm: &Arc<Assembly>) -> usize {
    let unknown_iid = [7u8; 16];
    let params = [0u32; 24];
    let mut device = std::ptr::null_mut();
    // SAFETY: valid pointers for each argument.
    let result = unsafe {
        create_assembled_device_on(asm, params.as_ptr(), unknown_iid.as_ptr(), &mut device)
    };
    assert_eq!(result, 0);
    device as usize
}

static ONE_GPU_TEST_AT_A_TIME: Mutex<()> = Mutex::new(());

struct Harness {
    asm: Arc<Assembly>,
    device: Ptr,
    _guard: MutexGuard<'static, ()>,
}

/// The first start of the device, behind the gate. One for the whole file: each holds a window-sized sparse buffer.
fn harness() -> Option<Harness> {
    let guard = ONE_GPU_TEST_AT_A_TIME
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    static FIRST: OnceLock<Option<Started>> = OnceLock::new();
    let first = FIRST
        .get_or_init(|| {
            let _ = std::fs::remove_dir_all(scratch("seen"));
            let built = Assembly::build_with(&config(), |env| {
                let slow: SharedBuilder = Arc::new(Slow(env));
                slow
            });
            match built {
                Ok(asm) => {
                    let device = device_on(&asm);
                    Some(Started { asm, device })
                }
                Err(e) => {
                    eprintln!("skipped: {e}");
                    None
                }
            }
        })
        .as_ref()?;
    Some(Harness {
        asm: first.asm.clone(),
        device: first.device as Ptr,
        _guard: guard,
    })
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

/// A compute shader that stores `group * multiplier + thread` for every thread; each test has its own multiplier so
/// that its pipeline is its own.
fn ids_program(multiplier: u32) -> Vec<u8> {
    let source = format!(
        "v_lshlrev_b32_e32 v1, 2, v0
    s_lshl_b32 s3, s6, 8
    v_add_nc_u32_e32 v1, s3, v1
    v_mad_u32_u24 v2, s6, {multiplier}, v0
    global_store_dword v1, v2, s[4:5]
    s_endpgm"
    );
    let code = rdna::assemble(&source, true).unwrap();
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
    testutil::container(5, &[blob])
}

fn uav_root(device: Ptr) -> Ptr {
    let signature = RootSignature {
        version: RootVersion::V1_1,
        parameters: vec![RootParameter {
            kind: ParameterKind::Uav {
                register: 0,
                space: 0,
                flags: 0,
            },
            visibility: Visibility::All,
        }],
        static_samplers: Vec::new(),
        flags: 0,
    }
    .serialize();
    let mut root = std::ptr::null_mut();
    // SAFETY: device slot 16.
    unsafe {
        let create_root: unsafe extern "win64" fn(
            Ptr,
            u32,
            *const u8,
            usize,
            *const u8,
            *mut Ptr,
        ) -> u32 = slot(device, 16);
        assert_eq!(
            create_root(
                device,
                0,
                signature.as_ptr(),
                signature.len(),
                guid(&iid::ROOT_SIGNATURE).as_ptr(),
                &mut root
            ),
            0
        );
    }
    root
}

#[derive(Clone, Copy)]
enum Creation {
    Slot62,
    Slot86,
}

/// Creates the compute pipeline of `program` through slot 62 or the compute stream of slot 86.
fn create(device: Ptr, root: Ptr, program: &[u8], creation: Creation) -> (u32, Ptr) {
    let mut pipeline = std::ptr::null_mut();
    let riid = guid(&iid::PIPELINE_STATE);
    // SAFETY: slots 62 and 86 with the argument orders the title uses; the description outlives the call.
    let made = unsafe {
        match creation {
            Creation::Slot62 => {
                let mut desc = [0u8; 56];
                desc[..8].copy_from_slice(&(root as u64).to_le_bytes());
                desc[8..16].copy_from_slice(&(program.as_ptr() as u64).to_le_bytes());
                desc[16..24].copy_from_slice(&(program.len() as u64).to_le_bytes());
                let create: unsafe extern "win64" fn(
                    Ptr,
                    *const u8,
                    usize,
                    usize,
                    *const u8,
                    *mut Ptr,
                ) -> u32 = slot(device, 62);
                create(device, desc.as_ptr(), 0, 0, riid.as_ptr(), &mut pipeline)
            }
            Creation::Slot86 => {
                let mut stream = [0u8; 40];
                stream[8..16].copy_from_slice(&(root as u64).to_le_bytes());
                stream[16..20].copy_from_slice(&6u32.to_le_bytes());
                stream[24..32].copy_from_slice(&(program.as_ptr() as u64).to_le_bytes());
                stream[32..40].copy_from_slice(&(program.len() as u64).to_le_bytes());
                let wrapper = [stream.len() as u64, stream.as_ptr() as u64];
                let create: unsafe extern "win64" fn(Ptr, *const u64, *const u8, *mut Ptr) -> u32 =
                    slot(device, 86);
                create(device, wrapper.as_ptr(), riid.as_ptr(), &mut pipeline)
            }
        }
    };
    (made, pipeline)
}

fn handle(pipeline: Ptr) -> Handle {
    // SAFETY: a pipeline state object of this layer.
    unsafe { pipeline_handle_of(pipeline) }.expect("a pipeline state object")
}

fn queue(device: Ptr) -> Ptr {
    let desc = [0u32, 0, 0, 0];
    let mut queue = std::ptr::null_mut();
    // SAFETY: slot 8 takes `(desc, riid, out)`.
    let result = unsafe {
        let create: unsafe extern "win64" fn(Ptr, *const u32, *const u8, *mut Ptr) -> u32 =
            slot(device, 8);
        create(
            device,
            desc.as_ptr(),
            guid(&iid::COMMAND_QUEUE).as_ptr(),
            &mut queue,
        )
    };
    assert_eq!(result, 0);
    queue
}

/// A closed list that sets the root signature and pipeline, binds `address` as the UAV and dispatches 4 groups.
fn dispatch_list(device: Ptr, root: Ptr, pipeline: Ptr, address: u64) -> Ptr {
    let (mut allocator, mut list) = (std::ptr::null_mut(), std::ptr::null_mut());
    // SAFETY: device slots 9 and 73, list slots 33, 25, 45, 14 and 9.
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
        let set_root: unsafe extern "win64" fn(Ptr, Ptr) = slot(list, 33);
        set_root(list, root);
        let set_pipeline: unsafe extern "win64" fn(Ptr, Ptr) = slot(list, 25);
        set_pipeline(list, pipeline);
        let set_uav: unsafe extern "win64" fn(Ptr, u32, u64) = slot(list, 45);
        set_uav(list, 0, address);
        let dispatch: unsafe extern "win64" fn(Ptr, u32, u32, u32) = slot(list, 14);
        dispatch(list, 4, 1, 1);
        let close: unsafe extern "win64" fn(Ptr) -> u32 = slot(list, 9);
        assert_eq!(close(list), 0);
    }
    list
}

/// Executes the closed list and waits on a fence the queue signals behind it.
fn run(device: usize, queue: usize, list: usize) {
    let (device, queue, list) = (device as Ptr, queue as Ptr, list as Ptr);
    let mut fence = std::ptr::null_mut();
    // SAFETY: device slot 36, queue slots 35 and 16, fence slot 9.
    unsafe {
        let create_fence: unsafe extern "win64" fn(Ptr, u64, u32, *const u8, *mut Ptr) -> u32 =
            slot(device, 36);
        assert_eq!(
            create_fence(device, 0, 0, guid(&iid::FENCE).as_ptr(), &mut fence),
            0
        );
        let execute: unsafe extern "win64" fn(Ptr, u32, *const Ptr, u64, u32) -> u32 =
            slot(queue, 35);
        assert_eq!(execute(queue, 1, &list, 0, 0), 0);
        let signal: unsafe extern "win64" fn(Ptr, Ptr, u64) -> u32 = slot(queue, 16);
        assert_eq!(signal(queue, fence, 1), 0);
        let wait: unsafe extern "win64" fn(Ptr, u64, Ptr) -> u32 = slot(fence, 9);
        assert_eq!(wait(fence, 1, std::ptr::null_mut()), 0);
    }
}

/// A zeroed committed buffer for the dispatch to write and the address of its memory.
struct Output {
    address: u64,
}

fn output(device: Ptr) -> Output {
    let heap = [1u32, 0, 0, 1, 1];
    let desc = buffer_desc(0x10000, 4);
    let mut buffer = std::ptr::null_mut();
    // SAFETY: slot 27 with the PC argument order, resource slot 11.
    let address = unsafe {
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
                8,
                std::ptr::null(),
                guid(&iid::RESOURCE).as_ptr(),
                &mut buffer
            ),
            0
        );
        let gpu_address: unsafe extern "win64" fn(Ptr) -> u64 = slot(buffer, 11);
        gpu_address(buffer)
    };
    let out = Output { address };
    out.clear();
    out
}

impl Output {
    fn words(&self) -> &[u32] {
        // SAFETY: the buffer is committed host memory at its address, and nothing runs on the GPU while a test reads.
        unsafe { std::slice::from_raw_parts(self.address as usize as *const u32, 4 * 64) }
    }

    fn clear(&self) {
        // SAFETY: as `words`.
        unsafe { std::ptr::write_bytes(self.address as usize as *mut u32, 0, 4 * 64) };
    }

    fn holds_ids(&self, multiplier: u32) -> bool {
        self.words()
            .iter()
            .enumerate()
            .all(|(n, w)| *w == (n as u32 / 64) * multiplier + n as u32 % 64)
    }
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let from = Instant::now();
    while !done() {
        assert!(from.elapsed() < LONG, "timed out waiting for {what}");
        std::thread::yield_now();
    }
}

#[test]
fn slot_62_returns_before_the_pipeline_is_built() {
    let Some(h) = harness() else { return };
    let root = uav_root(h.device);
    let program = ids_program(1001);
    let closed = Closed::new();
    let before = GATE.started.load(Ordering::SeqCst);
    let started = Instant::now();
    let (made, pipeline) = create(h.device, root, &program, Creation::Slot62);
    let returned = started.elapsed();
    assert_eq!(made, 0, "notes: {:?}", xbox_d3d12::com::notes());
    assert!(!pipeline.is_null());
    GATE.wait_for_starts(before + 1);
    let handle = handle(pipeline);
    assert!(
        handle.ready().is_none(),
        "the build was held, so the call returned without it (took {returned:?})"
    );
    assert!(
        h.asm.pipeline_service.stats().queue.building >= 1,
        "the build is running on a worker"
    );

    // SAFETY: pipeline slot 8, `GetCachedBlob(out)`.
    let blob = unsafe {
        let mut blob = std::ptr::null_mut();
        let cached_blob: unsafe extern "win64" fn(Ptr, *mut Ptr) -> u32 = slot(pipeline, 8);
        assert_eq!(cached_blob(pipeline, &mut blob), 0);
        blob
    };
    assert!(!blob.is_null());
    assert!(
        handle.ready().is_none(),
        "GetCachedBlob did not wait for the build"
    );
    assert!(handle.cached_blob().is_some_and(|b| !b.is_empty()));

    drop(closed);
    wait_until("the build", || handle.ready().is_some());
}

#[test]
fn slot_86_returns_before_the_pipeline_is_built() {
    let Some(h) = harness() else { return };
    let root = uav_root(h.device);
    let program = ids_program(1002);
    let closed = Closed::new();
    let before = GATE.started.load(Ordering::SeqCst);
    let (made, pipeline) = create(h.device, root, &program, Creation::Slot86);
    assert_eq!(made, 0, "notes: {:?}", xbox_d3d12::com::notes());
    GATE.wait_for_starts(before + 1);
    let handle = handle(pipeline);
    assert!(handle.ready().is_none());
    drop(closed);
    wait_until("the build", || handle.ready().is_some());
    assert_eq!(
        handle.ready().unwrap().kind,
        xbox_d3d12::pipeline::PipelineKind::Compute
    );
}

#[test]
fn a_dispatch_waits_for_its_pipeline_under_the_wait_policy() {
    let Some(h) = harness() else { return };
    h.asm.pipeline_service.set_draw_policy(DrawPolicy::Wait);
    let root = uav_root(h.device);
    let program = ids_program(1003);
    let out = output(h.device);
    let closed = Closed::new();
    let (made, pipeline) = create(h.device, root, &program, Creation::Slot62);
    assert_eq!(made, 0);
    let list = dispatch_list(h.device, root, pipeline, out.address);
    let queue = queue(h.device);
    let before = h.asm.pipeline_service.stats();

    let (device, queue, list) = (h.device as usize, queue as usize, list as usize);
    let (done, finished) = std::sync::mpsc::channel();
    let runner = std::thread::spawn(move || {
        run(device, queue, list);
        done.send(()).unwrap();
    });
    assert!(
        finished.recv_timeout(Duration::from_millis(500)).is_err(),
        "the submission returned while its pipeline was still building"
    );
    assert!(out.words().iter().all(|w| *w == 0));
    drop(closed);
    finished.recv_timeout(LONG).expect("the submission ends");
    runner.join().unwrap();
    assert!(out.holds_ids(1003), "notes: {:?}", xbox_d3d12::com::notes());
    let after = h.asm.pipeline_service.stats();
    assert_eq!(after.draws_waited, before.draws_waited + 1);
    assert_eq!(after.draws_skipped, before.draws_skipped);
    assert!(after.wait_micros > before.wait_micros);
}

#[test]
fn a_dispatch_is_dropped_and_counted_under_the_skip_policy_until_the_pipeline_is_built() {
    let Some(h) = harness() else { return };
    h.asm.pipeline_service.set_draw_policy(DrawPolicy::Skip);
    let root = uav_root(h.device);
    let program = ids_program(1004);
    let out = output(h.device);
    let closed = Closed::new();
    let (made, pipeline) = create(h.device, root, &program, Creation::Slot62);
    assert_eq!(made, 0);
    let list = dispatch_list(h.device, root, pipeline, out.address);
    let queue = queue(h.device);
    let before = h.asm.pipeline_service.stats();

    let (device, queue_id, list_id) = (h.device as usize, queue as usize, list as usize);
    let (done, finished) = std::sync::mpsc::channel();
    let runner = std::thread::spawn(move || {
        run(device, queue_id, list_id);
        done.send(()).unwrap();
    });
    finished
        .recv_timeout(LONG)
        .expect("a skipped draw does not wait for the build");
    runner.join().unwrap();
    assert!(
        out.words().iter().all(|w| *w == 0),
        "the dispatch was dropped"
    );
    let skipped = h.asm.pipeline_service.stats();
    assert_eq!(skipped.draws_skipped, before.draws_skipped + 1);
    assert_eq!(skipped.draws_waited, before.draws_waited);

    drop(closed);
    let handle = handle(pipeline);
    wait_until("the build", || handle.ready().is_some());
    let list = dispatch_list(h.device, root, pipeline, out.address);
    run(h.device as usize, queue as usize, list as usize);
    assert!(out.holds_ids(1004), "notes: {:?}", xbox_d3d12::com::notes());
    assert_eq!(
        h.asm.pipeline_service.stats().draws_skipped,
        skipped.draws_skipped,
        "a built pipeline is not skipped"
    );
    h.asm.pipeline_service.set_draw_policy(DrawPolicy::Wait);
}

#[test]
fn a_pipeline_that_fails_to_build_is_noted_and_its_draws_are_dropped() {
    let Some(h) = harness() else { return };
    h.asm.pipeline_service.set_draw_policy(DrawPolicy::Wait);
    let root = uav_root(h.device);
    let out = output(h.device);
    let (made, pipeline) = create(h.device, root, &[1, 2, 3, 4], Creation::Slot62);
    assert_eq!(made, 0, "the failure comes with the build, not the call");
    let handle = handle(pipeline);
    wait_until("the failure", || {
        matches!(
            handle.resolve(),
            xbox_d3d12::compile_queue::Resolved::Failed(_)
        )
    });
    assert!(
        xbox_d3d12::com::notes()
            .iter()
            .any(|n| n.starts_with("a compute pipeline failed")),
        "{:?}",
        xbox_d3d12::com::notes()
    );
    let before = h.asm.pipeline_service.stats();
    let list = dispatch_list(h.device, root, pipeline, out.address);
    run(h.device as usize, queue(h.device) as usize, list as usize);
    assert!(out.words().iter().all(|w| *w == 0));
    assert!(h.asm.pipeline_service.stats().draws_failed > before.draws_failed);
}

#[test]
fn a_second_start_finds_the_pipelines_the_first_logged_already_built() {
    let Some(h) = harness() else { return };
    let root = uav_root(h.device);
    let program = ids_program(1005);
    let (made, pipeline) = create(h.device, root, &program, Creation::Slot62);
    assert_eq!(made, 0);
    let first = handle(pipeline);
    wait_until("the first build", || first.ready().is_some());

    h.asm.pipeline_service.shutdown();
    assert!(
        std::fs::metadata(cache_file()).is_ok_and(|m| m.len() > 0),
        "shutting the service down saved the pipeline cache"
    );

    let second = Assembly::build(&config()).expect("the second start");
    let device = device_on(&second) as Ptr;
    let service = &second.pipeline_service;
    wait_until("the replay to name pipelines", || {
        service.stats().replayed >= 1
    });
    assert!(service.wait_idle(LONG));
    let replayed = service.stats();
    assert!(
        replayed.queue.built >= 1,
        "the replay built the logged pipelines"
    );
    assert_eq!(replayed.created, 0);

    let root = uav_root(device);
    let (made, pipeline) = create(device, root, &program, Creation::Slot62);
    assert_eq!(made, 0);
    let again = handle(pipeline);
    assert_eq!(again.key(), first.key(), "the same description");
    assert!(
        again.ready().is_some(),
        "the pipeline was built before the title asked for it"
    );
    let after = service.stats();
    assert_eq!(after.already_known, 1);
    assert_eq!(
        after.queue.built, replayed.queue.built,
        "nothing was built again"
    );

    let out = output(device);
    let list = dispatch_list(device, root, pipeline, out.address);
    run(device as usize, queue(device) as usize, list as usize);
    assert!(out.holds_ids(1005), "notes: {:?}", xbox_d3d12::com::notes());
    assert_eq!(service.stats().draws_waited, 0);
}
