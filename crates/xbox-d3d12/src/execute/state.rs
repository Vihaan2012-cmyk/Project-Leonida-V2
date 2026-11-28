//! Shadow copies of what a command buffer holds, so a state set that changes nothing records nothing.

use std::ops::Range;

use super::command::VertexBufferView;

/// What a command buffer's push-constant range holds. A draw pushes only the words that differ from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushShadow<const N: usize> {
    words: [u32; N],
    valid: bool,
}

impl<const N: usize> Default for PushShadow<N> {
    fn default() -> Self {
        PushShadow {
            words: [0; N],
            valid: false,
        }
    }
}

impl<const N: usize> PushShadow<N> {
    /// The word range to push so the command buffer holds `block`, which is then what the shadow holds.
    /// `None` when it already does. A fresh command buffer holds nothing defined, so the first block goes whole.
    pub fn update(&mut self, block: &[u32; N]) -> Option<Range<usize>> {
        if !self.valid {
            self.valid = true;
            self.words = *block;
            return Some(0..N);
        }
        let first = self.words.iter().zip(block).position(|(a, b)| a != b)?;
        let last = self
            .words
            .iter()
            .zip(block)
            .rposition(|(a, b)| a != b)
            .unwrap_or(first);
        self.words = *block;
        Some(first..last + 1)
    }
}

/// D3D12 has 32 input-assembler slots.
pub const VERTEX_SLOTS: usize = 32;

/// The vertex buffers a list set, and which of them the command buffer has bound for the current pipeline.
///
/// A binding's stride is dynamic state of the pipeline, so a binding stays valid across a pipeline switch only
/// for slots both pipelines read, and a switch to a pipeline without vertex input drops them all.
#[derive(Debug, Clone)]
pub struct VertexBinder {
    set: [Option<VertexBufferView>; VERTEX_SLOTS],
    applied: [Option<VertexBufferView>; VERTEX_SLOTS],
    /// The slots the bound pipeline reads.
    mask: u32,
    /// The slots the list has set.
    set_mask: u32,
    /// The slots set to something other than what is bound.
    dirty: u32,
}

impl Default for VertexBinder {
    fn default() -> Self {
        VertexBinder {
            set: [None; VERTEX_SLOTS],
            applied: [None; VERTEX_SLOTS],
            mask: 0,
            set_mask: 0,
            dirty: 0,
        }
    }
}

impl VertexBinder {
    /// `IASetVertexBuffers`. Slots past the 32nd are dropped; the return says whether any were.
    pub fn set(&mut self, start: u32, views: &[VertexBufferView]) -> bool {
        let start = start as usize;
        let room = VERTEX_SLOTS.saturating_sub(start);
        for (k, view) in views.iter().take(room).enumerate() {
            let slot = start + k;
            self.set[slot] = Some(*view);
            self.set_mask |= 1 << slot;
            if self.applied[slot] == self.set[slot] {
                self.dirty &= !(1 << slot);
            } else {
                self.dirty |= 1 << slot;
            }
        }
        views.len() > room
    }

    /// A pipeline reading the slots in `mask` was bound.
    pub fn pipeline_bound(&mut self, mask: u32) {
        let lost = self.mask & !mask;
        for slot in 0..VERTEX_SLOTS {
            if lost & (1 << slot) != 0 {
                self.applied[slot] = None;
                self.dirty |= self.set_mask & (1 << slot);
            }
        }
        self.mask = mask;
    }

