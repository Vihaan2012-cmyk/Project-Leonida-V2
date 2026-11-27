use std::collections::HashMap;

use rspirv::dr::Operand;
use rspirv::spirv::{BuiltIn, Capability, Decoration, SelectionControl, StorageClass, Word};

use super::{
    DepthOutput, EXEC, EmitError, Emitter, PUSH_CODE, PUSH_HEAP, PixelOptions, PixelOutput, ok,
};
use crate::images::Number;

const PERSP_CENTER: u32 = 1 << 1;
const POSITION: [u32; 4] = [1 << 8, 1 << 9, 1 << 10, 1 << 11];
const FRONT_FACE: u32 = 1 << 12;
const SUPPORTED: u32 = PERSP_CENTER | 0xF00 | FRONT_FACE;

pub(super) struct PixelState {
    frag_coord: Word,
    front_facing: Word,
    bary: Word,
    helper: Word,
    attributes: HashMap<u8, Word>,
    outputs: Vec<(PixelOutput, Word)>,
    depth: Option<Word>,
    alive: Word,
}

impl PixelState {
    pub(super) fn interface(&self) -> Vec<Word> {
        let mut list = vec![self.frag_coord, self.front_facing, self.bary, self.helper];
        list.extend(self.attributes.values().copied());
        list.extend(self.outputs.iter().map(|(_, var)| *var));
        list.extend(self.depth);
        list
    }
}

impl Emitter {
    /// A module-scope input or output, even while a function is being built.
    pub(super) fn interface_variable(&mut self, pointee: Word, storage: StorageClass) -> Word {
        let ptr = self.b.type_pointer(None, storage, pointee);
        let id = self.b.id();
        let inst = rspirv::dr::Instruction::new(
            rspirv::spirv::Op::Variable,
            Some(ptr),
            Some(id),
            vec![Operand::StorageClass(storage)],
        );
        self.b.module_mut().types_global_values.push(inst);
        id
    }

    pub(super) fn builtin_input(&mut self, pointee: Word, builtin: BuiltIn) -> Word {
        let var = self.interface_variable(pointee, StorageClass::Input);
        self.b
            .decorate(var, Decoration::BuiltIn, [Operand::BuiltIn(builtin)]);
        var
    }

