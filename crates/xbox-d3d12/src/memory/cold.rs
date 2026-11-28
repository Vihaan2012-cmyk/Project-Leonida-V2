//! Which of the game's memory the GPU reads through the window, and which only through other objects.
//!
//! A texture's tiled data sits in the game's memory, and the layer detiles it into a `VkImage` that shaders sample.
//! Once that is done the window's device-local copy of the texture's memory is dead weight: nothing reads it. The
//! resource layer, which knows what each range holds, says so:
//!
//! - **cold** (`mark_cold`): the GPU reads the range only through images made from it;
//! - **hot** (`mark_hot`): a buffer, a view over an address or a write of the GPU's lives there, so it stays
//!   device-local. Hot wins over cold and stays until the memory is unmapped.
//!
//! `release_cold` moves cold islands out of device-local memory into the spill tier, rebuilding what lies around
//! them, and `promote_spilled` mirrors hot runs again first when room appears. A released range is the game's own
//! host memory bound into the window, so it stays correct whatever touches it: the CPU writes it, the GPU reads
//! it slowly if something does. Writes the title makes to a cold range are reported by `take_cold_writes`, so the
//! images made from it can be made again: the window needs nothing, but nothing else tells the resource layer.
//!
//! Nothing here runs while the GPU may use the window: `release_cold` and `spill_range` are for the moments
//! between submissions.

use std::time::{Duration, Instant};

use super::dirty::{RangeSet, WATCH_PAGE, fetch};
use super::*;

/// The smallest island of cold memory worth giving up a memory object for: a smaller one costs more objects than
/// it frees.
const RELEASE_MIN: u64 = 4 << 20;

/// The most memory (in whole objects) one `release_cold` rebuilds, so a frame's work stays bounded.
const RELEASE_PER_CALL: u64 = 512 << 20;

/// Memory objects kept free of what releasing makes.
const ALLOCATION_HEADROOM: u64 = 256;

pub(super) struct Temperature {
    cold: RangeSet,
    hot: RangeSet,
    /// The part of `cold` already held out of device-local memory.
    released: RangeSet,
    /// Hot claims over released memory, not yet taken back.
    wake: RangeSet,
    /// Cold ranges the CPU wrote since `take_cold_writes`.
    written: RangeSet,
    due: bool,
    release_min: u64,
    release_gap: Duration,
    watch_gap: Duration,
    released_at: Option<Instant>,
    watched_at: Option<Instant>,
}

impl Default for Temperature {
    fn default() -> Temperature {
        Temperature {
            cold: RangeSet::default(),
            hot: RangeSet::default(),
            released: RangeSet::default(),
            wake: RangeSet::default(),
            written: RangeSet::default(),
            due: false,
            release_min: RELEASE_MIN,
            release_gap: Duration::from_millis(100),
            watch_gap: Duration::from_secs(1),
            released_at: None,
            watched_at: None,
        }
    }
}

impl Temperature {
    pub(super) fn forget(&mut self, lo: u64, hi: u64) {
        for set in [
            &mut self.cold,
            &mut self.hot,
            &mut self.released,
            &mut self.wake,
            &mut self.written,
        ] {
            set.remove(lo, hi);
        }
    }

    pub(super) fn is_hot(&self, lo: u64, hi: u64) -> bool {
        !self.hot.pieces(lo, hi).is_empty()
    }

    pub(super) fn note_written(&mut self, lo: u64, hi: u64) {
        for (a, b) in self.cold.pieces(lo, hi) {
            self.written.add(a, b);
        }
    }
}

fn covers(set: &RangeSet, lo: u64, hi: u64) -> bool {
    set.pieces(lo, hi) == [(lo, hi)]
}

/// What the game's memory holds, in bytes, mirrored in device-local memory and spilled to system memory.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Split {
    pub mirrored: u64,
    pub spilled: u64,
}

/// The game's memory by what the resource layer says it holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WindowClasses {
    /// Texture memory already detiled into images.
    pub cold: Split,
    /// Buffers, views and what the GPU writes.
    pub hot: Split,
    /// Everything nothing was said of.
    pub other: Split,
}

