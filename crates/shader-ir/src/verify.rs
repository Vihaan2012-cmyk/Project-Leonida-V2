use crate::ir::{Bin, Cmp, Expr, Function, Reg, Stmt, Terminator, Tmp, Un};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty {
    Bool,
    U32,
    U64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VerifyError {
    #[error("block {block}: t{tmp} is used before it is defined in this block")]
    Undefined { block: usize, tmp: Tmp },
    #[error("block {block}: t{tmp} is defined more than once")]
    Redefined { block: usize, tmp: Tmp },
    #[error("block {block}: t{tmp} is {found:?}, expected {expected:?}")]
    Type {
        block: usize,
        tmp: Tmp,
        expected: Ty,
        found: Ty,
    },
    #[error("block {block}: select arms t{a} and t{b} differ in type")]
    Arms { block: usize, a: Tmp, b: Tmp },
    #[error("block {block} branches to missing block {target}")]
    Target { block: usize, target: usize },
    #[error("t{0} is outside the function's {1} temporaries")]
    Range(Tmp, u32),
}

fn reg_type(reg: Reg) -> Ty {
    match reg {
        Reg::S(_) | Reg::V(_) | Reg::M0 => Ty::U32,
        Reg::Exec | Reg::Vcc => Ty::U64,
        Reg::Scc => Ty::Bool,
    }
}

fn unary(op: Un) -> (Ty, Ty) {
    match op {
        Un::Popcount64 => (Ty::U64, Ty::U32),
        Un::Not64 => (Ty::U64, Ty::U64),
        Un::Lo | Un::Hi => (Ty::U64, Ty::U32),
        Un::Zext => (Ty::U32, Ty::U64),
        Un::BoolToU32 => (Ty::Bool, Ty::U32),
        _ => (Ty::U32, Ty::U32),
    }
}

fn binary(op: Bin) -> (Ty, Ty, Ty) {
    match op {
        Bin::Add64
        | Bin::Sub64
        | Bin::And64
        | Bin::Or64
        | Bin::Xor64
        | Bin::Shl64
        | Bin::ShrU64 => (Ty::U64, Ty::U64, Ty::U64),
        Bin::Pack64 => (Ty::U32, Ty::U32, Ty::U64),
        Bin::LaneBit => (Ty::U64, Ty::U32, Ty::Bool),
        _ => (Ty::U32, Ty::U32, Ty::U32),
    }
}

struct Checker<'a> {
    types: Vec<Option<Ty>>,
    block: usize,
    defined_here: &'a mut Vec<bool>,
}

