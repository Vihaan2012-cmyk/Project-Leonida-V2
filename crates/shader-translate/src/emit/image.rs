use rspirv::dr::Operand;
use rspirv::spirv::{
    Capability, Decoration, Dim, FunctionControl, ImageFormat, ImageOperands, LoopControl,
    SelectionControl, StorageClass, Word,
};
use shader_ir::{AtomicOp, Image, ImageDim, ImageOp};

use super::{EmitError, Emitter, GLOp, ok};
use crate::images::{self, NUMBERS, Number, View};

#[derive(Debug, Clone, Copy)]
pub(super) struct ImageTypes {
    i32_: Word,
    floats: [Word; 5],
    ints: [Word; 5],
    sampler: Word,
    ptr_sampler: Word,
    samplers: Word,
    tables: [Word; 2],
}

/// The image operands that set the level of detail, and whether the hardware derives it from derivatives.
enum Level {
    Explicit(ImageOperands, Vec<Operand>),
    Implicit(Option<ImageOperands>, Vec<Operand>),
}

#[derive(Debug, Clone, Copy)]
struct Extras {
    compare: Option<Word>,
    clamp: Option<Word>,
    gradients: Option<(Word, Word)>,
    offset: Option<Word>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ViewIds {
    var: Word,
    image: Word,
    ptr: Word,
    sampled: Word,
}

impl Emitter {
    fn bind(&mut self, var: Word, binding: u32) {
        self.b.decorate(
            var,
            Decoration::DescriptorSet,
            [Operand::LiteralBit32(images::SET)],
        );
        self.b
            .decorate(var, Decoration::Binding, [Operand::LiteralBit32(binding)]);
    }

    /// A module-scope variable, even while a function is being built.
    fn global(&mut self, ty: Word, storage: StorageClass, binding: u32) -> Word {
        let id = self.b.id();
        let inst = rspirv::dr::Instruction::new(
            rspirv::spirv::Op::Variable,
            Some(ty),
            Some(id),
            vec![Operand::StorageClass(storage)],
        );
        self.b.module_mut().types_global_values.push(inst);
        self.bind(id, binding);
        id
    }

    fn nonuniform(&mut self, id: Word) -> Word {
        self.b.decorate(id, Decoration::NonUniform, []);
        id
    }

    fn image_types(&mut self) -> ImageTypes {
        if let Some(types) = self.image_types {
            return types;
        }
        for capability in [
            Capability::RuntimeDescriptorArray,
            Capability::ShaderNonUniform,
            Capability::SampledImageArrayNonUniformIndexing,
            Capability::ImageQuery,
        ] {
            self.b.capability(capability);
        }
        self.b.extension("SPV_EXT_descriptor_indexing");
        let i32_ = self.b.type_int(32, 1);
        let f32_ = self.t.f32_;
        let floats = [0, 1, 2, 3, 4].map(|n| match n {
            0 => 0,
            1 => f32_,
            _ => self.b.type_vector(f32_, n),
        });
        let ints = [0, 1, 2, 3, 4].map(|n| match n {
            0 => 0,
            1 => i32_,
            _ => self.b.type_vector(i32_, n),
        });
        let sampler = self.b.type_sampler();
        let array = self.b.type_runtime_array(sampler);
        let ptr_array = self
            .b
            .type_pointer(None, StorageClass::UniformConstant, array);
        let samplers = self.global(ptr_array, StorageClass::UniformConstant, images::SAMPLERS);
        let ptr_sampler = self
            .b
            .type_pointer(None, StorageClass::UniformConstant, sampler);
        let block = self.storage_block();
        let tables = [images::TEXTURE_TABLE, images::SAMPLER_TABLE]
            .map(|binding| self.global(block, StorageClass::StorageBuffer, binding));
        let types = ImageTypes {
            i32_,
            floats,
            ints,
            sampler,
            ptr_sampler,
            samplers,
            tables,
        };
        self.image_types = Some(types);
        types
    }

    fn view(&mut self, view: View, number: Number, storage: bool) -> ViewIds {
        if let Some(ids) = self.views.get(&(view, number, storage)) {
            return *ids;
        }
        let types = self.image_types();
        if storage {
            for capability in [
                Capability::StorageImageWriteWithoutFormat,
                Capability::StorageImageArrayNonUniformIndexing,
            ] {
                self.b.capability(capability);
            }
        }
        let component = match number {
            Number::Float => self.t.f32_,
            Number::Uint => self.t.u32_,
            Number::Sint => types.i32_,
        };
        let (dim, arrayed) = match view {
            View::D2 => (Dim::Dim2D, 0),
            View::D2Array => (Dim::Dim2D, 1),
            View::D3 => (Dim::Dim3D, 0),
        };
        let sampled = if storage { 2 } else { 1 };
        let image = self.b.type_image(
            component,
            dim,
            0,
            arrayed,
            0,
            sampled,
            ImageFormat::Unknown,
            None,
        );
        let array = self.b.type_runtime_array(image);
        let ptr_array = self
            .b
            .type_pointer(None, StorageClass::UniformConstant, array);
        let binding = if storage {
            images::storage_binding(view, number)
        } else {
            images::binding(view, number)
        };
        let var = self.global(ptr_array, StorageClass::UniformConstant, binding);
        let ids = ViewIds {
            var,
            image,
            ptr: self
                .b
                .type_pointer(None, StorageClass::UniformConstant, image),
            sampled: if storage {
                0
            } else {
                self.b.type_sampled_image(image)
            },
        };
        self.views.insert((view, number, storage), ids);
        ids
    }