    pub(super) fn pixel_prologue(
        &mut self,
        options: PixelOptions,
        alive: Word,
    ) -> Result<(), EmitError> {
        if options.inputs & !SUPPORTED != 0 || options.inputs & PERSP_CENTER == 0 {
            return Err(EmitError::Unsupported(
                "pixel inputs other than centre barycentrics, position and facing",
            ));
        }
        let (u32_, f32_, bool_) = (self.t.u32_, self.t.f32_, self.t.bool_);
        self.b.capability(Capability::FragmentBarycentricKHR);
        self.b.extension("SPV_KHR_fragment_shader_barycentric");
        let vec3 = self.b.type_vector(f32_, 3);
        let vec4 = self.b.type_vector(f32_, 4);
        let frag_coord = self.builtin_input(vec4, BuiltIn::FragCoord);
        let front_facing = self.builtin_input(bool_, BuiltIn::FrontFacing);
        let bary = self.builtin_input(vec3, BuiltIn::BaryCoordKHR);
        let helper = self.builtin_input(bool_, BuiltIn::HelperInvocation);
        let mut outputs = Vec::new();
        for output in &options.outputs {
            let component = match output.number {
                Number::Float => f32_,
                Number::Uint => u32_,
                Number::Sint => self.b.type_int(32, 1),
            };
            let ty = self.b.type_vector(component, 4);
            let var = self.interface_variable(ty, StorageClass::Output);
            self.b.decorate(
                var,
                Decoration::Location,
                [Operand::LiteralBit32(u32::from(output.target))],
            );
            outputs.push((*output, var));
        }
        let depth = match options.depth {
            Some(_) => {
                let var = self.interface_variable(f32_, StorageClass::Output);
                self.b.decorate(
                    var,
                    Decoration::BuiltIn,
                    [Operand::BuiltIn(BuiltIn::FragDepth)],
                );
                Some(var)
            }
            None => None,
        };
        let no = self.boolc(false);
        self.store(alive, no)?;
        self.pixel = Some(PixelState {
            frag_coord,
            front_facing,
            bary,
            helper,
            attributes: HashMap::new(),
            outputs,
            depth,
            alive,
        });
        let low = self.push_word(PUSH_HEAP)?;
        let high = self.push_word(PUSH_HEAP + 1)?;
        self.heap = Some(self.pack(low, high)?);
        let low = self.push_word(PUSH_CODE)?;
        let high = self.push_word(PUSH_CODE + 1)?;
        self.code = Some(self.pack(low, high)?);
        let is_helper = ok(self.b.load(bool_, None, helper, None, []))?;
        let live = ok(self.b.logical_not(bool_, None, is_helper))?;
        let scope = self.u32c(3);
        let uvec4 = self.t.uvec4;
        let ballot = ok(self.b.group_non_uniform_ballot(uvec4, None, scope, live))?;
        let exec = ok(self.b.composite_extract(u32_, None, ballot, [0]))?;
        let scalars: Vec<(u8, Word)> = self.sgpr.iter().map(|(r, v)| (*r, *v)).collect();
        for (reg, var) in scalars {
            let index = u32::from(reg);
            let value = if index < options.user_sgprs.min(32) {
                self.push_word(index)?
            } else if reg == EXEC {
                exec
            } else {
                self.u32c(0)
            };
            self.store(var, value)?;
        }
        let mut preload = Vec::new();
        let barycentrics = ok(self.b.load(vec3, None, bary, None, []))?;
        for axis in [1, 2] {
            let weight = ok(self.b.composite_extract(f32_, None, barycentrics, [axis]))?;
            preload.push(self.bits(weight)?);
        }
        let position = ok(self.b.load(vec4, None, frag_coord, None, []))?;
        for (axis, bit) in POSITION.iter().enumerate() {
            if options.inputs & bit != 0 {
                let value = ok(self
                    .b
                    .composite_extract(f32_, None, position, [axis as u32]))?;
                preload.push(self.bits(value)?);
            }
        }
        if options.inputs & FRONT_FACE != 0 {
            let front = ok(self.b.load(bool_, None, front_facing, None, []))?;
            let (yes, no) = (self.u32c(1.0f32.to_bits()), self.u32c(0));
            preload.push(ok(self.b.select(u32_, None, front, yes, no))?);
        }
        let vectors: Vec<(u8, Word)> = self.vgpr.iter().map(|(r, v)| (*r, *v)).collect();
        for (reg, var) in vectors {
            let value = match preload.get(usize::from(reg)) {
                Some(value) => *value,
                None => self.u32c(0),
            };
            self.store(var, value)?;
        }
        let no = self.boolc(false);
        self.store(self.scc, no)?;
        let zero = self.u32c(0);
        self.store(self.messages, zero)
    }

    /// Pixels the last export did not cover are discarded, as the hardware's valid mask does.
    pub(super) fn pixel_epilogue(&mut self) -> Result<(), EmitError> {
        let alive = self.pixel.as_ref().expect("pixel mode").alive;
        let bool_ = self.t.bool_;
        let keep = ok(self.b.load(bool_, None, alive, None, []))?;
        let (discard, done) = (self.b.id(), self.b.id());
        ok(self.b.selection_merge(done, SelectionControl::NONE))?;
        ok(self.b.branch_conditional(keep, done, discard, []))?;
        self.begin(discard)?;
        ok(self.b.kill())?;
        self.begin(done)
    }

    fn pixel_attribute(&mut self, attr: u8) -> Word {
        if let Some(var) = self.pixel.as_ref().and_then(|p| p.attributes.get(&attr)) {
            return *var;
        }
        let uvec4 = self.t.uvec4;
        let three = self.u32c(3);
        let array = self.b.type_array(uvec4, three);
        let var = self.interface_variable(array, StorageClass::Input);
        self.b.decorate(
            var,
            Decoration::Location,
            [Operand::LiteralBit32(u32::from(attr))],
        );
        self.b.decorate(var, Decoration::PerVertexKHR, []);
        self.pixel
            .as_mut()
            .expect("pixel mode")
            .attributes
            .insert(attr, var);
        var
    }

