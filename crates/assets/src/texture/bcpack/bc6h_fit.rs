//! Research harness: fit the unrecovered BC6H built-in history from real chunks (ignored tests, gated on
//! `LEONIDA_GAME_DIR`).
//!
//! A chunk's blocks form a forest: every block predicts its endpoints and indices from an earlier block, and the
//! roots (the first blocks) name a history entry. All state arithmetic is a modular addition, so decoding a chain
//! with an all-zero root and re-adding the real root later is exact up to truncation. The unknown roots are fitted
//! per field from what the format and the images imply: `x y z` endpoint deltas are small, anchor indices have a
//! clear top bit, neighbouring indices are close, and the block means follow the exactly stored next mip.

#![allow(clippy::needless_range_loop)]

use super::bc6h::{EVEN, MODES, ODD, interleave, stream_w, unzig_wide};
use super::blocks::{interleave4, unzig};
use super::bptc::{ANCHOR2, PART2};
use super::lanes::LaneSymbols;
use crate::texture::bcn;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Par {
    Mid,
    Back(u32),
    Hist(u8),
}

pub(super) struct Model {
    pub n: usize,
    pub mode: Vec<u8>,
    pub part: Vec<u8>,
    pub pe: Vec<Par>,
    pub pi: Vec<Par>,
    pub dz: Vec<[[i32; 4]; 3]>,
    pub ixd: Vec<[i32; 16]>,
    pub root_e: Vec<i16>,
    pub root_i: Vec<i16>,
}

impl Model {
    pub fn new(lanes: &LaneSymbols, n: usize) -> Model {
        let (l0, l1, l15) = (lanes.lane(0), lanes.lane(1), lanes.lane(15));
        let mut m = Model {
            n,
            mode: Vec::with_capacity(n),
            part: Vec::with_capacity(n),
            pe: Vec::with_capacity(n),
            pi: Vec::with_capacity(n),
            dz: Vec::with_capacity(n),
            ixd: Vec::with_capacity(n),
            root_e: vec![-1; n],
            root_i: vec![-1; n],
        };
        for i in 0..n {
            let (a, b) = (2 * i, 2 * i + 1);
            m.mode.push(l15[a].min(13));
            m.part.push(l15[b].min(31));
            let di = usize::from(interleave4(l1[a], l1[b]));
            let de = (di as i32 + unzig(interleave4(l0[a], l0[b]))) as usize & 255;
            let par = |d: usize| {
                if d == 0 {
                    Par::Mid
                } else if d <= i {
                    Par::Back((i - d) as u32)
                } else {
                    Par::Hist((256 + i - d) as u8)
                }
            };
            m.pe.push(par(de));
            m.pi.push(par(di));
            m.root_e[i] = match m.pe[i] {
                Par::Mid => -1,
                Par::Back(j) => m.root_e[j as usize],
                Par::Hist(k) => i16::from(k),
            };
            m.root_i[i] = match m.pi[i] {
                Par::Mid => -1,
                Par::Back(j) => m.root_i[j as usize],
                Par::Hist(k) => i16::from(k),
            };
            let mut dz = [[0i32; 4]; 3];
            for c in 0..3 {
                dz[c] = [
                    unzig_wide(interleave(lanes.lane(2 + c)[a], lanes.lane(2 + c)[b], 8)),
                    unzig_wide(interleave(lanes.lane(5 + c)[a], lanes.lane(5 + c)[b], 5)),
                    unzig(lanes.lane(8 + c)[a]),
                    unzig(lanes.lane(8 + c)[b]),
                ];
            }
            m.dz.push(dz);
            let mut ixd = [0i32; 16];
            for y in 0..4 {
                let (hi, lo) = if y < 3 { (&ODD, &EVEN) } else { (&EVEN, &ODD) };
                let (sa, sb) = (
                    usize::from(lanes.lane(11 + y)[a]),
                    usize::from(lanes.lane(11 + y)[b]),
                );
                for (off, d) in [hi[sa], lo[sa], hi[sb], lo[sb]].into_iter().enumerate() {
                    ixd[y * 4 + off] = unzig(d);
                }
            }
            m.ixd.push(ixd);
        }
        m
    }

    fn chain(
        &self,
        pars: &[Par],
        mid: u16,
        root: impl Fn(u8) -> u16,
        step: impl Fn(usize, u16) -> u16,
    ) -> Vec<u16> {
        let mut st = vec![0u16; self.n];
        for i in 0..self.n {
            let r = match pars[i] {
                Par::Mid => mid,
                Par::Back(j) => st[j as usize],
                Par::Hist(k) => root(k),
            };
            st[i] = step(i, r);
        }
        st
    }

