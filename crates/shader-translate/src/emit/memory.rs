use rspirv::dr::Operand;
use rspirv::spirv::{Capability, FunctionControl, SelectionControl, Word};
use shader_ir::{AtomicOp, Space};

use super::{EmitError, Emitter, GLOp, ok};

impl Emitter {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn atomic(
        &mut self,
        space: Space,
        op: AtomicOp,
        address: Word,
        value: Word,
        compare: Option<Word>,
        when: Option<Word>,
    ) -> Result<Word, EmitError> {
        let (u32_, u64_, bool_) = (self.t.u32_, self.t.u64_, self.t.bool_);
        if space == Space::Lds && self.pixel.is_some() {
            return Err(EmitError::Unsupported("LDS in a pixel shader"));
        }
        let mut condition = self.active()?;
        if let Some(when) = when {
            condition = ok(self.b.logical_and(bool_, None, condition, when))?;
        }
        let before = self.label;
        let then = self.b.id();
        let merge = self.b.id();
        ok(self.b.selection_merge(merge, SelectionControl::NONE))?;
        ok(self.b.branch_conditional(condition, then, merge, []))?;
        self.begin(then)?;
        let (ptr, scope) = match space {
            Space::Lds => {
                let index = self.lds_index(address)?;
                (self.lds_element(index)?, 2)
            }
            Space::Memory if self.heap.is_some() => {
                let mask = self.u64c(!3);
                let aligned = ok(self.b.bitwise_and(u64_, None, address, mask))?;
                (self.heap_pointer(aligned)?, 1)
            }
            Space::Memory => {
                let index = self.memory_index(address)?;
                let memory = self.memory;
                (self.element(memory, index)?, 1)
            }
        };
        let scope = self.u32c(scope);
        let relaxed = self.u32c(0);
        let b = &mut self.b;
        let old = ok(match op {
            AtomicOp::Swap => b.atomic_exchange(u32_, None, ptr, scope, relaxed, value),
            AtomicOp::CompareSwap => b.atomic_compare_exchange(
                u32_,
                None,
                ptr,
                scope,
                relaxed,
                relaxed,
                value,
                compare.expect("compare-swap has a compare value"),
            ),
            AtomicOp::Add => b.atomic_i_add(u32_, None, ptr, scope, relaxed, value),
            AtomicOp::Sub => b.atomic_i_sub(u32_, None, ptr, scope, relaxed, value),
            AtomicOp::MinS => b.atomic_s_min(u32_, None, ptr, scope, relaxed, value),
            AtomicOp::MinU => b.atomic_u_min(u32_, None, ptr, scope, relaxed, value),
            AtomicOp::MaxS => b.atomic_s_max(u32_, None, ptr, scope, relaxed, value),
            AtomicOp::MaxU => b.atomic_u_max(u32_, None, ptr, scope, relaxed, value),
            AtomicOp::And => b.atomic_and(u32_, None, ptr, scope, relaxed, value),
            AtomicOp::Or => b.atomic_or(u32_, None, ptr, scope, relaxed, value),
            AtomicOp::Xor => b.atomic_xor(u32_, None, ptr, scope, relaxed, value),
        })?;
        let end = self.label;
        ok(self.b.branch(merge))?;
        self.begin(merge)?;
        let zero = self.u32c(0);
        let result = ok(self.b.phi(u32_, None, [(old, end), (zero, before)]))?;
        if space == Space::Lds && self.lanes == 64 {
            self.barrier()?;
        }
        Ok(result)
    }

    fn encode_function(&mut self, d16: bool) -> Word {
        let which = usize::from(d16);
        if let Some(id) = self.encode[which] {
            return id;
        }
        let id = self.b.id();
        self.encode[which] = Some(id);
        self.b.capability(Capability::Float64);
        id
    }

