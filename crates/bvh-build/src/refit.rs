use crate::cpu::{union, write_box};
use crate::input::{BlasInfo, Geometry, Instance};
use crate::layout::{BOX16, BOX32, EMPTY_BOX, ElementKind, Header, INVALID, NodeKind};
use crate::structure::{ElementSet, instance_elements, triangle_elements};
use crate::{BuildError, INSTANCE, Result, TRI0};

fn word(bytes: &[u8], at: usize) -> Result<u32> {
    bytes
        .get(at..at + 4)
        .map(|b| u32::from_le_bytes(b.try_into().expect("4 bytes")))
        .ok_or(BuildError::Malformed("a pointer leaves the structure"))
}

fn set_word(bytes: &mut [u8], at: usize, value: u32) -> Result<()> {
    bytes
        .get_mut(at..at + 4)
        .ok_or(BuildError::Malformed("a pointer leaves the structure"))?
        .copy_from_slice(&value.to_le_bytes());
    Ok(())
}

struct Walk<'a> {
    bytes: &'a mut [u8],
    elements: usize,
    stride: usize,
    boxes: &'a [[f32; 6]],
}

impl Walk<'_> {
    fn visit(&mut self, pointer: u32, level: u32) -> Result<[f32; 6]> {
        if level > 128 {
            return Err(BuildError::Malformed("a tree deeper than any build makes"));
        }
        let unit = (pointer >> 3) as usize;
        match pointer & 7 {
            TRI0 | INSTANCE => {
                let at = (unit * 64)
                    .checked_sub(self.elements)
                    .ok_or(BuildError::Malformed(
                        "an element before the element region",
                    ))?;
                self.boxes
                    .get(at / self.stride)
                    .copied()
                    .ok_or(BuildError::Malformed("an element past the element count"))
            }
            kind @ (BOX16 | BOX32) => {
                let node_kind = if kind == BOX16 {
                    NodeKind::Box16
                } else {
                    NodeKind::Box32
                };
                let at = unit * 64;
                let mut record = vec![0u32; node_kind.words()];
                let mut total = EMPTY_BOX;
                for slot in 0..4 {
                    let child = word(self.bytes, at + 4 * slot)?;
                    record[slot] = child;
                    let b = if child == INVALID {
                        EMPTY_BOX
                    } else {
                        self.visit(child, level + 1)?
                    };
                    write_box(&mut record, node_kind, slot, &b);
                    total = union(&total, &b);
                }
                for (i, value) in record.iter().enumerate().skip(4) {
                    set_word(self.bytes, at + 4 * i, *value)?;
                }
                Ok(total)
            }
            _ => Err(BuildError::Malformed("a node of a type no build writes")),
        }
    }
}

fn refit(structure: &mut [u8], set: &ElementSet) -> Result<()> {
    let mut header = Header::read(structure)?;
    if header.is_bottom_level() != (set.kind == ElementKind::Triangle) {
        return Err(BuildError::Malformed("elements of the wrong level"));
    }
    if header.elements_count as usize != set.len() {
        return Err(BuildError::Malformed("an update changes the element count"));
    }
    let elements = header.elements as usize;
    let stride = 64 * set.kind.units();
    let words = set.kind.words();
    for i in 0..set.len() {
        let mut record = set.records[i * words..(i + 1) * words].to_vec();
        if set.kind == ElementKind::Instance {
            record[29] = word(structure, elements + stride * i + 4 * 29)?;
        }
        for (k, value) in record.iter().enumerate() {
            set_word(structure, elements + stride * i + 4 * k, *value)?;
        }
    }
    if set.len() >= 2 {
        let mut walk = Walk {
            bytes: structure,
            elements,
            stride,
            boxes: &set.boxes,
        };
        walk.visit(header.root, 0)?;
    }
    header.bounds = set.boxes.iter().fold(EMPTY_BOX, |a, b| union(&a, b));
    header.write(&mut structure[..64]);
    Ok(())
}

/// Rewrites a bottom-level structure's triangles and recomputes every node's boxes, keeping the tree: the
/// update of a build made with the same geometry counts.
pub fn refit_blas(structure: &mut [u8], geometries: &[Geometry]) -> Result<()> {
    refit(structure, &triangle_elements(geometries))
}

/// The same for a top-level structure and its instances.
pub fn refit_tlas(
    structure: &mut [u8],
    instances: &[Instance],
    lookup: &dyn Fn(u64) -> Option<BlasInfo>,
) -> Result<()> {
    refit(structure, &instance_elements(instances, lookup))
}