    fn atomic_view(&mut self, view: View, wide: bool) -> ViewIds {
        if let Some(ids) = self.atomic_views.get(&(view, wide)) {
            return *ids;
        }
        self.image_types();
        self.b
            .capability(Capability::StorageImageArrayNonUniformIndexing);
        let component = if wide {
            self.b.capability(Capability::Int64ImageEXT);
            self.b.capability(Capability::Int64Atomics);
            self.b.extension("SPV_EXT_shader_image_int64");
            self.t.u64_
        } else {
            self.t.u32_
        };
        let (dim, arrayed) = match view {
            View::D2 => (Dim::Dim2D, 0),
            View::D2Array => (Dim::Dim2D, 1),
            View::D3 => (Dim::Dim3D, 0),
        };
        let format = if wide {
            ImageFormat::R64ui
        } else {
            ImageFormat::R32ui
        };
        let image = self
            .b
            .type_image(component, dim, 0, arrayed, 0, 2, format, None);
        let array = self.b.type_runtime_array(image);
        let ptr_array = self
            .b
            .type_pointer(None, StorageClass::UniformConstant, array);
        let var = self.global(
            ptr_array,
            StorageClass::UniformConstant,
            images::atomic_binding(view, wide),
        );
        let ids = ViewIds {
            var,
            image,
            ptr: self
                .b
                .type_pointer(None, StorageClass::UniformConstant, image),
            sampled: 0,
        };
        self.atomic_views.insert((view, wide), ids);
        ids
    }

    /// One atomic per active lane on the texel's word; inactive lanes read zero and change nothing.
    fn image_atomic(&mut self, image: &Image, op: AtomicOp) -> Result<[Word; 4], EmitError> {
        let (u32_, u64_) = (self.t.u32_, self.t.u64_);
        let wide = image.data.len() == 2;
        let view = View::of(image.dim);
        let (index, _) = self.texture_slot(image)?;
        let coordinates: Vec<Word> = image
            .coordinates
            .iter()
            .map(|t| self.values[*t as usize])
            .collect();
        let coordinates = self.load_coordinates(image.dim, &coordinates)?;
        let words: Vec<Word> = image
            .data
            .iter()
            .map(|t| self.values[*t as usize])
            .collect();
        let value = if wide {
            self.pack(words[0], words[1])?
        } else {
            words[0]
        };
        let compare = image.compare.map(|t| self.values[t as usize]);
        let ty = if wide { u64_ } else { u32_ };
        let active = self.active()?;
        let before = self.label;
        let (then, merge) = (self.b.id(), self.b.id());
        ok(self.b.selection_merge(merge, SelectionControl::NONE))?;
        ok(self.b.branch_conditional(active, then, merge, []))?;
        self.begin(then)?;
        let ids = self.atomic_view(view, wide);
        let ptr = ok(self.b.access_chain(ids.ptr, None, ids.var, [index]))?;
        let ptr = self.nonuniform(ptr);
        let texel_type = self.b.type_pointer(None, StorageClass::Image, ty);
        let zero = self.u32c(0);
        let texel = ok(self
            .b
            .image_texel_pointer(texel_type, None, ptr, coordinates, zero))?;
        let texel = self.nonuniform(texel);
        let scope = self.u32c(1);
        let relaxed = self.u32c(0);
        let b = &mut self.b;
        let old = ok(match op {
            AtomicOp::Swap => b.atomic_exchange(ty, None, texel, scope, relaxed, value),
            AtomicOp::CompareSwap => b.atomic_compare_exchange(
                ty,
                None,
                texel,
                scope,
                relaxed,
                relaxed,
                value,
                compare.expect("compare-swap has a compare value"),
            ),
            AtomicOp::Add => b.atomic_i_add(ty, None, texel, scope, relaxed, value),
            AtomicOp::Sub => b.atomic_i_sub(ty, None, texel, scope, relaxed, value),
            AtomicOp::MinS => b.atomic_s_min(ty, None, texel, scope, relaxed, value),
            AtomicOp::MinU => b.atomic_u_min(ty, None, texel, scope, relaxed, value),
            AtomicOp::MaxS => b.atomic_s_max(ty, None, texel, scope, relaxed, value),
            AtomicOp::MaxU => b.atomic_u_max(ty, None, texel, scope, relaxed, value),
            AtomicOp::And => b.atomic_and(ty, None, texel, scope, relaxed, value),
            AtomicOp::Or => b.atomic_or(ty, None, texel, scope, relaxed, value),
            AtomicOp::Xor => b.atomic_xor(ty, None, texel, scope, relaxed, value),
        })?;
        let end = self.label;
        ok(self.b.branch(merge))?;
        self.begin(merge)?;
        let none = if wide { self.u64c(0) } else { zero };
        let old = ok(self.b.phi(ty, None, [(old, end), (none, before)]))?;
        let (low, high) = if wide { self.split(old)? } else { (old, zero) };
        Ok([low, high, zero, zero])
    }

    fn compare_samplers(&mut self) -> Word {
        if let Some(id) = self.compare_samplers {
            return id;
        }
        let types = self.image_types();
        let array = self.b.type_runtime_array(types.sampler);
        let ptr_array = self
            .b
            .type_pointer(None, StorageClass::UniformConstant, array);
        let id = self.global(
            ptr_array,
            StorageClass::UniformConstant,
            images::COMPARE_SAMPLERS,
        );
        self.compare_samplers = Some(id);
        id
    }
    fn i32c(&mut self, value: i32) -> Word {
        let ty = self.image_types().i32_;
        *self
            .constants
            .entry((4, u64::from(value as u32)))
            .or_insert_with(|| self.b.constant_bit32(ty, value as u32))
    }

    fn lookup_function(&mut self, which: usize) -> Word {
        match self.lookups[which] {
            Some(id) => id,
            None => {
                let id = self.b.id();
                self.lookups[which] = Some(id);
                id
            }
        }
    }

    pub(super) fn lookup_bodies(&mut self) -> Result<(), EmitError> {
        for which in 0..2 {
            if let Some(id) = self.lookups[which] {
                self.lookup_body(id, which)?;
            }
        }
        Ok(())
    }

