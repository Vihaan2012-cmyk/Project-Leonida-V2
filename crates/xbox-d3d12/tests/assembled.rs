//! The assembled device driven through its vtables the way the title does, on a real GPU with an offscreen
//! target. Every test skips, with a message, when the machine has no GPU that can host the layer.
#![cfg(windows)]
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use shader_container::testutil;
use xbox_d3d12::assembly::{Assembly, AssemblyConfig, create_assembled_device_on};
use xbox_d3d12::com::{Guid, iid};
use xbox_d3d12::present::{PlaneParameters, PresentParameters};
use xbox_d3d12::root::{ParameterKind, RootParameter, RootSignature, RootVersion, Visibility};

type Ptr = *mut c_void;

static ONE_GPU_TEST_AT_A_TIME: Mutex<()> = Mutex::new(());

struct Harness {
    asm: Arc<Assembly>,
    device: Ptr,
    _guard: MutexGuard<'static, ()>,
}

fn harness() -> Option<Harness> {
    let guard = ONE_GPU_TEST_AT_A_TIME
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    // One assembly for the whole file: each holds a window-sized sparse buffer, and the process keeps them all.
    static ASSEMBLY: OnceLock<Option<Arc<Assembly>>> = OnceLock::new();
    let asm = ASSEMBLY
        .get_or_init(
            || match Assembly::build(&AssemblyConfig::offscreen(256, 256)) {
                Ok(asm) => Some(asm),
                Err(e) => {
                    eprintln!("skipped: {e}");
                    None
                }
            },
        )
        .clone()?;
    let unknown_iid = [7u8; 16];
    let params = [0u32; 24];
    let mut device = std::ptr::null_mut();
    // SAFETY: valid pointers for each argument.
    let result = unsafe {
        create_assembled_device_on(&asm, params.as_ptr(), unknown_iid.as_ptr(), &mut device)
    };
    assert_eq!(result, 0);
    Some(Harness {
        asm,
        device,
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

type CreateCommitted = unsafe extern "win64" fn(
    Ptr,
    *const u32,
    u32,
    *const u8,
    u32,
    *const c_void,
    *const u8,
    *mut Ptr,
) -> u32;

impl Harness {
    fn committed(&self, props: u32, desc: &[u8; 56], state: u32) -> Ptr {
        let heap = [props, 0, 0, 1, 1];
        let mut out = std::ptr::null_mut();
        // SAFETY: slot 27 with the PC argument order.
        let result = unsafe {
            let create: CreateCommitted = slot(self.device, 27);
            create(
                self.device,
                heap.as_ptr(),
                0,
                desc.as_ptr(),
                state,
                std::ptr::null(),
                guid(&iid::RESOURCE).as_ptr(),
                &mut out,
            )
        };
        assert_eq!(result, 0);
        out
    }

    fn queue(&self) -> Ptr {
        let desc = [0u32, 0, 0, 0];
        let mut queue = std::ptr::null_mut();
        // SAFETY: slot 8 takes `(desc, riid, out)`.
        let result = unsafe {
            let create: unsafe extern "win64" fn(Ptr, *const u32, *const u8, *mut Ptr) -> u32 =
                slot(self.device, 8);
            create(
                self.device,
                desc.as_ptr(),
                guid(&iid::COMMAND_QUEUE).as_ptr(),
                &mut queue,
            )
        };
        assert_eq!(result, 0);
        queue
    }

    fn list(&self) -> Ptr {
        let (mut allocator, mut list) = (std::ptr::null_mut(), std::ptr::null_mut());
        // SAFETY: slots 9 and 73.
        unsafe {
            let create_allocator: unsafe extern "win64" fn(Ptr, u32, *const u8, *mut Ptr) -> u32 =
                slot(self.device, 9);
            assert_eq!(
                create_allocator(
                    self.device,
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
            ) -> u32 = slot(self.device, 73);
            let desc = [0u32; 4];
            assert_eq!(
                create_list(
                    self.device,
                    desc.as_ptr(),
                    allocator,
                    std::ptr::null_mut(),
                    guid(&iid::GRAPHICS_COMMAND_LIST).as_ptr(),
                    &mut list
                ),
                0
            );
        }
        list
    }

    /// Executes the closed list and waits on a fence the queue signals behind it.
    fn run(&self, queue: Ptr, list: Ptr) {
        let mut fence = std::ptr::null_mut();
        // SAFETY: device slot 36, queue slots 10 and 16, fence slot 9.
        unsafe {
            let create_fence: unsafe extern "win64" fn(Ptr, u64, u32, *const u8, *mut Ptr) -> u32 =
                slot(self.device, 36);
            assert_eq!(
                create_fence(self.device, 0, 0, guid(&iid::FENCE).as_ptr(), &mut fence),
                0
            );
            let execute: unsafe extern "win64" fn(Ptr, u32, *const Ptr) = slot(queue, 10);
            execute(queue, 1, &list);
            let signal: unsafe extern "win64" fn(Ptr, Ptr, u64) -> u32 = slot(queue, 16);
            assert_eq!(signal(queue, fence, 1), 0);
            let wait: unsafe extern "win64" fn(Ptr, u64, Ptr) -> u32 = slot(fence, 9);
            assert_eq!(wait(fence, 1, std::ptr::null_mut()), 0);
        }
    }
}

const STORE_IDS: &str = "v_lshlrev_b32_e32 v1, 2, v0
    s_lshl_b32 s3, s6, 8
    v_add_nc_u32_e32 v1, s3, v1
    v_mad_u32_u24 v2, s6, 1000, v0
    global_store_dword v1, v2, s[4:5]
    s_endpgm";

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

#[test]
fn a_compute_list_runs_on_the_assembled_device_and_its_result_is_in_game_memory() {
    let Some(h) = harness() else { return };
    let buffer = h.committed(1, &buffer_desc(0x10000, 4), 8);
    run_ids(&h, buffer, Creation::Slot11);
}

#[derive(Clone, Copy)]
enum Creation {
    Slot11,
    Slot62,
    Slot86,
}

/// Dispatches the id-storing shader at `buffer` and checks every thread's word in game memory.
fn run_ids(h: &Harness, buffer: Ptr, creation: Creation) {
    let queue = h.queue();
    // SAFETY: resource slot 11, device slots 16 and 11.
    let (address, root, pipeline) = unsafe {
        let gpu_address: unsafe extern "win64" fn(Ptr) -> u64 = slot(buffer, 11);
        let address = gpu_address(buffer);
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
        let create_root: unsafe extern "win64" fn(
            Ptr,
            u32,
            *const u8,
            usize,
            *const u8,
            *mut Ptr,
        ) -> u32 = slot(h.device, 16);
        assert_eq!(
            create_root(
                h.device,
                0,
                signature.as_ptr(),
                signature.len(),
                guid(&iid::ROOT_SIGNATURE).as_ptr(),
                &mut root
            ),
            0
        );
        let container = testutil::container(5, &[fssm_compute(STORE_IDS)]);
        let mut desc = [0u8; 56];
        desc[..8].copy_from_slice(&(root as u64).to_le_bytes());
        desc[8..16].copy_from_slice(&(container.as_ptr() as u64).to_le_bytes());
        desc[16..24].copy_from_slice(&(container.len() as u64).to_le_bytes());
        let mut pipeline = std::ptr::null_mut();
        let riid = guid(&iid::PIPELINE_STATE);
        let made = match creation {
            Creation::Slot11 => {
                let create: unsafe extern "win64" fn(Ptr, *const u8, *const u8, *mut Ptr) -> u32 =
                    slot(h.device, 11);
                create(h.device, desc.as_ptr(), riid.as_ptr(), &mut pipeline)
            }
            Creation::Slot62 => {
                let create: unsafe extern "win64" fn(
                    Ptr,
                    *const u8,
                    usize,
                    usize,
                    *const u8,
                    *mut Ptr,
                ) -> u32 = slot(h.device, 62);
                create(h.device, desc.as_ptr(), 0, 0, riid.as_ptr(), &mut pipeline)
            }
            Creation::Slot86 => {
                let mut stream = [0u8; 40];
                stream[8..16].copy_from_slice(&(root as u64).to_le_bytes());
                stream[16..20].copy_from_slice(&6u32.to_le_bytes());
                stream[24..32].copy_from_slice(&(container.as_ptr() as u64).to_le_bytes());
                stream[32..40].copy_from_slice(&(container.len() as u64).to_le_bytes());
                let wrapper = [stream.len() as u64, stream.as_ptr() as u64];
                let create: unsafe extern "win64" fn(Ptr, *const u64, *const u8, *mut Ptr) -> u32 =
                    slot(h.device, 86);
                create(h.device, wrapper.as_ptr(), riid.as_ptr(), &mut pipeline)
            }
        };
        assert_eq!(made, 0, "notes: {:?}", xbox_d3d12::com::notes());
        (address, root, pipeline)
    };
    let list = h.list();
    // SAFETY: list slots 33, 25, 45, 14 and 9.
    unsafe {
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
    h.run(queue, list);
    // SAFETY: the buffer is committed host memory at its address and the GPU is done with it.
    let words = unsafe { std::slice::from_raw_parts(address as usize as *const u32, 4 * 64) };
    for group in 0..4u32 {
        for thread in 0..64u32 {
            assert_eq!(
                words[(group * 64 + thread) as usize],
                group * 1000 + thread,
                "group {group} thread {thread}; notes: {:?}",
                xbox_d3d12::com::notes()
            );
        }
    }
}

#[test]
fn a_compute_pipeline_made_through_slot_62_with_its_two_zero_words_runs() {
    let Some(h) = harness() else { return };
    let buffer = h.committed(1, &buffer_desc(0x10000, 4), 8);
    run_ids(&h, buffer, Creation::Slot62);
}

#[test]
fn a_compute_pipeline_made_through_a_slot_86_stream_runs() {
    let Some(h) = harness() else { return };
    let buffer = h.committed(1, &buffer_desc(0x10000, 4), 8);
    run_ids(&h, buffer, Creation::Slot86);
}

fn empty_root(h: &Harness) -> Ptr {
    let signature = RootSignature {
        version: RootVersion::V1_1,
        parameters: Vec::new(),
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
        ) -> u32 = slot(h.device, 16);
        assert_eq!(
            create_root(
                h.device,
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

/// The stream of the title's own filler: 0x2b0 bytes, the tags of VS, PS, DS, HS and GS always written.
fn games_stream(root: Ptr, vs: &[u8], ps: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0u8; 0x2b0];
    for (at, ty) in [
        (0x10, 1u32),
        (0x28, 2),
        (0x40, 3),
        (0x58, 4),
        (0x70, 5),
        (0x88, 8),
        (0x1d8, 9),
        (0x1e0, 10),
        (0x210, 21),
        (0x250, 17),
        (0x260, 12),
        (0x278, 14),
        (0x280, 15),
        (0x2a8, 16),
    ] {
        bytes[at..at + 4].copy_from_slice(&ty.to_le_bytes());
    }
    bytes[8..16].copy_from_slice(&(root as u64).to_le_bytes());
    for (at, code) in [(0x18, vs), (0x30, ps)] {
        bytes[at..at + 8].copy_from_slice(&(code.as_ptr() as u64).to_le_bytes());
        bytes[at + 8..at + 16].copy_from_slice(&(code.len() as u64).to_le_bytes());
    }
    bytes[0x1dc..0x1e0].copy_from_slice(&u32::MAX.to_le_bytes());
    bytes[0x27c..0x280].copy_from_slice(&3u32.to_le_bytes());
    bytes[0x284..0x288].copy_from_slice(&28u32.to_le_bytes());
    bytes[0x2ac..0x2b0].copy_from_slice(&45u32.to_le_bytes());
    bytes
}

fn create_stream(h: &Harness, bytes: &[u8]) -> u32 {
    let wrapper = [bytes.len() as u64, bytes.as_ptr() as u64];
    let mut pipeline = std::ptr::null_mut();
    // SAFETY: device slot 86.
    unsafe {
        let create: unsafe extern "win64" fn(Ptr, *const u64, *const u8, *mut Ptr) -> u32 =
            slot(h.device, 86);
        create(
            h.device,
            wrapper.as_ptr(),
            guid(&iid::PIPELINE_STATE).as_ptr(),
            &mut pipeline,
        )
    }
}

#[test]
fn the_games_graphics_stream_reaches_graphics_pipeline_creation_through_slot_86() {
    let Some(h) = harness() else { return };
    let root = empty_root(&h);
    let (vs, ps) = (vec![1u8, 2, 3, 4], vec![9u8, 8, 7]);
    // not shader containers, so creation is refused as invalid: the stream got past the parser to the builder
    let before = xbox_d3d12::com::notes().len();
    assert_eq!(
        create_stream(&h, &games_stream(root, &vs, &ps)),
        0x8007_0057
    );
    let notes = xbox_d3d12::com::notes();
    assert!(
        notes[before..]
            .iter()
            .any(|n| n.starts_with("a graphics pipeline failed")),
        "{notes:?}"
    );
    // no root signature: refused before the builder
    assert_eq!(
        create_stream(&h, &games_stream(std::ptr::null_mut(), &vs, &ps)),
        0x8007_0057
    );
    // a stream with no shader at all
    assert_eq!(
        create_stream(&h, &games_stream(root, &[], &[])),
        0x8007_0057
    );
}

#[test]
fn the_cache_creation_slots_fail_without_ending_the_run() {
    let Some(h) = harness() else { return };
    let desc = [0u64; 32];
    let mut out = std::ptr::null_mut();
    for index in [55, 57, 67] {
        // SAFETY: the slots take a description, an interface id and an out pointer.
        let result = unsafe {
            let create: unsafe extern "win64" fn(Ptr, *const u64, *const u8, *mut Ptr) -> u32 =
                slot(h.device, index);
            create(
                h.device,
                desc.as_ptr(),
                guid(&iid::PIPELINE_STATE).as_ptr(),
                &mut out,
            )
        };
        assert_eq!(result, 0x8000_4001, "slot {index}");
        assert!(out.is_null());
    }
    let notes = xbox_d3d12::com::notes();
    for index in [55, 57, 67] {
        assert!(
            notes
                .iter()
                .any(|n| n.starts_with(&format!("device slot {index} "))),
            "{notes:?}"
        );
    }
}

#[test]
fn a_cleared_back_buffer_presents_to_the_offscreen_target() {
    let Some(h) = harness() else { return };
    let queue = h.queue();
    let back = h.committed(1, &texture_desc(64, 64, 28, 1), 0);
    // An RTV heap and a view of the back buffer.
    let mut heap = std::ptr::null_mut();
    // SAFETY: device slots 14 and 20, heap slot 9.
    let rtv = unsafe {
        let desc = [2u32, 4, 0, 1];
        let create_heap: unsafe extern "win64" fn(Ptr, *const u32, *const u8, *mut Ptr) -> u32 =
            slot(h.device, 14);
        assert_eq!(
            create_heap(
                h.device,
                desc.as_ptr(),
                guid(&iid::DESCRIPTOR_HEAP).as_ptr(),
                &mut heap
            ),
            0
        );
        let start: unsafe extern "win64" fn(Ptr) -> u64 = slot(heap, 9);
        let rtv = start(heap);
        let create_rtv: unsafe extern "win64" fn(Ptr, Ptr, *const u8, u64) = slot(h.device, 20);
        create_rtv(h.device, back, std::ptr::null(), rtv);
        rtv
    };
    // SAFETY: the view wrote 32 bytes of descriptor words at the handle.
    let words = unsafe { std::slice::from_raw_parts(rtv as usize as *const u32, 8) };
    assert!(
        words.iter().any(|w| *w != 0),
        "the render target view is written"
    );

    let list = h.list();
    #[repr(align(8))]
    struct Barrier([u8; 32]);
    let barrier = |before: u32, after: u32| {
        let mut b = Barrier([0u8; 32]);
        let b = &mut b.0;
        b[8..16].copy_from_slice(&(back as u64).to_le_bytes());
        b[16..20].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        b[20..24].copy_from_slice(&before.to_le_bytes());
        b[24..28].copy_from_slice(&after.to_le_bytes());
        Barrier(*b)
    };
    // SAFETY: list slots 26, 52 and 9.
    unsafe {
        let resource_barrier: unsafe extern "win64" fn(Ptr, u32, *const u8) = slot(list, 26);
        let clear: unsafe extern "win64" fn(Ptr, u64, *const [f32; 4], u32, *const c_void) =
            slot(list, 52);
        resource_barrier(list, 1, barrier(0, 4).0.as_ptr());
        clear(list, rtv, &[1.0, 0.0, 0.0, 1.0], 0, std::ptr::null());
        resource_barrier(list, 1, barrier(4, 0).0.as_ptr());
        let close: unsafe extern "win64" fn(Ptr) -> u32 = slot(list, 9);
        assert_eq!(close(list), 0);
    }
    h.run(queue, list);

    // The title's frame loop: interval, origin event, wait for a token, present with it.
    let mut token = 0u64;
    // SAFETY: device slots 78 to 80 and queue slot 32 with the console argument shapes.
    unsafe {
        let set_interval: unsafe extern "win64" fn(Ptr, usize, u32, usize, u32) -> u32 =
            slot(h.device, 78);
        assert_eq!(set_interval(h.device, 0, 16_667, 1, 0), 0);
        let schedule: unsafe extern "win64" fn(Ptr, u32, u32, usize, u32) -> u32 =
            slot(h.device, 79);
        assert_eq!(schedule(h.device, 0, 0, 0, 0), 0);
        let wait: unsafe extern "win64" fn(Ptr, u32, u32, usize, u32, *mut u64) -> u32 =
            slot(h.device, 80);
        assert_eq!(wait(h.device, 0, 1000, 0, 0, &mut token), 0);
        assert_ne!(token, 0);
        let resources = [back];
        let plane = PlaneParameters {
            token,
            resource_count: 1,
            resources: resources.as_ptr(),
            src_view_rects: std::ptr::null(),
            dest_placement_base: std::ptr::null(),
            color_space: 0,
            scale_filter: 0,
            extended_desc_count: 0,
            extended_descs: std::ptr::null(),
            flags: 0,
        };
        let params = PresentParameters {
            immediate_threshold_percent: 0.0,
            view_count: 1,
            extended_desc_count: 0,
            extended_descs: std::ptr::null(),
            flags: 0,
        };
        let present: unsafe extern "win64" fn(
            Ptr,
            u32,
            *const PlaneParameters,
            *const PresentParameters,
        ) -> u32 = slot(queue, 32);
        assert_eq!(present(queue, 1, &plane, &params), 0);
        xbox_d3d12::queue::queue_of(queue).expect("a queue").flush();
    }
    let pixels = h.asm.offscreen_pixels().expect("an offscreen target");
    let centre = (128 * 256 + 128) * 4;
    let rgba = &pixels[centre..centre + 4];
    assert!(
        rgba[0] > 240 && rgba[1] < 16 && rgba[2] < 16,
        "centre pixel {rgba:?}; notes: {:?}",
        xbox_d3d12::com::notes()
    );
}

#[test]
fn a_texture_view_is_written_into_a_heap_slot_and_registered_for_shaders() {
    let Some(h) = harness() else { return };
    let texture = h.committed(1, &texture_desc(32, 32, 28, 0), 0);
    let mut heap = std::ptr::null_mut();
    // SAFETY: device slots 14, 18, 22 and 24, heap slot 9.
    unsafe {
        let desc = [0u32, 8, 1, 1];
        let create_heap: unsafe extern "win64" fn(Ptr, *const u32, *const u8, *mut Ptr) -> u32 =
            slot(h.device, 14);
        assert_eq!(
            create_heap(
                h.device,
                desc.as_ptr(),
                guid(&iid::DESCRIPTOR_HEAP).as_ptr(),
                &mut heap
            ),
            0
        );
        let start: unsafe extern "win64" fn(Ptr) -> u64 = slot(heap, 9);
        let cpu = start(heap);
        let create_srv: unsafe extern "win64" fn(Ptr, Ptr, *const u8, u64) = slot(h.device, 18);
        create_srv(h.device, texture, std::ptr::null(), cpu);
        let words = std::slice::from_raw_parts(cpu as usize as *const u32, 8);
        assert!(words.iter().any(|w| *w != 0));
        let copy: unsafe extern "win64" fn(Ptr, u32, u64, u64, u32) = slot(h.device, 24);
        copy(h.device, 1, cpu + 32, cpu, 0);
        let copied = std::slice::from_raw_parts((cpu + 32) as usize as *const u32, 8);
        assert_eq!(copied, words);
    }
    assert!(h.asm.descriptors().registry.texture_count() >= 1);
}

#[test]
fn a_buffer_placed_in_memory_the_title_mapped_is_that_memory_for_the_gpu() {
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc,
    };
    let Some(h) = harness() else { return };
    let mut at = 0usize;
    for step in 0..28usize {
        let hint = (1usize << 30) + step * (1 << 26);
        // SAFETY: a fresh allocation at the hint or nowhere.
        let p = unsafe {
            VirtualAlloc(
                hint as *const c_void,
                1 << 20,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_READWRITE,
            )
        };
        if !p.is_null() {
            at = p as usize;
            break;
        }
    }
    assert_ne!(at, 0, "no address in the window to place a buffer at");
    let desc = buffer_desc(0x10000, 4);
    let mut out = std::ptr::null_mut();
    // SAFETY: device slot 133, `(va, desc, state, clear, riid, out)`.
    let result = unsafe {
        let create: unsafe extern "win64" fn(
            Ptr,
            u64,
            *const u8,
            u32,
            *const c_void,
            *const u8,
            *mut Ptr,
        ) -> u32 = slot(h.device, 133);
        create(
            h.device,
            at as u64,
            desc.as_ptr(),
            0xFFFF_FFFF,
            std::ptr::null(),
            guid(&iid::RESOURCE).as_ptr(),
            &mut out,
        )
    };
    assert_eq!(result, 0, "notes: {:?}", xbox_d3d12::com::notes());
    // SAFETY: resource slot 11.
    let address = unsafe {
        let gpu_address: unsafe extern "win64" fn(Ptr) -> u64 = slot(out, 11);
        gpu_address(out)
    };
    assert_eq!(address, at as u64);
    run_ids(&h, out, Creation::Slot11);
}
