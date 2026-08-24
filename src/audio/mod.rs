//! Audio capture with resampling to the target rate/channel count.
//!
//! Two backends behind one API: ALSA on Linux (HANDOFF.md §2 — no cpal
//! abstraction there), WASAPI via cpal on Windows. Both push mono `f32`
//! frames at the target sample rate into an mpsc channel, normalized to
//! [-1.0, 1.0] — the range rustpotter and whisper-rs both expect.

use anyhow::{bail, Context};
use tracing::info;

use crate::config::AudioConfig;

#[cfg(target_os = "linux")]
mod alsa;
#[cfg(target_os = "linux")]
pub use alsa::AudioCapture;

#[cfg(target_os = "windows")]
mod wasapi;
#[cfg(target_os = "windows")]
pub use wasapi::AudioCapture;

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
            None => bail!("capture died during recording"),
        }
    }
    samples.truncate(target_total);
    drop(capture);

    write_wav(path, &samples, config.target_rate)?;
    info!(samples = samples.len(), "recording written");
    Ok(())
}
