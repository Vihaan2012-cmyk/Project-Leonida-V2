//! Decoder tests against streams built here: a small XMA2 frame writer, a packetizer, and a reference
//! synthesis that uses the direct IMDCT formula and the sine window instead of the decoder's code.

use std::f64::consts::PI;

use super::frame::{FrameDecoder, tables};
use super::stream::{PACKET_SIZE, PacketHeader, Xma2Config, Xma2Decoder};
use super::tables::*;
use super::testenc::*;

/// Reference synthesis of identical-layout frames: dequantise, direct IMDCT, sine-window overlap-add.
fn reference(coefs_per_frame: &[Vec<f64>], quant_step: i32) -> Vec<f64> {
    let quant = 10f64.powf(f64::from(quant_step) / 20.0);
    let scale = 2.0 / N as f64 / 32768.0;
    let window: Vec<f64> = (0..2 * N)
        .map(|n| (PI * (n as f64 + 0.5) / (2 * N) as f64).sin())
        .collect();
    let mut prev: Option<Vec<f64>> = None;
    let mut out = Vec::new();
    for c in coefs_per_frame {
        let y: Vec<f64> = (0..2 * N)
            .map(|n| {
                (0..N)
                    .filter(|&k| c[k] != 0.0)
                    .map(|k| {
                        c[k] * quant
                            * scale
                            * (PI / N as f64 * (n as f64 + 0.5 + N as f64 / 2.0) * (k as f64 + 0.5))
                                .cos()
                    })
                    .sum()
            })
            .collect();
        for m in 0..N {
            let tail = prev.as_ref().map_or(0.0, |p| window[N + m] * p[N + m]);
            out.push(tail + window[m] * y[m]);
        }
        prev = Some(y);
    }
    out
}

fn decode(packets: &[Vec<u8>], channels: usize) -> Vec<f32> {
    let mut d = Xma2Decoder::new(Xma2Config::new(32000, channels, None)).unwrap();
    for p in packets {
        d.push_packet(p).unwrap();
    }
    let n = d.available();
    let mut out = vec![0f32; n * channels];
    d.pull_f32(&mut out);
    out
}

fn assert_close(got: &[f32], want: &[f64], what: &str) {
    assert_eq!(got.len(), want.len(), "{what}: length");
    let worst = got
        .iter()
        .zip(want)
        .map(|(&g, &w)| (f64::from(g) - w).abs())
        .fold(0.0, f64::max);
    let peak = want.iter().fold(0.0f64, |a, &b| a.max(b.abs()));
    assert!(peak > 0.01, "{what}: reference is silent");
    assert!(
        worst < 1e-5 * peak.max(1.0),
        "{what}: worst error {worst} for peak {peak}"
    );
}

#[test]
fn mono_frames_match_the_reference_synthesis() {
    let mut rng = Rng(7);
    let step = -3;
    let mut frames = Vec::new();
    let mut coefs = Vec::new();
    for _ in 0..6 {
        let t = random_tuples(&mut rng, 20);
        coefs.push(coefficients(&t));
        frames.push(finish_frame(&encode_frame(&[&t], step, false)));
    }
    let packets = packetize(&frames);
    assert_eq!(packets.len(), 1);
    let h = PacketHeader::parse(&packets[0]).unwrap();
    assert_eq!((h.frame_count, h.carry_bits), (6, 0));
    let got = decode(&packets, 1);
    assert_close(&got, &reference(&coefs, 90 + step), "mono");
}

#[test]
fn frames_that_cross_packets_decode_the_same() {
    let mut rng = Rng(99);
    let mut frames = Vec::new();
    let mut coefs = Vec::new();
    for _ in 0..70 {
        let count = 40 + (rng.next() % 40) as usize;
        let t = random_tuples(&mut rng, count);
        coefs.push(coefficients(&t));
        frames.push(finish_frame(&encode_frame(&[&t], 0, false)));
    }
    let packets = packetize(&frames);
    assert!(packets.len() >= 3);
    assert!(
        packets[1..]
            .iter()
            .any(|p| PacketHeader::parse(p).unwrap().carry_bits > 0),
        "some frame must span packets"
    );
    let got = decode(&packets, 1);
    assert_close(&got, &reference(&coefs, 90), "crossing");
}

#[test]
fn stereo_difference_transform_is_applied() {
    let mut rng = Rng(5);
    let mut frames = Vec::new();
    let mut want_l = Vec::new();
    let mut want_r = Vec::new();
    for _ in 0..4 {
        let a = random_tuples(&mut rng, 16);
        let d = random_tuples(&mut rng, 16);
        let (ca, cd) = (coefficients(&a), coefficients(&d));
        want_l.push(ca.iter().zip(&cd).map(|(x, y)| x - y).collect::<Vec<_>>());
        want_r.push(ca.iter().zip(&cd).map(|(x, y)| x + y).collect::<Vec<_>>());
        frames.push(finish_frame(&encode_frame(&[&a, &d], 0, true)));
    }
    let got = decode(&packetize(&frames), 2);
    let left: Vec<f32> = got.iter().step_by(2).copied().collect();
    let right: Vec<f32> = got.iter().skip(1).step_by(2).copied().collect();
    assert_close(&left, &reference(&want_l, 90), "left");
    assert_close(&right, &reference(&want_r, 90), "right");
}