    /// `images::lookup` in SPIR-V: FNV-1a picks the first entry, then up to `PROBES` entries are tried in order.
    fn lookup_body(&mut self, id: Word, which: usize) -> Result<(), EmitError> {
        let (u32_, bool_) = (self.t.u32_, self.t.bool_);
        let table = self.image_types().tables[which];
        let function_type = self.b.type_function(u32_, [u32_; 8]);
        ok(self
            .b
            .begin_function(u32_, Some(id), FunctionControl::NONE, function_type))?;
        let key: Vec<Word> = (0..8)
            .map(|_| ok(self.b.function_parameter(u32_)))
            .collect::<Result<_, _>>()?;
        let entry = ok(self.b.begin_block(None))?;
        let mut hash = self.u32c(0x811C_9DC5);
        let prime = self.u32c(0x0100_0193);
        for word in &key {
            hash = ok(self.b.bitwise_xor(u32_, None, hash, *word))?;
            hash = ok(self.b.i_mul(u32_, None, hash, prime))?;
        }
        let zero = self.u32c(0);
        let ptr = self.element(table, zero)?;
        let mask = self.load(ptr)?;
        let (header, body, cont, merge) = (self.b.id(), self.b.id(), self.b.id(), self.b.id());
        let (next, result) = (self.b.id(), self.b.id());
        ok(self.b.branch(header))?;
        ok(self.b.begin_block(Some(header)))?;
        let probe = ok(self.b.phi(u32_, None, [(zero, entry), (next, cont)]))?;
        let current = ok(self.b.phi(u32_, None, [(zero, entry), (result, cont)]))?;
        ok(self.b.loop_merge(merge, cont, LoopControl::NONE, []))?;
        ok(self.b.branch(body))?;
        ok(self.b.begin_block(Some(body)))?;
        let at = ok(self.b.i_add(u32_, None, hash, probe))?;
        let at = ok(self.b.bitwise_and(u32_, None, at, mask))?;
        let stride = self.u32c(images::ENTRY_WORDS as u32);
        let base = ok(self.b.i_mul(u32_, None, at, stride))?;
        let one = self.u32c(1);
        let base = ok(self.b.i_add(u32_, None, base, one))?;
        let word = |e: &mut Self, offset: u32| -> Result<Word, EmitError> {
            let offset = e.u32c(offset);
            let index = ok(e.b.i_add(u32_, None, base, offset))?;
            let ptr = e.element(table, index)?;
            e.load(ptr)
        };
        let stored = word(self, 8)?;
        let mut matches = self.boolc(true);
        for (offset, part) in key.iter().enumerate() {
            let value = word(self, offset as u32)?;
            let same = ok(self.b.i_equal(bool_, None, value, *part))?;
            matches = ok(self.b.logical_and(bool_, None, matches, same))?;
        }
        let empty_word = self.u32c(images::EMPTY);
        let empty = ok(self.b.i_equal(bool_, None, stored, empty_word))?;
        let occupied = ok(self.b.logical_not(bool_, None, empty))?;
        let hit = ok(self.b.logical_and(bool_, None, matches, occupied))?;
        ok(self.b.select(u32_, Some(result), hit, stored, current))?;
        ok(self.b.i_add(u32_, Some(next), probe, one))?;
        let probes = self.u32c(images::PROBES);
        let spent = ok(self.b.u_greater_than_equal(bool_, None, next, probes))?;
        let stop = ok(self.b.logical_or(bool_, None, empty, hit))?;
        let stop = ok(self.b.logical_or(bool_, None, stop, spent))?;
        ok(self.b.branch(cont))?;
        ok(self.b.begin_block(Some(cont)))?;
        ok(self.b.branch_conditional(stop, merge, header, []))?;
        ok(self.b.begin_block(Some(merge)))?;
        ok(self.b.ret_value(result))?;
        ok(self.b.end_function())?;
        Ok(())
    }

    pub(super) fn image(&mut self, image: &Image) -> Result<Word, EmitError> {
        let component = usize::from(image.component);
        if let Some((cached, words)) = &self.image_cache
            && cached.same_instruction(image)
        {
            return Ok(words[component]);
        }
        let words = self.image_words(image)?;
        self.image_cache = Some((image.clone(), words));
        Ok(words[component])
    }

    /// The bindless index and number type registered for the instruction's T#.
    fn texture_slot(&mut self, image: &Image) -> Result<(Word, Word), EmitError> {
        let u32_ = self.t.u32_;
        let key: Vec<Word> = image
            .resource
            .iter()
            .map(|t| self.values[*t as usize])
            .collect();
        let lookup = self.lookup_function(0);
        let entry = ok(self.b.function_call(u32_, None, lookup, key))?;
        let mask = self.u32c(images::SLOT_MASK);
        let index = ok(self.b.bitwise_and(u32_, None, entry, mask))?;
        let thirty = self.u32c(30);
        let number = ok(self.b.shift_right_logical(u32_, None, entry, thirty))?;
        Ok((index, number))
    }

    fn sampler_handle(&mut self, words: [u32; 4], compare: bool) -> Result<Word, EmitError> {
        let u32_ = self.t.u32_;
        let types = self.image_types();
        let zero = self.u32c(0);
        let key: Vec<Word> = words
            .iter()
            .map(|t| self.values[*t as usize])
            .chain([zero; 4])
            .collect();
        let lookup = self.lookup_function(1);
        let entry = ok(self.b.function_call(u32_, None, lookup, key))?;
        let mask = self.u32c(images::SLOT_MASK);
        let slot = ok(self.b.bitwise_and(u32_, None, entry, mask))?;
        let array = if compare {
            self.compare_samplers()
        } else {
            types.samplers
        };
        let ptr = ok(self.b.access_chain(types.ptr_sampler, None, array, [slot]))?;
        let ptr = self.nonuniform(ptr);
        let loaded = ok(self.b.load(types.sampler, None, ptr, None, []))?;
        Ok(self.nonuniform(loaded))
    }

