use std::collections::HashMap;

use iced_x86::code_asm::*;
use iced_x86::{Code, Instruction, MemoryOperand, Register};

use super::*;

pub const BASE: u64 = 0x1_4000_0000;
pub const START: u64 = BASE + 0x1000;
pub const DATA: u64 = BASE + 0x8000;

/// A program made of assembled code at `START`, optional data at `DATA`, and a few named addresses.
#[derive(Default)]
pub struct Image {
    pub code: Vec<u8>,
    pub data: Vec<u8>,
    pub names: HashMap<u64, String>,
    pub texts: HashMap<u64, String>,
    pub imports: HashMap<u64, String>,
    /// Bytes from `START` that belong to the function; code after them is other functions.
    pub function_len: u64,
}

impl Image {
    pub fn new(code: Vec<u8>) -> Self {
        Self {
            function_len: code.len() as u64,
            code,
            ..Self::default()
        }
    }
}

impl Program for Image {
    fn image_base(&self) -> u64 {
        BASE
    }

    fn code(&self, va: u64) -> Option<&[u8]> {
        self.code
            .get(usize::try_from(va.checked_sub(START)?).ok()?..)
    }

    fn bytes(&self, va: u64) -> Option<&[u8]> {
        self.code(va).or_else(|| {
            self.data
                .get(usize::try_from(va.checked_sub(DATA)?).ok()?..)
        })
    }

    fn symbol(&self, va: u64) -> Symbol {
        if let Some(name) = self.imports.get(&va) {
            Symbol::Import(name.clone())
        } else if let Some(text) = self.texts.get(&va) {
            Symbol::Text(text.clone())
        } else if self.code(va).is_some() {
            Symbol::Function
        } else if self.data.len() as u64 > va.wrapping_sub(DATA) {
            Symbol::Data
        } else {
            Symbol::Unknown
        }
    }

    fn name(&self, va: u64) -> Option<String> {
        self.names.get(&va).cloned()
    }

    fn inside(&self, _: u64, target: u64) -> bool {
        (START..START + self.function_len).contains(&target)
    }
}

/// `code reg, [rip+va]`: the assembler's own operands cannot spell an instruction-relative address.
pub fn ip_load(a: &mut CodeAssembler, code: Code, reg: Register, va: u64) -> Result<(), IcedError> {
    let memory = MemoryOperand::with_base_displ(Register::RIP, va as i64);
    a.add_instruction(Instruction::with2(code, reg, memory)?)
}

/// `code [rip+va], reg`.
pub fn ip_store(
    a: &mut CodeAssembler,
    code: Code,
    va: u64,
    reg: Register,
) -> Result<(), IcedError> {
    let memory = MemoryOperand::with_base_displ(Register::RIP, va as i64);
    a.add_instruction(Instruction::with2(code, memory, reg)?)
}

pub fn assemble(build: impl FnOnce(&mut CodeAssembler) -> Result<(), IcedError>) -> Vec<u8> {
    let mut a = CodeAssembler::new(64).expect("assembler");
    build(&mut a).expect("assembles");
    a.assemble(START).expect("encodes")
}

pub fn decompile_code(code: Vec<u8>) -> String {
    decompile(&Image::new(code), START).expect("decompiles")
}

/// Assembles `build` and decompiles it.
pub fn run(build: impl FnOnce(&mut CodeAssembler) -> Result<(), IcedError>) -> String {
    decompile_code(assemble(build))
}

#[test]
fn early_return() {
    let text = run(|a| {
        let mut zero = a.create_label();
        a.test(rcx, rcx)?;
        a.je(zero)?;
        a.lea(rax, ptr(rcx + 1))?;
        a.ret()?;
        a.set_label(&mut zero)?;
        a.xor(eax, eax)?;
        a.ret()
    });
    assert!(
        text.contains("uint32_t sub_00001000(uint64_t a1)"),
        "{text}"
    );
    assert!(
        text.contains(
            "if (a1 == 0) {
        return 0;
    }"
        ),
        "{text}"
    );
    assert!(text.contains("return (uint32_t)(a1 + 1);"), "{text}");
    assert!(!text.contains("goto"), "{text}");
}

#[test]
fn counted_loop() {
    let text = run(|a| {
        let mut done = a.create_label();
        let mut top = a.create_label();
        a.xor(eax, eax)?;
        a.test(ecx, ecx)?;
        a.je(done)?;
        a.set_label(&mut top)?;
        a.add(eax, ecx)?;
        a.dec(ecx)?;
        a.jne(top)?;
        a.set_label(&mut done)?;
        a.ret()
    });
    assert!(text.contains("do {"), "{text}");
    assert!(text.contains("v1 += a1;"), "{text}");
    assert!(text.contains("a1 -= 1;"), "{text}");
    assert!(text.contains("} while (a1 != 0);"), "{text}");
    assert!(text.contains("return v1;"), "{text}");
}

