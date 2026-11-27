//! Instruction-level scanning of function bodies.

use std::collections::HashMap;

use iced_x86::{
    Decoder, DecoderOptions, FlowControl, Instruction, InstructionInfoFactory,
    InstructionInfoOptions, Mnemonic, OpAccess, OpKind,
};
use pe_image::Image;

const MAX_TEXT: usize = 512;
const MIN_TEXT: usize = 4;
const MAX_GUESSED_FUNCTION: u32 = 1 << 20;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Region {
    Code,
    Data,
    Writable,
}

pub(crate) struct Layout<'a, 'b> {
    pub image: &'b Image<'a>,
    pub base: u64,
    pub imports: HashMap<u32, String>,
    regions: Vec<(u32, u32, Region)>,
}

impl<'a, 'b> Layout<'a, 'b> {
    pub fn new(image: &'b Image<'a>) -> Self {
        let mut imports = HashMap::new();
        for dll in image.imports() {
            for entry in dll.entries {
                let name = match (entry.name, entry.ordinal) {
                    (Some(name), _) => name,
                    (None, Some(ordinal)) => format!("{}!#{ordinal}", dll.name),
                    _ => continue,
                };
                imports.insert(entry.iat_rva, name);
            }
        }
        let mut regions: Vec<(u32, u32, Region)> = image
            .sections
            .iter()
            .map(|s| {
                let region = if s.executable() {
                    Region::Code
                } else if s.writable() {
                    Region::Writable
                } else {
                    Region::Data
                };
                (
                    s.virtual_address,
                    s.virtual_address + s.virtual_size.max(s.raw_size),
                    region,
                )
            })
            .collect();
        regions.sort_by_key(|r| r.0);
        Layout {
            image,
            base: image.image_base,
            imports,
            regions,
        }
    }

    fn region(&self, rva: u32) -> Option<Region> {
        let index = self.regions.partition_point(|r| r.0 <= rva);
        let (_, end, region) = *self.regions[..index].last()?;
        (rva < end).then_some(region)
    }

    pub fn rva(&self, va: u64) -> Option<u32> {
        u32::try_from(va.checked_sub(self.base)?).ok()
    }

    pub fn is_code(&self, rva: u32) -> bool {
        self.region(rva) == Some(Region::Code)
    }

    /// Code with bytes stored in the file.
    pub fn code_at(&self, rva: u32) -> Option<&'a [u8]> {
        self.is_code(rva)
            .then(|| self.image.bytes_at(rva))
            .flatten()
    }
}

/// Printable ASCII or UTF-16 text starting at `rva` in a data section.
pub(crate) fn text_at(layout: &Layout<'_, '_>, rva: u32) -> Option<String> {
    if layout.region(rva)? == Region::Code {
        return None;
    }
    let bytes = layout.image.bytes_at(rva)?;
    let printable = |b: u8| (0x20..0x7F).contains(&b) || matches!(b, b'\t' | b'\n' | b'\r');
    if bytes.len() >= 2 && printable(bytes[0]) && bytes[1] == 0 {
        let mut text = String::new();
        for unit in bytes.chunks_exact(2).take(MAX_TEXT) {
            if unit == [0, 0] {
                return (text.len() >= MIN_TEXT).then_some(text);
            }
            if unit[1] != 0 || !printable(unit[0]) {
                return None;
            }
            text.push(char::from(unit[0]));
        }
        return None;
    }
    let end = bytes.iter().take(MAX_TEXT).position(|b| *b == 0)?;
    (end >= MIN_TEXT && bytes[..end].iter().all(|b| printable(*b)))
        .then(|| String::from_utf8_lossy(&bytes[..end]).into_owned())
}

/// What one function's code refers to; all lists are sorted and unique.
#[derive(Default, Debug)]
pub(crate) struct Scan {
    pub calls: Vec<u32>,
    pub jumps: Vec<u32>,
    pub imports: Vec<u32>,
    pub strings: Vec<u32>,
    pub reads: Vec<u32>,
    pub writes: Vec<u32>,
    pub taken: Vec<u32>,
    pub thunk: Option<u32>,
}

impl Scan {
    fn finish(&mut self) {
        for list in [
            &mut self.calls,
            &mut self.jumps,
            &mut self.imports,
            &mut self.strings,
            &mut self.reads,
            &mut self.writes,
            &mut self.taken,
        ] {
            list.sort_unstable();
            list.dedup();
        }
    }
}

