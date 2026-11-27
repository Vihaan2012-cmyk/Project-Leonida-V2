//! The console D3D12 device and presentation, called the way the game does: through vtable slots of the objects
//! `D3D12XboxCreateDevice` hands out.
//!
//! The device stage covers what the layer implements by itself: the device, a queue, an allocator and a list,
//! descriptor heaps and their start handles, a fence, a submission and the frame events. The present stage needs
//! the assembled device (resources, views, clears, `PresentX`).

use iced_x86::Register;

use super::SYNCH;
use crate::emit::{Arg, Emitter, Res, Sym, Target};
use crate::script::{Script, Stage};

const D3D12: &str = "d3d12_xs.dll";

const IID_DEVICE: &str = "189819f1-1db6-4b57-be54-1821339b85f7";
const IID_QUEUE: &str = "0ec870a6-5d7e-4c22-8cfc-5baae07616ed";
const IID_ALLOCATOR: &str = "6102dee4-af59-4b09-b999-b44d73f09b24";
const IID_LIST: &str = "5b160d0f-ac1b-4185-8ba8-b3ae42a5a455";
const IID_HEAP: &str = "8efb471d-616c-4f49-90f7-127bb763fa51";
const IID_FENCE: &str = "0a753dcf-c4d8-4b91-adf6-be5a60d95a76";
const IID_RESOURCE: &str = "696442be-a72e-4059-bc79-5b5c98040fad";

// Device slots: the PC order for the public chain and the measured console methods.
const DEVICE_CHECK_FEATURE: u32 = 13;
const DEVICE_CREATE_ALLOCATOR: u32 = 9;
const DEVICE_CREATE_HEAP: u32 = 14;
const DEVICE_CREATE_RTV: u32 = 20;
const DEVICE_CREATE_COMMITTED: u32 = 27;
const DEVICE_CREATE_FENCE: u32 = 36;
const DEVICE_CREATE_QUEUE_X: u32 = 48;
const DEVICE_CREATE_LIST_X: u32 = 73;
const DEVICE_SET_FRAME_INTERVAL: u32 = 78;
const DEVICE_SCHEDULE_FRAME_EVENT: u32 = 79;
const DEVICE_WAIT_FRAME_EVENT: u32 = 80;
// Queue, list, fence and heap slots.
const QUEUE_EXECUTE: u32 = 10;
const QUEUE_SIGNAL: u32 = 14;
/// `PresentX` is slot 30 in the older console runtime layout and 32 in the newer; which the game's runtime uses is
/// not decided.
pub const PRESENT_X_SLOT: u32 = 32;
const ALLOCATOR_RESET: u32 = 8;
const LIST_CLOSE: u32 = 9;
const LIST_RESET: u32 = 10;
const LIST_CLEAR_RTV: u32 = 52;
const FENCE_COMPLETED: u32 = 8;
const FENCE_SET_EVENT: u32 = 9;
const HEAP_CPU_START: u32 = 9;
const HEAP_GPU_START: u32 = 10;

const RENDER_TARGET_SIZE: u32 = 64;
/// The colour the render target is cleared to, as the 8-bit channels the presented image should hold.
pub const CLEAR_COLOUR: [f32; 4] = [0.0, 0.5, 1.0, 1.0];

pub fn add_device(script: Script) -> Script {
    script
        .step(
            Stage::Device,
            "D3D12XboxCreateDevice gives a device",
            create_device,
        )
        .step(
            Stage::Device,
            "CheckFeatureSupport answers the options",
            features,
        )
        .step(
            Stage::Device,
            "CreateCommandQueueX (slot 48) gives a queue",
            command_queue,
        )
        .step(
            Stage::Device,
            "CreateCommandAllocator gives an allocator",
            command_allocator,
        )
        .step(
            Stage::Device,
            "CreateCommandListX (slot 73) gives a list",
            command_list,
        )
        .step(
            Stage::Device,
            "a descriptor heap's start handle (slot 9) is stable",
            descriptor_heap,
        )
        .step(
            Stage::Device,
            "a shader-visible heap has a GPU handle (slot 10)",
            visible_heap,
        )
        .step(
            Stage::Device,
            "a list is closed, executed and a fence signalled",
            submit,
        )
        .step(
            Stage::Device,
            "frame interval, schedule and wait give a token",
            frame_events,
        )
}