impl WindowClasses {
    pub fn line(&self) -> String {
        let mib = |s: Split| format!("{} / {}", s.mirrored >> 20, s.spilled >> 20);
        format!(
            "vram: game memory by class, MiB mirrored / spilled: detiled textures {}, buffers and targets {}, \
             unclassified {}",
            mib(self.cold),
            mib(self.hot),
            mib(self.other)
        )
    }
}

/// A part of one memory object that becomes a memory object of its own.
struct Piece {
    va: u64,
    len: u64,
    host: u64,
    pin: bool,
}

/// What `repiece` did.
#[derive(Default)]
struct Repieced {
    /// Device-local bytes given back.
    freed: u64,
    /// Bytes of memory objects that were copied back and rebuilt: the work done.
    rebuilt: u64,
}

impl AddressSpace {
    fn granule(&self) -> u64 {
        PAGE.max(self.support.host_alignment)
    }

    /// Test hook: the smallest cold island released, and how long `release_cold` and the write watch wait between
    /// runs.
    pub fn tune_release(&mut self, min_island: u64, gap: Duration) {
        self.temp.release_min = min_island;
        self.temp.release_gap = gap;
        self.temp.watch_gap = gap;
    }

    /// The memory in `va..va + len` is read by the GPU only through objects made from it (textures detiled into
    /// images). Parts already called hot stay hot.
    pub fn mark_cold(&mut self, va: u64, len: u64) {
        if self.backing != Backing::Mirrored || len == 0 {
            return;
        }
        let hi = va + len;
        let t = &mut self.temp;
        let mut fresh = RangeSet::default();
        fresh.add(va, hi);
        for (a, b) in t.hot.pieces(va, hi) {
            fresh.remove(a, b);
        }
        let mut added = Vec::new();
        for (a, b) in fresh.take() {
            if !covers(&t.cold, a, b) {
                t.cold.add(a, b);
                t.due = true;
                added.push((a, b));
            }
        }
        for (a, b) in added {
            self.reset_watch(a, b);
        }
    }

    /// Starts the write watch afresh over `lo..hi`, so what it reports from now on is what the title writes after
    /// the detile that made the range cold. A mirrored range keeps what the watch had seen as dirty for the upload.
    fn reset_watch(&mut self, lo: u64, hi: u64) {
        let mut seen = Vec::new();
        for (start, extent, from, to) in self.extents.overlapping(lo, hi) {
            let Some(memory) = self.memories.get(&extent.memory) else {
                continue;
            };
            if memory.policy != SyncPolicy::WriteWatch {
                continue;
            }
            let host = memory.host + extent.memory_offset + (from - start);
            if let Some(pages) = fetch(host, host + (to - from))
                && memory.mirror.is_some()
            {
                seen.extend(pages.iter().map(|&p| {
                    let at = from + (p as u64 - host);
                    (at, at + WATCH_PAGE)
                }));
            }
        }
        for (a, b) in seen {
            self.sync.dirty.add(a, b);
        }
    }

    /// The GPU reads or writes `va..va + len` through the window: a buffer is there, a view names it, or the GPU
    /// wrote it. It is taken out of the cold range and, if it was released, mirrored again (first, by
    /// `promote_spilled`) at the next `release_cold`.
    pub fn mark_hot(&mut self, va: u64, len: u64) {
        if self.backing != Backing::Mirrored || len == 0 {
            return;
        }
        let hi = va + len;
        let t = &mut self.temp;
        if covers(&t.hot, va, hi) {
            return;
        }
        t.hot.add(va, hi);
        t.cold.remove(va, hi);
        t.written.remove(va, hi);
        if !t.released.pieces(va, hi).is_empty() {
            t.released.remove(va, hi);
            t.wake.add(va, hi);
            t.due = true;
        }
    }

    /// Where the memory at `va` lives, if anything is bound there.
    pub fn placement_at(&self, va: u64) -> Option<Placement> {
        let (_, extent, _, _) = *self.extents.overlapping(va, va + 1).first()?;
        let memory = self.memories.get(&extent.memory)?;
        Some(if memory.is_spilled() {
            Placement::Spilled
        } else {
            Placement::DeviceLocal
        })
    }