    pub fn sim_w(&self, c: usize, root: impl Fn(u8) -> u16) -> Vec<u16> {
        self.chain(&self.pe, 0x8000, root, |i, r| {
            let md = &MODES[usize::from(self.mode[i])];
            let sh = 16 - md.base;
            let v = ((i32::from(r >> sh) + self.dz[i][c][0]) & ((1 << md.base) - 1)) as u16;
            v << sh
        })
    }

    pub fn sim_x(&self, c: usize, root: impl Fn(u8) -> u16) -> Vec<u16> {
        self.chain(&self.pe, 0x200, root, |i, r| {
            let dc = MODES[usize::from(self.mode[i])].delta[c];
            let sh = 10 - dc;
            let v = ((i32::from(r >> sh) + self.dz[i][c][1]) & ((1 << dc) - 1)) as u16;
            v << sh
        })
    }

    pub fn sim_yz(
        &self,
        c: usize,
        q: usize,
        root: impl Fn(u8) -> u16,
        w: &[u16],
        x: &[u16],
    ) -> Vec<u16> {
        self.chain(&self.pe, 0x20, root, |i, r| {
            let md = &MODES[usize::from(self.mode[i])];
            let dc = md.delta[c];
            let sh = 6u32.saturating_sub(dc);
            if md.two_regions {
                let v = ((i32::from(r >> sh) + self.dz[i][c][q]) & ((1 << dc) - 1)) as u16;
                v << sh
            } else if q == 3 {
                x[i] >> 4
            } else {
                let wv = u32::from(w[i] >> (16 - md.base));
                let v = (wv & ((1 << dc) - 1)) ^ if md.transformed { 1 << (dc - 1) } else { 0 };
                (if dc >= 6 { v >> (dc - 6) } else { v << sh }) as u16
            }
        })
    }

    fn subset_mask(&self, i: usize) -> u16 {
        if MODES[usize::from(self.mode[i])].two_regions {
            PART2[usize::from(self.part[i])]
        } else {
            0
        }
    }

    /// Index state of pixel `p` for every block, roots given by `root`.
    pub fn sim_index(&self, p: usize, root: impl Fn(u8) -> u8) -> Vec<u8> {
        let mut st = vec![0u8; self.n];
        for i in 0..self.n {
            let r = match self.pi[i] {
                Par::Mid => 8,
                Par::Back(j) => st[j as usize],
                Par::Hist(k) => root(k),
            };
            let ishift = u32::from(MODES[usize::from(self.mode[i])].two_regions);
            let mask = (1i32 << (4 - ishift)) - 1;
            st[i] = ((((i32::from(r) >> ishift) + self.ixd[i][p]) & mask) << ishift) as u8;
        }
        st
    }
}

fn signed(v: i32, bits: u32) -> i32 {
    let h = 1 << (bits - 1);
    (v ^ h) - h
}

const CAP: i32 = 2048;

