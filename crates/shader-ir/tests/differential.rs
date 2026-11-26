use shader_ir::{Machine, lift};
use testkit::{Memory, Wave};

const MEMORY: usize = 0x1000;

fn start(wave64: bool, seed: u64) -> Wave {
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
    wave.sgpr[17] = 4 * (rng.next() % 8);
    wave.sgpr[20] = 0x100;
    wave.sgpr[22] = 0x40 + 4 * (rng.next() % 0x80);
    wave.sgpr[24] = 0x400;
    wave.vgpr[9] = [0; 64];
    wave
}

fn memory(seed: u64) -> Memory {
    let mut rng = Rng(seed ^ 0x9E37_79B9_7F4A_7C15);
    let mut memory = Memory::new(MEMORY);
    for word in memory.bytes.chunks_mut(4) {
        word.copy_from_slice(&rng.value().to_le_bytes());
    }
    memory
}

fn check(source: &str, wave: Wave, memory: Memory) -> bool {
    let code = rdna::assemble(source, !wave.wave64).unwrap_or_else(|e| panic!("{e:?}\n{source}"));
    let mut machine = Machine {
        sgpr: wave.sgpr,
        vgpr: wave.vgpr.clone(),
        scc: wave.scc,
        lds: wave.lds.clone(),
        memory: memory.bytes.clone(),
    };
    let (mut reference, mut reference_memory) = (wave, memory);
    let expected = testkit::run(&code, &mut reference, &mut reference_memory);
    let function = lift(&code, reference.wave64).unwrap_or_else(|e| panic!("{e}\n{source}"));
    let actual = shader_ir::run(&function, &mut machine);
    assert_eq!(
        expected.is_ok(),
        actual.is_ok(),
        "{expected:?} vs {actual:?}\n{source}\n{function}"
    );
    let context = || format!("{source}\n{function}");
    for (reg, (a, b)) in machine.sgpr.iter().zip(&reference.sgpr).enumerate() {
        assert_eq!(a, b, "s{reg}\n{}", context());
    }
    assert_eq!(machine.scc, reference.scc, "scc\n{}", context());
    for (reg, (a, b)) in machine.vgpr.iter().zip(&reference.vgpr).enumerate() {
        assert_eq!(a, b, "v{reg}\n{}", context());
    }
    assert!(machine.lds == reference.lds, "lds\n{}", context());
    assert!(
        machine.memory == reference_memory.bytes,
        "memory\n{}",
        context()
    );
    expected.is_ok()
}

fn check_both(source: &str) {
    for wave64 in [false, true] {
        assert!(check(source, start(wave64, 7), memory(7)));
    }
}

#[test]
fn scalar_loop() {
    check_both(
        "s_mov_b32 s0, 0
         s_mov_b32 s1, 1
         s_add_u32 s0, s0, s1
         s_add_u32 s1, s1, 1
         s_cmp_le_u32 s1, 10
         s_cbranch_scc1 -4
         s_endpgm",
    );
}

#[test]
fn if_else_with_exec_masks() {
    assert!(check(
        "v_cmp_gt_u32_e32 vcc_lo, 16, v0
         s_and_saveexec_b32 s8, vcc_lo
         v_mov_b32_e32 v1, 0x64
         s_andn2_b32 exec_lo, s8, exec_lo
         v_mov_b32_e32 v1, 0xc8
         s_or_b32 exec_lo, exec_lo, s8
         s_endpgm",
        start(false, 1),
        memory(1),
    ));
    assert!(check(
        "v_cmp_gt_u32_e32 vcc, 40, v0
         s_and_saveexec_b64 s[8:9], vcc
         s_cbranch_execz 2
         v_mov_b32_e32 v1, 0x64
         s_andn2_b64 exec, s[8:9], exec
         v_mov_b32_e32 v1, 0xc8
         s_or_b64 exec, exec, s[8:9]
         s_endpgm",
        start(true, 1),
        memory(1),
    ));
}

#[test]
fn memory_programs() {
    assert!(check(
        "v_mad_u32_u24 v1, v0, 3, 1
         v_lshlrev_b32_e32 v2, 2, v0
         buffer_store_dword v1, v2, s[20:23], 0 offen
         buffer_load_dword v3, v2, s[20:23], s17 offen offset:8
         v_lshlrev_b32_e32 v2, 2, v0
         ds_write_b32 v2, v0
         v_sub_nc_u32_e32 v3, 31, v0
         v_lshlrev_b32_e32 v3, 2, v3
         ds_read_b32 v4, v3
         s_load_dwordx2 s[10:11], s[24:25], 0x10
         v_add_nc_u32_e32 v5, s10, v0
         global_store_dword v2, v5, s[24:25] offset:256
         global_load_dword v6, v2, s[24:25] offset:4
         s_endpgm",
        start(false, 3),
        memory(3),
    ));
}

