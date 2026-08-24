//! Command routing: easy questions go to the local 4B model (LM Studio,
//! OpenAI-compatible), coding/hard tasks go to the Kimi coding API.
//!
//! The split is a local heuristic — the 4B never sees hard tasks. Anything
//! that smells like code/work (keywords, or just very long) goes to Kimi;
//! questions, chit-chat, time/weather-style asks stay local.

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

const LOCAL_SYSTEM: &str = "You are Five, a voice assistant on a handheld PC. \
    Keep answers SHORT — they are spoken aloud, one or two sentences max. \
    No markdown, no lists, plain speech only.";

const KIMI_SYSTEM: &str = "You are Five, a voice assistant delegating a coding task. \
    The answer is SPOKEN ALOUD: reply with a short spoken summary of what you did \
    or would do, one to three sentences, plain speech, no code blocks.";

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

pub struct Brain {
    http: reqwest::Client,
    cfg: BrainConfig,
    kimi_key: Option<String>,
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
        })
    }

    async fn chat(
        &self,
        url: &str,
        model: &str,
        system: &str,
        user: &str,
        bearer: Option<&str>,
        disable_reasoning: bool,
    ) -> anyhow::Result<String> {
        let req = ChatRequest {
            model: model.to_string(),
            messages: vec![
                Message { role: "system".into(), content: system.into() },
                Message { role: "user".into(), content: user.into() },
            ],
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

    /// Route a command and return the reply text to speak.
    pub async fn respond(&self, text: &str) -> anyhow::Result<String> {
        match classify(text) {
            Route::Local => {
                info!(model = %self.cfg.local_model, "routing to local 4B");
                self.chat(&self.cfg.local_url, &self.cfg.local_model, LOCAL_SYSTEM, text, None, true)
                    .await
            }
            Route::Kimi => {
                let key = self
                    .kimi_key
                    .as_deref()
                    .context("Kimi route chosen but no API key (kimi_key_file)")?;
                info!(model = %self.cfg.kimi_model, "routing to Kimi");
                debug!("kimi task: {text}");
                self.chat(&self.cfg.kimi_url, &self.cfg.kimi_model, KIMI_SYSTEM, text, Some(key), false)
                    .await
            }
        }
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
}