    /// Runs `arm` once per number type, switching on the registered one, and joins the four result words.
    fn per_number(
        &mut self,
        number: Word,
        mut arm: impl FnMut(&mut Self, Number) -> Result<Option<Word>, EmitError>,
    ) -> Result<Option<Word>, EmitError> {
        let labels = [self.b.id(), self.b.id(), self.b.id()];
        let merge = self.b.id();
        ok(self.b.selection_merge(merge, SelectionControl::NONE))?;
        ok(self.b.switch(
            number,
            labels[0],
            [
                (Operand::LiteralBit32(1), labels[1]),
                (Operand::LiteralBit32(2), labels[2]),
            ],
        ))?;
        let mut arms = Vec::new();
        for (number, label) in NUMBERS.into_iter().zip(labels) {
            self.begin(label)?;
            if let Some(bits) = arm(self, number)? {
                arms.push((bits, self.label));
            }
            ok(self.b.branch(merge))?;
        }
        self.begin(merge)?;
        if arms.is_empty() {
            return Ok(None);
        }
        let uvec4 = self.t.uvec4;
        Ok(Some(ok(self.b.phi(uvec4, None, arms))?))
    }

    fn image_words(&mut self, image: &Image) -> Result<[Word; 4], EmitError> {
        if let ImageOp::Atomic(op) = image.op {
            return self.image_atomic(image, op);
        }
        if image.op.implicit() && self.pixel.is_none() {
            if image.op == ImageOp::QueryLod {
                return Err(EmitError::Unsupported(
                    "level-of-detail query outside a pixel shader",
                ));
            }
            if image.unnormalized {
                return Err(EmitError::Unsupported(
                    "implicit-LOD sampling at unnormalized coordinates outside a pixel shader",
                ));
            }
        }
        let sampled = image.sampler.is_some();
        if sampled
            && image.unnormalized
            && (image.dim == ImageDim::Cube || !image.gradients.is_empty())
        {
            return Err(EmitError::Unsupported(
                "unnormalized coordinates on a cube map or with gradients",
            ));
        }
        if image.offset.is_some() && matches!(image.dim, ImageDim::Cube | ImageDim::D3) {
            return Err(EmitError::Unsupported("offsets on cube maps and volumes"));
        }
        let u32_ = self.t.u32_;
        let view = View::of(image.dim);
        let (index, number) = self.texture_slot(image)?;
        let sampler = match image.sampler {
            Some(words) => Some(self.sampler_handle(words, image.compare.is_some())?),
            None => None,
        };
        let values: Vec<Word> = image
            .coordinates
            .iter()
            .map(|t| self.values[*t as usize])
            .collect();
        let coordinates = match image.op {
            ImageOp::Size => 0,
            _ if sampled => self.sample_coordinates(image.dim, &values)?,
            _ => self.load_coordinates(image.dim, &values)?,
        };
        let extras = self.sample_extras(image, view, coordinates)?;
        let result = self.per_number(number, |e, number| {
            e.image_arm(image, view, number, index, sampler, coordinates, &extras)
                .map(Some)
        })?;
        let result = result.expect("every arm returns a value");
        let mut words = [0; 4];
        for (component, word) in words.iter_mut().enumerate() {
            *word = ok(self
                .b
                .composite_extract(u32_, None, result, [component as u32]))?;
        }
        Ok(words)
    }

    fn sample_extras(
        &mut self,
        image: &Image,
        view: View,
        coordinates: Word,
    ) -> Result<Extras, EmitError> {
        let types = self.image_types();
        let f32_ = self.t.f32_;
        let float = |e: &mut Self, t: Option<u32>| -> Result<Option<Word>, EmitError> {
            t.map(|t| e.float(e.values[t as usize])).transpose()
        };
        let compare = float(self, image.compare)?;
        let clamp = float(self, image.clamp)?;
        let gradients = if image.gradients.is_empty() {
            None
        } else {
            let each = image.gradients.len() / 2;
            let spatial = if view == View::D3 { 3 } else { 2 };
            let mut directions = Vec::new();
            for direction in image.gradients.chunks(each) {
                let mut parts = Vec::new();
                for t in direction {
                    parts.push(self.float(self.values[*t as usize])?);
                }
                let zero = self.f32c(0.0);
                parts.resize(spatial, zero);
                directions.push(ok(self.b.composite_construct(
                    types.floats[spatial],
                    None,
                    parts,
                ))?);
            }
            Some((directions[0], directions[1]))
        };
        let gradients = if image.op.implicit() && self.pixel.is_none() {
            Some(self.quad_gradients(image, view, coordinates)?)
        } else {
            gradients
        };
        let gather = matches!(image.op, ImageOp::Gather { .. });
        let offset = match image.offset {
            Some(t) if !gather => {
                let word = self.values[t as usize];
                let packed = self
                    .constants
                    .iter()
                    .find_map(|((kind, bits), id)| (*kind == 0 && *id == word).then_some(*bits))
                    .ok_or(EmitError::Unsupported(
                        "sampling with an offset that is not a constant",
                    ))? as u32;
                let parts: Vec<Word> = [0, 8]
                    .iter()
                    .map(|shift| {
                        let field = ((packed >> shift) << 26) as i32 >> 26;
                        self.i32c(field)
                    })
                    .collect();
                Some(self.b.constant_composite(types.ints[2], parts))
            }
            Some(t) => {
                let packed = self.values[t as usize];
                let u32_ = self.t.u32_;
                let mut parts = Vec::new();
                for shift in [26, 18] {
                    let up = self.u32c(shift);
                    let raised = ok(self.b.shift_left_logical(u32_, None, packed, up))?;
                    let down = self.u32c(26);
                    let value = ok(self.b.shift_right_arithmetic(u32_, None, raised, down))?;
                    parts.push(ok(self.b.bitcast(types.i32_, None, value))?);
                }
                if view == View::D2Array || view == View::D2 {
                    self.b.capability(Capability::ImageGatherExtended);
                }
                Some(ok(self.b.composite_construct(types.ints[2], None, parts))?)
            }
            None => None,
        };
        if clamp.is_some() {
            self.b.capability(Capability::MinLod);
        }
        let _ = f32_;
        Ok(Extras {
            compare,
            clamp,
            gradients,
            offset,
        })
    }

