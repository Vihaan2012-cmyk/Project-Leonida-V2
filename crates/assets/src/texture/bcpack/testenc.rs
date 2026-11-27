//! Test-only encoder for the lane array: the exact inverse of [`super::lanes`] for the same models.

use super::lanes::{ByteSource, decode_lanes};
use super::model::{L, Model, model};

/// One fragment of a lane: `z` narrows the symbol width, `q` picks the probability model.
#[derive(Clone, Copy)]
pub struct Frag {
    pub z: u8,
    pub q: u8,
    /// Symbols the fragment should hold (cut fragments close at the first legal cut point past this).
    pub count: usize,
}

pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }

    /// A symbol below `1 << bits`, skewed towards small values.
    pub fn skewed(&mut self, bits: u32) -> u8 {
        let v = self.next() & ((1 << bits) - 1);
        let shift = self.next() % (bits + 1);
        (v >> shift) as u8
    }
}

fn eab_of(default_width: u8, z: u8) -> usize {
    usize::from(default_width - z - 1)
}

/// Bytes emitted before encoding `sym` from state `x` (renormalisation), and the new state.
fn encode_step(x: u32, m: &Model, sym: u8) -> (Vec<u8>, u32) {
    let (f, c) = m.interval(sym);
    let mut x = x;
    let mut out = Vec::new();
    while u64::from(x) >= u64::from(f) << 16 {
        out.push(x as u8);
        x >>= 8;
    }
    (out, ((x / f) << 16) + (x % f) + c)
}

fn would_emit(x: u32, m: &Model, sym: u8) -> bool {
    let (f, _) = m.interval(sym);
    u64::from(x) >= u64::from(f) << 16
}

fn header(frag: Frag, flush: bool, payload: usize) -> Vec<u8> {
    assert!((1..=65792).contains(&payload));
    let mut h0 = frag.z << 6 | frag.q | u8::from(flush) << 4;
    if payload <= 256 {
        vec![h0, (payload - 1) as u8]
    } else {
        h0 |= 0x20;
        let v = payload - 257;
        vec![h0, v as u8, (v >> 8) as u8]
    }
}

/// Independent flush fragments: each chunk of symbols is its own rANS stream ending at the lower bound.
pub fn encode_flush(default_width: u8, chunks: &[(Frag, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    for (frag, symbols) in chunks {
        let m = model(eab_of(default_width, frag.z), usize::from(frag.q));
        let mut x = L;
        let mut emitted = Vec::new();
        for (j, &s) in symbols.iter().enumerate().rev() {
            let (e, nx) = encode_step(x, m, s);
            // The bytes before the last symbol are the zero tail of the lower bound: the format omits them.
            if j + 1 < symbols.len() {
                emitted.extend(e);
            }
            x = nx;
        }
        let mut payload = x.to_be_bytes().to_vec();
        payload.extend(emitted.iter().rev());
        out.extend(header(*frag, true, payload.len()));
        out.extend(payload);
    }
    out
}

/// One rANS stream cut into fragments that carry the state across (no flush), switching model at the cuts.
pub fn encode_cut(default_width: u8, plan: &[Frag], symbols: &[u8]) -> Vec<u8> {
    let n = symbols.len();
    let m_of = |f: usize| model(eab_of(default_width, plan[f].z), usize::from(plan[f].q));
    let mut cur = plan.len() - 1;
    let mut in_cur = 0usize;
    let mut starts = vec![n];
    let mut x = L;
    let mut em: Vec<Vec<u8>> = vec![Vec::new(); n];
    for j in (0..n).rev() {
        if cur > 0 && in_cur >= plan[cur].count && would_emit(x, m_of(cur - 1), symbols[j]) {
            starts.push(j + 1);
            cur -= 1;
            in_cur = 0;
        }
        let (e, nx) = encode_step(x, m_of(cur), symbols[j]);
        em[j] = e;
        x = nx;
        in_cur += 1;
    }
    starts.push(0);
    starts.reverse();
    // starts: [0, cut1, cut2, ..., n]; fragment f holds symbols starts[f]..starts[f + 1].
    let mut out = Vec::new();
    let frags = starts.len() - 1;
    assert_eq!(frags, plan.len(), "the plan did not fit the data");
    for f in 0..frags {
        let (a, b) = (starts[f], starts[f + 1]);
        let mut payload = if f == 0 {
            x.to_be_bytes().to_vec()
        } else {
            Vec::new()
        };
        // Bytes owed after symbol i belong to the fragment that holds symbol i + 1.
        for e in &em[if f == 0 { 0 } else { a - 1 }..b - 1] {
            payload.extend(e.iter().rev());
        }
        out.extend(header(plan[f], false, payload.len()));
        out.extend(payload);
    }
    out
}

struct Recorder {
    queues: Vec<std::collections::VecDeque<u8>>,
    stream: Vec<u8>,
}

impl ByteSource for Recorder {
    fn next(&mut self, lane: usize) -> Option<u8> {
        let b = self.queues[lane].pop_front()?;
        self.stream.push(b);
        Some(b)
    }

    fn consumed(&self) -> usize {
        self.stream.len()
    }
}

/// Interleave per-lane byte strings into the shared stream by replaying the clock.
pub fn interleave(widths: &[u8], lane_bytes: Vec<Vec<u8>>, per_lane: usize) -> Vec<u8> {
    let mut rec = Recorder {
        queues: lane_bytes.into_iter().map(Into::into).collect(),
        stream: Vec::new(),
    };
    decode_lanes(&mut rec, widths, per_lane).expect("lanes decode from their own bytes");
    assert!(rec.queues.iter().all(|q| q.is_empty()), "bytes left over");
    rec.stream
}
