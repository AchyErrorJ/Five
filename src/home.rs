//! Smart home control via Home Assistant REST API.
//!
//! Voice commands:
//!   "turn on the living room light"
//!   "turn off the bedroom light"
//!   "dim the kitchen to fifty percent"
//!   "set the office to red"
//!
//! The config maps friendly names ("living room light") to HA entity IDs
//! ("light.living_room"). If no exact match, we fall back to fuzzy search.

use anyhow::Context;
use crate::config::HomeConfig;

/// Parsed voice command for a home action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomeCommand {
    TurnOn { device: String },
    TurnOff { device: String },
    SetBrightness { device: String, percent: u8 },
    SetColor { device: String, color_name: String },
    ActivateScene { scene: String },
}

/// Try to parse a home command from transcribed voice text.
pub fn parse_command(text: &str) -> Option<HomeCommand> {
    let t = text.to_lowercase();

    // Scene activation: "activate movie mode", "movie scene"
    if let Some(scene) = t
        .strip_prefix("activate ")
        .or_else(|| t.strip_prefix("turn on "))
    {
        let scene = scene.trim().trim_end_matches(" scene").trim();
        if !scene.is_empty() {
            return Some(HomeCommand::ActivateScene {
                scene: scene.to_string(),
            });
        }
    }

    // Turn on/off: "turn on the living room light", "switch off the kitchen"
    let on_patterns = &["turn on ", "switch on ", "enable "];
    let off_patterns = &["turn off ", "switch off ", "disable "];

    for pat in on_patterns {
        if let Some(rest) = t.strip_prefix(pat) {
            let device = strip_leading_the(rest);
            return Some(HomeCommand::TurnOn {
                device: device.to_string(),
            });
        }
    }
    for pat in off_patterns {
        if let Some(rest) = t.strip_prefix(pat) {
            let device = strip_leading_the(rest);
            return Some(HomeCommand::TurnOff {
                device: device.to_string(),
            });
        }
    }

    // Brightness: "dim the bedroom to fifty percent", "set kitchen brightness to 50"
    if t.starts_with("dim ") || t.starts_with("set ") {
        if let Some((device, percent)) = extract_brightness(&t) {
            return Some(HomeCommand::SetBrightness { device, percent });
        }
    }

    // Color: "set the office to red", "make the bedroom blue"
    if t.starts_with("set ") || t.starts_with("make ") || t.starts_with("change ") {
        if let Some((device, color)) = extract_color(&t) {
            return Some(HomeCommand::SetColor {
                device,
                color_name: color,
            });
        }
    }

    None
}

fn strip_leading_the(s: &str) -> &str {
    s.trim().strip_prefix("the ").unwrap_or(s).trim()
}

fn extract_brightness(text: &str) -> Option<(String, u8)> {
    // "dim the bedroom to fifty percent" or "set kitchen brightness to 50"
    let re = regex::Regex::new(
        r"(?:dim|set)\s+(?:the\s+)?(.+?)\s+(?:brightness\s+)?to\s+(\w+)\s*(?:percent|%)?",
    )
    .ok()?;
    let caps = re.captures(text)?;
    let device = caps.get(1)?.as_str().trim().to_string();
    let num_word = caps.get(2)?.as_str().trim();
    let percent = parse_number_word(num_word)?;
    Some((device, percent))
}

fn extract_color(text: &str) -> Option<(String, String)> {
    // "set the office to red" or "make the bedroom blue"
    let re =
        regex::Regex::new(r"(?:set|make|change)\s+(?:the\s+)?(.+?)\s+to\s+(\w+)").ok()?;
    let caps = re.captures(text)?;
    let device = caps.get(1)?.as_str().trim().to_string();
    let color = caps.get(2)?.as_str().trim().to_string();
    // Only accept known color-ish words
    const COLORS: &[&str] = &[
        "red", "green", "blue", "yellow", "orange", "purple", "pink", "white",
        "warm", "cool", "daylight", "soft",
    ];
    if COLORS.contains(&color.as_str()) {
        Some((device, color))
    } else {
        None
    }
}

fn parse_number_word(word: &str) -> Option<u8> {
    // Try digit first
    if let Ok(n) = word.parse::<u8>() {
        return Some(n.min(100));
    }
    // Word numbers
    match word {
        "zero" => Some(0),
        "one" => Some(1),
        "two" => Some(2),
        "three" => Some(3),
        "four" => Some(4),
        "five" => Some(5),
        "ten" => Some(10),
        "fifteen" => Some(15),
        "twenty" => Some(20),
        "twenty five" | "twenty-five" => Some(25),
        "thirty" => Some(30),
        "forty" => Some(40),
        "fifty" => Some(50),
        "sixty" => Some(60),
        "seventy" => Some(70),
        "seventy five" | "seventy-five" => Some(75),
        "eighty" => Some(80),
        "ninety" => Some(90),
        "hundred" | "one hundred" => Some(100),
        _ => None,
    }
}

