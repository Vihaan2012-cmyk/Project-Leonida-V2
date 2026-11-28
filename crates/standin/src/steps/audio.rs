//! The audio coprocessor libraries: `AcpHal.dll`, `xapu.dll` and `chad.dll`.

use iced_x86::Register;
use iced_x86::code_asm::{dword_ptr, eax, rax};

use super::SLEEP;
use crate::emit::{Arg, Emitter, Res, Target};
use crate::script::{Script, Stage};

const ACP: &str = "AcpHal.dll";
const XAPU: &str = "xapu.dll";
const CHAD: &str = "chad.dll";

pub fn add(script: Script) -> Script {
    script
        .step(Stage::Audio, "AcpHalCreate and the shape contexts", hal)
        .step(
            Stage::Audio,
            "ApuAlloc gives memory the coprocessor can address",
            apu_memory,
        )
        .step(
            Stage::Audio,
            "XApuConnect takes a command and gives a result",
            apu_queue,
        )
        .step(Stage::Audio, "the XDsp functions connect and run", dsp)
}

fn hal(e: &mut Emitter) -> Res {
    let create = e.import(ACP, "AcpHalCreate");
    let shapes = e.import(ACP, "AcpHalAllocateShapeContexts");
    let object = e.buffer("acp_object", 8);
    let info = e.buffer("acp_shape_info", 0x40);
    e.call(create, &[Arg::addr(object), Arg::Imm(0)])?;
    e.fail_if_hresult_failed()?;
    e.load64(Register::RAX, object)?;
    e.fail_if_zero64()?;
    e.vcall(object, 0, &[Arg::Imm(0x1000), Arg::Imm(0x1000)])?;
    e.fail_if_hresult_failed()?;
    e.store32_imm(info.at(0x10), 1)?;
    e.call(shapes, &[Arg::addr(info)])?;
    e.fail_if_hresult_failed()?;
    e.load64(Register::RAX, info.at(0x38))?;
    e.fail_if_zero64()
}

fn apu_memory(e: &mut Emitter) -> Res {
    let heap = e.import(ACP, "ApuCreateHeap");
    let alloc = e.import(ACP, "ApuAlloc");
    let valid = e.import(ACP, "ApuIsVirtualAddressValid");
    let map_apu = e.import(ACP, "ApuMapApuAddress");
    let map_cpu = e.import(ACP, "ApuMapVirtualAddress");
    let free = e.import(ACP, "ApuFree");
    let block = e.buffer("apu_block", 8);
    let apu = e.buffer("apu_address", 8);
    let own = e.buffer("apu_own_buffer", 0x1000);
    e.call(heap, &[Arg::Imm(1), Arg::Imm(0x10_0000)])?;
    e.fail_if_hresult_failed()?;
    e.call(
        alloc,
        &[
            Arg::addr(block),
            Arg::addr(apu),
            Arg::Imm(0x1000),
            Arg::Imm(0x1000),
            Arg::Imm(0),
        ],
    )?;
    e.fail_if_hresult_failed()?;
    e.load64(Register::RAX, block)?;
    e.fail_if_zero64()?;
    e.call(valid, &[Arg::load64(block), Arg::Imm(0x1000)])?;
    e.fail_if_zero32()?;
    e.call(map_apu, &[Arg::load32(apu)])?;
    e.fail_if_rax_ne_cell(block)?;
    e.call(map_cpu, &[Arg::addr(own), Arg::Imm(0x1000)])?;
    e.fail_if_zero32()?;
    e.call(free, &[Arg::load64(block)])?;
    e.fail_if_hresult_failed()
}

fn apu_queue(e: &mut Emitter) -> Res {
    let connect = e.import(XAPU, "XApuConnect");
    let enqueue = e.import(XAPU, "XApuEnqueueCommand");
    let count = e.import(XAPU, "XApuGetResultCount");
    let dequeue = e.import(XAPU, "XApuDequeueResult");
    let disconnect = e.import(XAPU, "XApuDisconnect");
    let handle = e.buffer("apu_handle", 8);
    let info = e.buffer("apu_connect_info", 16);
    let region = e.buffer("apu_region", 8);
    let counter = e.buffer("apu_polls", 4);
    let sleep = e.import(SLEEP, "Sleep");
    let mut command = [0u8; 64];
    command[0] = 1;
    command[4] = 5;
    command[0x18] = 2;
    let command = e.bytes("apu_command", &command);
    let result = e.buffer("apu_result", 8);
    e.call(
        connect,
        &[Arg::addr(info), Arg::addr(region), Arg::addr(handle)],
    )?;
    e.fail_if_hresult_failed()?;
    e.load64(Register::RAX, handle)?;
    e.fail_if_zero64()?;
    e.call(
        enqueue,
        &[Arg::load64(handle), Arg::addr(command), Arg::Imm(0)],
    )?;
    e.fail_if_hresult_failed()?;
    // The coprocessor's worker answers on its own thread: poll the result count for up to five seconds.
    let mut again = e.label();
    let mut ready = e.label();
    e.store32_imm(counter, 0)?;
    e.place(&mut again)?;
    e.call(count, &[])?;
    e.asm.test(eax, eax)?;
    e.asm.jnz(ready)?;
    e.add32_imm(counter, 1)?;
    e.call(sleep, &[Arg::Imm(1)])?;
    e.load32(Register::EAX, counter)?;
    e.asm.cmp(eax, 5000)?;
    e.asm.jb(again)?;
    e.fail()?;
    e.place(&mut ready)?;
    e.call(dequeue, &[Arg::load64(handle), Arg::addr(result)])?;
    e.fail_if_hresult_failed()?;
    e.call(disconnect, &[Arg::load64(handle), Arg::Imm(0x20)])?;
    e.fail_if_hresult_failed()
}

