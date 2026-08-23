//! ALSA audio capture with resampling to the target rate/channel count.
//!
//! The capture thread owns the ALSA device (exclusive hardware access) and
//! pushes mono `f32` frames at the target sample rate into an mpsc channel.
//! Samples are normalized to [-1.0, 1.0], the range rustpotter and
//! whisper-rs both expect.

use std::str::FromStr;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::thread::{self, JoinHandle};

use alsa::pcm::{Access, Format, HwParams, PCM};
use alsa::{Direction, ValueOr};
use anyhow::{anyhow, bail, Context};
use rubato::{FftFixedIn, Resampler};
use tracing::{debug, error, info, warn};

use crate::config::AudioConfig;

/// Handle to a running capture thread. Drop to stop capturing.
pub struct AudioCapture {
    rx: Receiver<Vec<f32>>,
    thread: Option<JoinHandle<()>>,
}

impl AudioCapture {
    /// Start capturing on the configured ALSA device.
    ///
    /// Spawns a dedicated thread: ALSA capture is blocking and must not run
    /// on the tokio runtime. Chunks of resampled mono audio arrive on the
    /// returned receiver.
    pub fn start(config: &AudioConfig) -> anyhow::Result<Self> {
        // Bounded: if downstream stalls, drop audio rather than grow memory
        // unboundedly. ~5s of 16 kHz mono audio in 100 ms chunks.
        let (tx, rx) = sync_channel(64);

        let pcm = open_pcm(config)?;
        let thread = thread::Builder::new()
            .name("audio-capture".into())
            .spawn({
                let config = config.clone();
                move || capture_loop(pcm, config, tx)
            })
            .context("failed to spawn audio capture thread")?;

        Ok(Self { rx, thread: Some(thread) })
    }

    /// Receive the next chunk of mono audio at the target sample rate.
    /// Blocks until a chunk is available or the capture thread dies.
    pub fn recv(&self) -> Option<Vec<f32>> {
        self.rx.recv().ok()
    }

    /// Borrow the underlying receiver for integration with select loops.
    #[allow(dead_code)] // used once wakeword/ambient consume the stream
    pub fn receiver(&self) -> &Receiver<Vec<f32>> {
        &self.rx
    }
}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        // Dropping the receiver signals the capture thread via SendError on
        // its next send (≤1 chunk later). Don't join here: if the capture
        // thread is wedged in a blocking ALSA read, joining would deadlock
        // the caller. Detached thread teardown on process exit is fine.
        let _ = self.thread.take();
    }
}

/// Open and configure the ALSA capture device.
fn open_pcm(config: &AudioConfig) -> anyhow::Result<PCM> {
    let format = Format::from_str(&config.format)
        .map_err(|_| anyhow!("unknown ALSA sample format: {:?}", config.format))?;

    let device = format!("hw:{},{}", config.alsa_card, config.alsa_device);
    let pcm = PCM::new(&device, Direction::Capture, false)
        .with_context(|| format!("failed to open ALSA capture device {device}"))?;

    let chunk_frames = chunk_frames(config);
    // physical_width() is in *bits* (24 for packed S24_3LE).
    let bytes_per_sample = format
        .physical_width()
        .context("could not determine sample width")? as usize
        / 8;
    let bytes_per_frame = config.channels as usize * bytes_per_sample;
    {
        let hwp = HwParams::any(&pcm).context("failed to query hw params")?;
        hwp.set_channels(config.channels.into())
            .context("device does not support requested channel count")?;
        hwp.set_rate(config.sample_rate, ValueOr::Nearest)
            .context("device does not support requested sample rate")?;
        hwp.set_format(format)
            .with_context(|| format!("device does not support format {}", config.format))?;
        hwp.set_access(Access::RWInterleaved)
            .context("device does not support interleaved access")?;
        // Buffer ~16 chunks (1.6s) — 4 chunks (400ms) overran with EPIPE
        // ("Broken pipe") whenever TTS/whisper stalled the read loop, and the
        // dropped frames silently killed wake word matches.
        hwp.set_buffer_size((chunk_frames * 16) as i64)
            .context("failed to set buffer size")?;
        hwp.set_period_size(chunk_frames as i64, ValueOr::Nearest)
            .context("failed to set period size")?;
        pcm.hw_params(&hwp).context("failed to apply hw params")?;
    }
    pcm.prepare().context("failed to prepare capture device")?;

    info!(
        device = %device,
        rate = config.sample_rate,
        channels = config.channels,
        format = %config.format,
        bytes_per_frame,
        chunk_frames,
        "ALSA capture device opened"
    );
    Ok(pcm)
}

fn chunk_frames(config: &AudioConfig) -> usize {
    (config.sample_rate as u64 * config.chunk_ms / 1000) as usize
}

