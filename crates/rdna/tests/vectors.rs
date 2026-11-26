fn words(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|word| word.to_le_bytes()).collect()
}

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

fn texts(code: &[u8], wave32: bool) -> Vec<String> {
    let decoded = rdna::decode_all(code, wave32, 0);
    assert_eq!(decoded.invalid, 0);
    decoded.insts.iter().map(rdna::Inst::text).collect()
}

fn one(code: &[u8]) -> String {
    let all = texts(code, true);
    assert_eq!(all.len(), 1, "{all:?}");
    all[0].clone()
}

#[test]
fn llvm_flat_scratch_global_vectors() {
    for (code, expected) in [
        ("004030dc00000101", "scratch_load_dword v1, off, s1"),
        (
            "204030dc00000101",
            "scratch_load_dword v1, off, s1 offset:32",
        ),
        ("004030dc00007e01", "scratch_load_dword v1, off, exec_lo"),
        ("004030dc00007c01", "scratch_load_dword v1, off, m0"),
        ("004070dc00020100", "scratch_store_dword off, v2, s1"),
        (
            "0c4070dc00020100",
            "scratch_store_dword off, v2, s1 offset:12",
        ),
        ("004064dc00020100", "scratch_store_byte_d16_hi off, v2, s1"),
        ("004030dc05000101", "scratch_load_dword v1, off, s1"),
        ("004030dc00007f01", "scratch_load_dword v1, off, off"),
        ("004038dc00007f01", "scratch_load_dwordx4 v[1:4], off, off"),
        (
            "ff4770dc00027f00",
            "scratch_store_dword off, v2, off offset:2047",
        ),
        ("004030dc02007d01", "scratch_load_dword v1, v2, off"),
        ("005030dc00007d05", "scratch_load_dword v5, v0, off dlc"),
        (
            "ff4f70dc00027d00",
            "scratch_store_dword v0, v2, off offset:-1",
        ),
        (
            "00a430dc02000400",
            "global_load_dword v2, s[4:5] offset:1024 lds",
        ),
        (
            "00a430dc02007d00",
            "global_load_dword v[2:3], off offset:1024 lds",
        ),
        (
            "00a424dc02000400",
            "global_load_sbyte v2, s[4:5] offset:1024 lds",
        ),
        (
            "006430dc02007d00",
            "scratch_load_dword v2, off offset:1024 lds",
        ),
        (
            "006430dc00000400",
            "scratch_load_dword off, s4 offset:1024 lds",
        ),
        (
            "006430dc00007f00",
            "scratch_load_dword off, off offset:1024 lds",
        ),
        (
            "00a430dc02000407",
            "global_load_dword v2, s[4:5] offset:1024 lds",
        ),
        (
            "008430dc02000401",
            "global_load_dword v1, v2, s[4:5] offset:1024",
        ),
    ] {
        assert_eq!(one(&hex(code)), expected, "{code}");
    }
}

#[test]
fn scalar_program_flow() {
    let code = words(&[0xBF8C0070, 0xBF85FFFC, 0xBF82FFEC, 0xBF810000]);
    assert_eq!(
        texts(&code, true),
        [
            "s_waitcnt vmcnt(0) lgkmcnt(0)",
            "s_cbranch_scc1 -4",
            "s_branch -20",
            "s_endpgm"
        ]
    );
    let pcs: Vec<usize> = rdna::decode_all(&code, true, 0)
        .insts
        .iter()
        .map(|inst| inst.pc)
        .collect();
    assert_eq!(pcs, [0, 4, 8, 12]);
}

#[test]
fn ttracedata_imm_prints_hex() {
    for (word, expected) in [
        (0xBFA80000, "s_ttracedata_imm 0x0"),
        (0xBFA80064, "s_ttracedata_imm 0x64"),
        (0xBFA8FFFF, "s_ttracedata_imm 0xffff"),
    ] {
        assert_eq!(one(&words(&[word])), expected);
    }
}

#[test]
fn scalar_literals_and_hwreg() {
    let mov = (0x17D << 23) | (4 << 16) | (3 << 8) | 0xFF;
    let add = (0b10 << 30) | (5 << 16) | (129 << 8) | 4;
    let cmp = (0x17E << 23) | (6 << 16) | (4 << 8) | 5;
    let setreg = (0xB << 28) | (0x15 << 23) | (1 | (2 << 6) | (1 << 11));
    assert_eq!(
        texts(&words(&[mov, 0x12345678, add, cmp, setreg, 3]), true),
        [
            "s_mov_b32 s4, 0x12345678",
            "s_add_u32 s5, s4, 1",
            "s_cmp_eq_u32 s5, s4",
            "s_setreg_imm32_b32 hwreg(HW_REG_MODE, 2, 2), 3",
        ]
    );
}

