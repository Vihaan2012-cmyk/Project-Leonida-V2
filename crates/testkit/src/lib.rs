mod memory;
pub mod random;
mod salu;
mod state;
mod valu;

use rdna::Inst;

pub use state::{Memory, Wave};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExecError {
    #[error("invalid instruction words in the program")]
    Invalid,
    #[error("unsupported instruction at 0x{pc:x}: {text}")]
    Unsupported { pc: usize, text: String },
    #[error("bad operand at 0x{pc:x}: {text}")]
    BadOperand { pc: usize, text: String },
    #[error("branch to 0x{target:x} is outside the program")]
    BadBranch { target: i64 },
    #[error("memory access at 0x{address:x} is outside the memory")]
    OutOfBounds { address: u64 },
    #[error("ran more than {0} instructions")]
    TooLong(usize),
}

pub type Result<T> = std::result::Result<T, ExecError>;

pub const STEP_LIMIT: usize = 1_000_000;

pub fn run(code: &[u8], wave: &mut Wave, memory: &mut Memory) -> Result<usize> {
    let decoded = rdna::decode_all(code, !wave.wave64, 0);
    if decoded.invalid > 0 {
        return Err(ExecError::Invalid);
    }
    let insts = decoded.insts;
    let mut index = 0;
    let mut steps = 0;
    while index < insts.len() {
        steps += 1;
        if steps > STEP_LIMIT {
            return Err(ExecError::TooLong(STEP_LIMIT));
        }
        let inst = &insts[index];
        match execute(inst, wave, memory)? {
            Flow::Next => index += 1,
            Flow::End => break,
            Flow::Jump(offset) => {
                let target = inst.pc as i64 + 4 + 4 * offset;
                index = insts
                    .iter()
                    .position(|candidate| candidate.pc as i64 == target)
                    .ok_or(ExecError::BadBranch { target })?;
            }
        }
    }
    Ok(steps)
}

pub enum Flow {
    Next,
    End,
    Jump(i64),
}

fn execute(inst: &Inst, wave: &mut Wave, memory: &mut Memory) -> Result<Flow> {
    let name = inst.name.as_str();
    let base = name
        .strip_suffix("_e32")
        .or_else(|| name.strip_suffix("_e64"))
        .unwrap_or(name);
    if let Some(flow) = salu::execute(base, inst, wave)? {
        return Ok(flow);
    }
    if valu::execute(base, inst, wave)? {
        return Ok(Flow::Next);
    }
    if memory::execute(base, inst, wave, memory)? {
        return Ok(Flow::Next);
    }
    Err(ExecError::Unsupported {
        pc: inst.pc,
        text: inst.text(),
    })
}
