use crate::{Memory, Wave};

pub const MEMORY: usize = 0x1000;

const APPROXIMATE: [&str; 5] = [
    "v_rcp_f32",
    "v_rsq_f32",
    "v_sqrt_f32",
    "v_exp_f32",
    "v_log_f32",
];

pub fn start(wave64: bool, seed: u64) -> Wave {
    let mut rng = Rng(seed | 1);
    let mut wave = Wave::new(wave64);
    for reg in 0..16 {
        wave.sgpr[reg] = rng.value();
    }
    for reg in 0..9 {
        for lane in 0..64 {
            wave.vgpr[reg][lane] = if reg == 0 { lane as u32 } else { rng.value() };
        }
    }
    wave.sgpr[17] = 4 * (rng.word() % 8);
    wave.sgpr[20] = 0x100;
    wave.sgpr[22] = 0x40 + 4 * (rng.word() % 0x80);
    wave.sgpr[24] = 0x400;
    wave.vgpr[9] = [0; 64];
    wave
}

pub fn memory(seed: u64) -> Memory {
    let mut rng = Rng(seed ^ 0x9E37_79B9_7F4A_7C15);
    let mut memory = Memory::new(MEMORY);
    for word in memory.bytes.chunks_mut(4) {
        word.copy_from_slice(&rng.value().to_le_bytes());
    }
    memory
}

pub struct Rng(pub u64);

impl Rng {
    pub fn word(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }

    pub fn below(&mut self, n: usize) -> usize {
        self.word() as usize % n
    }

    pub fn chance(&mut self, n: u32) -> bool {
        self.word().is_multiple_of(n)
    }

    pub fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }

    pub fn value(&mut self) -> u32 {
        match self.word() % 6 {
            0 => self.word() % 64,
            1 => ((self.word() % 64) as f32).to_bits(),
            2 => ((self.word() % 2000) as f32 / 16.0 - 60.0).to_bits(),
            3 => u32::MAX - self.word() % 8,
            _ => self.word() ^ (self.word() << 16),
        }
    }
}

pub struct Generator {
    pub rng: Rng,
    pub wave64: bool,
    pub gpu: bool,
}

const FLOATS: [&str; 8] = ["0.5", "-0.5", "1.0", "-1.0", "2.0", "-2.0", "4.0", "-4.0"];

impl Generator {
    fn sreg(&mut self) -> String {
        format!("s{}", self.rng.below(16))
    }

    fn vreg(&mut self) -> String {
        format!("v{}", self.rng.below(9))
    }

    fn vdst(&mut self) -> String {
        let count = if self.gpu { 4 } else { 8 };
        format!("v{}", 1 + self.rng.below(count))
    }

    fn fdst(&mut self) -> String {
        if self.gpu {
            format!("v{}", 5 + self.rng.below(4))
        } else {
            self.vdst()
        }
    }

    fn fvreg(&mut self) -> String {
        if self.gpu {
            format!("v{}", 5 + self.rng.below(4))
        } else {
            self.vreg()
        }
    }

    fn fvsrc(&mut self, literal: bool) -> String {
        if !self.gpu {
            self.vsrc(literal)
        } else if self.rng.chance(3) {
            self.rng.pick(&FLOATS).into()
        } else {
            self.fvreg()
        }
    }

    fn pair(&mut self) -> String {
        if self.rng.chance(6) {
            return "vcc".into();
        }
        let first = 2 * self.rng.below(8);
        format!("s[{}:{}]", first, first + 1)
    }

