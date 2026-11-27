//! Drives the device through its vtables the way the title does, with the console's call shapes.
#![cfg(windows)]
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use xbox_d3d12::com::{self, Guid, iid};
use xbox_d3d12::device::{Integration, create_device};
use xbox_d3d12::queue::{Executor, QueueInfo};
use xbox_d3d12::stream::{Command, CommandStream, GlobalBarrier};

type Ptr = *mut c_void;

const E_NOTIMPL: u32 = 0x8000_4001;
const E_NOINTERFACE: u32 = 0x8000_4002;
const E_FAIL: u32 = 0x8000_4005;
const E_INVALIDARG: u32 = 0x8007_0057;

unsafe fn slot<F: Copy>(object: Ptr, index: usize) -> F {
    // SAFETY: the object's first word is its vtable; the caller names the right signature.
    unsafe {
        let address = *(*(object as *const *const usize)).add(index);
        std::mem::transmute_copy::<usize, F>(&address)
    }
}

fn guid_bytes(g: &Guid) -> [u8; 16] {
    // SAFETY: `Guid` is 16 bytes of plain data.
    unsafe { std::mem::transmute::<Guid, [u8; 16]>(*g) }
}

#[derive(Default)]
struct Capture {
    streams: Mutex<Vec<Arc<CommandStream>>>,
    queues: Mutex<Vec<u32>>,
}

impl Executor for Capture {
    fn execute(&self, queue: &QueueInfo, lists: &[Arc<CommandStream>]) {
        self.queues.lock().unwrap().push(queue.queue_type);
        self.streams.lock().unwrap().extend(lists.iter().cloned());
    }
}

fn new_device(executor: Arc<Capture>) -> Ptr {
    let params = [0x1234u32; 24];
    let unknown_iid = [7u8; 16];
    let mut device = std::ptr::null_mut();
    let integration = Integration {
        executor,
        ..Default::default()
    };
    // SAFETY: valid pointers for each argument.
    let result = unsafe {
        create_device(
            params.as_ptr(),
            unknown_iid.as_ptr(),
            &mut device,
            integration,
        )
    };
    assert_eq!(result, 0);
    assert!(!device.is_null());
    device
}

type QueryFn = unsafe extern "win64" fn(Ptr, *const u8, *mut Ptr) -> u32;
type RefFn = unsafe extern "win64" fn(Ptr) -> u32;
type CreateQueueFn = unsafe extern "win64" fn(Ptr, *const u32, *const u8, *mut Ptr) -> u32;
type CreateAllocatorFn = unsafe extern "win64" fn(Ptr, u32, *const u8, *mut Ptr) -> u32;
type CreateListXFn =
    unsafe extern "win64" fn(Ptr, *const u32, Ptr, Ptr, *const u8, *mut Ptr) -> u32;
type CreateFenceFn = unsafe extern "win64" fn(Ptr, u64, u32, *const u8, *mut Ptr) -> u32;
type CreateHeapFn = unsafe extern "win64" fn(Ptr, *const u32, *const u8, *mut Ptr) -> u32;

fn make_queue(device: Ptr, queue_type: u32, console: bool) -> Ptr {
    let desc = [queue_type, 0, 0, 0, 0xAAAA, 0xBBBB, 0, 0];
    let iid = guid_bytes(&iid::COMMAND_QUEUE);
    let mut queue = std::ptr::null_mut();
    // SAFETY: slots 8 and 48 take `(desc, riid, out)` with the console prefix.
    let result = unsafe {
        let create: CreateQueueFn = slot(device, if console { 48 } else { 8 });
        create(device, desc.as_ptr(), iid.as_ptr(), &mut queue)
    };
    assert_eq!(result, 0);
    queue
}

fn make_allocator(device: Ptr, list_type: u32) -> Ptr {
    let iid = guid_bytes(&iid::COMMAND_ALLOCATOR);
    let mut allocator = std::ptr::null_mut();
    // SAFETY: slot 9.
    let result = unsafe {
        let create: CreateAllocatorFn = slot(device, 9);
        create(device, list_type, iid.as_ptr(), &mut allocator)
    };
    assert_eq!(result, 0);
    allocator
}

