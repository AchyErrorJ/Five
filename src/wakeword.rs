//! Wake word model training + offline evaluation (rustpotter).
//!
//! `train` builds a `.rpw` from the positive sample WAVs recorded by
//! `scripts/record-wakeword-samples.sh` (16 kHz mono, top level of the
//! samples dir). `evaluate` scores every positive and negative WAV against
//! the trained model so threshold tuning happens offline, not on the live mic.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use rustpotter::{
    Rustpotter, RustpotterConfig, WakewordRef, WakewordRefBuildFromBuffers, WakewordSave,
};

use crate::config::WakeWordConfig;
use crate::transcribe;

/// Streaming automatic gain control for the detector input. Rustpotter
/// templates are amplitude-sensitive: a model trained on loud samples scores
/// a genuine "five" at 0.000 when it's spoken at 3% peak instead of 30%.
/// This normalizes the stream toward TARGET_RMS with a smoothed, clamped
/// gain; frames at room-noise level pass through untouched so silence stays
/// silence and the max gain can't blow room tone up into "speech".
pub struct Agc {
    rms: f32, // EMA of recent input RMS
}

impl Agc {
    const TARGET_RMS: f32 = 0.05; // ≈ clear speech after normalization
    const MAX_GAIN: f32 = 12.0;
    const NOISE_FLOOR: f32 = 0.004;

    pub fn new() -> Self {
        Self { rms: Self::TARGET_RMS }
    }

    pub fn process(&mut self, samples: &mut [f32]) {
        if samples.is_empty() {
            return;
        }
        let in_rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
        if in_rms >= Self::NOISE_FLOOR {
            // EMA over speech frames only — silence mustn't crank the gain.
            self.rms = 0.7 * self.rms + 0.3 * in_rms;
            let gain = (Self::TARGET_RMS / self.rms.max(1e-4)).clamp(1.0, Self::MAX_GAIN);
            for s in samples.iter_mut() {
                *s = (*s * gain).clamp(-1.0, 1.0);
            }
        }
    }
}

/// MFCC coefficients per frame at build time. Stored inside the .rpw; the
/// detector adapts to it at load, so 40 (rustpotter's own default) is safe.
const MFCC_SIZE: u16 = 40;

/// Cut a recording to the speech region: anchor on the peak-energy 10ms
/// window, expand outward while energy stays above the gate, pad 150ms on
/// both sides. rustpotter treats the whole sample file as the template, so
/// trailing room tone must go — and one bloated template inflates the
/// detector's match window for every other template.
fn trim_to_speech(samples: &[f32], rate: usize) -> Option<Vec<f32>> {
    let win = rate / 100; // 10ms
    let rms: Vec<f32> = samples
        .chunks(win)
        .map(|c| (c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).sqrt())
        .collect();
    if rms.is_empty() {
        return None;
    }
    let mut sorted = rms.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let floor = sorted[sorted.len() / 4].max(1e-4); // 25th percentile
    let peak = rms.iter().copied().fold(0.0f32, f32::max);
    let gate = (floor * 4.0).max(peak * 0.08);
    if peak < floor * 4.0 {
        return None; // no speech above the noise
    }
    let peak_idx = rms
        .iter()
        .position(|&r| r >= peak * 0.99)
        .unwrap_or(rms.len() / 2);
    let mut start = peak_idx;
    while start > 0 && rms[start - 1] >= gate {
        start -= 1;
    }
    let mut end = peak_idx;
    while end < rms.len() - 1 && rms[end + 1] >= gate {
        end += 1;
    }
    let pad = 15; // 150ms in 10ms windows
    let start = start.saturating_sub(pad);
    let end = end.saturating_add(pad).min(rms.len() - 1);
    Some(samples[start * win..=(end * win).min(samples.len() - 1)].to_vec())
}

/// Encode f32 samples as a 16-bit PCM WAV byte buffer (what the MFCC
/// extractor reads).
fn encode_wav_i16(samples: &[f32], rate: u32) -> anyhow::Result<Vec<u8>> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut cursor, spec)?;
        for &s in samples {
            writer.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
        }
        writer.finalize()?;
    }
    Ok(cursor.into_inner())
}

fn positive_samples(samples_dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(samples_dir)
        .with_context(|| format!("cannot read {}", samples_dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "wav"))
        .collect();
    files.sort();
    Ok(files)
}