    /// The bytes of the game's memory held out of device-local memory because they are cold.
    pub fn released_bytes(&self) -> u64 {
        self.memories
            .values()
            .filter(|m| m.pinned && m.is_spilled())
            .map(|m| m.live)
            .sum()
    }

    /// Mirrored and spilled bytes of the game's memory by class.
    pub fn window_classes(&self) -> WindowClasses {
        let mut classes = WindowClasses::default();
        let t = &self.temp;
        for (&va, extent) in &self.extents.0 {
            let spilled = self
                .memories
                .get(&extent.memory)
                .is_some_and(Memory::is_spilled);
            let end = va + extent.len;
            let sum =
                |set: &RangeSet| -> u64 { set.pieces(va, end).iter().map(|(a, b)| b - a).sum() };
            let (cold, hot) = (sum(&t.cold), sum(&t.hot));
            for (class, bytes) in [
                (&mut classes.cold, cold),
                (&mut classes.hot, hot),
                (&mut classes.other, extent.len - cold - hot),
            ] {
                if spilled {
                    class.spilled += bytes;
                } else {
                    class.mirrored += bytes;
                }
            }
        }
        classes
    }

    /// Cold ranges the CPU wrote since the last call, found by `mark_dirty` (the host's own writes), by the
    /// write watch while the memory is still mirrored, and, no more often than every second, by asking the system
    /// about released memory made with `SyncPolicy::WriteWatch`. The caller makes the images again.
    pub fn take_cold_writes(&mut self) -> Vec<(u64, u64)> {
        if self.backing != Backing::Mirrored {
            return Vec::new();
        }
        let now = Instant::now();
        let due = self
            .temp
            .watched_at
            .is_none_or(|t| now.duration_since(t) >= self.temp.watch_gap);
        if due && !self.temp.released.is_empty() {
            self.temp.watched_at = Some(now);
            self.scan_released();
        }
        self.temp.written.take()
    }

    fn scan_released(&mut self) {
        let mut found = Vec::new();
        for (&lo, &hi) in &self.temp.released.0 {
            for (start, extent, from, to) in self.extents.overlapping(lo, hi) {
                let Some(memory) = self.memories.get(&extent.memory) else {
                    continue;
                };
                if !memory.is_spilled() || memory.policy != SyncPolicy::WriteWatch {
                    continue;
                }
                let host = memory.host + extent.memory_offset + (from - start);
                if let Some(pages) = fetch(host, host + (to - from)) {
                    found.extend(pages.iter().map(|&p| {
                        let at = from + (p as u64 - host);
                        (at, at + WATCH_PAGE)
                    }));
                }
            }
        }
        for (a, b) in found {
            self.temp.written.add(a, b);
        }
    }

    /// Releases the cold islands, takes back what became hot and returns the device-local bytes freed. Call it
    /// between submissions, with no GPU work in flight, and `reclaim` after it: that frees the old objects and
    /// mirrors the runs that go back.
    pub fn release_cold(&mut self) -> Result<u64, MemoryError> {
        if !self.spill || self.backing != Backing::Mirrored || !self.temp.due {
            return Ok(0);
        }
        if self
            .temp
            .released_at
            .is_some_and(|t| t.elapsed() < self.temp.release_gap)
        {
            return Ok(0);
        }
        self.temp.released_at = Some(Instant::now());
        self.temp.due = false;
        let result = self.release_pass();
        if result.is_err() {
            self.temp.due = true;
        }
        result
    }

    fn release_pass(&mut self) -> Result<u64, MemoryError> {
        let mut changed = false;
        for (lo, hi) in self.temp.wake.take() {
            let g = self.granule();
            changed |= self.wake_range(align_down(lo, g), align_up(hi, g))?;
        }
        let mut group: Vec<(u64, u64)> = Vec::new();
        let mut seen: Vec<u64> = Vec::new();
        let mut work = 0;
        for (lo, hi) in self.cold_islands() {
            if work >= RELEASE_PER_CALL {
                self.temp.due = true;
                break;
            }
            for (_, extent, _, _) in self.extents.overlapping(lo, hi) {
                if !seen.contains(&extent.memory) {
                    seen.push(extent.memory);
                    work += self.memories.get(&extent.memory).map_or(0, |m| m.live);
                }
            }
            group.push((lo, hi));
        }
        let freed = self.spill_islands(&group)?.freed;
        for &(lo, hi) in &group {
            self.temp.released.add(lo, hi);
        }
        if freed > 0 {
            self.h.budget.note_released(freed);
        }
        if freed > 0 || changed {
            self.promote_more = true;
            self.h.budget.say(self.window_classes().line());
        }
        Ok(freed)
    }

