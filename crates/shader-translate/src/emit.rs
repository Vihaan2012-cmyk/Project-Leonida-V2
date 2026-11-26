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
}

fn ok<T>(result: Result<T, rspirv::dr::Error>) -> Result<T, EmitError> {
    result.map_err(|e| EmitError::Build(format!("{e:?}")))
}

#[derive(Debug, Clone, Copy)]
pub struct HarnessOptions {
    pub lds_words: u32,
    pub nan_preserve: bool,
    pub flush_denormals: bool,
    pub preserve_denormals: bool,
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
    lds: Word,
    lds_words: u32,
    memory_last: Word,
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
                _ => {}
            }
        }
    }
    (scalars, vectors)
}

pub fn harness(function: &Function, options: &HarnessOptions) -> Result<Vec<u32>, EmitError> {
    let lanes: u32 = if function.wave64 { 64 } else { 32 };
    if !options.lds_words.is_power_of_two() {
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
    if options.nan_preserve {
        b.capability(Capability::SignedZeroInfNanPreserve);
    }
    if options.flush_denormals {
        b.capability(Capability::DenormFlushToZero);
    }
    if options.preserve_denormals {
        b.capability(Capability::DenormPreserve);
    }
    if options.nan_preserve || options.flush_denormals || options.preserve_denormals {
        b.extension("SPV_KHR_float_controls");
    }
    let glsl = b.ext_inst_import("GLSL.std.450");
    b.memory_model(AddressingModel::Logical, MemoryModel::GLSL450);
    let bool_ = b.type_bool();
    let u32_ = b.type_int(32, 0);
    let u64_ = b.type_int(64, 0);
    let f32_ = b.type_float(32, None);
    let uvec4 = b.type_vector(u32_, 4);
    let wide = b.type_struct([u32_, u32_]);
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
    let state = buffer(&mut b, 0);
    let memory = buffer(&mut b, 1);
    let lds_image = buffer(&mut b, 2);
    let lds_len = b.constant_bit32(u32_, options.lds_words);
    let lds_array = b.type_array(u32_, lds_len);
    let ptr_lds = b.type_pointer(None, StorageClass::Workgroup, lds_array);
    let lds = b.variable(ptr_lds, None, StorageClass::Workgroup, None);
    let ptr_input_u32 = b.type_pointer(None, StorageClass::Input, u32_);
    let lane_input = b.variable(ptr_input_u32, None, StorageClass::Input, None);
    b.decorate(
        lane_input,
        Decoration::BuiltIn,
        [Operand::BuiltIn(BuiltIn::SubgroupLocalInvocationId)],
    );
    let half_input = b.variable(ptr_input_u32, None, StorageClass::Input, None);
    b.decorate(
        half_input,
        Decoration::BuiltIn,
        [Operand::BuiltIn(BuiltIn::SubgroupId)],
    );
    let two = b.constant_bit32(u32_, 2);
    let exchange_array = b.type_array(u32_, two);
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
    };
    let void = b.type_void();
    let function_type = b.type_function(void, []);
    let main = ok(b.begin_function(void, None, FunctionControl::NONE, function_type))?;
    ok(b.begin_block(None))?;
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
    let local = ok(b.load(u32_, None, lane_input, None, []))?;
    let (lane, half) = if function.wave64 {
        let half = ok(b.load(u32_, None, half_input, None, []))?;
        let shift = b.constant_bit32(u32_, 5);
        let base = ok(b.shift_left_logical(u32_, None, half, shift))?;
        (ok(b.i_add(u32_, None, base, local))?, half)
    } else {
        (local, local)
    };
    let interface = if function.wave64 {
        vec![lane_input, half_input]
    } else {
        vec![lane_input]
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
        lds,
        lds_words: options.lds_words,
        memory_last: 0,
        values: vec![0; function.temps as usize],
        types,
    };
    e.prologue()?;
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
    e.epilogue()?;
    ok(e.b.ret())?;
    ok(e.b.end_function())?;
    e.b.entry_point(ExecutionModel::GLCompute, main, "main", interface);
    e.b.execution_mode(main, ExecutionMode::LocalSize, [lanes, 1, 1]);
    if options.nan_preserve {
        e.b.execution_mode(main, ExecutionMode::SignedZeroInfNanPreserve, [32]);
    }
    if options.flush_denormals {
        e.b.execution_mode(main, ExecutionMode::DenormFlushToZero, [32]);
    }
    if options.preserve_denormals {
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
        ok(self.b.begin_block(Some(then)))?;
        body(self)?;
        ok(self.b.branch(merge))?;
        ok(self.b.begin_block(Some(merge)))?;
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
            e.store(ptr, value)
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
        ok(self.b.begin_block(Some(header)))?;
        ok(self.b.loop_merge(merge, cont, LoopControl::NONE, []))?;
        ok(self.b.branch(body))?;
        ok(self.b.begin_block(Some(body)))?;
        let selector = self.load(current)?;
        ok(self.b.selection_merge(switch_merge, SelectionControl::NONE))?;
        let targets: Vec<(Operand, Word)> = cases
            .iter()
            .enumerate()
            .map(|(i, label)| (Operand::LiteralBit32(i as u32), *label))
            .collect();
        ok(self.b.switch(selector, default, targets))?;
        for (block, label) in function.blocks.iter().zip(&cases) {
            ok(self.b.begin_block(Some(*label)))?;
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
        ok(self.b.begin_block(Some(default)))?;
        let end = self.u32c(END);
        self.store(current, end)?;
        ok(self.b.branch(switch_merge))?;
        ok(self.b.begin_block(Some(switch_merge)))?;
        ok(self.b.branch(cont))?;
        ok(self.b.begin_block(Some(cont)))?;
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
        ok(self.b.begin_block(Some(merge)))?;
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
        ok(self.b.access_chain(ptr, None, exchange, [index]))
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
            Stmt::LdsStore(address, value) => {
                let active = self.active()?;
                let (address, value) = (v(self, address), v(self, value));
                self.if_then(active, |e| {
                    let index = e.lds_index(address)?;
                    let ptr = e.lds_element(index)?;
                    e.store(ptr, value)
                })?;
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
