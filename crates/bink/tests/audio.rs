//! Decodes the audio of real game movies. Skipped unless `LEONIDA_GAME_DIR` names the game folder.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;

use bink::{AudioDecoder, Container, Movie};

struct Entry {
    at: u64,
    len: usize,
    frames: u32,
    tracks: u32,
}

struct Archive {
    file: File,
    streams: Vec<Entry>,
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

impl Archive {
    fn open() -> Option<Archive> {
        let dir = PathBuf::from(std::env::var_os("LEONIDA_GAME_DIR")?);
        let mut file = File::open(dir.join("movies.rpf")).ok()?;
        let total = file.metadata().ok()?.len();
        let mut at = 0x7000u64;
        let mut streams = Vec::new();
        while at + 0x2C <= total {
            file.seek(SeekFrom::Start(at)).ok()?;
            let mut head = [0u8; 0x2C];
            file.read_exact(&mut head).ok()?;
            if &head[..3] != b"KB2" {
                break;
            }
            let len = word(&head, 4) as usize + 8;
            streams.push(Entry {
                at,
                len,
                frames: word(&head, 8),
                tracks: word(&head, 0x28),
            });
            at += len as u64;
            let mut buf = [0u8; 4096];
            loop {
                file.seek(SeekFrom::Start(at)).ok()?;
                let n = file.read(&mut buf).ok()?;
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
        Some(Archive { file, streams })
    }

    fn read(&mut self, index: usize) -> Arc<[u8]> {
        let (at, len) = (self.streams[index].at, self.streams[index].len);
        self.file.seek(SeekFrom::Start(at)).unwrap();
        let mut data = vec![0u8; len];
        self.file.read_exact(&mut data).unwrap();
        Arc::from(data)
    }

    /// The smallest streams (at least `min_frames` long) with exactly `tracks` audio tracks.
    fn smallest(&self, tracks: u32, min_frames: u32, count: usize) -> Vec<usize> {
        let mut found: Vec<usize> = (0..self.streams.len())
            .filter(|&i| self.streams[i].tracks == tracks && self.streams[i].frames >= min_frames)
            .collect();
        found.sort_by_key(|&i| self.streams[i].len);
        found.truncate(count);
        found
    }
}

struct Decoded {
    samples: Vec<Vec<f32>>,
    per_packet: Vec<Vec<usize>>,
}

fn decode_all(container: &Container) -> Decoded {
    let header = &container.header;
    let mut decoders: Vec<AudioDecoder> = header
        .audio
        .iter()
        .map(|t| AudioDecoder::new(t).expect("every track of the game's movies is DCT"))
        .collect();
    let mut samples = vec![Vec::new(); decoders.len()];
    let mut per_packet = vec![Vec::new(); decoders.len()];
    for frame in 0..header.frames {
        let packets = container.packets(frame).unwrap();
        for (t, decoder) in decoders.iter_mut().enumerate() {
            let packet = packets.audio[t];
            let announced = decoder.packet_samples(packet);
            let got = decoder
                .decode_packet(packet, &mut samples[t])
                .unwrap_or_else(|e| panic!("frame {frame} track {t}: {e}"));
            assert_eq!(got, announced, "frame {frame} track {t}");
            per_packet[t].push(got);
        }
    }
    Decoded {
        samples,
        per_packet,
    }
}

/// Checks one track; false when it decodes to exact silence, which some movies' tracks do.
fn check_track(name: &str, container: &Container, t: usize, decoded: &Decoded) -> bool {
    let header = &container.header;
    let track = &header.audio[t];
    let channels = usize::from(track.channels);
    let samples = &decoded.samples[t];
    assert_eq!(samples.len() % channels, 0);
    let per_channel = samples.len() / channels;
    let expected = u64::from(header.frames) * u64::from(header.fps_den) * u64::from(track.rate)
        / u64::from(header.fps_num);
    let block = 1920i64;
    assert!(
        (per_channel as i64 - expected as i64).abs() <= block,
        "{name} track {t}: {per_channel} samples a channel for {expected} expected from {} frames",
        header.frames
    );
    assert!(samples.iter().all(|s| s.is_finite()), "{name}: NaN or inf");
    let peak = samples.iter().fold(0f32, |m, s| m.max(s.abs()));
    assert!(peak <= 1.0, "{name} track {t}: peak {peak}");
    if peak == 0.0 {
        return false;
    }
    let rms =
        (samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / samples.len() as f64).sqrt();
    assert!(
        peak > 0.02 && rms > 0.002,
        "{name} track {t}: silent (peak {peak}, rms {rms})"
    );
    let first: Vec<f64> = samples
        .iter()
        .step_by(channels)
        .map(|s| f64::from(*s))
        .collect();
    let (mut lag0, mut lag1) = (0.0, 0.0);
    for w in first.windows(2) {
        lag0 += w[0] * w[0];
        lag1 += w[0] * w[1];
    }
    let correlation = lag1 / lag0;
    assert!(
        correlation > 0.3,
        "{name} track {t}: lag-1 correlation {correlation}: that is noise, not music or speech"
    );
    let head_start = decoded.per_packet[t].iter().position(|n| *n > 0).unwrap();
    assert_eq!(
        head_start, 0,
        "{name}: the first frame carries the head start"
    );
    assert_eq!(decoded.per_packet[t][0], 19 * 1920, "{name}: head start");
    assert!(
        decoded.per_packet[t][1..].iter().all(|n| *n <= 2 * 1920),
        "{name}: a later frame carries more than two blocks"
    );
    true
}

/// How many of the movie's tracks carry a signal.
fn run(archive: &mut Archive, index: usize) -> usize {
    let data = archive.read(index);
    let container = Container::parse(data).unwrap();
    let decoded = decode_all(&container);
    (0..container.header.audio.len())
        .filter(|&t| check_track(&format!("stream {index}"), &container, t, &decoded))
        .count()
}

#[test]
fn single_track_movies_decode_to_their_duration_without_error() {
    let Some(mut archive) = Archive::open() else {
        return;
    };
    let picks = archive.smallest(1, 60, 12);
    assert_eq!(picks.len(), 12);
    let sounding: usize = picks
        .into_iter()
        .map(|index| run(&mut archive, index))
        .sum();
    assert!(sounding >= 3, "only {sounding} of 12 movies carry a signal");
}

#[test]
fn every_track_of_a_multi_track_movie_decodes() {
    let Some(mut archive) = Archive::open() else {
        return;
    };
    let mut sounding = 0;
    for tracks in [2, 4] {
        for index in archive.smallest(tracks, 60, 1) {
            sounding += run(&mut archive, index);
        }
    }
    assert!(
        sounding >= 2,
        "{sounding} multi-track tracks carry a signal"
    );
}

#[test]
fn a_movies_first_frame_hands_over_its_head_start_through_movie_audio() {
    let Some(mut archive) = Archive::open() else {
        return;
    };
    let index = archive.smallest(1, 60, 1)[0];
    let mut movie = Movie::open(archive.read(index)).unwrap();
    movie.decode_next().unwrap();
    let audio = movie.audio(0);
    assert_eq!(audio.len(), 19 * 1920 * 2);
    assert!(
        movie.audio(0).is_empty(),
        "the samples are handed over once"
    );
    assert_eq!(movie.audio_failures(), 0);
}
