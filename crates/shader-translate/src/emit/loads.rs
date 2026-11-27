//! Runs of loads that share one condition, emitted behind a single branch instead of one branch each.

use shader_ir::{Block, Expr, Stmt};

use super::{EmitError, Emitter, ok};

impl Emitter {
    /// How many statements from `at` are loads under the same condition whose addresses were made before them.
    pub(super) fn load_run(&self, block: &Block, at: usize) -> usize {
        if self.heap.is_none() {
            return 0;
        }
        let Some(Stmt::Let(_, Expr::Load { when, .. })) = block.stmts.get(at) else {
            return 0;
        };
        let mut made = Vec::new();
        let mut count = 0;
        for stmt in &block.stmts[at..] {
            match stmt {
                Stmt::Let(tmp, Expr::Load { when: w, address })
                    if w == when
                        && !made.contains(address)
                        && w.is_none_or(|c| !made.contains(&c))
                        && (!self.pair || self.varying[*tmp as usize]) =>
                {
                    made.push(*tmp);
                    count += 1;
                }
                _ => break,
            }
        }
        count
    }

    /// Emits `stmts`, all loads from `load_run`, behind one guard per part.
    pub(super) fn load_batch(&mut self, stmts: &[Stmt]) -> Result<(), EmitError> {
        self.typed_cache = [None, None];
        let parts = if self.pair { 2 } else { 1 };
        let bool_ = self.t.bool_;
        for part in 0..parts {
            self.set_part(part);
            let mut condition = self.active()?;
            let mut addresses = Vec::new();
            for (n, stmt) in stmts.iter().enumerate() {
                let Stmt::Let(_, Expr::Load { when, address }) = stmt else {
                    unreachable!("a load run holds loads");
                };
                if n == 0
                    && let Some(when) = when
                {
                    let when = self.val(*when);
                    condition = ok(self.b.logical_and(bool_, None, condition, when))?;
                }
                addresses.push(self.val(*address));
            }
            let loaded = self.guarded_loads(condition, &addresses)?;
            for (stmt, id) in stmts.iter().zip(loaded) {
                let Stmt::Let(tmp, _) = stmt else {
                    unreachable!("a load run holds loads");
                };
                self.set_val(*tmp, id);
            }
        }
        self.set_part(0);
        Ok(())
    }
}
