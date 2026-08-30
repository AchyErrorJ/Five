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
use tracing::{debug, info, warn};

use crate::config::BrainConfig;

/// OpenAI-compatible chat message (both LM Studio and Moonshot speak this).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Serialize, Clone)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,
    max_tokens: u32,
    temperature: f32,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    stream: bool,
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

/// One SSE chunk from a streaming completion: choices[0].delta.content.
#[derive(Deserialize)]
struct StreamChunk {
    choices: Vec<StreamChoice>,
}

#[derive(Deserialize)]
struct StreamChoice {
    delta: StreamDelta,
}

#[derive(Deserialize)]
struct StreamDelta {
    #[serde(default)]
    content: Option<String>,
}

/// Tool-convention docs shared by both personas.
const TOOLS: &str = "You have three tools, invoked as whole lines in your reply:\n\
    - A line starting with \"NOTE: \" writes one fact to your memory notebook \
    (progress, weak spots, plans). Whenever the user tells you their name, \
    goal, or what they struggle with, ALWAYS write a NOTE.\n\
    - A line starting with \"ASK_BIG: \" sends a hard question to a much \
    bigger model; its answer is spoken to the user. When a question is beyond \
    your own knowledge, prefer ASK_BIG over deflecting or guessing.\n\
    - A line starting with \"SEARCH: \" searches the web and returns a \
    summary. Use this for facts you don't know, current events, or when \
    the user asks about something specific on an allowed site.\n\
    - A line starting with \"PLAN: \" asks the big model to write a lesson \
    plan on the given topic; the plan is saved and you then teach it step \
    by step. Use it when the user wants to learn a topic properly.\n\
    These lines are never spoken aloud.\n\
    Example:\n\
    User: I'm Sam and I keep mixing up pointers.\n\
    You: Pointers trip a lot of people up, Sam — let's untangle them.\n\
    NOTE: Sam struggles with pointers";

const ORCHESTRATOR_SYSTEM: &str = "You are Five, a voice orchestrator on a handheld PC. \
    Keep answers SHORT — they are spoken aloud, one or two sentences max, \
    plain speech, no markdown, no lists. Answer simple things yourself; \
    orchestrate the rest with your tools.";


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
    search: Option<String>,
    plan: Option<String>,
}

