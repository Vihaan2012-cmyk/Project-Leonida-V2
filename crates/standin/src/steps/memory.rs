//! The console memory functions: plain and GPU-visible allocations, and physical pages mapped at two addresses.

use iced_x86::Register;
use iced_x86::code_asm::{qword_ptr, rax, rcx, rdx};

use super::MEMORY;
use crate::emit::{Arg, Emitter, Res, Sym};
use crate::script::{Script, Stage};

const XMEM: &str = "xmem.dll";

const MEM_RELEASE: u64 = 0x8000;
/// `MEM_RESERVE | MEM_COMMIT | MEM_LARGE_PAGES`, the console's 2 MiB form.
const RESERVE_COMMIT_LARGE: u64 = 0x2000_3000;
/// The attribute word and the `PAGE_READWRITE | PAGE_GRAPHICS_READWRITE` protection the first allocations carry.
const GPU_ATTRIBUTES: u64 = 7;
const GPU_PROTECT: u64 = 0x2004;
/// `MEM_RESERVE | MEM_PHYSICAL | MEM_LARGE_PAGES`.
const RESERVE_PHYSICAL: u64 = 0x2040_2000;
/// `MEM_RESERVE | MEM_REPLACE_PLACEHOLDER | MEM_PHYSICAL | MEM_LARGE_PAGES`.
const CARVE_PHYSICAL: u64 = 0x2040_6000;
/// `MEM_RESERVE | MEM_RESERVE_PLACEHOLDER`.
const RESERVE_PLACEHOLDER: u64 = 0x4_2000;
const PAGE: u64 = 0x1_0000;
const PAGES: u64 = 4;
const PATTERN: u64 = 0x1122_3344_5566_7788;

pub fn add(script: Script) -> Script {
    script
        .step(
            Stage::Memory,
            "XMemAlloc gives writable memory and XMemFree takes it back",
            alloc,
        )
        .step(
            Stage::Memory,
            "XMemVirtualAlloc commits 2 MiB with GPU attributes",
            virtual_alloc,
        )
        .step(
            Stage::Memory,
            "physical pages are allocated for a reserved range",
            allocate_pages,
        )
        .step(
            Stage::Memory,
            "physical pages mapped at two addresses are the same memory",
            map_pages,
        )
        .step(
            Stage::Memory,
            "physical pages map into a range carved from a placeholder",
            carve_pages,
        )
        .step(Stage::Memory, "physical pages unmap and free", free_pages)
        .step(
            Stage::Memory,
            "the working set report and tool memory answer",
            statistics,
        )
}

fn alloc(e: &mut Emitter) -> Res {
    let allocate = e.import(XMEM, "XMemAlloc");
    let free = e.import(XMEM, "XMemFree");
    let block = e.buffer("alloc_block", 8);
    e.call(allocate, &[Arg::Imm(0x2_0000), Arg::Imm(3)])?;
    e.fail_if_zero64()?;
    e.store64(block, Register::RAX)?;
    e.touch(block, 0, PATTERN)?;
    e.touch(block, 0x1_FFF8, !PATTERN)?;
    e.call(free, &[Arg::load64(block), Arg::Imm(3)])
}

fn virtual_alloc(e: &mut Emitter) -> Res {
    let allocate = e.import(XMEM, "XMemVirtualAlloc");
    let release = e.import(MEMORY, "VirtualFree");
    let block = e.buffer("virtual_block", 8);
    e.call(
        allocate,
        &[
            Arg::Imm(0),
            Arg::Imm(0x20_0000),
            Arg::Imm(RESERVE_COMMIT_LARGE),
            Arg::Imm(GPU_ATTRIBUTES),
            Arg::Imm(GPU_PROTECT),
        ],
    )?;
    e.fail_if_zero64()?;
    e.store64(block, Register::RAX)?;
    e.touch(block, 0, PATTERN)?;
    e.touch(block, 0x1F_FFF8, PATTERN ^ 1)?;
    e.call(
        release,
        &[Arg::load64(block), Arg::Imm(0), Arg::Imm(MEM_RELEASE)],
    )?;
    e.fail_if_zero32()
}

fn pages(e: &mut Emitter) -> (Sym, Sym) {
    (
        e.buffer("page_count", 8),
        e.buffer("page_numbers", 8 * PAGES as usize),
    )
}

fn reserve_physical(e: &mut Emitter) -> Res {
    let reserve = e.import(XMEM, "XMemVirtualAlloc");
    e.call(
        reserve,
        &[
            Arg::Imm(0),
            Arg::Imm(PAGES * PAGE),
            Arg::Imm(RESERVE_PHYSICAL),
            Arg::Imm(GPU_ATTRIBUTES),
            Arg::Imm(1),
        ],
    )?;
    e.fail_if_zero64()
}

