#![allow(unsafe_code)]
mod common;

use common::{Buf, Ctx, noise};
use gpu_passes::bc1::{Bc1Tables, LANES, Lanes, rebuild_cpu};
use gpu_passes::{Barrier, Bc1Job, Passes};

fn context() -> Option<Ctx> {
    let ctx = Ctx::new();
    if ctx.is_none() {
        eprintln!("skipping: no Vulkan GPU");
    }
    ctx
}

fn tables() -> Bc1Tables {
    let mut quadrant = [0u8; 256];
    for (i, q) in quadrant.iter_mut().enumerate() {
        *q = (i as u8).wrapping_mul(37).wrapping_add(11);
    }
    let mut history = [None; 256];
    for (entry, h) in history.iter_mut().enumerate().skip(1) {
        *h = Some([
            (entry % 32) as u8,
            (entry % 64) as u8,
            (entry * 3 % 32) as u8,
            (entry * 5 % 32) as u8,
            (entry * 11 % 64) as u8,
            (entry * 13 % 32) as u8,
        ]);
    }
    Bc1Tables { quadrant, history }
}

fn run_bc1(
    ctx: &Ctx,
    passes: &Passes,
    lanes: &[&[u8]; LANES],
    tables: &Bc1Tables,
    max_chain: u32,
) -> (Vec<u8>, [u32; 2]) {
    let packed = Lanes::pack(lanes).unwrap();
    let lane_bytes: Vec<u8> = packed.words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let table_bytes: Vec<u8> = tables.pack().iter().flat_map(|w| w.to_le_bytes()).collect();
    let lanes_buf = ctx.buffer(&lane_bytes);
    let tables_buf = ctx.buffer(&table_bytes);
    let out_len = packed.blocks as usize * 8;
    let out: Buf = ctx.buffer(&vec![0u8; out_len]);
    let status = ctx.buffer(&[0u8; 8]);
    let job = Bc1Job {
        blocks: packed.blocks,
        lane_stride: packed.lane_stride,
        max_chain,
        lanes_offset: 0,
        out_offset: 0,
    };
    ctx.run(|cmd| {
        // SAFETY: `cmd` is recording and the buffers outlive the submission; `status` starts zeroed.
        unsafe {
            passes
                .record_bc1(
                    cmd,
                    gpu_passes::Bc1Buffers {
                        lanes: lanes_buf.buffer,
                        tables: tables_buf.buffer,
                        out: out.buffer,
                        status: status.buffer,
                    },
                    job,
                    Barrier::HOST_READ,
                )
                .unwrap()
        }
    });
    let bytes = ctx.read(&out, out_len);
    let s = ctx.read(&status, 8);
    for b in [lanes_buf, tables_buf, out, status] {
        ctx.free(b);
    }
    (
        bytes,
        [
            u32::from_le_bytes(s[0..4].try_into().unwrap()),
            u32::from_le_bytes(s[4..8].try_into().unwrap()),
        ],
    )
}

#[test]
fn bc1_rebuild_matches_the_cpu_reference() {
    let Some(ctx) = context() else { return };
    let passes = Passes::new(&ctx.device, 8).unwrap();
    let tables = tables();
    for blocks in [1usize, 5, 64, 1000, 5000] {
        let mut lanes: Vec<Vec<u8>> = (0..LANES).map(|l| noise(blocks, 40 + l as u64)).collect();
        // Lane 0 mixes absolute blocks, short back references and references into the built-in history.
        for (i, v) in lanes[0].iter_mut().enumerate() {
            *v = match i % 5 {
                0 => 0,
                1 | 2 => 2,
                3 => (*v % 40) * 2,
                _ => *v,
            };
        }
        lanes[1].fill(0);
        let refs: [&[u8]; LANES] = std::array::from_fn(|i| lanes[i].as_slice());
        let want = rebuild_cpu(&refs, &tables).unwrap();
        let (got, status) = run_bc1(&ctx, &passes, &refs, &tables, 1 << 20);
        assert!(got == want, "{blocks} blocks");
        assert_eq!(status, [0, 0]);
        passes.reset().unwrap();
    }
}

#[test]
fn bc1_flags_blocks_the_gpu_cannot_resolve() {
    let Some(ctx) = context() else { return };
    let passes = Passes::new(&ctx.device, 8).unwrap();
    let mut tables = tables();
    // Every block follows its predecessor (d = 1), so a chain is as long as the block's index.
    let blocks = 40;
    let mut lanes: Vec<Vec<u8>> = (0..LANES).map(|l| noise(blocks, 70 + l as u64)).collect();
    lanes[0].fill(2);
    lanes[1].fill(0);
    let refs: [&[u8]; LANES] = std::array::from_fn(|i| lanes[i].as_slice());
    let want = rebuild_cpu(&refs, &tables).unwrap();
    let (got, status) = run_bc1(&ctx, &passes, &refs, &tables, 100);
    assert!(got == want);
    assert_eq!(status, [0, 0]);
    passes.reset().unwrap();

    let (_, status) = run_bc1(&ctx, &passes, &refs, &tables, 10);
    assert_eq!(status[0], 0);
    assert_eq!(status[1], 29);
    passes.reset().unwrap();

    tables.history[255] = None;
    let (_, status) = run_bc1(&ctx, &passes, &refs, &tables, 100);
    assert_eq!(status, [40, 0]);
}