impl Checker<'_> {
    fn of(&self, tmp: Tmp) -> Result<Ty, VerifyError> {
        let index = tmp as usize;
        if index >= self.types.len() {
            return Err(VerifyError::Range(tmp, self.types.len() as u32));
        }
        match (self.defined_here[index], self.types[index]) {
            (true, Some(ty)) => Ok(ty),
            _ => Err(VerifyError::Undefined {
                block: self.block,
                tmp,
            }),
        }
    }

    fn expect(&self, tmp: Tmp, expected: Ty) -> Result<(), VerifyError> {
        let found = self.of(tmp)?;
        if found == expected {
            Ok(())
        } else {
            Err(VerifyError::Type {
                block: self.block,
                tmp,
                expected,
                found,
            })
        }
    }

    fn expr(&self, expr: &Expr) -> Result<Ty, VerifyError> {
        Ok(match expr {
            Expr::Const(_) | Expr::LaneId | Expr::FirstActiveLane => Ty::U32,
            Expr::Const64(_) | Expr::GetPair(_) | Expr::Active => Ty::U64,
            Expr::Get(reg) => reg_type(*reg),
            Expr::Un(op, a) => {
                let (input, output) = unary(*op);
                self.expect(*a, input)?;
                output
            }
            Expr::Bin(op, a, b) => {
                let (left, right, output) = binary(*op);
                self.expect(*a, left)?;
                self.expect(*b, right)?;
                output
            }
            Expr::Tri(_, a, b, c) => {
                for tmp in [a, b, c] {
                    self.expect(*tmp, Ty::U32)?;
                }
                Ty::U32
            }
            Expr::Cmp(op, a, b) => {
                let ty = if matches!(op, Cmp::Eq64 | Cmp::Ne64) {
                    Ty::U64
                } else {
                    Ty::U32
                };
                self.expect(*a, ty)?;
                self.expect(*b, ty)?;
                Ty::Bool
            }
            Expr::Select(c, a, b) => {
                self.expect(*c, Ty::Bool)?;
                let ty = self.of(*a)?;
                if self.of(*b)? != ty {
                    return Err(VerifyError::Arms {
                        block: self.block,
                        a: *a,
                        b: *b,
                    });
                }
                ty
            }
            Expr::Ballot(a) => {
                self.expect(*a, Ty::Bool)?;
                Ty::U64
            }
            Expr::ReadLane(source, lane) => {
                self.expect(*source, Ty::U32)?;
                self.expect(*lane, Ty::U32)?;
                Ty::U32
            }
            Expr::LdsLoad(address) => {
                self.expect(*address, Ty::U32)?;
                Ty::U32
            }
            Expr::Load { when, address } => {
                if let Some(when) = when {
                    self.expect(*when, Ty::Bool)?;
                }
                self.expect(*address, Ty::U64)?;
                Ty::U32
            }
            Expr::TypedLoad {
                format,
                address,
                in_range,
                select,
                ..
            } => {
                self.expect(*format, Ty::U32)?;
                self.expect(*address, Ty::U64)?;
                self.expect(*in_range, Ty::Bool)?;
                if let Some(select) = select {
                    self.expect(*select, Ty::U32)?;
                }
                Ty::U32
            }
            Expr::ScalarLoad(address) => {
                self.expect(*address, Ty::U64)?;
                Ty::U32
            }
        })
    }

    fn stmt(&mut self, stmt: &Stmt) -> Result<(), VerifyError> {
        match stmt {
            Stmt::Let(tmp, expr) => {
                let ty = self.expr(expr)?;
                let index = *tmp as usize;
                if index >= self.types.len() {
                    return Err(VerifyError::Range(*tmp, self.types.len() as u32));
                }
                if self.types[index].is_some() {
                    return Err(VerifyError::Redefined {
                        block: self.block,
                        tmp: *tmp,
                    });
                }
                self.types[index] = Some(ty);
                self.defined_here[index] = true;
            }
            Stmt::Set(reg, tmp) => self.expect(*tmp, reg_type(*reg))?,
            Stmt::SetPair(_, tmp) => self.expect(*tmp, Ty::U64)?,
            Stmt::SetLane(_, tmp) => self.expect(*tmp, Ty::U32)?,
            Stmt::WriteLane(_, lane, value) => {
                self.expect(*lane, Ty::U32)?;
                self.expect(*value, Ty::U32)?;
            }
            Stmt::LdsStore(address, value) => {
                self.expect(*address, Ty::U32)?;
                self.expect(*value, Ty::U32)?;
            }
            Stmt::Export { values, .. } => {
                for value in values.iter().flatten() {
                    self.expect(*value, Ty::U32)?;
                }
            }
            Stmt::Message(_, m0) => self.expect(*m0, Ty::U32)?,
            Stmt::Store {
                when,
                address,
                value,
            } => {
                if let Some(when) = when {
                    self.expect(*when, Ty::Bool)?;
                }
                self.expect(*address, Ty::U64)?;
                self.expect(*value, Ty::U32)?;
            }
        }
        Ok(())
    }
}

impl Function {
    pub fn verify(&self) -> Result<Vec<Ty>, VerifyError> {
        let mut defined_here = vec![false; self.temps as usize];
        let mut checker = Checker {
            types: vec![None; self.temps as usize],
            block: 0,
            defined_here: &mut defined_here,
        };
        for (id, block) in self.blocks.iter().enumerate() {
            checker.block = id;
            checker.defined_here.iter_mut().for_each(|d| *d = false);
            for stmt in &block.stmts {
                checker.stmt(stmt)?;
            }
            let targets = match block.terminator {
                Terminator::Jump(target) => vec![target],
                Terminator::Branch(condition, taken, fallthrough) => {
                    checker.expect(condition, Ty::Bool)?;
                    vec![taken, fallthrough]
                }
                Terminator::End => Vec::new(),
            };
            if let Some(target) = targets.into_iter().find(|t| *t >= self.blocks.len()) {
                return Err(VerifyError::Target { block: id, target });
            }
        }
        Ok(checker
            .types
            .into_iter()
            .map(|t| t.unwrap_or(Ty::U32))
            .collect())
    }
}
