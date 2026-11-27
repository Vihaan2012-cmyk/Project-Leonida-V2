use std::collections::BTreeMap;

use rspirv::dr::Operand;
use rspirv::spirv::{BuiltIn, Capability, Decoration, StorageClass, Word};

use super::{EXEC, EmitError, Emitter, MeshOptions, PUSH_HEAP, VertexInput, VertexOptions, ok};
use crate::images::Number;

const POSITION: u8 = 12;
const PRIMITIVE: u8 = 20;
const PARAMETER: u8 = 32;

/// A vertex or mesh entry's outputs: one value per invocation for a vertex shader, arrays of vertex and
/// primitive slots for a mesh shader.
pub(super) struct GeometryState {
    mesh: Option<MeshOptions>,
    position: Word,
    parameters: BTreeMap<u8, Word>,
    inputs: Vec<Word>,
    indices: Word,
    cull: Word,
}

impl GeometryState {
    pub(super) fn interface(&self) -> Vec<Word> {
        let mut list = vec![self.position];
        list.extend(self.parameters.values().copied());
        list.extend(&self.inputs);
        if self.mesh.is_some() {
            list.extend([self.indices, self.cull]);
        }
        list
    }
}

impl Emitter {
    pub(super) fn push_bases(&mut self) -> Result<(), EmitError> {
        let low = self.push_word(PUSH_HEAP)?;
        let high = self.push_word(PUSH_HEAP + 1)?;
        self.heap = Some(self.pack(low, high)?);
        let low = self.push_word(self.code_word)?;
        let high = self.push_word(self.code_word + 1)?;
        self.code = Some(self.pack(low, high)?);
        Ok(())
    }

    /// The current part's VGPRs: `preload`'s values, zero elsewhere.
    pub(super) fn preload_vgprs(&mut self, preload: &BTreeMap<u8, Word>) -> Result<(), EmitError> {
        let registers: Vec<u8> = self.vgpr.keys().copied().collect();
        for reg in registers {
            let value = match preload.get(&reg) {
                Some(value) => *value,
                None => self.u32c(0),
            };
            self.store(self.vgpr_var(reg), value)?;
        }
        let no = self.boolc(false);
        self.store(self.scc, no)?;
        let zero = self.u32c(0);
        self.store(self.messages, zero)
    }

    /// The vertex index into `index_vgpr` and the instance index past the base instance into `instance_vgpr`;
    /// returns the built-in inputs read.
    pub(super) fn vertex_indices(
        &mut self,
        preload: &mut BTreeMap<u8, Word>,
        index_vgpr: u8,
        instance_vgpr: u8,
    ) -> Result<Vec<Word>, EmitError> {
        let u32_ = self.t.u32_;
        self.b.capability(Capability::DrawParameters);
        let i32_ = self.b.type_int(32, 1);
        let index = self.builtin_input(i32_, BuiltIn::VertexIndex);
        let instance = self.builtin_input(i32_, BuiltIn::InstanceIndex);
        let base_instance = self.builtin_input(i32_, BuiltIn::BaseInstance);
        let vertex = ok(self.b.load(i32_, None, index, None, []))?;
        preload.insert(index_vgpr, ok(self.b.bitcast(u32_, None, vertex))?);
        let instance_value = ok(self.b.load(i32_, None, instance, None, []))?;
        let base = ok(self.b.load(i32_, None, base_instance, None, []))?;
        let local = ok(self.b.i_sub(i32_, None, instance_value, base))?;
        preload.insert(instance_vgpr, ok(self.b.bitcast(u32_, None, local))?);
        Ok(vec![index, instance, base_instance])
    }