/// Blocking capture loop: read hardware chunks, convert to f32, mix to mono,
/// resample to the target rate, send downstream. Exits when the receiver is
/// dropped or on unrecoverable error.
fn capture_loop(pcm: PCM, config: AudioConfig, tx: SyncSender<Vec<f32>>) {
    let chunk_frames = chunk_frames(&config);
    let bytes_per_frame = config.channels as usize * format_bytes(&config.format);
    let mut raw = vec![0u8; chunk_frames * bytes_per_frame];

    let mut resampler = if config.sample_rate != config.target_rate {
        match FftFixedIn::<f32>::new(
            config.sample_rate as usize,
            config.target_rate as usize,
            chunk_frames,
            1,
            1,
        ) {
            Ok(r) => Some(r),
            Err(e) => {
                error!("failed to create resampler: {e}");
                return;
            }
        }
    } else {
        None
    };

    let io = pcm.io_bytes();

    loop {
        match io.readi(&mut raw[..]) {
            Ok(_) => {}
            Err(e) => {
                // Overrun (xrun) or stream error: try to recover, else bail.
                warn!("capture read error: {e}; attempting recovery");
                if let Err(e) = pcm.recover(e.errno(), true) {
                    error!("capture recovery failed: {e}");
                    return;
                }
                continue;
            }
        }

        let mono = to_mono_f32(&raw, config.channels as usize, bytes_per_frame / config.channels as usize);
        if mono.is_empty() {
            error!("no samples decoded from chunk; check format/width handling");
            return;
        }
        let out = match &mut resampler {
            Some(r) => match resample_chunk(r, mono) {
                Ok(v) => v,
                Err(e) => {
                    error!("resampling failed: {e}");
                    return;
                }
            },
            None => mono,
        };

        if tx.send(out).is_err() {
            debug!("audio receiver dropped; capture thread exiting");
            return;
        }
    }
}

/// Decode interleaved hardware samples to normalized mono f32 by averaging
/// channels. `sample_width` is the physical byte width of one sample as
/// delivered by ALSA (3 for packed S24_3LE, 4 for left-justified 24-bit).
fn to_mono_f32(raw: &[u8], channels: usize, sample_width: usize) -> Vec<f32> {
    let frames = raw.len() / (channels * sample_width);
    let mut out = Vec::with_capacity(frames);

    match sample_width {
        2 => {
            for frame in raw.chunks_exact(channels * 2) {
                let mut sum = 0.0f32;
                for ch in 0..channels {
                    let s = i16::from_le_bytes([frame[ch * 2], frame[ch * 2 + 1]]);
                    sum += s as f32 / i16::MAX as f32;
                }
                out.push(sum / channels as f32);
            }
        }
        3 => {
            for frame in raw.chunks_exact(channels * 3) {
                let mut sum = 0.0f32;
                for ch in 0..channels {
                    let b = &frame[ch * 3..ch * 3 + 3];
                    // Sign-extend 24-bit to 32-bit.
                    let s = ((b[0] as i32) | ((b[1] as i32) << 8) | ((b[2] as i32) << 16)) << 8 >> 8;
                    sum += s as f32 / 8_388_607.0; // 2^23 - 1
                }
                out.push(sum / channels as f32);
            }
        }
        4 => {
            for frame in raw.chunks_exact(channels * 4) {
                let mut sum = 0.0f32;
                for ch in 0..channels {
                    let b: [u8; 4] = frame[ch * 4..ch * 4 + 4].try_into().unwrap();
                    let s = i32::from_le_bytes(b) as f32 / i32::MAX as f32;
                    sum += s;
                }
                out.push(sum / channels as f32);
            }
        }
        width => {
            warn!(width, "unusual sample width; generic little-endian decode");
            for frame in raw.chunks_exact(channels * width) {
                let mut sum = 0.0f32;
                for ch in 0..channels {
                    let b = &frame[ch * width..(ch + 1) * width];
                    let mut s: i64 = 0;
                    for (i, &byte) in b.iter().enumerate() {
                        s |= (byte as i64) << (8 * i);
                    }
                    let bits = (width * 8) as u32;
                    s = (s << (64 - bits)) >> (64 - bits); // sign-extend
                    sum += s as f32 / ((1i64 << (bits - 1)) - 1) as f32;
                }
                out.push(sum / channels as f32);
            }
        }
    }
    out
}

fn format_bytes(format: &str) -> usize {
    // physical_width() is in *bits*; convert to bytes.
    Format::from_str(format)
        .ok()
        .and_then(|f| f.physical_width().ok())
        .map(|bits| bits as usize / 8)
        .unwrap_or(4)
}

fn resample_chunk(r: &mut FftFixedIn<f32>, mono: Vec<f32>) -> anyhow::Result<Vec<f32>> {
    let out = r.process(&[mono], None)?;
    Ok(out.into_iter().next().unwrap_or_default())
}

/// Write f32 mono samples to a 16-bit PCM WAV file. Used by the `record`
/// subcommand and by ambient recording later.
pub fn write_wav(path: &std::path::Path, samples: &[f32], sample_rate: u32) -> anyhow::Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .with_context(|| format!("failed to create WAV file {}", path.display()))?;
    for &s in samples {
        let s = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        writer.write_sample(s)?;
    }
    writer.finalize()?;
    Ok(())
}

/// Record `duration_sec` seconds of audio to `path` at the target rate.
/// Standalone entry point for the `record` test subcommand.
pub fn record_to_file(
    config: &AudioConfig,
    path: &std::path::Path,
    duration_sec: u64,
) -> anyhow::Result<()> {
    let capture = AudioCapture::start(config)?;
    let target_total = (config.target_rate as u64 * duration_sec) as usize;
    let mut samples = Vec::with_capacity(target_total);

    info!(
        duration_sec,
        rate = config.target_rate,
        path = %path.display(),
        "recording"
    );
    while samples.len() < target_total {
        match capture.recv() {
            Some(chunk) => samples.extend_from_slice(&chunk),
            None => bail!("capture thread died during recording"),
        }
    }
    samples.truncate(target_total);
    drop(capture);

    write_wav(path, &samples, config.target_rate)?;
    info!(samples = samples.len(), "recording written");
    Ok(())
}
