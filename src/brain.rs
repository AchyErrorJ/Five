//! Command routing: easy questions go to the local 4B model (LM Studio,
//! OpenAI-compatible), coding/hard tasks go to the Kimi coding API.
//!
//! The local model runs a persona ("tutor" or "orchestrator") with two tool
//! conventions it invokes by emitting whole lines in its reply:
//!   NOTE: <fact>     — appended to the memory notebook (re-injected every
//!                      turn; this is Five's long-term memory)
//!   ASK_BIG: <q>     — escalates a hard question to Kimi; Kimi's answer is
//!                      what's spoken
//! Tool lines are stripped from the spoken reply. Conversation history (last
//! few exchanges) rides along so the persona has continuity.

use anyhow::Context;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::config::BrainConfig;

/// OpenAI-compatible chat message (both LM Studio and Moonshot speak this).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,
    max_tokens: u32,
    temperature: f32,
    /// LM Studio: "none" turns off gemma/qwen reasoning so the reply lands in
    /// `content` instead of being burned as invisible `reasoning_content`
    /// tokens. Only sent on the local route (Moonshot rejects/ignores it).
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<&'static str>,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: Message,
}

/// Tool-convention docs shared by both personas.
const TOOLS: &str = "You have two tools, invoked as whole lines in your reply:\n\
    - A line starting with \"NOTE: \" writes one fact to your memory notebook \
    (progress, weak spots, plans). Whenever the user tells you their name, \
    goal, or what they struggle with, ALWAYS write a NOTE.\n\
    - A line starting with \"ASK_BIG: \" sends a hard question to a much \
    bigger model; its answer is spoken to the user. When a question is beyond \
    your own knowledge, prefer ASK_BIG over deflecting or guessing.\n\
    These lines are never spoken aloud.\n\
    Example:\n\
    User: I'm Sam and I keep mixing up pointers.\n\
    You: Pointers trip a lot of people up, Sam — let's untangle them.\n\
    NOTE: Sam struggles with pointers";

const ORCHESTRATOR_SYSTEM: &str = "You are Five, a voice orchestrator on a handheld PC. \
    Keep answers SHORT — they are spoken aloud, one or two sentences max, \
    plain speech, no markdown, no lists. Answer simple things yourself; \
    orchestrate the rest with your tools.";

const KIMI_SYSTEM: &str = "You are the escalation brain behind Five, a voice assistant. \
    The user asked something the small local model couldn't handle. Your answer \
    is SPOKEN ALOUD: plain speech, no markdown, no code blocks, four sentences \
    max. If it's a coding task, summarize what to do rather than reciting code.";

/// Where a command gets routed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Local,
    Kimi,
}

/// Heuristic split: coding/work keywords or long requests → Kimi.
pub fn classify(text: &str) -> Route {
    const HARD: &[&str] = &[
        "code", "coding", "bug", "fix", "implement", "refactor", "function", "script",
        "compile", "error", "rust", "python", "javascript", "typescript", "commit",
        "git ", "github", "pull request", "merge", "deploy", "build", "test",
        "write a", "create a file", "edit", "debug", "stack trace", "api key",
    ];
    let lower = text.to_lowercase();
    if text.len() > 280 {
        return Route::Kimi;
    }
    for kw in HARD {
        if lower.contains(kw) {
            return Route::Kimi;
        }
    }
    Route::Local
}

/// A parsed model reply: the part to speak, plus tool invocations.
struct ParsedReply {
    speech: String,
    notes: Vec<String>,
    escalation: Option<String>,
}

/// Split a reply into spoken text and NOTE:/ASK_BIG: tool lines.
fn parse_tools(raw: &str) -> ParsedReply {
    let mut speech = Vec::new();
    let mut notes = Vec::new();
    let mut escalation = None;
    for line in raw.lines() {
        let t = line.trim();
        if let Some(rest) = strip_prefix_ci(t, "note:") {
            if !rest.is_empty() {
                notes.push(rest.to_string());
            }
        } else if let Some(rest) = strip_prefix_ci(t, "ask_big:") {
            if !rest.is_empty() {
                escalation = Some(rest.to_string());
            }
        } else if !t.is_empty() {
            speech.push(t);
        }
    }
    // Spoken text must be plain: 4B models leak markdown emphasis even when
    // told not to, and "asterisk is asterisk" sounds awful.
    let speech = speech
        .join(" ")
        .chars()
        .filter(|c| !matches!(c, '*' | '#' | '`' | '_'))
        .collect();
    ParsedReply { speech, notes, escalation }
}

