//! Typed buffer loads: an element's words are read once per instruction and each channel is decoded in
//! place, for the one format the instruction names or, from a descriptor, through a table of layouts.

use rdna::format::{self, BufferFormat, Number};
use rspirv::dr::Builder;
use rspirv::spirv::{StorageClass, Word};
use shader_ir::Tmp;

use super::{EmitError, Emitter, GLOp, ok};

const CODES: u32 = 128;

/// A load's address and range temporaries and the block it was read in, with the words it read.
pub(super) type Element = ((Tmp, Tmp, Word), [Word; 4]);

/// A format code's layout in one word: six-bit channel widths from bit 0, the channel count at bit 24 (zero
/// for codes with no buffer layout) and the number type at bit 27.
fn layout_word(code: u32) -> u32 {
    let Some(layout) = format::buffer_format(code) else {
        return 0;
    };
    let widths = (0..4).fold(0, |word, k| word | u32::from(layout.widths[k]) << (6 * k));
    widths | (layout.count as u32) << 24 | number_index(layout.number) << 27
}

fn number_index(number: Number) -> u32 {
    match number {
        Number::Unorm => 0,
        Number::Snorm => 1,
        Number::Uscaled => 2,
        Number::Sscaled => 3,
        Number::Uint => 4,
        Number::Sint => 5,
        Number::Float => 6,
    }
}

/// The layout table, a private array; made before the entry function begins so it is a module variable.
pub(super) fn layout_table(b: &mut Builder, u32_: Word) -> Word {
    let length = b.constant_bit32(u32_, CODES);
    let array = b.type_array(u32_, length);
    let entries: Vec<Word> = (0..CODES)
        .map(|code| b.constant_bit32(u32_, layout_word(code)))
        .collect();
    let table = b.constant_composite(array, entries);
    let pointer = b.type_pointer(None, StorageClass::Private, array);
    b.variable(pointer, None, StorageClass::Private, Some(table))
}

impl Emitter {
    pub(super) fn typed_load(
        &mut self,
        format: Tmp,
        address: Tmp,
        in_range: Tmp,
        select: Option<Tmp>,
        component: u8,
    ) -> Result<Word, EmitError> {
        let (u32_, bool_) = (self.t.u32_, self.t.bool_);
        let selector = match select {
            Some(word) => {
                let shift = self.u32c(3 * u32::from(component));
                let shifted = ok(self
                    .b
                    .shift_right_logical(u32_, None, self.val(word), shift))?;
                self.masked_shift(shifted, 7)?
            }
            None => self.u32c(4 + u32::from(component)),
        };
        let active = self.active()?;
        let fetch = ok(self.b.logical_and(bool_, None, active, self.val(in_range)))?;
        let words = self.typed_words(address, in_range, fetch)?;
        let format = self.val(format);
        let value = match (self.known(format), self.known(selector)) {
            (Some(code), Some(selector)) => self.decode_fixed(code, selector, words, fetch)?,
            _ => self.decode_any(format, selector, words, fetch)?,
        };
        let zero = self.u32c(0);
        ok(self.b.select(u32_, None, active, value, zero))
    }

    /// The four words from an element's first byte, shared by the channels of one load instruction.
    fn typed_words(
        &mut self,
        address: Tmp,
        in_range: Tmp,
        fetch: Word,
    ) -> Result<[Word; 4], EmitError> {
        if let Some(((a, r, label), words)) = self.typed_cache[self.part]
            && (a, r, label) == (address, in_range, self.label)
        {
            return Ok(words);
        }
        let (u32_, u64_) = (self.t.u32_, self.t.u64_);
        let start = self.val(address);
        let words = if self.heap.is_some() {
            let low = ok(self.b.u_convert(u32_, None, start))?;
            let three = self.u32c(3);
            let misaligned = ok(self.b.bitwise_and(u32_, None, low, three))?;
            let shift = ok(self.b.shift_left_logical(u32_, None, misaligned, three))?;
            let mask = self.u64c(!3);
            let aligned = ok(self.b.bitwise_and(u64_, None, start, mask))?;
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
            self.element_words(start)?
        };
        self.typed_cache[self.part] = Some(((address, in_range, self.label), words));
        Ok(words)
    }