/// Home Assistant API client.
pub struct HomeClient {
    http: reqwest::Client,
    url: String,
    token: String,
    devices: std::collections::HashMap<String, String>,
}

impl HomeClient {
    pub fn new(cfg: &HomeConfig) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(cfg.timeout_sec))
            .build()?;
        Ok(Self {
            http,
            url: cfg.url.trim_end_matches('/').to_string(),
            token: cfg.token.clone(),
            devices: cfg.devices.clone(),
        })
    }

    /// Return all configured device names and scene names.
    pub fn list_entities(&self) -> (Vec<String>, Vec<String>) {
        let mut devices = Vec::new();
        let mut scenes = Vec::new();
        for (name, entity_id) in &self.devices {
            if entity_id.starts_with("scene.") {
                scenes.push(name.clone());
            } else {
                devices.push(name.clone());
            }
        }
        devices.sort();
        scenes.sort();
        (devices, scenes)
    }

    /// Resolve a friendly device name to an HA entity_id.
    fn resolve(&self, name: &str) -> Option<String> {
        let key = name.to_lowercase();
        // Exact match first
        if let Some(id) = self.devices.get(&key) {
            return Some(id.clone());
        }
        // Fuzzy: contains
        for (k, v) in &self.devices {
            if k.contains(&key) || key.contains(k) {
                return Some(v.clone());
            }
        }
        None
    }

    /// Execute a parsed home command against Home Assistant.
    pub async fn execute(&self, cmd: &HomeCommand) -> anyhow::Result<String> {
        match cmd {
            HomeCommand::TurnOn { device } => {
                let entity = self.resolve(device).context("unknown device")?;
                self.call_service("homeassistant", "turn_on", &entity).await?;
                Ok(format!("Turned on the {device}."))
            }
            HomeCommand::TurnOff { device } => {
                let entity = self.resolve(device).context("unknown device")?;
                self.call_service("homeassistant", "turn_off", &entity).await?;
                Ok(format!("Turned off the {device}."))
            }
            HomeCommand::SetBrightness { device, percent } => {
                let entity = self.resolve(device).context("unknown device")?;
                let brightness = (*percent as f32 / 100.0 * 255.0) as u8;
                self.call_service_with_data(
                    "light",
                    "turn_on",
                    &entity,
                    serde_json::json!({ "brightness": brightness }),
                )
                .await?;
                Ok(format!("Set the {device} to {percent} percent."))
            }
            HomeCommand::SetColor { device, color_name } => {
                let entity = self.resolve(device).context("unknown device")?;
                let rgb = color_to_rgb(color_name);
                self.call_service_with_data(
                    "light",
                    "turn_on",
                    &entity,
                    serde_json::json!({ "rgb_color": [rgb.0, rgb.1, rgb.2] }),
                )
                .await?;
                Ok(format!("Set the {device} to {color_name}."))
            }
            HomeCommand::ActivateScene { scene } => {
                let entity = self.resolve(scene).context("unknown scene")?;
                self.call_service("scene", "turn_on", &entity).await?;
                Ok(format!("Activated {scene} scene."))
            }
        }
    }

    async fn call_service(
        &self,
        domain: &str,
        service: &str,
        entity_id: &str,
    ) -> anyhow::Result<()> {
        let url = format!("{}/api/services/{}/{}", self.url, domain, service);
        let body = serde_json::json!({ "entity_id": entity_id });
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .context("Home Assistant request failed")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Home Assistant returned {}: {}", status, &body[..body.len().min(200)]);
        }
        Ok(())
    }

    async fn call_service_with_data(
        &self,
        domain: &str,
        service: &str,
        entity_id: &str,
        data: serde_json::Value,
    ) -> anyhow::Result<()> {
        let url = format!("{}/api/services/{}/{}", self.url, domain, service);
        let mut body = serde_json::Map::new();
        body.insert("entity_id".into(), entity_id.into());
        if let serde_json::Value::Object(map) = data {
            for (k, v) in map {
                body.insert(k, v);
            }
        }
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Content-Type", "application/json")
            .json(&serde_json::Value::Object(body))
            .send()
            .await
            .context("Home Assistant request failed")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Home Assistant returned {}: {}", status, &body[..body.len().min(200)]);
        }
        Ok(())
    }
}

fn color_to_rgb(name: &str) -> (u8, u8, u8) {
    match name {
        "red" => (255, 0, 0),
        "green" => (0, 255, 0),
        "blue" => (0, 0, 255),
        "yellow" => (255, 255, 0),
        "orange" => (255, 165, 0),
        "purple" => (128, 0, 128),
        "pink" => (255, 192, 203),
        "white" => (255, 255, 255),
        "warm" => (255, 223, 186),
        "cool" => (200, 220, 255),
        "daylight" => (255, 255, 240),
        "soft" => (255, 240, 220),
        _ => (255, 255, 255),
    }
}