    pub(super) fn encode_bodies(&mut self) -> Result<(), EmitError> {
        for which in 0..2 {
            if let Some(id) = self.encode[which] {
                self.encode_body(id, which == 1)?;
            }
        }
        Ok(())
    }

    /// One element's stored words and byte count, for every buffer format; unknown formats store nothing.
    fn encode_body(&mut self, id: Word, d16: bool) -> Result<(), EmitError> {
        let u32_ = self.t.u32_;
        let uvec4 = self.t.uvec4;
        let result = self.encoded_type();
        let function_type = self.b.type_function(result, [u32_; 5]);
        ok(self
            .b
            .begin_function(result, Some(id), FunctionControl::NONE, function_type))?;
        let params: Vec<Word> = (0..5)
            .map(|_| ok(self.b.function_parameter(u32_)))
            .collect::<Result<_, _>>()?;
        let (format, values) = (params[0], [params[1], params[2], params[3], params[4]]);
        ok(self.b.begin_block(None))?;
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
        let mut arms = Vec::new();
        for (code, label) in codes.iter().zip(&labels) {
            ok(self.b.begin_block(Some(*label)))?;
            self.label = *label;
            let layout = rdna::format::buffer_format(*code).expect("listed above");
            let zero = self.u32c(0);
            let mut words = [zero; 4];
            for (channel, value) in values.iter().enumerate().take(layout.count) {
                let value = self.widen(*value, &layout, d16)?;
                let field = self.encode_channel(&layout, channel, value)?;
                let offset = layout.offset(channel);
                let (word, shift) = ((offset / 32) as usize, offset % 32);
                let shift = self.u32c(shift);
                let placed = ok(self.b.shift_left_logical(u32_, None, field, shift))?;
                words[word] = ok(self.b.bitwise_or(u32_, None, words[word], placed))?;
            }
            let packed = ok(self.b.composite_construct(uvec4, None, words))?;
            let bytes = self.u32c(layout.bytes() as u32);
            let value = ok(self.b.composite_construct(result, None, [packed, bytes]))?;
            arms.push((value, *label));
            ok(self.b.branch(merge))?;
        }
        ok(self.b.begin_block(Some(default)))?;
        let zero = self.u32c(0);
        let empty = ok(self.b.composite_construct(uvec4, None, [zero; 4]))?;
        let nothing = ok(self.b.composite_construct(result, None, [empty, zero]))?;
        arms.push((nothing, default));
        ok(self.b.branch(merge))?;
        ok(self.b.begin_block(Some(merge)))?;
        let value = ok(self.b.phi(result, None, arms))?;
        ok(self.b.ret_value(value))?;
        ok(self.b.end_function())?;
        Ok(())
    }

    fn encoded_type(&mut self) -> Word {
        let (uvec4, u32_) = (self.t.uvec4, self.t.u32_);
        self.b.type_struct([uvec4, u32_])
    }

    fn widen(
        &mut self,
        value: Word,
        layout: &rdna::format::BufferFormat,
        d16: bool,
    ) -> Result<Word, EmitError> {
        use rdna::format::Number;
        if !d16 {
            return Ok(value);
        }
        let u32_ = self.t.u32_;
        let mask = self.u32c(0xFFFF);
        let half = ok(self.b.bitwise_and(u32_, None, value, mask))?;
        Ok(match layout.number {
            Number::Uint => half,
            Number::Sint => {
                let sixteen = self.u32c(16);
                let up = ok(self.b.shift_left_logical(u32_, None, half, sixteen))?;
                ok(self.b.shift_right_arithmetic(u32_, None, up, sixteen))?
            }
            _ => self.half_to_single(half, true)?,
        })
    }