    fn known(&self, id: Word) -> Option<u32> {
        self.constant_values.get(&id).copied()
    }

    fn channel_field(
        &mut self,
        layout: &BufferFormat,
        channel: usize,
        words: [Word; 4],
    ) -> Result<Word, EmitError> {
        let u32_ = self.t.u32_;
        let offset = layout.offset(channel);
        let width = u32::from(layout.widths[channel]);
        let shift = self.u32c(offset % 32);
        let shifted =
            ok(self
                .b
                .shift_right_logical(u32_, None, words[(offset / 32) as usize], shift))?;
        if width == 32 {
            return Ok(shifted);
        }
        let mask = self.u32c(u32::MAX >> (32 - width));
        ok(self.b.bitwise_and(u32_, None, shifted, mask))
    }

    /// `format::select` for a format and selector the instruction fixes.
    fn decode_fixed(
        &mut self,
        code: u32,
        selector: u32,
        words: [Word; 4],
        fetch: Word,
    ) -> Result<Word, EmitError> {
        let zero = self.u32c(0);
        match (selector, format::buffer_format(code)) {
            (1, _) => Ok(self.u32c(format::one(code))),
            (4..=7, Some(layout)) => {
                let channel = (selector as usize - 4) % layout.count;
                let field = self.channel_field(&layout, channel, words)?;
                let value = self.convert_field(&layout, channel, field)?;
                let u32_ = self.t.u32_;
                ok(self.b.select(u32_, None, fetch, value, zero))
            }
            _ => Ok(zero),
        }
    }

