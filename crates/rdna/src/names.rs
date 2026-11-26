pub const HWREGS: [(u32, &str); 18] = [
    (1, "HW_REG_MODE"),
    (2, "HW_REG_STATUS"),
    (3, "HW_REG_TRAPSTS"),
    (4, "HW_REG_HW_ID1"),
    (5, "HW_REG_GPR_ALLOC"),
    (6, "HW_REG_LDS_ALLOC"),
    (7, "HW_REG_IB_STS"),
    (15, "HW_REG_SH_MEM_BASES"),
    (16, "HW_REG_TBA_LO"),
    (17, "HW_REG_TBA_HI"),
    (18, "HW_REG_TMA_LO"),
    (19, "HW_REG_TMA_HI"),
    (20, "HW_REG_FLAT_SCR_LO"),
    (21, "HW_REG_FLAT_SCR_HI"),
    (22, "HW_REG_XNACK_MASK"),
    (23, "HW_REG_HW_ID2"),
    (24, "HW_REG_POPS_PACKER"),
    (26, "HW_REG_SHADER_CYCLES"),
];

pub const MESSAGES: [(u32, &str); 12] = [
    (1, "MSG_INTERRUPT"),
    (2, "MSG_GS"),
    (3, "MSG_GS_DONE"),
    (4, "MSG_SAVEWAVE"),
    (5, "MSG_STALL_WAVE_GEN"),
    (6, "MSG_HALT_WAVES"),
    (7, "MSG_ORDERED_PS_DONE"),
    (8, "MSG_EARLY_PRIM_DEALLOC"),
    (9, "MSG_GS_ALLOC_REQ"),
    (10, "MSG_GET_DOORBELL"),
    (11, "MSG_GET_DDID"),
    (15, "MSG_SYSMSG"),
];

pub const GS_OPS: [&str; 4] = ["GS_OP_NOP", "GS_OP_CUT", "GS_OP_EMIT", "GS_OP_EMIT_CUT"];

pub const MIMG_DIMS: [&str; 8] = [
    "SQ_RSRC_IMG_1D",
    "SQ_RSRC_IMG_2D",
    "SQ_RSRC_IMG_3D",
    "SQ_RSRC_IMG_CUBE",
    "SQ_RSRC_IMG_1D_ARRAY",
    "SQ_RSRC_IMG_2D_ARRAY",
    "SQ_RSRC_IMG_2D_MSAA",
    "SQ_RSRC_IMG_2D_MSAA_ARRAY",
];

pub const SDWA_SELS: [&str; 8] = [
    "BYTE_0", "BYTE_1", "BYTE_2", "BYTE_3", "WORD_0", "WORD_1", "DWORD", "DWORD",
];

pub const SDWA_UNUSED: [&str; 4] = ["UNUSED_PAD", "UNUSED_SEXT", "UNUSED_PRESERVE", "UNUSED_PAD"];

pub fn hwreg(simm16: u32) -> String {
    let id = simm16 & 0x3F;
    let offset = (simm16 >> 6) & 0x1F;
    let size = ((simm16 >> 11) & 0x1F) + 1;
    let name = HWREGS
        .iter()
        .find(|(code, _)| *code == id)
        .map_or_else(|| id.to_string(), |(_, name)| (*name).to_string());
    if offset == 0 && size == 32 {
        format!("hwreg({name})")
    } else {
        format!("hwreg({name}, {offset}, {size})")
    }
}

pub fn parse_swizzle(text: &str) -> Option<u32> {
    let inner = text.strip_prefix("swizzle(")?.strip_suffix(')')?;
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    let number = |i: usize| parts.get(i)?.parse::<u32>().ok();
    let bitmask =
        |and: u32, or: u32, xor: u32| (and & 0x1F) | ((or & 0x1F) << 5) | ((xor & 0x1F) << 10);
    Some(match parts[0] {
        "QUAD_PERM" => {
            let mut offset = 0x8000;
            for i in 0..4 {
                offset |= (number(i + 1)? & 3) << (2 * i);
            }
            offset
        }
        "SWAP" => bitmask(0x1F, 0, number(1)?),
        "REVERSE" => bitmask(0x1F, 0, number(1)?.checked_sub(1)?),
        "BROADCAST" => bitmask(0x1F ^ (number(1)?.checked_sub(1)?), number(2)?, 0),
        "BITMASK_PERM" => {
            let pattern = parts.get(1)?.trim_matches('"');
            if pattern.len() != 5 {
                return None;
            }
            let (mut and, mut or, mut xor) = (0, 0, 0);
            for (i, c) in pattern.chars().enumerate() {
                let bit = 1 << (4 - i);
                match c {
                    '0' => {}
                    '1' => or |= bit,
                    'p' => and |= bit,
                    'i' => {
                        and |= bit;
                        xor |= bit;
                    }
                    _ => return None,
                }
            }
            bitmask(and, or, xor)
        }
        _ => return None,
    })
}

pub fn parse_hwreg(text: &str) -> Option<u32> {
    let inner = text.strip_prefix("hwreg(")?.strip_suffix(')')?;
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    let id = HWREGS
        .iter()
        .find(|(_, name)| *name == parts[0])
        .map(|(code, _)| *code)
        .or_else(|| parts[0].parse().ok())?;
    let (offset, size) = match parts.as_slice() {
        [_] => (0, 32),
        [_, offset, size] => (offset.parse().ok()?, size.parse::<u32>().ok()?),
        _ => return None,
    };
    Some(id | (offset << 6) | ((size - 1) << 11))
}

pub fn sendmsg(simm16: u32) -> String {
    let id = simm16 & 0xF;
    let op = (simm16 >> 4) & 7;
    let stream = (simm16 >> 8) & 3;
    let Some((_, name)) = MESSAGES.iter().find(|(code, _)| *code == id) else {
        return format!("sendmsg({simm16})");
    };
    if id == 2 || id == 3 {
        let op_name = GS_OPS
            .get(op as usize)
            .map_or_else(|| op.to_string(), |name| (*name).to_string());
        if op == 0 {
            return format!("sendmsg({name}|{op_name})");
        }
        return format!("sendmsg({name}|{op_name}|{stream})");
    }
    if op != 0 {
        return format!("sendmsg({name}|{op})");
    }
    format!("sendmsg({name})")
}

pub fn parse_sendmsg(text: &str) -> Option<u32> {
    let inner = text.strip_prefix("sendmsg(")?.strip_suffix(')')?;
    let parts: Vec<&str> = inner.split('|').collect();
    let Some((id, _)) = MESSAGES.iter().find(|(_, name)| *name == parts[0]) else {
        return inner.parse().ok();
    };
    let op = match parts.get(1) {
        None => 0,
        Some(op) => GS_OPS
            .iter()
            .position(|name| name == op)
            .map_or_else(|| op.parse().ok(), |p| Some(p as u32))?,
    };
    let stream: u32 = parts.get(2).map_or(Some(0), |s| s.parse().ok())?;
    Some(id | (op << 4) | (stream << 8))
}