#[test]
fn vector_32_bit_forms() {
    let mov = (0x3F << 25) | (2 << 17) | (1 << 9) | 242;
    let readfirstlane = (0x3F << 25) | (7 << 17) | (2 << 9) | (256 + 2);
    let fmaak = (0x2D << 25) | (8 << 17) | (3 << 9) | (256 + 2);
    let cmp = (0x3E << 25) | (0xC2 << 17) | (2 << 9) | 128;
    assert_eq!(
        texts(&words(&[mov, readfirstlane, fmaak, 0x3F800000, cmp]), true),
        [
            "v_mov_b32_e32 v2, 1.0",
            "v_readfirstlane_b32 s7, v2",
            "v_fmaak_f32 v8, v2, v3, 0x3f800000",
            "v_cmp_eq_u32_e32 vcc_lo, 0, v2",
        ]
    );
    assert_eq!(
        texts(&words(&[cmp]), false),
        ["v_cmp_eq_u32_e32 vcc, 0, v2"]
    );
}

#[test]
fn packed_math_modifiers() {
    assert_eq!(
        one(&words(&[0xCC165008, 0x0C0E0501])),
        "v_dot4_i32_i8 v8, v1, v2, v3 op_sel:[0,1,0] op_sel_hi:[1,0,1]"
    );
    for (index, name) in ["v_fma_mix_f32", "v_fma_mixlo_f16", "v_fma_mixhi_f16"]
        .iter()
        .enumerate()
    {
        assert_eq!(
            one(&words(&[0xCC200611 + ((index as u32) << 16), 0xA45E2B13])),
            format!("{name} v17, -v19, |v21|, -|v23|")
        );
    }
    assert_eq!(
        one(&words(&[0xD5400001, 0x04120702])),
        "v_mad_legacy_f32 v1, v2, v3, v4"
    );
}

#[test]
fn memory_and_export() {
    let smem = words(&[(0x3D << 26) | (0x0B << 18) | (100 << 6), 125 << 25]);
    assert_eq!(
        one(&smem),
        "s_buffer_load_dwordx8 [s100,s101,s102,s103,s104,s105,vcc_lo,vcc_hi], s[0:3], null"
    );
    assert_eq!(
        one(&[0x00, 0x10, 0x0C, 0xE2, 0x05, 0x08, 0x01, 0x80]),
        "buffer_load_format_d16_xyzw v[8:9], v5, s[4:7], 0 offen"
    );
    let bvh = [
        0x07, 0x9F, 0x98, 0xF1, 0x02, 0x14, 0x01, 0x00, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09,
        0x0A, 0x0B, 0x0C, 0x00, 0x00,
    ];
    assert_eq!(
        one(&bvh),
        "image_bvh_intersect_ray v[20:23], [v2,v3,v4,v5,v6,v7,v8,v9,v10,v11,v12], s[4:7]"
    );
    let export = words(&[
        (0x3E << 26) | (1 << 12) | (1 << 11) | 0xF,
        (1 << 8) | (2 << 16) | (3 << 24),
    ]);
    assert_eq!(one(&export), "exp mrt0 v0, v1, v2, v3 done vm");
    let interp = (0x32 << 26) | (2 << 18) | (1 << 10) | (1 << 8);
    assert_eq!(one(&words(&[interp])), "v_interp_p1_f32 v2, v0, attr1.y");
    let global = words(&[
        (0x37 << 26) | (0x1C << 18) | (2 << 14) | 0xFFC,
        (125 << 16) | (1 << 8) | 4,
    ]);
    assert_eq!(one(&global), "global_store_dword v[4:5], v1, off offset:-4");
    let atomic = words(&[0xF0001008 | (0x17 << 18) | (1 << 8), 0x00021004]);
    assert_eq!(
        one(&atomic),
        "image_atomic_umax v16, v[4:5], s[8:15] dmask:0x1 dim:SQ_RSRC_IMG_2D unorm"
    );
}

#[test]
fn listing_matches_llvm_layout_and_counts_invalid_words() {
    let (text, invalid) = rdna::disassemble(&words(&[0xBF800000, 0xBF810000]), true, 0x100);
    assert_eq!(
        text,
        format!(
            "\t{:<40}; encoding: [0x00,0x00,0x80,0xbf]\n\t{:<40}; encoding: [0x00,0x00,0x81,0xbf]\n",
            "s_nop 0", "s_endpgm"
        )
    );
    assert_eq!(invalid, 0);
    let decoded = rdna::decode_all(&words(&[0xFFFFFFFF, 0xBF810000]), true, 0);
    assert_eq!(decoded.invalid, 1);
    assert_eq!(decoded.insts.len(), 1);
}

#[test]
fn ds_swizzle_offsets_round_trip_through_text() {
    for offset in [
        0x8000u32 | 0b11_10_01_00,
        0x801B,
        0x041F,
        0x3C1F,
        0x0010 | (3 << 5),
        0x7FFF & 0x37BF,
        0x1F | (5 << 10),
    ] {
        let word0 = 0xD800_0000u32 | (53 << 18) | (offset & 0xFFFF);
        let code: Vec<u8> = [word0, 0x0100_0002]
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect();
        let inst = rdna::decode_one(&code, 0, true).unwrap();
        let text = inst.text();
        let again = rdna::assemble(&text, true).unwrap_or_else(|e| panic!("{text}: {e:?}"));
        assert_eq!(again, code, "{text}");
    }
}
