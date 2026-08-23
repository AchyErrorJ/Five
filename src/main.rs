mod audio;
mod captions;
mod config;
mod openclaw;
mod transcribe;
mod voice;
mod wakeword;

use anyhow::Context;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

use config::AppConfig;

/// Five — voice assistant daemon for OpenClaw integration.
#[derive(Parser, Debug)]
#[command(name = "five-daemon", version, about)]
struct Cli {
    /// Path to the YAML configuration file
    #[arg(short, long, default_value = "config.yaml", global = true)]
    config: PathBuf,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Record audio to a WAV file (tests capture + resampling)
    Record {
        /// Output WAV path
        #[arg(short, long, default_value = "recording.wav")]
        output: PathBuf,
        /// Duration in seconds
        #[arg(short, long, default_value = "5")]
        duration: u64,
    },
    /// Transcribe a WAV file with whisper (tests STT)
    Transcribe {
        /// Input WAV path (16 kHz mono, as produced by `record`)
        input: PathBuf,
    },
    /// Live assistant loop: rustpotter listens for the wake word on the
    /// mic stream, then records a command, transcribes it, dispatches to
    /// Orchestre, and speaks the reply
    Listen {
        /// Route commands to a file (one per line) instead of Orchestre —
        /// a local Claude Code session tails it, does the work, and replies
        /// via `five-daemon speak`
        #[arg(long, value_name = "FILE")]
        bridge: Option<PathBuf>,
    },
    /// Train the wake word .rpw from recorded samples, then score all
    /// samples (positives and negative/) against the trained model
    TrainWakeword {
        /// Samples directory (positives at top level, negatives in negative/)
        #[arg(short, long, default_value = "models/wakeword-samples")]
        samples: PathBuf,
        /// Output .rpw model path
        #[arg(short, long, default_value = "models/five.rpw")]
        output: PathBuf,
    },
    /// Score sample WAVs against the EXISTING wake word model without
    /// retraining (positives at top level, negatives in negative/)
    EvalWakeword {
        /// Samples directory (positives at top level, negatives in negative/)
        #[arg(short, long, default_value = "models/wakeword-samples")]
        samples: PathBuf,
    },
    /// Speak text aloud through the default output (tests TTS)
    Speak {
        /// Text to speak
        text: String,
    },
}

/// Speak `text` aloud, showing an on-screen caption for exactly the
/// duration of the speech (when captions are enabled in the config).
async fn say_with_caption(
    speaker: &voice::Speaker,
    text: &str,
    captions_enabled: bool,
) -> anyhow::Result<()> {
    let samples = speaker.synthesize(text).await?;
    if captions_enabled {
        let speech = std::time::Duration::from_secs_f32(
            samples.len() as f32 / voice::TTS_SAMPLE_RATE as f32,
        );
        captions::show(text, speech);
    }
    voice::play(&samples, voice::TTS_SAMPLE_RATE)
}

fn init_tracing(level: &str) {
    // Console logging for now; file rotation via tracing-appender lands with logging.rs.
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level)))
        .init();
}

fn load_config(path: &std::path::Path) -> anyhow::Result<AppConfig> {
    AppConfig::from_file(path)
        .with_context(|| format!("failed to load config from {}", path.display()))
}

/// The pre-trigger ring includes the wake word itself, so the transcript
/// usually starts with "five, ...". Strip that leading token (but not
/// lookalikes like "fiver").
fn strip_wakeword(text: &str) -> String {
    let t = text.trim();
    let lower = t.to_lowercase();
    // Whisper often renders the wake word as the digit ("5. do X") — treat
    // "5" the same as "five".
    let Some(rest) = lower
        .strip_prefix("five")
        .or_else(|| lower.strip_prefix('5'))
    else {
        return t.to_string();
    };
    if rest.starts_with(|c: char| c.is_alphabetic()) {
        return t.to_string();
    }
    let offset = t.len() - rest.len();
    t[offset..]
        .trim_start_matches([',', '.', '!', '?', ';', ':', ' '])
        .to_string()
}