#[test]
fn stereo_without_transform_keeps_channels_independent() {
    // no transform: channels are independent and unscaled
    let mut rng = Rng(11);
    let mut frames = Vec::new();
    let (mut lc, mut rc) = (Vec::new(), Vec::new());
    for _ in 0..3 {
        let l = random_tuples(&mut rng, 12);
        let r = random_tuples(&mut rng, 12);
        lc.push(coefficients(&l));
        rc.push(coefficients(&r));
        frames.push(finish_frame(&encode_frame(&[&l, &r], 2, false)));
    }
    let got = decode(&packetize(&frames), 2);
    let left: Vec<f32> = got.iter().step_by(2).copied().collect();
    let right: Vec<f32> = got.iter().skip(1).step_by(2).copied().collect();
    assert_close(&left, &reference(&lc, 92), "left");
    assert_close(&right, &reference(&rc, 92), "right");
}

#[test]
fn pull_in_small_pieces_and_i16_conversion() {
    let mut rng = Rng(3);
    let t = random_tuples(&mut rng, 30);
    let frames: Vec<Bits> = (0..3)
        .map(|_| finish_frame(&encode_frame(&[&t], 0, false)))
        .collect();
    let packets = packetize(&frames);
    let all = decode(&packets, 1);
    let mut d = Xma2Decoder::new(Xma2Config::new(32000, 1, Some(1000))).unwrap();
    for p in &packets {
        d.push_packet(p).unwrap();
    }
    assert_eq!(d.available(), 1000);
    let mut got = Vec::new();
    let mut buf = [0i16; 77];
    loop {
        let n = d.pull_i16(&mut buf);
        if n == 0 {
            break;
        }
        got.extend_from_slice(&buf[..n]);
    }
    assert_eq!(got.len(), 1000);
    for (g, f) in got.iter().zip(&all) {
        assert_eq!(*g, super::f32_to_i16(*f));
    }
}

#[test]
fn lead_in_drops_leading_samples() {
    let mut rng = Rng(21);
    let t = random_tuples(&mut rng, 30);
    let frames: Vec<Bits> = (0..3)
        .map(|_| finish_frame(&encode_frame(&[&t], 0, false)))
        .collect();
    let packets = packetize(&frames);
    let all = decode(&packets, 1);
    let mut cfg = Xma2Config::new(32000, 1, None);
    cfg.lead_in = 700;
    let mut d = Xma2Decoder::new(cfg).unwrap();
    for p in &packets {
        d.push_packet(p).unwrap();
    }
    let mut got = vec![0f32; all.len()];
    let n = d.pull_f32(&mut got);
    assert_eq!(n, all.len() - 700);
    assert_eq!(&got[..n], &all[700..]);
}

#[test]
fn seek_restarts_with_warm_overlap() {
    let mut rng = Rng(17);
    let mut frames = Vec::new();
    for _ in 0..300 {
        let t = random_tuples(&mut rng, 40);
        frames.push(finish_frame(&encode_frame(&[&t], 0, false)));
    }
    let packets = packetize(&frames);
    assert!(packets.len() >= 4);
    let all = decode(&packets, 1);
    // per-packet start samples as an encoder would store them
    let mut table = Vec::new();
    let mut started = 0u32;
    for p in &packets {
        table.push(started * 512);
        started += u32::from(PacketHeader::parse(p).unwrap().frame_count);
    }
    // frames that start in a packet may finish in the next one, so the sample table counts frames started;
    // seek to a position well inside the third packet
    let target = u64::from(table[2]) + 1500;
    let mut d = Xma2Decoder::new(Xma2Config::new(32000, 1, None)).unwrap();
    let first = d.seek(&table, target).unwrap();
    assert_eq!(first, 1);
    for p in &packets[first..] {
        d.push_packet(p).unwrap();
    }
    let mut got = vec![0f32; 4000];
    let n = d.pull_f32(&mut got);
    assert_eq!(n, 4000);
    let want = &all[target as usize..target as usize + 4000];
    for (g, w) in got.iter().zip(want) {
        assert!((g - w).abs() < 1e-5, "{g} vs {w}");
    }
}

#[test]
fn damaged_input_is_an_error_not_noise() {
    let mut rng = Rng(8);
    let t = random_tuples(&mut rng, 30);
    let frames = vec![finish_frame(&encode_frame(&[&t], 0, false))];
    let mut packets = packetize(&frames);
    // break the first frame's tile header by flipping its length prefix to something too short
    packets[0][4] = 0;
    packets[0][5] = 0x10;
    let mut d = Xma2Decoder::new(Xma2Config::new(32000, 1, None)).unwrap();
    assert!(d.push_packet(&packets[0]).is_err());
    assert_eq!(d.available(), 0);
    assert!(d.push_packet(&[0; 100]).is_err());
}

