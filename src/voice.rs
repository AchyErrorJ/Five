//! Text-to-speech via kokoro-en (Kokoro-82M, in-process ONNX) and audio
//! playback.
//!
//! Playback is platform-split: ALSA "default" device on Linux (PipeWire
//! routes to the user's default sink — the Bluetooth stereo), WASAPI default
//! output via cpal on Windows.

use std::time::Instant;

#[cfg(target_os = "linux")]
use alsa::pcm::{Access, Format, HwParams, PCM};
#[cfg(target_os = "linux")]
use alsa::{Direction, ValueOr};
use anyhow::Context;
use kokoro_en::{KokoroTts, Voice};
use tracing::{debug, info};

use crate::config::VoiceConfig;

/// Kokoro's output sample rate (fixed by the model).
pub const TTS_SAMPLE_RATE: u32 = 24_000;

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
        // kokoro-en reads KOKORO_ORT_PROVIDER at session build; set it from
        // config before KokoroTts::new so "cpu" actually takes effect.
        if let Some(provider) = &config.provider {
            // SAFETY-free: set_var is called before any ort threads exist.
            unsafe { std::env::set_var("KOKORO_ORT_PROVIDER", provider) };
        }
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

    /// Synthesize `text` to 24 kHz mono f32 samples without playing them.
    /// The caller decides what to do with them — `play()` to speak, and
    /// `samples.len() / TTS_SAMPLE_RATE` gives the exact speech duration
    /// (used to time on-screen captions).
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
#[cfg(target_os = "linux")]
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

/// Play mono f32 samples through the WASAPI default output device.
/// Blocks until playback finishes.
#[cfg(target_os = "windows")]
pub fn play(samples: &[f32], sample_rate: u32) -> anyhow::Result<()> {
    play_on(samples, sample_rate, None)
}

/// Cross-platform playback with optional device selection (device name is
/// only honored on Windows; Linux always uses the ALSA default).
#[cfg(target_os = "windows")]
pub fn play_out(samples: &[f32], sample_rate: u32, device: Option<&str>) -> anyhow::Result<()> {
    play_on(samples, sample_rate, device)
}

/// Cross-platform playback with optional device selection — Linux ignores
/// the device name and uses the ALSA default (see play above).
#[cfg(target_os = "linux")]
pub fn play_out(samples: &[f32], sample_rate: u32, _device: Option<&str>) -> anyhow::Result<()> {
    play(samples, sample_rate)
}

/// List the names of all output devices (for the `devices` command).
/// Linux playback always uses the ALSA default, so there is nothing to list.
#[cfg(target_os = "linux")]
pub fn output_devices() -> Vec<String> {
    vec!["default (ALSA)".to_string()]
}

/// List the names of all WASAPI output devices (for `devices` command).
#[cfg(target_os = "windows")]
pub fn output_devices() -> Vec<String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let mut out = Vec::new();
    if let Some(d) = host.default_output_device() {
        if let Ok(n) = d.name() {
            out.push(format!("{n} [default]"));
        }
    }
    if let Ok(devs) = host.output_devices() {
        for d in devs {
            if let Ok(n) = d.name() {
                if !out.iter().any(|x| x.starts_with(&n)) {
                    out.push(n);
                }
            }
        }
    }
    out
}