    /// Each fetched attribute r from the vertex input at location r into v[first + 4r] to v[first + 4r + 3];
    /// returns the inputs.
    pub(super) fn fetched_inputs(
        &mut self,
        inputs: &[VertexInput],
        first: u8,
        preload: &mut BTreeMap<u8, Word>,
    ) -> Result<Vec<Word>, EmitError> {
        let (u32_, f32_) = (self.t.u32_, self.t.f32_);
        let i32_ = self.b.type_int(32, 1);
        let mut variables = Vec::new();
        for input in inputs {
            let component = match input.number {
                Number::Float => f32_,
                Number::Uint => u32_,
                Number::Sint => i32_,
            };
            let ty = self.b.type_vector(component, 4);
            let var = self.interface_variable(ty, StorageClass::Input);
            self.b.decorate(
                var,
                Decoration::Location,
                [Operand::LiteralBit32(u32::from(input.register))],
            );
            variables.push(var);
            let value = ok(self.b.load(ty, None, var, None, []))?;
            for c in 0..4u32 {
                let part = ok(self.b.composite_extract(component, None, value, [c]))?;
                let word = if input.number == Number::Uint {
                    part
                } else {
                    ok(self.b.bitcast(u32_, None, part))?
                };
                preload.insert(first + 4 * input.register + c as u8, word);
            }
        }
        Ok(variables)
    }

    pub(super) fn vertex_prologue(&mut self, options: VertexOptions) -> Result<(), EmitError> {
        let (u32_, f32_) = (self.t.u32_, self.t.f32_);
        let vec4 = self.b.type_vector(f32_, 4);
        let position = self.interface_variable(vec4, StorageClass::Output);
        self.b.decorate(
            position,
            Decoration::BuiltIn,
            [Operand::BuiltIn(BuiltIn::Position)],
        );
        self.push_bases()?;
        let mut preload = BTreeMap::new();
        let mut inputs = self.vertex_indices(&mut preload, 0, 3)?;
        inputs.extend(self.fetched_inputs(&options.inputs, 4, &mut preload)?);
        let yes = self.boolc(true);
        let scope = self.u32c(3);
        let uvec4 = self.t.uvec4;
        let ballot = ok(self.b.group_non_uniform_ballot(uvec4, None, scope, yes))?;
        let exec = ok(self.b.composite_extract(u32_, None, ballot, [0]))?;
        let scalars: Vec<(u8, Word)> = self.sgpr.iter().map(|(r, v)| (*r, *v)).collect();
        for (reg, var) in scalars {
            let value = if u32::from(reg) < options.user_sgprs.min(32) {
                self.push_word(u32::from(reg))?
            } else if reg == EXEC {
                exec
            } else {
                self.u32c(0)
            };
            self.store(var, value)?;
        }
        self.geometry = Some(GeometryState {
            mesh: None,
            position,
            parameters: BTreeMap::new(),
            inputs,
            indices: 0,
            cull: 0,
        });
        self.preload_vgprs(&preload)
    }

