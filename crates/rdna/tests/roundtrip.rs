struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }

    fn src(&mut self) -> u32 {
        const CODES: [u32; 18] = [
            0, 5, 104, 105, 106, 107, 124, 125, 126, 127, 128, 192, 193, 208, 242, 248, 253, 255,
        ];
        if self.next().is_multiple_of(3) {
            CODES[(self.next() as usize) % CODES.len()]
        } else {
            256 + self.next() % 256
        }
    }

    fn vsrc(&mut self) -> u32 {
        match self.next() % 6 {
            0 => [249, 250, 233, 234][(self.next() % 4) as usize],
            _ => self.src(),
        }
    }

    fn ssrc(&mut self) -> u32 {
        const CODES: [u32; 12] = [0, 7, 104, 106, 107, 124, 125, 126, 128, 193, 242, 255];
        CODES[(self.next() as usize) % CODES.len()]
    }
}

fn bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|word| word.to_le_bytes()).collect()
}

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

fn fixpoint(code: &[u8], wave32: bool, failures: &mut Vec<String>) -> bool {
    let Ok(first) = rdna::decode_one(code, 0, wave32) else {
        return false;
    };
    let text = first.text();
    match rdna::assemble(&text, wave32) {
        Ok(again) => match rdna::decode_one(&again, 0, wave32) {
            Ok(second) if second.text() == text => {}
            Ok(second) => failures.push(format!("{text}  =>  {}", second.text())),
            Err(_) => failures.push(format!("{text}  =>  invalid bytes")),
        },
        Err(error) => failures.push(format!("{error}")),
    }
    true
}

fn random_words(rng: &mut Rng, family: u32) -> Vec<u32> {
    let w0 = match family {
        0 => 0xBE80_0000 | ((rng.next() % 106) << 16) | ((rng.next() & 0x7F) << 8) | rng.ssrc(),
        1 => {
            0x8000_0000
                | ((rng.next() % 0x36) << 23)
                | ((rng.next() % 106) << 16)
                | (rng.ssrc() << 8)
                | rng.ssrc()
        }
        2 => 0xBF00_0000 | ((rng.next() % 0x14) << 16) | (rng.ssrc() << 8) | rng.ssrc(),
        3 => {
            0xB000_0000
                | ((rng.next() % 0x1C) << 23)
                | ((rng.next() % 106) << 16)
                | (rng.next() & 0xFFFF)
        }
        4 => 0xBF80_0000 | ((rng.next() % 0x30) << 16) | (rng.next() & 0xFFFF),
        5 => 0x7E00_0000 | ((rng.next() & 0xFF) << 17) | ((rng.next() & 0xFF) << 9) | rng.vsrc(),
        6 => {
            ((rng.next() % 0x3E) << 25)
                | ((rng.next() & 0xFF) << 17)
                | ((rng.next() & 0xFF) << 9)
                | rng.vsrc()
        }
        7 => 0x7C00_0000 | ((rng.next() & 0xFF) << 17) | ((rng.next() & 0xFF) << 9) | rng.vsrc(),
        8 => {
            0xD400_0000
                | ((rng.next() & 0x3FF) << 16)
                | (rng.next() & 0x80FF)
                | ((rng.next() & 0xF) << 8)
                | ((rng.next() & 0xF) << 11)
        }
        9 => 0xCC00_0000 | ((rng.next() & 0x7F) << 16) | (rng.next() & 0xFFFF),
        10 => 0xD800_0000 | ((rng.next() & 0xFF) << 18) | (rng.next() & 0x2FFFF),
        11 => {
            0xDC00_0000
                | ((rng.next() & 0x7F) << 18)
                | ((rng.next() % 3) << 14)
                | (rng.next() & 0x33FFF)
        }
        12 => 0xE000_0000 | ((rng.next() & 0x7F) << 18) | (rng.next() & 0x1FFFF),
        13 => 0xE800_0000 | ((rng.next() & 0x7) << 16) | (rng.next() & 0x03F8_FFFF),
        14 => {
            0xF000_0000
                | ((rng.next() & 0x7F) << 18)
                | (rng.next() & 1)
                | (rng.next() & 0x0203_FFF8)
                | (u32::from(rng.next().is_multiple_of(4)) * ((rng.next() & 3) << 1))
        }
        15 => 0xF400_0000 | ((rng.next() & 0xFF) << 18) | (rng.next() & 0x1_7FFF),
        16 => 0xF800_0000 | (rng.next() & 0x1FFF),
        _ => 0xC800_0000 | (rng.next() & 0x03FC_FFFF) | ((rng.next() % 3) << 16),
    };
    let w1 = match family {
        8 | 9 => rng.src() | (rng.src() << 9) | (rng.src() << 18) | ((rng.next() & 0x1F) << 27),
        11 => {
            let saddr = [0x7D, 0x7F, rng.next() % 106][(rng.next() % 3) as usize];
            (rng.next() & 0xFF00_FFFF) | (saddr << 16)
        }
        12 | 13 => {
            (rng.next() & 0x00E0_FFFF) | ((rng.next() % 26) << 16) | ((rng.ssrc() % 255) << 24)
        }
        14 => (rng.next() & 0xC000_FFFF) | ((rng.next() % 26) << 16) | ((rng.next() % 26) << 21),
        15 => {
            (rng.next() & 0x001F_FFFF) | ([125, rng.next() % 106][(rng.next() % 2) as usize] << 25)
        }
        _ => rng.next(),
    };
    vec![w0, w1, rng.next(), rng.next(), rng.next()]
}

