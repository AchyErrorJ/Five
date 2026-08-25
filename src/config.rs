use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Main application configuration, loaded from YAML file.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppConfig {
    /// Audio input configuration
    pub audio: AudioConfig,
    /// Wake word detection settings
    pub wakeword: WakeWordConfig,
    /// Speech-to-text settings
    pub transcription: TranscriptionConfig,
    /// OpenClaw HTTP endpoint
    pub openclaw: OpenClawConfig,
    /// Ambient recording settings
    pub ambient: AmbientConfig,
    /// Text-to-speech (voice output) settings
    pub voice: VoiceConfig,
    /// On-screen captions while Five speaks (desktop notification)
    #[serde(default)]
    pub captions: CaptionsConfig,
    /// LLM routing: local 4B for easy asks, Kimi coding API for the rest
    #[serde(default)]
    pub brain: BrainConfig,
    /// Logging configuration
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AudioConfig {
    /// ALSA card number (e.g., 2 for Blue Yeti Nano) — Linux only
    #[serde(default)]
    pub alsa_card: u32,
    /// ALSA device number (e.g., 0) — Linux only
    #[serde(default)]
    pub alsa_device: u32,
    /// Input device name substring — Windows only (None = system default)
    #[serde(default)]
    pub input_device: Option<String>,
    /// Windows output device name (substring match); None = system default
    #[serde(default)]
    pub output_device: Option<String>,
    /// Hardware sample rate in Hz (e.g., 48000) — Linux only; Windows uses
    /// the device's native rate
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,
    /// Hardware channels (e.g., 2 for stereo) — Linux only
    #[serde(default = "default_channels")]
    pub channels: u16,
    /// ALSA format string (e.g., "S24_3LE") — Linux only
    #[serde(default = "default_format")]
    pub format: String,
    /// Target sample rate after resampling (e.g., 16000)
    pub target_rate: u32,
    /// Target channels after mixing (e.g., 1 for mono)
    pub target_channels: u16,
    /// Audio chunk size in milliseconds
    pub chunk_ms: u64,
}

fn default_sample_rate() -> u32 {
    48000
}
fn default_channels() -> u16 {
    2
}
fn default_format() -> String {
    "S24_3LE".into()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WakeWordConfig {
    /// When false, skip rustpotter entirely: the listen loop transcribes
    /// every utterance and triggers on the word "five" in the TEXT. Far
    /// more robust — the .rpw templates kept matching the recording
    /// session's room tone instead of the spoken word.
    #[serde(default = "default_wakeword_enabled")]
    pub enabled: bool,
    /// Path to the rustpotter wake word model (.rpw)
    pub model_path: PathBuf,
    /// Detection threshold — minimum average score (0.0 - 1.0)
    pub threshold: f32,
    /// Consecutive above-threshold frames required to fire (rustpotter
    /// min_scores). 2 catches short, crisp utterances; raise if background
    /// speech starts false-triggering.
    #[serde(default = "default_min_scores")]
    pub min_scores: usize,
    /// rustpotter avg_threshold: score against the averaged wakeword features.
    /// 0.0 disables the gate. Live utterances can score well above `threshold`
    /// yet have low avg similarity (avg≈0.2 vs default gate≈0.6) and get
    /// silently discarded — disable unless false positives appear.
    #[serde(default)]
    pub avg_threshold: f32,
}

fn default_wakeword_enabled() -> bool {
    true
}

fn default_min_scores() -> usize {
    2
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TranscriptionConfig {
    /// Path to whisper model file (ggml-*.bin), loaded in-process by whisper-rs
    pub model_path: PathBuf,
    /// Duration to record after wake word in seconds
    pub command_duration_sec: u64,
    /// Expected sample rate for transcription input
    pub sample_rate: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenClawConfig {
    /// Orchestrator base URL (e.g. "http://127.0.0.1:10000")
    pub endpoint: String,
    /// Request timeout in seconds
    pub timeout_sec: u64,
    /// Orchestrator admin password (exchanged for a JWT at startup)
    pub password: String,
    /// Sender agent ID or name — Five's identity in Orchestre.
    /// Must exist and be Running (orchestrator requirement).
    pub from_agent: String,
    /// Recipient agent ID or name — the agent voice commands go to
    pub to_agent: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AmbientConfig {
    /// Enable ambient recording
    pub enabled: bool,
    /// Interval between recordings in minutes
    pub interval_min: u64,
    /// Duration of each recording in seconds
    pub duration_sec: u64,
    /// Directory to store ambient clips
    pub log_dir: PathBuf,
    /// Maximum number of ambient files to keep
    pub max_files: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VoiceConfig {
    /// Path to the Kokoro ONNX model
    pub model_path: PathBuf,
    /// Directory containing voice .bin files (or a single .bin file)
    pub voices_dir: PathBuf,
    /// Voice name, Kokoro convention (e.g. "af_heart", "bm_george")
    pub voice: String,
    /// Speech speed multiplier (1.0 = normal)
    pub speed: f32,
    /// ONNX Runtime execution provider: "auto" (GPU if available) or "cpu".
    /// Maps to kokoro-en's KOKORO_ORT_PROVIDER. DirectML on the AMD iGPU
    /// intermittently fails Kokoro's ConvTranspose node, so Windows = "cpu".
    #[serde(default)]
    pub provider: Option<String>,
    /// Playback loudness multiplier applied to synthesized samples with a
    /// soft (tanh) limiter — 1.0 = synthesized level, 2.0 ≈ +6 dB.
    #[serde(default = "default_gain")]
    pub gain: f32,
}

fn default_gain() -> f32 {
    1.0
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CaptionsConfig {
    /// Show the spoken text as a desktop notification while Five speaks
    pub enabled: bool,
}

impl Default for CaptionsConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BrainConfig {
    /// Master switch — when false, listen falls back to bridge/orchestrator
    #[serde(default)]
    pub enabled: bool,
    /// Persona the local model runs: "tutor" or "orchestrator"
    #[serde(default = "default_persona")]
    pub persona: String,
    /// What the tutor teaches (folded into the tutor system prompt)
    #[serde(default)]
    pub subject: Option<String>,
    /// Memory notebook the local model reads (injected each turn) and writes
    /// (via NOTE: lines). Shared across sessions — this is Five's long-term
    /// memory of the student / ongoing tasks.
    #[serde(default)]
    pub notebook_path: Option<PathBuf>,
    /// LM Studio OpenAI-compatible base URL (no trailing slash)
    #[serde(default = "default_local_url")]
    pub local_url: String,
    /// Local model id (as listed by GET /v1/models)
    #[serde(default = "default_local_model")]
    pub local_model: String,
    /// Kimi (Moonshot) OpenAI-compatible base URL
    #[serde(default = "default_kimi_url")]
    pub kimi_url: String,
    /// Kimi model id
    #[serde(default = "default_kimi_model")]
    pub kimi_model: String,
    /// File containing ONLY the Kimi API key (kept out of the config so the
    /// config can be committed). None = Kimi route unavailable.
    #[serde(default)]
    pub kimi_key_file: Option<PathBuf>,
    /// Max completion tokens per reply. NOTE: the user's 16K budget is the
    /// model's context window; replies are spoken so they stay small.
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// Request timeout in seconds (local 4B on iGPU can be slow to start)
    #[serde(default = "default_brain_timeout")]
    pub timeout_sec: u64,
}

impl Default for BrainConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            persona: default_persona(),
            subject: None,
            notebook_path: None,
            local_url: default_local_url(),
            local_model: default_local_model(),
            kimi_url: default_kimi_url(),
            kimi_model: default_kimi_model(),
            kimi_key_file: None,
            max_tokens: default_max_tokens(),
            timeout_sec: default_brain_timeout(),
        }
    }
}

fn default_persona() -> String {
    "orchestrator".to_string()
}

fn default_local_url() -> String {
    "http://127.0.0.1:1234/v1".into()
}
fn default_local_model() -> String {
    "qwen3.5-4b-mp".into()
}
fn default_kimi_url() -> String {
    "https://api.moonshot.ai/v1".into()
}
fn default_kimi_model() -> String {
    "kimi-k2-0905-preview".into()
}
fn default_max_tokens() -> u32 {
    512
}
fn default_brain_timeout() -> u64 {
    120
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LoggingConfig {
    /// Log level (trace, debug, info, warn, error)
    pub level: String,
    /// Directory for log files
    pub log_dir: PathBuf,
    /// Maximum log file size in MB
    pub max_size_mb: u64,
    /// Maximum number of log files to keep
    pub max_files: usize,
}

impl AppConfig {
    /// Load configuration from a YAML file.
    pub fn from_file(path: &std::path::Path) -> anyhow::Result<Self> {
        let contents = std::fs::read_to_string(path)?;
        let config: AppConfig = serde_yaml::from_str(&contents)?;
        Ok(config)
    }
}