    /// The hardware's implicit derivatives outside a pixel shader: across each quad of lanes, horizontal from
    /// its first lane to the second and vertical from the first to the third, scaled by 2^bias.
    fn quad_gradients(
        &mut self,
        image: &Image,
        view: View,
        coordinates: Word,
    ) -> Result<(Word, Word), EmitError> {
        let types = self.image_types();
        let f32_ = self.t.f32_;
        self.b.capability(Capability::GroupNonUniformQuad);
        let spatial = if view == View::D3 { 3 } else { 2 };
        let ty = types.floats[spatial];
        let width = image.coordinates.len()
            + usize::from(matches!(image.dim, ImageDim::D1 | ImageDim::D1Array));
        let point = if width == spatial {
            coordinates
        } else {
            let parts: Vec<u32> = (0..spatial as u32).collect();
            ok(self
                .b
                .vector_shuffle(ty, None, coordinates, coordinates, parts))?
        };
        let scope = self.u32c(3);
        let corner = |e: &mut Self, lane: u32| {
            let lane = e.u32c(lane);
            ok(e.b.group_non_uniform_quad_broadcast(ty, None, scope, point, lane))
        };
        let first = corner(self, 0)?;
        let right = corner(self, 1)?;
        let below = corner(self, 2)?;
        let mut dx = ok(self.b.f_sub(ty, None, right, first))?;
        let mut dy = ok(self.b.f_sub(ty, None, below, first))?;
        if image.op == ImageOp::SampleBias
            && let Some(bias) = image.lod
        {
            let bias = self.float(self.values[bias as usize])?;
            let scale = self.glsl(f32_, GLOp::Exp2, &[bias])?;
            dx = ok(self.b.vector_times_scalar(ty, None, dx, scale))?;
            dy = ok(self.b.vector_times_scalar(ty, None, dy, scale))?;
        }
        Ok((dx, dy))
    }