    pub(super) fn mesh_prologue(&mut self, options: MeshOptions) -> Result<(), EmitError> {
        let (u32_, f32_, bool_) = (self.t.u32_, self.t.f32_, self.t.bool_);
        self.b.capability(Capability::MeshShadingEXT);
        self.b.extension("SPV_EXT_mesh_shader");
        let vec4 = self.b.type_vector(f32_, 4);
        let uvec3 = self.b.type_vector(u32_, 3);
        let vertices = self.u32c(options.max_vertices);
        let primitives = self.u32c(options.max_primitives);
        let position_array = self.b.type_array(vec4, vertices);
        let position = self.interface_variable(position_array, StorageClass::Output);
        self.b.decorate(
            position,
            Decoration::BuiltIn,
            [Operand::BuiltIn(BuiltIn::Position)],
        );
        let index_array = self.b.type_array(uvec3, primitives);
        let indices = self.interface_variable(index_array, StorageClass::Output);
        self.b.decorate(
            indices,
            Decoration::BuiltIn,
            [Operand::BuiltIn(BuiltIn::PrimitiveTriangleIndicesEXT)],
        );
        let cull_array = self.b.type_array(bool_, primitives);
        let cull = self.interface_variable(cull_array, StorageClass::Output);
        self.b.decorate(
            cull,
            Decoration::BuiltIn,
            [Operand::BuiltIn(BuiltIn::CullPrimitiveEXT)],
        );
        self.b.decorate(cull, Decoration::PerPrimitiveEXT, []);
        let uvec3_input = self.t.uvec3;
        let count_var = self.builtin_input(uvec3_input, BuiltIn::NumWorkgroups);
        self.push_bases()?;
        let counts = ok(self.b.load(uvec3_input, None, count_var, None, []))?;
        let part = |e: &mut Self, v: Word, i: u32| ok(e.b.composite_extract(u32_, None, v, [i]));
        let (gx, gy, gz) = (
            part(self, self.group_ids, 0)?,
            part(self, self.group_ids, 1)?,
            part(self, self.group_ids, 2)?,
        );
        let (nx, ny) = (part(self, counts, 0)?, part(self, counts, 1)?);
        let row = ok(self.b.i_mul(u32_, None, gy, nx))?;
        let plane = ok(self.b.i_mul(u32_, None, nx, ny))?;
        let plane = ok(self.b.i_mul(u32_, None, gz, plane))?;
        let group = ok(self.b.i_add(u32_, None, gx, row))?;
        let group = ok(self.b.i_add(u32_, None, group, plane))?;
        let size = self.u32c(options.group_size);
        let first = ok(self.b.i_mul(u32_, None, group, size))?;
        let lanes = self.u32c(self.lanes);
        let wave = ok(self.b.u_div(u32_, None, self.thread_index, lanes))?;
        let twenty_four = self.u32c(24);
        let wave_info = ok(self.b.shift_left_logical(u32_, None, wave, twenty_four))?;
        let scalars: Vec<(u8, Word)> = self.sgpr.iter().map(|(r, v)| (*r, *v)).collect();
        for (reg, var) in scalars {
            let index = u32::from(reg);
            let value = if (8..8 + options.user_sgprs.min(32)).contains(&index) {
                self.push_word(index - 8)?
            } else if reg == 3 {
                wave_info
            } else if reg == EXEC || (reg == EXEC + 1 && self.lanes == 64) {
                self.u32c(u32::MAX)
            } else {
                self.u32c(0)
            };
            self.store(var, value)?;
        }
        let threads = options.group_size;
        for part in self.parts() {
            self.set_part(part);
            for chunk in 0..self.lds_words.div_ceil(threads) {
                let base = self.u32c(threads * chunk);
                let index = ok(self.b.i_add(u32_, None, base, self.thread_index))?;
                let limit = self.u32c(self.lds_words);
                let inside = ok(self.b.u_less_than(bool_, None, index, limit))?;
                self.if_then(inside, |e| {
                    let ptr = e.lds_element(index)?;
                    let zero = e.u32c(0);
                    e.store(ptr, zero)
                })?;
            }
        }
        self.set_part(0);
        ok(self.b.set_mesh_outputs_ext(vertices, primitives))?;
        let ptr_bool = self.b.type_pointer(None, StorageClass::Output, bool_);
        let ptr_uvec3 = self.b.type_pointer(None, StorageClass::Output, uvec3);
        for part in self.parts() {
            self.set_part(part);
            for chunk in 0..options.max_primitives.div_ceil(threads) {
                let base = self.u32c(threads * chunk);
                let slot = ok(self.b.i_add(u32_, None, base, self.thread_index))?;
                let inside = ok(self.b.u_less_than(bool_, None, slot, primitives))?;
                self.if_then(inside, |e| {
                    let ptr = ok(e.b.access_chain(ptr_bool, None, cull, [slot]))?;
                    let yes = e.boolc(true);
                    e.store(ptr, yes)?;
                    let zero = e.u32c(0);
                    let none = ok(e.b.composite_construct(uvec3, None, [zero; 3]))?;
                    let ptr = ok(e.b.access_chain(ptr_uvec3, None, indices, [slot]))?;
                    ok(e.b.store(ptr, none, None, []))
                })?;
            }
        }
        self.set_part(0);
        self.barrier()?;
        self.geometry = Some(GeometryState {
            mesh: Some(options),
            position,
            parameters: BTreeMap::new(),
            inputs: Vec::new(),
            indices,
            cull,
        });
        for part in self.parts() {
            self.set_part(part);
            let dispatch = ok(self.b.i_add(u32_, None, first, self.thread_index))?;
            let preload = BTreeMap::from([(5, dispatch)]);
            self.preload_vgprs(&preload)?;
        }
        self.set_part(0);
        Ok(())
    }

    fn parameter(&mut self, number: u8) -> Word {
        let state = self.geometry.as_ref().expect("geometry mode");
        if let Some(var) = state.parameters.get(&number) {
            return *var;
        }
        let mesh = state.mesh;
        let uvec4 = self.t.uvec4;
        let ty = match mesh {
            Some(options) => {
                let count = self.u32c(options.max_vertices);
                self.b.type_array(uvec4, count)
            }
            None => uvec4,
        };
        let var = self.interface_variable(ty, StorageClass::Output);
        self.b.decorate(
            var,
            Decoration::Location,
            [Operand::LiteralBit32(u32::from(number))],
        );
        self.geometry
            .as_mut()
            .expect("geometry mode")
            .parameters
            .insert(number, var);
        var
    }

