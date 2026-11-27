//! Decoder tests against streams built here: a small XMA2 frame writer, a packetizer, and a reference
//! synthesis that uses the direct IMDCT formula and the sine window instead of the decoder's code.

use std::f64::consts::PI;

use super::frame::{FrameDecoder, tables};
use super::stream::{PACKET_SIZE, PacketHeader, Xma2Config, Xma2Decoder};
use super::tables::*;

const N: usize = 512;
const DATA_BITS: usize = PACKET_SIZE * 8 - 32;

type Bits = Vec<bool>;

fn put(b: &mut Bits, n: u32, v: u32) {
    for i in (0..n).rev() {
        b.push((v >> i) & 1 == 1);
    }
}

fn put_code(b: &mut Bits, (code, len): (u32, u32)) {
    put(b, len, code);
}

struct Rng(u32);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223);
        self.0 >> 8
    }
}

/// Four magnitudes (0..=15) and signs, one symbol of the vector coder.
#[derive(Clone, Copy)]
struct Tuple {
    mags: [u8; 4],
    neg: [bool; 4],
}

impl Tuple {
    fn packed(&self) -> i32 {
        self.mags.iter().fold(0, |v, &m| (v << 4) | i32::from(m))
    }
}

/// Tuples the vector table can express, without three zeros in a row (that would switch to run-level coding).
fn random_tuples(rng: &mut Rng, count: usize) -> Vec<Tuple> {
    let usable: Vec<u16> = VEC4_SYMS
        .iter()
        .copied()
        .filter(|&s| s > 1)
        .filter(|&s| {
            let p = s - 1;
            let m = [(p >> 12) & 15, (p >> 8) & 15, (p >> 4) & 15, p & 15];
            let zeros_edge = m.iter().take_while(|&&v| v == 0).count();
            let zeros_tail = m.iter().rev().take_while(|&&v| v == 0).count();
            zeros_edge <= 1 && zeros_tail <= 1 && !m.windows(3).any(|w| w.iter().all(|&v| v == 0))
        })
        .collect();
    (0..count)
        .map(|_| {
            let p = usable[(rng.next() as usize) % usable.len()] - 1;
            let mags = [
                (p >> 12) as u8 & 15,
                (p >> 8) as u8 & 15,
                (p >> 4) as u8 & 15,
                p as u8 & 15,
            ];
            let neg = std::array::from_fn(|i| mags[i] != 0 && rng.next() & 1 == 1);
            Tuple { mags, neg }
        })
        .collect()
}

fn coefficients(tuples: &[Tuple]) -> Vec<f64> {
    let mut c = vec![0.0; N];
    for (j, t) in tuples.iter().enumerate() {
        for i in 0..4 {
            let v = f64::from(t.mags[i]);
            c[4 * j + i] = if t.neg[i] { -v } else { v };
        }
    }
    c
}

fn vec4_code(packed: i32) -> (u32, u32) {
    let i = VEC4_SYMS
        .iter()
        .position(|&s| i32::from(s) - 1 == packed)
        .expect("tuple is in the vector table");
    tables().vec4.code_of(i)
}

fn encode_channel_coefficients(b: &mut Bits, tuples: &[Tuple]) {
    put(b, 1, 0);
    for t in tuples {
        put_code(b, vec4_code(t.packed()));
        for i in 0..4 {
            if t.mags[i] != 0 {
                put(b, 1, u32::from(t.neg[i]));
            }
        }
    }
    put_code(b, vec4_code(0));
    let eob = COEF0_SYMS.iter().position(|&s| s == 1).expect("EOB symbol");
    put_code(b, tables().coef[0].code_of(eob));
}

fn encode_scale_factors(b: &mut Bits, bands: usize) {
    put(b, 2, 0);
    let zero = SCALE_TABLE
        .iter()
        .position(|&(s, _)| s == 60)
        .expect("delta 0");
    for _ in 0..bands {
        put_code(b, tables().sf.code_of(zero));
    }
}

/// One frame with a single 512-sample subframe per channel and no length prefix, stop bit or trailer yet.
fn encode_frame(channels: &[&[Tuple]], step: i32, stereo_transform: bool) -> Bits {
    let bands = FrameDecoder::new(channels.len(), 32000)
        .unwrap()
        .band_count(0);
    let nb = channels.len();
    let mut b = Bits::new();
    put(&mut b, 1, 1);
    put(&mut b, 1, 0);
    if nb > 1 {
        put(&mut b, 1, 0);
    }
    put(&mut b, 8, 0);
    put(&mut b, 1, 0);
    put(&mut b, 1, 0);
    put(&mut b, 1, 0);
    if nb > 1 {
        put(&mut b, 1, 0);
        if stereo_transform {
            put(&mut b, 1, 0);
            put(&mut b, 1, 1);
        } else {
            put(&mut b, 1, 1);
            put(&mut b, 1, 0);
        }
    }
    for _ in 0..nb {
        put(&mut b, 1, 1);
    }
    put(&mut b, 1, 0);
    put(&mut b, 6, (step & 63) as u32);
    if nb > 1 {
        put(&mut b, 3, 0);
        for _ in 0..nb {
            put(&mut b, 1, 0);
        }
    }
    for _ in 0..nb {
        encode_scale_factors(&mut b, bands);
    }
    for tuples in channels {
        encode_channel_coefficients(&mut b, tuples);
    }
    b
}

/// The finished frame bits: length prefix, body, stop bit, trailer (0 until the packetizer decides).
fn finish_frame(body: &Bits) -> Bits {
    let len = body.len() + 15 + 2;
    let mut f = Bits::new();
    put(&mut f, 15, len as u32);
    f.extend_from_slice(body);
    f.push(true);
    f.push(false);
    f
}

struct Pk {
    carry: usize,
    started: usize,
    data: Bits,
}

fn packetize(frames: &[Bits]) -> Vec<Vec<u8>> {
    let mut pks = vec![Pk {
        carry: 0,
        started: 0,
        data: Bits::new(),
    }];
    let mut last_trailer: Option<(usize, usize)> = None;
    for f in frames {
        let cur = pks.len() - 1;
        if DATA_BITS - pks[cur].data.len() <= 17 {
            pks.push(Pk {
                carry: 0,
                started: 0,
                data: Bits::new(),
            });
        }
        let cur = pks.len() - 1;
        if let Some((pi, bi)) = last_trailer
            && pi == cur
        {
            pks[pi].data[bi] = true;
        }
        pks[cur].started += 1;
        let take = f.len().min(DATA_BITS - pks[cur].data.len());
        pks[cur].data.extend_from_slice(&f[..take]);
        let mut rest = &f[take..];
        if rest.is_empty() {
            last_trailer = Some((cur, pks[cur].data.len() - 1));
            continue;
        }
        while !rest.is_empty() {
            let chunk = rest.len().min(DATA_BITS);
            pks.push(Pk {
                carry: chunk,
                started: 0,
                data: rest[..chunk].to_vec(),
            });
            rest = &rest[chunk..];
        }
        let cur = pks.len() - 1;
        last_trailer = Some((cur, pks[cur].data.len() - 1));
    }
    pks.iter()
        .map(|p| {
            let mut out = vec![0u8; PACKET_SIZE];
            let word = ((p.started as u32) << 26) | ((p.carry as u32) << 11) | (1 << 8);
            out[..4].copy_from_slice(&word.to_be_bytes());
            for (i, &bit) in p.data.iter().enumerate() {
                if bit {
                    out[4 + i / 8] |= 0x80 >> (i % 8);
                }
            }
            out
        })
        .collect()
}

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