    /// The cold ranges not yet released, inside the granule and above `LOW_LIMIT`, long enough to be worth it.
    fn cold_islands(&self) -> Vec<(u64, u64)> {
        let g = self.granule();
        let t = &self.temp;
        let mut out = Vec::new();
        for (&lo, &hi) in &t.cold.0 {
            let mut open = RangeSet::default();
            open.add(lo, hi);
            for (a, b) in t.released.pieces(lo, hi) {
                open.remove(a, b);
            }
            for (&a, &b) in &open.0 {
                let (a, b) = (align_up(a.max(LOW_LIMIT), g), align_down(b, g));
                if b > a && b - a >= t.release_min {
                    out.push((a, b));
                }
            }
        }
        out
    }

    /// Takes `va..va + len` out of device-local memory, whatever the memory objects under it look like: whole
    /// objects inside it move as they are, objects it cuts are split and the rest is mirrored again by
    /// `promote_spilled`. What the GPU wrote there is copied back first; no commands that use the window may be
    /// in flight. Returns the device-local bytes freed. It does nothing below `LOW_LIMIT` (descriptor heaps) or
    /// with the spill tier off.
    pub fn spill_range(&mut self, va: u64, len: u64) -> Result<u64, MemoryError> {
        if !self.spill {
            return Ok(0);
        }
        let g = self.granule();
        let (lo, hi) = (align_up(va.max(LOW_LIMIT), g), align_down(va + len, g));
        if lo >= hi {
            return Ok(0);
        }
        Ok(self.spill_islands(&[(lo, hi)])?.freed)
    }

    fn spill_islands(&mut self, islands: &[(u64, u64)]) -> Result<Repieced, MemoryError> {
        let mut ids: Vec<u64> = Vec::new();
        for &(lo, hi) in islands {
            for (_, extent, _, _) in self.extents.overlapping(lo, hi) {
                if !ids.contains(&extent.memory) {
                    ids.push(extent.memory);
                }
            }
        }
        let mut total = Repieced::default();
        for id in ids {
            if self.memories.len() as u64 + ALLOCATION_HEADROOM
                > u64::from(self.support.max_allocation_count)
            {
                break;
            }
            let done = self.repiece(id, islands, true)?;
            total.freed += done.freed;
            total.rebuilt += done.rebuilt;
        }
        Ok(total)
    }

    /// Hot claims over released memory: the claimed parts stop being held, the rest stays.
    fn wake_range(&mut self, lo: u64, hi: u64) -> Result<bool, MemoryError> {
        let mut ids: Vec<u64> = Vec::new();
        for (_, extent, _, _) in self.extents.overlapping(lo, hi) {
            if self.memories.get(&extent.memory).is_some_and(|m| m.pinned)
                && !ids.contains(&extent.memory)
            {
                ids.push(extent.memory);
            }
        }
        let mut changed = false;
        for id in ids {
            let pins: Vec<(u64, u64)> = self
                .extents
                .0
                .iter()
                .filter(|(_, e)| e.memory == id)
                .flat_map(|(&va, e)| {
                    let end = va + e.len;
                    [(va, end.min(lo)), (va.max(hi), end)]
                })
                .filter(|(a, b)| a < b)
                .collect();
            self.repiece(id, &pins, false)?;
            changed = true;
        }
        Ok(changed)
    }

