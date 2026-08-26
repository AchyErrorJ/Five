//! Speech-to-text via whisper-rs (in-process whisper.cpp FFI).
//!
//! Loads the ggml model once at startup; `transcribe` then runs inference on
//! 16 kHz mono f32 samples — exactly what `audio::AudioCapture` produces.

use std::path::Path;
use std::time::Instant;

use anyhow::Context;
use tracing::{debug, info};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::config::TranscriptionConfig;

/// A loaded whisper model, ready to transcribe.
pub struct Transcriber {
    ctx: WhisperContext,
    sample_rate: u32,
}

/// One transcribed segment with its time bounds (in milliseconds).
#[derive(Debug, Clone)]
#[allow(dead_code)] // timestamps unused until the OpenClaw client sends them
pub struct Segment {
    pub text: String,
    pub start_ms: i64,
    pub end_ms: i64,
}

impl Transcriber {
    /// Load the whisper model from disk. Expensive (~1s); do it once.
    pub fn load(config: &TranscriptionConfig) -> anyhow::Result<Self> {
        if !config.model_path.exists() {
            anyhow::bail!(
                "whisper model not found: {} — download a ggml-*.bin (see HANDOFF.md §8)",
                config.model_path.display()
            );
        }
        // Route whisper.cpp/ggml native logs through `log` instead of stderr;
        // otherwise every inference dumps token-level spam to the console.
        whisper_rs::install_logging_hooks();
        let start = Instant::now();
        let ctx = WhisperContext::new_with_params(
            &config.model_path,
            WhisperContextParameters::default(),
        )
        .with_context(|| format!("failed to load whisper model {}", config.model_path.display()))?;
        info!(
            model = %config.model_path.display(),
            load_ms = start.elapsed().as_millis() as u64,
            "whisper model loaded"
        );
        Ok(Self { ctx, sample_rate: config.sample_rate })
    }

    /// Transcribe 16 kHz mono f32 samples to text segments.
    pub fn transcribe(&self, samples: &[f32]) -> anyhow::Result<Vec<Segment>> {
        if samples.len() < self.sample_rate as usize / 10 {
            // Less than 100 ms of audio: whisper produces garbage on silence.
            return Ok(Vec::new());
        }

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some("en"));
        params.set_translate(false);
        // Z1 Extreme: 8 cores — whisper.cpp defaults to 4 threads.
        params.set_n_threads(8);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        let mut state = self.ctx.create_state().context("failed to create whisper state")?;
        let start = Instant::now();
        state
            .full(params, samples)
            .context("whisper inference failed")?;
        let elapsed = start.elapsed();

        let n = state.full_n_segments();
        let mut segments = Vec::with_capacity(n as usize);
        for i in 0..n {
            let Some(seg) = state.get_segment(i) else { continue };
            // whisper timestamps are in 10 ms ticks.
            let t0 = seg.start_timestamp() * 10;
            let t1 = seg.end_timestamp() * 10;
            let text = seg.to_str_lossy()?.trim().to_string();
            if !text.is_empty() {
                segments.push(Segment { text, start_ms: t0, end_ms: t1 });
            }
        }

        debug!(
            segments = segments.len(),
            audio_ms = samples.len() as u64 * 1000 / self.sample_rate as u64,
            inference_ms = elapsed.as_millis() as u64,
            "transcription complete"
        );
        Ok(segments)
    }

    /// Transcribe and join all segments into one string.
    pub fn transcribe_to_string(&self, samples: &[f32]) -> anyhow::Result<String> {
        Ok(self
            .transcribe(samples)?
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" "))
    }
}

/// Read a 16-bit PCM WAV file into f32 samples. Companion to
/// `audio::write_wav`; used by the `transcribe` test subcommand.
pub fn read_wav(path: &Path) -> anyhow::Result<(Vec<f32>, u32)> {
    let mut reader = hound::WavReader::open(path)
        .with_context(|| format!("failed to open WAV file {}", path.display()))?;
    let spec = reader.spec();
    let samples: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Int, 16) => reader
            .samples::<i16>()
            .map(|s| s.map(|v| v as f32 / i16::MAX as f32))
            .collect::<Result<_, _>>()?,
        (hound::SampleFormat::Float, 32) => reader
            .samples::<f32>()
            .collect::<Result<_, _>>()?,
        (fmt, bits) => anyhow::bail!("unsupported WAV format: {fmt:?} {bits}-bit"),
    };
    // Downmix to mono if needed.
    let mono: Vec<f32> = if spec.channels > 1 {
        samples
            .chunks_exact(spec.channels as usize)
            .map(|frame| frame.iter().sum::<f32>() / spec.channels as f32)
            .collect()
    } else {
        samples
    };
    Ok((mono, spec.sample_rate))
}
