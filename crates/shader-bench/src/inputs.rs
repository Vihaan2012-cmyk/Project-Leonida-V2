//! The synthetic inputs a benchmarked shader runs on. Memory is a heap whose every word is a small number and
//! whose odd words are zero, so any word a shader treats as an address, an offset or the low half of a 64-bit
//! pointer lands inside the heap.

use std::collections::{BTreeSet, HashMap};

use gpu::{Resource, Sampler, Texture, TextureFormat, TextureView};
use shader_census::Analysis;
use shader_translate::images::{self, NUMBERS, Number, VIEWS, View};
use shader_translate::{PUSH_CODE, PUSH_CODE_GEOMETRY, PUSH_CODE_PIXEL, PUSH_HEAP, PUSH_WORDS};

pub const HEAP_WORDS: usize = 4 << 20;
const FILLED_WORDS: usize = 1 << 18;
const FILL_LIMIT: u32 = 1 << 18;
pub const CODE_ADDRESS: u64 = 1 << 20;
const BUFFER_SPACE: u32 = 256 << 10;

/// A small deterministic generator.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 32) as u32
    }
}

/// The heap before the shader runs: the filled low part, then the program's code at `CODE_ADDRESS`.
pub fn heap(code: &[u8]) -> Vec<u32> {
    let mut words = vec![0u32; HEAP_WORDS];
    let mut rng = Rng::new(12345);
    for pair in words[..FILLED_WORDS].chunks_exact_mut(2) {
        pair[0] = (rng.next() % FILL_LIMIT) & !3;
    }
    let at = CODE_ADDRESS as usize / 4;
    for (word, bytes) in words[at..].iter_mut().zip(code.chunks(4)) {
        let mut padded = [0u8; 4];
        padded[..bytes.len()].copy_from_slice(bytes);
        *word = u32::from_le_bytes(padded);
    }
    words
}

fn bits(mask: u128, reg: usize) -> bool {
    reg < 128 && mask >> reg & 1 != 0
}

/// A V# for 16-byte elements of four floats.
fn buffer_descriptor(base: u32) -> [u32; 4] {
    let format = (1..=77)
        .find(|code| {
            rdna::format::buffer_format(*code).is_some_and(|layout| {
                layout.count == 4
                    && layout.widths == [32; 4]
                    && layout.number == rdna::format::Number::Float
            })
        })
        .unwrap_or(0);
    [
        base,
        16 << 16,
        BUFFER_SPACE / 16,
        4 | 5 << 3 | 6 << 6 | 7 << 9 | format << 12,
    ]
}

/// The push constants: user SGPRs set from how the program uses them, the heap base and the code bases.
pub fn push(analysis: &Analysis, user_sgprs: u32, heap_base: u64, plain: u32) -> Vec<u32> {
    let mut words = vec![0u32; PUSH_WORDS as usize];
    let count = user_sgprs.min(32) as usize;
    let mut pointer = 0u32;
    let mut reg = 0;
    while reg < count {
        if bits(analysis.load_ptr64, reg) {
            words[reg] = (pointer * 4096) % FILL_LIMIT;
            pointer += 1;
            reg += 2;
        } else if bits(analysis.vsharp, reg) && reg + 4 <= count {
            let base = (pointer * 4096 + 2048) % FILL_LIMIT;
            pointer += 1;
            words[reg..reg + 4].copy_from_slice(&buffer_descriptor(base));
            reg += 4;
        } else if bits(analysis.tsharp, reg) {
            reg += 8;
        } else if bits(analysis.ssharp, reg) {
            reg += 4;
        } else if bits(analysis.read_first, reg) && !bits(analysis.offset, reg) {
            words[reg] = plain;
            reg += 1;
        } else {
            reg += 1;
        }
    }
    words[PUSH_HEAP as usize] = heap_base as u32;
    words[PUSH_HEAP as usize + 1] = (heap_base >> 32) as u32;
    for at in [PUSH_CODE, PUSH_CODE_GEOMETRY, PUSH_CODE_PIXEL] {
        words[at as usize] = CODE_ADDRESS as u32;
        words[at as usize + 1] = 0;
    }
    words
}