pub fn add_present(script: Script) -> Script {
    script
        .step(
            Stage::Present,
            "a render target is committed and viewed",
            render_target,
        )
        .step(
            Stage::Present,
            "the render target is cleared by a submitted list",
            clear,
        )
        .step(
            Stage::Present,
            "PresentX shows the render target as one plane",
            present,
        )
}

fn create_device(e: &mut Emitter) -> Res {
    let create = e.import(D3D12, "D3D12XboxCreateDevice");
    let iid = e.guid("iid_device", IID_DEVICE);
    let parameters = e.buffer("device_parameters", 96);
    let device = e.buffer("device", 8);
    e.call(
        create,
        &[
            Arg::Imm(0),
            Arg::addr(parameters),
            Arg::addr(iid),
            Arg::addr(device),
        ],
    )?;
    e.fail_if_hresult_failed()?;
    e.load64(Register::RAX, device)?;
    e.fail_if_zero64()
}

fn features(e: &mut Emitter) -> Res {
    let device = e.cell("device");
    let options = e.buffer("feature_options", 0x40);
    e.vcall(
        device,
        DEVICE_CHECK_FEATURE,
        &[Arg::Imm(0), Arg::addr(options), Arg::Imm(0x40)],
    )?;
    e.fail_if_hresult_failed()
}

fn create_object(
    e: &mut Emitter,
    device: Sym,
    slot: u32,
    leading: &[Arg],
    iid: Sym,
    out: Sym,
    trailing: &[Arg],
) -> Res {
    let mut args = leading.to_vec();
    args.push(Arg::addr(iid));
    args.push(Arg::addr(out));
    args.extend_from_slice(trailing);
    e.vcall(device, slot, &args)?;
    e.fail_if_hresult_failed()?;
    e.load64(Register::RAX, out)?;
    e.fail_if_zero64()
}

fn command_queue(e: &mut Emitter) -> Res {
    let device = e.cell("device");
    let zero_desc = e.buffer("zero_desc", 32);
    let iid = e.guid("iid_queue", IID_QUEUE);
    let queue = e.buffer("queue", 8);
    create_object(
        e,
        device,
        DEVICE_CREATE_QUEUE_X,
        &[Arg::addr(zero_desc)],
        iid,
        queue,
        &[],
    )
}

fn command_allocator(e: &mut Emitter) -> Res {
    let device = e.cell("device");
    let iid = e.guid("iid_allocator", IID_ALLOCATOR);
    let allocator = e.buffer("allocator", 8);
    create_object(
        e,
        device,
        DEVICE_CREATE_ALLOCATOR,
        &[Arg::Imm(0)],
        iid,
        allocator,
        &[],
    )
}

fn command_list(e: &mut Emitter) -> Res {
    let device = e.cell("device");
    let zero_desc = e.cell("zero_desc");
    let iid = e.guid("iid_list", IID_LIST);
    let allocator = e.cell("allocator");
    let list = e.buffer("list", 8);
    create_object(
        e,
        device,
        DEVICE_CREATE_LIST_X,
        &[Arg::addr(zero_desc), Arg::load64(allocator), Arg::Imm(0)],
        iid,
        list,
        &[],
    )
}

fn heap_desc(e: &mut Emitter, name: &str, kind: u32, count: u32, flags: u32) -> Sym {
    let mut bytes = [0u8; 16];
    bytes[0..4].copy_from_slice(&kind.to_le_bytes());
    bytes[4..8].copy_from_slice(&count.to_le_bytes());
    bytes[8..12].copy_from_slice(&flags.to_le_bytes());
    e.bytes(name, &bytes)
}

fn descriptor_heap(e: &mut Emitter) -> Res {
    let device = e.cell("device");
    let desc = heap_desc(e, "rtv_heap_desc", 2, 4, 0);
    let iid = e.guid("iid_heap", IID_HEAP);
    let heap = e.buffer("rtv_heap", 8);
    let start = e.buffer("rtv_start", 8);
    create_object(
        e,
        device,
        DEVICE_CREATE_HEAP,
        &[Arg::addr(desc)],
        iid,
        heap,
        &[],
    )?;
    e.vcall(heap, HEAP_CPU_START, &[])?;
    e.fail_if_zero64()?;
    e.store64(start, Register::RAX)?;
    e.vcall(heap, HEAP_CPU_START, &[])?;
    e.fail_if_rax_ne_cell(start)
}

