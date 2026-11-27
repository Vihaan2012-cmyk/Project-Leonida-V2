use rspirv::dr::Operand;
use rspirv::spirv::{
    Capability, Decoration, Dim, FunctionControl, ImageFormat, ImageOperands, LoopControl,
    SelectionControl, StorageClass, Word,
};
use shader_ir::{Image, ImageDim, ImageOp};

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
        if image.op.implicit() {
            return Err(EmitError::Unsupported(
                "implicit-LOD sampling outside a pixel shader",
            ));
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
        let extras = self.sample_extras(image, view)?;
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

    fn sample_extras(&mut self, image: &Image, view: View) -> Result<Extras, EmitError> {
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
        let offset = match image.offset {
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
                let level = |e: &mut Self| -> Result<(ImageOperands, Vec<Operand>), EmitError> {
                    Ok(match (op, extras.gradients) {
                        (ImageOp::SampleGrad, Some((dx, dy))) => {
                            let mut operands = vec![Operand::IdRef(dx), Operand::IdRef(dy)];
                            let mut mask = ImageOperands::GRAD;
                            if let Some(clamp) = extras.clamp {
                                mask |= ImageOperands::MIN_LOD;
                                operands.push(Operand::IdRef(clamp));
                            }
                            (mask, operands)
                        }
                        _ => {
                            let level = match lod {
                                Some(level) if op == ImageOp::SampleLod => e.float(level)?,
                                _ => e.f32c(0.0),
                            };
                            (ImageOperands::LOD, vec![Operand::IdRef(level)])
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
                        let (mask, operands) = level(self)?;
                        let f32_ = self.t.f32_;
                        let scalar = ok(self.b.image_sample_dref_explicit_lod(
                            f32_,
                            None,
                            combined,
                            coordinates,
                            reference,
                            mask,
                            operands,
                        ))?;
                        ok(self.b.composite_construct(vec4, None, [scalar; 4]))?
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
                    (_, None) => {
                        let (mask, operands) = level(self)?;
                        ok(self.b.image_sample_explicit_lod(
                            vec4,
                            None,
                            combined,
                            coordinates,
                            mask,
                            operands,
                        ))?
                    }
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