fn strip_prefix_ci<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    if line.len() >= prefix.len() && line[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(line[prefix.len()..].trim())
    } else {
        None
    }
}

pub struct Brain {
    http: reqwest::Client,
    cfg: BrainConfig,
    kimi_key: Option<String>,
    /// Rolling conversation history (user/assistant pairs), capped.
    history: std::sync::Mutex<Vec<Message>>,
}

impl Brain {
    pub fn new(cfg: &crate::config::BrainConfig) -> anyhow::Result<Self> {
        let kimi_key = match &cfg.kimi_key_file {
            Some(path) => {
                let key = std::fs::read_to_string(path)
                    .with_context(|| format!("failed to read Kimi key from {}", path.display()))?
                    .trim()
                    .to_string();
                if key.is_empty() || key == "PASTE_KIMI_KEY_HERE" {
                    tracing::warn!("Kimi key file is a placeholder — Kimi route will fail");
                    None
                } else {
                    Some(key)
                }
            }
            None => None,
        };
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(cfg.timeout_sec))
                .build()?,
            cfg: cfg.clone(),
            kimi_key,
            history: std::sync::Mutex::new(Vec::new()),
        })
    }

    /// The persona system prompt, with the memory notebook folded in.
    fn system_prompt(&self) -> String {
        let persona = match self.cfg.persona.as_str() {
            "tutor" => {
                let subject = self.cfg.subject.as_deref().unwrap_or("the student's current subject");
                format!(
                    "You are Five, a voice tutor on a handheld PC, teaching {subject}. \
                    Keep every reply SHORT — spoken aloud, one to three sentences, plain \
                    speech, no markdown, no lists. Teach step by step: explain briefly, \
                    then check understanding with one question. Remember where the \
                    student is and pick up there next session.\n{TOOLS}"
                )
            }
            _ => format!("{ORCHESTRATOR_SYSTEM}\n{TOOLS}"),
        };
        match &self.cfg.notebook_path {
            Some(path) => {
                let contents = std::fs::read_to_string(path).unwrap_or_default();
                if contents.trim().is_empty() {
                    format!("{persona}\n\nYour memory notebook is empty so far.")
                } else {
                    format!("{persona}\n\nYour memory notebook:\n{}", contents.trim())
                }
            }
            None => persona,
        }
    }

    fn append_notes(&self, notes: &[String]) {
        let Some(path) = &self.cfg.notebook_path else { return };
        use std::io::Write;
        match std::fs::OpenOptions::new().create(true).append(true).open(path) {
            Ok(mut f) => {
                for n in notes {
                    let _ = writeln!(f, "- {n}");
                    info!(note = %n, "notebook updated");
                }
            }
            Err(e) => tracing::warn!("failed to append to notebook: {e:#}"),
        }
    }

    fn push_history(&self, user: &str, assistant: &str) {
        const MAX_MESSAGES: usize = 16; // ~8 exchanges
        let mut h = self.history.lock().unwrap();
        h.push(Message { role: "user".into(), content: user.into() });
        h.push(Message { role: "assistant".into(), content: assistant.into() });
        if h.len() > MAX_MESSAGES {
            let drop = h.len() - MAX_MESSAGES;
            h.drain(..drop);
        }
    }

    async fn chat(
        &self,
        url: &str,
        model: &str,
        messages: Vec<Message>,
        bearer: Option<&str>,
        disable_reasoning: bool,
    ) -> anyhow::Result<String> {
        let req = ChatRequest {
            model: model.to_string(),
            messages,
            max_tokens: self.cfg.max_tokens,
            temperature: 0.7,
            reasoning_effort: disable_reasoning.then_some("none"),
        };
        let mut rb = self.http.post(format!("{url}/chat/completions")).json(&req);
        if let Some(key) = bearer {
            rb = rb.bearer_auth(key);
        }
        let resp = rb.send().await.context("LLM request failed")?;
        let status = resp.status();
        let body = resp.text().await.context("failed to read LLM response")?;
        if !status.is_success() {
            anyhow::bail!("LLM returned {status}: {}", &body[..body.len().min(300)]);
        }
        let parsed: ChatResponse =
            serde_json::from_str(&body).with_context(|| format!("bad LLM JSON: {}", &body[..body.len().min(300)]))?;
        parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content.trim().to_string())
            .filter(|s| !s.is_empty())
            .context("LLM returned no content")
    }

    /// Ask Kimi (tier 2) and return its spoken-form answer.
    async fn ask_big(&self, question: &str) -> anyhow::Result<String> {
        let key = self
            .kimi_key
            .as_deref()
            .context("escalation requested but no Kimi API key (kimi_key_file)")?;
        info!(model = %self.cfg.kimi_model, "escalating to Kimi");
        debug!("escalation question: {question}");
        let messages = vec![
            Message { role: "system".into(), content: KIMI_SYSTEM.into() },
            Message { role: "user".into(), content: question.into() },
        ];
        self.chat(&self.cfg.kimi_url, &self.cfg.kimi_model, messages, Some(key), false)
            .await
    }

    /// Route a command and return the reply text to speak.
    pub async fn respond(&self, text: &str) -> anyhow::Result<String> {
        if classify(text) == Route::Kimi {
            let reply = self.ask_big(text).await?;
            self.push_history(text, &reply);
            return Ok(reply);
        }

        info!(model = %self.cfg.local_model, persona = %self.cfg.persona, "routing to local 4B");
        let mut messages = vec![Message { role: "system".into(), content: self.system_prompt() }];
        messages.extend(self.history.lock().unwrap().iter().cloned());
        messages.push(Message { role: "user".into(), content: text.into() });

        let raw = self
            .chat(&self.cfg.local_url, &self.cfg.local_model, messages, None, true)
            .await?;
        let parsed = parse_tools(&raw);
        self.append_notes(&parsed.notes);

        let reply = if let Some(question) = parsed.escalation {
            // Local model punted: any lead-in it gave ("good question, let me
            // check") is spoken, then the big model's answer.
            match self.ask_big(&question).await {
                Ok(big) => [parsed.speech, big].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" "),
                Err(e) => {
                    tracing::error!("escalation failed: {e:#}");
                    if parsed.speech.is_empty() {
                        "That's beyond me right now, and my bigger brain isn't answering.".to_string()
                    } else {
                        parsed.speech
                    }
                }
            }
        } else {
            parsed.speech
        };

        let reply = if reply.is_empty() { "I'm not sure what to say to that.".to_string() } else { reply };
        self.push_history(text, &reply);
        Ok(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing() {
        assert_eq!(classify("what time is it?"), Route::Local);
        assert_eq!(classify("tell me a joke"), Route::Local);
        assert_eq!(classify("fix the bug in main.rs"), Route::Kimi);
        assert_eq!(classify("write a script that renames files"), Route::Kimi);
    }

    #[test]
    fn tool_parsing() {
        let p = parse_tools("Good question!\nNOTE: struggles with fractions\nASK_BIG: why is the sky blue\n");
        assert_eq!(p.speech, "Good question!");
        assert_eq!(p.notes, vec!["struggles with fractions"]);
        assert_eq!(p.escalation.as_deref(), Some("why is the sky blue"));

        let p = parse_tools("Just a plain answer, no tools.");
        assert_eq!(p.speech, "Just a plain answer, no tools.");
        assert!(p.notes.is_empty());
        assert!(p.escalation.is_none());

        // case-insensitive, stray whitespace
        let p = parse_tools("  ask_big:   test question  ");
        assert_eq!(p.escalation.as_deref(), Some("test question"));
    }
}