    /// One vertex's raw attribute word, as the hardware's parameter cache holds it.
    pub(super) fn pixel_attribute_word(
        &mut self,
        attr: u8,
        channel: u8,
        vertex: u8,
    ) -> Result<Word, EmitError> {
        let var = self.pixel_attribute(attr);
        let u32_ = self.t.u32_;
        let ptr = self.b.type_pointer(None, StorageClass::Input, u32_);
        let (vertex, channel) = (self.u32c(u32::from(vertex)), self.u32c(u32::from(channel)));
        let element = ok(self.b.access_chain(ptr, None, var, [vertex, channel]))?;
        self.load(element)
    }

    pub(super) fn pixel_export(
        &mut self,
        target: u8,
        values: [Option<Word>; 4],
        done: bool,
        compressed: bool,
    ) -> Result<(), EmitError> {
        let (u32_, f32_) = (self.t.u32_, self.t.f32_);
        let active = self.active()?;
        let state = self.pixel.as_ref().expect("pixel mode");
        match target {
            0..=7 => {
                let found = state
                    .outputs
                    .iter()
                    .find(|(o, _)| o.target == target)
                    .copied();
                if let Some((output, var)) = found {
                    let zero = self.u32c(0);
                    let mut words = Vec::new();
                    if compressed {
                        for value in values.iter().take(2) {
                            let value = value.unwrap_or(zero);
                            for shift in [0, 16] {
                                let shift = self.u32c(shift);
                                let moved =
                                    ok(self.b.shift_right_logical(u32_, None, value, shift))?;
                                let mask = self.u32c(0xFFFF);
                                let half = ok(self.b.bitwise_and(u32_, None, moved, mask))?;
                                words.push(match output.number {
                                    Number::Float => self.half_to_single(half, true)?,
                                    Number::Uint => half,
                                    Number::Sint => {
                                        let sixteen = self.u32c(16);
                                        let up = ok(self
                                            .b
                                            .shift_left_logical(u32_, None, half, sixteen))?;
                                        ok(self.b.shift_right_arithmetic(u32_, None, up, sixteen))?
                                    }
                                });
                            }
                        }
                    } else {
                        words.extend(values.iter().map(|v| v.unwrap_or(zero)));
                    }
                    let packed = ok(self.b.composite_construct(self.t.uvec4, None, words))?;
                    let ty = match output.number {
                        Number::Float => self.b.type_vector(f32_, 4),
                        Number::Uint => self.t.uvec4,
                        Number::Sint => {
                            let i32_ = self.b.type_int(32, 1);
                            self.b.type_vector(i32_, 4)
                        }
                    };
                    let value = if output.number == Number::Uint {
                        packed
                    } else {
                        ok(self.b.bitcast(ty, None, packed))?
                    };
                    self.if_then(active, |e| e.store(var, value))?;
                }
            }
            8 => {
                if let Some(var) = state.depth
                    && let Some(depth) = values[0]
                {
                    let depth = ok(self.b.bitcast(f32_, None, depth))?;
                    self.if_then(active, |e| e.store(var, depth))?;
                }
            }
            9 => {}
            _ => {
                return Err(EmitError::Unsupported(
                    "position or parameter exports in a pixel shader",
                ));
            }
        }
        if done {
            let alive = self.pixel.as_ref().expect("pixel mode").alive;
            self.store(alive, active)?;
        }
        Ok(())
    }
}

impl DepthOutput {
    /// From DB_SHADER_CONTROL: Z_EXPORT_ENABLE (bit 0) and CONSERVATIVE_Z_EXPORT (bits 13-14).
    pub fn from_control(control: u32) -> Option<DepthOutput> {
        if control & 1 == 0 {
            return None;
        }
        Some(match (control >> 13) & 3 {
            1 => DepthOutput::LessEqual,
            2 => DepthOutput::GreaterEqual,
            _ => DepthOutput::Any,
        })
    }
}