#[test]
fn call_arguments_and_frame() {
    let mut image = Image::new(assemble(|a| {
        a.sub(rsp, 0x28)?;
        a.mov(ecx, 5)?;
        a.call(START + 0x20)?;
        a.add(rsp, 0x28)?;
        a.ret()
    }));
    image.function_len = 0x20;
    image.code.resize(0x20, 0xCC);
    let text = decompile(&image, START).expect("decompiles");
    assert!(text.contains("return sub_00001020(5);"), "{text}");
    assert!(!text.contains("rsp"), "{text}");
}

#[test]
fn names_replace_addresses() {
    let mut image = Image::new(assemble(|a| {
        a.sub(rsp, 0x28)?;
        ip_load(a, Code::Mov_r64_rm64, Register::RCX, DATA)?;
        a.call(START + 0x40)?;
        ip_store(a, Code::Mov_rm64_r64, DATA + 8, Register::RAX)?;
        ip_load(a, Code::Lea_r64_m, Register::RCX, DATA + 0x10)?;
        a.add(rsp, 0x28)?;
        a.jmp(START + 0x50)
    }));
    image.function_len = 0x40;
    image.code.resize(0x60, 0xCC);
    image.data = vec![0; 0x20];
    image.names.insert(START, "Game::Update".into());
    image.names.insert(START + 0x40, "Game::Tick".into());
    image.names.insert(START + 0x50, "Game::Finish".into());
    image.names.insert(DATA, "g_world".into());
    image.names.insert(DATA + 8, "g_last".into());
    image.names.insert(DATA + 0x10, "g_table".into());
    let text = decompile(&image, START).expect("decompiles");
    assert!(text.contains(" Game::Update("), "{text}");
    assert!(text.contains("Game::Tick(g_world)"), "{text}");
    assert!(text.contains("g_last = "), "{text}");
    assert!(text.contains("Game::Finish(&g_table)"), "{text}");
}

#[test]
fn windows_arguments_and_stack_locals() {
    let mut image = Image::new(assemble(|a| {
        a.sub(rsp, 0x58)?;
        a.lea(eax, ptr(r8 + r9))?;
        a.mov(dword_ptr(rsp + 0x30), eax)?;
        a.mov(dword_ptr(rsp + 0x34), edx)?;
        a.lea(rcx, ptr(rsp + 0x30))?;
        a.call(START + 0x80)?;
        a.mov(eax, dword_ptr(rsp + 0x30))?;
        a.add(eax, dword_ptr(rsp + 0x34))?;
        a.add(eax, dword_ptr(rsp + 0x58 + 0x28))?;
        a.add(rsp, 0x58)?;
        a.ret()
    }));
    image.function_len = 0x80;
    image.code.resize(0x90, 0xCC);
    let text = decompile(&image, START).expect("decompiles");
    assert!(
        text.contains("(uint64_t a1, uint32_t a2, uint64_t a3, uint64_t a4, uint32_t a5)"),
        "{text}"
    );
    assert!(text.contains("v_28 = a3 + a4;"), "{text}");
    assert!(text.contains("sub_00001080(&v_28);"), "{text}");
    assert!(text.contains("+ a5"), "{text}");
    assert!(!text.contains("rsp"), "{text}");
}

#[test]
fn float_result_comes_from_xmm0() {
    let text = run(|a| {
        a.mulss(xmm0, xmm1)?;
        a.addss(xmm0, xmm2)?;
        a.ret()
    });
    assert!(
        text.contains("float sub_00001000(float a1, float a2, float a3)"),
        "{text}"
    );
    assert!(text.contains("return a1 * a2 + a3;"), "{text}");
}

#[test]
fn one_register_holding_two_values_gives_two_variables() {
    let text = run(|a| {
        a.mov(rax, qword_ptr(rcx))?;
        a.mov(qword_ptr(rcx + 8), rax)?;
        a.mov(qword_ptr(rcx + 0x10), rax)?;
        a.mov(rax, qword_ptr(rcx + 0x20))?;
        a.mov(qword_ptr(rcx + 0x28), rax)?;
        a.mov(qword_ptr(rcx + 0x30), rax)?;
        a.ret()
    });
    assert!(text.contains("v1 = a1->f_0;"), "{text}");
    assert!(text.contains("v2 = a1->f_20;"), "{text}");
    assert!(!text.contains("rax"), "{text}");
}

#[test]
fn a_value_merged_at_a_join_is_one_variable() {
    // if (a1 == 0) v = 1 else v = a2; return v * v
    let text = run(|a| {
        let mut zero = a.create_label();
        let mut join = a.create_label();
        a.test(rcx, rcx)?;
        a.je(zero)?;
        a.mov(rax, rdx)?;
        a.jmp(join)?;
        a.set_label(&mut zero)?;
        a.mov(eax, 1)?;
        a.set_label(&mut join)?;
        a.imul_2(rax, rax)?;
        a.ret()
    });
    assert!(text.contains("v1 = 1;"), "{text}");
    assert!(text.contains("v1 = a2;"), "{text}");
    assert!(text.contains("return v1 * v1;"), "{text}");
}

