//! Live dashboard -- HTTP server with Server-Sent Events.
//!
//! Serves a single-page dashboard at `http://localhost:<port>` that shows
//! the full pipeline in real time: utterances, routing decisions, tool
//! invocations, LLM tokens, and what Five is about to speak.
//!
//! Philosophy: speech is accessibility. The dashboard is the primary UI.

use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Event kinds pushed to the dashboard.
#[derive(Debug, Clone)]
pub enum DashEvent {
    /// User said something (post-transcription).
    Utterance { text: String, timestamp: String },
    /// Five's routing decision (local/Kimi/deterministic).
    Thinking { step: String, detail: String },
    /// A tool was invoked (NOTE, SEARCH, ASK_BIG, WRITE_FILE).
    Tool { name: String, args: String },
    /// LLM token or sentence arriving.
    Response { text: String, done: bool },
    /// What Five is about to speak (pre-TTS).
    Speak { text: String },
    /// Home Assistant command executed.
    Home { command: String, result: String },
    /// Brain mode switched.
    Mode { mode: String },
    /// File created or modified.
    File { path: String, action: String },
    /// System message (errors, state changes).
    System { message: String },
}

impl DashEvent {
    fn to_sse(&self) -> String {
        let (event, data) = match self {
            DashEvent::Utterance { text, timestamp } => (
                "utterance",
                format!("{{\"text\":{},\"time\":{}}}", serde_json::json!(text), serde_json::json!(timestamp)),
            ),
            DashEvent::Thinking { step, detail } => (
                "thinking",
                format!("{{\"step\":{},\"detail\":{}}}", serde_json::json!(step), serde_json::json!(detail)),
            ),
            DashEvent::Tool { name, args } => (
                "tool",
                format!("{{\"name\":{},\"args\":{}}}", serde_json::json!(name), serde_json::json!(args)),
            ),
            DashEvent::Response { text, done } => (
                "response",
                format!("{{\"text\":{},\"done\":{}}}", serde_json::json!(text), done),
            ),
            DashEvent::Speak { text } => (
                "speak",
                format!("{{\"text\":{}}}", serde_json::json!(text)),
            ),
            DashEvent::Home { command, result } => (
                "home",
                format!("{{\"command\":{},\"result\":{}}}", serde_json::json!(command), serde_json::json!(result)),
            ),
            DashEvent::Mode { mode } => (
                "mode",
                format!("{{\"mode\":{}}}", serde_json::json!(mode)),
            ),
            DashEvent::File { path, action } => (
                "file",
                format!("{{\"path\":{},\"action\":{}}}", serde_json::json!(path), serde_json::json!(action)),
            ),
            DashEvent::System { message } => (
                "system",
                format!("{{\"message\":{}}}", serde_json::json!(message)),
            ),
        };
        format!("event: {event}\ndata: {data}\n\n")
    }
}

/// Dashboard server state -- broadcast channel for SSE clients.
pub struct Dashboard {
    tx: tokio::sync::broadcast::Sender<DashEvent>,
    port: u16,
}

impl Dashboard {
    pub fn new(port: u16) -> Self {
        let (tx, _rx) = tokio::sync::broadcast::channel::<DashEvent>(256);
        Self { tx, port }
    }

    /// Spawn the HTTP server. Returns immediately; server runs in background.
    pub fn spawn(&self) {
        let tx = self.tx.clone();
        let port = self.port;
        tokio::spawn(async move {
            let addr = SocketAddr::from(([127, 0, 0, 1], port));
            let listener = match TcpListener::bind(addr).await {
                Ok(l) => l,
                Err(e) => {
                    tracing::error!("dashboard bind failed on port {}: {}", port, e);
                    return;
                }
            };
            tracing::info!("dashboard running at http://127.0.0.1:{}", port);
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let tx = tx.clone();
                        tokio::spawn(handle_client(stream, tx));
                    }
                    Err(e) => {
                        tracing::error!("dashboard accept error: {}", e);
                    }
                }
            }
        });
    }

    /// Push an event to all connected dashboard clients.
    pub fn push(&self, event: DashEvent) {
        let _ = self.tx.send(event);
    }
}

