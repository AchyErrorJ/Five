mod audio;
mod brain;
mod captions;
mod config;
mod dashboard;
mod files;
mod almanach;
mod openclaw;
mod search;
mod transcribe;
mod voice;
mod home;
mod manifest;
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
    /// List audio output devices (for the audio.output_device config key)
    Devices,
}

/// Speak `text` aloud, showing an on-screen caption for exactly the
/// duration of the speech (when captions are enabled in the config).
async fn say_with_caption(
    speaker: &voice::Speaker,
    text: &str,
    captions_enabled: bool,
    device: Option<&str>,
) -> anyhow::Result<()> {
    let t0 = std::time::Instant::now();
    let samples = speaker.synthesize(text).await?;
    tracing::info!(synth_ms = t0.elapsed().as_millis() as u64, "speech synthesized");
    if captions_enabled {
        let speech = std::time::Duration::from_secs_f32(
            samples.len() as f32 / voice::TTS_SAMPLE_RATE as f32,
        );
        captions::show(text, speech);
    }
    voice::play_out(&samples, voice::TTS_SAMPLE_RATE, device)
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

/// Whisper renders music, silence, and ambient noise as parenthesized junk —
/// "(upbeat music)", "[Music]", "♪". Always-listening mode would otherwise
/// log (and scan for the trigger word on) one of these every few seconds
/// whenever media is playing near the mic.
fn is_non_speech(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty()
        || (t.starts_with(['(', '[']) && t.ends_with([')', ']']))
        || t.chars().all(|c| !c.is_alphanumeric())
    {
        return true;
    }
    // Whisper's classic hallucinations on background music/TV — short,
    // fixed phrases it emits when there's no real speech.
    const HALLUCINATIONS: &[&str] = &[
        "you", "thank you", "thank you.", "thanks", "thanks for watching",
        "bye", "bye-bye", "so", "okay", "hmm", "uh-huh",
    ];
    HALLUCINATIONS.iter().any(|h| t.eq_ignore_ascii_case(h))
}

/// Find the word "five" (or "5") in a transcript and return everything
/// after it. Whole-word match only — "fiver" must not trigger. This is the
/// text-mode wake word: whisper hears "five" reliably even when rustpotter's
/// MFCC templates don't.
fn extract_command(text: &str) -> Option<String> {
    let mut pos = 0;
    while pos < text.len() {
        let ws = match text[pos..].find(|c: char| c.is_alphanumeric()) {
            Some(i) => pos + i,
            None => return None,
        };
        let we = text[ws..]
            .find(|c: char| !c.is_alphanumeric())
            .map(|i| ws + i)
            .unwrap_or(text.len());
        let word = &text[ws..we];
        if word.eq_ignore_ascii_case("five") || word == "5" {
            let rest = text[we..]
                .trim_start_matches([',', '.', '!', '?', ';', ':', ' '])
                .trim();
            return Some(rest.to_string());
        }
        pos = we;
    }
    None
}

/// Deterministic local answers for questions an LLM can only hallucinate —
/// time and date come from the system clock, phrased for speech. Anything
/// not matched here falls through to the brain/agents.
fn local_answer(
    text: &str,
    home_client: &Option<std::sync::Arc<home::HomeClient>>,
    has_files: bool,
    has_brain: bool,
) -> Option<String> {
    let t = text.to_lowercase();
    let now = chrono::Local::now();

    if t.contains("time") && (t.contains("what") || t.contains("current") || t.contains("tell")) {
        return Some(now.format("It's %-I:%M %p.").to_string());
    }
    if (t.contains("date") || t.contains("day is")) && (t.contains("what") || t.contains("today")) {
        return Some(now.format("It's %A, %B %-d.").to_string());
    }

    if manifest::wants_device_list(&t) {
        if let Some(ref home) = home_client {
            let (devices, scenes) = home.list_entities();
            if devices.is_empty() && scenes.is_empty() {
                return Some("No devices or scenes configured yet.".to_string());
            }
            let mut parts = Vec::new();
            if !devices.is_empty() {
                parts.push(format!("Your lights: {}.", natural_list(&devices)));
            }
            if !scenes.is_empty() {
                parts.push(format!("Your scenes: {}.", natural_list(&scenes)));
            }
            return Some(parts.join(" "));
        }
        return Some("Smart home is not configured.".to_string());
    }

    if manifest::wants_help(&t) {
        let (devices, scenes) = home_client
            .as_ref()
            .map(|h| h.list_entities())
            .unwrap_or_default();
        return Some(manifest_help_text(&devices, &scenes, has_files, has_brain));
    }

    None
}

/// "a, b, and c" — speakable list.
fn natural_list(items: &[String]) -> String {
    match items.len() {
        0 => String::new(),
        1 => items[0].clone(),
        2 => format!("{} and {}", items[0], items[1]),
        _ => {
            let all_but_last = items[..items.len() - 1].join(", ");
            format!("{}, and {}", all_but_last, items.last().unwrap())
        }
    }
}

/// Voice-friendly help text, compressed for speech.
fn manifest_help_text(devices: &[String], scenes: &[String], has_files: bool, has_brain: bool) -> String {
    let mut parts = vec![
        "Here's what I can do.".to_string(),
        "General chat: ask me anything.".to_string(),
        "Conversation: after saying five once, just keep talking — I'm still listening.".to_string(),
        "Modes: say switch to D M mode, or back to normal.".to_string(),
        "Coding: say switch to coding mode to talk to Claude Code; say back to normal to exit.".to_string(),
        "Context: say clear context to start fresh.".to_string(),
        "Time: ask what time is it.".to_string(),
    ];

    if has_brain {
        parts.push("Memory: I keep a notebook between sessions, and remember useful facts.".to_string());
        parts.push("Search: ask me to look something up on the web.".to_string());
        parts.push("Lessons: say make a lesson plan for, then a topic — then start the lesson, and I'll teach it step by step. Say next section, skip to, list lessons, or end the lesson anytime.".to_string());
    }

    if has_files {
        parts.push("Notes: say write this down, or save as, then a filename.".to_string());
    }

    if !devices.is_empty() {
        let device_list = natural_list(devices);
        parts.push(format!(
            "Lights: turn on or off the {device_list}. \
             Dim: set the {device_list} to fifty percent. \
             Color: set the {device_list} to red, blue, or warm.",
        ));
    }

    if !scenes.is_empty() {
        let scene_list = natural_list(scenes);
        parts.push(format!("Scenes: activate {scene_list}.",));
    }

    parts.push("Help: say help to hear this again.".to_string());

    parts.join(" ")
}

/// Adaptive noise floor for endpointing. The room is rarely quiet (TV,
/// the old EMA only drifted UP at 0.1%/chunk toward loud chunks — with the
/// TV on, the floor stayed low, "silence" never tripped, and every
/// utterance ran out to the 10s cap (~9s of dead air per command).
///
/// Instead, track the *background* level — whatever the room sounds like
/// right now, TV included — with a fast (~5s) EMA over ALL chunks, and use
/// hysteresis around it: speech is 4x background, silence is 2x. Constant
/// TV becomes the floor instead of defeating the endpointing; a person
/// talking to the mic is far louder than the TV across the room, so their
/// voice still crosses the speech threshold.
struct NoiseFloor(f32);

impl NoiseFloor {
    fn new() -> Self {
        Self(0.01) // seed near a typical quiet-room floor
    }
    /// Fold one chunk's RMS into the background estimate (~5s time constant;
    /// a 2-4s utterance only nudges it, sustained TV sets it).
    fn update(&mut self, rms: f32) {
        self.0 = self.0 * 0.98 + rms * 0.02;
    }
    /// Loud enough to count as speech (someone addressing the mic).
    fn is_speech(&self, rms: f32) -> bool {
        rms > (self.0 * 4.0).clamp(0.008, 0.2)
    }
    /// Quiet enough to count toward trailing silence (background or below).
    fn is_silence(&self, rms: f32) -> bool {
        rms < (self.0 * 2.0).clamp(0.004, 0.1)
    }
}

/// Split a reply into speakable sentences (keeping their punctuation), so
/// streaming playback can start on sentence one while the rest synthesize.
fn split_sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in text.chars() {
        cur.push(c);
        if matches!(c, '.' | '!' | '?') {
            let s = cur.trim().to_string();
            if !s.is_empty() {
                out.push(s);
            }
            cur.clear();
        }
    }
    let s = cur.trim().to_string();
    if !s.is_empty() {
        out.push(s);
    }
    out
}

