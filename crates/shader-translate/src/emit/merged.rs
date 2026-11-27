use std::collections::BTreeMap;

use rspirv::dr::{Builder, Operand};
use rspirv::spirv::{BuiltIn, Capability, Decoration, StorageClass, Word};

use super::{EXEC, EmitError, Emitter, EsOptions, GsOptions, ok};

const MSG_GS: u32 = 2;
const GS_CUT: u32 = 1;
const GS_EMIT: u32 = 2;

/// LDS for the two halves of a merged vertex-and-geometry shader, private to each invocation. The first half's
/// stores are its vertex's outputs; the second half loads its primitive's vertices and stores the vertices it
/// emits. Addresses count from the lane's own area, `lane_bytes` per lane.
#[derive(Debug, Clone, Copy)]
pub(super) struct StageLds {
    loads: Word,
    load_words: u32,
    load_lane_bytes: u32,
    stores: Word,
    store_words: u32,
    store_lane_bytes: u32,
}

pub(super) struct MergedState {
    interface: Vec<Word>,
    /// The first half's outputs, one per 16 bytes of its vertex.
    outputs: Vec<Word>,
    /// The second half's outputs by output register, and the built-in position's register.
    registers: Vec<(u8, Word)>,
    position: Option<(u8, Word)>,
    vertex_words: u32,
    emitted: Word,
}

impl MergedState {
    pub(super) fn second_half(&self) -> bool {
        self.vertex_words != 0
    }

    pub(super) fn interface(&self) -> Vec<Word> {
        let mut list = self.interface.clone();
        list.extend(&self.outputs);
        list.extend(self.registers.iter().map(|(_, var)| *var));
        list.extend(self.position.map(|(_, var)| var));
        list
    }
}

fn private_array(b: &mut Builder, u32_: Word, words: u32) -> Word {
    let length = b.constant_bit32(u32_, words);
    let array = b.type_array(u32_, length);
    let pointer = b.type_pointer(None, StorageClass::Function, array);
    b.variable(pointer, None, StorageClass::Function, None)
}

fn window(bytes: u32) -> u32 {
    (bytes / 4).max(4).next_power_of_two()
}

/// Declares the private LDS of a merged half; it must run among the function's first variables.
pub(super) fn first_half_lds(b: &mut Builder, u32_: Word, options: &EsOptions) -> StageLds {
    let words = window(options.item_bytes);
    let array = private_array(b, u32_, words);
    StageLds {
        loads: array,
        load_words: words,
        load_lane_bytes: options.item_bytes,
        stores: array,
        store_words: words,
        store_lane_bytes: options.item_bytes,
    }
}

pub(super) fn second_half_lds(b: &mut Builder, u32_: Word, options: &GsOptions) -> StageLds {
    let load_words = window(options.item_bytes * options.input.vertices());
    let store_words = window(options.vertex_bytes * options.max_vertices);
    StageLds {
        loads: private_array(b, u32_, load_words),
        load_words,
        load_lane_bytes: 0,
        stores: private_array(b, u32_, store_words),
        store_words,
        store_lane_bytes: options.vertex_bytes * options.max_vertices,
    }
}

impl Emitter {
    /// The private word an LDS address reaches: its offset from the lane's area, wrapped to the array.
    pub(super) fn merged_lds_element(
        &mut self,
        address: Word,
        store: bool,
    ) -> Result<Word, EmitError> {
        let stage = self.stage_lds.expect("merged half");
        let (array, words, lane_bytes) = if store {
            (stage.stores, stage.store_words, stage.store_lane_bytes)
        } else {
            (stage.loads, stage.load_words, stage.load_lane_bytes)
        };
        let u32_ = self.t.u32_;
        let stride = self.u32c(lane_bytes);
        let area = ok(self.b.i_mul(u32_, None, self.lane, stride))?;
        let offset = ok(self.b.i_sub(u32_, None, address, area))?;
        let two = self.u32c(2);
        let word = ok(self.b.shift_right_logical(u32_, None, offset, two))?;
        let mask = self.u32c(words - 1);
        let index = ok(self.b.bitwise_and(u32_, None, word, mask))?;
        let pointer = self.t.ptr_function_u32;
        ok(self.b.access_chain(pointer, None, array, [index]))
    }

    fn private_word(&mut self, array: Word, index: u32) -> Result<Word, EmitError> {
        let pointer = self.t.ptr_function_u32;
        let index = self.u32c(index);
        ok(self.b.access_chain(pointer, None, array, [index]))
    }

