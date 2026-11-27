//! Finding what the CPU wrote: address-range sets, and Windows' write watch queried so that it stays cheap.
//!
//! Measured on an RTX 4060 machine (Windows 11): `GetWriteWatch` over 2.5 GiB with nothing written takes
//! 0.3-0.5 ms in 128 MiB calls (1.2 ms in 16 MiB calls) while the pages were never touched, but 8-10 ms once they
//! are resident and imported into Vulkan (13 ns a page), and about 0.15 us for every page it reports and resets.
//! A scan every submission therefore suits only memory the game writes without saying so; large static memory
//! should be `SyncPolicy::Marked` and announced with `mark_dirty`.
//! The larger cost is the write fault the game thread takes on the first write to a page after the reset:
//! 0.7 us a page, so a buffer rewritten in full every frame costs the game 45 ms a 256 MiB. Tiles written
//! that densely are therefore left unreset and copied whole; they are probed now and then to see whether they
//! calmed down.

use std::collections::BTreeMap;
use std::ffi::c_void;

/// The write-watch page, and the size of the pages `GetWriteWatch` reports.
pub(super) const WATCH_PAGE: u64 = 4096;

/// Heat is tracked per tile of a memory object.
pub(super) const TILE: u64 = 1 << 20;

/// Pages asked of one `GetWriteWatch` call. A call never covers more, so its answer cannot overflow the buffer.
const QUERY_PAGES: u64 = 32768;

/// A tile with at least this many written pages (a quarter) in a frame is dense.
const DENSE_PAGES: u32 = (TILE / WATCH_PAGE / 4) as u32;

/// Dense frames in a row before a tile is copied whole without reset.
const DENSE_STREAK: u8 = 2;

/// A hot tile is looked at again after this many submissions.
const PROBE_AFTER: u16 = 32;

/// Disjoint half-open address ranges.
#[derive(Default, Debug, PartialEq, Eq)]
pub(super) struct RangeSet(pub(super) BTreeMap<u64, u64>);

impl RangeSet {
    pub(super) fn add(&mut self, lo: u64, hi: u64) {
        if lo >= hi {
            return;
        }
        let (mut lo, mut hi) = (lo, hi);
        if let Some((&s, &e)) = self.0.range(..=lo).next_back()
            && e >= lo
        {
            lo = s;
            hi = hi.max(e);
        }
        while let Some((&s, &e)) = self.0.range(lo..=hi).next() {
            self.0.remove(&s);
            hi = hi.max(e);
        }
        self.0.insert(lo, hi);
    }

    pub(super) fn remove(&mut self, lo: u64, hi: u64) {
        if lo >= hi {
            return;
        }
        let first = self
            .0
            .range(..lo)
            .next_back()
            .filter(|(_, e)| **e > lo)
            .map(|(s, _)| *s)
            .unwrap_or(lo);
        let hits: Vec<(u64, u64)> = self.0.range(first..hi).map(|(s, e)| (*s, *e)).collect();
        for (s, e) in hits {
            self.0.remove(&s);
            if s < lo {
                self.0.insert(s, lo);
            }
            if e > hi {
                self.0.insert(hi, e);
            }
        }
    }

