//! Almanach integration — bidirectional chat bridge.
//!
//! Five can pipe transcribed speech into Almanach chat conversations
//! and read LLM responses back via TTS. This turns Five into a voice
//! interface for the Almanach tutor.
//!
//! Flow:
//!   1. User speaks → Five transcribes
//!   2. Five POSTs message to Almanach /api/conversations/:id/messages
//!   3. Almanach streams response via SSE
//!   4. Five reads each chunk via TTS as it arrives
//!
//! Authentication: JWT token from Almanach login, refreshed as needed.

use std::time::{Duration, Instant};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::config::AlmanachConfig;

/// Almanach API client.
pub struct AlmanachClient {
    client: reqwest::Client,
    base_url: String,
    token: Option<String>,
    token_expires: Option<Instant>,
    username: String,
    password: String,
    current_conversation: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SendMessageRequest {
    pub content: String,
    pub role: String, // "user"
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SendMessageResponse {
    pub id: String,
    pub content: String,
    pub role: String,
}

impl AlmanachClient {
    pub fn new(cfg: &AlmanachConfig) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(cfg.timeout_sec))
            .build()
            .context("building Almanach HTTP client")?;

        Ok(Self {
            client,
            base_url: cfg.url.trim_end_matches('/').to_string(),
            token: None,
            token_expires: None,
            username: cfg.username.clone(),
            password: cfg.password.clone(),
            current_conversation: None,
        })
    }

    /// Ensure we have a valid token, logging in if needed.
    async fn ensure_auth(&mut self) -> anyhow::Result<String> {
        if let Some(ref token) = self.token {
            if let Some(expires) = self.token_expires {
                if expires > Instant::now() + Duration::from_secs(60) {
                    return Ok(token.clone());
                }
            }
        }

        info!("refreshing Almanach auth token");
        let url = format!("{}/api/login", self.base_url);
        let resp = self
            .client
            .post(&url)
            .json(&serde_json::json!({
                "password": self.password,
            }))
            .send()
            .await
            .with_context(|| format!("Almanach login request failed"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Almanach login failed: {} — {}", status, body);
        }

        let data: serde_json::Value = resp.json().await.context("parsing Almanach login response")?;
        let token = data["access_token"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("no access_token in Almanach login response"))?
            .to_string();
        let expires_in = data["expires_in"].as_i64().unwrap_or(3600) as u64;

        self.token = Some(token.clone());
        self.token_expires = Some(Instant::now() + Duration::from_secs(expires_in));
        info!("Almanach auth refreshed, expires in {}s", expires_in);

        Ok(token)
    }

    /// Create a new conversation and store its ID.
    pub async fn create_conversation(&mut self, title: &str) -> anyhow::Result<String> {
        let token = self.ensure_auth().await?;
        let url = format!("{}/api/conversations", self.base_url);
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&serde_json::json!({ "title": title }))
            .send()
            .await
            .with_context(|| format!("creating Almanach conversation"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Almanach create_conversation failed: {} — {}", status, body);
        }

        let data: serde_json::Value = resp.json().await.context("parsing conversation response")?;
        let id = data["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("no id in conversation response"))?
            .to_string();

        self.current_conversation = Some(id.clone());
        info!(conversation_id = %id, "Almanach conversation created");
        Ok(id)
    }

    /// Send a message to the current conversation and stream the response.
    /// `on_chunk` is called for each response chunk (for TTS + dashboard).
    pub async fn send_message_stream<F>(
        &mut self,
        content: &str,
        mut on_chunk: F,
    ) -> anyhow::Result<String>
    where
        F: FnMut(&str),
    {
        let token = self.ensure_auth().await?;
        let conversation_id = self
            .current_conversation
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no active conversation — create one first"))?
            .clone();

        let url = format!(
            "{}/api/conversations/{}/messages",
            self.base_url, conversation_id
        );

        debug!(conversation_id = %conversation_id, content, "sending message to Almanach");

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .header("Accept", "text/event-stream")
            .json(&serde_json::json!({
                "content": content,
                "role": "user",
            }))
            .send()
            .await
            .with_context(|| format!("sending message to Almanach"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Almanach message failed: {} — {}", status, body);
        }

        // Stream SSE response
        let mut stream = resp.bytes_stream();
        let mut full_response = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("reading Almanach SSE chunk")?;
            let text = String::from_utf8_lossy(&chunk);

            for line in text.lines() {
                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        return Ok(full_response);
                    }

                    // Try to parse as JSON message chunk
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                        if let Some(content) = json["content"].as_str() {
                            full_response.push_str(content);
                            on_chunk(content);
                        }
                    }
                }
            }
        }

        Ok(full_response)
    }

    /// Send a message and return the full response (non-streaming).
    pub async fn send_message(&mut self, content: &str) -> anyhow::Result<String> {
        let mut response = String::new();
        self.send_message_stream(content, |chunk| {
            response.push_str(chunk);
        })
        .await?;
        Ok(response)
    }

    /// Set an existing conversation as active.
    pub fn set_conversation(&mut self, id: String) {
        self.current_conversation = Some(id);
    }

    /// Close the current conversation.
    pub fn close_conversation(&mut self) {
        self.current_conversation = None;
    }
}

// Need this for bytes_stream()
use futures_util::StreamExt;