    /// Exec from every invocation present, and how many there are.
    fn present(&mut self) -> Result<(Word, Word), EmitError> {
        let u32_ = self.t.u32_;
        let yes = self.boolc(true);
        let scope = self.u32c(3);
        let uvec4 = self.t.uvec4;
        let ballot = ok(self.b.group_non_uniform_ballot(uvec4, None, scope, yes))?;
        let exec = ok(self.b.composite_extract(u32_, None, ballot, [0]))?;
        let count = ok(self.b.bit_count(u32_, None, exec))?;
        Ok((exec, count))
    }

    /// User SGPRs from s8, merged wave information in s3, exec from the invocations present.
    fn merged_scalars(
        &mut self,
        user_sgprs: u32,
        wave_info: Word,
        exec: Word,
    ) -> Result<(), EmitError> {
        let scalars: Vec<(u8, Word)> = self.sgpr.iter().map(|(r, v)| (*r, *v)).collect();
        for (reg, var) in scalars {
            let index = u32::from(reg);
            let value = if (8..8 + user_sgprs.min(32)).contains(&index) {
                self.push_word(index - 8)?
            } else if reg == 3 {
                wave_info
            } else if reg == EXEC {
                exec
            } else {
                self.u32c(0)
            };
            self.store(var, value)?;
        }
        Ok(())
    }

    /// The first half as a vertex shader: v5 holds the vertex index and v8 the instance index, fetched
    /// attribute r fills v[9 + 4r] to v[12 + 4r], and s3 carries the vertex count in bits 0-7.
    pub(super) fn first_half_prologue(&mut self, options: &EsOptions) -> Result<(), EmitError> {
        self.push_bases()?;
        let mut preload = BTreeMap::new();
        let mut interface = self.vertex_indices(&mut preload, 5, 8)?;
        interface.extend(self.fetched_inputs(&options.inputs, 9, &mut preload)?);
        let (exec, count) = self.present()?;
        self.merged_scalars(options.user_sgprs, count, exec)?;
        let uvec4 = self.t.uvec4;
        let mut outputs = Vec::new();
        for location in 0..options.item_bytes / 16 {
            let var = self.interface_variable(uvec4, StorageClass::Output);
            self.b
                .decorate(var, Decoration::Location, [Operand::LiteralBit32(location)]);
            outputs.push(var);
        }
        let stage = self.stage_lds.expect("merged half");
        for word in 0..stage.store_words {
            let pointer = self.private_word(stage.stores, word)?;
            let zero = self.u32c(0);
            self.store(pointer, zero)?;
        }
        self.merged = Some(MergedState {
            interface,
            outputs,
            registers: Vec::new(),
            position: None,
            vertex_words: 0,
            emitted: 0,
        });
        self.preload_vgprs(&preload)
    }

    /// Writes the first half's vertex from its LDS words.
    pub(super) fn first_half_epilogue(&mut self) -> Result<(), EmitError> {
        let stage = self.stage_lds.expect("merged half");
        let outputs = self.merged.as_ref().expect("merged half").outputs.clone();
        let uvec4 = self.t.uvec4;
        for (location, var) in outputs.into_iter().enumerate() {
            let mut words = Vec::new();
            for component in 0..4 {
                let pointer = self.private_word(stage.stores, location as u32 * 4 + component)?;
                words.push(self.load(pointer)?);
            }
            let value = ok(self.b.composite_construct(uvec4, None, words))?;
            self.store(var, value)?;
        }
        Ok(())
    }