/// Speak a reply sentence-by-sentence: synthesize sentence N+1 while
/// sentence N is still playing. A multi-sentence reply starts sounding
/// after ONE sentence's synth time instead of the whole reply's — with
/// kokoro at ~1.2s/sentence that's most of the perceived latency gone.
fn say_streamed(
    speaker: &voice::Speaker,
    text: &str,
    rt: &tokio::runtime::Runtime,
    device: Option<&str>,
) -> anyhow::Result<()> {
    let device = device.map(|d| d.to_string());
    let sentences = split_sentences(text);
    let mut playing: Option<std::thread::JoinHandle<()>> = None;
    for sentence in &sentences {
        let t0 = std::time::Instant::now();
        let samples = rt.block_on(speaker.synthesize(sentence))?;
        tracing::info!(synth_ms = t0.elapsed().as_millis() as u64, sentence, "sentence synthesized");
        if let Some(h) = playing.take() {
            let _ = h.join();
        }
        let device = device.clone();
        playing = Some(std::thread::spawn(move || {
            if let Err(e) = voice::play_out(&samples, voice::TTS_SAMPLE_RATE, device.as_deref()) {
                tracing::error!("playback failed: {e:#}");
            }
        }));
    }
    if let Some(h) = playing {
        let _ = h.join();
    }
    Ok(())
}

/// Deterministic "clear context" command: wipes the brain's rolling
/// conversation history (the notebook — long-term memory — stays).
fn wants_context_clear(text: &str) -> bool {
    let t = text.to_lowercase();
    t.contains("clear context")
        || t.contains("clear your context")
        || t.contains("new conversation")
        || t.contains("forget this conversation")
        || t.contains("forget everything we talked about")
}

/// Detect a request to switch brain modes: "switch to dm mode",
/// "dm mode", "normal mode", "back to normal", etc.
/// Returns the mode name (empty string for "reset to base / normal").
fn wants_mode_switch(text: &str) -> Option<String> {
    let t = text.to_lowercase();
    // Explicit "switch to X mode"
    if let Some(rest) = t.strip_prefix("switch to ") {
        let mode = rest.trim().trim_end_matches(" mode").trim();
        return Some(mode.to_string());
    }
    if let Some(rest) = t.strip_prefix("activate ") {
        let mode = rest.trim().trim_end_matches(" mode").trim();
        return Some(mode.to_string());
    }
    // Shorthand: "dm mode", "deep think mode"
    if t == "dm mode" || t == "deep think mode" || t == "deep thinking mode" {
        return Some("dm".to_string());
    }
    // "coding mode" — handled as a routing mode, not a brain mode
    if t == "coding mode" || t == "code mode" {
        return Some("coding".to_string());
    }
    if t == "normal mode" || t == "default mode" || t == "back to normal" {
        return Some("".to_string());
    }
    None
}

/// Lesson plan voice commands, handled deterministically before anything else.
enum LessonCmd {
    Create(String),
    List,
    Start(String),
    End,
    /// Advance to the next section.
    Next,
    /// Jump to the section matching these keywords.
    Jump(String),
}

/// Drop whisper's leading noise tags — "[clears throat] start the lesson"
/// should parse as "start the lesson".
fn strip_leading_noise(t: &str) -> &str {
    let mut t = t.trim_start();
    while t.starts_with(['[', '(']) {
        match t.find([']', ')']) {
            Some(i) => t = t[i + 1..].trim_start(),
            None => break,
        }
    }
    t
}