#[test]
fn wave64_lane_operations() {
    assert!(check(
        "v_mbcnt_lo_u32_b32 v5, -1, 0
         v_mbcnt_hi_u32_b32 v5, -1, v5
         v_cmp_lt_u32_e64 s[10:11], v0, 40
         s_bcnt1_i32_b64 s12, s[10:11]
         v_cndmask_b32_e64 v6, 7, 9, s[10:11]
         v_readlane_b32 s13, v0, 50
         v_readfirstlane_b32 s14, v3
         v_writelane_b32 v7, s14, 33
         s_endpgm",
        start(true, 5),
        memory(5),
    ));
}

#[test]
fn float_modifiers() {
    check_both(
        "v_cvt_f32_u32_e32 v1, v0
         v_mul_f32_e32 v2, 0.5, v1
         v_fma_f32 v3, v1, 2.0, -v2
         v_add_f32_e64 v4, v1, -1.0 clamp
         v_max_f32_e64 v5, -|v1|, -4.0
         v_mul_f32_e64 v6, v1, v2 mul:2
         v_fmac_f32_e32 v6, v1, v2
         s_endpgm",
    );
}

#[test]
fn unsupported_instructions_are_reported() {
    let code = rdna::assemble("v_frexp_mant_f64_e32 v[0:1], v[2:3]", true).unwrap();
    assert!(matches!(
        lift(&code, false),
        Err(shader_ir::LiftError::Unsupported { .. })
    ));
}

#[test]
fn random_programs_match_the_reference() {
    let mut completed = 0;
    for seed in 1..=400u64 {
        for wave64 in [false, true] {
            let mut generator = Generator {
                rng: Rng(seed.wrapping_mul(0x2545_F491_4F6C_DD1D) | 1),
                wave64,
            };
            let source = generator.program();
            completed += usize::from(check(&source, start(wave64, seed), memory(seed)));
        }
    }
    assert!(completed > 780, "{completed}");
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }

    fn below(&mut self, n: usize) -> usize {
        self.next() as usize % n
    }

    fn chance(&mut self, n: u32) -> bool {
        self.next().is_multiple_of(n)
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }

    fn value(&mut self) -> u32 {
        match self.next() % 6 {
            0 => self.next() % 64,
            1 => ((self.next() % 64) as f32).to_bits(),
            2 => ((self.next() % 2000) as f32 / 16.0 - 60.0).to_bits(),
            3 => u32::MAX - self.next() % 8,
            _ => self.next() ^ (self.next() << 16),
        }
    }
}

struct Generator {
    rng: Rng,
    wave64: bool,
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
        format!("v{}", 1 + self.rng.below(8))
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
        let value = self.vsrc(false);
        match self.rng.below(6) {
            0 => format!("-{value}"),
            1 => format!("|{value}|"),
            2 => format!("-|{value}|"),
            _ => value,
        }
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
                format!("{op} {}, 0x{:x}", self.sreg(), self.rng.next() % 0x10000)
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
        match self.rng.below(14) {
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
                        self.vdst(),
                        self.vsrc(true),
                        self.vreg()
                    )
                } else {
                    let omod = self.omod();
                    format!(
                        "{op}_e64 {}, {}, {}{omod}",
                        self.vdst(),
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
                format!("{op}_e32 {}, {}", self.vdst(), self.vsrc(true))
            }
            6 => {
                let op = self.rng.pick(&[
                    "v_floor_f32",
                    "v_fract_f32",
                    "v_cvt_f32_i32",
                    "v_sqrt_f32",
                    "v_mov_b32",
                ]);
                let omod = if op == "v_mov_b32" { "" } else { self.omod() };
                let source = if op == "v_mov_b32" {
                    self.vsrc(false)
                } else {
                    self.fsrc()
                };
                format!("{op}_e64 {}, {source}{omod}", self.vdst())
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
                    self.vdst(),
                    self.fsrc(),
                    self.fsrc(),
                    self.fsrc()
                )
            }
            9 => {
                let op = self.rng.pick(&["v_fmac_f32", "v_mac_f32"]);
                format!(
                    "{op}_e32 {}, {}, {}",
                    self.vdst(),
                    self.vsrc(true),
                    self.vreg()
                )
            }
            10 => self.compare(),
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
        if self.rng.chance(2) {
            format!(
                "v_cmp_{op}_{ty}_e32 {}, {}, {}",
                self.vcc(),
                self.vsrc(true),
                self.vreg()
            )
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
        let address = format!("v_and_b32_e32 v9, 0xfc, {}\n", self.vreg());
        let offset = 4 * self.rng.below(16);
        let op = match self.rng.below(7) {
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

    fn program(&mut self) -> String {
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
