//! The audio coprocessor libraries: `AcpHal.dll`, `xapu.dll` and `chad.dll`.

use iced_x86::Register;

use crate::emit::{Arg, Emitter, Res};
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
    let handle = e.buffer("acp_handle", 8);
    let contexts = e.buffer("acp_contexts", 8);
    e.call(create, &[Arg::addr(handle), Arg::Imm(0)])?;
    e.fail_if_hresult_failed()?;
    e.call(
        shapes,
        &[Arg::addr(contexts), Arg::Imm(1), Arg::Imm(1), Arg::Imm(0)],
    )?;
    e.fail_if_hresult_failed()?;
    e.load64(Register::RAX, contexts)?;
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
    let command = e.bytes("apu_command", &[0x11; 64]);
    let result = e.buffer("apu_result", 8);
    e.call(connect, &[])?;
    e.fail_if_zero64()?;
    e.store64(handle, Register::RAX)?;
    e.call(
        enqueue,
        &[Arg::load64(handle), Arg::addr(command), Arg::Imm(0)],
    )?;
    e.fail_if_hresult_failed()?;
    e.call(count, &[])?;
    e.fail_if_zero32()?;
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
    e.call(
        connect,
        &[Arg::Imm(0), Arg::Imm(0), Arg::Imm(0), Arg::Imm(0x80)],
    )?;
    e.fail_if_nonzero32()?;
    e.call(activate, &[])?;
    e.fail_if_nonzero32()?;
    e.call(submit, &[Arg::Imm(0), Arg::Imm(0)])?;
    e.fail_if_nonzero32()?;
    e.call(deactivate, &[])?;
    e.fail_if_nonzero32()?;
    e.call(disconnect, &[])?;
    e.fail_if_nonzero32()
}
