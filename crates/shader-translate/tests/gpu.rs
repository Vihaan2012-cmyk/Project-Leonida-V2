use std::path::PathBuf;
use std::process::Command;

use gpu::Gpu;
use shader_translate::{
    ComputeOptions, EXPORT_BUFFER_WORDS, FloatMode, HarnessOptions, MESSAGE_BASE, MESSAGE_LIMIT,
    PUSH_HEAP, PUSH_WORDS, STATE_SCC, STATE_VGPR, STATE_WORDS, harness,
};
use testkit::random::{Generator, Rng, memory, start};
use testkit::{Memory, Wave};

const LDS_WORDS: usize = 4096;

fn gpu() -> Option<Gpu> {
    match Gpu::new() {
        Ok(gpu)
            if gpu.info.subgroup_size == 32 && gpu.info.int64 && gpu.info.ballot_and_shuffle =>
        {
            Some(gpu)
        }
        Ok(gpu) => {
            eprintln!(
                "skipping: {} lacks 32-wide subgroups, int64 or ballot",
                gpu.info.name
            );
            None
        }
        Err(error) => {
            eprintln!("skipping: {error}");
            None
        }
    }
}

fn spirv_val(words: &[u32], name: &str) {
    let Ok(output) = Command::new("spirv-val").arg("--version").output() else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.spv"));
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    std::fs::write(&path, bytes).unwrap();
    let result = Command::new("spirv-val")
        .arg("--target-env")
        .arg("vulkan1.3")
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "spirv-val: {}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

fn plain(value: u32) -> u32 {
    if value & 0x7F80_0000 == 0 && value & 0x007F_FFFF != 0 {
        0
    } else {
        value
    }
}

fn without_denormals(wave: &mut Wave, memory: &mut Memory) {
    for s in &mut wave.sgpr[..16] {
        *s = plain(*s);
    }
    for v in &mut wave.vgpr[1..9] {
        for lane in v.iter_mut() {
            *lane = plain(*lane);
        }
    }
    for v in &mut wave.vgpr[5..9] {
        for lane in v.iter_mut() {
            if !f32::from_bits(*lane).is_finite() {
                *lane = 1.5f32.to_bits();
            }
        }
    }
    for word in memory.bytes.chunks_mut(4) {
        let value = plain(u32::from_le_bytes(word.try_into().unwrap()));
        word.copy_from_slice(&value.to_le_bytes());
    }
}

fn same(a: u32, b: u32) -> bool {
    let nan = |x: u32| x & 0x7F80_0000 == 0x7F80_0000 && x & 0x007F_FFFF != 0;
    let tiny = |x: u32| x & 0x7F80_0000 == 0;
    a == b || (nan(a) && nan(b)) || (tiny(a) && tiny(b))
}

fn words(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

fn run(gpu: &Gpu, source: &str, wave: Wave, memory: Memory, name: &str) -> bool {
    let wave64 = wave.wave64;
    let lanes = if wave64 { 64 } else { 32 };
    let code = rdna::assemble(source, !wave64).unwrap_or_else(|e| panic!("{e:?}\n{source}"));
    let mut state = vec![0u32; STATE_WORDS];
    state[..128].copy_from_slice(&wave.sgpr);
    state[STATE_SCC as usize] = u32::from(wave.scc);
    for (reg, lanes) in wave.vgpr.iter().enumerate() {
        let at = STATE_VGPR as usize + 64 * reg;
        state[at..at + 64].copy_from_slice(lanes);
    }
    let mut gpu_memory = words(&memory.bytes);
    let mut lds = words(&wave.lds[..4 * LDS_WORDS]);
    let mut exports = vec![0u32; EXPORT_BUFFER_WORDS];
    let (mut reference, mut reference_memory) = (wave, memory);
    reference.flush_denormals = !gpu.info.denorm_preserve_32;
    reference.canonical_nan = (gpu.info.vendor == 0x10DE).then_some(0x7FFF_FFFF);
    if testkit::run(&code, &mut reference, &mut reference_memory).is_err() {
        return false;
    }
    let function = shader_ir::lift(&code, wave64).unwrap_or_else(|e| panic!("{e}\n{source}"));
    let options = HarnessOptions {
        lds_words: LDS_WORDS as u32,
        nan_preserve: gpu.info.nan_preserve_32,
        flush_denormals: gpu.info.denorm_flush_32 && !gpu.info.denorm_preserve_32,
        preserve_denormals: gpu.info.denorm_preserve_32,
        code_base: reference.code_base,
    };
    let spirv = harness(&function, &options).unwrap_or_else(|e| panic!("{e}\n{source}"));
    spirv_val(&spirv, name);
    gpu.dispatch(
        &spirv,
        &mut [&mut state, &mut gpu_memory, &mut lds, &mut exports],
        [1, 1, 1],
        Some(32),
    )
    .unwrap();
    let context = || format!("{name}\n{source}\n{function}");
    if std::env::var_os("LEONIDA_DEBUG").is_some() {
        let cpu = words(&reference_memory.bytes);
        for i in 0..gpu_memory.len() {
            if gpu_memory[i] != cpu[i] {
                eprintln!("mem {i}: gpu {:#x} cpu {:#x}", gpu_memory[i], cpu[i]);
            }
        }
        for r in 1usize..9 {
            let at = STATE_VGPR as usize + 64 * r;
            eprintln!("v{r} gpu {:x?}", &state[at..at + 32]);
            eprintln!("v{r} cpu {:x?}", &reference.vgpr[r][..32]);
        }
        eprintln!(
            "s24 {:#x} {:#x} exec {:#x}",
            state[24], state[25], state[126]
        );
    }
    for (reg, (gpu, cpu)) in state.iter().zip(&reference.sgpr).enumerate() {
        assert!(
            same(*gpu, *cpu),
            "s{reg}: gpu {gpu:#x} cpu {cpu:#x}\n{}",
            context()
        );
    }
    assert_eq!(
        state[STATE_SCC as usize] != 0,
        reference.scc,
        "scc\n{}",
        context()
    );
    for (reg, cpu_lanes) in reference.vgpr.iter().enumerate() {
        let at = STATE_VGPR as usize + 64 * reg;
        for (lane, (gpu, cpu)) in state[at..at + lanes].iter().zip(cpu_lanes).enumerate() {
            assert!(
                same(*gpu, *cpu),
                "v{reg} lane {lane}: gpu {gpu:#x} cpu {cpu:#x}\n{}",
                context()
            );
        }
    }
    for (i, (a, b)) in exports.iter().zip(&reference.exports).enumerate() {
        assert!(
            same(*a, *b),
            "export word {i}: gpu {a:#x} cpu {b:#x}\n{}",
            context()
        );
    }
    let count = exports[MESSAGE_BASE as usize] as usize;
    assert_eq!(
        count,
        reference.messages.len(),
        "message count\n{}",
        context()
    );
    for (i, (code, m0)) in reference
        .messages
        .iter()
        .take(MESSAGE_LIMIT as usize)
        .enumerate()
    {
        let at = MESSAGE_BASE as usize + 1 + 2 * i;
        assert_eq!(
            (exports[at], exports[at + 1]),
            (*code, *m0),
            "message {i}\n{}",
            context()
        );
    }
    for (i, (a, b)) in lds
        .iter()
        .zip(words(&reference.lds[..4 * LDS_WORDS]))
        .enumerate()
    {
        assert!(
            same(*a, b),
            "lds word {i}: gpu {a:#x} cpu {b:#x}\n{}",
            context()
        );
    }
    for (i, (a, b)) in gpu_memory
        .iter()
        .zip(words(&reference_memory.bytes))
        .enumerate()
    {
        assert!(
            same(*a, b),
            "memory word {i}: gpu {a:#x} cpu {b:#x}\n{}",
            context()
        );
    }
    assert_eq!(gpu::validation_errors(), 0);
    true
}

fn prepared(seed: u64, wave64: bool) -> (Wave, Memory) {
    let (mut wave, mut memory) = (start(wave64, seed), memory(seed));
    without_denormals(&mut wave, &mut memory);
    (wave, memory)
}

#[test]
fn hand_written_programs_match_on_the_gpu() {
    let Some(gpu) = gpu() else { return };
    let programs = [
        "s_mov_b32 s0, 0
         s_mov_b32 s1, 1
         s_add_u32 s0, s0, s1
         s_add_u32 s1, s1, 1
         s_cmp_le_u32 s1, 10
         s_cbranch_scc1 -4
         s_endpgm",
        "v_cmp_gt_u32_e32 vcc_lo, 16, v0
         s_and_saveexec_b32 s8, vcc_lo
         v_mov_b32_e32 v1, 0x64
         s_andn2_b32 exec_lo, s8, exec_lo
         v_mov_b32_e32 v1, 0xc8
         s_or_b32 exec_lo, exec_lo, s8
         s_endpgm",
        "v_mad_u32_u24 v1, v0, 3, 1
         v_lshlrev_b32_e32 v2, 2, v0
         buffer_store_dword v1, v2, s[20:23], 0 offen
         buffer_load_dword v3, v2, s[20:23], s17 offen offset:8
         ds_write_b32 v2, v0
         v_sub_nc_u32_e32 v3, 31, v0
         v_lshlrev_b32_e32 v3, 2, v3
         ds_read_b32 v4, v3
         s_load_dwordx2 s[10:11], s[24:25], 0x10
         v_add_nc_u32_e32 v5, s10, v0
         global_store_dword v2, v5, s[24:25] offset:256
         global_load_dword v6, v2, s[24:25] offset:4
         v_mbcnt_lo_u32_b32 v7, -1, 0
         v_readlane_b32 s13, v0, 7
         v_readfirstlane_b32 s14, v3
         v_writelane_b32 v8, s14, 9
         s_endpgm",
        "v_cvt_f32_u32_e32 v1, v0
         v_mul_f32_e32 v2, 0.5, v1
         v_fma_f32 v3, v1, 2.0, -v2
         v_add_f32_e64 v4, v1, -1.0 clamp
         v_max_f32_e64 v5, -|v1|, -4.0
         v_mul_f32_e64 v6, v1, v2 mul:2
         v_fmac_f32_e32 v6, v1, v2
         v_cvt_u32_f32_e32 v7, v6
         s_endpgm",
    ];
    for (index, source) in programs.iter().enumerate() {
        let (wave, memory) = prepared(index as u64 + 11, false);
        assert!(run(&gpu, source, wave, memory, &format!("fixed{index}")));
    }
}

fn random_suite(wave64: bool) {
    let Some(gpu) = gpu() else { return };
    let mut completed = 0;
    let only: Option<u64> = std::env::var("LEONIDA_SEED")
        .ok()
        .and_then(|s| s.parse().ok());
    for seed in 1..=300u64 {
        if only.is_some_and(|o| o != seed) {
            continue;
        }
        let mut generator = Generator {
            rng: Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1),
            wave64,
            gpu: true,
        };
        let source = generator.program();
        let (wave, memory) = prepared(seed, wave64);
        let name = format!("random{}_{seed}", if wave64 { 64 } else { 32 });
        completed += usize::from(run(&gpu, &source, wave, memory, &name));
    }
    assert!(only.is_some() || completed > 290, "{completed}");
}

#[test]
fn random_wave32_programs_match_on_the_gpu() {
    random_suite(false);
}

#[test]
fn random_wave64_programs_match_on_the_gpu() {
    random_suite(true);
}

fn compute_options(gpu: &Gpu, group_size: [u32; 3], rsrc2: u32) -> ComputeOptions {
    let float = FloatMode {
        nan_preserve: gpu.info.nan_preserve_32,
        flush_denormals: gpu.info.denorm_flush_32 && !gpu.info.denorm_preserve_32,
        preserve_denormals: gpu.info.denorm_preserve_32,
    };
    ComputeOptions::from_rsrc2(rsrc2, group_size, float)
}

fn run_compute(gpu: &Gpu, spirv: &[u32], heap: &mut [u32], user: &[u32], groups: [u32; 3]) {
    let user = user.to_vec();
    gpu.dispatch_with(
        spirv,
        &mut [heap],
        |addresses| {
            let mut push = vec![0u32; PUSH_WORDS as usize];
            push[..user.len()].copy_from_slice(&user);
            push[PUSH_HEAP as usize] = addresses[0] as u32;
            push[PUSH_HEAP as usize + 1] = (addresses[0] >> 32) as u32;
            push
        },
        groups,
        Some(32),
    )
    .unwrap();
    assert_eq!(gpu::validation_errors(), 0);
}

#[test]
fn compute_entry_reads_user_sgprs_group_ids_and_thread_ids() {
    let Some(gpu) = gpu() else { return };
    for wave64 in [false, true] {
        let source = "v_lshlrev_b32_e32 v1, 2, v0
             s_lshl_b32 s3, s2, 8
             v_add_nc_u32_e32 v1, s3, v1
             v_mad_u32_u24 v2, s2, 1000, v0
             global_store_dword v1, v2, s[0:1]
             s_endpgm";
        let code = rdna::assemble(source, !wave64).unwrap();
        let function = shader_ir::lift(&code, wave64).unwrap();
        let rsrc2 = (2 << 1) | (1 << 7);
        let options = compute_options(&gpu, [64, 1, 1], rsrc2);
        let spirv = shader_translate::compute(&function, &options).unwrap();
        spirv_val(
            &spirv,
            &format!("compute_groups{}", if wave64 { 64 } else { 32 }),
        );
        let mut heap = vec![0u32; 1024];
        run_compute(&gpu, &spirv, &mut heap, &[0x100, 0], [4, 1, 1]);
        for group in 0..4u32 {
            for thread in 0..64u32 {
                let at = (0x100 + group * 256 + thread * 4) as usize / 4;
                assert_eq!(
                    heap[at],
                    group * 1000 + thread,
                    "wave64 {wave64} group {group} thread {thread}"
                );
            }
        }
    }
}

fn dump(wave64: bool) -> String {
    let mut lines = vec![
        if wave64 {
            "s_mov_b64 exec, -1"
        } else {
            "s_mov_b32 exec_lo, -1"
        }
        .to_string(),
    ];
    lines.push("v_mov_b32_e32 v11, 0".into());
    for reg in 1..=9 {
        lines.push(format!(
            "v_lshl_add_u32 v10, v0, 2, 0x{:x}",
            0x1000 + reg * 0x100
        ));
        lines.push(format!("global_store_dword v[10:11], v{reg}, off"));
    }
    for reg in 0..16 {
        lines.push(format!("v_mov_b32_e32 v12, s{reg}"));
        lines.push(format!("v_mov_b32_e32 v10, 0x{:x}", 0x1A00 + reg * 4));
        lines.push("global_store_dword v[10:11], v12, off".into());
    }
    lines.join("\n")
}

fn compute_suite(wave64: bool) {
    let Some(gpu) = gpu() else { return };
    let lanes = if wave64 { 64 } else { 32 };
    let mut completed = 0;
    for seed in 1..=200u64 {
        let mut generator = Generator {
            rng: Rng(seed.wrapping_mul(0xD1B5_4A32_D192_ED03) | 1),
            wave64,
            gpu: true,
        };
        let program = generator.program();
        let body = program
            .strip_suffix("s_endpgm")
            .expect("programs end with s_endpgm");
        let source = format!("{body}{}\ns_endpgm", dump(wave64));
        let (mut wave, memory) = prepared(seed, wave64);
        for reg in 1..256 {
            wave.vgpr[reg] = [0; 64];
        }
        let mut heap_bytes = memory.bytes.clone();
        heap_bytes.resize(0x2000, 0);
        let mut reference_memory = Memory {
            bytes: heap_bytes.clone(),
        };
        let code = rdna::assemble(&source, !wave64).unwrap_or_else(|e| panic!("{e:?}\n{source}"));
        let user: Vec<u32> = wave.sgpr[..32].to_vec();
        let mut reference = wave;
        reference.flush_denormals = !gpu.info.denorm_preserve_32;
        reference.canonical_nan = (gpu.info.vendor == 0x10DE).then_some(0x7FFF_FFFF);
        if testkit::run(&code, &mut reference, &mut reference_memory).is_err() {
            continue;
        }
        let function = shader_ir::lift(&code, wave64).unwrap_or_else(|e| panic!("{e}\n{source}"));
        let options = compute_options(&gpu, [lanes, 1, 1], (16 << 1) | (32 << 15));
        let mut options = options;
        options.user_sgprs = 32;
        let Ok(spirv) = shader_translate::compute(&function, &options) else {
            continue;
        };
        let name = format!("compute{lanes}_{seed}");
        spirv_val(&spirv, &name);
        let mut heap = words(&heap_bytes);
        run_compute(&gpu, &spirv, &mut heap, &user, [1, 1, 1]);
        for (i, (a, b)) in heap.iter().zip(words(&reference_memory.bytes)).enumerate() {
            assert!(
                same(*a, b),
                "{name} heap word {i:#x}: gpu {a:#x} cpu {b:#x}\n{source}\n{function}"
            );
        }
        completed += 1;
    }
    assert!(completed > 150, "{completed}");
}

#[test]
fn random_wave32_programs_match_as_compute_shaders() {
    compute_suite(false);
}

#[test]
fn random_wave64_programs_match_as_compute_shaders() {
    compute_suite(true);
}
