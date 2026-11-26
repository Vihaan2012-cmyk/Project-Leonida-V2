use rdna::{Arg, Inst, Operand};

use crate::{ExecError, Result};

pub const VCC: usize = 106;
pub const EXEC: usize = 126;

#[derive(Debug, Clone)]
pub struct Wave {
    pub wave64: bool,
    pub sgpr: [u32; 128],
    pub vgpr: Vec<[u32; 64]>,
    pub scc: bool,
    pub lds: Vec<u8>,
    pub flush_denormals: bool,
    pub canonical_nan: Option<u32>,
    pub exports: Vec<u32>,
    pub messages: Vec<(u32, u32)>,
}

pub const EXPORT_WORDS: usize = 64 * 4 * 64;

impl Wave {
    pub fn new(wave64: bool) -> Wave {
        let mut wave = Wave {
            wave64,
            sgpr: [0; 128],
            vgpr: vec![[0; 64]; 256],
            scc: false,
            lds: vec![0; 64 * 1024],
            flush_denormals: false,
            canonical_nan: None,
            exports: vec![0; EXPORT_WORDS],
            messages: Vec::new(),
        };
        wave.set_mask(EXEC, wave.full_mask());
        wave
    }

    pub fn lanes(&self) -> usize {
        if self.wave64 { 64 } else { 32 }
    }

    pub fn full_mask(&self) -> u64 {
        if self.wave64 { u64::MAX } else { 0xFFFF_FFFF }
    }

    pub fn mask(&self, slot: usize) -> u64 {
        let lo = u64::from(self.sgpr[slot]);
        let value = if self.wave64 {
            lo | (u64::from(self.sgpr[slot + 1]) << 32)
        } else {
            lo
        };
        value & self.full_mask()
    }

    pub fn set_mask(&mut self, slot: usize, value: u64) {
        let value = value & self.full_mask();
        self.sgpr[slot] = value as u32;
        if self.wave64 {
            self.sgpr[slot + 1] = (value >> 32) as u32;
        }
    }

    pub fn exec(&self) -> u64 {
        self.mask(EXEC)
    }

    pub fn active(&self, lane: usize) -> bool {
        self.exec() >> lane & 1 != 0
    }

    pub fn active_lanes(&self) -> Vec<usize> {
        (0..self.lanes())
            .filter(|lane| self.active(*lane))
            .collect()
    }
}

#[derive(Debug, Clone, Default)]
pub struct Memory {
    pub bytes: Vec<u8>,
}

impl Memory {
    pub fn new(size: usize) -> Memory {
        Memory {
            bytes: vec![0; size],
        }
    }

    pub fn read(&self, address: u64) -> Result<u32> {
        let at = usize::try_from(address).map_err(|_| ExecError::OutOfBounds { address })?;
        let bytes = self
            .bytes
            .get(at..at + 4)
            .ok_or(ExecError::OutOfBounds { address })?;
        Ok(u32::from_le_bytes(bytes.try_into().expect("4 bytes")))
    }

    pub fn write(&mut self, address: u64, value: u32) -> Result<()> {
        let at = usize::try_from(address).map_err(|_| ExecError::OutOfBounds { address })?;
        let bytes = self
            .bytes
            .get_mut(at..at + 4)
            .ok_or(ExecError::OutOfBounds { address })?;
        bytes.copy_from_slice(&value.to_le_bytes());
        Ok(())
    }

    pub fn word(&self, index: usize) -> u32 {
        self.read(4 * index as u64).unwrap_or(0)
    }
}

pub fn bad(inst: &Inst) -> ExecError {
    ExecError::BadOperand {
        pc: inst.pc,
        text: inst.text(),
    }
}

pub fn arg(inst: &Inst, index: usize) -> Result<&Arg> {
    inst.args.get(index).ok_or_else(|| bad(inst))
}

pub fn sgpr_slot(arg: &Arg) -> Option<usize> {
    match &arg.operand {
        Operand::Sgpr { reg, .. } => Some(usize::from(*reg)),
        other => other
            .special_code()
            .filter(|code| *code < 128)
            .map(|code| code as usize),
    }
}

impl Wave {
    pub fn read_scalar(&self, arg: &Arg, inst: &Inst) -> Result<u32> {
        if let Some(slot) = sgpr_slot(arg) {
            return Ok(self.sgpr[slot]);
        }
        match &arg.operand {
            Operand::Int(value) => Ok(*value as u32),
            Operand::Literal(value) => Ok(*value),
            Operand::Float(_) => Ok(arg.operand.float_bits().unwrap_or(0)),
            Operand::Named(name) if name == "src_scc" => Ok(u32::from(self.scc)),
            Operand::Named(name) if name == "src_execz" => Ok(u32::from(self.exec() == 0)),
            Operand::Named(name) if name == "src_vccz" => Ok(u32::from(self.mask(VCC) == 0)),
            _ => Err(bad(inst)),
        }
    }

    pub fn read_scalar64(&self, arg: &Arg, inst: &Inst) -> Result<u64> {
        if let Some(slot) = sgpr_slot(arg) {
            return Ok(u64::from(self.sgpr[slot]) | (u64::from(self.sgpr[slot + 1]) << 32));
        }
        let value = self.read_scalar(arg, inst)?;
        Ok(match arg.operand {
            Operand::Int(value) => value as u64,
            _ => u64::from(value),
        })
    }

    pub fn write_scalar(&mut self, arg: &Arg, value: u32, inst: &Inst) -> Result<()> {
        let slot = sgpr_slot(arg).ok_or_else(|| bad(inst))?;
        if slot != 125 {
            self.sgpr[slot] = value;
        }
        Ok(())
    }

    pub fn write_scalar64(&mut self, arg: &Arg, value: u64, inst: &Inst) -> Result<()> {
        let slot = sgpr_slot(arg).ok_or_else(|| bad(inst))?;
        if slot != 125 {
            self.sgpr[slot] = value as u32;
            self.sgpr[slot + 1] = (value >> 32) as u32;
        }
        Ok(())
    }

    pub fn read_lane(&self, arg: &Arg, lane: usize, inst: &Inst) -> Result<u32> {
        match &arg.operand {
            Operand::Vgpr { reg, .. } => Ok(self.vgpr[usize::from(*reg)][lane]),
            _ => self.read_scalar(arg, inst),
        }
    }

    pub fn read_lane_f32(&self, arg: &Arg, lane: usize, inst: &Inst) -> Result<f32> {
        let mut value = f32::from_bits(self.read_lane(arg, lane, inst)?);
        if arg.abs {
            value = value.abs();
        }
        if arg.neg {
            value = -value;
        }
        Ok(self.flush(value))
    }

    pub fn flush(&self, value: f32) -> f32 {
        match self.canonical_nan {
            Some(bits) if value.is_nan() => f32::from_bits(bits),
            _ if self.flush_denormals && value.is_subnormal() => 0.0f32.copysign(value),
            _ => value,
        }
    }

    pub fn write_lane(&mut self, arg: &Arg, lane: usize, value: u32, inst: &Inst) -> Result<()> {
        match &arg.operand {
            Operand::Vgpr { reg, .. } => {
                self.vgpr[usize::from(*reg)][lane] = value;
                Ok(())
            }
            _ => Err(bad(inst)),
        }
    }

    pub fn mask_operand(&self, arg: &Arg, inst: &Inst) -> Result<u64> {
        let slot = sgpr_slot(arg).ok_or_else(|| bad(inst))?;
        Ok(self.mask(slot))
    }
}