    /// `format::select` for any format code, reading its layout from the table.
    fn decode_any(
        &mut self,
        format: Word,
        selector: Word,
        words: [Word; 4],
        fetch: Word,
    ) -> Result<Word, EmitError> {
        let (u32_, bool_, f32_) = (self.t.u32_, self.t.bool_, self.t.f32_);
        let c = |e: &mut Self, v: u32| e.u32c(v);
        let table = self
            .layout_table
            .expect("made for programs with typed loads");
        let index = self.masked_shift(format, CODES - 1)?;
        let pointer = self.t.ptr_private_u32;
        let entry = ok(self.b.access_chain(pointer, None, table, [index]))?;
        let entry = self.load(entry)?;
        let field_of = |e: &mut Self, value: Word, shift: u32, mask: u32| {
            let shift = e.u32c(shift);
            let shifted = ok(e.b.shift_right_logical(u32_, None, value, shift))?;
            e.masked_shift(shifted, mask)
        };
        let count = field_of(self, entry, 24, 7)?;
        let number = field_of(self, entry, 27, 7)?;
        let zero = c(self, 0);
        let valid = ok(self.b.i_not_equal(bool_, None, count, zero))?;
        let one = c(self, 1);
        let divisor = self.glsl(u32_, GLOp::UMax, &[count, one])?;
        let four = c(self, 4);
        let picked = ok(self.b.i_sub(u32_, None, selector, four))?;
        let channel = ok(self.b.u_mod(u32_, None, picked, divisor))?;

        let six = c(self, 6);
        let at = ok(self.b.i_mul(u32_, None, channel, six))?;
        let width = ok(self.b.shift_right_logical(u32_, None, entry, at))?;
        let width = self.masked_shift(width, 63)?;
        let mut offset = zero;
        for k in 0..3 {
            let earlier = field_of(self, entry, 6 * k, 63)?;
            let k = c(self, k);
            let before = ok(self.b.u_greater_than(bool_, None, channel, k))?;
            let earlier = ok(self.b.select(u32_, None, before, earlier, zero))?;
            offset = ok(self.b.i_add(u32_, None, offset, earlier))?;
        }
        let five = c(self, 5);
        let which = ok(self.b.shift_right_logical(u32_, None, offset, five))?;
        let mut data = words[0];
        for (k, word) in words.iter().enumerate().skip(1) {
            let k = c(self, k as u32);
            let hit = ok(self.b.i_equal(bool_, None, which, k))?;
            data = ok(self.b.select(u32_, None, hit, *word, data))?;
        }
        let bit = self.masked_shift(offset, 31)?;
        let data = ok(self.b.shift_right_logical(u32_, None, data, bit))?;
        let safe = self.glsl(u32_, GLOp::UMax, &[width, one])?;
        let thirty_two = c(self, 32);
        let spare = ok(self.b.i_sub(u32_, None, thirty_two, safe))?;
        let all = c(self, u32::MAX);
        let mask = ok(self.b.shift_right_logical(u32_, None, all, spare))?;
        let field = ok(self.b.bitwise_and(u32_, None, data, mask))?;
        let up = ok(self.b.shift_left_logical(u32_, None, field, spare))?;
        let signed = ok(self.b.shift_right_arithmetic(u32_, None, up, spare))?;

        let is = |e: &mut Self, n: u32| {
            let n = e.u32c(n);
            ok(e.b.i_equal(bool_, None, number, n))
        };
        let uscaled = ok(self.b.convert_u_to_f(f32_, None, field))?;
        let uscaled = self.bits(uscaled)?;
        let sscaled = ok(self.b.convert_s_to_f(f32_, None, signed))?;
        let sscaled = self.bits(sscaled)?;

        let f64_ = self.doubles();
        let snorm = is(self, 1)?;
        let unsigned = ok(self.b.convert_u_to_f(f64_, None, field))?;
        let negative = ok(self.b.convert_s_to_f(f64_, None, signed))?;
        let numerator = ok(self.b.select(f64_, None, snorm, negative, unsigned))?;
        let half_mask = ok(self.b.shift_right_logical(u32_, None, mask, one))?;
        let largest = ok(self.b.select(u32_, None, snorm, half_mask, mask))?;
        let denominator = ok(self.b.convert_u_to_f(f64_, None, largest))?;
        let quotient = ok(self.b.f_div(f64_, None, numerator, denominator))?;
        let quotient = self.exact(quotient);
        let floor = self.b.constant_bit64(f64_, (-1.0f64).to_bits());
        let quotient = self.glsl(f64_, GLOp::NMax, &[quotient, floor])?;
        let normalized = ok(self.b.f_convert(f32_, None, quotient))?;
        let normalized = self.bits(normalized)?;

        let fifteen = c(self, 15);
        let lift = ok(self.b.i_sub(u32_, None, fifteen, safe))?;
        let lift = self.masked_shift(lift, 31)?;
        let small = ok(self.b.shift_left_logical(u32_, None, field, lift))?;
        let sixteen = c(self, 16);
        let is_half = ok(self.b.i_equal(bool_, None, width, sixteen))?;
        let half = ok(self.b.select(u32_, None, is_half, field, small))?;
        let single = self.half_to_single(half, false)?;
        let is_word = ok(self.b.i_equal(bool_, None, width, thirty_two))?;
        let float = ok(self.b.select(u32_, None, is_word, field, single))?;

        let mut value = field;
        for (n, candidate) in [(5, signed), (2, uscaled), (3, sscaled), (6, float)] {
            let hit = is(self, n)?;
            value = ok(self.b.select(u32_, None, hit, candidate, value))?;
        }
        let norm = ok(self.b.u_less_than_equal(bool_, None, number, one))?;
        let value = ok(self.b.select(u32_, None, norm, normalized, value))?;
        let value = ok(self.b.select(u32_, None, valid, value, zero))?;

        let unsigned_int = is(self, 4)?;
        let signed_int = is(self, 5)?;
        let integer = ok(self.b.logical_or(bool_, None, unsigned_int, signed_int))?;
        let unit = ok(self.b.logical_not(bool_, None, integer))?;
        let unit = ok(self.b.logical_and(bool_, None, valid, unit))?;
        let float_one = c(self, 0x3F80_0000);
        let constant = ok(self.b.select(u32_, None, unit, float_one, one))?;

        let value = ok(self.b.select(u32_, None, fetch, value, zero))?;
        let data = ok(self.b.u_greater_than_equal(bool_, None, selector, four))?;
        let value = ok(self.b.select(u32_, None, data, value, zero))?;
        let is_one = ok(self.b.i_equal(bool_, None, selector, one))?;
        ok(self.b.select(u32_, None, is_one, constant, value))
    }
}