fn negative_samples(samples_dir: &Path) -> Vec<PathBuf> {
    let neg_dir = samples_dir.join("negative");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&neg_dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "wav"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

pub fn train(samples_dir: &Path, output: &Path) -> anyhow::Result<()> {
    let files = positive_samples(samples_dir)?;
    if files.len() < 2 {
        bail!(
            "need at least 2 positive samples in {}, found {}",
            samples_dir.display(),
            files.len()
        );
    }
    // Trim each sample to the spoken word; rustpotter uses the entire file
    // as the template, and our 2s recordings are mostly room tone.
    let mut buffers = std::collections::HashMap::new();
    let mut skipped = Vec::new();
    for f in &files {
        let (samples, rate) = transcribe::read_wav(f)?;
        match trim_to_speech(&samples, rate as usize) {
            Some(mut trimmed) if trimmed.len() >= rate as usize / 5 => {
                // Peak-normalize every template to the same level — otherwise
                // the model bakes in each recording's mic level and only ever
                // matches utterances at that exact volume.
                let peak = trimmed.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
                if peak > 1e-4 {
                    for s in trimmed.iter_mut() {
                        *s *= 0.5 / peak;
                    }
                }
                let name = f.file_name().unwrap().to_string_lossy().into_owned();
                println!(
                    "  {name:<16} kept {:.2}s of {:.2}s",
                    trimmed.len() as f32 / rate as f32,
                    samples.len() as f32 / rate as f32
                );
                buffers.insert(name, encode_wav_i16(&trimmed, rate)?);
            }
            _ => skipped.push(f.file_name().unwrap().to_string_lossy().into_owned()),
        }
    }
    if !skipped.is_empty() {
        println!("skipped (no speech found): {}", skipped.join(", "));
    }
    if buffers.len() < 2 {
        bail!("only {} usable samples after trimming", buffers.len());
    }
    tracing::info!(count = buffers.len(), "training wake word model");
    let wakeword = WakewordRef::new_from_sample_buffers(
        "five".to_string(),
        None, // threshold — use rustpotter default, tune via config
        None, // avg_threshold — same
        buffers,
        MFCC_SIZE,
    )
    .map_err(|e| anyhow::anyhow!("training failed: {e}"))?;
    wakeword
        .save_to_file(&output.display().to_string())
        .map_err(|e| anyhow::anyhow!("failed to save {}: {e}", output.display()))?;
    println!("trained model saved to {}", output.display());
    Ok(())
}

/// Score one WAV against the model; returns (detected, max partial score).
/// Feeds ~1.5s of trailing silence after the file so the detector's
/// post-match countdown can complete (a detection only finalizes once the
/// score stops improving for half a match-window).
fn score_file(detector: &mut Rustpotter, path: &Path) -> anyhow::Result<(bool, f32)> {
    let (samples, rate) = transcribe::read_wav(path)?;
    if rate != 16000 {
        bail!("{} is {rate} Hz, expected 16000", path.display());
    }
    detector.reset();
    let frame = detector.get_samples_per_frame();
    let silence = vec![0.0f32; frame * 50];
    let stream: Vec<f32> = samples.iter().copied().chain(silence).collect();
    let debug = std::env::var("FIVE_DEBUG_SCORES").is_ok();
    let mut agc = Agc::new();
    let mut max_score: f32 = 0.0;
    let mut detected = false;
    for (i, chunk) in stream.chunks(frame).enumerate() {
        if chunk.len() < frame {
            break; // process_samples silently ignores short frames
        }
        let mut normed = chunk.to_vec();
        agc.process(&mut normed);
        let det = detector.process_samples(normed);
        if debug {
            match detector.get_partial_detection() {
                Some(p) => println!(
                    "    frame {i:3} score={:.3} avg={:.3} counter={}",
                    p.score, p.avg_score, p.counter
                ),
                None => println!("    frame {i:3} (no partial)"),
            }
        }
        if det.is_some() {
            detected = true;
        }
        if let Some(partial) = detector.get_partial_detection() {
            max_score = max_score.max(partial.score);
        }
    }
    Ok((detected, max_score))
}

/// Build a live detector from the app config. Shared by `evaluate` and the
/// `listen` loop so threshold/min_scores can't drift between offline tuning
/// and production behavior.
pub fn build_detector(cfg: &WakeWordConfig) -> anyhow::Result<Rustpotter> {
    let mut config = RustpotterConfig::default();
    config.detector.threshold = cfg.threshold;
    // 2 consecutive above-threshold frames suffice; short, crisp wakewords
    // produce a match peak only one or two frames wide. Safe here because
    // the negatives never cross the threshold at all (max 0.56 < 0.6).
    config.detector.min_scores = cfg.min_scores;
    // The avg-score gate (match against averaged wakeword features) was
    // silently discarding real detections: live utterances scored 0.73 but
    // avg-similarity only ~0.2, below rustpotter's default avg_threshold.
    // 0.0 disables the gate entirely.
    config.detector.avg_threshold = cfg.avg_threshold;
    let mut detector = Rustpotter::new(&config)
        .map_err(|e| anyhow::anyhow!("failed to init rustpotter: {e}"))?;
    detector
        .add_wakeword_from_file("five", &cfg.model_path.display().to_string())
        .map_err(|e| anyhow::anyhow!("failed to load {}: {e}", cfg.model_path.display()))?;
    Ok(detector)
}

pub fn evaluate(samples_dir: &Path, cfg: &WakeWordConfig) -> anyhow::Result<()> {
    let mut detector = build_detector(cfg)?;

    println!("\n=== positives (want: detected) ===");
    let mut hits = 0;
    let positives = positive_samples(samples_dir)?;
    for f in &positives {
        let (detected, max) = score_file(&mut detector, f)?;
        hits += detected as usize;
        println!(
            "  {:<24} {:<4} max_score={:.3}",
            f.file_name().unwrap().to_string_lossy(),
            if detected { "YES" } else { "no" },
            max
        );
    }

    println!("=== negatives (want: no) ===");
    let mut false_pos = 0;
    for f in &negative_samples(samples_dir) {
        let (detected, max) = score_file(&mut detector, f)?;
        false_pos += detected as usize;
        println!(
            "  {:<24} {:<4} max_score={:.3}",
            f.file_name().unwrap().to_string_lossy(),
            if detected { "YES ⚠️" } else { "no" },
            max
        );
    }

    println!(
        "\nsummary: {}/{} positives detected, {}/{} false positives",
        hits,
        positives.len(),
        false_pos,
        negative_samples(samples_dir).len()
    );
    Ok(())
}