fn make_list(device: Ptr, allocator: Ptr, console_iid: &[u8; 16]) -> (u32, Ptr) {
    let desc = [0u32, 0, 0x55, 0x66];
    let mut list = std::ptr::null_mut();
    // SAFETY: slot 73 takes `(desc, allocator, pso, riid, out)`.
    let result = unsafe {
        let create: CreateListXFn = slot(device, 73);
        create(
            device,
            desc.as_ptr(),
            allocator,
            std::ptr::null_mut(),
            console_iid.as_ptr(),
            &mut list,
        )
    };
    (result, list)
}

fn make_fence(device: Ptr, initial: u64) -> Ptr {
    let iid = guid_bytes(&iid::FENCE);
    let mut fence = std::ptr::null_mut();
    // SAFETY: slot 36.
    let result = unsafe {
        let create: CreateFenceFn = slot(device, 36);
        create(device, initial, 0, iid.as_ptr(), &mut fence)
    };
    assert_eq!(result, 0);
    fence
}

fn completed(fence: Ptr) -> u64 {
    // SAFETY: fence slot 8.
    unsafe {
        let get: unsafe extern "win64" fn(Ptr) -> u64 = slot(fence, 8);
        get(fence)
    }
}

#[test]
fn an_unimplemented_slot_notes_and_fails_and_the_vtable_is_long() {
    let device = new_device(Arc::default());
    // SAFETY: any slot takes `this` and up to three arguments.
    unsafe {
        let method: unsafe extern "win64" fn(Ptr, usize, usize, usize) -> u32 = slot(device, 62);
        assert_eq!(method(device, 1, 2, 3), E_NOTIMPL);
        let far: unsafe extern "win64" fn(Ptr, usize, usize, usize) -> u32 = slot(device, 200);
        assert_eq!(far(device, 0, 0, 0), E_NOTIMPL);
    }
    let notes = com::notes();
    assert!(
        notes
            .iter()
            .any(|n| n.starts_with("ID3D12Device slot 62 (CreateComputePipelineStateX, measured)")),
        "{notes:?}"
    );
    assert!(notes.iter().any(|n| n.contains("slot 200 (unknown)")));
    assert!(notes.iter().any(|n| n.contains("parameters 00001234")));
    assert!(notes.iter().any(|n| n.contains("0x1 0x2 0x3")));
}

#[test]
fn query_interface_answers_pc_iids_refuses_known_ones_and_traps_unknown_ones() {
    let device = new_device(Arc::default());
    let mut other = std::ptr::null_mut();
    // SAFETY: slots 0, 1 and 2 of a COM object.
    unsafe {
        let query: QueryFn = slot(device, 0);
        let add_ref: RefFn = slot(device, 1);
        let release: RefFn = slot(device, 2);
        for wanted in [iid::DEVICE, iid::DEVICE10, iid::DEVICE11, iid::OBJECT] {
            assert_eq!(query(device, guid_bytes(&wanted).as_ptr(), &mut other), 0);
            assert_eq!(other, device);
            assert_eq!(release(device), 1);
        }
        assert_eq!(
            query(device, guid_bytes(&iid::DEVICE12).as_ptr(), &mut other),
            E_NOINTERFACE
        );
        assert!(other.is_null());
        assert_eq!(query(device, [9u8; 16].as_ptr(), &mut other), 0);
        assert_ne!(other, device);
        let trapped: unsafe extern "win64" fn(Ptr, usize, usize, usize) -> u32 = slot(other, 20);
        assert_eq!(trapped(other, 0, 0, 0), E_NOTIMPL);
        assert_eq!(add_ref(device), 2);
        assert_eq!(release(device), 1);
    }
}

#[test]
fn a_struct_return_takes_the_buffer_first() {
    let device = new_device(Arc::default());
    let mut props = [0xFFu32; 5];
    // SAFETY: slot 26 is `(buffer, this, node_mask, heap_type)` on the console.
    let returned = unsafe {
        let method: unsafe extern "win64" fn(*mut u32, Ptr, u32, u32) -> *mut u32 =
            slot(device, 26);
        method(props.as_mut_ptr(), device, 1, 2)
    };
    assert_eq!(returned, props.as_mut_ptr());
    assert_eq!(props, [4, 2, 1, 1, 1]);
}