/// Play mono f32 samples through a named WASAPI output device (substring
/// match, case-insensitive), or the system default when `device_name` is
/// None. Blocks until playback finishes.
///
/// cpal output is callback-driven, so this resamples to the device's native
/// rate, duplicates the mono signal across its channel count, and feeds a
/// shared buffer the callback drains. The function returns once every sample
/// has been handed to the device.
#[cfg(target_os = "windows")]
pub fn play_on(samples: &[f32], sample_rate: u32, device_name: Option<&str>) -> anyhow::Result<()> {
    use std::sync::{Arc, Mutex};

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use rubato::{FftFixedIn, Resampler};

    // WASAPI drops the last device-buffer-worth of frames when the stream
    // closes, clipping the final syllable — pad with 1s of silence so what
    // gets dropped is padding, not speech.
    let mut padded = samples.to_vec();
    padded.resize(samples.len() + sample_rate as usize, 0.0);
    let samples = &padded[..];

    let host = cpal::default_host();
    let device = match device_name {
        Some(name) => {
            let needle = name.to_lowercase();
            host.output_devices()
                .context("failed to enumerate output devices")?
                .find(|d| {
                    d.name()
                        .map(|n| n.to_lowercase().contains(&needle))
                        .unwrap_or(false)
                })
                .with_context(|| format!("no output device matching '{name}'"))?
        }
        None => host
            .default_output_device()
            .context("no default output device")?,
    };
    tracing::info!(device = %device.name().unwrap_or_default(), "playing audio");
    let supported = device
        .default_output_config()
        .context("failed to query default output config")?;
    let stream_config: cpal::StreamConfig = supported.clone().into();

    let dev_rate = stream_config.sample_rate.0;
    let channels = stream_config.channels as usize;

    // Resample 24 kHz TTS output to whatever the device runs at.
    let at_device_rate: Vec<f32> = if dev_rate != sample_rate {
        let mut r = FftFixedIn::<f32>::new(sample_rate as usize, dev_rate as usize, samples.len(), 1, 1)
            .context("failed to create playback resampler")?;
        r.process(&[samples.to_vec()], None)
            .context("playback resampling failed")?
            .into_iter()
            .next()
            .unwrap_or_default()
    } else {
        samples.to_vec()
    };

    // Mono → interleaved device channels.
    let mut interleaved = Vec::with_capacity(at_device_rate.len() * channels);
    for &s in &at_device_rate {
        for _ in 0..channels {
            interleaved.push(s);
        }
    }

    let shared = Arc::new(Mutex::new(interleaved));
    let written = Arc::new(Mutex::new(0usize));

    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => build_out::<f32>(&device, &stream_config, &shared, &written),
        cpal::SampleFormat::I16 => build_out::<i16>(&device, &stream_config, &shared, &written),
        cpal::SampleFormat::U16 => build_out::<u16>(&device, &stream_config, &shared, &written),
        fmt => anyhow::bail!("unsupported playback sample format: {fmt:?}"),
    }
    .context("failed to build playback stream")?;
    stream.play().context("failed to start playback stream")?;

    // Block until the audio has actually finished sounding. The callback can
    // run AHEAD of real time (WASAPI pulls big buffers up front), so
    // "all samples handed over" is not "all samples played" — waiting only on
    // `written >= total` cuts off the tail of every sentence. Wait out the
    // full wall-clock duration instead, plus a beat for the device buffer.
    let total = shared.lock().map(|b| b.len()).unwrap_or(0);
    let frames = total / channels.max(1);
    let duration = std::time::Duration::from_secs_f32(frames as f32 / dev_rate as f32);
    let start = std::time::Instant::now();
    loop {
        std::thread::sleep(std::time::Duration::from_millis(20));
        let handed = written.lock().map(|w| *w >= total).unwrap_or(true);
        if handed && start.elapsed() >= duration {
            break;
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(150));
    drop(stream);

    debug!(frames = total, rate = dev_rate, "playback complete");
    Ok(())
}

/// Build an output stream whose callback drains `shared` (f32 samples at the
/// device rate, already interleaved) into the device buffer, converting to
/// the hardware sample format. Past the end of the buffer it emits silence.
#[cfg(target_os = "windows")]
fn build_out<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    shared: &std::sync::Arc<std::sync::Mutex<Vec<f32>>>,
    written: &std::sync::Arc<std::sync::Mutex<usize>>,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::Sample + cpal::SizedSample + cpal::FromSample<f32> + Send + 'static,
{
    use cpal::traits::DeviceTrait;

    let shared = std::sync::Arc::clone(shared);
    let written = std::sync::Arc::clone(written);
    device.build_output_stream(
        config,
        move |out: &mut [T], _| {
            let buf = match shared.lock() {
                Ok(b) => b,
                Err(_) => return,
            };
            let mut w = match written.lock() {
                Ok(w) => w,
                Err(_) => return,
            };
            for slot in out.iter_mut() {
                let s = buf.get(*w).copied().unwrap_or(0.0);
                *slot = T::from_sample(s);
                *w += 1;
            }
        },
        |e| tracing::error!("playback stream error: {e}"),
        None,
    )
}