fn visible_heap(e: &mut Emitter) -> Res {
    let device = e.cell("device");
    let desc = heap_desc(e, "view_heap_desc", 0, 16, 1);
    let iid = e.cell("iid_heap");
    let heap = e.buffer("view_heap", 8);
    let cpu = e.buffer("view_start", 8);
    create_object(
        e,
        device,
        DEVICE_CREATE_HEAP,
        &[Arg::addr(desc)],
        iid,
        heap,
        &[],
    )?;
    e.vcall(heap, HEAP_CPU_START, &[])?;
    e.fail_if_zero64()?;
    e.store64(cpu, Register::RAX)?;
    e.vcall(heap, HEAP_GPU_START, &[])?;
    e.fail_if_zero64()
}

fn wait_for_fence(e: &mut Emitter, value: u64) -> Res {
    let create_event = e.import(SYNCH, "CreateEventW");
    let wait = e.import(SYNCH, "WaitForSingleObject");
    let event = e.buffer("fence_event", 8);
    let fence = e.cell("fence");
    e.call(
        create_event,
        &[Arg::Imm(0), Arg::Imm(0), Arg::Imm(0), Arg::Imm(0)],
    )?;
    e.fail_if_zero64()?;
    e.store64(event, Register::RAX)?;
    e.vcall(
        fence,
        FENCE_SET_EVENT,
        &[Arg::Imm(value), Arg::load64(event)],
    )?;
    e.fail_if_hresult_failed()?;
    e.call(wait, &[Arg::load64(event), Arg::Imm(10_000)])?;
    e.fail_if_nonzero32()?;
    e.vcall(fence, FENCE_COMPLETED, &[])?;
    e.fail_if_below64(value as u32)
}

fn submit(e: &mut Emitter) -> Res {
    let device = e.cell("device");
    let queue = e.cell("queue");
    let list = e.cell("list");
    let fence_iid = e.guid("iid_fence", IID_FENCE);
    let fence = e.buffer("fence", 8);
    create_object(
        e,
        device,
        DEVICE_CREATE_FENCE,
        &[Arg::Imm(0), Arg::Imm(0)],
        fence_iid,
        fence,
        &[],
    )?;
    e.vcall(list, LIST_CLOSE, &[])?;
    e.fail_if_hresult_failed()?;
    e.vcall(queue, QUEUE_EXECUTE, &[Arg::Imm(1), Arg::addr(list)])?;
    e.vcall(queue, QUEUE_SIGNAL, &[Arg::load64(fence), Arg::Imm(1)])?;
    e.fail_if_hresult_failed()?;
    wait_for_fence(e, 1)
}

fn frame_events(e: &mut Emitter) -> Res {
    let device = e.cell("device");
    let token = e.buffer("frame_token", 8);
    e.vcall(
        device,
        DEVICE_SET_FRAME_INTERVAL,
        &[Arg::Imm(0), Arg::Imm(16_667), Arg::Imm(0), Arg::Imm(0)],
    )?;
    e.fail_if_hresult_failed()?;
    e.vcall(
        device,
        DEVICE_SCHEDULE_FRAME_EVENT,
        &[Arg::Imm(0), Arg::Imm(0), Arg::Imm(0), Arg::Imm(0)],
    )?;
    e.fail_if_hresult_failed()?;
    e.vcall(
        device,
        DEVICE_WAIT_FRAME_EVENT,
        &[
            Arg::Imm(0),
            Arg::Imm(1000),
            Arg::Imm(0),
            Arg::Imm(0),
            Arg::addr(token),
        ],
    )?;
    e.fail_if_hresult_failed()?;
    e.load64(Register::RAX, token)?;
    e.fail_if_zero64()
}