/// Detect a lesson command: "make a lesson plan for X", "list lessons",
/// "start the lesson" / "teach me X", "end the lesson".
fn wants_lesson_command(text: &str) -> Option<LessonCmd> {
    let lowered = text.trim().to_lowercase();
    let t = strip_leading_noise(&lowered);
    let t = t.trim_end_matches(['.', '!', '?']);

    // Next section
    if matches!(
        t,
        "next section" | "next part" | "next lesson section" | "move on" | "move to the next section"
            | "go on" | "continue the lesson" | "continue lesson" | "skip ahead"
    ) {
        return Some(LessonCmd::Next);
    }

    // Jump: "skip to X", "jump to the section on X", "go to the part about X"
    for prefix in ["skip to the section on ", "skip to the section ", "skip to ", "jump to the section on ", "jump to the section ", "jump to ", "go to the part about ", "go to the section on ", "go to the section "] {
        if let Some(rest) = t.strip_prefix(prefix) {
            let rest = rest.trim();
            if !rest.is_empty() {
                return Some(LessonCmd::Jump(rest.to_string()));
            }
        }
    }

    // End
    if matches!(
        t,
        "end lesson" | "end the lesson" | "stop lesson" | "stop the lesson"
            | "drop the lesson" | "finish the lesson" | "lesson over"
    ) {
        return Some(LessonCmd::End);
    }

    // List
    if t.contains("list lessons")
        || t.contains("list my lessons")
        || t.contains("list lesson plans")
        || t.contains("what lessons")
        || t.contains("which lessons")
        || t == "lessons"
        || t == "my lessons"
    {
        return Some(LessonCmd::List);
    }

    // Create: "<verb> (a|the) lesson plan <prep> <topic>"
    const CREATE_VERBS: &[&str] = &[
        "make", "create", "write", "generate", "build", "draft", "new",
    ];
    const TOPIC_PREPS: &[&str] = &[" for ", " about ", " on "];
    if t.contains("lesson plan") || t.contains("lessons plan") || t.contains("lessonplan")
        || t.contains("lesson on") || t.contains("lesson about") {
        let looks_like_create = CREATE_VERBS.iter().any(|v| {
            t.starts_with(&format!("{v} ")) || t.starts_with(&format!("{v} me "))
                || t.starts_with(&format!("{v} a ")) || t.starts_with(&format!("{v} the "))
        });
        if looks_like_create {
            // Topic is whatever follows the first preposition; fall back to the
            // tail after "lesson plan"/"lesson".
            for prep in TOPIC_PREPS {
                if let Some(idx) = t.find(prep) {
                    let topic = t[idx + prep.len()..].trim();
                    if !topic.is_empty() {
                        return Some(LessonCmd::Create(topic.to_string()));
                    }
                }
            }
            if let Some(idx) = t.find("lesson") {
                let topic = t[idx + "lesson".len()..]
                    .trim_start_matches(" plan")
                    .trim();
                if !topic.is_empty() {
                    return Some(LessonCmd::Create(topic.to_string()));
                }
            }
            return None; // "make a lesson plan" with no topic — let the LLM ask
        }
    }

    // Start: "start lesson", "start the lesson on X", "teach me X", "load lesson X"
    for prefix in ["start the lesson", "start a lesson", "start lesson", "start my lesson", "begin the lesson", "begin a lesson", "begin lesson", "load lesson", "open lesson", "resume lesson", "resume the lesson", "do the lesson", "do a lesson", "do lesson"] {
        if let Some(rest) = t.strip_prefix(prefix) {
            let rest = rest
                .trim()
                .trim_start_matches("on ")
                .trim_start_matches("about ")
                .trim();
            return Some(LessonCmd::Start(rest.to_string()));
        }
    }
    if let Some(rest) = t.strip_prefix("teach me ") {
        let rest = rest.trim().trim_start_matches("about ");
        return Some(LessonCmd::Start(rest.to_string()));
    }
    if t == "teach me" || t == "start teaching" || t == "start the lesson" {
        return Some(LessonCmd::Start(String::new()));
    }

    // "do the reading buildings lesson plan" / "teach the X lesson" —
    // leading verb + trailing "lesson (plan)" with the lesson name inside.
    if t.ends_with("lesson plan") || t.ends_with("lesson") {
        if let Some(mid) = t
            .strip_prefix("do the ")
            .or_else(|| t.strip_prefix("do "))
            .or_else(|| t.strip_prefix("teach the "))
        {
            let mid = mid
                .trim_end_matches("lesson plan")
                .trim_end_matches("lesson")
                .trim();
            return Some(LessonCmd::Start(mid.to_string()));
        }
    }

    None
}

fn wants_conversation_end(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    let t = t.trim_end_matches(['.', '!', '?']);
    matches!(
        t,
        "never mind" | "nevermind" | "that's all" | "thats all" | "that is all"
            | "goodbye" | "good bye" | "bye" | "stop listening" | "thanks five"
            | "thank you five" | "thanks" | "thank you"
    )
}

/// A fully pre-computed answer to the predicted follow-up: reply text plus
/// one synthesized audio buffer per sentence, ready to play instantly.
struct Speculation {
    question: String,
    reply: String,
    audio: Vec<Vec<f32>>,
}

/// Speculative follow-up: after each brain reply, predict the next question,
/// pre-generate and pre-synthesize its answer in a background thread. If the
/// user's next utterance matches the prediction, it plays with zero brain or
/// synth latency. `gen` invalidates in-flight work on any new utterance.
#[derive(Clone)]
struct SpecCtx {
    brain: std::sync::Arc<brain::Brain>,
    speaker: std::sync::Arc<voice::Speaker>,
    rt: std::sync::Arc<tokio::runtime::Runtime>,
    store: std::sync::Arc<std::sync::Mutex<Option<Speculation>>>,
    gen: std::sync::Arc<std::sync::atomic::AtomicU64>,
    device: Option<String>,
}

impl SpecCtx {
    fn spawn(&self) {
        let me = self.clone();
        std::thread::spawn(move || {
            use std::sync::atomic::Ordering::SeqCst;
            let my_gen = me.gen.load(SeqCst);
            let question = match me.rt.block_on(me.brain.predict_followup()) {
                Ok(Some(q)) => q,
                Ok(None) => return,
                Err(e) => {
                    tracing::warn!("follow-up prediction failed: {e:#}");
                    return;
                }
            };
            if me.gen.load(SeqCst) != my_gen {
                return;
            }
            let brain = me.brain.clone();
            let speaker = me.speaker.clone();
            let q = question.clone();
            let result = me.rt.block_on(async move {
                let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(8);
                let producer = brain.respond_stream(&q, tx, false);
                let consumer = async {
                    let mut audio = Vec::new();
                    while let Some(sentence) = rx.recv().await {
                        audio.push(speaker.synthesize(&sentence).await?);
                    }
                    Ok::<Vec<Vec<f32>>, anyhow::Error>(audio)
                };
                let (reply, audio) = tokio::join!(producer, consumer);
                Ok::<(String, Vec<Vec<f32>>), anyhow::Error>((reply?, audio?))
            });
            if me.gen.load(SeqCst) != my_gen {
                return;
            }
            match result {
                Ok((reply, audio)) if !audio.is_empty() => {
                    tracing::info!(question, "speculative follow-up ready");
                    *me.store.lock().unwrap() = Some(Speculation { question, reply, audio });
                }
                Ok(_) => {}
                Err(e) => tracing::warn!("speculation failed: {e:#}"),
            }
        });
    }