    #[allow(clippy::too_many_arguments)]
    fn image_arm(
        &mut self,
        image: &Image,
        view: View,
        number: Number,
        index: Word,
        sampler: Option<Word>,
        coordinates: Word,
        extras: &Extras,
    ) -> Result<Word, EmitError> {
        let types = self.image_types();
        let ids = self.view(view, number, false);
        let ptr = ok(self.b.access_chain(ids.ptr, None, ids.var, [index]))?;
        let ptr = self.nonuniform(ptr);
        let texture = ok(self.b.load(ids.image, None, ptr, None, []))?;
        let texture = self.nonuniform(texture);
        let vec4 = match number {
            Number::Float => types.floats[4],
            Number::Uint => self.t.uvec4,
            Number::Sint => types.ints[4],
        };
        let lod = image.lod.map(|t| self.values[t as usize]);
        if image.op == ImageOp::Size {
            return self.size_query(image.dim, view, texture, lod);
        }
        let value = match (image.op, sampler) {
            (ImageOp::Load | ImageOp::LoadMip, _) => {
                let mip = match lod {
                    Some(mip) => ok(self.b.bitcast(types.i32_, None, mip))?,
                    None => self.i32c(0),
                };
                ok(self.b.image_fetch(
                    vec4,
                    None,
                    texture,
                    coordinates,
                    Some(ImageOperands::LOD),
                    [Operand::IdRef(mip)],
                ))?
            }
            (op, Some(sampler)) => {
                let coordinates = if image.unnormalized {
                    self.normalize(texture, view, coordinates)?
                } else {
                    coordinates
                };
                let combined = ok(self.b.sampled_image(ids.sampled, None, texture, sampler))?;
                let combined = self.nonuniform(combined);
                if op == ImageOp::QueryLod {
                    if number != Number::Float {
                        let zero = self.u32c(0);
                        return ok(self.b.composite_construct(self.t.uvec4, None, [zero; 4]));
                    }
                    let vec2 = types.floats[2];
                    let lods = ok(self.b.image_query_lod(vec2, None, combined, coordinates))?;
                    let f32_ = self.t.f32_;
                    let mut words = Vec::new();
                    for axis in 0..2 {
                        let part = ok(self.b.composite_extract(f32_, None, lods, [axis]))?;
                        words.push(self.bits(part)?);
                    }
                    let zero = self.u32c(0);
                    words.extend([zero, zero]);
                    return ok(self.b.composite_construct(self.t.uvec4, None, words));
                }
                let level = |e: &mut Self| -> Result<Level, EmitError> {
                    let with_offset = |mut mask: ImageOperands, mut operands: Vec<Operand>| {
                        if let Some(offset) = extras.offset {
                            mask |= ImageOperands::CONST_OFFSET;
                            operands.push(Operand::IdRef(offset));
                        }
                        (mask, operands)
                    };
                    let with_clamp = |mask: ImageOperands, operands: Vec<Operand>| {
                        let (mut mask, mut operands) = with_offset(mask, operands);
                        if let Some(clamp) = extras.clamp {
                            mask |= ImageOperands::MIN_LOD;
                            operands.push(Operand::IdRef(clamp));
                        }
                        (mask, operands)
                    };
                    Ok(match (op, extras.gradients) {
                        (
                            ImageOp::SampleGrad | ImageOp::Sample | ImageOp::SampleBias,
                            Some((dx, dy)),
                        ) => {
                            let (mask, operands) = with_clamp(
                                ImageOperands::GRAD,
                                vec![Operand::IdRef(dx), Operand::IdRef(dy)],
                            );
                            Level::Explicit(mask, operands)
                        }
                        (ImageOp::Sample, _) => {
                            let (mask, operands) = with_clamp(ImageOperands::NONE, Vec::new());
                            Level::Implicit((!mask.is_empty()).then_some(mask), operands)
                        }
                        (ImageOp::SampleBias, _) => {
                            let bias = match lod {
                                Some(bias) => e.float(bias)?,
                                None => e.f32c(0.0),
                            };
                            let (mask, operands) =
                                with_clamp(ImageOperands::BIAS, vec![Operand::IdRef(bias)]);
                            Level::Implicit(Some(mask), operands)
                        }
                        _ => {
                            let level = match lod {
                                Some(level) if op == ImageOp::SampleLod => e.float(level)?,
                                _ => e.f32c(0.0),
                            };
                            let (mask, operands) =
                                with_offset(ImageOperands::LOD, vec![Operand::IdRef(level)]);
                            Level::Explicit(mask, operands)
                        }
                    })
                };
                let gather_operands = match extras.offset {
                    Some(offset) => (Some(ImageOperands::OFFSET), vec![Operand::IdRef(offset)]),
                    None => (None, Vec::new()),
                };
                match (op, extras.compare) {
                    (_, Some(_)) if number != Number::Float => {
                        let zero = self.u32c(0);
                        return ok(self.b.composite_construct(self.t.uvec4, None, [zero; 4]));
                    }
                    (ImageOp::Gather { .. }, Some(reference)) => ok(self.b.image_dref_gather(
                        vec4,
                        None,
                        combined,
                        coordinates,
                        reference,
                        gather_operands.0,
                        gather_operands.1,
                    ))?,
                    (_, Some(reference)) => {
                        let f32_ = self.t.f32_;
                        let scalar = match level(self)? {
                            Level::Explicit(mask, operands) => {
                                ok(self.b.image_sample_dref_explicit_lod(
                                    f32_,
                                    None,
                                    combined,
                                    coordinates,
                                    reference,
                                    mask,
                                    operands,
                                ))?
                            }
                            Level::Implicit(mask, operands) => {
                                ok(self.b.image_sample_dref_implicit_lod(
                                    f32_,
                                    None,
                                    combined,
                                    coordinates,
                                    reference,
                                    mask,
                                    operands,
                                ))?
                            }
                        };
                        ok(self.b.composite_construct(vec4, None, [scalar; 4]))?
                    }
                    (ImageOp::Gather { .. }, None)
                        if lod.is_some() && image.dim == ImageDim::Cube =>
                    {
                        return Err(EmitError::Unsupported("gathers from cube maps at a level"));
                    }
                    (ImageOp::Gather { channel }, None) if lod.is_some() => {
                        let words = image.sampler.expect("gathers are sampled");
                        let modes = self.values[words[0] as usize];
                        let level = self.float(lod.expect("checked by the guard"))?;
                        self.gather_at_level(
                            view,
                            vec4,
                            texture,
                            coordinates,
                            level,
                            channel,
                            modes,
                            extras.offset,
                        )?
                    }
                    (ImageOp::Gather { channel }, None) => {
                        let channel = self.u32c(u32::from(channel));
                        ok(self.b.image_gather(
                            vec4,
                            None,
                            combined,
                            coordinates,
                            channel,
                            gather_operands.0,
                            gather_operands.1,
                        ))?
                    }
                    (_, None) => match level(self)? {
                        Level::Explicit(mask, operands) => ok(self.b.image_sample_explicit_lod(
                            vec4,
                            None,
                            combined,
                            coordinates,
                            mask,
                            operands,
                        ))?,
                        Level::Implicit(mask, operands) => ok(self.b.image_sample_implicit_lod(
                            vec4,
                            None,
                            combined,
                            coordinates,
                            mask,
                            operands,
                        ))?,
                    },
                }
            }
            (_, None) => return Err(EmitError::Unsupported("sampling without a sampler")),
        };
        let bits = match number {
            Number::Uint => value,
            _ => ok(self.b.bitcast(self.t.uvec4, None, value))?,
        };
        if !image.d16 {
            return Ok(bits);
        }
        let u32_ = self.t.u32_;
        let mut halves = Vec::new();
        for component in 0..4 {
            let word = ok(self.b.composite_extract(u32_, None, bits, [component]))?;
            halves.push(match number {
                Number::Float => self.single_to_half(word, shader_ir::Round::Even)?,
                _ => {
                    let mask = self.u32c(0xFFFF);
                    ok(self.b.bitwise_and(u32_, None, word, mask))?
                }
            });
        }
        ok(self.b.composite_construct(self.t.uvec4, None, halves))
    }