    fn encode_channel(
        &mut self,
        layout: &rdna::format::BufferFormat,
        channel: usize,
        value: Word,
    ) -> Result<Word, EmitError> {
        use rdna::format::Number;
        let (u32_, f32_) = (self.t.u32_, self.t.f32_);
        let width = u32::from(layout.widths[channel]);
        let mask = u32::MAX >> (32 - width);
        let field = match layout.number {
            Number::Uint | Number::Sint => value,
            Number::Float if width == 32 => value,
            Number::Float if width == 16 => self.single_to_half(value, shader_ir::Round::Even)?,
            Number::Float => self.single_to_small(value, width - 5)?,
            Number::Uscaled | Number::Sscaled => {
                let signed = layout.number == Number::Sscaled;
                let (low, high) = if signed {
                    (
                        -((1u32 << (width - 1)) as f32),
                        ((1u32 << (width - 1)) - 1) as f32,
                    )
                } else {
                    (0.0, mask as f32)
                };
                let x = self.float(value)?;
                let x = self.not_a_number_is_zero(x)?;
                let (low, high) = (self.f32c(low), self.f32c(high));
                let clamped = self.glsl(f32_, GLOp::NMax, &[x, low])?;
                let clamped = self.glsl(f32_, GLOp::NMin, &[clamped, high])?;
                if signed {
                    ok(self.b.convert_f_to_s(u32_, None, clamped))?
                } else {
                    ok(self.b.convert_f_to_u(u32_, None, clamped))?
                }
            }
            Number::Unorm | Number::Snorm => {
                let signed = layout.number == Number::Snorm;
                let f64_ = self.b.type_float(64, None);
                let x = self.float(value)?;
                let x = self.not_a_number_is_zero(x)?;
                let low = self.f32c(if signed { -1.0 } else { 0.0 });
                let one = self.f32c(1.0);
                let clamped = self.glsl(f32_, GLOp::NMax, &[x, low])?;
                let clamped = self.glsl(f32_, GLOp::NMin, &[clamped, one])?;
                let wide = ok(self.b.f_convert(f64_, None, clamped))?;
                let scale = if signed {
                    ((1u64 << (width - 1)) - 1) as f64
                } else {
                    mask as f64
                };
                let scale = self.b.constant_bit64(f64_, scale.to_bits());
                let scaled = ok(self.b.f_mul(f64_, None, wide, scale))?;
                let scaled = self.exact(scaled);
                let rounded = self.glsl(f64_, GLOp::RoundEven, &[scaled])?;
                if signed {
                    ok(self.b.convert_f_to_s(u32_, None, rounded))?
                } else {
                    ok(self.b.convert_f_to_u(u32_, None, rounded))?
                }
            }
        };
        if width == 32 {
            return Ok(field);
        }
        let mask = self.u32c(mask);
        ok(self.b.bitwise_and(u32_, None, field, mask))
    }

    fn not_a_number_is_zero(&mut self, x: Word) -> Result<Word, EmitError> {
        let (f32_, bool_) = (self.t.f32_, self.t.bool_);
        let nan = ok(self.b.is_nan(bool_, None, x))?;
        let zero = self.f32c(0.0);
        ok(self.b.select(f32_, None, nan, zero, x))
    }