const W3: [i32; 8] = [0, 9, 18, 27, 37, 46, 55, 64];
const W4: [i32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

fn unquantize(v: i32, bits: u32) -> i32 {
    if bits >= 15 {
        v
    } else if v == 0 {
        0
    } else if v == (1 << bits) - 1 {
        0xFFFF
    } else {
        ((v << 16) + 0x8000) >> bits
    }
}

/// Mean over the block's pixels of one colour channel, from the endpoint fields in BCPack order.
#[allow(clippy::too_many_arguments)]
pub(super) fn block_mean(
    mode: usize,
    part: usize,
    c: usize,
    w: i32,
    xf: i32,
    yf: i32,
    zf: i32,
    ix: &[u8; 16],
) -> f32 {
    let md = &MODES[mode];
    let mask = (1i32 << md.base) - 1;
    let dc = md.delta[c];
    let e0 = stream_w(mode, w as u32) as i32;
    let ends = if md.transformed {
        [
            e0,
            (e0 + signed(xf, dc)) & mask,
            (e0 + signed(yf, dc)) & mask,
            (e0 + signed(zf, dc)) & mask,
        ]
    } else {
        [e0, xf, yf, zf]
    };
    let ends = ends.map(|e| unquantize(e, md.base));
    let mut sum = 0.0f32;
    for p in 0..16 {
        let (sub, weight) = if md.two_regions {
            (
                usize::from(PART2[part] >> p & 1) * 2,
                W3[usize::from(ix[p] >> 1)],
            )
        } else {
            (0, W4[usize::from(ix[p])])
        };
        let v = ((64 - weight) * ends[sub] + weight * ends[sub + 1] + 32) >> 6;
        sum += bcn::half_to_f32(((v.max(0) * 31) >> 6).min(0x7BFF) as u16);
    }
    sum / 16.0
}

pub(super) struct Target {
    /// Mean colour per block of the chunk, `None` where the next mip has no pixels.
    pub mean: Vec<Option<[f32; 3]>>,
}

pub(super) fn log_err(pred: f32, tgt: f32) -> f32 {
    ((pred + 1e-3).ln() - (tgt + 1e-3).ln()).abs().min(4.0)
}

pub(super) struct Item {
    pub name: String,
    pub model: Model,
    pub lanes: LaneSymbols,
    pub target: Option<Target>,
    pub link: Option<Link>,
}

/// A chunk whose next mip is another BCPack chunk: its target comes from decoding that chunk.
pub(super) struct Link {
    pub next: usize,
    pub pos: Vec<Option<(u32, u32)>>,
    pub npos: Vec<Option<(u32, u32)>>,
    pub next_mip: crate::texture::layout::MipLayout,
}

/// The fitted table: per entry `[channel]` roots of each endpoint field and the 16 index roots.
pub(super) struct Table {
    pub w: Vec<[Option<u16>; 3]>,
    pub x: Vec<[Option<u16>; 3]>,
    pub y: Vec<[Option<u16>; 3]>,
    pub z: Vec<[Option<u16>; 3]>,
    pub ix: Vec<Option<[u8; 16]>>,
}

impl Table {
    pub fn new() -> Table {
        Table {
            w: vec![[None; 3]; 256],
            x: vec![[None; 3]; 256],
            y: vec![[None; 3]; 256],
            z: vec![[None; 3]; 256],
            ix: vec![None; 256],
        }
    }

    pub fn endpoints(&self, k: usize) -> [[u16; 4]; 3] {
        std::array::from_fn(|c| {
            [
                self.w[k][c].unwrap_or(0x8000),
                self.x[k][c].unwrap_or(0x200),
                self.y[k][c].unwrap_or(0x20),
                self.z[k][c].unwrap_or(0x20),
            ]
        })
    }
}

fn argmin<const N: usize>(row: &[i64; N]) -> usize {
    (0..N).min_by_key(|&t| row[t]).unwrap()
}

fn get(tab: &[[Option<u16>; 3]], c: usize) -> impl Fn(u8) -> u16 + '_ {
    move |k: u8| tab[usize::from(k)][c].unwrap_or(0)
}

/// `x` roots: smallest signed deltas.
fn fit_x(items: &[Item], c: usize) -> Vec<Option<u16>> {
    let mut cost = vec![[0i64; 1024]; 256];
    let mut seen = [false; 256];
    for it in items {
        let m = &it.model;
        for t in 0..1024u16 {
            let st = m.sim_x(c, |_| t);
            for i in 0..m.n {
                let k = m.root_e[i];
                if k < 0 {
                    continue;
                }
                seen[k as usize] = true;
                let md = &MODES[usize::from(m.mode[i])];
                if !md.transformed {
                    continue;
                }
                let dc = md.delta[c];
                let sx = signed(i32::from(st[i] >> (10 - dc)), dc);
                cost[k as usize][usize::from(t)] +=
                    i64::from((sx.abs() << (16 - md.base)).min(CAP));
            }
        }
    }
    (0..256)
        .map(|k| seen[k].then(|| argmin(&cost[k]) as u16))
        .collect()
}

fn fit_yz(items: &[Item], c: usize, q: usize, w: &[Vec<u16>], x: &[Vec<u16>]) -> Vec<Option<u16>> {
    let mut cost = vec![[0i64; 64]; 256];
    let mut seen = [false; 256];
    for (n, it) in items.iter().enumerate() {
        let m = &it.model;
        for t in 0..64u16 {
            let st = m.sim_yz(c, q, |_| t, &w[n], &x[n]);
            for i in 0..m.n {
                let k = m.root_e[i];
                if k < 0 {
                    continue;
                }
                seen[k as usize] = true;
                let md = &MODES[usize::from(m.mode[i])];
                if !md.two_regions || !md.transformed {
                    continue;
                }
                let dc = md.delta[c];
                let sv = signed(i32::from(st[i] >> 6u32.saturating_sub(dc)), dc);
                cost[k as usize][usize::from(t)] +=
                    i64::from((sv.abs() << (16 - md.base)).min(CAP));
            }
        }
    }
    (0..256)
        .map(|k| seen[k].then(|| argmin(&cost[k]) as u16))
        .collect()
}