    /// Rebinds memory object `id` as spilled pieces: host memory bound straight into the window, those inside
    /// `pins` held out of `promote_spilled` (and, with `keep`, those that were held before). A mirrored object's
    /// device-local copy is retired, and what the GPU wrote there copied back first.
    fn repiece(
        &mut self,
        id: u64,
        pins: &[(u64, u64)],
        keep: bool,
    ) -> Result<Repieced, MemoryError> {
        let Some(old) = self.memories.get(&id) else {
            return Ok(Repieced::default());
        };
        let (was_spilled, was_pinned, host0, policy) =
            (old.is_spilled(), old.pinned, old.host, old.policy);
        let extents: Vec<(u64, Extent)> = self
            .extents
            .0
            .iter()
            .filter(|(_, e)| e.memory == id)
            .map(|(&va, &e)| (va, e))
            .collect();
        let mut pieces: Vec<Piece> = Vec::new();
        for &(va, e) in &extents {
            let end = va + e.len;
            let host = |at: u64| host0 + e.memory_offset + (at - va);
            let mut at = va;
            let mut push = |from: u64, to: u64, inside: bool| {
                if from < to {
                    pieces.push(Piece {
                        va: from,
                        len: to - from,
                        host: host(from),
                        pin: inside || (keep && was_pinned),
                    });
                }
            };
            for &(a, b) in pins {
                let (a, b) = (a.max(va), b.min(end));
                if a >= b || b <= at {
                    continue;
                }
                let a = a.max(at);
                push(at, a, false);
                push(a, b, true);
                at = b;
            }
            push(at, end, false);
        }
        if was_spilled && pieces.len() == 1 && extents.len() == 1 {
            let pin = pieces[0].pin;
            if let Some(m) = self.memories.get_mut(&id) {
                m.pinned = pin;
            }
            if pin {
                self.sync.forget(pieces[0].va, pieces[0].va + pieces[0].len);
            }
            return Ok(Repieced::default());
        }
        let mut done = Repieced::default();
        if !was_spilled {
            for &(va, e) in &extents {
                self.read_back(va, e.len)?;
            }
            done.rebuilt = extents.iter().map(|(_, e)| e.len).sum();
        }
        // Importing pins the pages, which the write watch counts as a write: what the title wrote before is kept
        // and the slate wiped after.
        let watched = |p: &Piece| p.pin && policy == SyncPolicy::WriteWatch;
        let mut early: Vec<(u64, u64)> = Vec::new();
        for p in pieces.iter().filter(|p| watched(p)) {
            if let Some(pages) = fetch(p.host, p.host + p.len) {
                early.extend(pages.iter().map(|&page| {
                    let at = p.va + (page as u64 - p.host);
                    (at, at + WATCH_PAGE)
                }));
            }
        }
        let mut made: Vec<(vk::DeviceMemory, Grant)> = Vec::with_capacity(pieces.len());
        for p in &pieces {
            match self.import_spilled(p.host, p.len) {
                Ok((handle, _, grant)) => made.push((handle, grant)),
                Err(e) => {
                    for (handle, grant) in made {
                        // SAFETY: imported just now and never bound.
                        unsafe { self.h.device.free_memory(handle, None) };
                        self.h.budget.release(&grant);
                    }
                    return Err(e);
                }
            }
        }
        for (va, _) in &extents {
            self.extents.0.remove(va);
        }
        let old = self.memories.remove(&id).expect("found above");
        self.retired_memory.push(old.handle);
        self.retired_grants.extend(old.grant);
        if let Some(mirror) = old.mirror {
            self.retired_buffers.push(mirror.buffer);
            self.retired_memory.push(mirror.import);
        }
        for (p, (handle, grant)) in pieces.iter().zip(made) {
            let nid = self.next_memory;
            self.next_memory += 1;
            self.memories.insert(
                nid,
                Memory {
                    handle,
                    live: p.len,
                    mirror: None,
                    policy: old.policy,
                    host: p.host,
                    tiles: Tiles::default(),
                    grant: Some(grant),
                    pinned: p.pin,
                },
            );
            self.extents.insert(
                p.va,
                Extent {
                    len: p.len,
                    memory: nid,
                    memory_offset: 0,
                },
            );
            self.pending.push(Bind {
                offset: p.va - self.window.base,
                size: p.len,
                memory: handle,
                memory_offset: 0,
            });
            if p.pin {
                self.sync.forget(p.va, p.va + p.len);
                if !was_spilled {
                    done.freed += p.len;
                }
            }
        }
        self.commit()?;
        for p in pieces.iter().filter(|p| watched(p)) {
            let _ = fetch(p.host, p.host + p.len);
        }
        for (a, b) in early {
            self.temp.note_written(a, b);
        }
        Ok(done)
    }
}
