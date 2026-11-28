//! What a run wrote, and whether two runs wrote the same.

use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub struct Output {
    /// Heap words the shader changed, by index.
    pub heap: Vec<(u32, u32)>,
    /// Bytes of each written texture and render target.
    pub images: Vec<Vec<u8>>,
}

pub fn heap_diff(before: &[u32], after: &[u32]) -> Vec<(u32, u32)> {
    before
        .iter()
        .zip(after)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(i, (_, b))| (i as u32, *b))
        .collect()
}

fn close(a: u32, b: u32) -> bool {
    if a == b {
        return true;
    }
    let (x, y) = (f32::from_bits(a), f32::from_bits(b));
    if x.is_nan() && y.is_nan() {
        return true;
    }
    if !x.is_finite() || !y.is_finite() {
        return false;
    }
    (x - y).abs() <= 1e-4 * x.abs().max(y.abs()).max(1e-2)
}

#[derive(Default, Debug)]
pub struct Verdict {
    pub mismatches: usize,
    pub compared: usize,
    pub unstable: usize,
    pub exact: bool,
    pub written: usize,
}

impl Verdict {
    pub fn text(&self) -> String {
        if self.mismatches == 0 {
            format!(
                "same ({} {} written, {} unstable skipped)",
                self.written,
                if self.exact {
                    "words, bit-exact"
                } else {
                    "words, within tolerance"
                },
                self.unstable
            )
        } else {
            format!("DIFFERENT: {} of {} values", self.mismatches, self.compared)
        }
    }
}

/// `a` and `b` against each other, ignoring what `again` (a second run of `a`) shows to be unstable.
pub fn compare(a: &Output, b: &Output, again: &Output) -> Verdict {
    let mut verdict = Verdict::default();
    let second: HashMap<u32, u32> = again.heap.iter().copied().collect();
    let mine: HashMap<u32, u32> = a.heap.iter().copied().collect();
    let theirs: HashMap<u32, u32> = b.heap.iter().copied().collect();
    let mut unstable = HashSet::new();
    let indexes: HashSet<u32> = mine
        .keys()
        .chain(theirs.keys())
        .chain(second.keys())
        .copied()
        .collect();
    let mut exact = true;
    for index in &indexes {
        let (x, y, z) = (mine.get(index), theirs.get(index), second.get(index));
        if x != z {
            unstable.insert(*index);
            continue;
        }
        verdict.compared += 1;
        match (x, y) {
            (Some(x), Some(y)) => {
                if x != y {
                    exact = false;
                }
                if !close(*x, *y) {
                    verdict.mismatches += 1;
                }
            }
            _ => verdict.mismatches += 1,
        }
    }
    verdict.unstable = unstable.len();
    verdict.written = mine.len();
    for (n, (x, y)) in a.images.iter().zip(&b.images).enumerate() {
        let z = again.images.get(n);
        let mut differing = 0;
        let mut compared = 0;
        for (i, (p, q)) in x.chunks_exact(4).zip(y.chunks_exact(4)).enumerate() {
            let p = u32::from_le_bytes(p.try_into().expect("4 bytes"));
            let q = u32::from_le_bytes(q.try_into().expect("4 bytes"));
            if let Some(z) = z {
                let r = u32::from_le_bytes(z[i * 4..i * 4 + 4].try_into().expect("4 bytes"));
                if r != p {
                    continue;
                }
            }
            compared += 1;
            if p != q {
                exact = false;
                if !close(p, q) {
                    differing += 1;
                }
            }
        }
        verdict.compared += compared;
        verdict.mismatches += differing;
        verdict.written += x.iter().filter(|b| **b != 0).count() / 4;
    }
    verdict.exact = exact;
    verdict
}