    fn vcc(&self) -> &'static str {
        if self.wave64 { "vcc" } else { "vcc_lo" }
    }

    fn exec(&self) -> &'static str {
        if self.wave64 { "exec" } else { "exec_lo" }
    }

    fn mask_reg(&mut self) -> String {
        if self.wave64 {
            self.pair()
        } else {
            self.sreg()
        }
    }

    fn inline(&mut self) -> String {
        match self.rng.below(3) {
            0 => format!("{}", self.rng.below(65)),
            1 => format!("-{}", 1 + self.rng.below(16)),
            _ => self.rng.pick(&FLOATS).into(),
        }
    }

    fn ssrc(&mut self, literal: bool) -> String {
        match self.rng.below(12) {
            0..=5 => self.sreg(),
            6 | 7 => self.inline(),
            8 if literal => format!("0x{:x}", self.rng.value()),
            9 => self.rng.pick(&["vcc_lo", "exec_lo", "m0", "vcc_hi"]).into(),
            _ => self.sreg(),
        }
    }

    fn vsrc(&mut self, literal: bool) -> String {
        if self.rng.chance(3) {
            self.ssrc(literal)
        } else {
            self.vreg()
        }
    }

    fn fsrc(&mut self) -> String {
        let value = self.fvsrc(false);
        match self.rng.below(6) {
            0 => format!("-{value}"),
            1 => format!("|{value}|"),
            2 => format!("-|{value}|"),
            _ => value,
        }
    }

    fn typed_load(&mut self) -> String {
        let count = 1 + self.rng.below(4);
        let suffix = ["x", "xy", "xyz", "xyzw"][count - 1];
        let limit = if self.gpu { 4 } else { 8 };
        let first = 1 + self.rng.below(limit + 1 - count);
        let target = if count == 1 {
            format!("v{first}")
        } else {
            format!("v[{first}:{}]", first + count - 1)
        };
        let offset = 4 * self.rng.below(16) + self.rng.below(4);
        let code = 1 + self.rng.word() % 77;
        let name = rdna::format::name(code).expect("codes 1 to 77 have names");
        if self.rng.chance(2) {
            format!(
                "tbuffer_load_format_{suffix} {target}, v9, s[20:23], 0 offen offset:{offset} format:[{name}]"
            )
        } else {
            let mut select = 0;
            for channel in 0..4 {
                select |= [0u32, 1, 4, 5, 6, 7][self.rng.below(6)] << (3 * channel);
            }
            if self.rng.chance(3) {
                select = 4 | (5 << 3) | (6 << 6) | (7 << 9);
            }
            format!(
                "s_mov_b32 s23, 0x{:x}\nbuffer_load_format_{suffix} {target}, v9, s[20:23], 0 offen offset:{offset}",
                (code << 12) | select
            )
        }
    }

    fn dpp_control(&mut self) -> String {
        let n = 1 + self.rng.below(15);
        let mut control = match self.rng.below(9) {
            0 => {
                let q: Vec<String> = (0..4).map(|_| self.rng.below(4).to_string()).collect();
                format!("quad_perm:[{}]", q.join(","))
            }
            1 => format!("row_shl:{n}"),
            2 => format!("row_shr:{n}"),
            3 => format!("row_ror:{n}"),
            4 => "row_mirror".to_string(),
            5 => "row_half_mirror".to_string(),
            6 => format!("row_share:{}", self.rng.below(16)),
            7 => format!("row_xmask:{}", self.rng.below(16)),
            _ => {
                let lanes: Vec<String> = (0..8).map(|_| self.rng.below(8).to_string()).collect();
                let fi = if self.rng.chance(2) { " fi:1" } else { "" };
                return format!("dpp8:[{}]{fi}", lanes.join(","));
            }
        };
        let (row, bank) = if self.rng.chance(2) {
            (15, 15)
        } else {
            (self.rng.below(16), self.rng.below(16))
        };
        control += &format!(" row_mask:0x{row:x} bank_mask:0x{bank:x}");
        if self.rng.chance(2) {
            control += " bound_ctrl:1";
        }
        if self.rng.chance(3) {
            control += " fi:1";
        }
        control
    }

    fn cross_lane(&mut self) -> String {
        match self.rng.below(5) {
            0 | 1 => {
                let control = self.dpp_control();
                if self.rng.chance(3) {
                    let op = self
                        .rng
                        .pick(&["v_add_f32", "v_mul_f32", "v_max_f32", "v_fmac_f32"]);
                    format!(
                        "{op}_dpp {}, {}, {} {control}",
                        self.fdst(),
                        self.fvreg(),
                        self.fvreg()
                    )
                } else if self.rng.chance(3) {
                    let op = self.rng.pick(&["v_mov_b32", "v_not_b32"]);
                    format!("{op}_dpp {}, {} {control}", self.vdst(), self.vreg())
                } else {
                    let op = self.rng.pick(&[
                        "v_add_nc_u32",
                        "v_sub_nc_u32",
                        "v_and_b32",
                        "v_or_b32",
                        "v_xor_b32",
                        "v_min_u32",
                        "v_max_u32",
                    ]);
                    format!(
                        "{op}_dpp {}, {}, {} {control}",
                        self.vdst(),
                        self.vreg(),
                        self.vreg()
                    )
                }
            }
            2 => {
                let op = self.rng.pick(&["v_permlane16_b32", "v_permlanex16_b32"]);
                let (fi, bc) = (self.rng.below(2), self.rng.below(2));
                format!(
                    "{op} {}, {}, {}, {} op_sel:[{fi},{bc}]",
                    self.vdst(),
                    self.vreg(),
                    self.sreg(),
                    self.sreg()
                )
            }
            3 => {
                let pattern = match self.rng.below(5) {
                    0 => {
                        let q: Vec<String> =
                            (0..4).map(|_| self.rng.below(4).to_string()).collect();
                        format!("QUAD_PERM,{}", q.join(","))
                    }
                    1 => format!("SWAP,{}", 1 << self.rng.below(5)),
                    2 => format!("REVERSE,{}", 2 << self.rng.below(4)),
                    3 => {
                        let size = 2 << self.rng.below(4);
                        format!("BROADCAST,{size},{}", self.rng.below(size))
                    }
                    _ => {
                        let chars: String = (0..5)
                            .map(|_| ['0', '1', 'p', 'i'][self.rng.below(4)])
                            .collect();
                        format!("BITMASK_PERM,\"{chars}\"")
                    }
                };
                format!(
                    "ds_swizzle_b32 {}, {} offset:swizzle({pattern})",
                    self.vdst(),
                    self.vreg()
                )
            }
            _ => {
                let index = if self.rng.chance(6) {
                    300
                } else {
                    self.rng.below(9)
                };
                format!(
                    "s_mov_b32 m0, {index}\nv_movrels_b32_e32 {}, v{}",
                    self.vdst(),
                    self.rng.below(3)
                )
            }
        }
    }

    fn half_source(&mut self) -> String {
        let value = if self.rng.chance(5) {
            self.rng.pick(&FLOATS).to_string()
        } else {
            self.vsrc(false)
        };
        match self.rng.below(5) {
            0 => format!("-{value}"),
            1 => format!("|{value}|"),
            _ => value,
        }
    }

    fn half(&mut self) -> String {
        let bit = |g: &mut Self| g.rng.below(2);
        match self.rng.below(6) {
            0 => format!("v_cvt_f16_f32_e32 {}, {}", self.vdst(), self.fvsrc(true)),
            1 => format!("v_cvt_f16_f32_e64 {}, {}", self.vdst(), self.fsrc()),
            2 => {
                if self.rng.chance(2) {
                    format!("v_cvt_f32_f16_e32 {}, {}", self.fdst(), self.vsrc(true))
                } else {
                    let omod = self.omod();
                    format!(
                        "v_cvt_f32_f16_e64 {}, {}{omod}",
                        self.fdst(),
                        self.half_source()
                    )
                }
            }
            3 => {
                let (a, b) = (bit(self), bit(self));
                let (x, y) = (self.half_source(), self.half_source());
                format!("v_pack_b32_f16 {}, {x}, {y} op_sel:[{a},{b}]", self.vdst())
            }
            _ => {
                let op = self
                    .rng
                    .pick(&["v_fma_mix_f32", "v_fma_mixlo_f16", "v_fma_mixhi_f16"]);
                let hi = [bit(self), bit(self), bit(self)];
                let sel = [bit(self), bit(self), bit(self)];
                let mut sources = Vec::new();
                for half in hi {
                    let source = if half == 1 {
                        self.half_source()
                    } else {
                        self.fsrc()
                    };
                    sources.push(source);
                }
                let target = if op == "v_fma_mix_f32" {
                    self.fdst()
                } else {
                    self.vdst()
                };
                let clamp = if self.rng.chance(4) { " clamp" } else { "" };
                format!(
                    "{op} {target}, {} op_sel:[{},{},{}] op_sel_hi:[{},{},{}]{clamp}",
                    sources.join(", "),
                    sel[0],
                    sel[1],
                    sel[2],
                    hi[0],
                    hi[1],
                    hi[2]
                )
            }
        }
    }

    fn sdwa_source(&mut self, scalar: bool) -> String {
        let register = if scalar && self.rng.chance(4) {
            self.sreg()
        } else {
            self.vreg()
        };
        if self.rng.chance(4) {
            format!("sext({register})")
        } else {
            register
        }
    }

    fn sdwa(&mut self) -> String {
        const SELECTS: [&str; 7] = [
            "BYTE_0", "BYTE_1", "BYTE_2", "BYTE_3", "WORD_0", "WORD_1", "DWORD",
        ];
        let (s0, s1, d) = (
            self.rng.pick(&SELECTS),
            self.rng.pick(&SELECTS),
            self.rng.pick(&SELECTS),
        );
        let unused = self
            .rng
            .pick(&["UNUSED_PAD", "UNUSED_SEXT", "UNUSED_PRESERVE"]);
        match self.rng.below(3) {
            0 => {
                let op = self.rng.pick(&[
                    "v_add_nc_u32",
                    "v_sub_nc_u32",
                    "v_and_b32",
                    "v_or_b32",
                    "v_xor_b32",
                    "v_lshlrev_b32",
                    "v_mul_u32_u24",
                    "v_min_u32",
                    "v_max_i32",
                ]);
                let (a, b) = (self.sdwa_source(true), self.sdwa_source(false));
                format!(
                    "{op}_sdwa {}, {a}, {b} dst_sel:{d} dst_unused:{unused} src0_sel:{s0} src1_sel:{s1}",
                    self.vdst()
                )
            }
            1 => {
                let op = self.rng.pick(&[
                    "v_mov_b32",
                    "v_not_b32",
                    "v_cvt_f32_u32",
                    "v_cvt_f32_i32",
                    "v_cvt_f16_f32",
                    "v_cvt_f32_f16",
                ]);
                let float_out = op.starts_with("v_cvt_f32");
                let target = if float_out { self.fdst() } else { self.vdst() };
                let d = if float_out { "DWORD" } else { d };
                let source = self.sdwa_source(true);
                format!(
                    "{op}_sdwa {target}, {source} dst_sel:{d} dst_unused:{unused} src0_sel:{s0}"
                )
            }
            _ => {
                let op = self.rng.pick(&["lt", "eq", "ne", "gt"]);
                let ty = self.rng.pick(&["u32", "i32"]);
                let (a, b) = (self.sdwa_source(true), self.sdwa_source(false));
                if self.rng.chance(4) {
                    format!("v_cmpx_{op}_{ty}_sdwa {a}, {b} src0_sel:{s0} src1_sel:{s1}")
                } else {
                    let target = if self.rng.chance(2) {
                        self.vcc().to_string()
                    } else {
                        self.mask_reg()
                    };
                    format!("v_cmp_{op}_{ty}_sdwa {target}, {a}, {b} src0_sel:{s0} src1_sel:{s1}")
                }
            }
        }
    }

    fn export(&mut self) -> String {
        match self.rng.below(4) {
            0 => "s_sendmsg sendmsg(MSG_GS_ALLOC_REQ)".into(),
            1 => "s_setreg_imm32_b32 hwreg(HW_REG_MODE, 2, 2), 0".into(),
            2 => {
                let target = self.rng.pick(&[
                    "mrt0", "mrt3", "pos0", "pos1", "prim", "param0", "param5", "param31",
                ]);
                let (a, b) = (self.vreg(), self.vreg());
                format!("exp {target} {a}, {a}, {b}, {b} compr")
            }
            _ => {
                let target = self.rng.pick(&[
                    "mrt1", "mrtz", "pos0", "pos3", "prim", "param1", "param7", "param30",
                ]);
                let mut sources = Vec::new();
                for _ in 0..4 {
                    sources.push(if self.rng.chance(4) {
                        "off".to_string()
                    } else {
                        self.vreg()
                    });
                }
                let done = if self.rng.chance(3) { " done" } else { "" };
                format!("exp {target} {}{done}", sources.join(", "))
            }
        }
    }

    fn float_literal(&mut self) -> String {
        let value = ((self.rng.word() % 2000) as f32 / 16.0 - 60.0).to_bits();
        format!("0x{value:x}")
    }

    fn omod(&mut self) -> &'static str {
        self.rng.pick(&[
            "",
            "",
            "",
            " clamp",
            " mul:2",
            " mul:4",
            " div:2",
            " mul:2 clamp",
        ])
    }

    fn sdst(&mut self) -> String {
        match self.rng.below(30) {
            0 => "m0".into(),
            1 => "vcc_lo".into(),
            _ => self.sreg(),
        }
    }

    fn scalar(&mut self) -> String {
        match self.rng.below(9) {
            0 => {
                let op = self.rng.pick(&[
                    "s_add_u32",
                    "s_addc_u32",
                    "s_sub_u32",
                    "s_subb_u32",
                    "s_add_i32",
                    "s_sub_i32",
                    "s_mul_i32",
                    "s_and_b32",
                    "s_or_b32",
                    "s_xor_b32",
                    "s_andn2_b32",
                    "s_orn2_b32",
                    "s_nand_b32",
                    "s_nor_b32",
                    "s_xnor_b32",
                    "s_lshl_b32",
                    "s_lshr_b32",
                    "s_ashr_i32",
                    "s_min_u32",
                    "s_max_u32",
                    "s_min_i32",
                    "s_max_i32",
                    "s_cselect_b32",
                    "s_bfe_u32",
                    "s_lshl1_add_u32",
                    "s_lshl2_add_u32",
                    "s_lshl3_add_u32",
                    "s_lshl4_add_u32",
                    "s_pack_ll_b32_b16",
                    "s_pack_lh_b32_b16",
                    "s_pack_hh_b32_b16",
                    "s_mul_hi_u32",
                    "s_bfe_i32",
                ]);
                format!(
                    "{op} {}, {}, {}",
                    self.sdst(),
                    self.ssrc(true),
                    self.ssrc(false)
                )
            }
            1 => {
                let op = self.rng.pick(&[
                    "s_mov_b32",
                    "s_cmov_b32",
                    "s_not_b32",
                    "s_brev_b32",
                    "s_bcnt1_i32_b32",
                    "s_ff1_i32_b32",
                    "s_sext_i32_i8",
                    "s_sext_i32_i16",
                    "s_flbit_i32_b32",
                    "s_bitset1_b32",
                ]);
                format!("{op} {}, {}", self.sdst(), self.ssrc(true))
            }
            2 => {
                let op = self.rng.pick(&[
                    "s_cmp_eq_u32",
                    "s_cmp_lg_u32",
                    "s_cmp_lt_u32",
                    "s_cmp_le_u32",
                    "s_cmp_gt_u32",
                    "s_cmp_ge_u32",
                    "s_cmp_eq_i32",
                    "s_cmp_lg_i32",
                    "s_cmp_lt_i32",
                    "s_cmp_le_i32",
                    "s_cmp_gt_i32",
                    "s_cmp_ge_i32",
                ]);
                format!("{op} {}, {}", self.ssrc(true), self.ssrc(false))
            }
            3 => {
                let op = self
                    .rng
                    .pick(&["s_movk_i32", "s_addk_i32", "s_mulk_i32", "s_cmovk_i32"]);
                format!("{op} {}, 0x{:x}", self.sreg(), self.rng.word() % 0x10000)
            }
            4 => {
                let op = self.rng.pick(&[
                    "s_and_b64",
                    "s_or_b64",
                    "s_xor_b64",
                    "s_andn2_b64",
                    "s_orn2_b64",
                    "s_nand_b64",
                    "s_nor_b64",
                    "s_xnor_b64",
                    "s_cselect_b64",
                ]);
                format!("{op} {}, {}, {}", self.pair(), self.pair(), self.pair())
            }
            5 => {
                let op = self.rng.pick(&["s_mov_b64", "s_not_b64", "s_cmov_b64"]);
                format!("{op} {}, {}", self.pair(), self.pair())
            }
            6 => {
                let op = self.rng.pick(&["s_lshl_b64", "s_lshr_b64"]);
                format!(
                    "{op} {}, {}, {}",
                    self.pair(),
                    self.pair(),
                    self.ssrc(false)
                )
            }
            7 => format!("s_bcnt1_i32_b64 {}, {}", self.sreg(), self.pair()),
            _ => format!("s_mov_b32 {}, {}", self.sreg(), self.ssrc(true)),
        }
    }

    fn vector(&mut self) -> String {
        match self.rng.below(23) {
            0 | 1 => {
                let op = self.rng.pick(&[
                    "v_add_nc_u32",
                    "v_sub_nc_u32",
                    "v_subrev_nc_u32",
                    "v_and_b32",
                    "v_or_b32",
                    "v_xor_b32",
                    "v_xnor_b32",
                    "v_lshlrev_b32",
                    "v_lshrrev_b32",
                    "v_ashrrev_i32",
                    "v_min_u32",
                    "v_max_u32",
                    "v_min_i32",
                    "v_max_i32",
                    "v_mul_u32_u24",
                    "v_mul_i32_i24",
                    "v_bfm_b32",
                    "v_bcnt_u32_b32",
                ]);
                if op.starts_with("v_bfm") || op.starts_with("v_bcnt") {
                    format!(
                        "{op} {}, {}, {}",
                        self.vdst(),
                        self.vsrc(false),
                        self.vsrc(false)
                    )
                } else {
                    format!(
                        "{op}_e32 {}, {}, {}",
                        self.vdst(),
                        self.vsrc(true),
                        self.vreg()
                    )
                }
            }
            2 => {
                let op = self.rng.pick(&[
                    "v_mul_lo_u32",
                    "v_mul_hi_u32",
                    "v_add_nc_u32_e64",
                    "v_xor_b32_e64",
                ]);
                format!(
                    "{op} {}, {}, {}",
                    self.vdst(),
                    self.vsrc(false),
                    self.vsrc(false)
                )
            }
            3 | 4 => {
                let op = self.rng.pick(&[
                    "v_add_f32",
                    "v_sub_f32",
                    "v_subrev_f32",
                    "v_mul_f32",
                    "v_min_f32",
                    "v_max_f32",
                    "v_mul_legacy_f32",
                ]);
                if self.rng.chance(2) {
                    format!(
                        "{op}_e32 {}, {}, {}",
                        self.fdst(),
                        self.fvsrc(true),
                        self.fvreg()
                    )
                } else {
                    let omod = self.omod();
                    format!(
                        "{op}_e64 {}, {}, {}{omod}",
                        self.fdst(),
                        self.fsrc(),
                        self.fsrc()
                    )
                }
            }
            5 => {
                let op = self.rng.pick(&[
                    "v_mov_b32",
                    "v_not_b32",
                    "v_bfrev_b32",
                    "v_ffbl_b32",
                    "v_ffbh_u32",
                    "v_cvt_f32_u32",
                    "v_cvt_f32_i32",
                    "v_cvt_u32_f32",
                    "v_cvt_i32_f32",
                    "v_rcp_f32",
                    "v_rsq_f32",
                    "v_sqrt_f32",
                    "v_exp_f32",
                    "v_log_f32",
                    "v_floor_f32",
                    "v_ceil_f32",
                    "v_trunc_f32",
                    "v_rndne_f32",
                    "v_fract_f32",
                    "v_cvt_f32_ubyte0",
                    "v_cvt_f32_ubyte1",
                    "v_cvt_f32_ubyte2",
                    "v_cvt_f32_ubyte3",
                ]);
                let op = if self.gpu && APPROXIMATE.contains(&op) {
                    "v_floor_f32"
                } else {
                    op
                };
                let int_in = !op.ends_with("_f32") || op.starts_with("v_cvt_f32");
                let int_out = !op.starts_with("v_cvt_f32")
                    && (!op.ends_with("_f32") || op.starts_with("v_cvt"));
                let destination = if int_out { self.vdst() } else { self.fdst() };
                let source = if int_in {
                    self.vsrc(true)
                } else {
                    self.fvsrc(true)
                };
                format!("{op}_e32 {destination}, {source}")
            }
            6 => {
                let op = self.rng.pick(&[
                    "v_floor_f32",
                    "v_fract_f32",
                    "v_cvt_f32_i32",
                    "v_sqrt_f32",
                    "v_mov_b32",
                ]);
                let op = if self.gpu && APPROXIMATE.contains(&op) {
                    "v_trunc_f32"
                } else {
                    op
                };
                let omod = if op == "v_mov_b32" { "" } else { self.omod() };
                let source = if op == "v_mov_b32" || op == "v_cvt_f32_i32" {
                    self.vsrc(false)
                } else {
                    self.fsrc()
                };
                let destination = if op == "v_mov_b32" {
                    self.vdst()
                } else {
                    self.fdst()
                };
                format!("{op}_e64 {destination}, {source}{omod}")
            }
            7 => {
                let op = self.rng.pick(&[
                    "v_add3_u32",
                    "v_lshl_add_u32",
                    "v_mad_u32_u24",
                    "v_bfe_u32",
                    "v_bfe_i32",
                    "v_and_or_b32",
                    "v_or3_b32",
                    "v_xor3_b32",
                    "v_add_lshl_u32",
                    "v_lshl_or_b32",
                    "v_xad_u32",
                    "v_bfi_b32",
                    "v_alignbit_b32",
                    "v_alignbyte_b32",
                    "v_min3_u32",
                    "v_max3_u32",
                    "v_min3_i32",
                    "v_max3_i32",
                    "v_med3_u32",
                    "v_med3_i32",
                ]);
                format!(
                    "{op} {}, {}, {}, {}",
                    self.vdst(),
                    self.vsrc(false),
                    self.vsrc(false),
                    self.vsrc(false)
                )
            }
            8 => {
                let op = self
                    .rng
                    .pick(&["v_fma_f32", "v_mad_f32", "v_min3_f32", "v_max3_f32"]);
                let omod = self.omod();
                format!(
                    "{op} {}, {}, {}, {}{omod}",
                    self.fdst(),
                    self.fsrc(),
                    self.fsrc(),
                    self.fsrc()
                )
            }
            9 => {
                let op = self.rng.pick(&["v_fmac_f32", "v_mac_f32"]);
                format!(
                    "{op}_e32 {}, {}, {}",
                    self.fdst(),
                    self.fvsrc(true),
                    self.fvreg()
                )
            }
            10 => self.compare(),
            16 => self.sdwa(),
            18 | 19 => self.half(),
            20 | 21 => self.cross_lane(),
            17 => {
                let mut selectors = 0u32;
                for i in 0..4 {
                    selectors |= (self.rng.word() % 16) << (8 * i);
                }
                format!(
                    "v_perm_b32 {}, {}, {}, 0x{selectors:x}",
                    self.vdst(),
                    self.vsrc(false),
                    self.vsrc(false)
                )
            }
            14 => match self.rng.below(4) {
                0 => {
                    let op = self.rng.pick(&["v_fmaak_f32", "v_madak_f32"]);
                    let k = self.float_literal();
                    format!(
                        "{op} {}, {}, {}, {k}",
                        self.fdst(),
                        self.fvsrc(false),
                        self.fvreg()
                    )
                }
                1 => {
                    let op = self.rng.pick(&["v_fmamk_f32", "v_madmk_f32"]);
                    let k = self.float_literal();
                    format!(
                        "{op} {}, {}, {k}, {}",
                        self.fdst(),
                        self.fvsrc(false),
                        self.fvreg()
                    )
                }
                2 => {
                    let op = self.rng.pick(&["v_mad_legacy_f32", "v_med3_f32"]);
                    let omod = self.omod();
                    format!(
                        "{op} {}, {}, {}, {}{omod}",
                        self.fdst(),
                        self.fsrc(),
                        self.fsrc(),
                        self.fsrc()
                    )
                }
                _ => {
                    let op = if self.gpu {
                        "v_trunc_f32"
                    } else {
                        self.rng
                            .pick(&["v_rcp_iflag_f32", "v_sin_f32", "v_cos_f32"])
                    };
                    format!("{op}_e32 {}, {}", self.fdst(), self.fvsrc(true))
                }
            },
            15 => {
                if self.rng.chance(2) {
                    format!(
                        "v_cvt_pk_u8_f32 {}, {}, {}, {}",
                        self.vdst(),
                        self.fsrc(),
                        self.vsrc(false),
                        self.vsrc(false)
                    )
                } else {
                    let low = if self.gpu {
                        1 + 2 * self.rng.below(2)
                    } else {
                        1 + self.rng.below(7)
                    };
                    let source = self.rng.below(8);
                    format!(
                        "v_lshlrev_b64 v[{low}:{}], {}, v[{source}:{}]",
                        low + 1,
                        self.vsrc(false),
                        source + 1
                    )
                }
            }
            11 => {
                if self.rng.chance(2) {
                    format!(
                        "v_cndmask_b32_e32 {}, {}, {}, {}",
                        self.vdst(),
                        self.vsrc(true),
                        self.vreg(),
                        self.vcc()
                    )
                } else {
                    format!(
                        "v_cndmask_b32_e64 {}, {}, {}, {}",
                        self.vdst(),
                        self.vsrc(false),
                        self.vsrc(false),
                        self.mask_reg()
                    )
                }
            }
            12 => match self.rng.below(5) {
                0 => format!("v_readfirstlane_b32 {}, {}", self.sreg(), self.vreg()),
                1 => format!(
                    "v_readlane_b32 {}, {}, {}",
                    self.sreg(),
                    self.vreg(),
                    self.ssrc(false)
                ),
                2 => format!(
                    "v_writelane_b32 {}, {}, {}",
                    self.vdst(),
                    self.ssrc(false),
                    self.ssrc(false)
                ),
                3 => format!(
                    "v_mbcnt_lo_u32_b32 {}, {}, {}",
                    self.vdst(),
                    self.ssrc(false),
                    self.vsrc(false)
                ),
                _ => format!(
                    "v_mbcnt_hi_u32_b32 {}, {}, {}",
                    self.vdst(),
                    self.ssrc(false),
                    self.vsrc(false)
                ),
            },
            _ => {
                let op = self.rng.pick(&[
                    "v_add_co_u32",
                    "v_sub_co_u32",
                    "v_subrev_co_u32",
                    "v_add_co_ci_u32",
                    "v_sub_co_ci_u32",
                    "v_subrev_co_ci_u32",
                ]);
                if op.contains("_ci_") {
                    if self.rng.chance(2) {
                        let vcc = self.vcc();
                        format!(
                            "{op}_e32 {}, {vcc}, {}, {}, {vcc}",
                            self.vdst(),
                            self.vsrc(true),
                            self.vreg()
                        )
                    } else {
                        format!(
                            "{op}_e64 {}, {}, {}, {}, {}",
                            self.vdst(),
                            self.mask_reg(),
                            self.vsrc(false),
                            self.vsrc(false),
                            self.mask_reg()
                        )
                    }
                } else {
                    format!(
                        "{op}_e64 {}, {}, {}, {}",
                        self.vdst(),
                        self.mask_reg(),
                        self.vsrc(false),
                        self.vsrc(false)
                    )
                }
            }
        }
    }

    fn compare(&mut self) -> String {
        let float = self.rng.chance(2);
        let (op, ty) = if float {
            (
                self.rng.pick(&[
                    "f", "lt", "eq", "le", "gt", "lg", "ge", "o", "u", "nge", "nlg", "ngt", "nle",
                    "neq", "nlt", "tru",
                ]),
                "f32",
            )
        } else {
            (
                self.rng
                    .pick(&["f", "lt", "eq", "le", "gt", "ne", "ge", "t"]),
                self.rng.pick(&["u32", "i32"]),
            )
        };
        if float && self.rng.chance(6) {
            let mask = self.vsrc(false);
            return format!(
                "v_cmp_class_f32_e64 {}, {}, {mask}",
                self.mask_reg(),
                self.fsrc()
            );
        }
        if self.rng.chance(2) {
            let (a, b) = if float {
                (self.fvsrc(true), self.fvreg())
            } else {
                (self.vsrc(true), self.vreg())
            };
            format!("v_cmp_{op}_{ty}_e32 {}, {a}, {b}", self.vcc())
        } else {
            let (a, b) = if float {
                (self.fsrc(), self.fsrc())
            } else {
                (self.vsrc(false), self.vsrc(false))
            };
            format!("v_cmp_{op}_{ty}_e64 {}, {a}, {b}", self.mask_reg())
        }
    }

    fn memory_op(&mut self) -> String {
        let address = if self.gpu {
            format!("v_lshl_add_u32 v9, v0, 2, {}\n", 64 * self.rng.below(2))
        } else {
            format!("v_and_b32_e32 v9, 0xfc, {}\n", self.vreg())
        };
        let offset = 4 * self.rng.below(16);
        if self.rng.chance(4) {
            return address + &self.typed_load();
        }
        let op = match self.rng.below(7) {
            0 if self.rng.chance(2) => {
                let (op, limit) = if self.rng.chance(3) {
                    ("ds_write2st64_b32", 16)
                } else {
                    ("ds_write2_b32", 64)
                };
                let (a, b) = (self.rng.below(limit), self.rng.below(limit));
                format!(
                    "{op} v9, {}, {} offset0:{a} offset1:{b}",
                    self.vreg(),
                    self.vreg()
                )
            }
            1 if self.rng.chance(2) => {
                let (op, limit) = if self.rng.chance(3) {
                    ("ds_read2st64_b32", 16)
                } else {
                    ("ds_read2_b32", 64)
                };
                let (a, b) = (self.rng.below(limit), self.rng.below(limit));
                let low = 1 + self.rng.below(if self.gpu { 3 } else { 7 });
                format!("{op} v[{low}:{}], v9 offset0:{a} offset1:{b}", low + 1)
            }
            0 => format!("ds_write_b32 v9, {} offset:{offset}", self.vreg()),
            1 => format!("ds_read_b32 {}, v9 offset:{offset}", self.vdst()),
            2 => format!(
                "buffer_store_dword {}, v9, s[20:23], {} offen offset:{offset}",
                self.vreg(),
                self.rng.pick(&["0", "s17"])
            ),
            3 => format!(
                "buffer_load_dword {}, v9, s[20:23], {} offen offset:{offset}",
                self.vdst(),
                self.rng.pick(&["0", "s17"])
            ),
            4 => format!(
                "global_store_dword v9, {}, s[24:25] offset:{offset}",
                self.vreg()
            ),
            5 => format!(
                "global_load_dword {}, v9, s[24:25] offset:{offset}",
                self.vdst()
            ),
            _ => {
                if self.rng.chance(2) {
                    format!("s_load_dword {}, s[24:25], 0x{:x}", self.sreg(), offset)
                } else {
                    let first = 2 * self.rng.below(8);
                    format!(
                        "s_load_dwordx2 s[{}:{}], s[24:25], 0x{:x}",
                        first,
                        first + 1,
                        offset
                    )
                }
            }
        };
        address + &op
    }

    fn straight(&mut self, count: usize) -> String {
        let mut lines = Vec::new();
        for _ in 0..count {
            lines.push(match self.rng.below(10) {
                0..=2 => self.scalar(),
                3..=7 => self.vector(),
                8 => self.memory_op(),
                _ => {
                    if self.rng.chance(4) {
                        format!("v_cmpx_gt_u32_e32 {}, {}", self.vsrc(true), self.vreg())
                    } else if self.rng.chance(3) {
                        let (op, source) = if self.wave64 {
                            ("s_andn2_wrexec_b64", self.pair())
                        } else {
                            ("s_andn2_wrexec_b32", self.sreg())
                        };
                        let target = self.mask_reg();
                        format!("{op} {target}, {source}")
                    } else if self.rng.chance(5) {
                        let first = 2 * self.rng.below(8);
                        format!("s_getpc_b64 s[{first}:{}]", first + 1)
                    } else if self.rng.chance(2) {
                        self.export()
                    } else if self.rng.chance(3) {
                        let which = self.rng.pick(&["LO", "HI"]);
                        format!(
                            "s_setreg_imm32_b32 hwreg(HW_REG_FLAT_SCR_{which}), 0x{:x}",
                            self.rng.word()
                        )
                    } else {
                        let mov = if self.wave64 {
                            "s_mov_b64"
                        } else {
                            "s_mov_b32"
                        };
                        format!("{mov} {}, -1", self.exec())
                    }
                }
            });
        }
        lines.join("\n")
    }

    fn words(&self, source: &str) -> usize {
        rdna::assemble(source, !self.wave64)
            .unwrap_or_else(|e| panic!("{e:?}\n{source}"))
            .len()
            / 4
    }

    pub fn program(&mut self) -> String {
        let mut parts = Vec::new();
        let (vcc, exec) = (self.vcc(), self.exec());
        let (save, and_save, andn2, or) = if self.wave64 {
            ("s[30:31]", "s_and_saveexec_b64", "s_andn2_b64", "s_or_b64")
        } else {
            ("s30", "s_and_saveexec_b32", "s_andn2_b32", "s_or_b32")
        };
        for _ in 0..1 + self.rng.below(6) {
            let count = 1 + self.rng.below(6);
            parts.push(match self.rng.below(5) {
                0 => {
                    let body = self.straight(count);
                    let other = self.straight(count);
                    let compare = self.compare_into_vcc();
                    format!("{compare}\n{and_save} {save}, {vcc}\n{body}\n{andn2} {exec}, {save}, {exec}\n{other}\n{or} {exec}, {exec}, {save}")
                }
                1 => {
                    let body = self.straight(count);
                    let compare = self.compare_into_vcc();
                    let skip = self.words(&body);
                    format!("{compare}\n{and_save} {save}, {vcc}\ns_cbranch_execz {skip}\n{body}\n{or} {exec}, {exec}, {save}")
                }
                2 => {
                    let body = self.straight(count);
                    let trips = 1 + self.rng.below(4);
                    let back = self.words(&body) + 3;
                    format!("s_mov_b32 s28, {trips}\n{body}\ns_sub_u32 s28, s28, 1\ns_cmp_lg_u32 s28, 0\ns_cbranch_scc1 -{back}")
                }
                3 => {
                    let body = self.straight(count);
                    let condition = self.rng.pick(&["s_cbranch_scc0", "s_cbranch_scc1", "s_cbranch_vccz", "s_cbranch_vccnz", "s_cbranch_execnz"]);
                    let skip = self.words(&body);
                    format!("{condition} {skip}\n{body}")
                }
                _ => self.straight(count),
            });
        }
        parts.push("s_endpgm".into());
        parts.join("\n")
    }

    fn compare_into_vcc(&mut self) -> String {
        format!(
            "v_cmp_gt_u32_e32 {}, {}, {}",
            self.vcc(),
            self.vsrc(true),
            self.vreg()
        )
    }
}
