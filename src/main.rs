mod audio;
mod config;
mod openclaw;
mod transcribe;
mod voice;

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
    /// Live pipeline test: Enter simulates the wake word, then records a
    /// command and transcribes it (no rustpotter model needed)
    Listen,
    /// Speak text aloud through the default output (tests TTS)
    Speak {
        /// Text to speak
        text: String,
    },
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

/// Full assistant loop with a keyboard stub for the wake word: press Enter to
/// trigger, speak a command, get it transcribed, dispatched to Orchestre, and
/// the reply spoken back. NOTE: recording starts at the trigger, so the first
/// ~word of a real command would be cut; wakeword.rs will add a pre-trigger
/// ring buffer.
fn listen_loop(config: AppConfig) -> anyhow::Result<()> {
    use std::io::BufRead;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    // Single-threaded runtime for the async pieces (HTTP, TTS synthesis).
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build listen runtime")?;

    let transcriber = transcribe::Transcriber::load(&config.transcription)?;
    let speaker = rt.block_on(voice::Speaker::load(&config.voice))?;
    let client = openclaw::OrchestreClient::new(&config.openclaw)?;
    let capture = audio::AudioCapture::start(&config.audio)?;

    // Enter key → trigger flag (stdin blocks, so it gets its own thread).
    let triggered = Arc::new(AtomicBool::new(false));
    std::thread::spawn({
        let triggered = triggered.clone();
        move || {
            let stdin = std::io::stdin();
            loop {
                match stdin.lock().lines().next() {
                    Some(Ok(_)) => triggered.store(true, Ordering::Relaxed),
                    _ => break, // stdin closed
                }
            }
        }
    });

    tracing::info!("listening — press Enter to simulate the wake word (Ctrl-C to quit)");
    let rate = config.audio.target_rate as usize;
    let need = rate * config.transcription.command_duration_sec as usize;
    let mut collecting: Option<Vec<f32>> = None;

    while let Some(chunk) = capture.recv() {
        if triggered.swap(false, Ordering::Relaxed) {
            tracing::info!(
                secs = config.transcription.command_duration_sec,
                "wake word (stub) — recording command"
            );
            collecting = Some(Vec::with_capacity(need));
        }
        if let Some(buf) = &mut collecting {
            buf.extend_from_slice(&chunk);
            if buf.len() >= need {
                let audio = std::mem::take(buf);
                collecting = None;
                match transcriber.transcribe_to_string(&audio) {
                    Ok(text) if text.is_empty() => println!(">> (nothing recognized)"),
                    Ok(text) => {
                        println!(">> {text}");
                        match rt.block_on(client.send_command(&text)) {
                            Ok(result) => {
                                println!("<< [{}] {}", result.status, result.response.as_deref().unwrap_or("(no reply)"));
                                if let Some(reply) = result.response {
                                    if let Err(e) = rt.block_on(speaker.say(&reply)) {
                                        tracing::error!("speech failed: {e:#}");
                                    }
                                }
                            }
                            Err(e) => tracing::error!("orchestrator dispatch failed: {e:#}"),
                        }
                    }
                    Err(e) => tracing::error!("transcription failed: {e:#}"),
                }
                tracing::info!("listening — press Enter to simulate the wake word");
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
        Some(Command::Listen) => {
            let config = load_config(&cli.config)?;
            init_tracing(&config.logging.level);
            // The listen loop is blocking (ALSA channel recv); run it on a
            // dedicated thread with its own runtime for the async clients
            // (Orchestre HTTP, Kokoro synth).
            let handle = std::thread::spawn(move || listen_loop(config));
            handle
                .join()
                .map_err(|_| anyhow::anyhow!("listen thread panicked"))??;
        }
        Some(Command::Speak { text }) => {
            let config = load_config(&cli.config)?;
            init_tracing(&config.logging.level);
            let speaker = voice::Speaker::load(&config.voice).await?;
            speaker.say(&text).await?;
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