#[test]
fn the_measured_boot_sequence_creates_queues_allocators_lists_and_the_first_heap() {
    let capture = Arc::new(Capture::default());
    let device = new_device(capture.clone());

    let first = make_queue(device, 0, false);
    let compute = make_queue(device, 2, true);
    let more: Vec<Ptr> = (0..3).map(|_| make_queue(device, 3, true)).collect();
    assert_eq!(more.len(), 3);

    // SAFETY: the object is a queue; slot 18 is `GetDesc` with the buffer first.
    let desc = unsafe {
        let mut desc = [0u32; 4];
        let get: unsafe extern "win64" fn(*mut u32, Ptr) -> *mut u32 = slot(compute, 18);
        get(desc.as_mut_ptr(), compute);
        desc
    };
    assert_eq!(desc, [2, 0, 0, 0]);
    let _ = first;

    let fences: Vec<Ptr> = (0..5).map(|_| make_fence(device, 0)).collect();
    let allocators: Vec<Ptr> = (0..7).map(|_| make_allocator(device, 0)).collect();
    assert_eq!(fences.len() + allocators.len(), 12);

    let console_list_iid = [0x42u8; 16];
    let allocator = allocators[0];
    for _ in 0..581 {
        let (result, list) = make_list(device, allocator, &console_list_iid);
        assert_eq!(result, 0);
        // SAFETY: list slots 9 (`Close`) and 2 (`Release`).
        unsafe {
            let close: unsafe extern "win64" fn(Ptr) -> u32 = slot(list, 9);
            assert_eq!(close(list), 0);
            let release: RefFn = slot(list, 2);
            release(list);
        }
    }

    let heap_desc = [0u32, 1024, 1, 0];
    let heap_iid = guid_bytes(&iid::DESCRIPTOR_HEAP);
    let mut heap = std::ptr::null_mut();
    // SAFETY: device slots 14 and 15; heap slots 8, 9 and 10.
    unsafe {
        let create: CreateHeapFn = slot(device, 14);
        assert_eq!(
            create(device, heap_desc.as_ptr(), heap_iid.as_ptr(), &mut heap),
            0
        );
        let increment: unsafe extern "win64" fn(Ptr, u32) -> u32 = slot(device, 15);
        assert_eq!(increment(device, 0), 32);
        let cpu: unsafe extern "win64" fn(Ptr) -> u64 = slot(heap, 9);
        let gpu: unsafe extern "win64" fn(Ptr) -> u64 = slot(heap, 10);
        let (cpu, gpu) = (cpu(heap), gpu(heap));
        assert_ne!(cpu, 0);
        assert_eq!(cpu, gpu);
        assert_eq!(cpu % 65536, 0);
        let again: unsafe extern "win64" fn(Ptr) -> u64 = slot(heap, 9);
        assert_eq!(again(heap), cpu);
        // the heap's bytes are writable through the CPU handle, all the way to the last descriptor
        let last = (cpu + 1023 * 32) as *mut u8;
        std::ptr::write_bytes(last, 0xAB, 32);
        assert_eq!(*last, 0xAB);

        let mut got = [0u32; 4];
        let desc: unsafe extern "win64" fn(*mut u32, Ptr) -> *mut u32 = slot(heap, 8);
        desc(got.as_mut_ptr(), heap);
        assert_eq!(got, heap_desc);
    }
}

#[test]
fn a_heap_that_is_not_shader_visible_has_no_gpu_handle_and_bad_descs_fail() {
    let device = new_device(Arc::default());
    let iid = guid_bytes(&iid::DESCRIPTOR_HEAP);
    let mut heap = std::ptr::null_mut();
    // SAFETY: device slot 14, heap slots 9 and 10.
    unsafe {
        let create: CreateHeapFn = slot(device, 14);
        assert_eq!(
            create(device, [2u32, 16, 0, 0].as_ptr(), iid.as_ptr(), &mut heap),
            0
        );
        let cpu: unsafe extern "win64" fn(Ptr) -> u64 = slot(heap, 9);
        let gpu: unsafe extern "win64" fn(Ptr) -> u64 = slot(heap, 10);
        assert_ne!(cpu(heap), 0);
        assert_eq!(gpu(heap), 0);
        let mut none = std::ptr::null_mut();
        assert_eq!(
            create(device, [2u32, 16, 1, 0].as_ptr(), iid.as_ptr(), &mut none),
            E_INVALIDARG
        );
        assert_eq!(
            create(device, [9u32, 16, 0, 0].as_ptr(), iid.as_ptr(), &mut none),
            E_INVALIDARG
        );
        assert_eq!(
            create(device, [0u32, 0, 0, 0].as_ptr(), iid.as_ptr(), &mut none),
            E_INVALIDARG
        );
    }
}

