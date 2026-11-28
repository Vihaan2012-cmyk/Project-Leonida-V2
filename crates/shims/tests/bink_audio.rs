//! A real game movie's sound through the movie player and the shared mixer, with the audio device run slightly
//! fast so the movie's clock has to follow it. Skipped unless `LEONIDA_GAME_DIR` names the game folder.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bink::{AudioDecoder, Container};
use shims::audio::Mixer;
use shims::media::player::Player;
use shims::media::sound::EngineSink;
use shims::media::testing::ManualClock;
use shims::media::{AudioTrackInfo, DecodeError, DecodedFrame, MovieDecoder, Plane, StreamInfo};

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

/// Every stream of the movie archive: where it starts, how long it is, its frame count and audio track count.
fn streams(file: &mut File) -> Vec<(u64, usize, u32, u32)> {
    let total = file.metadata().unwrap().len();
    let mut at = 0x7000u64;
    let mut found = Vec::new();
    while at + 0x2C <= total {
        file.seek(SeekFrom::Start(at)).unwrap();
        let mut head = [0u8; 0x2C];
        file.read_exact(&mut head).unwrap();
        if &head[..3] != b"KB2" {
            break;
        }
        let len = word(&head, 4) as usize + 8;
        found.push((at, len, word(&head, 8), word(&head, 0x28)));
        at += len as u64;
        let mut buf = [0u8; 4096];
        loop {
            file.seek(SeekFrom::Start(at)).unwrap();
            let n = file.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            if let Some(skip) = buf[..n].iter().position(|b| *b != 0) {
                at += skip as u64;
                break;
            }
            at += n as u64;
        }
    }
    found
}

/// The real movie's audio with blank pictures.
struct AudioOnly {
    container: Container,
    decoder: AudioDecoder,
    next: u32,
    last: Vec<f32>,
}

impl MovieDecoder for AudioOnly {
    fn info(&self) -> StreamInfo {
        let h = &self.container.header;
        StreamInfo {
            width: 16,
            height: 16,
            frames: h.frames,
            fps_num: h.fps_num,
            fps_den: h.fps_den,
            has_alpha: false,
            audio: h
                .audio
                .iter()
                .map(|t| AudioTrackInfo {
                    rate: t.rate,
                    channels: t.channels as u8,
                    id: t.id,
                })
                .collect(),
        }
    }

    fn decode_next(&mut self) -> Result<DecodedFrame, DecodeError> {
        let index = self.next;
        let packets = self
            .container
            .packets(index)
            .map_err(|e| DecodeError(e.to_string()))?;
        self.last.clear();
        self.decoder
            .decode_packet(packets.audio[0], &mut self.last)
            .map_err(|e| DecodeError(e.to_string()))?;
        self.next += 1;
        Ok(DecodedFrame {
            index,
            planes: vec![Plane {
                data: vec![0; 16],
                stride: 4,
                width: 4,
                height: 4,
            }],
        })
    }

    fn seek(&mut self, frame: u32) -> Result<(), DecodeError> {
        self.next = frame;
        Ok(())
    }

    fn audio(&mut self, _track: usize) -> Vec<f32> {
        std::mem::take(&mut self.last)
    }
}

/// The smallest movie with one audio track, at least `frames` long, whose first seconds make sound.
fn pick(file: &mut File, frames: u32) -> Option<Container> {
    let mut candidates: Vec<_> = streams(file)
        .into_iter()
        .filter(|s| s.3 == 1 && s.2 >= frames)
        .collect();
    candidates.sort_by_key(|s| s.1);
    for (at, len, ..) in candidates.into_iter().take(12) {
        file.seek(SeekFrom::Start(at)).unwrap();
        let mut data = vec![0u8; len];
        file.read_exact(&mut data).unwrap();
        let container = Container::parse(Arc::from(data)).unwrap();
        let mut decoder = AudioDecoder::new(&container.header.audio[0]).unwrap();
        let mut samples = Vec::new();
        for f in 0..frames {
            let packet = container.packets(f).unwrap().audio[0];
            decoder.decode_packet(packet, &mut samples).unwrap();
        }
        let tail = &samples[samples.len() / 2..];
        if tail.iter().any(|s| s.abs() > 0.01) {
            return Some(container);
        }
    }
    None
}

#[test]
fn the_pictures_keep_to_the_sound_when_the_device_runs_two_percent_fast() {
    let Some(dir) = std::env::var_os("LEONIDA_GAME_DIR").map(PathBuf::from) else {
        return;
    };
    let mut file = File::open(dir.join("movies.rpf")).unwrap();
    let seconds = 6u32;
    let container = pick(&mut file, seconds * 30 + 30).expect("a movie with sound");
    let track = container.header.audio[0].clone();
    let mut reference = Vec::new();
    let mut reference_decoder = AudioDecoder::new(&track).unwrap();
    for f in 0..seconds * 30 + 30 {
        let packet = container.packets(f).unwrap().audio[0];
        reference_decoder
            .decode_packet(packet, &mut reference)
            .unwrap();
    }
    let decoder = AudioOnly {
        decoder: AudioDecoder::new(&track).unwrap(),
        container,
        next: 0,
        last: Vec::new(),
    };
    let info = decoder.info();
    let mixer = Arc::new(Mutex::new(Mixer::new(48000)));
    let clock = Arc::new(ManualClock::default());
    let mut player = Player::new(
        info.clone(),
        vec![0],
        Some(Box::new(move || Ok(Box::new(decoder) as _))),
        Box::new(EngineSink::on(mixer.clone())),
        clock.clone(),
    );
    player.do_frame();
    assert_eq!(player.current(), 1);

    let speed = 1.02;
    let mut rendered = Vec::<f32>::new();
    let mut frames_due = 0.0f64;
    let mut worst_early_drift = 0f64;
    let mut final_drift = 0f64;
    for _ in 0..seconds * 1000 {
        clock.advance(Duration::from_millis(1));
        frames_due += 48.0 * speed;
        let n = frames_due as usize - rendered.len() / 2;
        let mut out = vec![0.0f32; n * 2];
        mixer.lock().unwrap().render(&mut out);
        rendered.extend_from_slice(&out);
        if !player.wait() && player.current() < info.frames {
            player.next_frame();
            player.do_frame_start();
            assert!(
                player.do_frame_wait(Duration::from_secs(5)),
                "decoder fell behind"
            );
            let audio_time = (rendered.len() / 2) as f64 / 48000.0;
            let movie_time = f64::from(player.current() - 1) / 30.0;
            final_drift = audio_time - movie_time;
            if movie_time < 1.0 {
                worst_early_drift = worst_early_drift.max(final_drift.abs());
            }
        }
    }
    player.close();
    let wall = f64::from(seconds);
    let uncorrected = wall * (speed - 1.0);
    assert!(
        final_drift.abs() < 0.04,
        "after {wall} s the sound is {final_drift:.3} s from the picture (it would be {uncorrected:.3} s on the wall clock)"
    );
    assert!(worst_early_drift < 0.1, "early drift {worst_early_drift}");

    let compared = rendered.len().min(reference.len());
    assert!(compared > 48000 * 2 * (seconds as usize - 1));
    let worst = rendered[..compared]
        .iter()
        .zip(&reference[..compared])
        .fold(0f32, |m, (a, b)| m.max((a - b).abs()));
    assert!(
        worst < 1e-5,
        "the mixer played other samples than the decoder made: {worst}"
    );
    assert!(rendered.iter().any(|s| s.abs() > 0.01));
}