#[test]
fn stream_layout_for_channel_counts() {
    assert_eq!(Xma2Config::new(32000, 1, None).stream_channels(), vec![1]);
    assert_eq!(Xma2Config::new(32000, 2, None).stream_channels(), vec![2]);
    assert_eq!(
        Xma2Config::new(32000, 5, None).stream_channels(),
        vec![2, 2, 1]
    );
    assert!(Xma2Decoder::new(Xma2Config::new(32000, 0, None)).is_err());
}

#[test]
fn band_layout_for_32khz() {
    let d = FrameDecoder::new(1, 32000).unwrap();
    assert!(d.band_count(0) > 10 && d.band_count(0) < 29);
    assert!(d.band_count(2) <= d.band_count(0));
    assert_eq!(super::band_rate_class(32007), 32000);
    assert_eq!(super::band_rate_class(22050), 24000);
    assert_eq!(super::band_rate_class(44100), 44100);
    assert_eq!(super::band_rate_class(48000), 48000);
}

#[test]
fn every_table_entry_decodes_to_its_own_symbol() {
    use crate::bits::{BitReader, BitWriter};
    let t = tables();
    let all = [
        (&t.sf, SCALE_TABLE.len()),
        (&t.sf_rl, SCALE_RL_TABLE.len()),
        (&t.coef[0], COEF0_LENS.len()),
        (&t.coef[1], COEF1_TABLE.len()),
        (&t.vec4, VEC4_LENS.len()),
        (&t.vec2, VEC2_TABLE.len()),
        (&t.vec1, VEC1_TABLE.len()),
    ];
    for (vlc, n) in all {
        assert_eq!(vlc.entries(), n);
        let mut w = BitWriter::new();
        for i in 0..n {
            let (code, len) = vlc.code_of(i);
            w.put(len, code);
        }
        let mut r = BitReader::with_bit_len(&w.bytes, w.bits);
        for i in 0..n {
            assert_eq!(
                vlc.decode(&mut r),
                Some(vlc.symbol_of(i)),
                "entry {i} of a table of {n}"
            );
        }
        assert_eq!(r.bits_left(), 0);
    }
}

#[test]
fn run_and_level_tables_line_up_with_the_code_tables() {
    assert_eq!(COEF0_RUN.len(), COEF0_SYMS.len());
    assert_eq!(COEF0_LEVEL.len(), COEF0_SYMS.len());
    assert_eq!(COEF1_RUN.len(), COEF1_TABLE.len());
    assert_eq!(COEF1_LEVEL.len(), COEF1_TABLE.len());
    assert_eq!(SCALE_RL_RUN.len(), SCALE_RL_TABLE.len());
    assert_eq!(SCALE_RL_LEVEL.len(), SCALE_RL_TABLE.len());
    // symbols 0 (escape) and 1 (end of block) exist in both coefficient code tables
    for s in [0u16, 1] {
        assert!(COEF0_SYMS.contains(&s));
        assert!(COEF1_TABLE.iter().any(|&(sym, _)| u16::from(sym) == s));
    }
    // symbols are within the run/level tables
    assert!(COEF0_SYMS.iter().all(|&s| usize::from(s) < COEF0_RUN.len()));
    assert!(
        COEF1_TABLE
            .iter()
            .all(|&(s, _)| usize::from(s) < COEF1_RUN.len())
    );
}

#[test]
fn garbage_and_damaged_packets_never_panic() {
    let mut rng = Rng(1234);
    let t = random_tuples(&mut rng, 40);
    let good = packetize(&[
        finish_frame(&encode_frame(&[&t], 0, false)),
        finish_frame(&encode_frame(&[&t, &t], 0, true)),
    ]);
    for channels in [1usize, 2] {
        for round in 0..300 {
            let mut d = Xma2Decoder::new(Xma2Config::new(32000, channels, None)).unwrap();
            let mut p = if round % 3 == 0 {
                (0..PACKET_SIZE)
                    .map(|_| rng.next() as u8)
                    .collect::<Vec<u8>>()
            } else {
                good[0].clone()
            };
            if round % 3 != 0 {
                for _ in 0..1 + round % 7 {
                    let at = 4 + (rng.next() as usize) % 300;
                    p[at] ^= 1 << (rng.next() % 8);
                }
            }
            // a valid-looking header so frames are attempted
            if round % 3 == 0 {
                p[0] = 0x04;
                p[1] = 0;
                p[2] = 0;
                p[3] = 0;
            }
            let _ = d.push_packet(&p);
            let _ = d.push_packet(&good[0]);
            let mut out = vec![0f32; 4096 * channels];
            d.pull_f32(&mut out);
            assert!(out.iter().all(|v| v.is_finite()));
        }
    }
}