#[test]
fn heaps_get_distinct_handles_that_the_device_resolves_back() {
    let device = new_device(Arc::default());
    let iid = guid_bytes(&iid::DESCRIPTOR_HEAP);
    let mut heaps = Vec::new();
    for count in [64u32, 4096] {
        let mut heap = std::ptr::null_mut();
        // SAFETY: device slot 14.
        unsafe {
            let create: CreateHeapFn = slot(device, 14);
            assert_eq!(
                create(
                    device,
                    [0u32, count, 1, 0].as_ptr(),
                    iid.as_ptr(),
                    &mut heap
                ),
                0
            );
        }
        heaps.push(heap);
    }
    // SAFETY: heap objects of this layer.
    let infos: Vec<_> = heaps
        .iter()
        .map(|&h| unsafe { xbox_d3d12::heap::info_of(h) }.expect("a heap"))
        .collect();
    assert_ne!(infos[0].gpu_start, infos[1].gpu_start);
    // SAFETY: a device of this layer.
    let core = unsafe { xbox_d3d12::device::device_core(device) };
    let inside = infos[1].gpu_at(100);
    assert_eq!(core.heaps.find(inside), Some(infos[1]));
    assert_eq!(core.heaps.find(infos[0].gpu_at(63)), Some(infos[0]));
    assert_eq!(core.heaps.find(infos[0].gpu_start + 64 * 32), None);
}

#[test]
fn a_list_records_through_the_console_slots_and_the_queue_hands_it_to_the_executor() {
    let capture = Arc::new(Capture::default());
    let device = new_device(capture.clone());
    let queue = make_queue(device, 0, false);
    let allocator = make_allocator(device, 0);
    let (result, list) = make_list(device, allocator, &guid_bytes(&iid::GRAPHICS_COMMAND_LIST));
    assert_eq!(result, 0);

    // the inline command buffer of the console list is full: cursor and both limits read zero
    // SAFETY: the list object is wider than its vtable; these words are inside its state.
    unsafe {
        let words = list as *const u64;
        assert_eq!(*words.add(3), 0);
        assert_eq!(*words.add(5), 0);
        assert_eq!(*words.add(6), 0);
    }

    let barrier_group = [GlobalBarrier {
        sync_before: 0x80,
        sync_after: 0x80,
        access_before: 0x10,
        access_after: 0x10,
    }];
    let group: [u64; 2] = [1u64 << 32, barrier_group.as_ptr() as u64];
    let constants = [7u32, 8, 9];
    // SAFETY: the measured slots and shapes of the console list.
    unsafe {
        let dispatch: unsafe extern "win64" fn(Ptr, u32, u32, u32) = slot(list, 14);
        dispatch(list, 4, 5, 6);
        let dispatch_x: unsafe extern "win64" fn(Ptr, u32, u32, u32, u32) = slot(list, 80);
        dispatch_x(list, 1, 1, 1, 0x8020);
        let root_constants: unsafe extern "win64" fn(Ptr, u32, u32, *const u32, u32) =
            slot(list, 39);
        root_constants(list, 3, 3, constants.as_ptr(), 0);
        let root_srv: unsafe extern "win64" fn(Ptr, u32, u64) = slot(list, 43);
        root_srv(list, 0, 0xDEAD_0000);
        let barrier: unsafe extern "win64" fn(Ptr, u32, *const u64) = slot(list, 146);
        barrier(list, 1, group.as_ptr());
        let upload: unsafe extern "win64" fn(Ptr, usize, usize) -> usize = slot(list, 148);
        let memory = upload(list, 200, 16);
        assert_eq!(memory % 16, 0);
        std::ptr::write_bytes(memory as *mut u8, 1, 200);
        let marker: unsafe extern "win64" fn(Ptr, u8, *const u8, u32) = slot(list, 59);
        marker(list, 1, b"label".as_ptr(), 5);
        let unknown: unsafe extern "win64" fn(Ptr, usize, usize, usize) -> u32 = slot(list, 16);
        assert_eq!(unknown(list, 0, 0, 0), E_NOTIMPL);

        let close: unsafe extern "win64" fn(Ptr) -> u32 = slot(list, 9);
        assert_eq!(close(list), 0);
        assert_eq!(close(list), E_FAIL);
        let execute: unsafe extern "win64" fn(Ptr, u32, *const Ptr) = slot(queue, 10);
        execute(queue, 1, &list);
        let queue_flush = xbox_d3d12::queue::queue_of(queue).expect("a queue");
        queue_flush.flush();
    }

    let streams = capture.streams.lock().unwrap();
    assert_eq!(streams.len(), 1);
    let commands = &streams[0].commands;
    assert!(matches!(
        commands[0],
        Command::Dispatch {
            x: 4,
            y: 5,
            z: 6,
            extra: None
        }
    ));
    assert!(matches!(
        commands[1],
        Command::Dispatch {
            x: 1,
            y: 1,
            z: 1,
            extra: Some(0x8020)
        }
    ));
    match &commands[2] {
        Command::SetRootConstants {
            compute,
            index,
            values,
            ..
        } => {
            assert!(*compute);
            assert_eq!(*index, 3);
            assert_eq!(values, &[7, 8, 9]);
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        commands[3],
        Command::SetRootView {
            address: 0xDEAD_0000,
            ..
        }
    ));
    match &commands[4] {
        Command::EnhancedBarrier(groups) => assert_eq!(groups.len(), 1),
        other => panic!("{other:?}"),
    }
    assert!(matches!(&commands[5], Command::GpuEvent { begin: true, data } if data == b"label"));
    assert_eq!(commands.len(), 6);
    assert_eq!(streams[0].uploads.len(), 1);
}