/// Split a reply into spoken text and NOTE:/ASK_BIG:/SEARCH:/PLAN: tool lines.
fn parse_tools(raw: &str) -> ParsedReply {
    let mut speech = Vec::new();
    let mut notes = Vec::new();
    let mut escalation = None;
    let mut search = None;
    let mut plan = None;
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
        } else if let Some(rest) = strip_prefix_ci(t, "search:") {
            if !rest.is_empty() {
                search = Some(rest.to_string());
            }
        } else if let Some(rest) = strip_prefix_ci(t, "plan:") {
            if !rest.is_empty() {
                plan = Some(rest.to_string());
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
    ParsedReply { speech, notes, escalation, search, plan }
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
    /// Optional web search tool.
    searcher: Option<crate::search::Searcher>,
    /// Active mode name (from `cfg.modes`). None = use base config.
    active_mode: std::sync::Mutex<Option<String>>,
    /// Lesson plan currently being taught: title + markdown body. Set when
    /// a plan is authored or loaded; folded into the system prompt so the
    /// local model administers it section by section.
    active_lesson: std::sync::Mutex<Option<ActiveLesson>>,
}

struct ActiveLesson {
    title: String,
    body: String,
}

impl Brain {
    pub fn new(cfg: &crate::config::BrainConfig, search_cfg: &crate::config::SearchConfig) -> anyhow::Result<Self> {
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
        let searcher = if search_cfg.enabled {
            let s = crate::search::Searcher::new(crate::search::SearchConfig {
                allowed_sites: search_cfg.allowed_sites.clone(),
                max_results: search_cfg.max_results,
                timeout_sec: search_cfg.timeout_sec,
            })?;
            Some(s)
        } else {
            None
        };
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(cfg.timeout_sec))
                .build()?,
            cfg: cfg.clone(),
            kimi_key,
            history: std::sync::Mutex::new(Vec::new()),
            searcher,
            active_mode: std::sync::Mutex::new(None),
            active_lesson: std::sync::Mutex::new(None),
        })
    }

    /// Read the soul file, if configured and non-empty. Errors fall back to
    /// the built-in persona with a warning rather than failing the turn.
    fn soul(&self) -> Option<String> {
        let path = self.cfg.soul_path.as_ref()?;
        match std::fs::read_to_string(path) {
            Ok(contents) if !contents.trim().is_empty() => Some(contents.trim().to_string()),
            Ok(_) => None,
            Err(e) => {
                tracing::warn!("soul file {} unreadable ({e:#}) — using built-in persona", path.display());
                None
            }
        }
    }

    /// Effective context window: base `context_window` or active mode's override.
    fn effective_context_window(&self) -> u32 {
        let mode = self.active_mode.lock().unwrap();
        match mode.as_deref() {
            Some(name) => self.cfg.modes.get(name).map(|m| m.context_window).unwrap_or(self.cfg.context_window),
            None => self.cfg.context_window,
        }
    }

    /// Effective max tokens: base or active mode's override.
    fn effective_max_tokens(&self) -> u32 {
        let mode = self.active_mode.lock().unwrap();
        match mode.as_deref() {
            Some(name) => self.cfg.modes.get(name).map(|m| m.max_tokens).unwrap_or(self.cfg.max_tokens),
            None => self.cfg.max_tokens,
        }
    }

    /// Effective persona: base or active mode's override.
    fn effective_persona(&self) -> String {
        let mode = self.active_mode.lock().unwrap();
        match mode.as_deref() {
            Some(name) => self.cfg.modes.get(name).map(|m| m.persona.clone()).unwrap_or_else(|| self.cfg.persona.clone()),
            None => self.cfg.persona.clone(),
        }
    }

    /// Effective local model: base or active mode's override.
    fn effective_local_model(&self) -> String {
        let mode = self.active_mode.lock().unwrap();
        match mode.as_deref() {
            Some(name) => self.cfg.modes.get(name).map(|m| m.local_model.clone()).unwrap_or_else(|| self.cfg.local_model.clone()),
            None => self.cfg.local_model.clone(),
        }
    }

    /// Switch to a named mode (from `cfg.modes`). Clears history so the new
    /// context window budget isn't polluted by old conversation. Returns
    /// true if the mode exists.
    pub fn switch_mode(&self, name: &str) -> bool {
        if name.is_empty() || self.cfg.modes.contains_key(name) {
            let mut m = self.active_mode.lock().unwrap();
            *m = if name.is_empty() { None } else { Some(name.to_string()) };
            self.clear_history();
            true
        } else {
            false
        }
    }

    /// Name of the currently active mode, if any.
    pub fn current_mode(&self) -> Option<String> {
        self.active_mode.lock().unwrap().clone()
    }

    /// Author a lesson plan on `topic` with the big model (Kimi), save it to
    /// the lesson directory as markdown, and make it the active lesson.
    /// Returns the plan's title.
    pub async fn create_lesson_plan(&self, topic: &str) -> anyhow::Result<String> {
        let key = self
            .kimi_key
            .as_deref()
            .context("lesson plans need the big model — no Kimi API key (kimi_key_file)")?;
        info!(topic, model = %self.cfg.kimi_model, "authoring lesson plan");
        let messages = vec![
            Message {
                role: "system".into(),
                content: "You are a curriculum author. Write lesson plans that a voice tutor \
                          will teach aloud in short sessions. Markdown, no tables, no images."
                    .into(),
            },
            Message {
                role: "user".into(),
                content: format!(
                    "Write a lesson plan for teaching \"{topic}\" to a curious student.\n\
                     Format:\n\
                     # <short title>\n\
                     One-line summary of what the student will learn.\n\
                     Then 4 to 6 numbered sections. Each section: a heading, 2-3 sentences of \
                     explanation in plain spoken English, and one \"Check:\" question to verify \
                     understanding. The whole plan should be teachable aloud in about ten minutes."
                ),
            },
        ];
        let body = self
            .chat_budget(&self.cfg.kimi_url, &self.cfg.kimi_model, messages, Some(key), false, 4096)
            .await?;
        let title = body
            .lines()
            .find_map(|l| l.trim().strip_prefix("# "))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| topic.trim().to_string());
        std::fs::create_dir_all(&self.cfg.lesson_dir)?;
        let path = self.cfg.lesson_dir.join(format!("{}.md", slugify(&title)));
        std::fs::write(&path, &body).with_context(|| format!("failed to write {}", path.display()))?;
        info!(path = %path.display(), "lesson plan saved");
        *self.active_lesson.lock().unwrap() = Some(ActiveLesson {
            title: title.clone(),
            body,
        });
        Ok(title)
    }

    /// Lesson titles on disk (markdown files in the lesson directory).
    pub fn list_lessons(&self) -> Vec<String> {
        let mut out: Vec<String> = std::fs::read_dir(&self.cfg.lesson_dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter_map(|e| {
                        let p = e.path();
                        (p.extension().is_some_and(|x| x == "md")).then(|| {
                            p.file_stem().unwrap_or_default().to_string_lossy().replace('-', " ")
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort();
        out
    }

    /// Load a lesson from disk by (fuzzy) name and make it active. An empty
    /// name resumes whichever lesson is already active. Returns its title.
    pub fn load_lesson(&self, name: &str) -> anyhow::Result<String> {
        let name = name.trim();
        if name.is_empty() {
            let lesson = self.active_lesson.lock().unwrap();
            return lesson
                .as_ref()
                .map(|l| l.title.clone())
                .context("no lesson is loaded — say start lesson, then a topic");
        }
        let want = slugify(name);
        let rd = std::fs::read_dir(&self.cfg.lesson_dir)
            .with_context(|| format!("lesson folder {} not found", self.cfg.lesson_dir.display()))?;
        for e in rd.filter_map(|e| e.ok()) {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "md") {
                let stem = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
                if stem.contains(&want) {
                    let body = std::fs::read_to_string(&p)
                        .with_context(|| format!("failed to read {}", p.display()))?;
                    let title = body
                        .lines()
                        .find_map(|l| l.trim().strip_prefix("# "))
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| stem.replace('-', " "));
                    *self.active_lesson.lock().unwrap() = Some(ActiveLesson {
                        title: title.clone(),
                        body,
                    });
                    self.clear_history(); // start the lesson with a clean slate
                    return Ok(title);
                }
            }
        }
        anyhow::bail!("I don't have a lesson called {name}. Say list lessons to see them.")
    }

    /// Title of the lesson currently being taught, if any.
    pub fn current_lesson(&self) -> Option<String> {
        self.active_lesson.lock().unwrap().as_ref().map(|l| l.title.clone())
    }

    /// Stop teaching the active lesson. Returns false if none was running.
    pub fn end_lesson(&self) -> bool {
        self.active_lesson.lock().unwrap().take().is_some()
    }

    /// Max history messages derived from the effective context window.
    /// Reserves ~1K tokens for system prompt + notebook, ~150 per message.
    fn max_messages(&self) -> usize {
        let ctx = self.effective_context_window();
        let available = ctx.saturating_sub(self.effective_max_tokens()).saturating_sub(1024);
        let per_message = 150u32;
        (available / per_message).max(4).min(512) as usize
    }

    /// The persona system prompt, with the memory notebook folded in.
    /// A soul file (brain.soul_path) replaces the built-in persona and is
    /// re-read every turn, so edits are live; the tool convention is always
    /// appended so NOTE:/ASK_BIG: keep working regardless of the soul text.
    fn system_prompt(&self) -> String {
        let persona_key = self.effective_persona();
        let persona = match self.soul() {
            Some(soul) => format!("{soul}\n{TOOLS}"),
            None => match persona_key.as_str() {
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
            },
        };
        let base = match &self.cfg.notebook_path {
            Some(path) => {
                let contents = std::fs::read_to_string(path).unwrap_or_default();
                if contents.trim().is_empty() {
                    format!("{persona}\n\nYour memory notebook is empty so far.")
                } else {
                    format!("{persona}\n\nYour memory notebook:\n{}", contents.trim())
                }
            }
            None => persona,
        };
        // Active lesson plan rides along: the local model administers it
        // section by section. Capped so a long plan can't crowd out history.
        let lesson = self.active_lesson.lock().unwrap();
        match lesson.as_ref() {
            Some(l) => {
                let body: String = l.body.chars().take(6000).collect();
                format!(
                    "{base}\n\nYou are currently teaching this lesson plan: \"{}\". \
                    Walk through it section by section, ONE section at a time. \
                    Explain the section briefly in your own words, then ask its \
                    check question. Wait for the student's answer, say whether it \
                    was right, then move on. Never read the plan verbatim or dump \
                    multiple sections at once.\n\n{body}",
                    l.title,
                    base = base
                )
            }
            None => base,
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

    pub fn push_history(&self, user: &str, assistant: &str) {
        let max_msgs = self.max_messages();
        let mut h = self.history.lock().unwrap();
        h.push(Message { role: "user".into(), content: user.into() });
        h.push(Message { role: "assistant".into(), content: assistant.into() });
        if h.len() > max_msgs {
            let drop = h.len() - max_msgs;
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
        self.chat_budget(url, model, messages, bearer, disable_reasoning, self.effective_max_tokens())
            .await
    }

    /// `chat` with an explicit completion budget — lesson plans need far
    /// more than the short spoken-reply cap.
    async fn chat_budget(
        &self,
        url: &str,
        model: &str,
        messages: Vec<Message>,
        bearer: Option<&str>,
        disable_reasoning: bool,
        max_tokens: u32,
    ) -> anyhow::Result<String> {
        let req = ChatRequest {
            model: model.to_string(),
            messages,
            max_tokens,
            temperature: 0.7,
            stream: false,
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

    /// Ask Kimi (tier 2) with full conversation context so it assumes the
    /// role of Five's brain — persona, notebook, and history included.
    async fn ask_big(&self, question: &str) -> anyhow::Result<String> {
        let key = self
            .kimi_key
            .as_deref()
            .context("escalation requested but no Kimi API key (kimi_key_file)")?;
        info!(model = %self.cfg.kimi_model, "escalating to Kimi with full context");
        debug!("escalation question: {question}");

        let mut messages = vec![
            Message { role: "system".into(), content: self.system_prompt() },
        ];
        messages.extend(self.history.lock().unwrap().iter().cloned());
        messages.push(Message {
            role: "user".into(),
            content: format!(
                "{question}\n\n(Your answer will be spoken aloud as Five. Keep it plain speech, no markdown, four sentences max. If it's a coding task, summarize what to do rather than reciting code.)"
            ),
        });

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

        info!(model = %self.effective_local_model(), persona = %self.effective_persona(), "routing to local 4B");
        let mut messages = vec![Message { role: "system".into(), content: self.system_prompt() }];
        messages.extend(self.history.lock().unwrap().iter().cloned());
        messages.push(Message { role: "user".into(), content: text.into() });

        let raw = self
            .chat(&self.cfg.local_url, &self.effective_local_model(), messages, None, true)
            .await?;
        let parsed = parse_tools(&raw);
        self.append_notes(&parsed.notes);

        // Handle SEARCH tool first — may produce speech directly or feed into model
        let search_reply = if let Some(ref query) = parsed.search {
            if let Some(ref searcher) = self.searcher {
                match searcher.search(query).await {
                    Ok(results) => Some(crate::search::summarize(&results)),
                    Err(e) => {
                        tracing::warn!("search failed: {e:#}");
                        Some("I tried to search but couldn't reach the web.".to_string())
                    }
                }
            } else {
                Some("Search isn't configured right now.".to_string())
            }
        } else {
            None
        };

        let reply = if let Some(topic) = parsed.plan {
            // The local model asked for a lesson plan: author it with the big
            // model, then it's loaded as the active lesson automatically.
            match self.create_lesson_plan(&topic).await {
                Ok(title) => [parsed.speech, format!("Lesson plan ready: {title}. Say start the lesson when you want to begin.")]
                    .into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" "),
                Err(e) => {
                    tracing::error!("lesson plan failed: {e:#}");
                    if parsed.speech.is_empty() {
                        "I couldn't write that lesson plan right now.".to_string()
                    } else {
                        parsed.speech
                    }
                }
            }
        } else if let Some(question) = parsed.escalation {
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
        } else if let Some(search_speech) = search_reply {
            // Search result available: prepend any lead-in from the model
            if parsed.speech.is_empty() {
                search_speech
            } else {
                format!("{} {}", parsed.speech, search_speech)
            }
        } else {
            parsed.speech
        };

        let reply = if reply.is_empty() { "I'm not sure what to say to that.".to_string() } else { reply };
        self.push_history(text, &reply);
        Ok(reply)
    }

    /// Guess the student's most likely follow-up question (one cheap local
    /// call). Used to pre-generate a speculative reply while the mic is hot;
    /// never touches history or the notebook itself.
    pub async fn predict_followup(&self) -> anyhow::Result<Option<String>> {
        let history = self.history.lock().unwrap().clone();
        if history.is_empty() {
            return Ok(None);
        }
        let mut messages = vec![Message { role: "system".into(), content: self.system_prompt() }];
        messages.extend(history);
        messages.push(Message {
            role: "user".into(),
            content: "In one short line, guess the most likely follow-up question the \
                      student asks next. Reply with ONLY that question — no preamble, \
                      no explanation."
                .into(),
        });
        let raw = self
            .chat(&self.cfg.local_url, &self.effective_local_model(), messages, None, true)
            .await?;
        let q = raw.trim().trim_matches('"').lines().next().unwrap_or("").trim().to_string();
        if q.len() < 8 || q.len() > 200 {
            return Ok(None);
        }
        Ok(Some(q))
    }

    /// Clear the rolling conversation history (the "clear context" command).
    /// The notebook is untouched — that's long-term memory, cleared by hand.
    pub fn clear_history(&self) {
        self.history.lock().unwrap().clear();
    }

    /// Streaming variant of `respond`: POSTs the local route with
    /// stream:true and forwards each complete spoken sentence to `out` the
    /// moment it arrives, so TTS can synthesize sentence one while the rest
    /// of the reply is still generating. Tool lines are handled inline
    /// (NOTE appended immediately; ASK_BIG answered after the stream ends,
    /// its answer forwarded as sentences too). Returns the full spoken
    /// reply (for the log line and history).
    /// `record: false` runs it speculatively — no history push, no notebook
    /// writes — so a wrong guess leaves no trace in the conversation.
    pub async fn respond_stream(
        &self,
        text: &str,
        out: tokio::sync::mpsc::Sender<String>,
        record: bool,
    ) -> anyhow::Result<String> {
        if classify(text) == Route::Kimi {
            let reply = self.ask_big(text).await?;
            if record {
                self.push_history(text, &reply);
            }
            let _ = out.send(reply.clone()).await;
            return Ok(reply);
        }

        info!(model = %self.effective_local_model(), persona = %self.effective_persona(), "routing to local 4B (streaming)");
        let mut messages = vec![Message { role: "system".into(), content: self.system_prompt() }];
        messages.extend(self.history.lock().unwrap().iter().cloned());
        messages.push(Message { role: "user".into(), content: text.into() });

        let req = ChatRequest {
            model: self.effective_local_model(),
            messages,
            max_tokens: self.effective_max_tokens(),
            temperature: 0.7,
            stream: true,
            reasoning_effort: Some("none"),
        };
        // LM Studio occasionally aborts a fresh connection (os error 10053)
        // when it's still tearing down an abandoned stream (e.g. a cancelled
        // speculative request). One retry on a fresh connection absorbs it.
        let mut last_err = None;
        let resp = {
            let mut resp = None;
            for attempt in 0..2 {
                if attempt > 0 {
                    warn!("stream request failed; retrying once on a fresh connection");
                    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                }
                match self
                    .http
                    .post(format!("{}/chat/completions", self.cfg.local_url))
                    .json(&req)
                    .send()
                    .await
                {
                    Ok(r) => { resp = Some(r); break; }
                    Err(e) => last_err = Some(e),
                }
            }
            match resp {
                Some(r) => r,
                None => return Err(last_err.unwrap()).context("LLM stream request failed"),
            }
        };
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("LLM returned {status}: {}", &body[..body.len().min(300)]);
        }

        // Consume the SSE stream, splitting complete spoken sentences off as
        // they land. Tool lines are parsed whole (they end with \n).
        use futures_util::StreamExt;
        let mut stream = resp.bytes_stream();
        let mut sse_buf = String::new();   // undecoded SSE bytes
        let mut line_buf = String::new();  // decoded text, no complete line yet
        let mut speech_buf = String::new(); // spoken text, no complete sentence yet
        let mut spoken: Vec<String> = Vec::new();
        let mut escalation: Option<String> = None;
        let mut search: Option<String> = None;
        let mut plan: Option<String> = None;

        while let Some(chunk) = stream.next().await {
            let bytes = chunk.context("LLM stream read failed")?;
            sse_buf.push_str(&String::from_utf8_lossy(&bytes));
            // SSE events are "data: {...}\n" lines; "data: [DONE]" ends it.
            while let Some(nl) = sse_buf.find('\n') {
                let event = sse_buf[..nl].trim_end_matches('\r').to_string();
                sse_buf.drain(..=nl);
                let Some(payload) = event.strip_prefix("data: ") else { continue };
                if payload.trim() == "[DONE]" {
                    break;
                }
                let Ok(chunk) = serde_json::from_str::<StreamChunk>(payload) else { continue };
                let Some(piece) = chunk.choices.into_iter().next().and_then(|c| c.delta.content) else { continue };
                line_buf.push_str(&piece);

                // Whole lines: route tool lines, keep spoken lines.
                while let Some(le) = line_buf.find('\n') {
                    let line = line_buf[..le].trim().to_string();
                    line_buf.drain(..=le);
                    self.handle_stream_line(&line, &mut speech_buf, &mut escalation, &mut search, &mut plan, record);
                }
                // Complete spoken sentences can go straight to TTS.
                self.drain_sentences(&mut speech_buf, &out, &mut spoken, false).await;
            }
        }
        // Flush: last line without a trailing newline, then the last clause.
        let tail = line_buf.trim().to_string();
        if !tail.is_empty() {
            self.handle_stream_line(&tail, &mut speech_buf, &mut escalation, &mut search, &mut plan, record);
        }
        self.drain_sentences(&mut speech_buf, &out, &mut spoken, true).await;

        // Search after the local stream: if the model asked for a web search,
        // perform it and speak the results.
        if let Some(query) = search {
            if let Some(ref searcher) = self.searcher {
                match searcher.search(&query).await {
                    Ok(results) => {
                        let summary = crate::search::summarize(&results);
                        let mut buf = summary;
                        self.drain_sentences(&mut buf, &out, &mut spoken, true).await;
                    }
                    Err(e) => {
                        tracing::warn!("search failed: {e:#}");
                        let mut buf = "I tried to search but couldn't reach the web.".to_string();
                        self.drain_sentences(&mut buf, &out, &mut spoken, true).await;
                    }
                }
            } else {
                let mut buf = "Search isn't configured right now.".to_string();
                self.drain_sentences(&mut buf, &out, &mut spoken, true).await;
            }
        }

        // Lesson plan after the local stream: author it with the big model;
        // it becomes the active lesson, so the next turns teach it.
        if let Some(topic) = plan {
            let mut buf = match self.create_lesson_plan(&topic).await {
                Ok(title) => format!("Lesson plan ready: {title}. Say start the lesson when you want to begin."),
                Err(e) => {
                    tracing::error!("lesson plan failed: {e:#}");
                    "I couldn't write that lesson plan right now.".to_string()
                }
            };
            self.drain_sentences(&mut buf, &out, &mut spoken, true).await;
        }

        // Escalation after the local stream: the big model's answer rides
        // the same sentence channel so it speaks seamlessly after any lead-in.
        if let Some(question) = escalation {
            match self.ask_big(&question).await {
                Ok(big) => {
                    let mut buf = big;
                    self.drain_sentences(&mut buf, &out, &mut spoken, true).await;
                }
                Err(e) => tracing::error!("escalation failed: {e:#}"),
            }
        }

        let reply = if spoken.is_empty() {
            "I'm not sure what to say to that.".to_string()
        } else {
            spoken.join(" ")
        };
        if record {
            self.push_history(text, &reply);
        }
        Ok(reply)
    }

    /// Route one completed line of model output: tool lines to their
    /// handlers, spoken lines into the speech buffer. With `record: false`
    /// (speculation), NOTE lines are dropped instead of written.
    fn handle_stream_line(&self, line: &str, speech_buf: &mut String, escalation: &mut Option<String>, search: &mut Option<String>, plan: &mut Option<String>, record: bool) {
        if line.is_empty() {
            return;
        }
        if let Some(rest) = strip_prefix_ci(line, "note:") {
            if record && !rest.is_empty() {
                self.append_notes(&[rest.to_string()]);
            }
        } else if let Some(rest) = strip_prefix_ci(line, "ask_big:") {
            if !rest.is_empty() {
                *escalation = Some(rest.to_string());
            }
        } else if let Some(rest) = strip_prefix_ci(line, "search:") {
            if !rest.is_empty() {
                *search = Some(rest.to_string());
            }
        } else if let Some(rest) = strip_prefix_ci(line, "plan:") {
            if record && !rest.is_empty() {
                *plan = Some(rest.to_string());
            }
        } else {
            if !speech_buf.is_empty() {
                speech_buf.push(' ');
            }
            speech_buf.push_str(line);
        }
    }

    /// Cut complete sentences out of `speech_buf` and forward them. With
    /// `flush`, the trailing fragment goes too (end of reply). Sentences are
    /// sanitized for speech (no markdown punctuation read aloud).
    async fn drain_sentences(
        &self,
        speech_buf: &mut String,
        out: &tokio::sync::mpsc::Sender<String>,
        spoken: &mut Vec<String>,
        flush: bool,
    ) {
        loop {
            // Sentence ends at . ! ? followed by a space (or at flush, by EOF).
            let cut = speech_buf.char_indices().find_map(|(i, c)| {
                if matches!(c, '.' | '!' | '?') && speech_buf[i + c.len_utf8()..].starts_with(' ') {
                    Some(i + c.len_utf8())
                } else {
                    None
                }
            });
            let Some(end) = cut else { break };
            let sentence: String = speech_buf[..end].trim().to_string();
            speech_buf.drain(..end);
            if !sentence.is_empty() {
                let clean = sanitize_speech(&sentence);
                spoken.push(clean.clone());
                let _ = out.send(clean).await;
            }
        }
        if flush {
            let rest = speech_buf.trim().to_string();
            speech_buf.clear();
            if !rest.is_empty() {
                let clean = sanitize_speech(&rest);
                spoken.push(clean.clone());
                let _ = out.send(clean).await;
            }
        }
    }
}

/// Strip markdown emphasis from spoken text — 4B models leak it even when
/// told not to, and "asterisk" is not a word Five should say.
fn sanitize_speech(s: &str) -> String {
    s.chars().filter(|c| !matches!(c, '*' | '#' | '`' | '_')).collect()
}

/// "The French Revolution!" -> "the-french-revolution"
fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.trim().chars() {
        if c.is_alphanumeric() {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.push(c.to_ascii_lowercase());
        } else {
            dash = true;
        }
    }
    out
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

        // lesson plan tool
        let p = parse_tools("Great idea!\nPLAN: the french revolution");
        assert_eq!(p.speech, "Great idea!");
        assert_eq!(p.plan.as_deref(), Some("the french revolution"));
    }

    #[test]
    fn slugify_cases() {
        assert_eq!(slugify("The French Revolution!"), "the-french-revolution");
        assert_eq!(slugify("  photosynthesis  "), "photosynthesis");
        assert_eq!(slugify("C++ pointers"), "c-pointers");
    }

    #[test]
    fn soul_file_overrides_persona() {
        let dir = std::env::temp_dir().join(format!("five-soul-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let soul_path = dir.join("soul.md");
        std::fs::write(&soul_path, "You are a pirate tutor. Speak like the sea.").unwrap();

        let mut cfg = crate::config::BrainConfig::default();
        cfg.soul_path = Some(soul_path);
        let search_cfg = crate::config::SearchConfig::default();
        let brain = Brain::new(&cfg, &search_cfg).unwrap();
        let prompt = brain.system_prompt();
        assert!(prompt.contains("pirate tutor"));
        // Tool convention always rides along so NOTE:/ASK_BIG: keep working.
        assert!(prompt.contains("NOTE: "));
        assert!(!prompt.contains("voice tutor on a handheld PC"));

        // Missing file falls back to the built-in persona.
        let mut cfg = crate::config::BrainConfig::default();
        cfg.soul_path = Some(dir.join("does-not-exist.md"));
        let brain = Brain::new(&cfg, &search_cfg).unwrap();
        assert!(brain.system_prompt().contains("Five"));

        std::fs::remove_dir_all(&dir).ok();
    }
}