    /// `rdna::format::f32_to_small_float` in SPIR-V.
    fn single_to_small(&mut self, single: Word, mantissa: u32) -> Result<Word, EmitError> {
        let (u32_, bool_) = (self.t.u32_, self.t.bool_);
        let c = |e: &mut Self, v: u32| e.u32c(v);
        let twenty_three = c(self, 23);
        let exponent = ok(self.b.shift_right_logical(u32_, None, single, twenty_three))?;
        let byte = c(self, 0xFF);
        let exponent = ok(self.b.bitwise_and(u32_, None, exponent, byte))?;
        let fraction_mask = c(self, 0x7F_FFFF);
        let fraction = ok(self.b.bitwise_and(u32_, None, single, fraction_mask))?;
        let thirty_one = c(self, 31);
        let sign = ok(self.b.shift_right_logical(u32_, None, single, thirty_one))?;
        let zero = c(self, 0);
        let one = c(self, 1);
        let negative = ok(self.b.i_not_equal(bool_, None, sign, zero))?;
        let top = c(self, 0x1F << mantissa);
        let nan = c(self, (0x1F << mantissa) | (1 << (mantissa - 1)));
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
            let step = ok(e.b.select(u32_, None, up, one, none))?;
            ok(e.b.i_add(u32_, None, quotient, step))
        };
        let shift = 23 - mantissa;
        let bias = c(self, 112);
        let biased = ok(self.b.i_sub(u32_, None, exponent, bias))?;
        let packed = ok(self.b.shift_left_logical(u32_, None, biased, twenty_three))?;
        let packed = ok(self.b.bitwise_or(u32_, None, packed, fraction))?;
        let shift_word = c(self, shift);
        let normal = round(self, packed, shift_word)?;
        let implicit = c(self, 0x80_0000);
        let full = ok(self.b.bitwise_or(u32_, None, fraction, implicit))?;
        let tiny_base = c(self, shift + 1);
        let tiny_shift = ok(self.b.i_sub(u32_, None, tiny_base, biased))?;
        let tiny_shift = self.masked_shift(tiny_shift, 31)?;
        let tiny = round(self, full, tiny_shift)?;
        let positive = ok(self.b.s_greater_than_equal(bool_, None, biased, one))?;
        let limit = c(self, (-(mantissa as i32)) as u32);
        let vanished = ok(self.b.s_less_than(bool_, None, biased, limit))?;
        let overflow = ok(self.b.s_greater_than_equal(bool_, None, biased, thirty_one))?;
        let value = ok(self.b.select(u32_, None, positive, normal, tiny))?;
        let value = ok(self.b.select(u32_, None, vanished, zero, value))?;
        let value = ok(self.b.select(u32_, None, overflow, top, value))?;
        let value = ok(self.b.select(u32_, None, negative, zero, value))?;
        let empty = ok(self.b.i_equal(bool_, None, fraction, zero))?;
        let infinity = ok(self.b.select(u32_, None, negative, zero, top))?;
        let special = ok(self.b.select(u32_, None, empty, infinity, nan))?;
        let all_ones = ok(self.b.i_equal(bool_, None, exponent, byte))?;
        ok(self.b.select(u32_, None, all_ones, special, value))
    }

    pub(super) fn typed_store(
        &mut self,
        format: Word,
        address: Word,
        in_range: Word,
        values: [Word; 4],
        d16: bool,
    ) -> Result<(), EmitError> {
        let (u32_, u64_, bool_) = (self.t.u32_, self.t.u64_, self.t.bool_);
        let active = self.active()?;
        let condition = ok(self.b.logical_and(bool_, None, active, in_range))?;
        let encode = self.encode_function(d16);
        let result = self.encoded_type();
        self.if_then(condition, |e| {
            let arguments = [format, values[0], values[1], values[2], values[3]];
            let encoded = ok(e.b.function_call(result, None, encode, arguments))?;
            let uvec4 = e.t.uvec4;
            let words = ok(e.b.composite_extract(uvec4, None, encoded, [0]))?;
            let bytes = ok(e.b.composite_extract(u32_, None, encoded, [1]))?;
            let low = ok(e.b.u_convert(u32_, None, address))?;
            let three = e.u32c(3);
            let misaligned = ok(e.b.bitwise_and(u32_, None, low, three))?;
            let shift = ok(e.b.shift_left_logical(u32_, None, misaligned, three))?;
            let shift = ok(e.b.u_convert(u64_, None, shift))?;
            let mut element = Vec::new();
            let mut mask = Vec::new();
            for k in 0..4u32 {
                element.push(ok(e.b.composite_extract(u32_, None, words, [k]))?);
                let bits = e.u32c(32 * k);
                let eight = e.u32c(8);
                let total = ok(e.b.i_mul(u32_, None, bytes, eight))?;
                let covered = ok(e.b.u_greater_than(bool_, None, total, bits))?;
                let left = ok(e.b.i_sub(u32_, None, total, bits))?;
                let thirty_two = e.u32c(32);
                let left = e.glsl(u32_, GLOp::UMin, &[left, thirty_two])?;
                let left = ok(e.b.u_convert(u64_, None, left))?;
                let one = e.u64c(1);
                let ones = ok(e.b.shift_left_logical(u64_, None, one, left))?;
                let ones = ok(e.b.i_sub(u64_, None, ones, one))?;
                let ones = ok(e.b.u_convert(u32_, None, ones))?;
                let zero = e.u32c(0);
                mask.push(ok(e.b.select(u32_, None, covered, ones, zero))?);
            }
            let aligned_mask = e.u64c(!3);
            let aligned = ok(e.b.bitwise_and(u64_, None, address, aligned_mask))?;
            for k in 0..5usize {
                let zero = e.u32c(0);
                let current = |list: &[Word], i: usize| if i < 4 { list[i] } else { zero };
                let previous = |list: &[Word], i: usize| if i > 0 { list[i - 1] } else { zero };
                let mut placed = [0; 2];
                for (slot, list) in placed.iter_mut().zip([&element, &mask]) {
                    let pair = e.pack(previous(list, k), current(list, k))?;
                    let moved = ok(e.b.shift_left_logical(u64_, None, pair, shift))?;
                    let thirty_two = e.u64c(32);
                    let moved = ok(e.b.shift_right_logical(u64_, None, moved, thirty_two))?;
                    *slot = ok(e.b.u_convert(u32_, None, moved))?;
                }
                let [value, bits] = placed;
                let step = e.u64c(4 * k as u64);
                let at = ok(e.b.i_add(u64_, None, aligned, step))?;
                e.masked_word_write(at, value, bits)?;
            }
            Ok(())
        })
    }

    /// Writes the `mask` bits of `value` into the word at byte address `at`: whole words are stored, partial
    /// ones are merged atomically so neighbouring lanes' bytes survive.
    fn masked_word_write(&mut self, at: Word, value: Word, mask: Word) -> Result<(), EmitError> {
        let (u32_, bool_) = (self.t.u32_, self.t.bool_);
        let zero = self.u32c(0);
        let full = self.u32c(u32::MAX);
        let some = ok(self.b.i_not_equal(bool_, None, mask, zero))?;
        self.if_then(some, |e| {
            let ptr = if e.heap.is_some() {
                e.heap_pointer(at)?
            } else {
                let index = e.memory_index(at)?;
                let memory = e.memory;
                e.element(memory, index)?
            };
            let whole = ok(e.b.i_equal(bool_, None, mask, full))?;
            let (store, merge_label, done) = (e.b.id(), e.b.id(), e.b.id());
            ok(e.b.selection_merge(done, SelectionControl::NONE))?;
            ok(e.b.branch_conditional(whole, store, merge_label, []))?;
            e.begin(store)?;
            if e.heap.is_some() {
                ok(e.b.store(
                    ptr,
                    value,
                    Some(rspirv::spirv::MemoryAccess::ALIGNED),
                    [Operand::LiteralBit32(4)],
                ))?;
            } else {
                ok(e.b.store(ptr, value, None, []))?;
            }
            ok(e.b.branch(done))?;
            e.begin(merge_label)?;
            let scope = e.u32c(1);
            let relaxed = e.u32c(0);
            let keep = ok(e.b.not(u32_, None, mask))?;
            ok(e.b.atomic_and(u32_, None, ptr, scope, relaxed, keep))?;
            let bits = ok(e.b.bitwise_and(u32_, None, value, mask))?;
            ok(e.b.atomic_or(u32_, None, ptr, scope, relaxed, bits))?;
            ok(e.b.branch(done))?;
            e.begin(done)?;
            Ok(())
        })
    }
}
