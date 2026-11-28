//! Driver of the BC6H history fit on real pools (ignored test).

use super::bc6h::{Entry, History, Trace, rebuild};
use super::bc6h_fit::{Item, Link, Model, Table, Target, fit_corpus, log_err};
use super::lanes::decode_slice;
use super::{BcPackCodec, DecodeOptions};
use crate::texture::bcn;
use crate::texture::extract::extract_mip;
use crate::texture::layout::{MipLayout, SurfaceLayout, compact_surface_layout};
use crate::texture::pool::{ChunkCache, Pool};
use crate::texture::tiling::TileSize;
use std::collections::HashSet;

fn list(var: &str, default: &str) -> Vec<String> {
    std::env::var(var)
        .unwrap_or(default.into())
        .split(',')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn quantile(v: &mut [f32], q: f32) -> f32 {
    v.sort_by(|a, b| a.total_cmp(b));
    v.get(((v.len() as f32 - 1.0) * q) as usize)
        .copied()
        .unwrap_or(f32::NAN)
}

/// Block grid position of every block of a chunk covering `mip`.
fn positions(layout: &SurfaceLayout, mip: &MipLayout, n: usize) -> Vec<Option<(u32, u32)>> {
    let mut tagged = vec![0u8; mip.region as usize];
    for j in 0..n {
        tagged[j * 16..j * 16 + 4].copy_from_slice(&(j as u32).to_le_bytes());
    }
    let lin = extract_mip(layout, mip, &tagged).unwrap();
    let (vw, vh) = mip.visible_blocks(4);
    let mut pos = vec![None; n];
    for l in 0..(vw * vh) as usize {
        let j = u32::from_le_bytes(lin[l * 16..l * 16 + 4].try_into().unwrap()) as usize;
        if j < n {
            pos[j] = Some((l as u32 % vw, l as u32 / vw));
        }
    }
    pos
}

/// Mean colour of the 2x2 pixels of the next mip under each block, from the next mip's pixel grid.
fn means(
    pos: &[Option<(u32, u32)>],
    pix: &[[f32; 3]],
    stride: usize,
    pw: usize,
    ph: usize,
) -> Vec<Option<[f32; 3]>> {
    pos.iter()
        .map(|p| {
            let (x, y) = (*p)?;
            let mut acc = [0f32; 3];
            let mut cnt = 0.0;
            for dy in 0..2usize {
                for dx in 0..2usize {
                    let (px, py) = (2 * x as usize + dx, 2 * y as usize + dy);
                    if px < pw && py < ph {
                        let v = pix[py * stride + px];
                        (0..3).for_each(|c| acc[c] += v[c]);
                        cnt += 1.0;
                    }
                }
            }
            (cnt > 0.0).then(|| acc.map(|a| a / cnt))
        })
        .collect()
}

/// Pixel grid of a mip from its blocks in chunk order (`pos` gives each block's place).
fn pixel_grid(
    next: &MipLayout,
    pos: &[Option<(u32, u32)>],
    blocks: &[u8],
) -> (Vec<[f32; 3]>, usize) {
    let (nvw, nvh) = next.visible_blocks(4);
    let stride = nvw as usize * 4;
    let mut pix = vec![[0f32; 3]; stride * nvh as usize * 4];
    for (j, p) in pos.iter().enumerate() {
        let Some((bx, by)) = *p else { continue };
        for (q, v) in bcn::bc6h_block(&blocks[j * 16..j * 16 + 16], false)
            .into_iter()
            .enumerate()
        {
            pix[(by as usize * 4 + q / 4) * stride + bx as usize * 4 + q % 4] = v;
        }
    }
    (pix, stride)
}

fn load_pool(dir: &std::path::Path, rel: &str, untargeted: bool) -> Vec<Item> {
    let widths = BcPackCodec::Bc6h.lane_widths().unwrap();
    let file = std::fs::read(dir.join("data/ttdb").join(rel)).unwrap();
    let pool = Pool::parse(&file).unwrap();
    let mut items: Vec<Item> = Vec::new();
    let mut done: HashSet<u64> = HashSet::new();
    for ti in 0..pool.texture_count() {
        let Some(d) = pool.descriptor(ti) else {
            continue;
        };
        if !matches!(d.format, 95 | 96) || d.dimension != 1 || d.depth != 1 || d.is_unmapped() {
            continue;
        }
        let Ok(layout) = compact_surface_layout(
            d.width,
            d.height,
            d.mips,
            16,
            4,
            TileSize::from_descriptor_mode(d.mode),
        ) else {
            continue;
        };
        if pool.span(ti, d.base) != Some(layout.size) {
            continue;
        }
        let mut item_of: Vec<Option<usize>> = vec![None; layout.mips.len()];
        let mut mip_pos: Vec<Vec<Option<(u32, u32)>>> = vec![Vec::new(); layout.mips.len()];
        for mi in 0..layout.mips.len() {
            let mip = &layout.mips[mi];
            let (start, size) = (d.base + mip.offset, mip.region);
            let Ok(chs) = pool.chunks_for(start, size) else {
                continue;
            };
            if chs.len() != 1 || chs[0].codec != 6 || chs[0].virt != start || chs[0].size != size {
                continue;
            }
            let c = chs[0];
            let raw = &file[c.src as usize..(c.src + c.packed) as usize];
            let n = size as usize / 16;
            let Ok(lanes) = decode_slice(raw, widths, n * 2) else {
                continue;
            };
            done.insert(c.src);
            mip_pos[mi] = positions(&layout, mip, n);
            item_of[mi] = Some(items.len());
            items.push(Item {
                name: format!("{rel}#{ti}m{mi}"),
                model: Model::new(&lanes, n),
                lanes,
                target: None,
                link: None,
            });
        }
        for mi in 0..layout.mips.len().saturating_sub(1) {
            let Some(it) = item_of[mi] else { continue };
            let next = &layout.mips[mi + 1];
            let (nstart, nsize) = (d.base + next.offset, next.region);
            let Ok(nchs) = pool.chunks_for(nstart, nsize) else {
                continue;
            };
            if nchs.iter().all(|c| c.codec == 0 || c.codec == 8) {
                let ntiled = pool
                    .read_range(nstart, nsize, &mut ChunkCache::new())
                    .unwrap();
                let nlin = extract_mip(&layout, next, &ntiled).unwrap();
                let (nvw, nvh) = next.visible_blocks(4);
                let npos: Vec<Option<(u32, u32)>> =
                    (0..(nvw * nvh)).map(|l| Some((l % nvw, l / nvw))).collect();
                let (pix, stride) = pixel_grid(next, &npos, &nlin);
                items[it].target = Some(Target {
                    mean: means(
                        &mip_pos[mi],
                        &pix,
                        stride,
                        next.pixels_w as usize,
                        next.pixels_h as usize,
                    ),
                });
            } else if let Some(nit) = item_of[mi + 1] {
                items[it].link = Some(Link {
                    next: nit,
                    pos: mip_pos[mi].clone(),
                    npos: mip_pos[mi + 1].clone(),
                    next_mip: next.clone(),
                });
            }
        }
    }
    if untargeted {
        for (j, c) in pool.chunks().iter().enumerate() {
            if c.codec != 6 || done.contains(&c.src) {
                continue;
            }
            let raw = &file[c.src as usize..(c.src + c.packed) as usize];
            let n = c.size as usize / 16;
            let Ok(lanes) = decode_slice(raw, widths, n * 2) else {
                continue;
            };
            items.push(Item {
                name: format!("{rel}:chunk{j}"),
                model: Model::new(&lanes, n),
                lanes,
                target: None,
                link: None,
            });
        }
    }
    items
}

/// Targets of the linked chunks from the decode of their next mip under `tab`.
fn bootstrap(items: &mut [Item], tab: &Table) {
    for i in 0..items.len() {
        let Some(link) = items[i].link.take() else {
            continue;
        };
        let nxt = &items[link.next];
        let hist = history_of(nxt, tab);
        let mut out = vec![0u8; nxt.model.n * 16];
        let opts = DecodeOptions {
            allow_provisional: true,
        };
        rebuild(
            &nxt.lanes,
            &mut out,
            &hist,
            &opts,
            Some(&mut Trace::default()),
        )
        .unwrap();
        let (pix, stride) = pixel_grid(&link.next_mip, &link.npos, &out);
        let t = means(
            &link.pos,
            &pix,
            stride,
            link.next_mip.pixels_w as usize,
            link.next_mip.pixels_h as usize,
        );
        items[i].target = Some(Target { mean: t });
        items[i].link = Some(link);
    }
}

fn history_of(item: &Item, tab: &Table) -> History {
    let mut hist = History::empty();
    for k in 0..256 {
        let used = |roots: &[i16]| roots.contains(&(k as i16));
        if used(&item.model.root_e) {
            hist.endpoints[k] = Entry::Confirmed(tab.endpoints(k));
        }
        if used(&item.model.root_i) {
            hist.indices[k] = Entry::Confirmed(tab.ix[k].unwrap_or([8; 16]));
        }
    }
    hist
}

fn evaluate(items: &[Item], tab: &Table, label: &str) {
    let mut all = Vec::new();
    for it in items {
        let hist = history_of(it, tab);
        let mut out = vec![0u8; it.model.n * 16];
        let mut trace = Trace::default();
        let opts = DecodeOptions {
            allow_provisional: true,
        };
        rebuild(&it.lanes, &mut out, &hist, &opts, Some(&mut trace)).unwrap();
        let Some(target) = &it.target else {
            eprintln!(
                "{label} {}: {} blocks, {} anchor faults",
                it.name, it.model.n, trace.faulty
            );
            continue;
        };
        let mut errs = Vec::new();
        for j in 0..it.model.n {
            let Some(t) = target.mean[j] else {
                continue;
            };
            let px = bcn::bc6h_block(&out[j * 16..j * 16 + 16], false);
            for c in 0..3 {
                let m: f32 = px.iter().map(|p| p[c]).sum::<f32>() / 16.0;
                errs.push(log_err(m, t[c]));
            }
        }
        all.extend(errs.iter().copied());
        let (med, p90) = (quantile(&mut errs.clone(), 0.5), quantile(&mut errs, 0.9));
        eprintln!(
            "{label} {}: {} blocks, {} anchor faults, block-mean log error median {med:.3} p90 {p90:.3}",
            it.name, it.model.n, trace.faulty
        );
    }
    eprintln!(
        "{label} overall: median {:.3} p90 {:.3} over {} samples",
        quantile(&mut all.clone(), 0.5),
        quantile(&mut all, 0.9),
        all.len()
    );
}

#[test]
#[ignore]
fn fit_history_on_pools() {
    let Some(dir) = std::env::var_os("LEONIDA_GAME_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let rounds: usize = std::env::var("LEONIDA_BC6H_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let mut fit_items = Vec::new();
    for rel in list(
        "LEONIDA_BC6H_POOLS",
        "lighting/lighting.ttpf,vfx/ttdb_vfx.ttpf",
    ) {
        let base = fit_items.len();
        let mut part = load_pool(&dir, &rel, true);
        for it in &mut part {
            if let Some(l) = &mut it.link {
                l.next += base;
            }
        }
        fit_items.extend(part);
    }
    eprintln!(
        "{} fit chunks, {} blocks",
        fit_items.len(),
        fit_items.iter().map(|i| i.model.n).sum::<usize>()
    );
    let mut tab = fit_corpus(&fit_items, rounds);
    for pass in 0..std::env::var("LEONIDA_BC6H_BOOTSTRAP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
    {
        bootstrap(&mut fit_items, &tab);
        let linked = fit_items.iter().filter(|i| i.link.is_some()).count();
        eprintln!("bootstrap pass {pass}: {linked} linked chunks");
        tab = fit_corpus(&fit_items, rounds);
    }
    evaluate(&fit_items, &tab, "fit");
    let held: Vec<Item> = list("LEONIDA_BC6H_EVAL", "")
        .iter()
        .flat_map(|rel| load_pool(&dir, rel, false))
        .collect();
    if !held.is_empty() {
        evaluate(&held, &tab, "held-out");
    }
    let mut dump = String::new();
    for k in 0..256 {
        use std::fmt::Write;
        let _ = writeln!(
            dump,
            "{k} w {:?} x {:?} y {:?} z {:?} ix {:?}",
            tab.w[k], tab.x[k], tab.y[k], tab.z[k], tab.ix[k]
        );
    }
    let out = std::path::Path::new("E:/Project Leonida V2/wt/target-5/bc6h-fit");
    std::fs::create_dir_all(out).unwrap();
    std::fs::write(out.join("table.txt"), dump).unwrap();
}