/// Index roots: no anchor violations, then neighbouring pixels of one subset as close as possible.
fn fit_index(items: &[Item], sweeps: usize) -> Vec<Option<[u8; 16]>> {
    let mut used = [false; 256];
    let mut viol = vec![[[0i64; 16]; 16]; 256];
    for it in items {
        let m = &it.model;
        for p in 0..16 {
            for t in 0..16u8 {
                let st = m.sim_index(p, |_| t);
                for i in 0..m.n {
                    let k = m.root_i[i];
                    if k < 0 {
                        continue;
                    }
                    used[k as usize] = true;
                    let two = MODES[usize::from(m.mode[i])].two_regions;
                    let anchor =
                        p == 0 || (two && usize::from(ANCHOR2[usize::from(m.part[i])]) == p);
                    if anchor && st[i] >= 8 {
                        viol[k as usize][p][usize::from(t)] += 1;
                    }
                }
            }
        }
    }
    let mut r = vec![[8u8; 16]; 256];
    for k in 0..256 {
        for p in 0..16 {
            r[k][p] = argmin(&viol[k][p]) as u8;
        }
    }
    let masks: Vec<Vec<u16>> = items
        .iter()
        .map(|it| (0..it.model.n).map(|i| it.model.subset_mask(i)).collect())
        .collect();
    for _ in 0..sweeps {
        for p in 0..16 {
            let nbrs: Vec<usize> = [
                (p % 4 > 0).then(|| p - 1),
                (p % 4 < 3).then(|| p + 1),
                (p >= 4).then(|| p - 4),
                (p < 12).then(|| p + 4),
            ]
            .into_iter()
            .flatten()
            .collect();
            let mut cost = vec![[0i64; 16]; 256];
            for k in 0..256 {
                for t in 0..16 {
                    cost[k][t] = 1000 * viol[k][p][t];
                }
            }
            for (n, it) in items.iter().enumerate() {
                let m = &it.model;
                let nb: Vec<Vec<u8>> = nbrs
                    .iter()
                    .map(|&q| m.sim_index(q, |k| r[usize::from(k)][q]))
                    .collect();
                for t in 0..16u8 {
                    let st = m.sim_index(p, |_| t);
                    for i in 0..m.n {
                        let k = m.root_i[i];
                        if k < 0 {
                            continue;
                        }
                        let mask = masks[n][i];
                        let mut s = 0i64;
                        for (j, &q) in nbrs.iter().enumerate() {
                            if (mask >> p ^ mask >> q) & 1 == 0 {
                                s += i64::from(st[i].abs_diff(nb[j][i]));
                            }
                        }
                        cost[k as usize][usize::from(t)] += s;
                    }
                }
            }
            for k in 0..256 {
                if used[k] {
                    r[k][p] = argmin(&cost[k]) as u8;
                }
            }
        }
    }
    (0..256).map(|k| used[k].then_some(r[k])).collect()
}

struct Prepared {
    sampled: Vec<Vec<usize>>,
    ixs: Vec<[u8; 16]>,
}

fn prepare(it: &Item, ix: &[Option<[u8; 16]>]) -> Prepared {
    let m = &it.model;
    let mut sampled: Vec<Vec<usize>> = vec![Vec::new(); 256];
    if let Some(t) = &it.target {
        let mut per: Vec<Vec<usize>> = vec![Vec::new(); 256];
        for i in 0..m.n {
            if m.root_e[i] >= 0 && t.mean[i].is_some() {
                per[m.root_e[i] as usize].push(i);
            }
        }
        for k in 0..256 {
            let take = per[k].len().min(48);
            for s in 0..take {
                sampled[k].push(per[k][s * per[k].len() / take]);
            }
        }
    }
    let mut ixs = vec![[0u8; 16]; m.n];
    for p in 0..16 {
        let st = m.sim_index(p, |k| ix[usize::from(k)].map_or(8, |r| r[p]));
        for i in 0..m.n {
            ixs[i][p] = st[i] >> u32::from(MODES[usize::from(m.mode[i])].two_regions);
        }
    }
    Prepared { sampled, ixs }
}

