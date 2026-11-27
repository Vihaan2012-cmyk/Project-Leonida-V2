use super::ModelError;
use super::pages::Pages;
use super::types::{Bone, Skeleton};

const MAX_BONES: u16 = 4096;
const BONE_STRIDE: usize = 0x50;

/// Skeleton header: +0x28 bone records, +0x30 one 64-byte inverse bind matrix per bone, +0x40 a u16
/// parent per bone (0xFFFF for a root), +0x6C and +0x6E the bone count. A bone record holds the
/// default rotation at +0x00, translation at +0x10, name hash at +0x1C, scale at +0x20, name pointer
/// at +0x30, parent at +0x46 and its own index at +0x48.
pub(crate) fn read_skeleton(p: &Pages<'_>, header: usize) -> Result<Skeleton, ModelError> {
    p.slice(header, 0x70, "skeleton header")?;
    let count = p.u16(header + 0x6C, "bone count")?;
    if count == 0 || count > MAX_BONES || count != p.u16(header + 0x6E, "bone count")? {
        return Err(ModelError::Invalid(format!("skeleton bone count {count}")));
    }
    let n = count as usize;
    let records = p.pointer(header + 0x28, n * BONE_STRIDE, "bone records")?;
    let inverse = p.pointer(header + 0x30, n * 64, "inverse bind matrices")?;
    let parents = p.pointer(header + 0x40, n * 2, "bone parents")?;
    let mut bones = Vec::with_capacity(n);
    for i in 0..n {
        let r = records + i * BONE_STRIDE;
        let parent = p.u16(parents + 2 * i, "bone parent")?;
        if p.u16(r + 0x48, "bone index")? as usize != i || p.u16(r + 0x46, "bone parent")? != parent
        {
            return Err(ModelError::Invalid(format!(
                "bone record {i} disagrees with the parent table"
            )));
        }
        if parent != 0xFFFF && (parent as usize >= n || parent as usize == i) {
            return Err(ModelError::Invalid(format!("bone {i} has parent {parent}")));
        }
        let mut matrix: [f32; 16] = p.f32s(inverse + 64 * i, "inverse bind matrix")?;
        if !matrix.iter().all(|v| v.is_finite())
            || matrix[3] != 0.0
            || matrix[7] != 0.0
            || matrix[11] != 0.0
        {
            return Err(ModelError::Invalid(format!(
                "inverse bind matrix {i} is not an affine finite matrix"
            )));
        }
        // The stored w entry is not 1 in the game data; the matrix is affine.
        matrix[15] = 1.0;
        bones.push(Bone {
            name: p.c_string(r + 0x30),
            hash: p.u32(r + 0x1C, "bone hash")?,
            parent: (parent != 0xFFFF).then_some(parent),
            rotation: p.f32s(r, "bone rotation")?,
            translation: p.f32s(r + 0x10, "bone translation")?,
            scale: p.f32s(r + 0x20, "bone scale")?,
            inverse_bind: matrix,
        });
    }
    reject_cycles(&bones)?;
    Ok(Skeleton { bones })
}

fn reject_cycles(bones: &[Bone]) -> Result<(), ModelError> {
    for start in 0..bones.len() {
        let mut at = start;
        for _ in 0..=bones.len() {
            match bones[at].parent {
                Some(parent) => at = parent as usize,
                None => break,
            }
        }
        if bones[at].parent.is_some() {
            return Err(ModelError::Invalid(format!(
                "bone {start} is in a parent cycle"
            )));
        }
    }
    Ok(())
}

/// `joaat32` of a lowercased name, the hash the game stores for bones and shader parameters.
pub fn joaat(name: &str) -> u32 {
    let mut h: u32 = 0;
    for b in name.bytes() {
        h = h.wrapping_add(b.to_ascii_lowercase() as u32);
        h = h.wrapping_add(h << 10);
        h ^= h >> 6;
    }
    h = h.wrapping_add(h << 3);
    h ^= h >> 11;
    h.wrapping_add(h << 15)
}

impl Skeleton {
    /// Index of the bone with the given name hash; the first one when several share it.
    pub fn find_hash(&self, hash: u32) -> Option<usize> {
        self.bones.iter().position(|b| b.hash == hash)
    }
}