    /// Invalidate in-flight speculation, then play the buffered answer if the
    /// utterance matches the prediction. Returns true when it handled the turn.
    fn try_play(&self, asked: &str) -> bool {
        self.gen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let spec = self.store.lock().unwrap().take();
        let Some(spec) = spec else { return false };
        if !spec_matches(asked, &spec.question) {
            tracing::info!(asked, predicted = spec.question, "speculation missed");
            return false;
        }
        println!("<< (anticipated: {})", spec.question);
        self.brain.push_history(asked, &spec.reply);
        let mut playing: Option<std::thread::JoinHandle<()>> = None;
        for samples in spec.audio {
            if let Some(h) = playing.take() {
                let _ = h.join();
            }
            let device = self.device.clone();
            playing = Some(std::thread::spawn(move || {
                if let Err(e) = voice::play_out(&samples, voice::TTS_SAMPLE_RATE, device.as_deref()) {
                    tracing::error!("playback failed: {e:#}");
                }
            }));
        }
        if let Some(h) = playing {
            let _ = h.join();
        }
        true
    }
}

/// Word-overlap match between what the user actually asked and the predicted
/// follow-up: at least 60% of the actual question's content words must appear
/// in the prediction (the prediction is allowed extra words).
fn spec_matches(asked: &str, predicted: &str) -> bool {
    fn words(s: &str) -> Vec<String> {
        const STOP: &[&str] = &[
            "the", "and", "for", "you", "your", "what", "how", "why", "does", "can",
            "could", "would", "that", "this", "with", "about", "tell", "five",
        ];
        s.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 2 && !STOP.contains(w))
            .map(str::to_string)
            .collect()
    }
    let a = words(asked);
    if a.is_empty() {
        return false;
    }
    let p: std::collections::HashSet<String> = words(predicted).into_iter().collect();
    let hits = a.iter().filter(|w| p.contains(*w)).count();
    hits * 5 >= a.len() * 3
}

/// Ask the brain and speak the reply as it streams: the brain forwards
/// each sentence the moment it arrives over SSE, we synthesize and play
/// them pipelined. First audio lands after sentence one's synth (~1.5s)
/// instead of after the whole reply generates. Returns the full reply.
fn ask_and_speak(
    brain: &brain::Brain,
    speaker: &voice::Speaker,
    rt: &tokio::runtime::Runtime,
    device: Option<&str>,
    text: &str,
    ack: &Option<Vec<f32>>,
) -> anyhow::Result<String> {
    // Instant ack while the brain spins up — it becomes the first playback
    // in the pipeline, so sentences queue behind it naturally.
    let acking = ack.as_ref().map(|samples| {
        let samples = samples.clone();
        let device = device.map(|d| d.to_string());
        std::thread::spawn(move || {
            let _ = voice::play_out(&samples, voice::TTS_SAMPLE_RATE, device.as_deref());
        })
    });
    rt.block_on(async {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(8);
        let producer = brain.respond_stream(text, tx, true);
        let consumer = async {
            let mut playing = acking;
            let mut first = true;
            while let Some(sentence) = rx.recv().await {
                let t0 = std::time::Instant::now();
                let samples = speaker.synthesize(&sentence).await?;
                tracing::info!(
                    synth_ms = t0.elapsed().as_millis() as u64,
                    first,
                    sentence,
                    "sentence synthesized"
                );
                first = false;
                if let Some(h) = playing.take() {
                    let _ = h.join();
                }
                let device = device.map(|d| d.to_string());
                playing = Some(std::thread::spawn(move || {
                    if let Err(e) = voice::play_out(&samples, voice::TTS_SAMPLE_RATE, device.as_deref()) {
                        tracing::error!("playback failed: {e:#}");
                    }
                }));
            }
            if let Some(h) = playing {
                let _ = h.join();
            }
            Ok::<(), anyhow::Error>(())
        };
        let (reply, consumed) = tokio::join!(producer, consumer);
        consumed?;
        reply
    })
}

