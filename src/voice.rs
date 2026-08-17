//! Text-to-speech via kokoro-en (Kokoro-82M, in-process ONNX) and ALSA
//! playback.
//!
//! Playback targets the ALSA "default" device: on PipeWire/PulseAudio systems
//! that routes to the user's default sink — in Five's deployment, the
//! Bluetooth stereo.

use std::time::Instant;

use alsa::pcm::{Access, Format, HwParams, PCM};
use alsa::{Direction, ValueOr};
use anyhow::Context;
use kokoro_en::{KokoroTts, Voice};
use tracing::{debug, info};

use crate::config::VoiceConfig;

/// Kokoro's output sample rate (fixed by the model).
const TTS_SAMPLE_RATE: u32 = 24_000;

/// A loaded TTS engine plus the configured voice.
pub struct Speaker {
    tts: KokoroTts,
    voice: String,
    speed: f32,
}

impl Speaker {
    /// Load the Kokoro model and voice pack. Expensive; do it once.
    pub async fn load(config: &VoiceConfig) -> anyhow::Result<Self> {
        if !config.model_path.exists() {
            anyhow::bail!(
                "Kokoro model not found: {} — see HANDOFF.md §8",
                config.model_path.display()
            );
        }
        let start = Instant::now();
        let tts = KokoroTts::new(&config.model_path, &config.voices_dir)
            .await
            .map_err(|e| anyhow::anyhow!("failed to load Kokoro model: {e}"))?;
        info!(
            model = %config.model_path.display(),
            voice = %config.voice,
            load_ms = start.elapsed().as_millis() as u64,
            "Kokoro TTS loaded"
        );
        Ok(Self { tts, voice: config.voice.clone(), speed: config.speed })
    }

    /// Synthesize `text` and play it through the default output.
    pub async fn say(&self, text: &str) -> anyhow::Result<()> {
        let samples = self.synthesize(text).await?;
        play(&samples, TTS_SAMPLE_RATE)
    }

    /// Synthesize `text` to 24 kHz mono f32 samples without playing them.
    pub async fn synthesize(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        let voice = Voice::new(&*self.voice).with_speed(self.speed);
        let start = Instant::now();
        let (audio, _) = self
            .tts
            .synth(text, voice)
            .await
            .map_err(|e| anyhow::anyhow!("TTS synthesis failed: {e}"))?;
        debug!(
            samples = audio.len(),
            synth_ms = start.elapsed().as_millis() as u64,
            "speech synthesized"
        );
        Ok(audio)
    }
}

/// Play mono f32 samples through the ALSA default device as S16_LE.
/// Blocks until playback finishes.
pub fn play(samples: &[f32], sample_rate: u32) -> anyhow::Result<()> {
    let pcm = PCM::new("default", Direction::Playback, false)
        .context("failed to open ALSA default playback device")?;
    {
        let hwp = HwParams::any(&pcm).context("failed to query playback hw params")?;
        hwp.set_channels(1).context("playback device rejected mono")?;
        hwp.set_rate(sample_rate, ValueOr::Nearest)
            .context("playback device rejected sample rate")?;
        hwp.set_format(Format::S16LE)
            .context("playback device rejected S16_LE")?;
        hwp.set_access(Access::RWInterleaved)
            .context("playback device rejected interleaved access")?;
        pcm.hw_params(&hwp).context("failed to apply playback hw params")?;
    }
    pcm.prepare().context("failed to prepare playback device")?;

    let io = pcm.io_i16().context("failed to get playback io handle")?;
    let s16: Vec<i16> = samples
        .iter()
        .map(|s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
        .collect();

    let mut offset = 0;
    while offset < s16.len() {
        match io.writei(&s16[offset..]) {
            Ok(n) => offset += n,
            Err(e) => {
                // Underrun or stream error: recover and retry the write.
                pcm.recover(e.errno(), true)
                    .context("playback recovery failed")?;
            }
        }
    }
    pcm.drain().context("failed to drain playback device")?;
    debug!(frames = s16.len(), rate = sample_rate, "playback complete");
    Ok(())
}