    /// Writes `value` to a vertex output: the invocation's own for a vertex shader, the slot of its index in
    /// the group for a mesh shader.
    fn vertex_output(&mut self, var: Word, element: Word, value: Word) -> Result<(), EmitError> {
        let bool_ = self.t.bool_;
        let active = self.active()?;
        match self.geometry.as_ref().expect("geometry mode").mesh {
            None => self.if_then(active, |e| e.store(var, value)),
            Some(options) => {
                let limit = self.u32c(options.max_vertices);
                let slot = self.thread_index;
                let inside = ok(self.b.u_less_than(bool_, None, slot, limit))?;
                let both = ok(self.b.logical_and(bool_, None, active, inside))?;
                let ptr = self.b.type_pointer(None, StorageClass::Output, element);
                self.if_then(both, |e| {
                    let at = ok(e.b.access_chain(ptr, None, var, [slot]))?;
                    ok(e.b.store(at, value, None, []))
                })
            }
        }
    }

    pub(super) fn geometry_export(
        &mut self,
        target: u8,
        values: [Option<Word>; 4],
    ) -> Result<(), EmitError> {
        let (u32_, f32_, bool_) = (self.t.u32_, self.t.f32_, self.t.bool_);
        let zero = self.u32c(0);
        let words: Vec<Word> = values.iter().map(|v| v.unwrap_or(zero)).collect();
        match target {
            POSITION => {
                let packed = ok(self.b.composite_construct(self.t.uvec4, None, words))?;
                let vec4 = self.b.type_vector(f32_, 4);
                let value = ok(self.b.bitcast(vec4, None, packed))?;
                let var = self.geometry.as_ref().expect("geometry mode").position;
                self.vertex_output(var, vec4, value)
            }
            13..=15 => Ok(()),
            PRIMITIVE => {
                let state = self.geometry.as_ref().expect("geometry mode");
                let Some(options) = state.mesh else {
                    return Ok(());
                };
                let (indices, cull) = (state.indices, state.cull);
                let packed = words[0];
                let mut parts = Vec::new();
                for shift in [0, 10, 20] {
                    let shift = self.u32c(shift);
                    let moved = ok(self.b.shift_right_logical(u32_, None, packed, shift))?;
                    let mask = self.u32c(0x1FF);
                    parts.push(ok(self.b.bitwise_and(u32_, None, moved, mask))?);
                }
                let uvec3 = self.b.type_vector(u32_, 3);
                let triangle = ok(self.b.composite_construct(uvec3, None, parts))?;
                let thirty_one = self.u32c(31);
                let top = ok(self.b.shift_right_logical(u32_, None, packed, thirty_one))?;
                let empty = ok(self.b.i_not_equal(bool_, None, top, zero))?;
                let active = self.active()?;
                let limit = self.u32c(options.max_primitives);
                let slot = self.thread_index;
                let inside = ok(self.b.u_less_than(bool_, None, slot, limit))?;
                let both = ok(self.b.logical_and(bool_, None, active, inside))?;
                let ptr_indices = self.b.type_pointer(None, StorageClass::Output, uvec3);
                let ptr_cull = self.b.type_pointer(None, StorageClass::Output, bool_);
                self.if_then(both, |e| {
                    let at = ok(e.b.access_chain(ptr_indices, None, indices, [slot]))?;
                    ok(e.b.store(at, triangle, None, []))?;
                    let at = ok(e.b.access_chain(ptr_cull, None, cull, [slot]))?;
                    ok(e.b.store(at, empty, None, []))
                })
            }
            PARAMETER..=63 => {
                let var = self.parameter(target - PARAMETER);
                let packed = ok(self.b.composite_construct(self.t.uvec4, None, words))?;
                let uvec4 = self.t.uvec4;
                self.vertex_output(var, uvec4, packed)
            }
            _ => Err(EmitError::Unsupported(
                "colour or depth exports outside a pixel shader",
            )),
        }
    }
}