/// Full assistant loop: rustpotter listens for the wake word on the live
/// stream, then records a command, transcribes, dispatches to Orchestre,
/// and speaks the reply. A pre-trigger ring buffer keeps the ~2s before
/// detection fires — rustpotter only finalizes a detection once its match
/// window expires (~0.5s after the wakeword peak), and by then the user may
/// already be mid-command, so the ring keeps the first syllables.
fn listen_loop(config: AppConfig, bridge: Option<PathBuf>) -> anyhow::Result<()> {
    use std::collections::VecDeque;

    // Single-threaded runtime for the async pieces (HTTP, TTS synthesis).
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build listen runtime")?;

    let transcriber = transcribe::Transcriber::load(&config.transcription)?;
    let speaker = rt.block_on(voice::Speaker::load(&config.voice))?;
    // Bridge mode skips the orchestrator entirely — commands go to a file
    // for a local Claude Code session; it speaks its own replies.
    let client = if bridge.is_none() {
        Some(openclaw::OrchestreClient::new(&config.openclaw)?)
    } else {
        None
    };
    let capture = audio::AudioCapture::start(&config.audio)?;
    let mut detector = wakeword::build_detector(&config.wakeword)?;

    let rate = config.audio.target_rate as usize;
    let frame = detector.get_samples_per_frame();
    let command_len = rate * config.transcription.command_duration_sec as usize;
    let ring_cap = rate * 2; // 2s of pre-trigger audio
    let mut ring: VecDeque<f32> = VecDeque::with_capacity(ring_cap);
    // Incoming chunks aren't aligned to rustpotter's frame size; buffer the
    // remainder between chunks.
    let mut pending: Vec<f32> = Vec::with_capacity(frame * 2);
    let mut command: Option<Vec<f32>> = None;
    // End-of-speech endpointing: finish the command after 1.5s of trailing
    // silence (min 1s of speech) instead of always waiting the full
    // command_duration_sec — a fixed 10s window made every reply feel laggy.
    let min_command = rate; // 1s
    let silence_limit = rate * 3 / 2; // 1.5s
    let silence_rms: f32 = 0.01; // speech RMS ≈0.04, room noise well under 0.005
    let mut trailing_silence = 0usize;
    // FIVE_DEBUG_SCORES=1 logs every partial detection above a noise floor —
    // for diagnosing "wake word never fires" on the live mic.
    let debug_scores = std::env::var("FIVE_DEBUG_SCORES").is_ok();

    tracing::info!(
        threshold = config.wakeword.threshold,
        min_scores = config.wakeword.min_scores,
        "listening for wake word (Ctrl-C to quit)"
    );

    while let Some(chunk) = capture.recv() {
        // --- Collecting a command: accumulate until command_len, then handle.
        if let Some(buf) = &mut command {
            let rms = (chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len().max(1) as f32).sqrt();
            if rms < silence_rms {
                trailing_silence += chunk.len();
            } else {
                trailing_silence = 0;
            }
            buf.extend_from_slice(&chunk);
            let endpointed = buf.len() >= min_command && trailing_silence >= silence_limit;
            if buf.len() < command_len && !endpointed {
                continue;
            }
            let audio = std::mem::take(buf);
            command = None;
            match transcriber.transcribe_to_string(&audio) {
                Ok(text) => {
                    let text = strip_wakeword(&text);
                    if text.is_empty() {
                        println!(">> (nothing recognized)");
                    } else {
                        println!(">> {text}");
                        if let Some(path) = &bridge {
                            use std::io::Write;
                            let mut f = std::fs::OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open(path)
                                .with_context(|| format!("failed to open bridge {}", path.display()))?;
                            writeln!(f, "{text}")?;
                            println!("<< [bridged to Claude Code]");
                        } else {
                            let client = client.as_ref().expect("client when no bridge");
                            match rt.block_on(client.send_command(&text)) {
                            Ok(result) => {
                                println!(
                                    "<< [{}] {}",
                                    result.status,
                                    result.response.as_deref().unwrap_or("(no reply)")
                                );
                                if let Some(reply) = result.response {
                                    if let Err(e) = rt.block_on(say_with_caption(
                                        &speaker,
                                        &reply,
                                        config.captions.enabled,
                                    )) {
                                        tracing::error!("speech failed: {e:#}");
                                    }
                                }
                            }
                            Err(e) => tracing::error!("orchestrator dispatch failed: {e:#}"),
                            }
                        }
                    }
                }
                Err(e) => tracing::error!("transcription failed: {e:#}"),
            }
            // Drop audio that piled up during transcription/playback —
            // it contains Five's own voice and must not retrigger detection.
            while capture.receiver().try_recv().is_ok() {}
            pending.clear();
            ring.clear();
            detector.reset();
            tracing::info!("listening for wake word");
            continue;
        }

        // --- Listening: keep the ring current, feed rustpotter frame-aligned.
        if chunk.len() >= ring_cap {
            ring.clear();
            ring.extend(chunk[chunk.len() - ring_cap..].iter().copied());
        } else {
            let overflow = ring.len() + chunk.len() - ring_cap.min(ring.len() + chunk.len());
            ring.drain(..overflow);
            ring.extend(chunk.iter().copied());
        }
        pending.extend_from_slice(&chunk);
        while pending.len() >= frame {
            let f: Vec<f32> = pending.drain(..frame).collect();
            let hit = detector.process_samples(f).is_some();
            if debug_scores {
                if let Some(p) = detector.get_partial_detection() {
                    if p.score > 0.2 {
                        tracing::info!(score = p.score, avg = p.avg_score, counter = p.counter, "partial detection");
                    }
                }
            }
            if hit {
                tracing::info!("wake word detected — recording command");
                let mut buf: Vec<f32> = ring.iter().copied().collect();
                buf.reserve(command_len);
                command = Some(buf);
                trailing_silence = 0;
                pending.clear();
                break;
            }
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::Record { output, duration }) => {
            let config = load_config(&cli.config)?;
            init_tracing(&config.logging.level);
            audio::record_to_file(&config.audio, &output, duration)?;
        }
        Some(Command::Transcribe { input }) => {
            let config = load_config(&cli.config)?;
            init_tracing(&config.logging.level);
            let (samples, rate) = transcribe::read_wav(&input)?;
            if rate != config.transcription.sample_rate {
                anyhow::bail!(
                    "WAV is {} Hz but whisper expects {} Hz — use `record` output",
                    rate,
                    config.transcription.sample_rate
                );
            }
            let transcriber = transcribe::Transcriber::load(&config.transcription)?;
            let text = transcriber.transcribe_to_string(&samples)?;
            println!("{text}");
        }
        Some(Command::Listen { bridge }) => {
            let config = load_config(&cli.config)?;
            init_tracing(&config.logging.level);
            // The listen loop is blocking (ALSA channel recv); run it on a
            // dedicated thread with its own runtime for the async clients
            // (Orchestre HTTP, Kokoro synth).
            let handle = std::thread::spawn(move || listen_loop(config, bridge));
            handle
                .join()
                .map_err(|_| anyhow::anyhow!("listen thread panicked"))??;
        }
        Some(Command::TrainWakeword { samples, output }) => {
            let config = load_config(&cli.config)?;
            init_tracing(&config.logging.level);
            wakeword::train(&samples, &output)?;
            wakeword::evaluate(&samples, &config.wakeword)?;
        }
        Some(Command::EvalWakeword { samples }) => {
            let config = load_config(&cli.config)?;
            init_tracing(&config.logging.level);
            wakeword::evaluate(&samples, &config.wakeword)?;
        }
        Some(Command::Speak { text }) => {
            let config = load_config(&cli.config)?;
            init_tracing(&config.logging.level);
            let speaker = voice::Speaker::load(&config.voice).await?;
            say_with_caption(&speaker, &text, config.captions.enabled).await?;
        }
        None => {
            let config = load_config(&cli.config)?;
            init_tracing(&config.logging.level);

            tracing::info!(
                version = env!("CARGO_PKG_VERSION"),
                config = %cli.config.display(),
                "five-daemon starting"
            );
            tracing::debug!(?config, "loaded configuration");

            // TODO: wakeword, transcribe, openclaw, ambient (see HANDOFF.md §7)
            tracing::warn!("no subsystems implemented yet — exiting");
        }
    }

    Ok(())
}