fn render_target(e: &mut Emitter) -> Res {
    let device = e.cell("device");
    let iid = e.guid("iid_resource", IID_RESOURCE);
    let resource = e.buffer("render_target", 8);
    let heap_properties = {
        let mut bytes = [0u8; 20];
        bytes[0..4].copy_from_slice(&1u32.to_le_bytes());
        bytes[12..16].copy_from_slice(&1u32.to_le_bytes());
        bytes[16..20].copy_from_slice(&1u32.to_le_bytes());
        e.bytes("target_heap_properties", &bytes)
    };
    // D3D12_RESOURCE_DESC: texture 2D, 64x64, one mip, R8G8B8A8_UNORM, one sample, allow render target.
    let desc = {
        let mut bytes = [0u8; 56];
        bytes[0..4].copy_from_slice(&3u32.to_le_bytes());
        bytes[16..24].copy_from_slice(&u64::from(RENDER_TARGET_SIZE).to_le_bytes());
        bytes[24..28].copy_from_slice(&RENDER_TARGET_SIZE.to_le_bytes());
        bytes[28..30].copy_from_slice(&1u16.to_le_bytes());
        bytes[30..32].copy_from_slice(&1u16.to_le_bytes());
        bytes[32..36].copy_from_slice(&28u32.to_le_bytes());
        bytes[36..40].copy_from_slice(&1u32.to_le_bytes());
        bytes[48..52].copy_from_slice(&1u32.to_le_bytes());
        e.bytes("target_desc", &bytes)
    };
    let clear_value = {
        let mut bytes = [0u8; 20];
        bytes[0..4].copy_from_slice(&28u32.to_le_bytes());
        for (i, channel) in CLEAR_COLOUR.iter().enumerate() {
            bytes[4 + 4 * i..8 + 4 * i].copy_from_slice(&channel.to_le_bytes());
        }
        e.bytes("target_clear_value", &bytes)
    };
    // CreateCommittedResource(heap properties, flags, desc, state RENDER_TARGET, clear value, riid, out)
    create_object(
        e,
        device,
        DEVICE_CREATE_COMMITTED,
        &[
            Arg::addr(heap_properties),
            Arg::Imm(0),
            Arg::addr(desc),
            Arg::Imm(4),
            Arg::addr(clear_value),
        ],
        iid,
        resource,
        &[],
    )?;
    let start = e.cell("rtv_start");
    e.vcall(
        device,
        DEVICE_CREATE_RTV,
        &[Arg::load64(resource), Arg::Imm(0), Arg::load64(start)],
    )
}

fn clear(e: &mut Emitter) -> Res {
    let queue = e.cell("queue");
    let list = e.cell("list");
    let allocator = e.cell("allocator");
    let start = e.cell("rtv_start");
    let colour = {
        let mut bytes = [0u8; 16];
        for (i, channel) in CLEAR_COLOUR.iter().enumerate() {
            bytes[4 * i..4 * i + 4].copy_from_slice(&channel.to_le_bytes());
        }
        e.bytes("clear_colour", &bytes)
    };
    let fence = e.cell("fence");
    e.vcall(allocator, ALLOCATOR_RESET, &[])?;
    e.fail_if_hresult_failed()?;
    e.vcall(list, LIST_RESET, &[Arg::load64(allocator), Arg::Imm(0)])?;
    e.fail_if_hresult_failed()?;
    e.vcall(
        list,
        LIST_CLEAR_RTV,
        &[
            Arg::load64(start),
            Arg::addr(colour),
            Arg::Imm(0),
            Arg::Imm(0),
        ],
    )?;
    e.vcall(list, LIST_CLOSE, &[])?;
    e.fail_if_hresult_failed()?;
    e.vcall(queue, QUEUE_EXECUTE, &[Arg::Imm(1), Arg::addr(list)])?;
    e.vcall(queue, QUEUE_SIGNAL, &[Arg::load64(fence), Arg::Imm(2)])?;
    e.fail_if_hresult_failed()?;
    wait_for_fence(e, 2)
}

fn present(e: &mut Emitter) -> Res {
    let queue = e.cell("queue");
    let resource = e.cell("render_target");
    let token = e.cell("frame_token");
    // D3D12XBOX_PRESENT_PLANE_PARAMETERS: token, resource count, pointer to the resource array.
    let mut plane = [0u8; 0x48];
    plane[8..12].copy_from_slice(&1u32.to_le_bytes());
    let plane = e.bytes_with_pointers(
        "present_plane",
        &plane,
        vec![(16, Target::Place(resource.into()))],
    );
    let parameters = e.buffer("present_parameters", 0x20);
    e.load64(Register::RAX, token)?;
    e.store64(plane, Register::RAX)?;
    e.vcall(
        queue,
        PRESENT_X_SLOT,
        &[Arg::Imm(1), Arg::addr(plane), Arg::addr(parameters)],
    )?;
    e.fail_if_hresult_failed()
}
