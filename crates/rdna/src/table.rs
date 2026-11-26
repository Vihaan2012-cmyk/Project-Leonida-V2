#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Addr,
    Attr,
    Data,
    Data0,
    Data1,
    Literal,
    OprExec,
    Saddr,
    Sbase,
    Sdata,
    Sdst,
    Simm16,
    Simm32,
    Soffset,
    Src0,
    Src1,
    Src2,
    Srsrc,
    Ssamp,
    Ssrc0,
    Ssrc1,
    Vaddr,
    Vcc,
    Vdata,
    Vdst,
    Vsrc,
    Vsrc1,
}

#[derive(Debug, Clone, Copy)]
pub struct Spec {
    pub field: Field,
    pub regs: i8,
}

#[derive(Debug)]
pub struct OpInfo {
    pub name: &'static str,
    pub spec: &'static [Spec],
}

pub fn lookup(table: &'static [(u16, OpInfo)], opcode: u32) -> Option<&'static OpInfo> {
    let opcode = u16::try_from(opcode).ok()?;
    table
        .binary_search_by_key(&opcode, |(op, _)| *op)
        .ok()
        .map(|index| &table[index].1)
}

pub fn contains(table: &'static [(u16, OpInfo)], opcode: u32) -> bool {
    lookup(table, opcode).is_some()
}