#[test]
fn types_follow_width_float_and_dereference() {
    let text = run(|a| {
        a.mulsd(xmm1, xmm1)?;
        a.movsd_2(qword_ptr(rcx + 8), xmm1)?;
        a.movzx(eax, byte_ptr(rcx + 3))?;
        a.ret()
    });
    assert!(
        text.contains("uint32_t sub_00001000(struct S1 *a1, double a2)"),
        "{text}"
    );
    assert!(text.contains("a1->f_8 = a2 * a2;"), "{text}");
    assert!(text.contains("return a1->f_3;"), "{text}");
}

#[test]
fn vectors_and_integer_locals_are_declared() {
    let text = run(|a| {
        let mut skip = a.create_label();
        a.movaps(xmm0, xmmword_ptr(rcx))?;
        a.test(edx, edx)?;
        a.je(skip)?;
        a.movaps(xmm0, xmmword_ptr(rcx + 0x10))?;
        a.set_label(&mut skip)?;
        a.movaps(xmmword_ptr(rcx + 0x20), xmm0)?;
        a.movaps(xmmword_ptr(rcx + 0x30), xmm0)?;
        a.ret()
    });
    assert!(text.contains("(struct S1 *a1, uint32_t a2)"), "{text}");
    assert!(text.contains("__m128 v1;"), "{text}");
}

#[test]
fn pointer_locals_to_structures_are_declared() {
    let text = run(|a| {
        let mut other = a.create_label();
        a.mov(rax, qword_ptr(rcx + 8))?;
        a.mov(rdx, qword_ptr(rcx + 0x10))?;
        a.cmp(dword_ptr(rax), 0)?;
        a.je(other)?;
        a.mov(eax, dword_ptr(rdx + 4))?;
        a.ret()?;
        a.set_label(&mut other)?;
        a.mov(eax, dword_ptr(rax + 4))?;
        a.add(eax, dword_ptr(rdx + 8))?;
        a.ret()
    });
    assert!(text.contains("struct S2 *v1;"), "{text}");
    assert!(text.contains("struct S3 *v2;"), "{text}");
    assert!(text.contains("struct S2 *f_8;"), "{text}");
}

#[test]
fn one_pointer_read_at_several_offsets_becomes_a_structure() {
    let text = run(|a| {
        a.mov(eax, dword_ptr(rcx + 0x10))?;
        a.add(eax, dword_ptr(rcx + 0x14))?;
        a.mov(rdx, qword_ptr(rcx + 0x20))?;
        a.add(eax, dword_ptr(rdx + 4))?;
        a.add(eax, dword_ptr(rdx + 8))?;
        a.ret()
    });
    assert!(text.contains("struct S1 {"), "{text}");
    assert!(text.contains("uint32_t f_10;"), "{text}");
    assert!(text.contains("struct S2 *f_20;"), "{text}");
    assert!(text.contains("(struct S1 *a1)"), "{text}");
    assert!(
        text.contains("a1->f_10 + a1->f_14 + v1->f_4 + v1->f_8"),
        "{text}"
    );
}

#[test]
fn a_virtual_call_on_the_first_argument_is_a_method_call() {
    let text = run(|a| {
        a.push(rbx)?;
        a.sub(rsp, 0x20)?;
        a.mov(rbx, rcx)?;
        a.mov(rax, qword_ptr(rcx))?;
        a.mov(edx, 7)?;
        a.call(qword_ptr(rax + 0x70))?;
        a.mov(rcx, qword_ptr(rbx + 0x18))?;
        a.mov(rax, qword_ptr(rcx))?;
        a.add(rsp, 0x20)?;
        a.pop(rbx)?;
        a.jmp(qword_ptr(rax + 8))
    });
    assert!(text.contains("struct S1 *this"), "{text}");
    assert!(text.contains("void **vtable;"), "{text}");
    assert!(text.contains("this->vf14(7);"), "{text}");
    assert!(text.contains("v1->vf1()"), "{text}");
}

#[test]
fn a_pointer_read_at_one_offset_stays_void() {
    let text = run(|a| {
        a.mov(eax, dword_ptr(rcx + 8))?;
        a.ret()
    });
    assert!(text.contains("uint32_t sub_00001000(void *a1)"), "{text}");
    assert!(text.contains("return *(uint32_t *)(a1 + 8);"), "{text}");
    assert!(!text.contains("struct"), "{text}");
}

#[test]
fn a_function_over_the_block_limit_is_a_stub() {
    let image = Image::new(assemble(|a| {
        for _ in 0..5 {
            let mut next = a.create_label();
            a.test(rcx, rcx)?;
            a.je(next)?;
            a.inc(rax)?;
            a.set_label(&mut next)?;
        }
        a.ret()
    }));
    let options = Options {
        max_blocks: 3,
        ..Options::default()
    };
    let text = decompile_with(&image, START, &options).expect("a stub");
    assert!(text.contains("/* too large: "), "{text}");
    assert!(text.contains(" blocks */"), "{text}");
}

#[test]
fn garbage_bytes_never_panic() {
    for seed in 0u32..200 {
        let mut x = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
        let code: Vec<u8> = (0..96)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect();
        let _ = decompile(&Image::new(code), START);
    }
}
