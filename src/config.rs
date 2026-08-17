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
    /// Logging configuration
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AudioConfig {
    /// ALSA card number (e.g., 2 for Blue Yeti Nano)
    pub alsa_card: u32,
    /// ALSA device number (e.g., 0)
    pub alsa_device: u32,
    /// Hardware sample rate in Hz (e.g., 48000)
    pub sample_rate: u32,
    /// Hardware channels (e.g., 2 for stereo)
    pub channels: u16,
    /// ALSA format string (e.g., "S24_3LE")
    pub format: String,
    /// Target sample rate after resampling (e.g., 16000)
    pub target_rate: u32,
    /// Target channels after mixing (e.g., 1 for mono)
    pub target_channels: u16,
    /// Audio chunk size in milliseconds
    pub chunk_ms: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WakeWordConfig {
    /// Path to the rustpotter wake word model (.rpw)
    pub model_path: PathBuf,
    /// Detection threshold — minimum average score (0.0 - 1.0)
    pub threshold: f32,
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