/// The set 1 bindings a program declares.
pub fn bindings(spirv: &[u32]) -> BTreeSet<u32> {
    use rspirv::dr::Operand;
    use rspirv::spirv::{Decoration, Op};
    let Ok(module) = rspirv::dr::load_words(spirv) else {
        return BTreeSet::new();
    };
    let mut sets = HashMap::new();
    let mut slots = HashMap::new();
    for inst in &module.annotations {
        if inst.class.opcode != Op::Decorate {
            continue;
        }
        let (
            Some(Operand::IdRef(target)),
            Some(Operand::Decoration(kind)),
            Some(Operand::LiteralBit32(value)),
        ) = (
            inst.operands.first(),
            inst.operands.get(1),
            inst.operands.get(2),
        )
        else {
            continue;
        };
        match kind {
            Decoration::DescriptorSet => {
                sets.insert(*target, *value);
            }
            Decoration::Binding => {
                slots.insert(*target, *value);
            }
            _ => {}
        }
    }
    slots
        .into_iter()
        .filter(|(target, _)| sets.get(target) == Some(&images::SET))
        .map(|(_, binding)| binding)
        .collect()
}

fn texture(rng: &mut Rng, view: View, format: TextureFormat, size: u32) -> Texture {
    let (kind, depth) = match view {
        View::D2 => (TextureView::D2, 1),
        View::D2Array => (TextureView::D2Array, 4),
        View::D3 => (TextureView::D3, 16),
    };
    let mut texture = Texture {
        view: kind,
        format,
        width: size,
        height: size,
        depth,
        mips: 1,
        data: Vec::new(),
    };
    let length = texture.mip_bytes(0);
    texture.data = (0..length).map(|_| (rng.next() >> 8) as u8).collect();
    texture
}

/// One resource per binding the program declares, each holding the single texture or sampler its slot 0 names.
pub fn resources(spirv: &[u32]) -> Vec<(u32, Resource)> {
    let mut rng = Rng::new(777);
    let mut list = Vec::new();
    let empty = images::table(&[]).expect("an empty table fits");
    for binding in bindings(spirv) {
        let mut resource = match binding {
            images::TEXTURE_TABLE | images::SAMPLER_TABLE => Some(Resource::Buffer(empty.clone())),
            images::SAMPLERS => Some(Resource::Samplers(vec![Sampler {
                linear: true,
                repeat: true,
                compare: None,
            }])),
            images::COMPARE_SAMPLERS => Some(Resource::Samplers(vec![Sampler {
                linear: true,
                repeat: true,
                compare: Some(3),
            }])),
            _ => None,
        };
        for view in VIEWS {
            for number in NUMBERS {
                let sampled = images::binding(view, number) == binding;
                let storage = images::storage_binding(view, number) == binding;
                if !sampled && !storage {
                    continue;
                }
                let (format, size) = match number {
                    Number::Float => (TextureFormat::Rgba8Unorm, 512),
                    Number::Uint => (TextureFormat::Rgba32Uint, 128),
                    Number::Sint => (TextureFormat::Rgba32Sint, 128),
                };
                let item = texture(&mut rng, view, format, size);
                resource = Some(if storage {
                    Resource::StorageTextures(vec![item])
                } else {
                    Resource::Textures(vec![item])
                });
            }
            for wide in [false, true] {
                if images::atomic_binding(view, wide) == binding {
                    let format = if wide {
                        TextureFormat::R64Uint
                    } else {
                        TextureFormat::R32Uint
                    };
                    resource = Some(Resource::StorageTextures(vec![texture(
                        &mut rng, view, format, 128,
                    )]));
                }
            }
        }
        if let Some(resource) = resource {
            list.push((binding, resource));
        }
    }
    list
}