    /// A gather at an explicit level, which Vulkan has no instruction for: the mip nearest the level, the
    /// footprint's four texels addressed by the sampler's wrap, mirror or clamp modes, then fetched.
    #[allow(clippy::too_many_arguments)]
    fn gather_at_level(
        &mut self,
        view: View,
        vec4: Word,
        texture: Word,
        coordinates: Word,
        level: Word,
        channel: u8,
        modes: Word,
        offset: Option<Word>,
    ) -> Result<Word, EmitError> {
        let types = self.image_types();
        let (u32_, f32_, i32_) = (self.t.u32_, self.t.f32_, types.i32_);
        let half = self.f32c(0.5);
        let raised = ok(self.b.f_add(f32_, None, level, half))?;
        let nearest = self.glsl(f32_, GLOp::Floor, &[raised])?;
        let nearest = ok(self.b.convert_f_to_s(i32_, None, nearest))?;
        let levels = ok(self.b.image_query_levels(i32_, None, texture))?;
        let (zero, one) = (self.i32c(0), self.i32c(1));
        let last = ok(self.b.i_sub(i32_, None, levels, one))?;
        let mip = self.glsl(i32_, GLOp::SClamp, &[nearest, zero, last])?;
        let count = if view == View::D2 { 2 } else { 3 };
        let size = ok(self
            .b
            .image_query_size_lod(types.ints[count], None, texture, mip))?;
        let mut axes = Vec::new();
        for axis in 0..2u32 {
            let extent = ok(self.b.composite_extract(i32_, None, size, [axis]))?;
            let coordinate = ok(self.b.composite_extract(f32_, None, coordinates, [axis]))?;
            let scale = ok(self.b.convert_s_to_f(f32_, None, extent))?;
            let texel = ok(self.b.f_mul(f32_, None, coordinate, scale))?;
            let texel = ok(self.b.f_sub(f32_, None, texel, half))?;
            let texel = self.glsl(f32_, GLOp::Floor, &[texel])?;
            let mut first = ok(self.b.convert_f_to_s(i32_, None, texel))?;
            if let Some(offset) = offset {
                let shift = ok(self.b.composite_extract(i32_, None, offset, [axis]))?;
                first = ok(self.b.i_add(i32_, None, first, shift))?;
            }
            let second = ok(self.b.i_add(i32_, None, first, one))?;
            let shift = self.u32c(3 * axis);
            let mode = ok(self.b.shift_right_logical(u32_, None, modes, shift))?;
            let seven = self.u32c(7);
            let mode = ok(self.b.bitwise_and(u32_, None, mode, seven))?;
            let mut pair = Vec::new();
            for texel in [first, second] {
                pair.push(self.address_texel(texel, extent, mode)?);
            }
            axes.push(pair);
        }
        let layer = if count == 3 {
            let coordinate = ok(self.b.composite_extract(f32_, None, coordinates, [2]))?;
            let rounded = self.glsl(f32_, GLOp::RoundEven, &[coordinate])?;
            let rounded = ok(self.b.convert_f_to_s(i32_, None, rounded))?;
            let layers = ok(self.b.composite_extract(i32_, None, size, [2]))?;
            let top = ok(self.b.i_sub(i32_, None, layers, one))?;
            Some(self.glsl(i32_, GLOp::SClamp, &[rounded, zero, top])?)
        } else {
            None
        };
        let component = if vec4 == self.t.uvec4 {
            u32_
        } else if vec4 == types.ints[4] {
            i32_
        } else {
            f32_
        };
        let mut texels = Vec::new();
        for (x, y) in [(0, 1), (1, 1), (1, 0), (0, 0)] {
            let mut parts = vec![axes[0][x], axes[1][y]];
            parts.extend(layer);
            let at = ok(self.b.composite_construct(types.ints[count], None, parts))?;
            let fetched = ok(self.b.image_fetch(
                vec4,
                None,
                texture,
                at,
                Some(ImageOperands::LOD),
                [Operand::IdRef(mip)],
            ))?;
            texels.push(ok(self.b.composite_extract(
                component,
                None,
                fetched,
                [u32::from(channel)],
            ))?);
        }
        ok(self.b.composite_construct(vec4, None, texels))
    }

    /// A texel index brought inside `0..extent` by a sampler addressing mode: 0 wraps, 1 mirrors, 3, 5 and 7
    /// mirror once, and the rest clamp to the edge.
    fn address_texel(&mut self, texel: Word, extent: Word, mode: Word) -> Result<Word, EmitError> {
        let (u32_, bool_, i32_) = (self.t.u32_, self.t.bool_, self.image_types().i32_);
        let (zero, one) = (self.i32c(0), self.i32c(1));
        let last = ok(self.b.i_sub(i32_, None, extent, one))?;
        let wrapped = ok(self.b.s_mod(i32_, None, texel, extent))?;
        let period = ok(self.b.i_add(i32_, None, extent, extent))?;
        let cycle = ok(self.b.s_mod(i32_, None, texel, period))?;
        let back = ok(self.b.i_sub(i32_, None, period, one))?;
        let back = ok(self.b.i_sub(i32_, None, back, cycle))?;
        let forward = ok(self.b.s_less_than(bool_, None, cycle, extent))?;
        let mirrored = ok(self.b.select(i32_, None, forward, cycle, back))?;
        let negative = ok(self.b.s_less_than(bool_, None, texel, zero))?;
        let flipped = ok(self.b.i_sub(i32_, None, zero, texel))?;
        let flipped = ok(self.b.i_sub(i32_, None, flipped, one))?;
        let once = ok(self.b.select(i32_, None, negative, flipped, texel))?;
        let once = self.glsl(i32_, GLOp::SClamp, &[once, zero, last])?;
        let clamped = self.glsl(i32_, GLOp::SClamp, &[texel, zero, last])?;
        let is = |e: &mut Self, value: u32| {
            let constant = e.u32c(value);
            ok(e.b.i_equal(bool_, None, mode, constant))
        };
        let wrap = is(self, 0)?;
        let mirror = is(self, 1)?;
        let low = self.u32c(1);
        let low_bit = ok(self.b.bitwise_and(u32_, None, mode, low))?;
        let odd = ok(self.b.i_equal(bool_, None, low_bit, low))?;
        let result = ok(self.b.select(i32_, None, odd, once, clamped))?;
        let result = ok(self.b.select(i32_, None, mirror, mirrored, result))?;
        ok(self.b.select(i32_, None, wrap, wrapped, result))
    }