/// Route one recognized command: append to the bridge file, answer locally
/// (local 4B / Kimi) and speak its reply, or dispatch to Orchestre.
fn dispatch_command(
    text: &str,
    bridge: &Option<PathBuf>,
    coding: &Option<PathBuf>,
    coding_active: &std::cell::Cell<bool>,
    brain: &Option<std::sync::Arc<brain::Brain>>,
    home_client: &Option<std::sync::Arc<home::HomeClient>>,
    file_mgr: &Option<std::sync::Arc<files::FileManager>>,
    almanach_client: &Option<std::sync::Arc<tokio::sync::Mutex<almanach::AlmanachClient>>>,
    client: &Option<openclaw::OrchestreClient>,
    speaker: &voice::Speaker,
    rt: &tokio::runtime::Runtime,
    captions_enabled: bool,
    device: Option<&str>,
    ack: &Option<Vec<f32>>,
    spec: Option<&SpecCtx>,
    dash: &Option<dashboard::Dashboard>,
) -> anyhow::Result<()> {
    println!(">> {text}");
    if let Some(d) = dash {
        d.push(dashboard::DashEvent::Utterance {
            text: text.to_string(),
            timestamp: chrono::Local::now().to_rfc3339(),
        });
    }

    // File creation: deterministic, fast, no LLM needed.
    if let Some(ref fm) = file_mgr {
        if let Some((action, filename, content)) = files::parse_file_command(text) {
            let result = match action {
                "write_down" => fm.write_down(&content),
                "save_as" => {
                    let name = filename.unwrap_or_else(|| "untitled.txt".to_string());
                    fm.save_as(&name, &content)
                }
                "append" => {
                    let name = filename.unwrap_or_else(|| "untitled.txt".to_string());
                    fm.append_to(&name, &content)
                }
                _ => anyhow::bail!("unknown file action"),
            };
            let reply = match result {
                Ok(path) => {
                    let msg = format!("Saved to {}.", path.file_name().unwrap_or_default().to_string_lossy());
                    if let Some(d) = dash {
                        d.push(dashboard::DashEvent::File {
                            path: path.display().to_string(),
                            action: action.to_string(),
                        });
                    }
                    msg
                }
                Err(e) => {
                    let msg = format!("Could not save file: {e:#}");
                    if let Some(d) = dash {
                        d.push(dashboard::DashEvent::System { message: msg.clone() });
                    }
                    msg
                }
            };
            println!("<< {reply}");
            if let Err(e) = say_streamed(speaker, &reply, rt, device) {
                tracing::error!("speech failed: {e:#}");
            }
            return Ok(());
        }
    }

    // Almanach tutor bridge: send speech to Almanach chat, stream response via TTS.
    if let Some(ref almanach) = almanach_client {
        if let Some(d) = dash {
            d.push(dashboard::DashEvent::Thinking {
                step: "almanach".to_string(),
                detail: "routing to Almanach tutor".to_string(),
            });
        }
        let mut client = rt.block_on(almanach.lock());
        let speaker_ref = speaker.clone();
        let device_owned = device.map(|d| d.to_string());
        let dash_clone = dash.clone();

        match rt.block_on(client.send_message_stream(text, move |chunk: &str| {
            // Stream each chunk to TTS as it arrives
            if let Some(ref d) = dash_clone {
                d.push(dashboard::DashEvent::Response {
                    text: chunk.to_string(),
                    done: false,
                });
            }
        })) {
            Ok(full_response) => {
                if let Some(d) = dash {
                    d.push(dashboard::DashEvent::Response {
                        text: full_response.clone(),
                        done: true,
                    });
                }
                println!("<< [Almanach] {full_response}");
                if let Err(e) = say_streamed(&speaker_ref, &full_response, rt, device_owned.as_deref()) {
                    tracing::error!("speech failed: {e:#}");
                }
                return Ok(());
            }
            Err(e) => {
                tracing::warn!("Almanach failed: {e:#}");
                // Fall through to brain/Orchestre as fallback
            }
        }
    }

    if let Some(path) = bridge {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("failed to open bridge {}", path.display()))?;
        writeln!(f, "{text}")?;
        println!("<< [bridged to Claude Code]");
        if let Some(d) = dash {
            d.push(dashboard::DashEvent::System {
                message: "Bridged to Claude Code".to_string(),
            });
        }
        return Ok(());
    }

    // Coding mode: a runtime-toggled bridge to a live Claude Code session.
    // While active, utterances are appended to the bridge file and the
    // session replies aloud via `five-daemon speak`. ("switch to coding
    // mode" enters; "back to normal" exits.)
    if let Some(path) = coding {
        let t = text.trim().to_lowercase();
        let t = t.trim_end_matches(['.', '!', '?']);
        let switch = wants_mode_switch(t);
        if !coding_active.get() && switch.as_deref() == Some("coding") {
            coding_active.set(true);
            let reply = "Coding mode on — everything you say goes to Claude Code. Say back to normal to exit.";
            println!("<< {reply}");
            if let Some(d) = dash {
                d.push(dashboard::DashEvent::System {
                    message: "Coding mode on".to_string(),
                });
            }
            if let Err(e) = say_streamed(speaker, reply, rt, device) {
                tracing::error!("speech failed: {e:#}");
            }
            return Ok(());
        }
        if coding_active.get() {
            if switch.as_deref() == Some("")
                || matches!(t, "exit coding mode" | "stop coding" | "leave coding mode")
            {
                coding_active.set(false);
                let reply = "Back to normal.";
                println!("<< {reply}");
                if let Some(d) = dash {
                    d.push(dashboard::DashEvent::System {
                        message: "Coding mode off".to_string(),
                    });
                }
                if let Err(e) = say_streamed(speaker, reply, rt, device) {
                    tracing::error!("speech failed: {e:#}");
                }
                return Ok(());
            }
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .with_context(|| format!("failed to open coding bridge {}", path.display()))?;
            writeln!(f, "{text}")?;
            println!("<< [to Claude Code]");
            if let Some(d) = dash {
                d.push(dashboard::DashEvent::System {
                    message: "Sent to Claude Code".to_string(),
                });
            }
            return Ok(());
        }
    }
    if let Some(brain) = brain {
        // Home Assistant commands: deterministic, fast, no LLM needed.
        if let Some(ref home) = home_client {
            if let Some(cmd) = home::parse_command(text) {
                match rt.block_on(home.execute(&cmd)) {
                    Ok(reply) => {
                        println!("<< {reply}");
                        if let Err(e) = say_streamed(speaker, &reply, rt, device) {
                            tracing::error!("speech failed: {e:#}");
                        }
                        return Ok(());
                    }
                    Err(e) => {
                        tracing::warn!("home command failed: {e:#}");
                        // Fall through to brain — maybe it's a conversation about lights.
                    }
                }
            }
        }
        if let Some(mode) = wants_mode_switch(text) {
            let ok = brain.switch_mode(&mode);
            let reply = if ok {
                if mode.is_empty() {
                    "Back to normal mode.".to_string()
                } else {
                    format!("Switched to {} mode.", mode)
                }
            } else {
                format!("I don't know a '{}' mode.", mode)
            };
            if let Some(d) = dash {
                d.push(dashboard::DashEvent::Mode {
                    mode: if mode.is_empty() { "normal".to_string() } else { mode.clone() },
                });
            }
            println!("<< {reply}");
            if let Err(e) = say_streamed(speaker, &reply, rt, device) {
                tracing::error!("speech failed: {e:#}");
            }
            return Ok(());
        }
        if wants_context_clear(text) {
            brain.clear_history();
            let reply = "Done. Fresh conversation.";
            if let Some(d) = dash {
                d.push(dashboard::DashEvent::System {
                    message: "Context cleared".to_string(),
                });
            }
            println!("<< {reply}");
            if let Err(e) = say_streamed(speaker, reply, rt, device) {
                tracing::error!("speech failed: {e:#}");
            }
            return Ok(());
        }
        if let Some(cmd) = wants_lesson_command(text) {
            let reply = match cmd {
                LessonCmd::Create(topic) => {
                    println!("<< authoring lesson plan for \"{topic}\" (Kimi)...");
                    if let Some(d) = dash {
                        d.push(dashboard::DashEvent::Thinking {
                            step: "lesson".to_string(),
                            detail: format!("authoring plan: {topic}"),
                        });
                    }
                    match rt.block_on(brain.create_lesson_plan(&topic)) {
                        Ok(title) => {
                            if let Some(d) = dash {
                                d.push(dashboard::DashEvent::File {
                                    path: format!("lessonplans/{}.md", title),
                                    action: "lesson plan".to_string(),
                                });
                            }
                            format!("Lesson plan ready: {title}. I've saved it and I'm holding it — say start the lesson when you want to begin.")
                        }
                        Err(e) => {
                            tracing::error!("lesson plan failed: {e:#}");
                            "I couldn't write that lesson plan right now.".to_string()
                        }
                    }
                }
                LessonCmd::List => {
                    let lessons = brain.list_lessons();
                    if lessons.is_empty() {
                        "No lesson plans yet. Say make a lesson plan for, then a topic.".to_string()
                    } else {
                        format!("Your lessons: {}.", natural_list(&lessons))
                    }
                }
                LessonCmd::Start(name) => match brain.load_lesson(&name) {
                    Ok(title) => format!("Starting {title}. Let's go."),
                    Err(e) => format!("{e:#}"),
                },
                LessonCmd::End => {
                    if brain.end_lesson() {
                        "Lesson ended. Back to free chat.".to_string()
                    } else {
                        "There's no lesson running.".to_string()
                    }
                }
                LessonCmd::Next => match brain.next_section() {
                    // Cursor moved — hand the model the new section so it
                    // starts teaching it right away.
                    Some(_) => rt
                        .block_on(brain.respond("(The student is ready — teach the next section now.)"))
                        .unwrap_or_else(|e| {
                            tracing::error!("brain failed after section advance: {e:#}");
                            "Moving on.".to_string()
                        }),
                    None => "That was the last section — lesson complete. Well done!".to_string(),
                },
                LessonCmd::Jump(kw) => match brain.goto_section(&kw) {
                    Some(heading) => rt
                        .block_on(brain.respond(&format!("(The student asked about {heading} — teach that section now.)")))
                        .unwrap_or_else(|e| {
                            tracing::error!("brain failed after section jump: {e:#}");
                            format!("Jumping to {heading}.")
                        }),
                    None => match brain.current_lesson() {
                        Some(_) => "I can't find a section about that in this lesson.".to_string(),
                        None => "There's no lesson running.".to_string(),
                    },
                },
            };
            if let Some(d) = dash {
                d.push(dashboard::DashEvent::Response { text: reply.clone(), done: true });
            }
            println!("<< {reply}");
            if let Err(e) = say_streamed(speaker, &reply, rt, device) {
                tracing::error!("speech failed: {e:#}");
            }
            return Ok(());
        }
        let reply = match local_answer(text, home_client, file_mgr.is_some(), true) {
            Some(reply) => {
                tracing::info!("answered locally (deterministic command)");
                if let Some(d) = dash {
                    d.push(dashboard::DashEvent::Thinking {
                        step: "local".to_string(),
                        detail: "deterministic command matched".to_string(),
                    });
                }
                reply
            }
            None => {
                if let Some(d) = dash {
                    d.push(dashboard::DashEvent::Thinking {
                        step: "brain".to_string(),
                        detail: "routing to LLM".to_string(),
                    });
                }
                let t0 = std::time::Instant::now();
                match ask_and_speak(brain, speaker, rt, device, text, ack) {
                    Ok(reply) => {
                        tracing::info!(brain_ms = t0.elapsed().as_millis() as u64, "brain replied (streamed)");
                        println!("<< {reply}");
                        if let Some(d) = dash {
                            d.push(dashboard::DashEvent::Response {
                                text: reply.clone(),
                                done: true,
                            });
                        }
                        // Pre-compute the likely follow-up while the user listens.
                        if let Some(s) = spec {
                            s.spawn();
                        }
                        return Ok(());
                    }
                    Err(e) => {
                        tracing::error!("brain failed: {e:#}");
                        if let Some(d) = dash {
                            d.push(dashboard::DashEvent::System {
                                message: format!("Brain error: {e:#}"),
                            });
                        }
                        return Ok(());
                    }
                }
            }
        };
        if let Some(d) = dash {
            d.push(dashboard::DashEvent::Response {
                text: reply.clone(),
                done: true,
            });
        }
        println!("<< {reply}");
        if let Err(e) = say_streamed(speaker, &reply, rt, device) {
            tracing::error!("speech failed: {e:#}");
        }
        return Ok(());
    }
    let client = client.as_ref().expect("client when no bridge and no brain");
    match rt.block_on(client.send_command(text)) {
        Ok(result) => {
            println!(
                "<< [{}] {}",
                result.status,
                result.response.as_deref().unwrap_or("(no reply)")
            );
            if let Some(reply) = result.response {
                if let Err(e) = rt.block_on(say_with_caption(speaker, &reply, captions_enabled, device)) {
                    tracing::error!("speech failed: {e:#}");
                }
            }
        }
        Err(e) => tracing::error!("orchestrator dispatch failed: {e:#}"),
    }
    Ok(())
}