#[test]
fn random_instructions_survive_decode_assemble_decode() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut failures = Vec::new();
    let mut checked = 0;
    for _ in 0..80_000 {
        let family = rng.next() % 18;
        let code = bytes(&random_words(&mut rng, family));
        let wave32 = rng.next().is_multiple_of(2);
        if fixpoint(&code, wave32, &mut failures) {
            checked += 1;
        }
    }
    assert!(checked > 40_000, "only {checked} checked");
    assert!(
        failures.is_empty(),
        "{} of {checked} failed, first ones:\n{}",
        failures.len(),
        failures
            .iter()
            .take(30)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
#[test]
fn llvm_mc_listings_reassemble_to_the_same_bytes() {
    for line in include_str!("golden/llvm_mc.txt")
        .lines()
        .filter(|line| !line.starts_with('#'))
    {
        let mut fields = line.splitn(3, '\t');
        let (_, code, text) = (
            fields.next().unwrap(),
            fields.next().unwrap(),
            fields.next().unwrap(),
        );
        assert_eq!(rdna::assemble(text, true).unwrap(), hex(code), "{text}");
    }
}

#[test]
fn hand_written_programs_assemble() {
    let program = "
        s_mov_b32 s4, 0x12345678       ; literal
        v_mov_b32_e32 v2, 1.0
        v_fmaak_f32 v8, v2, v3, 0x3f800000
        v_cmp_eq_u32_e32 vcc_lo, 0, v2
        v_mad_legacy_f32 v1, v2, v3, v4
        global_store_dword v[4:5], v1, off offset:-4
        exp mrt0 v0, v1, v2, v3 done vm
        s_waitcnt vmcnt(0) lgkmcnt(0)
        s_endpgm
    ";
    let code = rdna::assemble(program, true).unwrap();
    let decoded = rdna::decode_all(&code, true, 0);
    assert_eq!(decoded.invalid, 0);
    let texts: Vec<String> = decoded.insts.iter().map(rdna::Inst::text).collect();
    assert_eq!(
        texts,
        [
            "s_mov_b32 s4, 0x12345678",
            "v_mov_b32_e32 v2, 1.0",
            "v_fmaak_f32 v8, v2, v3, 0x3f800000",
            "v_cmp_eq_u32_e32 vcc_lo, 0, v2",
            "v_mad_legacy_f32 v1, v2, v3, v4",
            "global_store_dword v[4:5], v1, off offset:-4",
            "exp mrt0 v0, v1, v2, v3 done vm",
            "s_waitcnt vmcnt(0) lgkmcnt(0)",
            "s_endpgm",
        ]
    );
    let error = rdna::assemble("s_nop 0\nv_frobnicate v0", true).unwrap_err();
    assert_eq!(error.line, 2);
}
