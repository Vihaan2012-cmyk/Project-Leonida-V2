//! The assembled device presenting to a real window, through the title's frame-event path.
#![cfg(windows)]
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::time::{Duration, Instant};

use xbox_d3d12::assembly::{
    Assembly, AssemblyConfig, Output, WindowOptionsConfig, create_assembled_device_on,
};
use xbox_d3d12::com::{Guid, iid};
use xbox_d3d12::present::{PlaneParameters, PresentParameters};

type Ptr = *mut c_void;

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

#[repr(align(8))]
struct Barrier([u8; 32]);

fn barrier(resource: Ptr, before: u32, after: u32) -> Barrier {
    let mut b = [0u8; 32];
    b[8..16].copy_from_slice(&(resource as u64).to_le_bytes());
    b[16..20].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    b[20..24].copy_from_slice(&before.to_le_bytes());
    b[24..28].copy_from_slice(&after.to_le_bytes());
    Barrier(b)
}

#[test]
#[ignore = "opens a window on the desktop"]
fn sixty_cleared_frames_present_to_a_real_window_and_it_shuts_down() {
    let started = Instant::now();
    let mut config = AssemblyConfig::windowed();
    config.output = Output::Window(WindowOptionsConfig {
        title: "leonida window test".into(),
        size: (320, 240),
        borderless: false,
        hotkeys: false,
    });
    let asm = match Assembly::build(&config) {
        Ok(asm) => asm,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let window = asm
        .presentation
        .window()
        .expect("the presentation owns a window")
        .clone();
    assert!(!window.state().close_requested);

    let mut device = std::ptr::null_mut();
    let (unknown_iid, params) = ([7u8; 16], [0u32; 24]);
    // SAFETY: valid pointers for each argument.
    let result = unsafe {
        create_assembled_device_on(&asm, params.as_ptr(), unknown_iid.as_ptr(), &mut device)
    };
    assert_eq!(result, 0);

    // SAFETY: every slot below is called with the console argument shapes the title uses.
    unsafe {
        let mut queue = std::ptr::null_mut();
        let create_queue: unsafe extern "win64" fn(Ptr, *const u32, *const u8, *mut Ptr) -> u32 =
            slot(device, 8);
        let desc = [0u32; 4];
        assert_eq!(
            create_queue(
                device,
                desc.as_ptr(),
                guid(&iid::COMMAND_QUEUE).as_ptr(),
                &mut queue
            ),
            0
        );

        let mut back = std::ptr::null_mut();
        let create_committed: unsafe extern "win64" fn(
            Ptr,
            *const u32,
            u32,
            *const u8,
            u32,
            *const c_void,
            *const u8,
            *mut Ptr,
        ) -> u32 = slot(device, 27);
        let heap_props = [1u32, 0, 0, 1, 1];
        assert_eq!(
            create_committed(
                device,
                heap_props.as_ptr(),
                0,
                texture_desc(320, 240, 28, 1).as_ptr(),
                0,
                std::ptr::null(),
                guid(&iid::RESOURCE).as_ptr(),
                &mut back
            ),
            0
        );

        let mut heap = std::ptr::null_mut();
        let create_heap: unsafe extern "win64" fn(Ptr, *const u32, *const u8, *mut Ptr) -> u32 =
            slot(device, 14);
        let heap_desc = [2u32, 4, 0, 1];
        assert_eq!(
            create_heap(
                device,
                heap_desc.as_ptr(),
                guid(&iid::DESCRIPTOR_HEAP).as_ptr(),
                &mut heap
            ),
            0
        );
        let heap_start: unsafe extern "win64" fn(Ptr) -> u64 = slot(heap, 9);
        let rtv = heap_start(heap);
        let create_rtv: unsafe extern "win64" fn(Ptr, Ptr, *const u8, u64) = slot(device, 20);
        create_rtv(device, back, std::ptr::null(), rtv);

        let create_allocator: unsafe extern "win64" fn(Ptr, u32, *const u8, *mut Ptr) -> u32 =
            slot(device, 9);
        let create_list: unsafe extern "win64" fn(
            Ptr,
            *const u32,
            Ptr,
            Ptr,
            *const u8,
            *mut Ptr,
        ) -> u32 = slot(device, 73);
        let create_fence: unsafe extern "win64" fn(Ptr, u64, u32, *const u8, *mut Ptr) -> u32 =
            slot(device, 36);
        let mut fence = std::ptr::null_mut();
        assert_eq!(
            create_fence(device, 0, 0, guid(&iid::FENCE).as_ptr(), &mut fence),
            0
        );
        let fence_wait: unsafe extern "win64" fn(Ptr, u64, Ptr) -> u32 = slot(fence, 9);
        let execute: unsafe extern "win64" fn(Ptr, u32, *const Ptr, u64, u32) -> u32 =
            slot(queue, 35);
        let signal: unsafe extern "win64" fn(Ptr, Ptr, u64) -> u32 = slot(queue, 16);

        let set_interval: unsafe extern "win64" fn(Ptr, usize, u32, usize, u32) -> u32 =
            slot(device, 78);
        let schedule: unsafe extern "win64" fn(Ptr, u32, u32, usize, u32) -> u32 = slot(device, 79);
        let wait: unsafe extern "win64" fn(Ptr, u32, u32, usize, u32, *mut u64) -> u32 =
            slot(device, 80);
        let present: unsafe extern "win64" fn(
            Ptr,
            u32,
            *const PlaneParameters,
            *const PresentParameters,
        ) -> u32 = slot(queue, 32);
        assert_eq!(set_interval(device, 0, 16_667, 1, 0), 0);
        assert_eq!(schedule(device, 0, 0, 0, 0), 0);

        for frame in 0..60u32 {
            let mut allocator = std::ptr::null_mut();
            let mut list = std::ptr::null_mut();
            assert_eq!(
                create_allocator(
                    device,
                    0,
                    guid(&iid::COMMAND_ALLOCATOR).as_ptr(),
                    &mut allocator
                ),
                0
            );
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
            let resource_barrier: unsafe extern "win64" fn(Ptr, u32, *const u8) = slot(list, 26);
            let clear: unsafe extern "win64" fn(Ptr, u64, *const [f32; 4], u32, *const c_void) =
                slot(list, 52);
            let close: unsafe extern "win64" fn(Ptr) -> u32 = slot(list, 9);
            let t = frame as f32 / 59.0;
            resource_barrier(list, 1, barrier(back, 0, 4).0.as_ptr());
            clear(list, rtv, &[t, 0.2, 1.0 - t, 1.0], 0, std::ptr::null());
            resource_barrier(list, 1, barrier(back, 4, 0).0.as_ptr());
            assert_eq!(close(list), 0);
            assert_eq!(execute(queue, 1, &list, 0, 0), 0);
            assert_eq!(signal(queue, fence, u64::from(frame) + 1), 0);
            assert_eq!(
                fence_wait(fence, u64::from(frame) + 1, std::ptr::null_mut()),
                0
            );

            let mut token = 0u64;
            assert_eq!(wait(device, 0, 1000, 0, 0, &mut token), 0, "frame {frame}");
            assert_ne!(token, 0, "frame {frame}");
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
            assert_eq!(
                present(queue, 1, &plane, &params),
                0,
                "present {frame}; notes: {:?}",
                xbox_d3d12::com::notes()
            );
            assert!(
                !window.state().close_requested,
                "close requested at frame {frame}"
            );
        }
        xbox_d3d12::queue::queue_of(queue).expect("a queue").flush();
    }

    let state = window.state();
    assert!(!state.close_requested);
    eprintln!("window state after 60 frames: {state:?}");
    eprintln!("notes: {:?}", xbox_d3d12::com::notes());
    drop(window);
    drop(asm);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "took {:?}",
        started.elapsed()
    );
}