async fn handle_client(mut stream: TcpStream, tx: tokio::sync::broadcast::Sender<DashEvent>) {
    let mut reader = BufReader::new(&mut stream);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).await.is_err() {
        return;
    }

    let path = request_line.split_whitespace().nth(1).unwrap_or("/");

    // Drain headers
    let mut line = String::new();
    while reader.read_line(&mut line).await.is_ok() {
        if line == "\r\n" || line == "\n" {
            break;
        }
        line.clear();
    }

    match path {
        "/events" => {
            let mut rx = tx.subscribe();
            let headers = "HTTP/1.1 200 OK\r\n\
                Content-Type: text/event-stream\r\n\
                Cache-Control: no-cache\r\n\
                Connection: keep-alive\r\n\r\n";
            if stream.write_all(headers.as_bytes()).await.is_err() {
                return;
            }
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        let sse = event.to_sse();
                        if stream.write_all(sse.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
        _ => {
            let html = DASHBOARD_HTML;
            let headers = format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: text/html; charset=utf-8\r\n\
                 Content-Length: {}\r\n\r\n",
                html.len()
            );
            let _ = stream.write_all(headers.as_bytes()).await;
            let _ = stream.write_all(html).await;
        }
    }
}

const DASHBOARD_HTML: &[u8] = br#"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Five -- Dashboard</title>
<style>
:root { --bg:#0d0d0f; --panel:#16161a; --text:#e4e4e7; --muted:#71717a;
       --accent:#7c3aed; --utter:#22c55e; --think:#f59e0b; --tool:#06b6d4;
       --resp:#e879f9; --speak:#f472b6; --home:#60a5fa; --file:#a78bfa;
       --warn:#ef4444; --border:#27272a; }
* { box-sizing:border-box; margin:0; padding:0; }
body { background:var(--bg); color:var(--text); font-family:system-ui,-apple-system,sans-serif;
       height:100vh; display:flex; flex-direction:column; overflow:hidden; }
header { padding:12px 20px; border-bottom:1px solid var(--border); display:flex;
         align-items:center; justify-content:space-between; background:var(--panel); }
h1 { font-size:1.1rem; font-weight:600; letter-spacing:-0.02em; }
#status { display:flex; align-items:center; gap:6px; font-size:0.8rem; color:var(--muted); }
#status::before { content:''; width:8px; height:8px; border-radius:50%; background:var(--warn); }
#status.connected::before { background:var(--utter); }
main { flex:1; overflow-y:auto; padding:16px 20px; display:flex; flex-direction:column; gap:10px; }
.card { background:var(--panel); border:1px solid var(--border); border-radius:10px;
        padding:12px 14px; animation:slideIn 0.2s ease; }
@keyframes slideIn { from { opacity:0; transform:translateY(8px);} to { opacity:1; transform:translateY(0);} }
.card .meta { display:flex; align-items:center; gap:8px; margin-bottom:6px; font-size:0.7rem;
              text-transform:uppercase; letter-spacing:0.06em; font-weight:600; }
.card .body { font-size:0.92rem; line-height:1.5; white-space:pre-wrap; word-break:break-word; }
.card.utterance { border-left:3px solid var(--utter); }
.card.utterance .meta { color:var(--utter); }
.card.thinking { border-left:3px solid var(--think); opacity:0.85; }
.card.thinking .meta { color:var(--think); }
.card.tool { border-left:3px solid var(--tool); }
.card.tool .meta { color:var(--tool); }
.card.response { border-left:3px solid var(--resp); }
.card.response .meta { color:var(--resp); }
.card.speak { border-left:3px solid var(--speak); background:rgba(244,114,182,0.05); }
.card.speak .meta { color:var(--speak); }
.card.home { border-left:3px solid var(--home); }
.card.home .meta { color:var(--home); }
.card.file { border-left:3px solid var(--file); }
.card.file .meta { color:var(--file); }
.card.system { border-left:3px solid var(--warn); }
.card.system .meta { color:var(--warn); }
.timestamp { margin-left:auto; color:var(--muted); font-size:0.7rem; text-transform:none; }
#input-bar { padding:12px 20px; border-top:1px solid var(--border); background:var(--panel);
             display:flex; gap:8px; }