#[test]
fn allocator_and_list_state_rules_give_the_pc_results() {
    let device = new_device(Arc::default());
    let allocator = make_allocator(device, 0);
    let iid = guid_bytes(&iid::GRAPHICS_COMMAND_LIST);
    let (_, list) = make_list(device, allocator, &iid);
    // a second list on an allocator that is recording
    let (second, _) = make_list(device, allocator, &iid);
    assert_eq!(second, E_INVALIDARG);
    // SAFETY: allocator slot 8 (`Reset`), list slots 9 (`Close`) and 10 (`Reset`).
    unsafe {
        let reset_allocator: unsafe extern "win64" fn(Ptr) -> u32 = slot(allocator, 8);
        assert_eq!(reset_allocator(allocator), E_FAIL);
        let close: unsafe extern "win64" fn(Ptr) -> u32 = slot(list, 9);
        let reset: unsafe extern "win64" fn(Ptr, Ptr, Ptr) -> u32 = slot(list, 10);
        assert_eq!(reset(list, allocator, std::ptr::null_mut()), E_FAIL);
        assert_eq!(close(list), 0);
        assert_eq!(reset_allocator(allocator), 0);
        assert_eq!(reset(list, allocator, std::ptr::null_mut()), 0);
        assert_eq!(close(list), 0);
    }
}

#[test]
fn a_queue_waits_for_a_fence_and_signals_another() {
    let device = new_device(Arc::default());
    let queue = make_queue(device, 0, false);
    let (a, b) = (make_fence(device, 0), make_fence(device, 0));
    // SAFETY: queue slots 14 (`Signal`) and 15 (`Wait`), fence slots 9, 10 and 12.
    unsafe {
        let wait: unsafe extern "win64" fn(Ptr, Ptr, u64) -> u32 = slot(queue, 15);
        let signal: unsafe extern "win64" fn(Ptr, Ptr, u64) -> u32 = slot(queue, 14);
        assert_eq!(wait(queue, a, 5), 0);
        assert_eq!(signal(queue, b, 1), 0);
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(completed(b), 0);

        let address: unsafe extern "win64" fn(Ptr) -> *const u64 = slot(b, 12);
        let cell = address(b);
        assert_eq!(*cell, 0);

        let cpu_signal: unsafe extern "win64" fn(Ptr, u64) -> u32 = slot(a, 10);
        assert_eq!(cpu_signal(a, 5), 0);
        let on_completion: unsafe extern "win64" fn(Ptr, u64, Ptr) -> u32 = slot(b, 9);
        assert_eq!(on_completion(b, 1, std::ptr::null_mut()), 0);
        assert_eq!(completed(b), 1);
        assert_eq!(*cell, 1);
    }
}