/// Full assistant loop. With `wakeword.enabled` (rustpotter): listens for
/// the wake word on the live stream, then records a command. With it
/// disabled (text trigger): every speech utterance is transcribed and a
/// whole-word "five" in the text fires the command — whisper is a far more
/// reliable "five" detector than the .rpw templates. Either way the command
/// is dispatched and the reply spoken. A pre-trigger ring buffer keeps the
/// ~2s before detection fires — rustpotter only finalizes a detection once
/// its match window expires (~0.5s after the wakeword peak), and by then the
/// user may already be mid-command, so the ring keeps the first syllables.
fn listen_loop(config: AppConfig, bridge: Option<PathBuf>) -> anyhow::Result<()> {
    use std::collections::VecDeque;

    // Single-threaded runtime for the async pieces (HTTP, TTS synthesis).
    let rt = std::sync::Arc::new(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("failed to build listen runtime")?,
    );

    let transcriber = transcribe::Transcriber::load(&config.transcription)?;
    let speaker = std::sync::Arc::new(rt.block_on(voice::Speaker::load(&config.voice))?);
    // Routing precedence: bridge file > brain > Orchestre. Bridge mode skips
    // both — commands go to a file for a local Claude Code session, which
    // speaks its own replies.
    let brain = if bridge.is_none() && config.brain.enabled {
        Some(std::sync::Arc::new(brain::Brain::new(&config.brain, &config.search)?))
    } else {
        None
    };
    // Coding mode: same bridge-file mechanism, toggled at runtime by voice
    // ("switch to coding mode" / "back to normal").
    let coding: Option<PathBuf> = if bridge.is_none() && config.coding.enabled {
        Some(config.coding.bridge_file.clone())
    } else {
        None
    };
    let coding_active = std::cell::Cell::new(false);
    let home_client = if !config.home.url.is_empty() && !config.home.token.is_empty() {
        Some(std::sync::Arc::new(home::HomeClient::new(&config.home)?))
    } else {
        None
    };
    let file_mgr = if config.files.enabled {
        Some(std::sync::Arc::new(files::FileManager::new(&config.files)?))
    } else {
        None
    };
    let dash = if config.dashboard.enabled {
        let d = dashboard::Dashboard::new(config.dashboard.port);
        d.spawn();
        Some(d)
    } else {
        None
    };
    let almanach_client = if config.almanach.enabled {
        let mut client = almanach::AlmanachClient::new(&config.almanach)?;
        if config.almanach.auto_create_conversation {
            rt.block_on(client.create_conversation(&config.almanach.conversation_title))?;
        }
        Some(std::sync::Arc::new(tokio::sync::Mutex::new(client)))
    } else {
        None
    };
    let client = if bridge.is_none() && brain.is_none() {
        Some(openclaw::OrchestreClient::new(&config.openclaw)?)
    } else {
        None
    };
    let capture = audio::AudioCapture::start(&config.audio)?;
    let out_device = config.audio.output_device.as_deref();
    // Speculative follow-up context (only when the brain is active).
    let spec = brain.as_ref().map(|b| SpecCtx {
        brain: b.clone(),
        speaker: speaker.clone(),
        rt: rt.clone(),
        store: Default::default(),
        gen: Default::default(),
        device: out_device.map(str::to_string),
    });
    // Pre-synthesized ack played the instant a command is recognized, while
    // the brain thinks — fills the 3-6s of dead air the LLM needs. Kept
    // short so it never outlasts even a fast reply.
    let ack = if brain.is_some() {
        rt.block_on(speaker.synthesize("One moment.")).ok()
    } else {
        None
    };
    let text_trigger = !config.wakeword.enabled;
    let mut detector = if text_trigger {
        None
    } else {
        Some(wakeword::build_detector(&config.wakeword)?)
    };

    let rate = config.audio.target_rate as usize;
    let frame = detector
        .as_ref()
        .map(|d| d.get_samples_per_frame())
        .unwrap_or(rate / 10);
    let command_len = rate * config.transcription.command_duration_sec as usize;
    let ring_cap = rate * 2; // 2s of pre-trigger audio
    let mut ring: VecDeque<f32> = VecDeque::with_capacity(ring_cap);
    // Incoming chunks aren't aligned to rustpotter's frame size; buffer the
    // remainder between chunks.
    let mut pending: Vec<f32> = Vec::with_capacity(frame * 2);
    let mut command: Option<Vec<f32>> = None;
    // End-of-speech endpointing: finish the command after 1s of trailing
    // silence (min 1s of speech) instead of always waiting the full
    // command_duration_sec — a fixed 10s window made every reply feel laggy.
    let min_command = rate; // 1s
    let silence_limit = rate; // 1s
    let mut noise = NoiseFloor::new();
    let mut trailing_silence = 0usize;
    let mut static_diag_counter = 0u32;
    // Text-trigger mode: utterance accumulator (endpointed the same way as
    // commands — fire after 1s of trailing silence, min 0.5s of speech).
    let mut utterance: Vec<f32> = Vec::new();
    let mut speech_total = 0usize;
    // Conversation mode: after a "five" trigger fires, the mic stays hot for
    // 5 minutes — follow-up utterances go straight to dispatch without the
    // wake word. Every accepted utterance slides the window, so an active
    // conversation never times out; "never mind" / "that's all" / "goodbye"
    // ends it early. Expiry is checked lazily per utterance, so no timer task
    // is needed.
    let mut hot_until: Option<std::time::Instant> = None;
    const HOT_WINDOW: std::time::Duration = std::time::Duration::from_secs(300);
    // FIVE_DEBUG_SCORES=1 logs every partial detection above a noise floor —
    // for diagnosing "wake word never fires" on the live mic.
    let debug_scores = std::env::var("FIVE_DEBUG_SCORES").is_ok();
    // Normalize detector input — the templates are level-normalized, so the
    // live stream must be too (mic volume varies hugely between utterances).
    let mut agc = wakeword::Agc::new();

    if text_trigger {
        tracing::info!("always listening — transcribing speech, trigger word: five (Ctrl-C to quit)");
    } else {
        tracing::info!(
            threshold = config.wakeword.threshold,
            min_scores = config.wakeword.min_scores,
            "listening for wake word (Ctrl-C to quit)"
        );
    }

    while let Some(chunk) = capture.recv() {
        // --- Collecting a command: accumulate until command_len, then handle.
        if let Some(buf) = &mut command {
            let rms = (chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len().max(1) as f32).sqrt();
            if noise.is_silence(rms) {
                trailing_silence += chunk.len();
            } else {
                trailing_silence = 0;
            }
            noise.update(rms);
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
                    } else if spec.as_ref().is_some_and(|s| s.try_play(&text)) {
                        // Answered from the speculative cache.
                    } else if let Err(e) = dispatch_command(
                        &text,
                        &bridge,
                        &coding,
                        &coding_active,
                        &brain,
                        &home_client,
                        &file_mgr,
                        &almanach_client,
                        &client,
                        &speaker,
                        &rt,
                        config.captions.enabled,
                        out_device,
                        &ack,
                        spec.as_ref(),
                        &dash,
                    ) {
                        tracing::error!("dispatch failed: {e:#}");
                    }
                }
                Err(e) => tracing::error!("transcription failed: {e:#}"),
            }
            // Drop audio that piled up during transcription/playback —
            // it contains Five's own voice and must not retrigger detection.
            while capture.receiver().try_recv().is_ok() {}
            pending.clear();
            ring.clear();
            if let Some(det) = &mut detector {
                det.reset();
            }
            agc = wakeword::Agc::new();
            tracing::info!("listening for wake word");
            continue;
        }

        // --- Text-trigger listening: endpoint each utterance, transcribe it,
        // fire when the text contains the word "five".
        if text_trigger {
            let rms = (chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len().max(1) as f32).sqrt();
            // Tuning telemetry: every ~5s, log background floor vs current
            // level so endpointing thresholds can be set from real data.
            static_diag_counter += 1;
            if static_diag_counter >= 50 {
                static_diag_counter = 0;
                tracing::info!(
                    floor = format!("{:.4}", noise.0),
                    rms = format!("{:.4}", rms),
                    speech_at = format!("{:.4}", (noise.0 * 4.0).clamp(0.008, 0.2)),
                    silence_at = format!("{:.4}", (noise.0 * 2.0).clamp(0.004, 0.1)),
                    "endpointing levels"
                );
            }
            if noise.is_speech(rms) {
                speech_total += chunk.len();
                trailing_silence = 0;
            } else if noise.is_silence(rms) {
                trailing_silence += chunk.len();
            }
            // Between the two thresholds (hysteresis band): count as neither —
            // background noise neither extends speech nor ends it.
            noise.update(rms);
            utterance.extend_from_slice(&chunk);
            let endpointed = speech_total >= rate / 2 && trailing_silence >= silence_limit;
            if utterance.len() < command_len && !endpointed {
                continue;
            }
            let audio = std::mem::take(&mut utterance);
            speech_total = 0;
            trailing_silence = 0;
            let t0 = std::time::Instant::now();
            let text = match transcriber.transcribe_to_string(&audio) {
                Ok(t) => t,
                Err(e) => {
                    tracing::error!("transcription failed: {e:#}");
                    continue;
                }
            };
            tracing::info!(
                stt_ms = t0.elapsed().as_millis() as u64,
                audio_ms = audio.len() * 1000 / rate,
                "utterance transcribed"
            );
            if is_non_speech(&text) {
                continue;
            }
            let hot = hot_until.is_some_and(|t| t > std::time::Instant::now());
            match extract_command(&text) {
                Some(cmd) if !cmd.is_empty() => {
                    if !spec.as_ref().is_some_and(|s| s.try_play(&cmd)) {
                        if let Err(e) = dispatch_command(
                            &cmd,
                            &bridge,
                            &coding,
                            &coding_active,
                            &brain,
                            &home_client,
                            &file_mgr,
                            &almanach_client,
                            &client,
                            &speaker,
                            &rt,
                            config.captions.enabled,
                            out_device,
                            &ack,
                            spec.as_ref(),
                            &dash,
                        ) {
                            tracing::error!("dispatch failed: {e:#}");
                        }
                    }
                    hot_until = Some(std::time::Instant::now() + HOT_WINDOW);
                    // Drop Five's own reply from the mic queue so it isn't
                    // transcribed back as a new utterance.
                    while capture.receiver().try_recv().is_ok() {}
                }
                Some(_) => {
                    // Bare "five" with nothing after: record a fresh command.
                    tracing::info!("wake word heard — recording command");
                    command = Some(Vec::with_capacity(command_len));
                    while capture.receiver().try_recv().is_ok() {}
                }
                None if hot => {
                    let cmd = text.trim();
                    if wants_conversation_end(cmd) {
                        hot_until = None;
                        tracing::info!("conversation ended by user");
                        if let Err(e) = say_streamed(&speaker, "Okay, going quiet.", &rt, out_device) {
                            tracing::error!("speech failed: {e:#}");
                        }
                    } else {
                        println!(">> (hot) {cmd}");
                        if !spec.as_ref().is_some_and(|s| s.try_play(cmd)) {
                            if let Err(e) = dispatch_command(
                                cmd,
                                &bridge,
                                &coding,
                                &coding_active,
                                &brain,
                                &home_client,
                                &file_mgr,
                                &almanach_client,
                                &client,
                                &speaker,
                                &rt,
                                config.captions.enabled,
                                out_device,
                                &ack,
                                spec.as_ref(),
                                &dash,
                            ) {
                                tracing::error!("dispatch failed: {e:#}");
                            }
                        }
                        hot_until = Some(std::time::Instant::now() + HOT_WINDOW);
                    }
                    while capture.receiver().try_recv().is_ok() {}
                }
                None => println!(".. (ignored: {})", text.trim()),
            }
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
            let mut f: Vec<f32> = pending.drain(..frame).collect();
            agc.process(&mut f);
            let det = detector.as_mut().expect("detector in rustpotter mode");
            let hit = det.process_samples(f).is_some();
            if debug_scores {
                if let Some(p) = det.get_partial_detection() {
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
        Some(Command::Devices) => {
            for name in voice::output_devices() {
                println!("{name}");
            }
        }
        Some(Command::Speak { text }) => {
            let config = load_config(&cli.config)?;
            init_tracing(&config.logging.level);
            let speaker = voice::Speaker::load(&config.voice).await?;
            say_with_caption(
                &speaker,
                &text,
                config.captions.enabled,
                config.audio.output_device.as_deref(),
            )
            .await?;
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

#[cfg(test)]
mod tests {
    use super::wants_mode_switch;

    #[test]
    fn mode_switch_parsing() {
        assert_eq!(wants_mode_switch("switch to coding mode").as_deref(), Some("coding"));
        assert_eq!(wants_mode_switch("coding mode").as_deref(), Some("coding"));
        assert_eq!(wants_mode_switch("code mode").as_deref(), Some("coding"));
        assert_eq!(wants_mode_switch("switch to dm mode").as_deref(), Some("dm"));
        assert_eq!(wants_mode_switch("back to normal").as_deref(), Some(""));
        assert_eq!(wants_mode_switch("what time is it"), None);
    }
}