    /// Slots the pipeline reads that the list never set.
    pub fn unset(&self) -> impl Iterator<Item = u32> + '_ {
        let missing = self.mask & !self.set_mask;
        (0..VERTEX_SLOTS as u32).filter(move |&s| missing & (1 << s) != 0)
    }

    /// Whether a slot the pipeline reads has not been set.
    pub fn any_unset(&self) -> bool {
        self.mask & !self.set_mask != 0
    }

    /// The command buffer binds nothing any more (a new one): every slot the list set is to be bound again.
    pub fn invalidate(&mut self) {
        self.applied = [None; VERTEX_SLOTS];
        self.dirty = self.set_mask;
    }

    /// Whether a bind call is due.
    pub fn any_pending(&self) -> bool {
        self.dirty & self.mask != 0
    }

    /// The next run of consecutive slots at or after `from` that need a bind call, as `(first, views)`. The
    /// caller binds the views and calls `applied` for them.
    pub fn next_run(&self, from: u32) -> Option<(u32, &[Option<VertexBufferView>])> {
        let wanted = (self.dirty & self.mask) >> from.min(31) << from.min(31);
        if wanted == 0 || from >= VERTEX_SLOTS as u32 {
            return None;
        }
        let first = wanted.trailing_zeros();
        let count = (!(wanted >> first)).trailing_zeros();
        Some((first, &self.set[first as usize..(first + count) as usize]))
    }

    /// The run `first..first + count` was bound.
    pub fn applied(&mut self, first: u32, count: usize) {
        let first = first as usize;
        self.applied[first..first + count].copy_from_slice(&self.set[first..first + count]);
        for slot in first..first + count {
            self.dirty &= !(1 << slot);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(va: u64) -> VertexBufferView {
        VertexBufferView {
            va,
            size: 64,
            stride: 16,
        }
    }

    #[test]
    fn the_first_push_is_whole_and_an_identical_block_pushes_nothing() {
        let mut shadow = PushShadow::<8>::default();
        let mut block = [1, 2, 3, 4, 5, 6, 7, 8];
        assert_eq!(shadow.update(&block), Some(0..8));
        assert_eq!(shadow.update(&block), None);
        block[2] = 9;
        assert_eq!(shadow.update(&block), Some(2..3));
        assert_eq!(shadow.update(&block), None);
    }

    #[test]
    fn a_changed_block_pushes_from_its_first_to_its_last_changed_word() {
        let mut shadow = PushShadow::<8>::default();
        let mut block = [0; 8];
        shadow.update(&block);
        block[1] = 5;
        block[6] = 5;
        assert_eq!(shadow.update(&block), Some(1..7));
        block[6] = 0;
        assert_eq!(shadow.update(&block), Some(6..7));
        block[0] = 1;
        assert_eq!(shadow.update(&block), Some(0..1));
    }

    #[test]
    fn a_block_that_returns_to_an_earlier_value_is_pushed_again() {
        let mut shadow = PushShadow::<4>::default();
        let a = [1, 1, 1, 1];
        let b = [2, 1, 1, 1];
        shadow.update(&a);
        assert_eq!(shadow.update(&b), Some(0..1));
        assert_eq!(shadow.update(&a), Some(0..1));
    }

    fn bind_all(binder: &mut VertexBinder) -> Vec<(u32, usize)> {
        let mut runs = Vec::new();
        let mut from = 0;
        while let Some((first, views)) = binder.next_run(from) {
            let count = views.len();
            runs.push((first, count));
            binder.applied(first, count);
            from = first + count as u32;
        }
        runs
    }

    #[test]
    fn consecutive_slots_bind_in_one_run_and_a_repeat_binds_nothing() {
        let mut binder = VertexBinder::default();
        binder.pipeline_bound(0b11);
        binder.set(0, &[view(0x100), view(0x200)]);
        assert_eq!(bind_all(&mut binder), vec![(0, 2)]);
        assert_eq!(bind_all(&mut binder), vec![]);
        binder.set(0, &[view(0x100), view(0x200)]);
        assert_eq!(bind_all(&mut binder), vec![]);
    }

    #[test]
    fn a_changed_slot_rebinds_alone() {
        let mut binder = VertexBinder::default();
        binder.pipeline_bound(0b111);
        binder.set(0, &[view(1), view(2), view(3)]);
        bind_all(&mut binder);
        binder.set(1, &[view(9)]);
        assert_eq!(bind_all(&mut binder), vec![(1, 1)]);
        binder.set(0, &[view(5)]);
        binder.set(2, &[view(6)]);
        assert_eq!(bind_all(&mut binder), vec![(0, 1), (2, 1)]);
    }

    #[test]
    fn a_stride_change_alone_rebinds() {
        let mut binder = VertexBinder::default();
        binder.pipeline_bound(1);
        binder.set(0, &[view(1)]);
        bind_all(&mut binder);
        binder.set(
            0,
            &[VertexBufferView {
                stride: 32,
                ..view(1)
            }],
        );
        assert_eq!(bind_all(&mut binder), vec![(0, 1)]);
    }

    #[test]
    fn slots_the_pipeline_does_not_read_are_not_bound() {
        let mut binder = VertexBinder::default();
        binder.pipeline_bound(0b101);
        binder.set(0, &[view(1), view(2), view(3)]);
        assert_eq!(bind_all(&mut binder), vec![(0, 1), (2, 1)]);
    }

    #[test]
    fn a_pipeline_switch_keeps_bindings_of_shared_slots_and_drops_the_rest() {
        let mut binder = VertexBinder::default();
        binder.pipeline_bound(0b11);
        binder.set(0, &[view(1), view(2)]);
        bind_all(&mut binder);
        binder.pipeline_bound(0b01);
        assert_eq!(bind_all(&mut binder), vec![]);
        binder.pipeline_bound(0b11);
        assert_eq!(bind_all(&mut binder), vec![(1, 1)]);
        binder.pipeline_bound(0);
        binder.pipeline_bound(0b11);
        assert_eq!(bind_all(&mut binder), vec![(0, 2)]);
    }

    #[test]
    fn invalidated_bindings_are_all_bound_again() {
        let mut binder = VertexBinder::default();
        binder.pipeline_bound(0b101);
        binder.set(0, &[view(1), view(2), view(3)]);
        bind_all(&mut binder);
        assert_eq!(bind_all(&mut binder), vec![]);
        binder.invalidate();
        assert_eq!(bind_all(&mut binder), vec![(0, 1), (2, 1)]);
    }

    #[test]
    fn unset_slots_the_pipeline_reads_are_reported_and_extra_views_are_refused() {
        let mut binder = VertexBinder::default();
        binder.pipeline_bound(0b110);
        binder.set(1, &[view(1)]);
        assert_eq!(binder.unset().collect::<Vec<_>>(), vec![2]);
        assert!(binder.set(31, &[view(1), view(2)]));
        assert!(!binder.set(30, &[view(1), view(2)]));
    }
}