/// Scans every piece of one function. `chunks` are all of its `(begin, end)` ranges.
pub(crate) fn scan_function(layout: &Layout<'_, '_>, chunks: &[(u32, u32)]) -> Scan {
    let mut scan = Scan::default();
    let mut factory = InstructionInfoFactory::new();
    let mut instruction = Instruction::default();
    for (index, &(begin, end)) in chunks.iter().enumerate() {
        let Some(bytes) = layout.image.bytes_at(begin) else {
            continue;
        };
        let bytes = &bytes[..bytes.len().min((end - begin) as usize)];
        let mut decoder = Decoder::with_ip(
            64,
            bytes,
            layout.base + u64::from(begin),
            DecoderOptions::NONE,
        );
        let mut first = true;
        while decoder.can_decode() {
            decoder.decode_out(&mut instruction);
            if instruction.is_invalid() {
                break;
            }
            let flow = instruction.flow_control();
            if let Some(target) = direct_target(&instruction)
                && let Some(target) = layout.rva(target)
            {
                match flow {
                    FlowControl::Call => scan.calls.push(target),
                    FlowControl::UnconditionalBranch
                        if !chunks.iter().any(|&(b, e)| target >= b && target < e) =>
                    {
                        scan.jumps.push(target);
                    }
                    _ => {}
                }
            }
            if instruction.is_ip_rel_memory_operand()
                && let Some(slot) = layout.rva(instruction.ip_rel_memory_address())
            {
                note_memory(layout, &mut scan, &mut factory, &instruction, slot);
                if first
                    && index == 0
                    && flow == FlowControl::IndirectBranch
                    && layout.imports.contains_key(&slot)
                {
                    scan.thunk = Some(slot);
                }
            }
            first = false;
        }
    }
    scan.finish();
    scan
}

fn direct_target(instruction: &Instruction) -> Option<u64> {
    (instruction.op0_kind() == OpKind::NearBranch64).then(|| instruction.near_branch_target())
}

fn note_memory(
    layout: &Layout<'_, '_>,
    scan: &mut Scan,
    factory: &mut InstructionInfoFactory,
    instruction: &Instruction,
    target: u32,
) {
    if layout.imports.contains_key(&target) {
        scan.imports.push(target);
        return;
    }
    let mnemonic = instruction.mnemonic();
    let Some(region) = layout.region(target) else {
        return;
    };
    if region == Region::Code {
        return;
    }
    if matches!(mnemonic, Mnemonic::Lea | Mnemonic::Mov) && text_at(layout, target).is_some() {
        scan.strings.push(target);
        return;
    }
    if region != Region::Writable {
        return;
    }
    if mnemonic == Mnemonic::Lea {
        scan.taken.push(target);
        return;
    }
    let info = factory.info_options(instruction, InstructionInfoOptions::NO_REGISTER_USAGE);
    let (mut read, mut write) = (false, false);
    for memory in info.used_memory() {
        match memory.access() {
            OpAccess::Read | OpAccess::CondRead => read = true,
            OpAccess::Write | OpAccess::CondWrite => write = true,
            OpAccess::ReadWrite | OpAccess::ReadCondWrite => {
                read = true;
                write = true;
            }
            OpAccess::None | OpAccess::NoMemAccess => {}
        }
    }
    if read {
        scan.reads.push(target);
    }
    if write {
        scan.writes.push(target);
    }
}

/// How long the code at `start` plausibly is, never reaching `limit`: it ends at the first
/// return, unconditional jump, `int3` or `ud2` that no forward branch jumps past.
pub(crate) fn guess_extent(layout: &Layout<'_, '_>, start: u32, limit: u32) -> Option<u32> {
    let bytes = layout.code_at(start)?;
    let room = limit.saturating_sub(start).min(MAX_GUESSED_FUNCTION) as usize;
    let bytes = &bytes[..bytes.len().min(room)];
    let mut decoder = Decoder::with_ip(
        64,
        bytes,
        layout.base + u64::from(start),
        DecoderOptions::NONE,
    );
    let mut instruction = Instruction::default();
    let mut farthest = start;
    let mut end = start;
    while decoder.can_decode() {
        decoder.decode_out(&mut instruction);
        if instruction.is_invalid() {
            break;
        }
        let next = (instruction.next_ip() - layout.base) as u32;
        let flow = instruction.flow_control();
        if let Some(target) = direct_target(&instruction).and_then(|t| layout.rva(t))
            && matches!(
                flow,
                FlowControl::ConditionalBranch | FlowControl::UnconditionalBranch
            )
            && target > start
            && target < limit
        {
            farthest = farthest.max(target);
        }
        end = next;
        let stops = matches!(
            flow,
            FlowControl::Return | FlowControl::UnconditionalBranch | FlowControl::IndirectBranch
        ) || instruction.mnemonic() == Mnemonic::Int3
            || instruction.mnemonic() == Mnemonic::Ud2;
        if stops && next >= farthest {
            break;
        }
    }
    (end > start).then_some(end - start)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testimage::Builder;

    #[test]
    fn finds_text_in_both_encodings() {
        let mut builder = Builder::new();
        builder.rdata(0x20, b"hello world\0");
        builder.rdata(
            0x40,
            &"wide text"
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .chain([0, 0])
                .collect::<Vec<_>>(),
        );
        builder.rdata(0x80, b"ab\0");
        builder.rdata(0x90, b"bin\x01\x02\x03\0");
        let bytes = builder.finish();
        let image = Image::parse(&bytes).unwrap();
        let layout = Layout::new(&image);
        let rdata = Builder::RDATA;
        assert_eq!(
            text_at(&layout, rdata + 0x20).as_deref(),
            Some("hello world")
        );
        assert_eq!(text_at(&layout, rdata + 0x40).as_deref(), Some("wide text"));
        assert_eq!(text_at(&layout, rdata + 0x80), None);
        assert_eq!(text_at(&layout, rdata + 0x90), None);
        assert_eq!(text_at(&layout, Builder::TEXT), None);
    }
}