    /// The second half as a geometry shader: one invocation per primitive, its vertices' LDS words loaded from
    /// the first half's outputs. v0, v1 and v4 hold the vertices' word offsets two to a register, v2 the
    /// primitive index and v3 the invocation index; s3 carries the primitive count in bits 8-15.
    pub(super) fn second_half_prologue(
        &mut self,
        options: &GsOptions,
        emitted: Word,
    ) -> Result<(), EmitError> {
        let (u32_, f32_) = (self.t.u32_, self.t.f32_);
        self.b.capability(Capability::Geometry);
        self.push_bases()?;
        let i32_ = self.b.type_int(32, 1);
        let uvec4 = self.t.uvec4;
        let vertices = options.input.vertices();
        let count = self.u32c(vertices);
        let per_vertex = self.b.type_array(uvec4, count);
        let item_words = options.item_bytes / 4;
        let stage = self.stage_lds.expect("merged half");
        let mut interface = Vec::new();
        for location in 0..options.item_bytes / 16 {
            let var = self.interface_variable(per_vertex, StorageClass::Input);
            self.b
                .decorate(var, Decoration::Location, [Operand::LiteralBit32(location)]);
            interface.push(var);
            let all = ok(self.b.load(per_vertex, None, var, None, []))?;
            for vertex in 0..vertices {
                let value = ok(self.b.composite_extract(uvec4, None, all, [vertex]))?;
                for component in 0..4 {
                    let word = ok(self.b.composite_extract(u32_, None, value, [component]))?;
                    let at = vertex * item_words + location * 4 + component;
                    let pointer = self.private_word(stage.loads, at)?;
                    self.store(pointer, word)?;
                }
            }
        }
        let primitive = self.builtin_input(i32_, BuiltIn::PrimitiveId);
        let invocation = self.builtin_input(i32_, BuiltIn::InvocationId);
        interface.extend([primitive, invocation]);
        let mut preload = BTreeMap::new();
        for (reg, first) in [(0u8, 0u32), (1, 2), (4, 4)] {
            let pair = (first * item_words) | ((first + 1) * item_words) << 16;
            preload.insert(reg, self.u32c(pair));
        }
        for (reg, var) in [(2u8, primitive), (3, invocation)] {
            let value = ok(self.b.load(i32_, None, var, None, []))?;
            preload.insert(reg, ok(self.b.bitcast(u32_, None, value))?);
        }
        let (exec, count) = self.present()?;
        let eight = self.u32c(8);
        let wave_info = ok(self.b.shift_left_logical(u32_, None, count, eight))?;
        self.merged_scalars(options.user_sgprs, wave_info, exec)?;
        let vec4 = self.b.type_vector(f32_, 4);
        let mut registers = Vec::new();
        let mut position = None;
        for register in 0..(options.vertex_bytes / 16) as u8 {
            if options.position == Some(register) {
                let var = self.interface_variable(vec4, StorageClass::Output);
                self.b.decorate(
                    var,
                    Decoration::BuiltIn,
                    [Operand::BuiltIn(BuiltIn::Position)],
                );
                position = Some((register, var));
            } else {
                let var = self.interface_variable(uvec4, StorageClass::Output);
                self.b.decorate(
                    var,
                    Decoration::Location,
                    [Operand::LiteralBit32(u32::from(register))],
                );
                registers.push((register, var));
            }
        }
        let zero = self.u32c(0);
        self.store(emitted, zero)?;
        self.merged = Some(MergedState {
            interface,
            outputs: Vec::new(),
            registers,
            position,
            vertex_words: options.vertex_bytes / 4,
            emitted,
        });
        self.preload_vgprs(&preload)
    }

    /// A geometry message: an emit writes the next vertex from its LDS words, a cut ends the strip.
    pub(super) fn second_half_message(&mut self, code: u32) -> Result<(), EmitError> {
        if code & 0xF != MSG_GS {
            return Ok(());
        }
        if (code >> 8) & 3 != 0 {
            return Err(EmitError::Unsupported(
                "geometry output to a stream past the first",
            ));
        }
        let op = (code >> 4) & 3;
        let active = self.active()?;
        self.if_then(active, |e| {
            if op & GS_EMIT != 0 {
                e.emit_vertex()?;
            }
            if op & GS_CUT != 0 {
                ok(e.b.end_primitive())?;
            }
            Ok(())
        })
    }

    fn emit_vertex(&mut self) -> Result<(), EmitError> {
        let state = self.merged.as_ref().expect("merged half");
        let (emitted, vertex_words, position) = (state.emitted, state.vertex_words, state.position);
        let registers = state.registers.clone();
        let stage = self.stage_lds.expect("merged half");
        let (u32_, f32_) = (self.t.u32_, self.t.f32_);
        let uvec4 = self.t.uvec4;
        let count = self.load(emitted)?;
        let size = self.u32c(vertex_words);
        let first = ok(self.b.i_mul(u32_, None, count, size))?;
        let mask = self.u32c(stage.store_words - 1);
        let pointer = self.t.ptr_function_u32;
        let read = |e: &mut Self, register: u8| -> Result<Word, EmitError> {
            let mut words = Vec::new();
            for component in 0..4 {
                let offset = e.u32c(u32::from(register) * 4 + component);
                let at = ok(e.b.i_add(u32_, None, first, offset))?;
                let at = ok(e.b.bitwise_and(u32_, None, at, mask))?;
                let element = ok(e.b.access_chain(pointer, None, stage.stores, [at]))?;
                words.push(e.load(element)?);
            }
            ok(e.b.composite_construct(uvec4, None, words))
        };
        if let Some((register, var)) = position {
            let words = read(self, register)?;
            let vec4 = self.b.type_vector(f32_, 4);
            let value = ok(self.b.bitcast(vec4, None, words))?;
            self.store(var, value)?;
        }
        for (register, var) in registers {
            let value = read(self, register)?;
            self.store(var, value)?;
        }
        ok(self.b.emit_vertex())?;
        let one = self.u32c(1);
        let next = ok(self.b.i_add(u32_, None, count, one))?;
        self.store(emitted, next)
    }
}