pub(super) fn fit_corpus(items: &[Item], rounds: usize) -> Table {
    let mut tab = Table::new();
    for c in 0..3 {
        for (k, v) in fit_x(items, c).into_iter().enumerate() {
            tab.x[k][c] = v;
        }
    }
    tab.ix = fit_index(items, 3);
    let prep: Vec<Prepared> = items.iter().map(|it| prepare(it, &tab.ix)).collect();
    for _ in 0..rounds {
        let xs: Vec<Vec<Vec<u16>>> = items
            .iter()
            .map(|it| (0..3).map(|c| it.model.sim_x(c, get(&tab.x, c))).collect())
            .collect();
        let ws: Vec<Vec<Vec<u16>>> = items
            .iter()
            .map(|it| (0..3).map(|c| it.model.sim_w(c, get(&tab.w, c))).collect())
            .collect();
        for c in 0..3 {
            let wc: Vec<Vec<u16>> = ws.iter().map(|w| w[c].clone()).collect();
            let xc: Vec<Vec<u16>> = xs.iter().map(|x| x[c].clone()).collect();
            for (k, v) in fit_yz(items, c, 2, &wc, &xc).into_iter().enumerate() {
                tab.y[k][c] = v;
            }
            for (k, v) in fit_yz(items, c, 3, &wc, &xc).into_iter().enumerate() {
                tab.z[k][c] = v;
            }
        }
        for c in 0..3 {
            let sts: Vec<(Vec<u16>, Vec<u16>, Vec<u16>)> = items
                .iter()
                .enumerate()
                .map(|(n, it)| {
                    let wc = &ws[n][c];
                    (
                        xs[n][c].clone(),
                        it.model.sim_yz(c, 2, get(&tab.y, c), wc, &xs[n][c]),
                        it.model.sim_yz(c, 3, get(&tab.z, c), wc, &xs[n][c]),
                    )
                })
                .collect();
            let eval = |n: usize, tw: &[u16], cost: &mut [f64]| {
                let (m, t) = (&items[n].model, items[n].target.as_ref().unwrap());
                for k in 0..256 {
                    for &i in &prep[n].sampled[k] {
                        let md = &MODES[usize::from(m.mode[i])];
                        let sh_y = 6u32.saturating_sub(md.delta[c]);
                        let pred = block_mean(
                            usize::from(m.mode[i]),
                            usize::from(m.part[i]),
                            c,
                            i32::from(tw[i] >> (16 - md.base)),
                            i32::from(sts[n].0[i] >> (10 - md.delta[c])),
                            i32::from(sts[n].1[i] >> sh_y),
                            i32::from(sts[n].2[i] >> sh_y),
                            &prep[n].ixs[i],
                        );
                        cost[k] += f64::from(log_err(pred, t.mean[i].unwrap()[c]));
                    }
                }
            };
            let with_target: Vec<usize> = (0..items.len())
                .filter(|&n| items[n].target.is_some())
                .collect();
            let seen: Vec<bool> = (0..256)
                .map(|k| with_target.iter().any(|&n| !prep[n].sampled[k].is_empty()))
                .collect();
            let mut coarse: Vec<Vec<(f64, u16)>> = vec![Vec::new(); 256];
            for tcand in (0..65536u32).step_by(128) {
                let mut cost = vec![0f64; 256];
                for &n in &with_target {
                    let tw = items[n].model.sim_w(c, |_| tcand as u16);
                    eval(n, &tw, &mut cost);
                }
                for k in 0..256 {
                    if seen[k] {
                        coarse[k].push((cost[k], tcand as u16));
                    }
                }
            }
            let keep = |v: &mut Vec<(f64, u16)>| {
                v.sort_by(|a, b| a.0.total_cmp(&b.0));
                v.truncate(3);
            };
            coarse.iter_mut().for_each(keep);
            for (step, span) in [(8i32, 128i32), (1, 8)] {
                let mut next: Vec<Vec<(f64, u16)>> = vec![Vec::new(); 256];
                for rank in 0..3 {
                    for o in (-span..=span).step_by(step as usize) {
                        let at = |k: u8| {
                            coarse[usize::from(k)]
                                .get(rank)
                                .map_or(0u16, |(_, t)| (i32::from(*t) + o).rem_euclid(65536) as u16)
                        };
                        let mut cost = vec![0f64; 256];
                        for &n in &with_target {
                            let tw = items[n].model.sim_w(c, at);
                            eval(n, &tw, &mut cost);
                        }
                        for k in 0..256 {
                            if coarse[k].get(rank).is_some() {
                                next[k].push((cost[k], at(k as u8)));
                            }
                        }
                    }
                }
                next.iter_mut().for_each(keep);
                coarse = next;
            }
            for k in 0..256 {
                if let Some(&(_, t)) = coarse[k].first() {
                    tab.w[k][c] = Some(t);
                }
            }
        }
    }
    tab
}