fn allocate_pages(e: &mut Emitter) -> Res {
    let allocate = e.import(XMEM, "XMemAllocatePhysicalPages");
    let (count, numbers) = pages(e);
    let range = e.buffer("physical_range", 8);
    e.store32_imm(count, PAGES as u32)?;
    e.store32_imm(count.at(4), 0)?;
    e.call(
        allocate,
        &[Arg::Imm(0x280), Arg::addr(count), Arg::addr(numbers)],
    )?;
    e.fail_if_zero32()?;
    e.load64(Register::RAX, count)?;
    e.fail_if_ne64(PAGES)?;
    reserve_physical(e)?;
    e.store64(range, Register::RAX)
}

fn map_pages(e: &mut Emitter) -> Res {
    let map = e.import(XMEM, "XMemMapPhysicalPages");
    let (_, numbers) = pages(e);
    let first = e.cell("physical_range");
    let second = e.buffer("physical_alias", 8);
    reserve_physical(e)?;
    e.store64(second, Register::RAX)?;
    for range in [first, second] {
        e.call(
            map,
            &[Arg::load64(range), Arg::Imm(PAGES), Arg::addr(numbers)],
        )?;
        e.fail_if_zero32()?;
    }
    e.touch(first, 0, PATTERN)?;
    e.load64(Register::RCX, second)?;
    e.asm.mov(rax, qword_ptr(rcx))?;
    e.asm.mov(rdx, PATTERN)?;
    e.fail_if_rax_ne_rdx()?;
    let far = (3 * PAGE) as i32 + 0x100;
    e.touch(second, far, PATTERN ^ 0xFF)?;
    e.load64(Register::RCX, first)?;
    e.asm.mov(rax, qword_ptr(rcx + far))?;
    e.asm.mov(rdx, PATTERN ^ 0xFF)?;
    e.fail_if_rax_ne_rdx()
}

fn carve_pages(e: &mut Emitter) -> Res {
    let reserve = e.import(XMEM, "XMemVirtualAlloc");
    let map = e.import(XMEM, "XMemMapPhysicalPages");
    let (_, numbers) = pages(e);
    let whole = e.buffer("placeholder", 8);
    let part = e.buffer("carved", 8);
    e.call(
        reserve,
        &[
            Arg::Imm(0),
            Arg::Imm(16 * PAGE),
            Arg::Imm(RESERVE_PLACEHOLDER),
            Arg::Imm(GPU_ATTRIBUTES),
            Arg::Imm(1),
        ],
    )?;
    e.fail_if_zero64()?;
    e.store64(whole, Register::RAX)?;
    e.asm.add(rax, (4 * PAGE) as i32)?;
    e.store64(part, Register::RAX)?;
    e.call(
        reserve,
        &[
            Arg::load64(part),
            Arg::Imm(PAGES * PAGE),
            Arg::Imm(CARVE_PHYSICAL),
            Arg::Imm(GPU_ATTRIBUTES),
            Arg::Imm(1),
        ],
    )?;
    e.fail_if_zero64()?;
    e.fail_if_rax_ne_cell(part)?;
    e.call(
        map,
        &[Arg::load64(part), Arg::Imm(PAGES), Arg::addr(numbers)],
    )?;
    e.fail_if_zero32()?;
    e.touch(part, 8, PATTERN ^ 0xAA)
}

fn free_pages(e: &mut Emitter) -> Res {
    let map = e.import(XMEM, "XMemMapPhysicalPages");
    let free = e.import(XMEM, "XMemFreePhysicalPages");
    let (_, numbers) = pages(e);
    for range in ["physical_range", "physical_alias", "carved"] {
        let cell = e.cell(range);
        e.call(map, &[Arg::load64(cell), Arg::Imm(PAGES), Arg::Imm(0)])?;
        e.fail_if_zero32()?;
    }
    e.call(free, &[Arg::Imm(PAGES), Arg::addr(numbers)])?;
    e.fail_if_zero32()
}

fn statistics(e: &mut Emitter) -> Res {
    let working_set = e.import(XMEM, "XMemGetWorkingSetStatistics");
    let tools = e.import(XMEM, "XMemIsToolsMemAvailable");
    let report = e.buffer("working_set", 0x80);
    e.call(working_set, &[Arg::Imm(0x2000_0001), Arg::addr(report)])?;
    e.fail_if_zero32()?;
    e.load64(Register::RAX, report)?;
    e.fail_if_zero64()?;
    e.call(tools, &[])?;
    e.fail_if_nonzero32()
}
