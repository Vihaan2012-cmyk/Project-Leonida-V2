use shader_ir::{Expr, Function, Machine, Stmt, free_writes, lift};
use testkit::random::{Generator, Rng, memory, start};

/// The function with junk written to every lane of a free write's register just before it, so lanes the
/// write skips hold junk afterwards instead of their old values.
fn scrambled(function: &Function, free: &[Vec<bool>]) -> (Function, usize) {
    let mut out = function.clone();
    let lanes = if function.wave64 { 64 } else { 32 };
    let mut temps = function.temps;
    let mut count = 0;
    for (block, flags) in out.blocks.iter_mut().zip(free) {
        let mut stmts = Vec::new();
        for (at, (stmt, free)) in block.stmts.iter().zip(flags).enumerate() {
            if let (true, Stmt::SetLane(reg, _)) = (free, stmt) {
                count += 1;
                for lane in 0..lanes {
                    let (which, junk) = (temps, temps + 1);
                    temps += 2;
                    stmts.push(Stmt::Let(which, Expr::Const(lane)));
                    stmts.push(Stmt::Let(
                        junk,
                        Expr::Const(0x7BAD_0000 ^ lane << 8 ^ at as u32),
                    ));
                    stmts.push(Stmt::WriteLane(*reg, which, junk));
                }
            }
            stmts.push(stmt.clone());
        }
        block.stmts = stmts;
    }
    out.temps = temps;
    (out, count)
}

fn machine(wave64: bool, seed: u64) -> Machine {
    let wave = start(wave64, seed);
    Machine {
        sgpr: wave.sgpr,
        vgpr: wave.vgpr.clone(),
        scc: wave.scc,
        lds: wave.lds.clone(),
        memory: memory(seed).bytes,
        flush_denormals: wave.flush_denormals,
        canonical_nan: wave.canonical_nan,
        exports: wave.exports.clone(),
        messages: wave.messages.clone(),
        code_base: wave.code_base,
        attributes: wave.attributes.clone(),
    }
}

#[test]
fn junk_in_lanes_free_writes_skip_never_shows() {
    let mut freed = 0;
    for seed in 1..=1500u64 {
        for wave64 in [false, true] {
            let mut generator = Generator::new(
                Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1),
                wave64,
                false,
            );
            let source = generator.program();
            let code =
                rdna::assemble(&source, !wave64).unwrap_or_else(|e| panic!("{e:?}\n{source}"));
            let function = lift(&code, wave64).unwrap_or_else(|e| panic!("{e}\n{source}"));
            let (junk, count) = scrambled(&function, &free_writes(&function, false));
            freed += count;
            let mut plain = machine(wave64, seed);
            let mut scrambled = machine(wave64, seed);
            let expected = shader_ir::run(&function, &mut plain);
            let actual = shader_ir::run(&junk, &mut scrambled);
            let context = || format!("{source}\n{function}");
            assert_eq!(
                expected.is_ok(),
                actual.is_ok(),
                "{expected:?} vs {actual:?}\n{}",
                context()
            );
            if expected.is_err() {
                continue;
            }
            assert!(plain.memory == scrambled.memory, "memory\n{}", context());
            assert!(plain.lds == scrambled.lds, "lds\n{}", context());
            assert!(plain.exports == scrambled.exports, "exports\n{}", context());
            assert_eq!(
                plain.messages,
                scrambled.messages,
                "messages\n{}",
                context()
            );
        }
    }
    assert!(freed > 3000, "{freed}");
}

#[test]
fn writes_read_after_exec_returns_keep_their_lanes() {
    let source = "s_mov_b64 s[10:11], exec
                  v_cmpx_gt_u32_e32 4, v0
                  v_mov_b32_e32 v1, 7
                  v_mov_b32_e32 v2, 9
                  v_mov_b32_e32 v3, v2
                  s_mov_b64 exec, s[10:11]
                  v_mov_b32_e32 v4, v1
                  s_endpgm";
    let code = rdna::assemble(source, false).unwrap();
    let function = lift(&code, true).unwrap();
    let free = free_writes(&function, false);
    let writes: Vec<(u8, bool)> = function
        .blocks
        .iter()
        .zip(&free)
        .flat_map(|(block, flags)| block.stmts.iter().zip(flags))
        .filter_map(|(stmt, free)| match stmt {
            Stmt::SetLane(reg, _) => Some((*reg, *free)),
            _ => None,
        })
        .collect();
    assert_eq!(writes, [(1, false), (2, true), (3, true), (4, true)]);
    let harness = free_writes(&function, true);
    assert!(harness.iter().flatten().all(|free| !free));
}

#[test]
fn reads_across_lanes_keep_skipped_lanes() {
    for (read, wave64) in [
        ("v_readlane_b32 s0, v1, 10", true),
        (
            "v_mov_b32_dpp v2, v1 quad_perm:[1,0,3,2] row_mask:0xf bank_mask:0xf",
            false,
        ),
        (
            "v_lshlrev_b32_e32 v3, 2, v0\nds_bpermute_b32 v2, v3, v1",
            false,
        ),
    ] {
        let mask = if wave64 {
            "s_mov_b64 s[10:11], exec"
        } else {
            "s_mov_b32 s10, exec_lo"
        };
        let source = format!(
            "{mask}\nv_cmpx_gt_u32_e32 4, v0\nv_mov_b32_e32 v1, 9\n{read}\ns_waitcnt lgkmcnt(0)\nexp param0 v2, v2, v2, v2\ns_endpgm"
        );
        let code = rdna::assemble(&source, !wave64).unwrap();
        let function = lift(&code, wave64).unwrap();
        let free = free_writes(&function, false);
        let first = function
            .blocks
            .iter()
            .zip(&free)
            .flat_map(|(block, flags)| block.stmts.iter().zip(flags))
            .find_map(|(stmt, free)| match stmt {
                Stmt::SetLane(1, _) => Some(*free),
                _ => None,
            });
        assert_eq!(first, Some(false), "{source}");
    }
}
