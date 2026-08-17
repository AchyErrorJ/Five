//! HTTP client for the Orchestre orchestrator (Five's "openclaw" endpoint).
//!
//! Contract (from Orchestre orchestrator source, 2026-08-17):
//! - `POST /auth/login` `{password}` → `{access_token, refresh_token, ...}`;
//!   all `/api/*` routes require `Authorization: Bearer <access_token>`.
//! - `POST /api/agents/{from_id}/send` with body
//!   `{to, content, message_type, timeout}` →
//!   `{message_id, status, response?, error?}`.
//!   `from_id` must be a Running agent; `to` accepts agent ID *or* name.
//!   With `message_type: "request"`, `response` carries the agent's reply.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::config::OpenClawConfig;

/// A voice command's delivery result.
#[derive(Debug)]
#[allow(dead_code)] // message_id is for future correlation/logging
pub struct CommandResult {
    pub message_id: String,
    pub status: String,
    /// The agent's reply text, if it answered within the request timeout.
    pub response: Option<String>,
}

pub struct OrchestreClient {
    http: reqwest::Client,
    base: String,
    password: String,
    from_agent: String,
    to_agent: String,
    timeout_sec: u64,
    token: Arc<RwLock<Option<String>>>,
}

#[derive(Serialize)]
struct LoginRequest<'a> {
    password: &'a str,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Serialize)]
struct SendMessageRequest<'a> {
    to: &'a str,
    content: &'a str,
    message_type: &'a str,
    timeout: u64,
}

#[derive(Deserialize)]
struct SendMessageResponse {
    message_id: String,
    status: MessageStatus,
    #[serde(default)]
    response: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum MessageStatus {
    Queued,
    Delivering,
    Delivered,
    Failed,
    Read,
}

impl std::fmt::Display for MessageStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl OrchestreClient {
    pub fn new(config: &OpenClawConfig) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_sec))
            .build()
            .context("failed to build HTTP client")?;
        Ok(Self {
            http,
            base: config.endpoint.trim_end_matches('/').to_string(),
            password: config.password.clone(),
            from_agent: config.from_agent.clone(),
            to_agent: config.to_agent.clone(),
            timeout_sec: config.timeout_sec,
            token: Arc::new(RwLock::new(None)),
        })
    }

    /// Exchange the admin password for a JWT and cache it.
    async fn login(&self) -> anyhow::Result<String> {
        let resp = self
            .http
            .post(format!("{}/auth/login", self.base))
            .json(&LoginRequest { password: &self.password })
            .send()
            .await
            .context("login request failed — is the orchestrator running?")?;
        if !resp.status().is_success() {
            anyhow::bail!("orchestrator login failed: HTTP {}", resp.status());
        }
        let token = resp
            .json::<TokenResponse>()
            .await
            .context("failed to parse login response")?
            .access_token;
        *self.token.write().await = Some(token.clone());
        info!("authenticated with orchestrator");
        Ok(token)
    }

    async fn bearer(&self) -> anyhow::Result<String> {
        if let Some(t) = self.token.read().await.as_ref() {
            return Ok(t.clone());
        }
        self.login().await
    }

    /// Send a transcribed voice command to the configured agent.
    /// Re-authenticates and retries once on 401 (expired JWT).
    pub async fn send_command(&self, text: &str) -> anyhow::Result<CommandResult> {
        for attempt in 0..2 {
            let token = self.bearer().await?;
            let resp = self
                .http
                .post(format!("{}/api/agents/{}/send", self.base, self.from_agent))
                .bearer_auth(token)
                .json(&SendMessageRequest {
                    to: &self.to_agent,
                    content: text,
                    message_type: "request",
                    timeout: self.timeout_sec,
                })
                .send()
                .await
                .context("send request failed")?;

            if resp.status() == reqwest::StatusCode::UNAUTHORIZED && attempt == 0 {
                warn!("JWT expired or rejected; re-authenticating");
                *self.token.write().await = None;
                continue;
            }
            if resp.status() == reqwest::StatusCode::NOT_FOUND {
                let body = resp.text().await.unwrap_or_default();
                anyhow::bail!(
                    "orchestrator 404: {body} — does agent {:?} exist and is it Running?",
                    self.from_agent
                );
            }
            if !resp.status().is_success() {
                anyhow::bail!("send failed: HTTP {} — {}", resp.status(), resp.text().await.unwrap_or_default());
            }

            let msg: SendMessageResponse = resp
                .json()
                .await
                .context("failed to parse send response")?;
            debug!(id = %msg.message_id, status = %msg.status, "command delivered");
            if let Some(err) = msg.error {
                anyhow::bail!("orchestrator reported delivery error: {err}");
            }
            return Ok(CommandResult {
                message_id: msg.message_id,
                status: msg.status.to_string(),
                response: msg.response,
            });
        }
        Err(anyhow!("authentication retry exhausted"))
    }
}