    pub(super) fn take(&mut self) -> Vec<(u64, u64)> {
        std::mem::take(&mut self.0).into_iter().collect()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The parts of this set inside `lo..hi`, in order.
    pub(super) fn pieces(&self, lo: u64, hi: u64) -> Vec<(u64, u64)> {
        let first = self
            .0
            .range(..lo)
            .next_back()
            .filter(|(_, e)| **e > lo)
            .map(|(s, _)| *s)
            .unwrap_or(lo);
        self.0
            .range(first..hi)
            .map(|(s, e)| ((*s).max(lo), (*e).min(hi)))
            .collect()
    }
}

/// Sorts `runs` and joins the ones that overlap or touch.
pub(super) fn coalesce(runs: &mut Vec<(u64, u64)>) {
    if runs.len() < 2 {
        return;
    }
    if !runs.windows(2).all(|w| w[0].0 <= w[1].0) {
        runs.sort_unstable();
    }
    let mut out = 0;
    for i in 1..runs.len() {
        if runs[i].0 <= runs[out].1 {
            runs[out].1 = runs[out].1.max(runs[i].1);
        } else {
            out += 1;
            runs[out] = runs[i];
        }
    }
    runs.truncate(out + 1);
}

/// `runs` (sorted, disjoint) without the bytes in `set`.
pub(super) fn subtract(runs: &[(u64, u64)], set: &RangeSet) -> Vec<(u64, u64)> {
    if set.is_empty() {
        return runs.to_vec();
    }
    let mut out = Vec::with_capacity(runs.len());
    for &(lo, hi) in runs {
        let mut at = lo;
        for (s, e) in set.pieces(lo, hi) {
            if s > at {
                out.push((at, s));
            }
            at = e;
        }
        if at < hi {
            out.push((at, hi));
        }
    }
    out
}

/// The parts of `runs` (sorted, disjoint) inside `set`.
pub(super) fn intersect(runs: &[(u64, u64)], set: &RangeSet) -> Vec<(u64, u64)> {
    if set.is_empty() {
        return Vec::new();
    }
    runs.iter()
        .flat_map(|&(lo, hi)| set.pieces(lo, hi))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Heat {
    /// Queried and reset every time; the number of dense frames in a row.
    Cold(u8),
    /// Copied whole without a query; the submissions since it was last looked at.
    Hot(u16),
    /// Reset last time it was looked at: the next query says how densely it is written now.
    Probing,
}

/// How hot each tile of a memory object is.
#[derive(Default)]
pub(super) struct Tiles {
    heat: Vec<Heat>,
    /// Set when the system could not report on the memory: it is then copied whole.
    unwatchable: bool,
}

impl Tiles {
    #[cfg(test)]
    pub(super) fn hot_tiles(&self) -> usize {
        self.heat
            .iter()
            .filter(|h| matches!(h, Heat::Hot(_)))
            .count()
    }
}

/// Reusable buffers for the queries.
#[derive(Default)]
pub(super) struct Scanner {
    counts: Vec<u32>,
}

impl Scanner {
    /// Appends to `out` the host ranges written since the last scan, in `host..host + len`, which is the part of a
    /// memory object that starts `offset` bytes into it. Returns `false`, appending nothing, when the system
    /// cannot say: the caller must treat everything as written.
    pub(super) fn scan(
        &mut self,
        tiles: &mut Tiles,
        host: u64,
        len: u64,
        offset: u64,
        out: &mut Vec<(u64, u64)>,
    ) -> bool {
        if tiles.unwatchable {
            return false;
        }
        let first = offset / TILE;
        let last = (offset + len - 1) / TILE;
        if tiles.heat.len() <= last as usize {
            tiles.heat.resize(last as usize + 1, Heat::Cold(0));
        }
        let span = |tile: u64| -> (u64, u64) {
            let lo = (tile * TILE).max(offset);
            let hi = ((tile + 1) * TILE).min(offset + len);
            (host + (lo - offset), host + (hi - offset))
        };
        self.counts.clear();
        self.counts.resize((last - first + 1) as usize, 0);
        let mark = out.len();
        let mut tile = first;
        while tile <= last {
            if matches!(tiles.heat[tile as usize], Heat::Hot(age) if age < PROBE_AFTER) {
                tile += 1;
                continue;
            }
            let mut end = tile;
            while end < last
                && !matches!(tiles.heat[end as usize + 1], Heat::Hot(age) if age < PROBE_AFTER)
            {
                end += 1;
            }
            let (lo, _) = span(tile);
            let (_, hi) = span(end);
            if !self.query(lo, hi, host, offset, first, out) {
                out.truncate(mark);
                tiles.unwatchable = true;
                return false;
            }
            tile = end + 1;
        }
        for tile in first..=last {
            let count = self.counts[(tile - first) as usize];
            let heat = &mut tiles.heat[tile as usize];
            let (lo, hi) = span(tile);
            let whole = match *heat {
                Heat::Hot(age) if age < PROBE_AFTER => {
                    *heat = Heat::Hot(age + 1);
                    true
                }
                Heat::Hot(_) => {
                    *heat = Heat::Probing;
                    true
                }
                Heat::Probing if count >= DENSE_PAGES => {
                    *heat = Heat::Hot(0);
                    true
                }
                Heat::Probing => {
                    *heat = Heat::Cold(0);
                    false
                }
                Heat::Cold(streak) if count >= DENSE_PAGES => {
                    *heat = if streak + 1 >= DENSE_STREAK {
                        Heat::Hot(0)
                    } else {
                        Heat::Cold(streak + 1)
                    };
                    false
                }
                Heat::Cold(_) => {
                    *heat = Heat::Cold(0);
                    false
                }
            };
            if whole {
                out.push((lo, hi));
            }
        }
        true
    }

    /// Queries `lo..hi`, appending the written pages as runs and counting them per tile. The system walks every
    /// resident page it is asked about (13 ns each, 8 ms for 2.5 GiB), and threads do not speed that up: the
    /// calls are serialised inside the kernel.
    fn query(
        &mut self,
        lo: u64,
        hi: u64,
        host: u64,
        offset: u64,
        first_tile: u64,
        out: &mut Vec<(u64, u64)>,
    ) -> bool {
        let Some(pages) = fetch(lo, hi) else {
            return false;
        };
        let mut run: Option<(u64, u64)> = None;
        for page in pages {
            let page = page as u64;
            let tile = (page - host + offset) / TILE;
            self.counts[(tile - first_tile) as usize] += 1;
            match &mut run {
                Some((_, end)) if *end == page => *end = page + WATCH_PAGE,
                _ => {
                    if let Some(done) = run.take() {
                        out.push(done);
                    }
                    run = Some((page, page + WATCH_PAGE));
                }
            }
        }
        if let Some(done) = run {
            out.push(done);
        }
        true
    }
}

/// The pages written in `lo..hi`, queried in calls of at most `QUERY_PAGES` pages; `None` where the system cannot
/// say.
fn fetch(lo: u64, hi: u64) -> Option<Vec<usize>> {
    let mut found = Vec::new();
    let mut window = Vec::new();
    let mut at = lo;
    while at < hi {
        let part = (QUERY_PAGES * WATCH_PAGE).min(hi - at);
        window.resize((part / WATCH_PAGE) as usize, 0);
        let count = write_watch_call(at, part, &mut window)?;
        found.extend_from_slice(&window[..count]);
        at += part;
    }
    Some(found)
}

/// `WRITE_WATCH_FLAG_RESET`, which windows-sys does not carry.
#[cfg(windows)]
const WRITE_WATCH_FLAG_RESET: u32 = 1;

/// The pages written in `va..va + len` since the last call, which are reset, stored in `pages`; their number.
/// `None` where the system cannot say: the region was not reserved with `MEM_WRITE_WATCH`.
#[cfg(windows)]
pub(super) fn write_watch_call(va: u64, len: u64, pages: &mut [usize]) -> Option<usize> {
    use windows_sys::Win32::System::Memory::GetWriteWatch;
    let mut count = pages.len();
    let mut granularity = 0u32;
    // SAFETY: `pages` has room for `count` entries; the call only reads the system's write-watch table and
    // resets the pages it reports.
    let status = unsafe {
        GetWriteWatch(
            WRITE_WATCH_FLAG_RESET,
            va as usize as *const c_void,
            len as usize,
            pages.as_mut_ptr().cast::<*mut c_void>(),
            &mut count,
            &mut granularity,
        )
    };
    (status == 0).then_some(count)
}

#[cfg(not(windows))]
pub(super) fn write_watch_call(_va: u64, _len: u64, _pages: &mut [usize]) -> Option<usize> {
    None
}
