use std::collections::{BTreeSet, HashMap};

use rspirv::binary::Assemble;
use rspirv::dr::{Builder, Operand};
use rspirv::spirv::{
    AddressingModel, BuiltIn, Capability, Decoration, ExecutionMode, ExecutionModel,
    FunctionControl, GlslStd450Op as GLOp, LoopControl, MemoryModel, SelectionControl,
    StorageClass, Word,
};
use shader_ir::{Bin, Cmp, Expr, Function, Reg, Stmt, Terminator, Tri, Ty, Un, VerifyError};

pub const STATE_SCC: u32 = 128;
pub const STATE_VGPR: u32 = 256;
pub const STATE_WORDS: usize = 256 + 256 * 64;
pub const MESSAGE_BASE: u32 = 64 * 4 * 64;
pub const MESSAGE_LIMIT: u32 = 64;
pub const EXPORT_BUFFER_WORDS: usize = MESSAGE_BASE as usize + 1 + 2 * MESSAGE_LIMIT as usize;
const EXEC: u8 = 126;
const VCC: u8 = 106;
const M0: u8 = 124;
const END: u32 = u32::MAX;
const ITERATION_LIMIT: u32 = 1 << 20;

#[derive(Debug, thiserror::Error)]
pub enum EmitError {
    #[error("the IR does not verify: {0}")]
    Verify(#[from] VerifyError),
    #[error("building SPIR-V failed: {0}")]
    Build(String),
    #[error("LDS size must be a power of two number of words")]
    LdsSize,
    #[error("{0}")]
    Unsupported(&'static str),
}

fn ok<T>(result: Result<T, rspirv::dr::Error>) -> Result<T, EmitError> {
    result.map_err(|e| EmitError::Build(format!("{e:?}")))
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FloatMode {
    pub nan_preserve: bool,
    pub flush_denormals: bool,
    pub preserve_denormals: bool,
}

pub const PUSH_WORDS: u32 = 36;
pub const PUSH_HEAP: u32 = 32;
pub const PUSH_CODE: u32 = 34;

#[derive(Debug, Clone, Copy)]
pub struct ComputeOptions {
    pub group_size: [u32; 3],
    pub user_sgprs: u32,
    pub workgroup_ids: [bool; 3],
    pub thread_ids: u32,
    pub lds_bytes: u32,
    pub float: FloatMode,
}

impl ComputeOptions {
    /// From the FSSM's COMPUTE_PGM_RSRC2 word and thread-group size.
    pub fn from_rsrc2(rsrc2: u32, group_size: [u32; 3], float: FloatMode) -> ComputeOptions {
        ComputeOptions {
            group_size,
            user_sgprs: (rsrc2 >> 1) & 31,
            workgroup_ids: [
                rsrc2 >> 7 & 1 != 0,
                rsrc2 >> 8 & 1 != 0,
                rsrc2 >> 9 & 1 != 0,
            ],
            thread_ids: ((rsrc2 >> 11) & 3) + 1,
            lds_bytes: ((rsrc2 >> 15) & 0x1FF) * 512,
            float,
        }
    }
}

enum Entry<'a> {
    Harness(&'a HarnessOptions),
    Compute(&'a ComputeOptions),
}

#[derive(Debug, Clone, Copy)]
pub struct HarnessOptions {
    pub lds_words: u32,
    pub nan_preserve: bool,
    pub flush_denormals: bool,
    pub preserve_denormals: bool,
    pub code_base: u64,
}

struct Types {
    bool_: Word,
    u32_: Word,
    u64_: Word,
    f32_: Word,
    uvec4: Word,
    wide: Word,
    ptr_function_u32: Word,
    ptr_function_bool: Word,
    ptr_storage_u32: Word,
    ptr_workgroup_u32: Word,
    ptr_heap_u32: Word,
    ptr_push_u32: Word,
}

struct Emitter {
    b: Builder,
    glsl: Word,
    t: Types,
    constants: HashMap<(u8, u64), Word>,
    sgpr: HashMap<u8, Word>,
    vgpr: HashMap<u8, Word>,
    scc: Word,
    lane: Word,
    local: Word,
    half: Word,
    lanes: u32,
    exchange: Word,
    state: Word,
    memory: Word,
    lds_image: Word,
    exports: Word,
    messages: Word,
    lds: Word,
    lds_words: u32,
    memory_last: Word,
    decode: Option<Word>,
    code_base: u64,
    heap: Option<Word>,
    code: Option<Word>,
    push: Word,
    exchange_base: Word,
    label: Word,
    compute: Option<ComputeOptions>,
    group_ids: Word,
    thread_ids: Word,
    thread_index: Word,
    values: Vec<Word>,
    types: Vec<Ty>,
}

fn registers(function: &Function) -> (BTreeSet<u8>, BTreeSet<u8>) {
    let mut scalars = BTreeSet::from([EXEC, EXEC + 1]);
    let mut vectors = BTreeSet::new();
    let reg = |reg: &Reg, scalars: &mut BTreeSet<u8>, vectors: &mut BTreeSet<u8>| match reg {
        Reg::S(r) => {
            scalars.insert(*r);
        }
        Reg::V(r) => {
            vectors.insert(*r);
        }
        Reg::Exec => {}
        Reg::Vcc => {
            scalars.extend([VCC, VCC + 1]);
        }
        Reg::Scc => {}
        Reg::M0 => {
            scalars.insert(M0);
        }
    };
    for block in &function.blocks {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let(_, Expr::Get(r)) | Stmt::Set(r, _) => reg(r, &mut scalars, &mut vectors),
                Stmt::Let(_, Expr::GetPair(r)) | Stmt::SetPair(r, _) => {
                    scalars.extend([*r, r + 1]);
                }
                Stmt::SetLane(r, _) | Stmt::WriteLane(r, _, _) => {
                    vectors.insert(*r);
                }
                Stmt::Let(_, Expr::GetRelative(base, _)) => {
                    vectors.insert(0);
                    vectors.extend(*base..=255);
                }
                _ => {}
            }
        }
    }
    (scalars, vectors)
}

pub fn harness(function: &Function, options: &HarnessOptions) -> Result<Vec<u32>, EmitError> {
    build(function, Entry::Harness(options))
}

/// A compute shader following the runtime contract: push constants hold s0-s31, the heap base and the code
/// base; enabled workgroup IDs follow the user SGPRs; v0-v2 hold the thread ID; memory is heap base + Xbox VA.
pub fn compute(function: &Function, options: &ComputeOptions) -> Result<Vec<u32>, EmitError> {
    build(function, Entry::Compute(options))
}

fn build(function: &Function, entry: Entry<'_>) -> Result<Vec<u32>, EmitError> {
    let lanes: u32 = if function.wave64 { 64 } else { 32 };
    let (float, group_size, lds_words) = match entry {
        Entry::Harness(options) => (
            FloatMode {
                nan_preserve: options.nan_preserve,
                flush_denormals: options.flush_denormals,
                preserve_denormals: options.preserve_denormals,
            },
            [lanes, 1, 1],
            options.lds_words,
        ),
        Entry::Compute(options) => {
            let words = (options.lds_bytes / 4).max(lanes).next_power_of_two();
            if words > 12 * 1024 {
                return Err(EmitError::Unsupported("LDS larger than 48 KiB"));
            }
            (options.float, options.group_size, words)
        }
    };
    let threads: u32 = group_size.iter().product();
    if threads == 0 || !threads.is_multiple_of(lanes) {
        return Err(EmitError::Unsupported(
            "thread-group size is not a whole number of waves",
        ));
    }
    let waves = threads / lanes;
    let heap_mode = matches!(entry, Entry::Compute(_));
    if !lds_words.is_power_of_two() {
        return Err(EmitError::LdsSize);
    }
    let types = function.verify()?;
    let mut b = Builder::new();
    b.set_version(1, 3);
    for capability in [
        Capability::Shader,
        Capability::Int64,
        Capability::GroupNonUniform,
        Capability::GroupNonUniformBallot,
        Capability::GroupNonUniformShuffle,
    ] {
        b.capability(capability);
    }
    if heap_mode {
        b.capability(Capability::PhysicalStorageBufferAddresses);
        b.extension("SPV_KHR_physical_storage_buffer");
    }
    if float.nan_preserve {
        b.capability(Capability::SignedZeroInfNanPreserve);
    }
    if float.flush_denormals {
        b.capability(Capability::DenormFlushToZero);
    }
    if float.preserve_denormals {
        b.capability(Capability::DenormPreserve);
    }
    if float.nan_preserve || float.flush_denormals || float.preserve_denormals {
        b.extension("SPV_KHR_float_controls");
    }
    let glsl = b.ext_inst_import("GLSL.std.450");
    let addressing = if heap_mode {
        AddressingModel::PhysicalStorageBuffer64
    } else {
        AddressingModel::Logical
    };
    b.memory_model(addressing, MemoryModel::GLSL450);
    let bool_ = b.type_bool();
    let u32_ = b.type_int(32, 0);
    let u64_ = b.type_int(64, 0);
    let f32_ = b.type_float(32, None);
    let uvec3 = b.type_vector(u32_, 3);
    let uvec4 = b.type_vector(u32_, 4);
    let wide = b.type_struct([u32_, u32_]);
    let (mut state, mut memory, mut lds_image, mut exports, mut push) = (0, 0, 0, 0, 0);
    if heap_mode {
        let count = b.constant_bit32(u32_, PUSH_WORDS);
        let array = b.type_array(u32_, count);
        b.decorate(array, Decoration::ArrayStride, [Operand::LiteralBit32(4)]);
        let block = b.type_struct([array]);
        b.decorate(block, Decoration::Block, []);
        b.member_decorate(block, 0, Decoration::Offset, [Operand::LiteralBit32(0)]);
        let ptr_block = b.type_pointer(None, StorageClass::PushConstant, block);
        push = b.variable(ptr_block, None, StorageClass::PushConstant, None);
    } else {
        let runtime = b.type_runtime_array(u32_);
        b.decorate(runtime, Decoration::ArrayStride, [Operand::LiteralBit32(4)]);
        let block = b.type_struct([runtime]);
        b.decorate(block, Decoration::Block, []);
        b.member_decorate(block, 0, Decoration::Offset, [Operand::LiteralBit32(0)]);
        let ptr_block = b.type_pointer(None, StorageClass::StorageBuffer, block);
        let buffer = |b: &mut Builder, binding: u32| {
            let var = b.variable(ptr_block, None, StorageClass::StorageBuffer, None);
            b.decorate(var, Decoration::DescriptorSet, [Operand::LiteralBit32(0)]);
            b.decorate(var, Decoration::Binding, [Operand::LiteralBit32(binding)]);
            var
        };
        state = buffer(&mut b, 0);
        memory = buffer(&mut b, 1);
        lds_image = buffer(&mut b, 2);
        exports = buffer(&mut b, 3);
    }
    let lds_len = b.constant_bit32(u32_, lds_words);
    let lds_array = b.type_array(u32_, lds_len);
    let ptr_lds = b.type_pointer(None, StorageClass::Workgroup, lds_array);
    let lds = b.variable(ptr_lds, None, StorageClass::Workgroup, None);
    let ptr_input_u32 = b.type_pointer(None, StorageClass::Input, u32_);
    let ptr_input_uvec3 = b.type_pointer(None, StorageClass::Input, uvec3);
    let input = |b: &mut Builder, ty: Word, builtin: BuiltIn| {
        let var = b.variable(ty, None, StorageClass::Input, None);
        b.decorate(var, Decoration::BuiltIn, [Operand::BuiltIn(builtin)]);
        var
    };
    let lane_input = input(&mut b, ptr_input_u32, BuiltIn::SubgroupLocalInvocationId);
    let half_input = input(&mut b, ptr_input_u32, BuiltIn::SubgroupId);
    let group_input = input(&mut b, ptr_input_uvec3, BuiltIn::WorkgroupId);
    let thread_input = input(&mut b, ptr_input_uvec3, BuiltIn::LocalInvocationId);
    let slots = b.constant_bit32(u32_, 64 * waves);
    let exchange_array = b.type_array(u32_, slots);
    let ptr_exchange = b.type_pointer(None, StorageClass::Workgroup, exchange_array);
    let exchange = b.variable(ptr_exchange, None, StorageClass::Workgroup, None);
    let t = Types {
        bool_,
        u32_,
        u64_,
        f32_,
        uvec4,
        wide,
        ptr_function_u32: b.type_pointer(None, StorageClass::Function, u32_),
        ptr_function_bool: b.type_pointer(None, StorageClass::Function, bool_),
        ptr_storage_u32: b.type_pointer(None, StorageClass::StorageBuffer, u32_),
        ptr_workgroup_u32: b.type_pointer(None, StorageClass::Workgroup, u32_),
        ptr_heap_u32: if heap_mode {
            b.type_pointer(None, StorageClass::PhysicalStorageBuffer, u32_)
        } else {
            0
        },
        ptr_push_u32: if heap_mode {
            b.type_pointer(None, StorageClass::PushConstant, u32_)
        } else {
            0
        },
    };
    let void = b.type_void();
    let function_type = b.type_function(void, []);
    let main = ok(b.begin_function(void, None, FunctionControl::NONE, function_type))?;
    let entry_label = ok(b.begin_block(None))?;
    let (scalars, vectors) = registers(function);
    let sgpr: HashMap<u8, Word> = scalars
        .iter()
        .map(|r| {
            (
                *r,
                b.variable(t.ptr_function_u32, None, StorageClass::Function, None),
            )
        })
        .collect();
    let vgpr: HashMap<u8, Word> = vectors
        .iter()
        .map(|r| {
            (
                *r,
                b.variable(t.ptr_function_u32, None, StorageClass::Function, None),
            )
        })
        .collect();
    let scc = b.variable(t.ptr_function_bool, None, StorageClass::Function, None);
    let current = b.variable(t.ptr_function_u32, None, StorageClass::Function, None);
    let iterations = b.variable(t.ptr_function_u32, None, StorageClass::Function, None);
    let messages = b.variable(t.ptr_function_u32, None, StorageClass::Function, None);
    let local = ok(b.load(u32_, None, lane_input, None, []))?;
    let subgroup = ok(b.load(u32_, None, half_input, None, []))?;
    let (lane, half, wave) = if function.wave64 {
        let one = b.constant_bit32(u32_, 1);
        let half = ok(b.bitwise_and(u32_, None, subgroup, one))?;
        let wave = ok(b.shift_right_logical(u32_, None, subgroup, one))?;
        let shift = b.constant_bit32(u32_, 5);
        let base = ok(b.shift_left_logical(u32_, None, half, shift))?;
        (ok(b.i_add(u32_, None, base, local))?, half, wave)
    } else {
        (local, local, subgroup)
    };
    let six = b.constant_bit32(u32_, 6);
    let exchange_base = ok(b.shift_left_logical(u32_, None, wave, six))?;
    let lanes_constant = b.constant_bit32(u32_, lanes);
    let thread_index = ok(b.i_mul(u32_, None, wave, lanes_constant))?;
    let thread_index = ok(b.i_add(u32_, None, thread_index, lane))?;
    let (group_ids, thread_ids) = if heap_mode {
        (
            ok(b.load(uvec3, None, group_input, None, []))?,
            ok(b.load(uvec3, None, thread_input, None, []))?,
        )
    } else {
        (0, 0)
    };
    let mut interface = vec![lane_input, half_input];
    if heap_mode {
        interface.extend([group_input, thread_input]);
    }
    let compute_options = match entry {
        Entry::Compute(options) => Some(*options),
        Entry::Harness(_) => None,
    };
    let code_base = match entry {
        Entry::Harness(options) => options.code_base,
        Entry::Compute(_) => 0,
    };
    let mut e = Emitter {
        b,
        glsl,
        t,
        constants: HashMap::new(),
        sgpr,
        vgpr,
        scc,
        lane,
        local,
        half,
        lanes,
        exchange,
        state,
        memory,
        lds_image,
        exports,
        messages,
        lds,
        lds_words,
        memory_last: 0,
        decode: None,
        code_base,
        heap: None,
        code: None,
        push,
        exchange_base,
        label: entry_label,
        compute: compute_options,
        group_ids,
        thread_ids,
        thread_index,
        values: vec![0; function.temps as usize],
        types,
    };
    if heap_mode {
        e.compute_prologue()?;
    } else {
        e.prologue()?;
    }
    let linear = function
        .blocks
        .iter()
        .enumerate()
        .all(|(id, block)| match block.terminator {
            Terminator::Jump(target) => target == id + 1,
            Terminator::Branch(..) => false,
            Terminator::End => true,
        });
    if linear {
        for block in &function.blocks {
            for stmt in &block.stmts {
                e.stmt(stmt)?;
            }
            if block.terminator == Terminator::End {
                break;
            }
        }
    } else {
        e.dispatch(function, current, iterations)?;
    }
    if !heap_mode {
        e.epilogue()?;
    }
    ok(e.b.ret())?;
    ok(e.b.end_function())?;
    if let Some(decode) = e.decode {
        e.decode_function(decode)?;
    }
    e.b.entry_point(ExecutionModel::GLCompute, main, "main", interface);
    e.b.execution_mode(main, ExecutionMode::LocalSize, group_size);
    if float.nan_preserve {
        e.b.execution_mode(main, ExecutionMode::SignedZeroInfNanPreserve, [32]);
    }
    if float.flush_denormals {
        e.b.execution_mode(main, ExecutionMode::DenormFlushToZero, [32]);
    }
    if float.preserve_denormals {
        e.b.execution_mode(main, ExecutionMode::DenormPreserve, [32]);
    }
    Ok(e.b.module().assemble())
}

impl Emitter {
    fn u32c(&mut self, value: u32) -> Word {
        let ty = self.t.u32_;
        *self
            .constants
            .entry((0, u64::from(value)))
            .or_insert_with(|| self.b.constant_bit32(ty, value))
    }

    fn u64c(&mut self, value: u64) -> Word {
        let ty = self.t.u64_;
        *self
            .constants
            .entry((1, value))
            .or_insert_with(|| self.b.constant_bit64(ty, value))
    }

    fn f32c(&mut self, value: f32) -> Word {
        let ty = self.t.f32_;
        *self
            .constants
            .entry((2, u64::from(value.to_bits())))
            .or_insert_with(|| self.b.constant_bit32(ty, value.to_bits()))
    }

    fn boolc(&mut self, value: bool) -> Word {
        let ty = self.t.bool_;
        *self
            .constants
            .entry((3, u64::from(value)))
            .or_insert_with(|| {
                if value {
                    self.b.constant_true(ty)
                } else {
                    self.b.constant_false(ty)
                }
            })
    }

    fn glsl(&mut self, ty: Word, op: GLOp, args: &[Word]) -> Result<Word, EmitError> {
        let glsl = self.glsl;
        ok(self.b.ext_inst(
            ty,
            None,
            glsl,
            op as u32,
            args.iter().map(|a| Operand::IdRef(*a)),
        ))
    }

    fn element(&mut self, buffer: Word, index: Word) -> Result<Word, EmitError> {
        let zero = self.u32c(0);
        let ptr = self.t.ptr_storage_u32;
        ok(self.b.access_chain(ptr, None, buffer, [zero, index]))
    }

    fn lds_element(&mut self, index: Word) -> Result<Word, EmitError> {
        let ptr = self.t.ptr_workgroup_u32;
        let lds = self.lds;
        ok(self.b.access_chain(ptr, None, lds, [index]))
    }

    fn load(&mut self, ptr: Word) -> Result<Word, EmitError> {
        let ty = self.t.u32_;
        ok(self.b.load(ty, None, ptr, None, []))
    }

    fn store(&mut self, ptr: Word, value: Word) -> Result<(), EmitError> {
        ok(self.b.store(ptr, value, None, []))
    }

    fn barrier(&mut self) -> Result<(), EmitError> {
        let workgroup = self.u32c(2);
        let semantics = self.u32c(0x108);
        ok(self.b.control_barrier(workgroup, workgroup, semantics))
    }

    fn if_then(
        &mut self,
        condition: Word,
        body: impl FnOnce(&mut Self) -> Result<(), EmitError>,
    ) -> Result<(), EmitError> {
        let then = self.b.id();
        let merge = self.b.id();
        ok(self.b.selection_merge(merge, SelectionControl::NONE))?;
        ok(self.b.branch_conditional(condition, then, merge, []))?;
        self.begin(then)?;
        body(self)?;
        ok(self.b.branch(merge))?;
        self.begin(merge)?;
        Ok(())
    }

    fn state_vgpr(&mut self, reg: u8) -> Result<Word, EmitError> {
        let base = self.u32c(STATE_VGPR + 64 * u32::from(reg));
        let u32_ = self.t.u32_;
        let index = ok(self.b.i_add(u32_, None, base, self.lane))?;
        let state = self.state;
        self.element(state, index)
    }

    fn copy_lds(&mut self, inward: bool) -> Result<(), EmitError> {
        let u32_ = self.t.u32_;
        let lanes = self.lanes;
        for chunk in 0..self.lds_words / lanes {
            let base = self.u32c(lanes * chunk);
            let index = ok(self.b.i_add(u32_, None, base, self.lane))?;
            let image = self.lds_image;
            let outside = self.element(image, index)?;
            let inside = self.lds_element(index)?;
            if inward {
                let value = self.load(outside)?;
                self.store(inside, value)?;
            } else {
                let value = self.load(inside)?;
                self.store(outside, value)?;
            }
        }
        Ok(())
    }

    fn prologue(&mut self) -> Result<(), EmitError> {
        let zero = self.u32c(0);
        self.store(self.messages, zero)?;
        let state = self.state;
        let scalars: Vec<(u8, Word)> = self.sgpr.iter().map(|(r, v)| (*r, *v)).collect();
        for (reg, var) in scalars {
            let index = self.u32c(u32::from(reg));
            let ptr = self.element(state, index)?;
            let value = self.load(ptr)?;
            self.store(var, value)?;
        }
        let vectors: Vec<(u8, Word)> = self.vgpr.iter().map(|(r, v)| (*r, *v)).collect();
        for (reg, var) in vectors {
            let ptr = self.state_vgpr(reg)?;
            let value = self.load(ptr)?;
            self.store(var, value)?;
        }
        let index = self.u32c(STATE_SCC);
        let ptr = self.element(state, index)?;
        let raw = self.load(ptr)?;
        let zero = self.u32c(0);
        let bool_ = self.t.bool_;
        let flag = ok(self.b.i_not_equal(bool_, None, raw, zero))?;
        self.store(self.scc, flag)?;
        let u32_ = self.t.u32_;
        let memory = self.memory;
        let length = ok(self.b.array_length(u32_, None, memory, 0))?;
        let one = self.u32c(1);
        self.memory_last = ok(self.b.i_sub(u32_, None, length, one))?;
        self.copy_lds(true)?;
        self.barrier()
    }

    fn epilogue(&mut self) -> Result<(), EmitError> {
        self.barrier()?;
        self.copy_lds(false)?;
        let vectors: Vec<(u8, Word)> = self.vgpr.iter().map(|(r, v)| (*r, *v)).collect();
        for (reg, var) in vectors {
            let value = self.load(var)?;
            let ptr = self.state_vgpr(reg)?;
            self.store(ptr, value)?;
        }
        let zero = self.u32c(0);
        let bool_ = self.t.bool_;
        let first = ok(self.b.i_equal(bool_, None, self.lane, zero))?;
        self.if_then(first, |e| {
            let state = e.state;
            let scalars: Vec<(u8, Word)> = e.sgpr.iter().map(|(r, v)| (*r, *v)).collect();
            for (reg, var) in scalars {
                let value = e.load(var)?;
                let index = e.u32c(u32::from(reg));
                let ptr = e.element(state, index)?;
                e.store(ptr, value)?;
            }
            let bool_ = e.t.bool_;
            let flag = ok(e.b.load(bool_, None, e.scc, None, []))?;
            let (one, zero) = (e.u32c(1), e.u32c(0));
            let u32_ = e.t.u32_;
            let value = ok(e.b.select(u32_, None, flag, one, zero))?;
            let index = e.u32c(STATE_SCC);
            let ptr = e.element(state, index)?;
            e.store(ptr, value)?;
            let count = e.load(e.messages)?;
            let index = e.u32c(MESSAGE_BASE);
            let exports = e.exports;
            let ptr = e.element(exports, index)?;
            e.store(ptr, count)
        })
    }

    fn dispatch(
        &mut self,
        function: &Function,
        current: Word,
        iterations: Word,
    ) -> Result<(), EmitError> {
        let zero = self.u32c(0);
        self.store(current, zero)?;
        self.store(iterations, zero)?;
        let (header, body, switch_merge, cont, merge) = (
            self.b.id(),
            self.b.id(),
            self.b.id(),
            self.b.id(),
            self.b.id(),
        );
        let default = self.b.id();
        let cases: Vec<Word> = function.blocks.iter().map(|_| self.b.id()).collect();
        ok(self.b.branch(header))?;
        self.begin(header)?;
        ok(self.b.loop_merge(merge, cont, LoopControl::NONE, []))?;
        ok(self.b.branch(body))?;
        self.begin(body)?;
        let selector = self.load(current)?;
        ok(self.b.selection_merge(switch_merge, SelectionControl::NONE))?;
        let targets: Vec<(Operand, Word)> = cases
            .iter()
            .enumerate()
            .map(|(i, label)| (Operand::LiteralBit32(i as u32), *label))
            .collect();
        ok(self.b.switch(selector, default, targets))?;
        for (block, label) in function.blocks.iter().zip(&cases) {
            self.begin(*label)?;
            for stmt in &block.stmts {
                self.stmt(stmt)?;
            }
            let next = match block.terminator {
                Terminator::Jump(target) => self.u32c(target as u32),
                Terminator::End => self.u32c(END),
                Terminator::Branch(condition, taken, fallthrough) => {
                    let condition = self.values[condition as usize];
                    let (taken, fallthrough) =
                        (self.u32c(taken as u32), self.u32c(fallthrough as u32));
                    let u32_ = self.t.u32_;
                    ok(self.b.select(u32_, None, condition, taken, fallthrough))?
                }
            };
            self.store(current, next)?;
            ok(self.b.branch(switch_merge))?;
        }
        self.begin(default)?;
        let end = self.u32c(END);
        self.store(current, end)?;
        ok(self.b.branch(switch_merge))?;
        self.begin(switch_merge)?;
        ok(self.b.branch(cont))?;
        self.begin(cont)?;
        let u32_ = self.t.u32_;
        let bool_ = self.t.bool_;
        let count = self.load(iterations)?;
        let one = self.u32c(1);
        let count = ok(self.b.i_add(u32_, None, count, one))?;
        self.store(iterations, count)?;
        let limit = self.u32c(ITERATION_LIMIT);
        let exhausted = ok(self.b.u_greater_than_equal(bool_, None, count, limit))?;
        let next = self.load(current)?;
        let finished = ok(self.b.i_equal(bool_, None, next, end))?;
        let leave = ok(self.b.logical_or(bool_, None, finished, exhausted))?;
        ok(self.b.branch_conditional(leave, merge, header, []))?;
        self.begin(merge)?;
        Ok(())
    }

    fn sgpr_var(&self, reg: u8) -> Word {
        self.sgpr[&reg]
    }

    fn pack(&mut self, low: Word, high: Word) -> Result<Word, EmitError> {
        let u64_ = self.t.u64_;
        let low = ok(self.b.u_convert(u64_, None, low))?;
        let high = ok(self.b.u_convert(u64_, None, high))?;
        let shift = self.u32c(32);
        let high = ok(self.b.shift_left_logical(u64_, None, high, shift))?;
        ok(self.b.bitwise_or(u64_, None, low, high))
    }

    fn get_pair(&mut self, reg: u8) -> Result<Word, EmitError> {
        let low = self.load(self.sgpr_var(reg))?;
        let high = self.load(self.sgpr_var(reg + 1))?;
        self.pack(low, high)
    }

    fn set_pair(&mut self, reg: u8, value: Word) -> Result<(), EmitError> {
        let (low, high) = self.split(value)?;
        self.store(self.sgpr_var(reg), low)?;
        self.store(self.sgpr_var(reg + 1), high)
    }

    fn split(&mut self, value: Word) -> Result<(Word, Word), EmitError> {
        let (u32_, u64_) = (self.t.u32_, self.t.u64_);
        let low = ok(self.b.u_convert(u32_, None, value))?;
        let shift = self.u32c(32);
        let high = ok(self.b.shift_right_logical(u64_, None, value, shift))?;
        let high = ok(self.b.u_convert(u32_, None, high))?;
        Ok((low, high))
    }

    fn exec_word(&mut self) -> Result<Word, EmitError> {
        let low = self.load(self.sgpr_var(EXEC))?;
        if self.lanes == 32 {
            return Ok(low);
        }
        let high = self.load(self.sgpr_var(EXEC + 1))?;
        let (u32_, bool_) = (self.t.u32_, self.t.bool_);
        let zero = self.u32c(0);
        let upper = ok(self.b.i_not_equal(bool_, None, self.half, zero))?;
        ok(self.b.select(u32_, None, upper, high, low))
    }

    fn exchange_slot(&mut self, index: Word) -> Result<Word, EmitError> {
        let ptr = self.t.ptr_workgroup_u32;
        let exchange = self.exchange;
        let u32_ = self.t.u32_;
        let index = ok(self.b.i_add(u32_, None, index, self.exchange_base))?;
        ok(self.b.access_chain(ptr, None, exchange, [index]))
    }

    fn begin(&mut self, label: Word) -> Result<(), EmitError> {
        ok(self.b.begin_block(Some(label)))?;
        self.label = label;
        Ok(())
    }

    fn push_word(&mut self, index: u32) -> Result<Word, EmitError> {
        let ptr = self.t.ptr_push_u32;
        let zero = self.u32c(0);
        let index = self.u32c(index);
        let push = self.push;
        let ptr = ok(self.b.access_chain(ptr, None, push, [zero, index]))?;
        self.load(ptr)
    }

    fn heap_pointer(&mut self, address: Word) -> Result<Word, EmitError> {
        let u64_ = self.t.u64_;
        let base = self.heap.expect("heap mode");
        let sum = ok(self.b.i_add(u64_, None, base, address))?;
        let ptr = self.t.ptr_heap_u32;
        ok(self.b.convert_u_to_ptr(ptr, None, sum))
    }

    fn heap_load(&mut self, address: Word) -> Result<Word, EmitError> {
        let ptr = self.heap_pointer(address)?;
        let u32_ = self.t.u32_;
        ok(self.b.load(
            u32_,
            None,
            ptr,
            Some(rspirv::spirv::MemoryAccess::ALIGNED),
            [Operand::LiteralBit32(4)],
        ))
    }

    fn heap_store(&mut self, address: Word, value: Word) -> Result<(), EmitError> {
        let ptr = self.heap_pointer(address)?;
        ok(self.b.store(
            ptr,
            value,
            Some(rspirv::spirv::MemoryAccess::ALIGNED),
            [Operand::LiteralBit32(4)],
        ))
    }

    fn guarded_loads(
        &mut self,
        condition: Word,
        addresses: &[Word],
    ) -> Result<Vec<Word>, EmitError> {
        let before = self.label;
        let then = self.b.id();
        let merge = self.b.id();
        ok(self.b.selection_merge(merge, SelectionControl::NONE))?;
        ok(self.b.branch_conditional(condition, then, merge, []))?;
        self.begin(then)?;
        let mut loaded = Vec::new();
        for address in addresses {
            loaded.push(self.heap_load(*address)?);
        }
        let end = self.label;
        ok(self.b.branch(merge))?;
        self.begin(merge)?;
        let zero = self.u32c(0);
        let u32_ = self.t.u32_;
        loaded
            .into_iter()
            .map(|value| ok(self.b.phi(u32_, None, [(value, end), (zero, before)])))
            .collect()
    }

    fn compute_prologue(&mut self) -> Result<(), EmitError> {
        let options = self.compute.expect("compute mode");
        let u32_ = self.t.u32_;
        let low = self.push_word(PUSH_HEAP)?;
        let high = self.push_word(PUSH_HEAP + 1)?;
        self.heap = Some(self.pack(low, high)?);
        let low = self.push_word(PUSH_CODE)?;
        let high = self.push_word(PUSH_CODE + 1)?;
        self.code = Some(self.pack(low, high)?);
        let mut group_slots = Vec::new();
        for (axis, enabled) in options.workgroup_ids.iter().enumerate() {
            if *enabled {
                group_slots.push(axis as u32);
            }
        }
        let scalars: Vec<(u8, Word)> = self.sgpr.iter().map(|(r, v)| (*r, *v)).collect();
        for (reg, var) in scalars {
            let index = u32::from(reg);
            let value = if index < options.user_sgprs.min(32) {
                self.push_word(index)?
            } else if let Some(axis) = index
                .checked_sub(options.user_sgprs)
                .and_then(|k| group_slots.get(k as usize))
            {
                ok(self
                    .b
                    .composite_extract(u32_, None, self.group_ids, [*axis]))?
            } else if reg == EXEC || (reg == EXEC + 1 && self.lanes == 64) {
                self.u32c(u32::MAX)
            } else {
                self.u32c(0)
            };
            self.store(var, value)?;
        }
        let vectors: Vec<(u8, Word)> = self.vgpr.iter().map(|(r, v)| (*r, *v)).collect();
        for (reg, var) in vectors {
            let value = if u32::from(reg) < options.thread_ids.min(3) {
                ok(self
                    .b
                    .composite_extract(u32_, None, self.thread_ids, [u32::from(reg)]))?
            } else {
                self.u32c(0)
            };
            self.store(var, value)?;
        }
        let no = self.boolc(false);
        self.store(self.scc, no)?;
        let zero = self.u32c(0);
        self.store(self.messages, zero)?;
        let threads: u32 = options.group_size.iter().product();
        for chunk in 0..self.lds_words.div_ceil(threads) {
            let base = self.u32c(threads * chunk);
            let index = ok(self.b.i_add(u32_, None, base, self.thread_index))?;
            let limit = self.u32c(self.lds_words);
            let bool_ = self.t.bool_;
            let inside = ok(self.b.u_less_than(bool_, None, index, limit))?;
            self.if_then(inside, |e| {
                let ptr = e.lds_element(index)?;
                let zero = e.u32c(0);
                e.store(ptr, zero)
            })?;
        }
        self.barrier()
    }

    fn active(&mut self) -> Result<Word, EmitError> {
        let exec = self.exec_word()?;
        let (u32_, bool_) = (self.t.u32_, self.t.bool_);
        let shifted = ok(self.b.shift_right_logical(u32_, None, exec, self.local))?;
        let one = self.u32c(1);
        let bit = ok(self.b.bitwise_and(u32_, None, shifted, one))?;
        let zero = self.u32c(0);
        ok(self.b.i_not_equal(bool_, None, bit, zero))
    }

    fn float(&mut self, value: Word) -> Result<Word, EmitError> {
        let f32_ = self.t.f32_;
        ok(self.b.bitcast(f32_, None, value))
    }

    fn bits(&mut self, value: Word) -> Result<Word, EmitError> {
        let u32_ = self.t.u32_;
        ok(self.b.bitcast(u32_, None, value))
    }

    fn exact(&mut self, id: Word) -> Word {
        self.b.decorate(id, Decoration::NoContraction, []);
        id
    }

    fn masked_shift(&mut self, amount: Word, mask: u32) -> Result<Word, EmitError> {
        let u32_ = self.t.u32_;
        let mask = self.u32c(mask);
        ok(self.b.bitwise_and(u32_, None, amount, mask))
    }

    fn element_words(&mut self, address: Word) -> Result<[Word; 4], EmitError> {
        let (u32_, u64_) = (self.t.u32_, self.t.u64_);
        let low = ok(self.b.u_convert(u32_, None, address))?;
        let three = self.u32c(3);
        let misaligned = ok(self.b.bitwise_and(u32_, None, low, three))?;
        let shift = ok(self.b.shift_left_logical(u32_, None, misaligned, three))?;
        let first = self.memory_index(address)?;
        let last = self.memory_last;
        let mut dwords = Vec::new();
        for k in 0..5 {
            let step = self.u32c(k);
            let index = ok(self.b.i_add(u32_, None, first, step))?;
            let index = self.glsl(u32_, GLOp::UMin, &[index, last])?;
            let memory = self.memory;
            let ptr = self.element(memory, index)?;
            dwords.push(self.load(ptr)?);
        }
        let mut words = [0; 4];
        for (k, word) in words.iter_mut().enumerate() {
            let pair = self.pack(dwords[k], dwords[k + 1])?;
            let moved = ok(self.b.shift_right_logical(u64_, None, pair, shift))?;
            *word = ok(self.b.u_convert(u32_, None, moved))?;
        }
        Ok(words)
    }

    fn half_to_single(&mut self, half: Word, quiet: bool) -> Result<Word, EmitError> {
        let (u32_, f32_, bool_) = (self.t.u32_, self.t.f32_, self.t.bool_);
        let c = |e: &mut Self, v: u32| e.u32c(v);
        let fifteen = c(self, 15);
        let sign = ok(self.b.shift_right_logical(u32_, None, half, fifteen))?;
        let one = c(self, 1);
        let sign = ok(self.b.bitwise_and(u32_, None, sign, one))?;
        let thirty_one = c(self, 31);
        let sign = ok(self.b.shift_left_logical(u32_, None, sign, thirty_one))?;
        let ten = c(self, 10);
        let exponent = ok(self.b.shift_right_logical(u32_, None, half, ten))?;
        let five_bits = c(self, 0x1F);
        let exponent = ok(self.b.bitwise_and(u32_, None, exponent, five_bits))?;
        let ten_bits = c(self, 0x3FF);
        let mantissa = ok(self.b.bitwise_and(u32_, None, half, ten_bits))?;
        let thirteen = c(self, 13);
        let wide = ok(self.b.shift_left_logical(u32_, None, mantissa, thirteen))?;
        let bias = c(self, 112);
        let rebased = ok(self.b.i_add(u32_, None, exponent, bias))?;
        let twenty_three = c(self, 23);
        let rebased = ok(self.b.shift_left_logical(u32_, None, rebased, twenty_three))?;
        let normal = ok(self.b.bitwise_or(u32_, None, rebased, wide))?;
        let special = c(self, if quiet { 0x7FC0_0000 } else { 0x7F80_0000 });
        let empty = c(self, 0);
        let is_nan_payload = ok(self.b.i_not_equal(bool_, None, mantissa, empty))?;
        let infinity = c(self, 0x7F80_0000);
        let special = ok(self.b.select(u32_, None, is_nan_payload, special, infinity))?;
        let special = if quiet {
            special
        } else {
            ok(self.b.bitwise_or(u32_, None, special, wide))?
        };
        let float = ok(self.b.convert_u_to_f(f32_, None, mantissa))?;
        let scale = self.f32c(f32::from_bits(0x3380_0000));
        let small = ok(self.b.f_mul(f32_, None, float, scale))?;
        let small = self.exact(small);
        let small = self.bits(small)?;
        let top = ok(self.b.i_equal(bool_, None, exponent, five_bits))?;
        let bottom = ok(self.b.i_equal(bool_, None, exponent, empty))?;
        let magnitude = ok(self.b.select(u32_, None, bottom, small, normal))?;
        let magnitude = ok(self.b.select(u32_, None, top, special, magnitude))?;
        ok(self.b.bitwise_or(u32_, None, sign, magnitude))
    }

    fn single_to_half(&mut self, single: Word) -> Result<Word, EmitError> {
        let (u32_, bool_) = (self.t.u32_, self.t.bool_);
        let c = |e: &mut Self, v: u32| e.u32c(v);
        let sixteen = c(self, 16);
        let sign = ok(self.b.shift_right_logical(u32_, None, single, sixteen))?;
        let sign_bit = c(self, 0x8000);
        let sign = ok(self.b.bitwise_and(u32_, None, sign, sign_bit))?;
        let twenty_three = c(self, 23);
        let exponent = ok(self.b.shift_right_logical(u32_, None, single, twenty_three))?;
        let byte = c(self, 0xFF);
        let exponent = ok(self.b.bitwise_and(u32_, None, exponent, byte))?;
        let fraction = c(self, 0x7F_FFFF);
        let mantissa = ok(self.b.bitwise_and(u32_, None, single, fraction))?;
        let zero = c(self, 0);
        let one = c(self, 1);
        let bias = c(self, 112);
        let biased = ok(self.b.i_sub(u32_, None, exponent, bias))?;
        let round = |e: &mut Self, value: Word, shift: Word| -> Result<Word, EmitError> {
            let quotient = ok(e.b.shift_right_logical(u32_, None, value, shift))?;
            let one = e.u32c(1);
            let bit = ok(e.b.shift_left_logical(u32_, None, one, shift))?;
            let low = ok(e.b.i_sub(u32_, None, bit, one))?;
            let rest = ok(e.b.bitwise_and(u32_, None, value, low))?;
            let half = ok(e.b.shift_right_logical(u32_, None, bit, one))?;
            let above = ok(e.b.u_greater_than(bool_, None, rest, half))?;
            let tie = ok(e.b.i_equal(bool_, None, rest, half))?;
            let odd = ok(e.b.bitwise_and(u32_, None, quotient, one))?;
            let none = e.u32c(0);
            let odd = ok(e.b.i_not_equal(bool_, None, odd, none))?;
            let tie = ok(e.b.logical_and(bool_, None, tie, odd))?;
            let up = ok(e.b.logical_or(bool_, None, above, tie))?;
            let zero = e.u32c(0);
            let step = ok(e.b.select(u32_, None, up, one, zero))?;
            ok(e.b.i_add(u32_, None, quotient, step))
        };
        let packed = ok(self.b.shift_left_logical(u32_, None, biased, twenty_three))?;
        let packed = ok(self.b.bitwise_or(u32_, None, packed, mantissa))?;
        let thirteen = c(self, 13);
        let normal = round(self, packed, thirteen)?;
        let implicit = c(self, 0x80_0000);
        let full = ok(self.b.bitwise_or(u32_, None, mantissa, implicit))?;
        let fourteen = c(self, 14);
        let shift = ok(self.b.i_sub(u32_, None, fourteen, biased))?;
        let shift = self.masked_shift(shift, 31)?;
        let tiny = round(self, full, shift)?;
        let infinity = c(self, 0x7C00);
        let nan = c(self, 0x7E00);
        let empty = ok(self.b.i_equal(bool_, None, mantissa, zero))?;
        let special = ok(self.b.select(u32_, None, empty, infinity, nan))?;
        let top = ok(self.b.i_equal(bool_, None, exponent, byte))?;
        let thirty_one = c(self, 31);
        let overflow = ok(self.b.s_greater_than_equal(bool_, None, biased, thirty_one))?;
        let positive = ok(self.b.s_greater_than_equal(bool_, None, biased, one))?;
        let minus_ten = c(self, (-10i32) as u32);
        let underflow = ok(self.b.s_less_than(bool_, None, biased, minus_ten))?;
        let value = ok(self.b.select(u32_, None, positive, normal, tiny))?;
        let value = ok(self.b.select(u32_, None, underflow, zero, value))?;
        let value = ok(self.b.select(u32_, None, overflow, infinity, value))?;
        let value = ok(self.b.select(u32_, None, top, special, value))?;
        ok(self.b.bitwise_or(u32_, None, sign, value))
    }

    fn convert_field(
        &mut self,
        layout: &rdna::format::BufferFormat,
        channel: usize,
        field: Word,
    ) -> Result<Word, EmitError> {
        use rdna::format::Number;
        let (u32_, f32_) = (self.t.u32_, self.t.f32_);
        let width = u32::from(layout.widths[channel]);
        let signed = if width == 32 {
            field
        } else {
            let spare = self.u32c(32 - width);
            let up = ok(self.b.shift_left_logical(u32_, None, field, spare))?;
            ok(self.b.shift_right_arithmetic(u32_, None, up, spare))?
        };
        Ok(match layout.number {
            Number::Uint => field,
            Number::Sint => signed,
            Number::Uscaled => {
                let f = ok(self.b.convert_u_to_f(f32_, None, field))?;
                self.bits(f)?
            }
            Number::Sscaled => {
                let f = ok(self.b.convert_s_to_f(f32_, None, signed))?;
                self.bits(f)?
            }
            Number::Unorm | Number::Snorm => {
                let f64_ = self.b.type_float(64, None);
                let max = if layout.number == Number::Unorm {
                    (1u64 << width) - 1
                } else {
                    (1u64 << (width - 1)) - 1
                };
                let numerator = if layout.number == Number::Unorm {
                    ok(self.b.convert_u_to_f(f64_, None, field))?
                } else {
                    ok(self.b.convert_s_to_f(f64_, None, signed))?
                };
                let denominator = self.b.constant_bit64(f64_, (max as f64).to_bits());
                let quotient = ok(self.b.f_div(f64_, None, numerator, denominator))?;
                let quotient = self.exact(quotient);
                let quotient = if layout.number == Number::Snorm {
                    let floor = self.b.constant_bit64(f64_, (-1.0f64).to_bits());
                    self.glsl(f64_, GLOp::NMax, &[quotient, floor])?
                } else {
                    quotient
                };
                let narrow = ok(self.b.f_convert(f32_, None, quotient))?;
                self.bits(narrow)?
            }
            Number::Float if width == 32 => field,
            Number::Float => {
                let half = if width == 16 {
                    field
                } else {
                    let shift = self.u32c(15 - width);
                    ok(self.b.shift_left_logical(u32_, None, field, shift))?
                };
                self.half_to_single(half, false)?
            }
        })
    }

    fn decode_function(&mut self, id: Word) -> Result<(), EmitError> {
        let (u32_, bool_) = (self.t.u32_, self.t.bool_);
        let function_type = self.b.type_function(u32_, [u32_; 7]);
        ok(self
            .b
            .begin_function(u32_, Some(id), FunctionControl::NONE, function_type))?;
        let params: Vec<Word> = (0..7)
            .map(|_| ok(self.b.function_parameter(u32_)))
            .collect::<Result<_, _>>()?;
        let (format, words, selector, fetch) = (
            params[0],
            [params[1], params[2], params[3], params[4]],
            params[5],
            params[6],
        );
        ok(self.b.begin_block(None))?;
        let four = self.u32c(4);
        let channel = ok(self.b.i_sub(u32_, None, selector, four))?;
        let merge = self.b.id();
        let default = self.b.id();
        let codes: Vec<u32> = (1..=77)
            .filter(|c| rdna::format::buffer_format(*c).is_some())
            .collect();
        let labels: Vec<Word> = codes.iter().map(|_| self.b.id()).collect();
        ok(self.b.selection_merge(merge, SelectionControl::NONE))?;
        let targets: Vec<(Operand, Word)> = codes
            .iter()
            .zip(&labels)
            .map(|(c, l)| (Operand::LiteralBit32(*c), *l))
            .collect();
        ok(self.b.switch(format, default, targets))?;
        let mut values = Vec::new();
        let mut ones = Vec::new();
        for (code, label) in codes.iter().zip(&labels) {
            ok(self.b.begin_block(Some(*label)))?;
            let layout = rdna::format::buffer_format(*code).expect("listed above");
            let mut decoded = Vec::new();
            for index in 0..layout.count {
                let offset = layout.offset(index);
                let (word, shift) = ((offset / 32) as usize, offset % 32);
                let width = u32::from(layout.widths[index]);
                let shift = self.u32c(shift);
                let shifted = ok(self.b.shift_right_logical(u32_, None, words[word], shift))?;
                let field = if width == 32 {
                    shifted
                } else {
                    let mask = self.u32c(u32::MAX >> (32 - width));
                    ok(self.b.bitwise_and(u32_, None, shifted, mask))?
                };
                decoded.push(self.convert_field(&layout, index, field)?);
            }
            let count = self.u32c(layout.count as u32);
            let which = ok(self.b.u_mod(u32_, None, channel, count))?;
            let mut value = decoded[0];
            for (index, candidate) in decoded.iter().enumerate().skip(1) {
                let index = self.u32c(index as u32);
                let hit = ok(self.b.i_equal(bool_, None, which, index))?;
                value = ok(self.b.select(u32_, None, hit, *candidate, value))?;
            }
            let block = *label;
            values.push((value, block));
            ones.push((self.u32c(rdna::format::one(*code)), block));
            ok(self.b.branch(merge))?;
        }
        ok(self.b.begin_block(Some(default)))?;
        ok(self.b.branch(merge))?;
        ok(self.b.begin_block(Some(merge)))?;
        let zero = self.u32c(0);
        let one = self.u32c(1);
        values.push((zero, default));
        ones.push((one, default));
        let value = ok(self.b.phi(u32_, None, values))?;
        let constant = ok(self.b.phi(u32_, None, ones))?;
        let fetched = ok(self.b.i_not_equal(bool_, None, fetch, zero))?;
        let value = ok(self.b.select(u32_, None, fetched, value, zero))?;
        let data = ok(self.b.u_greater_than_equal(bool_, None, selector, four))?;
        let value = ok(self.b.select(u32_, None, data, value, zero))?;
        let is_one = ok(self.b.i_equal(bool_, None, selector, one))?;
        let value = ok(self.b.select(u32_, None, is_one, constant, value))?;
        ok(self.b.ret_value(value))?;
        ok(self.b.end_function())?;
        Ok(())
    }

    fn memory_index(&mut self, address: Word) -> Result<Word, EmitError> {
        let (u32_, u64_) = (self.t.u32_, self.t.u64_);
        let two = self.u32c(2);
        let word = ok(self.b.shift_right_logical(u64_, None, address, two))?;
        let word = ok(self.b.u_convert(u32_, None, word))?;
        let last = self.memory_last;
        self.glsl(u32_, GLOp::UMin, &[word, last])
    }

    fn lds_index(&mut self, address: Word) -> Result<Word, EmitError> {
        let u32_ = self.t.u32_;
        let two = self.u32c(2);
        let word = ok(self.b.shift_right_logical(u32_, None, address, two))?;
        self.masked_shift(word, self.lds_words - 1)
    }

    fn stmt(&mut self, stmt: &Stmt) -> Result<(), EmitError> {
        let v = |e: &Self, t: &u32| e.values[*t as usize];
        match stmt {
            Stmt::Let(tmp, expr) => {
                let id = self.expr(expr)?;
                self.values[*tmp as usize] = id;
                if self.lanes == 64 && matches!(expr, Expr::LdsLoad(_)) {
                    self.barrier()?;
                }
            }
            Stmt::Set(reg, tmp) => {
                let value = v(self, tmp);
                match reg {
                    Reg::S(r) => self.store(self.sgpr_var(*r), value)?,
                    Reg::M0 => self.store(self.sgpr_var(M0), value)?,
                    Reg::V(r) => self.store(self.vgpr[r], value)?,
                    Reg::Exec => self.set_pair(EXEC, value)?,
                    Reg::Vcc => self.set_pair(VCC, value)?,
                    Reg::Scc => self.store(self.scc, value)?,
                }
            }
            Stmt::SetPair(reg, tmp) => self.set_pair(*reg, v(self, tmp))?,
            Stmt::SetLane(reg, tmp) => {
                let active = self.active()?;
                let var = self.vgpr[reg];
                let old = self.load(var)?;
                let u32_ = self.t.u32_;
                let value = ok(self.b.select(u32_, None, active, v(self, tmp), old))?;
                self.store(var, value)?;
            }
            Stmt::WriteLane(reg, lane, value) => {
                let target = self.masked_shift(v(self, lane), self.lanes - 1)?;
                let bool_ = self.t.bool_;
                let hit = ok(self.b.i_equal(bool_, None, target, self.lane))?;
                let var = self.vgpr[reg];
                let old = self.load(var)?;
                let u32_ = self.t.u32_;
                let new = ok(self.b.select(u32_, None, hit, v(self, value), old))?;
                self.store(var, new)?;
            }
            Stmt::Export { .. } | Stmt::Message(..) if self.heap.is_some() => {
                return Err(EmitError::Unsupported(
                    "exports and messages in a compute shader",
                ));
            }
            Stmt::Export { target, values } => {
                let active = self.active()?;
                let values = *values;
                let target = *target;
                let resolved: Vec<Option<Word>> = values
                    .iter()
                    .map(|v| v.map(|t| self.values[t as usize]))
                    .collect();
                self.if_then(active, |e| {
                    let exports = e.exports;
                    for (component, value) in resolved.iter().enumerate() {
                        if let Some(value) = value {
                            let base = e.u32c((u32::from(target) * 4 + component as u32) * 64);
                            let u32_ = e.t.u32_;
                            let index = ok(e.b.i_add(u32_, None, base, e.lane))?;
                            let ptr = e.element(exports, index)?;
                            e.store(ptr, *value)?;
                        }
                    }
                    Ok(())
                })?;
            }
            Stmt::FetchVertexInputs => {}
            Stmt::Message(code, m0) => {
                let m0 = v(self, m0);
                let code = *code;
                let zero = self.u32c(0);
                let bool_ = self.t.bool_;
                let first = ok(self.b.i_equal(bool_, None, self.lane, zero))?;
                let counter = self.messages;
                let count = self.load(counter)?;
                let limit = self.u32c(MESSAGE_LIMIT);
                let room = ok(self.b.u_less_than(bool_, None, count, limit))?;
                let first = ok(self.b.logical_and(bool_, None, first, room))?;
                self.if_then(first, |e| {
                    let u32_ = e.t.u32_;
                    let base = e.u32c(MESSAGE_BASE + 1);
                    let two = e.u32c(2);
                    let at = ok(e.b.i_mul(u32_, None, count, two))?;
                    let at = ok(e.b.i_add(u32_, None, at, base))?;
                    let exports = e.exports;
                    let ptr = e.element(exports, at)?;
                    let code = e.u32c(code);
                    e.store(ptr, code)?;
                    let one = e.u32c(1);
                    let next = ok(e.b.i_add(u32_, None, at, one))?;
                    let ptr = e.element(exports, next)?;
                    e.store(ptr, m0)
                })?;
                let one = self.u32c(1);
                let u32_ = self.t.u32_;
                let count = ok(self.b.i_add(u32_, None, count, one))?;
                self.store(counter, count)?;
            }
            Stmt::LdsStore(address, value) => {
                let active = self.active()?;
                let (address, value) = (v(self, address), v(self, value));
                self.if_then(active, |e| {
                    let index = e.lds_index(address)?;
                    let ptr = e.lds_element(index)?;
                    e.store(ptr, value)
                })?;
                if self.lanes == 64 {
                    self.barrier()?;
                }
            }
            Stmt::Store {
                when,
                address,
                value,
            } => {
                let mut condition = self.active()?;
                if let Some(when) = when {
                    let bool_ = self.t.bool_;
                    condition = ok(self.b.logical_and(bool_, None, condition, v(self, when)))?;
                }
                let (address, value) = (v(self, address), v(self, value));
                self.if_then(condition, |e| {
                    if e.heap.is_some() {
                        return e.heap_store(address, value);
                    }
                    let index = e.memory_index(address)?;
                    let memory = e.memory;
                    let ptr = e.element(memory, index)?;
                    e.store(ptr, value)
                })?;
            }
        }
        Ok(())
    }

    fn expr(&mut self, expr: &Expr) -> Result<Word, EmitError> {
        let v = |e: &Self, t: &u32| e.values[*t as usize];
        let (u32_, u64_, bool_) = (self.t.u32_, self.t.u64_, self.t.bool_);
        Ok(match expr {
            Expr::Const(c) => self.u32c(*c),
            Expr::Const64(c) => self.u64c(*c),
            Expr::Get(reg) => match reg {
                Reg::S(r) => self.load(self.sgpr_var(*r))?,
                Reg::M0 => self.load(self.sgpr_var(M0))?,
                Reg::V(r) => self.load(self.vgpr[r])?,
                Reg::Exec => self.get_pair(EXEC)?,
                Reg::Vcc => self.get_pair(VCC)?,
                Reg::Scc => ok(self.b.load(bool_, None, self.scc, None, []))?,
            },
            Expr::GetPair(r) => self.get_pair(*r)?,
            Expr::Un(op, a) => self.unary(*op, v(self, a))?,
            Expr::Bin(op, a, b) => self.binary(*op, v(self, a), v(self, b))?,
            Expr::Tri(op, a, b, c) => self.ternary(*op, v(self, a), v(self, b), v(self, c))?,
            Expr::Cmp(op, a, b) => self.compare(*op, v(self, a), v(self, b))?,
            Expr::Select(c, a, b) => {
                let ty = match self.types[*a as usize] {
                    Ty::Bool => bool_,
                    Ty::U32 => u32_,
                    Ty::U64 => u64_,
                };
                ok(self.b.select(ty, None, v(self, c), v(self, a), v(self, b)))?
            }
            Expr::LaneId => self.lane,
            Expr::CodeBase => match self.code {
                Some(code) => code,
                None => self.u64c(self.code_base),
            },
            Expr::Active => {
                if self.lanes == 64 {
                    self.get_pair(EXEC)?
                } else {
                    let exec = self.load(self.sgpr_var(EXEC))?;
                    ok(self.b.u_convert(u64_, None, exec))?
                }
            }
            Expr::Ballot(a) => {
                let active = self.active()?;
                let both = ok(self.b.logical_and(bool_, None, v(self, a), active))?;
                let scope = self.u32c(3);
                let uvec4 = self.t.uvec4;
                let ballot = ok(self.b.group_non_uniform_ballot(uvec4, None, scope, both))?;
                let mine = ok(self.b.composite_extract(u32_, None, ballot, [0]))?;
                if self.lanes == 32 {
                    ok(self.b.u_convert(u64_, None, mine))?
                } else {
                    let zero = self.u32c(0);
                    let first = ok(self.b.i_equal(bool_, None, self.local, zero))?;
                    let half = self.half;
                    self.if_then(first, |e| {
                        let slot = e.exchange_slot(half)?;
                        e.store(slot, mine)
                    })?;
                    self.barrier()?;
                    let one = self.u32c(1);
                    let low = self.exchange_slot(zero)?;
                    let low = self.load(low)?;
                    let high = self.exchange_slot(one)?;
                    let high = self.load(high)?;
                    self.barrier()?;
                    self.pack(low, high)?
                }
            }
            Expr::Shuffle(source, lane) => {
                let lane = self.masked_shift(v(self, lane), self.lanes - 1)?;
                let source = v(self, source);
                if self.lanes == 32 {
                    let scope = self.u32c(3);
                    ok(self
                        .b
                        .group_non_uniform_shuffle(u32_, None, scope, source, lane))?
                } else {
                    let mine = self.lane;
                    let slot = self.exchange_slot(mine)?;
                    self.store(slot, source)?;
                    self.barrier()?;
                    let slot = self.exchange_slot(lane)?;
                    let value = self.load(slot)?;
                    self.barrier()?;
                    value
                }
            }
            Expr::GetRelative(base, index) => {
                let base_value = self.u32c(u32::from(*base));
                let at = ok(self.b.i_add(u32_, None, base_value, v(self, index)))?;
                let limit = self.u32c(256);
                let inside = ok(self.b.u_less_than(bool_, None, at, limit))?;
                let zero = self.u32c(0);
                let at = ok(self.b.select(u32_, None, inside, at, zero))?;
                let first = self.vgpr[&0];
                let mut value = self.load(first)?;
                for reg in *base..=255u8 {
                    if reg == 0 {
                        continue;
                    }
                    let candidate = self.vgpr[&reg];
                    let candidate = self.load(candidate)?;
                    let number = self.u32c(u32::from(reg));
                    let hit = ok(self.b.i_equal(bool_, None, at, number))?;
                    value = ok(self.b.select(u32_, None, hit, candidate, value))?;
                }
                value
            }
            Expr::ReadLane(source, lane) if self.lanes == 64 => {
                let target = self.masked_shift(v(self, lane), 63)?;
                let hit = ok(self.b.i_equal(bool_, None, target, self.lane))?;
                let source = v(self, source);
                let zero = self.u32c(0);
                self.if_then(hit, |e| {
                    let slot = e.exchange_slot(zero)?;
                    e.store(slot, source)
                })?;
                self.barrier()?;
                let slot = self.exchange_slot(zero)?;
                let value = self.load(slot)?;
                self.barrier()?;
                value
            }
            Expr::ReadLane(source, lane) => {
                let lane = self.masked_shift(v(self, lane), 31)?;
                let scope = self.u32c(3);
                ok(self
                    .b
                    .group_non_uniform_shuffle(u32_, None, scope, v(self, source), lane))?
            }
            Expr::FirstActiveLane => {
                let exec = self.load(self.sgpr_var(EXEC))?;
                let lowest = self.glsl(u32_, GLOp::FindILsb, &[exec])?;
                let zero = self.u32c(0);
                let none = ok(self.b.i_equal(bool_, None, exec, zero))?;
                if self.lanes == 32 {
                    ok(self.b.select(u32_, None, none, zero, lowest))?
                } else {
                    let high = self.load(self.sgpr_var(EXEC + 1))?;
                    let upper = self.glsl(u32_, GLOp::FindILsb, &[high])?;
                    let thirty_two = self.u32c(32);
                    let upper = ok(self.b.i_add(u32_, None, upper, thirty_two))?;
                    let high_none = ok(self.b.i_equal(bool_, None, high, zero))?;
                    let upper = ok(self.b.select(u32_, None, high_none, zero, upper))?;
                    ok(self.b.select(u32_, None, none, upper, lowest))?
                }
            }
            Expr::LdsLoad(address) => {
                let index = self.lds_index(v(self, address))?;
                let ptr = self.lds_element(index)?;
                let value = self.load(ptr)?;
                let active = self.active()?;
                let zero = self.u32c(0);
                ok(self.b.select(u32_, None, active, value, zero))?
            }
            Expr::Load { when, address } if self.heap.is_some() => {
                let mut condition = self.active()?;
                if let Some(when) = when {
                    condition = ok(self.b.logical_and(bool_, None, condition, v(self, when)))?;
                }
                self.guarded_loads(condition, &[v(self, address)])?[0]
            }
            Expr::ScalarLoad(address) if self.heap.is_some() => self.heap_load(v(self, address))?,
            Expr::Load { when, address } => {
                let index = self.memory_index(v(self, address))?;
                let memory = self.memory;
                let ptr = self.element(memory, index)?;
                let value = self.load(ptr)?;
                let mut condition = self.active()?;
                if let Some(when) = when {
                    condition = ok(self.b.logical_and(bool_, None, condition, v(self, when)))?;
                }
                let zero = self.u32c(0);
                ok(self.b.select(u32_, None, condition, value, zero))?
            }
            Expr::TypedLoad {
                format,
                address,
                in_range,
                select,
                component,
            } => {
                let selector = match select {
                    Some(word) => {
                        let shift = self.u32c(3 * u32::from(*component));
                        let shifted =
                            ok(self.b.shift_right_logical(u32_, None, v(self, word), shift))?;
                        self.masked_shift(shifted, 7)?
                    }
                    None => self.u32c(4 + u32::from(*component)),
                };
                let address = v(self, address);
                let active = self.active()?;
                let fetch = ok(self.b.logical_and(bool_, None, active, v(self, in_range)))?;
                let words = if self.heap.is_some() {
                    let low = ok(self.b.u_convert(u32_, None, address))?;
                    let three = self.u32c(3);
                    let misaligned = ok(self.b.bitwise_and(u32_, None, low, three))?;
                    let shift = ok(self.b.shift_left_logical(u32_, None, misaligned, three))?;
                    let mask = self.u64c(!3);
                    let aligned = ok(self.b.bitwise_and(u64_, None, address, mask))?;
                    let mut addresses = Vec::new();
                    for k in 0..5u64 {
                        let step = self.u64c(4 * k);
                        addresses.push(ok(self.b.i_add(u64_, None, aligned, step))?);
                    }
                    let dwords = self.guarded_loads(fetch, &addresses)?;
                    let mut words = [0; 4];
                    for (k, word) in words.iter_mut().enumerate() {
                        let pair = self.pack(dwords[k], dwords[k + 1])?;
                        let moved = ok(self.b.shift_right_logical(u64_, None, pair, shift))?;
                        *word = ok(self.b.u_convert(u32_, None, moved))?;
                    }
                    words
                } else {
                    self.element_words(address)?
                };
                let (one, zero) = (self.u32c(1), self.u32c(0));
                let fetch = ok(self.b.select(u32_, None, fetch, one, zero))?;
                let decode = match self.decode {
                    Some(id) => id,
                    None => {
                        let id = self.b.id();
                        self.decode = Some(id);
                        self.b.capability(Capability::Float64);
                        id
                    }
                };
                let format = v(self, format);
                let arguments = [
                    format, words[0], words[1], words[2], words[3], selector, fetch,
                ];
                let value = ok(self.b.function_call(u32_, None, decode, arguments))?;
                let zero = self.u32c(0);
                ok(self.b.select(u32_, None, active, value, zero))?
            }
            Expr::ScalarLoad(address) => {
                let index = self.memory_index(v(self, address))?;
                let memory = self.memory;
                let ptr = self.element(memory, index)?;
                self.load(ptr)?
            }
        })
    }

    fn float_unary(&mut self, op: GLOp, a: Word) -> Result<Word, EmitError> {
        let f32_ = self.t.f32_;
        let x = self.float(a)?;
        let r = self.glsl(f32_, op, &[x])?;
        self.bits(r)
    }

    fn saturate(
        &mut self,
        a: Word,
        low: f32,
        high: f32,
        top: u32,
        signed: bool,
    ) -> Result<Word, EmitError> {
        let (f32_, u32_, bool_) = (self.t.f32_, self.t.u32_, self.t.bool_);
        let x = self.float(a)?;
        let (lo, hi) = (self.f32c(low), self.f32c(high));
        let clamped = self.glsl(f32_, GLOp::NMax, &[x, lo])?;
        let clamped = self.glsl(f32_, GLOp::NMin, &[clamped, hi])?;
        let converted = if signed {
            ok(self.b.convert_f_to_s(u32_, None, clamped))?
        } else {
            ok(self.b.convert_f_to_u(u32_, None, clamped))?
        };
        let limit = self.f32c(if signed {
            2_147_483_648.0
        } else {
            4_294_967_296.0
        });
        let over = ok(self.b.f_ord_greater_than_equal(bool_, None, x, limit))?;
        let top = self.u32c(top);
        ok(self.b.select(u32_, None, over, top, converted))
    }

    fn unary(&mut self, op: Un, a: Word) -> Result<Word, EmitError> {
        let (u32_, u64_, f32_, bool_) = (self.t.u32_, self.t.u64_, self.t.f32_, self.t.bool_);
        Ok(match op {
            Un::Not => ok(self.b.not(u32_, None, a))?,
            Un::F16ToF32 => {
                let mask = self.u32c(0xFFFF);
                let half = ok(self.b.bitwise_and(u32_, None, a, mask))?;
                self.half_to_single(half, true)?
            }
            Un::F32ToF16 => self.single_to_half(a)?,
            Un::Not64 => ok(self.b.not(u64_, None, a))?,
            Un::Bfrev => ok(self.b.bit_reverse(u32_, None, a))?,
            Un::Ffbl => self.glsl(u32_, GLOp::FindILsb, &[a])?,
            Un::Ffbh => {
                let msb = self.glsl(u32_, GLOp::FindUMsb, &[a])?;
                let thirty_one = self.u32c(31);
                let zeros = ok(self.b.i_sub(u32_, None, thirty_one, msb))?;
                let zero = self.u32c(0);
                let empty = ok(self.b.i_equal(bool_, None, a, zero))?;
                let all = self.u32c(u32::MAX);
                ok(self.b.select(u32_, None, empty, all, zeros))?
            }
            Un::Popcount => ok(self.b.bit_count(u32_, None, a))?,
            Un::Popcount64 => {
                let (low, high) = self.split(a)?;
                let low = ok(self.b.bit_count(u32_, None, low))?;
                let high = ok(self.b.bit_count(u32_, None, high))?;
                ok(self.b.i_add(u32_, None, low, high))?
            }
            Un::Lo => ok(self.b.u_convert(u32_, None, a))?,
            Un::Hi => self.split(a)?.1,
            Un::Zext => ok(self.b.u_convert(u64_, None, a))?,
            Un::BoolToU32 => {
                let (one, zero) = (self.u32c(1), self.u32c(0));
                ok(self.b.select(u32_, None, a, one, zero))?
            }
            Un::CvtF32U32 => {
                let f = ok(self.b.convert_u_to_f(f32_, None, a))?;
                self.bits(f)?
            }
            Un::CvtF32I32 => {
                let f = ok(self.b.convert_s_to_f(f32_, None, a))?;
                self.bits(f)?
            }
            Un::CvtU32F32 => self.saturate(a, 0.0, 4_294_967_040.0, u32::MAX, false)?,
            Un::CvtI32F32 => {
                self.saturate(a, -2_147_483_648.0, 2_147_483_520.0, i32::MAX as u32, true)?
            }
            Un::FNeg => {
                let sign = self.u32c(0x8000_0000);
                ok(self.b.bitwise_xor(u32_, None, a, sign))?
            }
            Un::FAbs => {
                let magnitude = self.u32c(0x7FFF_FFFF);
                ok(self.b.bitwise_and(u32_, None, a, magnitude))?
            }
            Un::FClamp01 => {
                let x = self.float(a)?;
                let (zero, one) = (self.f32c(0.0), self.f32c(1.0));
                let positive = ok(self.b.f_ord_greater_than(bool_, None, x, zero))?;
                let capped = self.glsl(f32_, GLOp::NMin, &[x, one])?;
                let r = ok(self.b.select(f32_, None, positive, capped, zero))?;
                self.bits(r)?
            }
            Un::FRcp | Un::FRsq => {
                let x = self.float(a)?;
                let r = if op == Un::FRcp {
                    let one = self.f32c(1.0);
                    ok(self.b.f_div(f32_, None, one, x))?
                } else {
                    self.glsl(f32_, GLOp::InverseSqrt, &[x])?
                };
                self.bits(r)?
            }
            Un::FSqrt => self.float_unary(GLOp::Sqrt, a)?,
            Un::FExp2 => self.float_unary(GLOp::Exp2, a)?,
            Un::FLog2 => self.float_unary(GLOp::Log2, a)?,
            Un::FFloor => self.float_unary(GLOp::Floor, a)?,
            Un::FCeil => self.float_unary(GLOp::Ceil, a)?,
            Un::FTrunc => self.float_unary(GLOp::Trunc, a)?,
            Un::FRndne => self.float_unary(GLOp::RoundEven, a)?,
            Un::FSin | Un::FCos => {
                let x = self.float(a)?;
                let tau = self.f32c(std::f32::consts::TAU);
                let turns = ok(self.b.f_mul(f32_, None, x, tau))?;
                let turns = self.exact(turns);
                let op = if op == Un::FSin { GLOp::Sin } else { GLOp::Cos };
                let r = self.glsl(f32_, op, &[turns])?;
                self.bits(r)?
            }
            Un::FFract => {
                let x = self.float(a)?;
                let floor = self.glsl(f32_, GLOp::Floor, &[x])?;
                let r = ok(self.b.f_sub(f32_, None, x, floor))?;
                let r = self.exact(r);
                self.bits(r)?
            }
        })
    }

    fn float_binary(&mut self, op: Bin, a: Word, b: Word) -> Result<Word, EmitError> {
        let f32_ = self.t.f32_;
        let (x, y) = (self.float(a)?, self.float(b)?);
        let r = match op {
            Bin::FAdd => ok(self.b.f_add(f32_, None, x, y))?,
            Bin::FSub => ok(self.b.f_sub(f32_, None, x, y))?,
            Bin::FMul => ok(self.b.f_mul(f32_, None, x, y))?,
            Bin::FMin => self.glsl(f32_, GLOp::NMin, &[x, y])?,
            _ => self.glsl(f32_, GLOp::NMax, &[x, y])?,
        };
        let r = self.exact(r);
        self.bits(r)
    }

    fn binary(&mut self, op: Bin, a: Word, b: Word) -> Result<Word, EmitError> {
        let (u32_, u64_, bool_) = (self.t.u32_, self.t.u64_, self.t.bool_);
        Ok(match op {
            Bin::Add => ok(self.b.i_add(u32_, None, a, b))?,
            Bin::Sub => ok(self.b.i_sub(u32_, None, a, b))?,
            Bin::Mul => ok(self.b.i_mul(u32_, None, a, b))?,
            Bin::MulHiU => {
                let wide = self.t.wide;
                let both = ok(self.b.u_mul_extended(wide, None, a, b))?;
                ok(self.b.composite_extract(u32_, None, both, [1]))?
            }
            Bin::And => ok(self.b.bitwise_and(u32_, None, a, b))?,
            Bin::Or => ok(self.b.bitwise_or(u32_, None, a, b))?,
            Bin::Xor => ok(self.b.bitwise_xor(u32_, None, a, b))?,
            Bin::Shl | Bin::ShrU | Bin::ShrS => {
                let amount = self.masked_shift(b, 31)?;
                match op {
                    Bin::Shl => ok(self.b.shift_left_logical(u32_, None, a, amount))?,
                    Bin::ShrU => ok(self.b.shift_right_logical(u32_, None, a, amount))?,
                    _ => ok(self.b.shift_right_arithmetic(u32_, None, a, amount))?,
                }
            }
            Bin::MinU => self.glsl(u32_, GLOp::UMin, &[a, b])?,
            Bin::MaxU => self.glsl(u32_, GLOp::UMax, &[a, b])?,
            Bin::MinS => self.glsl(u32_, GLOp::SMin, &[a, b])?,
            Bin::MaxS => self.glsl(u32_, GLOp::SMax, &[a, b])?,
            Bin::FAdd | Bin::FSub | Bin::FMul | Bin::FMin | Bin::FMax => {
                self.float_binary(op, a, b)?
            }
            Bin::Add64 => ok(self.b.i_add(u64_, None, a, b))?,
            Bin::Sub64 => ok(self.b.i_sub(u64_, None, a, b))?,
            Bin::And64 => ok(self.b.bitwise_and(u64_, None, a, b))?,
            Bin::Or64 => ok(self.b.bitwise_or(u64_, None, a, b))?,
            Bin::Xor64 => ok(self.b.bitwise_xor(u64_, None, a, b))?,
            Bin::Shl64 | Bin::ShrU64 => {
                let mask = self.u64c(63);
                let amount = ok(self.b.bitwise_and(u64_, None, b, mask))?;
                if op == Bin::Shl64 {
                    ok(self.b.shift_left_logical(u64_, None, a, amount))?
                } else {
                    ok(self.b.shift_right_logical(u64_, None, a, amount))?
                }
            }
            Bin::Pack64 => self.pack(a, b)?,
            Bin::LaneBit => {
                let amount = self.masked_shift(b, 63)?;
                let shifted = ok(self.b.shift_right_logical(u64_, None, a, amount))?;
                let one = self.u64c(1);
                let bit = ok(self.b.bitwise_and(u64_, None, shifted, one))?;
                let zero = self.u64c(0);
                ok(self.b.i_not_equal(bool_, None, bit, zero))?
            }
        })
    }

    fn ternary(&mut self, op: Tri, a: Word, b: Word, c: Word) -> Result<Word, EmitError> {
        let (u32_, f32_, bool_) = (self.t.u32_, self.t.f32_, self.t.bool_);
        Ok(match op {
            Tri::FFma => {
                let (x, y, z) = (self.float(a)?, self.float(b)?, self.float(c)?);
                let r = self.glsl(f32_, GLOp::Fma, &[x, y, z])?;
                self.bits(r)?
            }
            Tri::FMad => {
                let product = self.float_binary(Bin::FMul, a, b)?;
                self.float_binary(Bin::FAdd, product, c)?
            }
            Tri::BfeU => {
                let width = self.masked_shift(c, 31)?;
                let offset = self.masked_shift(b, 31)?;
                let shifted = ok(self.b.shift_right_logical(u32_, None, a, offset))?;
                let thirty_two = self.u32c(32);
                let spare = ok(self.b.i_sub(u32_, None, thirty_two, width))?;
                let spare = self.masked_shift(spare, 31)?;
                let all = self.u32c(u32::MAX);
                let mask = ok(self.b.shift_right_logical(u32_, None, all, spare))?;
                let field = ok(self.b.bitwise_and(u32_, None, shifted, mask))?;
                let zero = self.u32c(0);
                let empty = ok(self.b.i_equal(bool_, None, width, zero))?;
                ok(self.b.select(u32_, None, empty, zero, field))?
            }
        })
    }

    fn class(&mut self, bits: Word, mask: Word) -> Result<Word, EmitError> {
        let (u32_, bool_) = (self.t.u32_, self.t.bool_);
        let c = |e: &mut Self, v: u32| e.u32c(v);
        let thirty_one = c(self, 31);
        let sign = ok(self.b.shift_right_logical(u32_, None, bits, thirty_one))?;
        let zero = c(self, 0);
        let negative = ok(self.b.i_not_equal(bool_, None, sign, zero))?;
        let twenty_three = c(self, 23);
        let exponent = ok(self.b.shift_right_logical(u32_, None, bits, twenty_three))?;
        let byte = c(self, 0xFF);
        let exponent = ok(self.b.bitwise_and(u32_, None, exponent, byte))?;
        let fraction = c(self, 0x7F_FFFF);
        let mantissa = ok(self.b.bitwise_and(u32_, None, bits, fraction))?;
        let quiet_bit = c(self, 0x40_0000);
        let quiet = ok(self.b.bitwise_and(u32_, None, bits, quiet_bit))?;
        let quiet = ok(self.b.i_not_equal(bool_, None, quiet, zero))?;
        let top = ok(self.b.i_equal(bool_, None, exponent, byte))?;
        let bottom = ok(self.b.i_equal(bool_, None, exponent, zero))?;
        let empty = ok(self.b.i_equal(bool_, None, mantissa, zero))?;
        let pick = |e: &mut Self, when: Word, yes: u32, no: u32| -> Result<Word, EmitError> {
            let (yes, no) = (e.u32c(yes), e.u32c(no));
            ok(e.b.select(u32_, None, when, yes, no))
        };
        let normal = pick(self, negative, 3, 8)?;
        let denormal = pick(self, negative, 4, 7)?;
        let zero_class = pick(self, negative, 5, 6)?;
        let infinity = pick(self, negative, 2, 9)?;
        let nan = pick(self, quiet, 1, 0)?;
        let small = ok(self.b.select(u32_, None, empty, zero_class, denormal))?;
        let large = ok(self.b.select(u32_, None, empty, infinity, nan))?;
        let index = ok(self.b.select(u32_, None, bottom, small, normal))?;
        let index = ok(self.b.select(u32_, None, top, large, index))?;
        let shifted = ok(self.b.shift_right_logical(u32_, None, mask, index))?;
        let one = c(self, 1);
        let bit = ok(self.b.bitwise_and(u32_, None, shifted, one))?;
        ok(self.b.i_not_equal(bool_, None, bit, zero))
    }

    fn compare(&mut self, op: Cmp, a: Word, b: Word) -> Result<Word, EmitError> {
        let bool_ = self.t.bool_;
        let b_ = &mut self.b;
        let integer = match op {
            Cmp::Eq | Cmp::Eq64 => Some(ok(b_.i_equal(bool_, None, a, b))?),
            Cmp::Ne | Cmp::Ne64 => Some(ok(b_.i_not_equal(bool_, None, a, b))?),
            Cmp::LtU => Some(ok(b_.u_less_than(bool_, None, a, b))?),
            Cmp::LeU => Some(ok(b_.u_less_than_equal(bool_, None, a, b))?),
            Cmp::GtU => Some(ok(b_.u_greater_than(bool_, None, a, b))?),
            Cmp::GeU => Some(ok(b_.u_greater_than_equal(bool_, None, a, b))?),
            Cmp::LtS => Some(ok(b_.s_less_than(bool_, None, a, b))?),
            Cmp::LeS => Some(ok(b_.s_less_than_equal(bool_, None, a, b))?),
            Cmp::GtS => Some(ok(b_.s_greater_than(bool_, None, a, b))?),
            Cmp::GeS => Some(ok(b_.s_greater_than_equal(bool_, None, a, b))?),
            Cmp::False => return Ok(self.boolc(false)),
            Cmp::True => return Ok(self.boolc(true)),
            Cmp::Class => return self.class(a, b),
            _ => None,
        };
        if let Some(result) = integer {
            return Ok(result);
        }
        let (x, y) = (self.float(a)?, self.float(b)?);
        let b_ = &mut self.b;
        Ok(match op {
            Cmp::FLt => ok(b_.f_ord_less_than(bool_, None, x, y))?,
            Cmp::FEq => ok(b_.f_ord_equal(bool_, None, x, y))?,
            Cmp::FLe => ok(b_.f_ord_less_than_equal(bool_, None, x, y))?,
            Cmp::FGt => ok(b_.f_ord_greater_than(bool_, None, x, y))?,
            Cmp::FLg => ok(b_.f_ord_not_equal(bool_, None, x, y))?,
            Cmp::FGe => ok(b_.f_ord_greater_than_equal(bool_, None, x, y))?,
            Cmp::FNge => ok(b_.f_unord_less_than(bool_, None, x, y))?,
            Cmp::FNlg => ok(b_.f_unord_equal(bool_, None, x, y))?,
            Cmp::FNgt => ok(b_.f_unord_less_than_equal(bool_, None, x, y))?,
            Cmp::FNle => ok(b_.f_unord_greater_than(bool_, None, x, y))?,
            Cmp::FNeq => ok(b_.f_unord_not_equal(bool_, None, x, y))?,
            Cmp::FNlt => ok(b_.f_unord_greater_than_equal(bool_, None, x, y))?,
            Cmp::FO | Cmp::FU => {
                let nan_x = ok(b_.is_nan(bool_, None, x))?;
                let nan_y = ok(b_.is_nan(bool_, None, y))?;
                let either = ok(b_.logical_or(bool_, None, nan_x, nan_y))?;
                if op == Cmp::FU {
                    either
                } else {
                    ok(b_.logical_not(bool_, None, either))?
                }
            }
            _ => unreachable!("integer comparisons handled above"),
        })
    }
}
