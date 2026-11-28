//! Control flow as blocks run in order, each guarded by whether a branch reached it, with a structured loop
//! around each span of blocks that a branch goes back to the start of. Drivers compile this far faster than
//! a loop over a switch of every block.

use std::collections::{BTreeMap, HashMap};

use rspirv::dr::Builder;
use rspirv::spirv::{StorageClass, Word};
use shader_ir::{Function, Terminator};

use super::{EmitError, Emitter, ITERATION_LIMIT, ok};

pub(super) struct Flow {
    reached: Vec<Word>,
    /// Per block some later block branches back to: whether one did so this time round.
    again: HashMap<usize, Word>,
    loops: Vec<Loop>,
}

/// A span of blocks that runs again while a branch goes back into it.
struct Loop {
    head: usize,
    end: usize,
    /// The blocks inside whose back branches belong to this loop and not to one nested in it.
    targets: Vec<usize>,
}

fn targets(terminator: &Terminator) -> Vec<usize> {
    match terminator {
        Terminator::Jump(target) => vec![*target],
        Terminator::Branch(_, taken, fallthrough) => vec![*taken, *fallthrough],
        Terminator::End => Vec::new(),
    }
}

/// The loops: each spans from a block some later block branches back to, through the last such block. Loops
/// that only partly overlap are joined into one, which runs again from whichever block was branched back to.
fn loops(function: &Function) -> Vec<Loop> {
    let mut back = Vec::new();
    let mut ends: BTreeMap<usize, usize> = BTreeMap::new();
    for (id, block) in function.blocks.iter().enumerate() {
        for target in targets(&block.terminator) {
            if target <= id {
                back.push((target, id));
                let end = ends.entry(target).or_insert(id);
                *end = (*end).max(id);
            }
        }
    }
    let mut spans: Vec<(usize, usize)> = ends.into_iter().collect();
    while let Some((outer, inner)) = partial_overlap(&spans) {
        spans[outer].1 = spans[inner].1;
        spans.remove(inner);
    }
    let mut loops: Vec<Loop> = spans
        .into_iter()
        .map(|(head, end)| Loop {
            head,
            end,
            targets: Vec::new(),
        })
        .collect();
    for (target, source) in back {
        let innermost = loops
            .iter_mut()
            .filter(|l| l.head <= target && source <= l.end)
            .min_by_key(|l| l.end - l.head)
            .expect("a back branch lies in the span of its target");
        if !innermost.targets.contains(&target) {
            innermost.targets.push(target);
        }
    }
    loops
}

/// Two spans (by index) where the second starts inside the first and ends after it.
fn partial_overlap(spans: &[(usize, usize)]) -> Option<(usize, usize)> {
    for (i, outer) in spans.iter().enumerate() {
        for (j, inner) in spans.iter().enumerate() {
            if outer.0 < inner.0 && inner.0 <= outer.1 && inner.1 > outer.1 {
                return Some((i, j));
            }
        }
    }
    None
}

pub(super) fn flow(b: &mut Builder, function: &Function, ptr_bool: Word) -> Flow {
    let loops = loops(function);
    let mut flag = || b.variable(ptr_bool, None, StorageClass::Function, None);
    let reached = function.blocks.iter().map(|_| flag()).collect();
    let again = loops
        .iter()
        .flat_map(|l| l.targets.iter().copied())
        .map(|target| (target, flag()))
        .collect();
    Flow {
        reached,
        again,
        loops,
    }
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
                .find(|l| l.head == at && Some((l.head, l.end)) != inside);
            match found {
                Some(found) => {
                    self.loop_span(function, flow, iterations, found)?;
                    at = found.end + 1;
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
        found: &Loop,
    ) -> Result<(), EmitError> {
        let (head, end) = (found.head, found.end);
        let (u32_, bool_) = (self.t.u32_, self.t.bool_);
        let no = self.boolc(false);
        for target in &found.targets {
            self.store(flow.again[target], no)?;
        }
        let (header, body, next, merge) = (self.b.id(), self.b.id(), self.b.id(), self.b.id());
        ok(self.b.branch(header))?;
        self.begin(header)?;
        self.looping(merge, next)?;
        ok(self.b.branch(body))?;
        self.begin(body)?;
        self.span(function, flow, iterations, head..end + 1, Some((head, end)))?;
        ok(self.b.branch(next))?;
        self.begin(next)?;
        let mut go = no;
        let mut restart = HashMap::new();
        for target in &found.targets {
            let flag = ok(self.b.load(bool_, None, flow.again[target], None, []))?;
            restart.insert(*target, flag);
            go = if go == no {
                flag
            } else {
                ok(self.b.logical_or(bool_, None, go, flag))?
            };
        }
        for (block, flag) in flow.reached[head..=end].iter().enumerate() {
            self.store(*flag, restart.get(&(head + block)).copied().unwrap_or(no))?;
        }
        for target in &found.targets {
            self.store(flow.again[target], no)?;
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use shader_ir::Block;

    fn function(terminators: Vec<Terminator>) -> Function {
        Function {
            wave64: false,
            blocks: terminators
                .into_iter()
                .enumerate()
                .map(|(pc, terminator)| Block {
                    pc,
                    stmts: Vec::new(),
                    terminator,
                })
                .collect(),
            temps: 1,
            ordered: false,
            upper_lanes: false,
        }
    }

    fn shape(loops: &[Loop]) -> Vec<(usize, usize, Vec<usize>)> {
        loops
            .iter()
            .map(|l| (l.head, l.end, l.targets.clone()))
            .collect()
    }

    #[test]
    fn two_branches_back_to_one_block_make_one_loop() {
        let function = function(vec![
            Terminator::Jump(1),
            Terminator::Jump(2),
            Terminator::Branch(0, 1, 3),
            Terminator::Branch(0, 1, 4),
            Terminator::End,
        ]);
        assert_eq!(shape(&loops(&function)), vec![(1, 3, vec![1])]);
    }

    #[test]
    fn partly_overlapping_loops_become_one_that_restarts_at_either_target() {
        let function = function(vec![
            Terminator::Jump(1),
            Terminator::Jump(2),
            Terminator::Branch(0, 1, 3),
            Terminator::Jump(4),
            Terminator::Branch(0, 2, 5),
            Terminator::End,
        ]);
        assert_eq!(shape(&loops(&function)), vec![(1, 4, vec![1, 2])]);
    }

    #[test]
    fn an_inner_loop_keeps_its_own_targets() {
        let function = function(vec![
            Terminator::Jump(1),
            Terminator::Jump(2),
            Terminator::Branch(0, 2, 3),
            Terminator::Branch(0, 1, 4),
            Terminator::End,
        ]);
        assert_eq!(
            shape(&loops(&function)),
            vec![(1, 3, vec![1]), (2, 2, vec![2])]
        );
    }
}
