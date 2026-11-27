//! Control flow as blocks run in order, each guarded by whether a branch reached it, with a structured loop
//! around each span of blocks that a branch goes back to the start of. Drivers compile this far faster than
//! a loop over a switch of every block.

use std::collections::{BTreeMap, HashMap};

use rspirv::dr::Builder;
use rspirv::spirv::{LoopControl, StorageClass, Word};
use shader_ir::{Function, Terminator};

use super::{EmitError, Emitter, ITERATION_LIMIT, ok};

pub(super) struct Flow {
    reached: Vec<Word>,
    /// Per loop head: whether a block in its loop branched back to it this time round.
    again: HashMap<usize, Word>,
    loops: Vec<(usize, usize)>,
}

fn targets(terminator: &Terminator) -> Vec<usize> {
    match terminator {
        Terminator::Jump(target) => vec![*target],
        Terminator::Branch(_, taken, fallthrough) => vec![*taken, *fallthrough],
        Terminator::End => Vec::new(),
    }
}

/// The loops: each spans from a block some later block branches back to, through the last such block.
/// `None` when two loops share blocks without one lying inside the other.
fn loops(function: &Function) -> Option<Vec<(usize, usize)>> {
    let mut ends: BTreeMap<usize, usize> = BTreeMap::new();
    for (id, block) in function.blocks.iter().enumerate() {
        for target in targets(&block.terminator) {
            if target <= id {
                let end = ends.entry(target).or_insert(id);
                *end = (*end).max(id);
            }
        }
    }
    let loops: Vec<(usize, usize)> = ends.into_iter().collect();
    for (i, outer) in loops.iter().enumerate() {
        for inner in &loops[i + 1..] {
            if inner.0 <= outer.1 && inner.1 > outer.1 {
                return None;
            }
        }
    }
    Some(loops)
}

pub(super) fn flow(b: &mut Builder, function: &Function, ptr_bool: Word) -> Option<Flow> {
    let loops = loops(function)?;
    let mut flag = || b.variable(ptr_bool, None, StorageClass::Function, None);
    let reached = function.blocks.iter().map(|_| flag()).collect();
    let again = loops.iter().map(|(head, _)| (*head, flag())).collect();
    Some(Flow {
        reached,
        again,
        loops,
    })
}

impl Emitter {
    pub(super) fn structured(
        &mut self,
        function: &Function,
        flow: &Flow,
        iterations: Word,
    ) -> Result<(), EmitError> {
        let (yes, no) = (self.boolc(true), self.boolc(false));
        for (id, flag) in flow.reached.iter().enumerate() {
            self.store(*flag, if id == 0 { yes } else { no })?;
        }
        let zero = self.u32c(0);
        self.store(iterations, zero)?;
        self.span(function, flow, iterations, 0..function.blocks.len(), None)
    }

    fn span(
        &mut self,
        function: &Function,
        flow: &Flow,
        iterations: Word,
        blocks: std::ops::Range<usize>,
        inside: Option<(usize, usize)>,
    ) -> Result<(), EmitError> {
        let mut at = blocks.start;
        while at < blocks.end {
            let found = flow
                .loops
                .iter()
                .find(|span| span.0 == at && Some(**span) != inside);
            match found {
                Some(&(head, end)) => {
                    self.loop_span(function, flow, iterations, head, end)?;
                    at = end + 1;
                }
                None => {
                    self.guarded_block(function, flow, at)?;
                    at += 1;
                }
            }
        }
        Ok(())
    }

    fn guarded_block(
        &mut self,
        function: &Function,
        flow: &Flow,
        index: usize,
    ) -> Result<(), EmitError> {
        let bool_ = self.t.bool_;
        let yes = self.boolc(true);
        let block = &function.blocks[index];
        let here = ok(self.b.load(bool_, None, flow.reached[index], None, []))?;
        self.if_then(here, |e| {
            e.block_stmts(index, block)?;
            let mark = |e: &mut Self, target: usize, when: Word| {
                let flag = if target > index {
                    flow.reached.get(target)
                } else {
                    flow.again.get(&target)
                };
                let Some(flag) = flag else {
                    return Ok(());
                };
                let old = ok(e.b.load(bool_, None, *flag, None, []))?;
                let new = ok(e.b.logical_or(bool_, None, old, when))?;
                e.store(*flag, new)
            };
            match block.terminator {
                Terminator::Jump(target) => mark(e, target, yes),
                Terminator::Branch(condition, taken, fallthrough) => {
                    let condition = e.val(condition);
                    let other = ok(e.b.logical_not(bool_, None, condition))?;
                    mark(e, taken, condition)?;
                    mark(e, fallthrough, other)
                }
                Terminator::End => Ok(()),
            }
        })
    }

    /// One loop: its blocks run in order, then again from the head while one of them branched back to it.
    fn loop_span(
        &mut self,
        function: &Function,
        flow: &Flow,
        iterations: Word,
        head: usize,
        end: usize,
    ) -> Result<(), EmitError> {
        let (u32_, bool_) = (self.t.u32_, self.t.bool_);
        let no = self.boolc(false);
        let again = flow.again[&head];
        self.store(again, no)?;
        let (header, body, next, merge) = (self.b.id(), self.b.id(), self.b.id(), self.b.id());
        ok(self.b.branch(header))?;
        self.begin(header)?;
        ok(self.b.loop_merge(merge, next, LoopControl::NONE, []))?;
        ok(self.b.branch(body))?;
        self.begin(body)?;
        self.span(function, flow, iterations, head..end + 1, Some((head, end)))?;
        ok(self.b.branch(next))?;
        self.begin(next)?;
        let go = ok(self.b.load(bool_, None, again, None, []))?;
        for (offset, flag) in flow.reached[head..=end].iter().enumerate() {
            self.store(*flag, if offset == 0 { go } else { no })?;
        }
        self.store(again, no)?;
        let count = self.load(iterations)?;
        let one = self.u32c(1);
        let count = ok(self.b.i_add(u32_, None, count, one))?;
        self.store(iterations, count)?;
        let limit = self.u32c(ITERATION_LIMIT);
        let room = ok(self.b.u_less_than(bool_, None, count, limit))?;
        let repeat = ok(self.b.logical_and(bool_, None, go, room))?;
        ok(self.b.branch_conditional(repeat, header, merge, []))?;
        self.begin(merge)
    }
}