    /// Width, height, depth or layers, and the mip count; zero sizes past the last mip.
    fn size_query(
        &mut self,
        dim: ImageDim,
        view: View,
        texture: Word,
        mip: Option<Word>,
    ) -> Result<Word, EmitError> {
        let types = self.image_types();
        let (u32_, bool_) = (self.t.u32_, self.t.bool_);
        let levels = ok(self.b.image_query_levels(types.i32_, None, texture))?;
        let levels = ok(self.b.bitcast(u32_, None, levels))?;
        let mip = match mip {
            Some(mip) => mip,
            None => self.u32c(0),
        };
        let one = self.u32c(1);
        let last = ok(self.b.i_sub(u32_, None, levels, one))?;
        let level = self.glsl(u32_, GLOp::UMin, &[mip, last])?;
        let level = ok(self.b.bitcast(types.i32_, None, level))?;
        let count = if view == View::D2 { 2 } else { 3 };
        let size = ok(self
            .b
            .image_query_size_lod(types.ints[count], None, texture, level))?;
        let mut parts = Vec::new();
        for axis in 0..count as u32 {
            let part = ok(self.b.composite_extract(types.i32_, None, size, [axis]))?;
            parts.push(ok(self.b.bitcast(u32_, None, part))?);
        }
        let inside = ok(self.b.u_less_than(bool_, None, mip, levels))?;
        let zero = self.u32c(0);
        let x = parts[0];
        let (y, z) = match dim {
            ImageDim::D1 => (one, one),
            ImageDim::D1Array => (parts[2], one),
            ImageDim::D2 => (parts[1], one),
            ImageDim::Cube => {
                let six = self.u32c(6);
                (parts[1], ok(self.b.u_div(u32_, None, parts[2], six))?)
            }
            ImageDim::D2Array | ImageDim::D3 => (parts[1], parts[2]),
        };
        let mut words = Vec::new();
        for value in [x, y, z] {
            words.push(ok(self.b.select(u32_, None, inside, value, zero))?);
        }
        words.push(levels);
        ok(self.b.composite_construct(self.t.uvec4, None, words))
    }

    pub(super) fn image_store(
        &mut self,
        image: &Image,
        values: [Option<u32>; 4],
    ) -> Result<(), EmitError> {
        let types = self.image_types();
        let u32_ = self.t.u32_;
        let view = View::of(image.dim);
        let (index, number) = self.texture_slot(image)?;
        let coordinates: Vec<Word> = image
            .coordinates
            .iter()
            .map(|t| self.values[*t as usize])
            .collect();
        let coordinates = self.load_coordinates(image.dim, &coordinates)?;
        let zero = self.u32c(0);
        let words: Vec<Word> = values
            .iter()
            .map(|v| v.map_or(zero, |t| self.values[t as usize]))
            .collect();
        self.per_number(number, |e, number| {
            let ids = e.view(view, number, true);
            let ptr = ok(e.b.access_chain(ids.ptr, None, ids.var, [index]))?;
            let ptr = e.nonuniform(ptr);
            let texture = ok(e.b.load(ids.image, None, ptr, None, []))?;
            let texture = e.nonuniform(texture);
            let mut parts = Vec::new();
            for word in &words {
                parts.push(if image.d16 {
                    match number {
                        Number::Float => e.half_to_single(*word, true)?,
                        Number::Uint => *word,
                        Number::Sint => {
                            let sixteen = e.u32c(16);
                            let up = ok(e.b.shift_left_logical(u32_, None, *word, sixteen))?;
                            ok(e.b.shift_right_arithmetic(u32_, None, up, sixteen))?
                        }
                    }
                } else {
                    *word
                });
            }
            let texel = ok(e.b.composite_construct(e.t.uvec4, None, parts))?;
            let texel = match number {
                Number::Uint => texel,
                Number::Float => ok(e.b.bitcast(types.floats[4], None, texel))?,
                Number::Sint => ok(e.b.bitcast(types.ints[4], None, texel))?,
            };
            ok(e.b.image_write(texture, coordinates, texel, None, []))?;
            Ok(None)
        })?;
        Ok(())
    }
    /// Hardware cube coordinates are face-relative in [1, 2]; the 2D array view wants [0, 1].
    fn sample_coordinates(&mut self, dim: ImageDim, values: &[Word]) -> Result<Word, EmitError> {
        let types = self.image_types();
        let mut parts = Vec::new();
        for value in values {
            parts.push(self.float(*value)?);
        }
        let half = self.f32c(0.5);
        match dim {
            ImageDim::D1 => parts.push(half),
            ImageDim::D1Array => parts.insert(1, half),
            ImageDim::Cube => {
                let one = self.f32c(1.0);
                let f32_ = self.t.f32_;
                for part in parts.iter_mut().take(2) {
                    *part = ok(self.b.f_sub(f32_, None, *part, one))?;
                }
            }
            ImageDim::D2 | ImageDim::D2Array | ImageDim::D3 => {}
        }
        let ty = types.floats[parts.len()];
        ok(self.b.composite_construct(ty, None, parts))
    }

    fn load_coordinates(&mut self, dim: ImageDim, values: &[Word]) -> Result<Word, EmitError> {
        let types = self.image_types();
        let mut parts = Vec::new();
        for value in values {
            parts.push(ok(self.b.bitcast(types.i32_, None, *value))?);
        }
        let zero = self.i32c(0);
        match dim {
            ImageDim::D1 => parts.push(zero),
            ImageDim::D1Array => parts.insert(1, zero),
            _ => {}
        }
        let ty = types.ints[parts.len()];
        ok(self.b.composite_construct(ty, None, parts))
    }

    fn normalize(
        &mut self,
        texture: Word,
        view: View,
        coordinates: Word,
    ) -> Result<Word, EmitError> {
        let types = self.image_types();
        let f32_ = self.t.f32_;
        let (count, spatial) = match view {
            View::D2 => (2, 2),
            View::D2Array => (3, 2),
            View::D3 => (3, 3),
        };
        let level = self.i32c(0);
        let size = ok(self
            .b
            .image_query_size_lod(types.ints[count], None, texture, level))?;
        let size = ok(self.b.convert_s_to_f(types.floats[count], None, size))?;
        let mut parts = Vec::new();
        for axis in 0..count as u32 {
            let part = ok(self.b.composite_extract(f32_, None, coordinates, [axis]))?;
            parts.push(if axis < spatial {
                let extent = ok(self.b.composite_extract(f32_, None, size, [axis]))?;
                ok(self.b.f_div(f32_, None, part, extent))?
            } else {
                part
            });
        }
        ok(self.b.composite_construct(types.floats[count], None, parts))
    }
}
