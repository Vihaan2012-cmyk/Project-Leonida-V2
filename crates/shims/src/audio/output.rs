//! Sound output through `cpal` at the device's own rate.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, FromSample, SampleFormat, SizedSample, StreamConfig, SupportedBufferSize};

use super::mixer::Mixer;

pub type SharedMixer = Arc<Mutex<Mixer>>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OutputError {
    #[error("no output device")]
    NoDevice,
    #[error("the device uses sample format {0}, which is not supported")]
    Format(String),
    #[error("audio device error: {0}")]
    Device(String),
}

/// An open output stream. Dropping it stops the sound.
pub struct Output {
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
    rate: u32,
    channels: u16,
    lost: Arc<AtomicBool>,
}

/// Longest the device buffer may hold, in milliseconds: short enough that a lock held by the title never
/// starves the device for long, long enough to ride out a scheduling delay at high frame rates.
const TARGET_BUFFER_MS: u32 = 10;

/// The fixed buffer size to ask the device for: about [`TARGET_BUFFER_MS`] of frames, kept inside what the
/// device supports; `None` leaves the choice to the host.
pub fn choose_buffer_frames(supported: &SupportedBufferSize, rate: u32) -> Option<u32> {
    match supported {
        SupportedBufferSize::Range { min, max } if min <= max => {
            let wanted = (rate / 1000 * TARGET_BUFFER_MS).next_power_of_two();
            Some(wanted.clamp(*min, *max))
        }
        _ => None,
    }
}

impl Output {
    /// Opens the default device and starts pulling stereo frames from `mixer`, whose rate becomes the
    /// device's.
    pub fn start(mixer: SharedMixer) -> Result<Output, OutputError> {
        let (ready_tx, ready_rx) = channel::<Result<(u32, u16), OutputError>>();
        let (stop_tx, stop_rx) = channel::<()>();
        let lost = Arc::new(AtomicBool::new(false));
        let thread_lost = lost.clone();
        let thread = std::thread::Builder::new()
            .name("leonida-audio".into())
            .spawn(move || {
                // The stream is not `Send` on every host, so it lives and dies on this thread.
                match open(mixer, thread_lost) {
                    Ok((stream, rate, channels)) => {
                        let _ = ready_tx.send(Ok((rate, channels)));
                        let _ = stop_rx.recv();
                        drop(stream);
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                    }
                }
            })
            .map_err(|e| OutputError::Device(e.to_string()))?;
        match ready_rx.recv() {
            Ok(Ok((rate, channels))) => Ok(Output {
                stop: Some(stop_tx),
                thread: Some(thread),
                rate,
                channels,
                lost,
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err(OutputError::Device("the audio thread ended early".into()))
            }
        }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// Whether the device went away or failed since the stream started; the stream is dead then.
    pub fn lost(&self) -> bool {
        self.lost.load(Ordering::Acquire)
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn open(
    mixer: SharedMixer,
    lost: Arc<AtomicBool>,
) -> Result<(cpal::Stream, u32, u16), OutputError> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or(OutputError::NoDevice)?;
    let supported = device
        .default_output_config()
        .map_err(|e| OutputError::Device(e.to_string()))?;
    let format = supported.sample_format();
    let buffer = choose_buffer_frames(supported.buffer_size(), supported.sample_rate());
    let mut config: StreamConfig = supported.into();
    let (rate, channels) = (config.sample_rate, config.channels);
    if let Ok(mut mixer) = mixer.lock() {
        mixer.set_rate(rate);
    }
    let build_with = |config: StreamConfig| match format {
        SampleFormat::F32 => build::<f32>(&device, config, mixer.clone(), lost.clone()),
        SampleFormat::I16 => build::<i16>(&device, config, mixer.clone(), lost.clone()),
        SampleFormat::I32 => build::<i32>(&device, config, mixer.clone(), lost.clone()),
        SampleFormat::U16 => build::<u16>(&device, config, mixer.clone(), lost.clone()),
        other => Err(OutputError::Format(other.to_string())),
    };
    let stream = match buffer {
        Some(frames) => {
            config.buffer_size = BufferSize::Fixed(frames);
            build_with(config).or_else(|_| {
                config.buffer_size = BufferSize::Default;
                build_with(config)
            })
        }
        None => build_with(config),
    }?;
    stream
        .play()
        .map_err(|e| OutputError::Device(e.to_string()))?;
    Ok((stream, rate, channels))
}

fn build<T>(
    device: &cpal::Device,
    config: StreamConfig,
    mixer: SharedMixer,
    lost: Arc<AtomicBool>,
) -> Result<cpal::Stream, OutputError>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels.max(1));
    let mut stereo: Vec<f32> = Vec::new();
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| {
                let frames = data.len() / channels;
                stereo.resize(frames * 2, 0.0);
                // Never wait on the title: if it holds the mixer for the moment, this buffer is silence.
                match mixer.try_lock() {
                    Ok(mut mixer) => mixer.render(&mut stereo),
                    Err(_) => stereo.fill(0.0),
                }
                for (frame, pair) in data.chunks_mut(channels).zip(stereo.chunks(2)) {
                    for (c, slot) in frame.iter_mut().enumerate() {
                        *slot = match (c, channels) {
                            (_, 1) => T::from_sample((pair[0] + pair[1]) * 0.5),
                            (0, _) => T::from_sample(pair[0]),
                            (1, _) => T::from_sample(pair[1]),
                            _ => T::EQUILIBRIUM,
                        };
                    }
                }
            },
            move |_error| lost.store(true, Ordering::Release),
            None,
        )
        .map_err(|e| OutputError::Device(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_buffer_is_about_ten_milliseconds_inside_the_supported_range() {
        let range = |min, max| SupportedBufferSize::Range { min, max };
        assert_eq!(choose_buffer_frames(&range(32, 4096), 48_000), Some(512));
        assert_eq!(choose_buffer_frames(&range(32, 4096), 44_100), Some(512));
        assert_eq!(choose_buffer_frames(&range(1024, 8192), 48_000), Some(1024));
        assert_eq!(choose_buffer_frames(&range(32, 256), 48_000), Some(256));
        assert_eq!(choose_buffer_frames(&range(256, 32), 48_000), None);
        assert_eq!(
            choose_buffer_frames(&SupportedBufferSize::Unknown, 48_000),
            None
        );
    }

    #[test]
    fn a_device_plays_silence_and_the_mixer_follows_its_rate() {
        let mixer: SharedMixer = Arc::new(Mutex::new(Mixer::new(1)));
        let Ok(output) = Output::start(mixer.clone()) else {
            eprintln!("no audio device: skipped");
            return;
        };
        assert!(output.rate() > 0 && output.channels() > 0);
        assert_eq!(mixer.lock().unwrap().rate(), output.rate());
        std::thread::sleep(std::time::Duration::from_millis(150));
        assert!(mixer.lock().unwrap().rendered_frames() > 0);
        drop(output);
    }
}
