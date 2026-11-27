use crate::{BuildError, INSTANCE, Result, TRI0};

pub const HEADER_BYTES: usize = 64;
pub const INVALID: u32 = u32::MAX;
pub const BOX16: u32 = 4;
pub const BOX32: u32 = 5;
pub const BLAS_FLAGS: u32 = 0x8000_0006;
pub const TLAS_FLAGS: u32 = 0x0000_0006;
pub const EMPTY_BOX: [f32; 6] = [
    f32::INFINITY,
    f32::INFINITY,
    f32::INFINITY,
    f32::NEG_INFINITY,
    f32::NEG_INFINITY,
    f32::NEG_INFINITY,
];

/// What a structure's leaves are: triangle records of one 64-byte unit or instance records of two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementKind {
    Triangle,
    Instance,
}

impl ElementKind {
    pub fn units(self) -> usize {
        match self {
            ElementKind::Triangle => 1,
            ElementKind::Instance => 2,
        }
    }

    pub fn words(self) -> usize {
        16 * self.units()
    }

    pub fn pointer_type(self) -> u32 {
        match self {
            ElementKind::Triangle => TRI0,
            ElementKind::Instance => INSTANCE,
        }
    }
}

/// The box node a structure's inner nodes use: 16-bit boxes in 64 bytes or 32-bit boxes in 128.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Box16,
    Box32,
}

impl NodeKind {
    pub fn units(self) -> usize {
        match self {
            NodeKind::Box16 => 1,
            NodeKind::Box32 => 2,
        }
    }

    pub fn words(self) -> usize {
        16 * self.units()
    }

    pub fn pointer_type(self) -> u32 {
        match self {
            NodeKind::Box16 => BOX16,
            NodeKind::Box32 => BOX32,
        }
    }
}

/// Where each region of a structure sits, as byte offsets from its header: the header, the elements from
/// offset 128, the box nodes, then the parent table. A BOX16 build ends its nodes with one zeroed unit,
/// because the traversal instruction loads 112 bytes of a box node before it looks at the node's type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub elements: usize,
    pub nodes: usize,
    pub parents: usize,
    pub total: usize,
    /// Table entries before the first node unit.
    pub node_table_base: usize,
}

fn round_up(value: usize, to: usize) -> usize {
    value.div_ceil(to) * to
}

pub fn layout(
    count: usize,
    element: ElementKind,
    node: NodeKind,
    node_count: usize,
) -> Result<Layout> {
    let elements = 2 * HEADER_BYTES;
    let nodes = elements + round_up(count * element.units() * 64, 128);
    let spare = if node == NodeKind::Box16 && node_count > 0 {
        64
    } else {
        0
    };
    let parents = round_up(nodes + node_count * node.units() * 64 + spare, 128);
    let node_table_base = (nodes - elements) / 64;
    let entries = node_table_base + node_count * node.units();
    let total = round_up(parents + 4 * entries, 128);
    if total >= 1 << 32 {
        return Err(BuildError::TooLarge);
    }
    Ok(Layout {
        elements,
        nodes,
        parents,
        total,
        node_table_base,
    })
}

/// The most box nodes a tree over `count` elements can have: a node has two children only when both are
/// elements, so at most (3n - 2) / 4 of them.
pub fn max_nodes(count: usize) -> usize {
    if count < 2 { 0 } else { (3 * count - 2) / 4 }
}

pub fn pointer(unit: usize, kind: u32) -> u32 {
    ((unit as u32) << 3) | kind
}

/// A structure's header, offsets in bytes from its start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Header {
    pub flags: u32,
    pub total_nodes: u32,
    pub elements_count: u32,
    pub bounds: [f32; 6],
    pub nodes: u32,
    pub root: u32,
    pub elements: u32,
    pub parents: u32,
}

impl Header {
    pub fn write(&self, bytes: &mut [u8]) {
        let words: [u32; 16] = [
            self.flags,
            self.total_nodes,
            0,
            self.elements_count,
            self.bounds[0].to_bits(),
            self.bounds[1].to_bits(),
            self.bounds[2].to_bits(),
            self.bounds[3].to_bits(),
            self.bounds[4].to_bits(),
            self.bounds[5].to_bits(),
            self.nodes,
            0,
            self.root,
            self.elements,
            self.parents,
            0,
        ];
        for (i, word) in words.iter().enumerate() {
            bytes[4 * i..4 * i + 4].copy_from_slice(&word.to_le_bytes());
        }
    }

    pub fn read(bytes: &[u8]) -> Result<Header> {
        if bytes.len() < HEADER_BYTES {
            return Err(BuildError::Malformed("shorter than a header"));
        }
        let word =
            |i: usize| u32::from_le_bytes(bytes[4 * i..4 * i + 4].try_into().expect("4 bytes"));
        let header = Header {
            flags: word(0),
            total_nodes: word(1),
            elements_count: word(3),
            bounds: [4, 5, 6, 7, 8, 9].map(|i| f32::from_bits(word(i))),
            nodes: word(10),
            root: word(12),
            elements: word(13),
            parents: word(14),
        };
        if header.flags & 0xFFFF != 6 {
            return Err(BuildError::Malformed("not a version 6 structure"));
        }
        if word(11) != 0 {
            return Err(BuildError::Malformed("a primitive remap table"));
        }
        if header.size() > bytes.len() {
            return Err(BuildError::Malformed("shorter than its header says"));
        }
        Ok(header)
    }

    /// The structure's size: its 64-byte nodes and the header.
    pub fn size(&self) -> usize {
        64 * (self.total_nodes as usize + 1)
    }

    pub fn is_bottom_level(&self) -> bool {
        self.flags & 0x8000_0000 != 0
    }
}
