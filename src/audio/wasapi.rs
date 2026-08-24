//! WASAPI audio capture via cpal (Windows backend).
//!
//! cpal is callback-driven, unlike the blocking ALSA read loop: the audio
//! callback converts interleaved hardware samples to mono `f32`, accumulates
//! them, and pushes `chunk_ms`-sized resampled blocks into the same kind of
//! mpsc channel the ALSA backend uses. Downstream code can't tell the
//! difference.

use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rubato::{FftFixedIn, Resampler};
use tracing::{error, info, warn};

use crate::config::AudioConfig;

/// Handle to a running capture stream. Drop to stop capturing.
pub struct AudioCapture {
    rx: Receiver<Vec<f32>>,
    // The stream must be kept alive; dropping it stops capture. Never
    // touched otherwise.
    _stream: cpal::Stream,
}

/// Shared between the audio callback (producer) — owns accumulation,
/// resampling, and the downstream channel.
struct CaptureState {
    /// Mono f32 samples at the hardware rate, not yet a full chunk.
    pending: Vec<f32>,
    chunk_frames: usize,
    resampler: Option<FftFixedIn<f32>>,
    tx: SyncSender<Vec<f32>>,
}

impl CaptureState {
    /// Append interleaved hardware samples (any channel count), mixing down
    /// to mono, and emit every complete chunk downstream.
    fn push_interleaved(&mut self, data: &[f32], channels: usize) {
        if channels == 1 {
            self.pending.extend_from_slice(data);
        } else {
            self.pending
                .extend(data.chunks_exact(channels).map(|frame| {
                    frame.iter().sum::<f32>() / channels as f32
                }));
        }

        while self.pending.len() >= self.chunk_frames {
            let chunk: Vec<f32> = self.pending.drain(..self.chunk_frames).collect();
            let out = match &mut self.resampler {
                Some(r) => match r.process(&[chunk], None) {
                    Ok(v) => v.into_iter().next().unwrap_or_default(),
                    Err(e) => {
                        error!("resampling failed: {e}");
                        return;
                    }
                },
                None => chunk,
            };
            // try_send: if downstream stalls, drop audio rather than grow
            // memory — same policy as the ALSA backend's bounded channel.
            if self.tx.try_send(out).is_err() {
                warn!("audio channel full or closed; dropping chunk");
            }
        }
    }
}

impl AudioCapture {
    /// Start capturing from the configured (or default) input device.
    pub fn start(config: &AudioConfig) -> anyhow::Result<Self> {
        let host = cpal::default_host();

        let device = match &config.input_device {
            Some(name) => {
                let mut devices = host
                    .input_devices()
                    .context("failed to enumerate input devices")?;
                devices
                    .find(|d| d.name().map(|n| n.contains(name.as_str())).unwrap_or(false))
                    .ok_or_else(|| anyhow!("no input device matching {name:?}"))?
            }
            None => host
                .default_input_device()
                .context("no default input device")?,
        };
        let device_name = device.name().unwrap_or_else(|_| "?".into());

        let supported = device
            .default_input_config()
            .context("failed to query default input config")?;
        let stream_config: cpal::StreamConfig = supported.clone().into();

        let hw_rate = stream_config.sample_rate.0;
        let channels = stream_config.channels as usize;
        let chunk_frames = (hw_rate as u64 * config.chunk_ms / 1000) as usize;

        let resampler = if hw_rate != config.target_rate {
            Some(
                FftFixedIn::<f32>::new(
                    hw_rate as usize,
                    config.target_rate as usize,
                    chunk_frames,
                    1,
                    1,
                )
                .context("failed to create resampler")?,
            )
        } else {
            None
        };

        // Bounded: ~5s of 16 kHz mono audio in 100 ms chunks.
        let (tx, rx) = sync_channel(64);
        let state = Arc::new(Mutex::new(CaptureState {
            pending: Vec::with_capacity(chunk_frames * 2),
            chunk_frames,
            resampler,
            tx,
        }));

        info!(
            device = %device_name,
            rate = hw_rate,
            channels,
            format = ?supported.sample_format(),
            chunk_frames,
            "WASAPI capture device opened"
        );

        let err_fn = |e: cpal::StreamError| error!("capture stream error: {e}");
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => build_stream::<f32>(&device, &stream_config, state, channels, err_fn),
            cpal::SampleFormat::I16 => build_stream::<i16>(&device, &stream_config, state, channels, err_fn),
            cpal::SampleFormat::U16 => build_stream::<u16>(&device, &stream_config, state, channels, err_fn),
            fmt => return Err(anyhow!("unsupported capture sample format: {fmt:?}")),
        }
        .context("failed to build capture stream")?;
        stream.play().context("failed to start capture stream")?;

        Ok(Self { rx, _stream: stream })
    }

    /// Receive the next chunk of mono audio at the target sample rate.
    /// Blocks until a chunk is available or the stream dies.
    pub fn recv(&self) -> Option<Vec<f32>> {
        self.rx.recv().ok()
    }

    /// Borrow the underlying receiver for integration with select loops.
    pub fn receiver(&self) -> &Receiver<Vec<f32>> {
        &self.rx
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    state: Arc<Mutex<CaptureState>>,
    channels: usize,
    err_fn: impl Fn(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::Sample + cpal::SizedSample + Send + 'static,
    f32: cpal::FromSample<T>,
{
    device.build_input_stream(
        config,
        move |data: &[T], _| {
            let mut state = match state.lock() {
                Ok(s) => s,
                Err(_) => return, // poisoned: nothing sane to do mid-callback
            };
            let f32_data: Vec<f32> = data.iter().map(|s| s.to_sample::<f32>()).collect();
            state.push_interleaved(&f32_data, channels);
        },
        err_fn,
        None,
    )
}