fn dsp(e: &mut Emitter) -> Res {
    let connect = e.import(CHAD, "XDspConnectWithMaximumStreamLimit");
    let activate = e.import(CHAD, "XDspActivate");
    let submit = e.import(CHAD, "XDspSubmitCommand");
    let deactivate = e.import(CHAD, "XDspDeactivate");
    let disconnect = e.import(CHAD, "XDspDisconnect");
    let handle = e.buffer("dsp_handle", 8);
    let status = e.buffer("dsp_status", 8);
    let stream = e.buffer("dsp_stream", 8);
    let fence = e.buffer("dsp_fence", 4);
    // A response of 16 samples whose second sample is 1.0, so a batch comes back delayed by one sample.
    let mut response = [0u8; 64];
    response[4..8].copy_from_slice(&1.0f32.to_le_bytes());
    let response = e.bytes("dsp_response", &response);
    let mut input = vec![0u8; 4096];
    input[..4].copy_from_slice(&1.0f32.to_le_bytes());
    let input = e.bytes("dsp_input", &input);
    let output = e.buffer("dsp_output", 4096);
    // The reverb node's 0x1c-byte stream description: kind 1, mono, block 0x400, one channel, 16 words, then the
    // response's address in the unaligned field at +0x14.
    let mut info = [0u8; 0x1c];
    info[0] = 1;
    info[9] = 4;
    info[0xc] = 1;
    info[0x10] = 16;
    let info = e.bytes_with_pointers(
        "dsp_info",
        &info,
        vec![(0x14, Target::Place(response.into()))],
    );
    let mut command = [0u8; 0x20];
    command[..4].copy_from_slice(&1.0f32.to_le_bytes());
    command[4..8].copy_from_slice(&1.0f32.to_le_bytes());
    let command = e.bytes_with_pointers(
        "dsp_command",
        &command,
        vec![
            (8, Target::Place(input.into())),
            (0x10, Target::Place(output.into())),
        ],
    );
    e.call(
        connect,
        &[
            Arg::Imm(0),
            Arg::Imm(0x60000),
            Arg::Imm(0x18),
            Arg::Imm(0x80),
            Arg::Imm(1),
            Arg::addr(handle),
        ],
    )?;
    e.fail_if_nonzero32()?;
    e.load64(Register::RAX, handle)?;
    e.fail_if_zero64()?;
    e.call(
        activate,
        &[
            Arg::load64(handle),
            Arg::addr(info),
            Arg::addr(status),
            Arg::addr(stream),
        ],
    )?;
    e.fail_if_nonzero32()?;
    e.load64(Register::RAX, status)?;
    e.fail_if_zero64()?;
    e.load64(Register::RAX, stream)?;
    e.fail_if_zero64()?;
    e.call(
        submit,
        &[Arg::load64(stream), Arg::addr(command), Arg::addr(fence)],
    )?;
    e.fail_if_nonzero32()?;
    e.fail_if_mem32_ne(fence, 1)?;
    // The title's own check before it reads the batch: the status block's finished fence matches and no error.
    e.load64(Register::RAX, status)?;
    e.asm.mov(eax, dword_ptr(rax + 4))?;
    e.fail_if_ne32(1)?;
    e.load64(Register::RAX, status)?;
    e.asm.mov(eax, dword_ptr(rax + 0xc))?;
    e.fail_if_nonzero32()?;
    // The convolution is in floating point, so the delayed impulse is 1.0 to within a hundredth and the sample
    // before it is as good as silent.
    e.load32(Register::EAX, output.at(4))?;
    e.fail_if_below32(0x3f7d_70a4)?;
    e.fail_if_above32(0x3f82_8f5c)?;
    e.load32(Register::EAX, output.at(0))?;
    e.asm.and(eax, 0x7fff_ffff)?;
    e.fail_if_above32(0x3c23_d70a)?;
    e.call(deactivate, &[Arg::load64(stream)])?;
    e.fail_if_nonzero32()?;
    e.call(disconnect, &[Arg::load64(handle)])?;
    e.fail_if_nonzero32()
}