#[test]
fn a_fence_sets_a_windows_event_when_it_reaches_the_value() {
    use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
    let device = new_device(Arc::default());
    let fence = make_fence(device, 2);
    // SAFETY: a fresh auto-reset event, fence slots 9 and 10.
    unsafe {
        let event = CreateEventW(std::ptr::null(), 0, 0, std::ptr::null());
        assert!(!event.is_null());
        let on_completion: unsafe extern "win64" fn(Ptr, u64, Ptr) -> u32 = slot(fence, 9);
        assert_eq!(on_completion(fence, 2, event), 0);
        assert_eq!(WaitForSingleObject(event, 1000), 0);
        assert_eq!(on_completion(fence, 9, event), 0);
        assert_eq!(WaitForSingleObject(event, 30), 0x102);
        let signal: unsafe extern "win64" fn(Ptr, u64) -> u32 = slot(fence, 10);
        signal(fence, 9);
        assert_eq!(WaitForSingleObject(event, 1000), 0);
    }
}

#[test]
fn frame_events_give_increasing_tokens_through_the_device_slots() {
    let device = new_device(Arc::default());
    // SAFETY: device slots 78 to 80 with the measured argument counts.
    unsafe {
        let interval: unsafe extern "win64" fn(Ptr, usize, u32, usize, u32) -> u32 =
            slot(device, 78);
        assert_eq!(interval(device, 0, 16667, 0, 0), 0);
        let schedule: unsafe extern "win64" fn(Ptr, u32, u32, usize, u32) -> u32 = slot(device, 79);
        assert_eq!(schedule(device, 0, 0, 0, 0), 0);
        let wait: unsafe extern "win64" fn(Ptr, u32, u32, usize, u32, *mut u64) -> u32 =
            slot(device, 80);
        let (mut first, mut second) = (0u64, 0u64);
        assert_eq!(wait(device, 0, 100, 0, 0, &mut first), 0);
        assert_eq!(wait(device, 0, 100, 0, 0, &mut second), 0);
        assert!(first != 0 && second > first);
    }
}

#[test]
fn compute_and_copy_queues_run_their_lists_and_an_open_list_is_refused() {
    let capture = Arc::new(Capture::default());
    let device = new_device(capture.clone());
    let compute = make_queue(device, 2, true);
    let allocator = make_allocator(device, 0);
    let (_, list) = make_list(device, allocator, &guid_bytes(&iid::GRAPHICS_COMMAND_LIST));
    // SAFETY: queue slot 10, list slot 9.
    unsafe {
        let execute: unsafe extern "win64" fn(Ptr, u32, *const Ptr) = slot(compute, 10);
        execute(compute, 1, &list);
        xbox_d3d12::queue::queue_of(compute).unwrap().flush();
        assert!(capture.streams.lock().unwrap().is_empty());
        let close: unsafe extern "win64" fn(Ptr) -> u32 = slot(list, 9);
        close(list);
        execute(compute, 1, &list);
        xbox_d3d12::queue::queue_of(compute).unwrap().flush();
    }
    assert_eq!(capture.queues.lock().unwrap().as_slice(), &[2]);
    assert_eq!(capture.streams.lock().unwrap().len(), 1);
}

#[test]
fn a_dropped_queue_with_a_wait_that_never_comes_shuts_down() {
    let device = new_device(Arc::default());
    let queue = make_queue(device, 0, false);
    let fence = make_fence(device, 0);
    // SAFETY: queue slot 15 (`Wait`) and `Release`.
    unsafe {
        let wait: unsafe extern "win64" fn(Ptr, Ptr, u64) -> u32 = slot(queue, 15);
        wait(queue, fence, 99);
        let release: RefFn = slot(queue, 2);
        assert_eq!(release(queue), 0);
    }
}