#text-input { flex:1; background:var(--bg); border:1px solid var(--border); border-radius:8px;
              padding:10px 14px; color:var(--text); font-size:0.9rem; outline:none; }
#text-input:focus { border-color:var(--accent); }
#send-btn { background:var(--accent); color:#fff; border:none; border-radius:8px; padding:0 18px;
            font-weight:600; cursor:pointer; font-size:0.85rem; }
#send-btn:hover { filter:brightness(1.1); }
</style>
</head>
<body>
<header>
  <h1>Five Dashboard</h1>
  <div id="status">Connecting...</div>
</header>
<main id="feed"></main>
<div id="input-bar">
  <input type="text" id="text-input" placeholder="Type a command... (Enter to send)">
  <button id="send-btn">Send</button>
</div>
<script>
const feed = document.getElementById('feed');
const status = document.getElementById('status');
const input = document.getElementById('text-input');
const sendBtn = document.getElementById('send-btn');

function addCard(kind, meta, body, extra='') {
  const card = document.createElement('div');
  card.className = `card ${kind}`;
  const time = new Date().toLocaleTimeString([], {hour:'2-digit', minute:'2-digit', second:'2-digit'});
  card.innerHTML = `<div class="meta">${meta}<span class="timestamp">${time}</span></div><div class="body">${escapeHtml(body)}</div>${extra}`;
  feed.appendChild(card);
  feed.scrollTop = feed.scrollHeight;
}

function escapeHtml(t) {
  const d = document.createElement('div');
  d.textContent = t;
  return d.innerHTML;
}

function connect() {
  const es = new EventSource('/events');
  es.onopen = () => { status.textContent = 'Live'; status.classList.add('connected'); };
  es.onerror = () => { status.textContent = 'Reconnecting...'; status.classList.remove('connected'); };

  es.addEventListener('utterance', e => {
    const d = JSON.parse(e.data);
    addCard('utterance', 'You said', d.text);
  });
  es.addEventListener('thinking', e => {
    const d = JSON.parse(e.data);
    addCard('thinking', d.step, d.detail);
  });
  es.addEventListener('tool', e => {
    const d = JSON.parse(e.data);
    addCard('tool', d.name, d.args);
  });
  es.addEventListener('response', e => {
    const d = JSON.parse(e.data);
    if (d.done) addCard('response', 'Response', d.text);
  });
  es.addEventListener('speak', e => {
    const d = JSON.parse(e.data);
    addCard('speak', 'Speaking', d.text);
  });
  es.addEventListener('home', e => {
    const d = JSON.parse(e.data);
    addCard('home', 'Home', `${d.command} -> ${d.result}`);
  });
  es.addEventListener('mode', e => {
    const d = JSON.parse(e.data);
    addCard('system', 'Mode', `Switched to ${d.mode || 'normal'}`);
  });
  es.addEventListener('file', e => {
    const d = JSON.parse(e.data);
    addCard('file', 'File', `${d.action}: ${d.path}`);
  });
  es.addEventListener('system', e => {
    const d = JSON.parse(e.data);
    addCard('system', 'System', d.message);
  });
}

connect();

// Text input support (for when user types instead of speaks)
async function sendText() {
  const text = input.value.trim();
  if (!text) return;
  input.value = '';
  addCard('utterance', 'You typed', text);
  // POST to a local endpoint if Five exposes one, or just show in dashboard
  // For now, this just logs to the dashboard -- Five needs a text-input receiver
}

sendBtn.addEventListener('click', sendText);
input.addEventListener('keydown', e => { if (e.key === 'Enter') sendText(); });
</script>
</body>
</html>"#;
