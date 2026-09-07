//! Direct light control — Nanoleaf local Open API (port 16021).
//!
//! Voice commands:
//!   "turn on the nanoleaf"
//!   "turn off the panels"
//!   "dim the nanoleaf to forty percent"
//!   "nanoleaf effect northern lights"
//!   "list nanoleaf effects"
//!
//! Pairing (one-time): put the controller in pairing mode (hold power 5–7s,
//! or Nanoleaf app → device settings → Connect to API), then run
//! `five-daemon pair-nanoleaf <name>` — the token is written to the
//! configured token_file (git-ignored). Newer firmware (Outdoor String
//! Lights, some Matter Essentials) answers GET instead of POST on
//! /api/v1/new; we try both.

use anyhow::Context;
use crate::config::LightsConfig;

/// Parsed voice command for a light action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LightCommand {
    TurnOn { device: String },
    TurnOff { device: String },
    SetBrightness { device: String, percent: u8 },
    SetEffect { device: String, effect: String },
    ListEffects { device: String },
}

/// Names that clearly refer to a light device — used to decide whether a
/// generic "turn off the X" belongs to us before Home Assistant gets it.
fn mentions_light(t: &str, names: &[&str]) -> Option<String> {
    for n in names {
        if t.contains(n) {
            return Some(n.to_string());
        }
    }
    None
}

/// Try to parse a light command from transcribed voice text.
/// `names` are the configured device names ("nanoleaf", "panels", ...).
pub fn parse_command(text: &str, names: &[&str]) -> Option<LightCommand> {
    let t = text.to_lowercase();
    let t = t.trim().trim_end_matches(['.', '!', '?']);

    // Effects: "nanoleaf effect northern lights", "set nanoleaf effect to calm"
    if let Some(rest) = t.split(" effect ").nth(1).or_else(|| {
        t.strip_prefix("nanoleaf effect ").map(|r| r)
    }) {
        let device = t.split(" effect ").next().unwrap_or("nanoleaf").trim();
        let effect = rest.trim().trim_start_matches("to ").trim();
        if !effect.is_empty() {
            return Some(LightCommand::SetEffect {
                device: device.to_string(),
                effect: effect.to_string(),
            });
        }
    }

    // "list nanoleaf effects"
    if t.contains("list") && t.contains("effect") {
        if let Some(d) = mentions_light(t, names) {
            return Some(LightCommand::ListEffects { device: d });
        }
    }

    let device = mentions_light(t, names)?;

    // Brightness: "dim the nanoleaf to forty percent", "set nanoleaf brightness to 50"
    if t.starts_with("dim ") || t.starts_with("set ") {
        if let Some(percent) = extract_percent(&t) {
            return Some(LightCommand::SetBrightness { device, percent });
        }
    }

    // On/off
    if t.starts_with("turn on ") || t.starts_with("switch on ") || t.contains("turn on the") {
        return Some(LightCommand::TurnOn { device });
    }
    if t.starts_with("turn off ") || t.starts_with("switch off ") || t.contains("turn off the") {
        return Some(LightCommand::TurnOff { device });
    }

    None
}

fn extract_percent(t: &str) -> Option<u8> {
    // Strip the unit first so "forty percent" captures just "forty".
    let t = t.replace(" percent", "").replace('%', "");
    let re = regex::Regex::new(r"to\s+([a-z0-9]+(?:\s+[a-z0-9]+)?)\s*$").ok()?;
    let caps = re.captures(&t)?;
    let word = caps.get(1)?.as_str().trim();
    parse_number_word(word)
}

fn parse_number_word(word: &str) -> Option<u8> {
    if let Ok(n) = word.parse::<u8>() {
        return Some(n.min(100));
    }
    match word {
        "zero" => Some(0), "ten" => Some(10), "fifteen" => Some(15),
        "twenty" => Some(20), "twenty five" | "twenty-five" => Some(25),
        "thirty" => Some(30), "forty" => Some(40), "fifty" => Some(50),
        "sixty" => Some(60), "seventy" => Some(70),
        "seventy five" | "seventy-five" => Some(75),
        "eighty" => Some(80), "ninety" => Some(90),
        "hundred" | "one hundred" | "full" | "max" => Some(100),
        _ => None,
    }
}

/// One paired Nanoleaf controller.
pub struct NanoleafClient {
    http: reqwest::Client,
    /// e.g. "http://192.168.1.50:16021/api/v1/<token>"
    base: String,
}

impl NanoleafClient {
    fn new(host: &str, token: &str, timeout_sec: u64) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_sec))
            .build()?;
        let host = host.trim_end_matches('/');
        let host = if host.starts_with("http") {
            host.to_string()
        } else {
            format!("http://{host}")
        };
        Ok(Self {
            http,
            base: format!("{host}:16021/api/v1/{token}"),
        })
    }

    /// Load token from its file (written by `five-daemon pair-nanoleaf`).
    pub fn from_config(cfg: &crate::config::NanoleafConfig, timeout_sec: u64) -> anyhow::Result<Self> {
        let token = std::fs::read_to_string(&cfg.token_file)
            .with_context(|| format!("no Nanoleaf token at {} — run pair-nanoleaf", cfg.token_file.display()))?;
        Self::new(&cfg.host, token.trim(), timeout_sec)
    }

    /// Pair with a controller in pairing mode. POST first (classic panels),
    /// GET as fallback (Outdoor String Lights, some Matter Essentials).
    pub async fn pair(host: &str, timeout_sec: u64) -> anyhow::Result<String> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_sec))
            .build()?;
        let host = host.trim_end_matches('/');
        let host = if host.starts_with("http") { host.to_string() } else { format!("http://{host}") };
        let url = format!("{host}:16021/api/v1/new");
        let resp = http
            .post(&url)
            .send()
            .await
            .context("pairing request failed — is the controller on the network?")?;
        let body = if resp.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            http.get(&url).send().await.context("GET fallback failed")?.text().await?
        } else if resp.status() == reqwest::StatusCode::FORBIDDEN
            || resp.status() == reqwest::StatusCode::UNAUTHORIZED
        {
            anyhow::bail!("pairing window not open — hold the power button 5–7s (or Nanoleaf app → Connect to API) and retry within 30s");
        } else if !resp.status().is_success() {
            anyhow::bail!("pairing returned HTTP {}", resp.status());
        } else {
            resp.text().await?
        };
        let v: serde_json::Value = serde_json::from_str(&body).context("bad pairing response")?;
        v.get("auth_token")
            .and_then(|t| t.as_str())
            .map(|s| s.to_string())
            .context("no auth_token in pairing response")
    }

    async fn put_state(&self, body: serde_json::Value) -> anyhow::Result<()> {
        let resp = self
            .http
            .put(format!("{}/state", self.base))
            .json(&body)
            .send()
            .await
            .context("Nanoleaf request failed — device offline?")?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            anyhow::bail!("token rejected (401) — re-run pair-nanoleaf");
        }
        if !resp.status().is_success() {
            anyhow::bail!("Nanoleaf returned HTTP {}", resp.status());
        }
        Ok(())
    }

    async fn power(&self, on: bool) -> anyhow::Result<()> {
        self.put_state(serde_json::json!({ "on": { "value": on } })).await
    }

    async fn brightness(&self, percent: u8) -> anyhow::Result<()> {
        self.put_state(serde_json::json!({ "brightness": { "value": percent.min(100) } })).await
    }

    async fn effect(&self, name: &str) -> anyhow::Result<()> {
        self.put_state(serde_json::json!({ "select": name })).await
    }

    async fn effects(&self) -> anyhow::Result<Vec<String>> {
        let resp = self
            .http
            .get(format!("{}/effects/effectsList", self.base))
            .send()
            .await
            .context("Nanoleaf request failed")?;
        if !resp.status().is_success() {
            anyhow::bail!("Nanoleaf returned HTTP {}", resp.status());
        }
        Ok(resp.json::<Vec<String>>().await?)
    }
}

/// All configured lights; resolves friendly names to clients.
pub struct Lights {
    nanoleaf: Vec<(String, NanoleafClient)>,
    names: Vec<String>,
}

impl Lights {
    pub fn new(cfg: &LightsConfig) -> Self {
        let mut nanoleaf = Vec::new();
        let mut names = Vec::new();
        for (name, ncfg) in &cfg.nanoleaf {
            match NanoleafClient::from_config(ncfg, cfg.timeout_sec) {
                Ok(c) => {
                    nanoleaf.push((name.clone(), c));
                    names.push(name.clone());
                }
                Err(e) => tracing::warn!("light '{name}' unavailable: {e:#}"),
            }
        }
        // Aliases that always resolve to the first device.
        if !nanoleaf.is_empty() {
            for alias in ["nanoleaf", "panels", "lights", "the lights"] {
                names.push(alias.to_string());
            }
        }
        Self { nanoleaf, names }
    }

    pub fn names(&self) -> Vec<&str> {
        self.names.iter().map(|s| s.as_str()).collect()
    }

    fn resolve(&self, device: &str) -> Option<&NanoleafClient> {
        let key = device.to_lowercase();
        self.nanoleaf
            .iter()
            .find(|(n, _)| n == &key)
            .map(|(_, c)| c)
            // Aliases ("panels", "the lights") -> first device
            .or_else(|| self.nanoleaf.first().map(|(_, c)| c))
    }

    /// Execute a parsed command; returns the spoken reply.
    pub async fn execute(&self, cmd: &LightCommand) -> anyhow::Result<String> {
        match cmd {
            LightCommand::TurnOn { device } => {
                let c = self.resolve(device).context("unknown light")?;
                c.power(true).await?;
                Ok(format!("{device} on."))
            }
            LightCommand::TurnOff { device } => {
                let c = self.resolve(device).context("unknown light")?;
                c.power(false).await?;
                Ok(format!("{device} off."))
            }
            LightCommand::SetBrightness { device, percent } => {
                let c = self.resolve(device).context("unknown light")?;
                c.brightness(*percent).await?;
                Ok(format!("{device} at {percent} percent."))
            }
            LightCommand::SetEffect { device, effect } => {
                let c = self.resolve(device).context("unknown light")?;
                // Tolerate voice-ish names: exact match first, then
                // case-insensitive contains against the effects list.
                let available = c.effects().await.unwrap_or_default();
                let matched = available
                    .iter()
                    .find(|e| e.eq_ignore_ascii_case(effect))
                    .or_else(|| {
                        available
                            .iter()
                            .find(|e| e.to_lowercase().contains(&effect.to_lowercase()))
                    })
                    .cloned()
                    .unwrap_or_else(|| effect.clone());
                c.effect(&matched).await?;
                Ok(format!("{device} playing {matched}."))
            }
            LightCommand::ListEffects { device } => {
                let c = self.resolve(device).context("unknown light")?;
                let mut list = c.effects().await?;
                list.truncate(8); // spoken reply — keep it short
                if list.is_empty() {
                    Ok(format!("No effects found on {device}."))
                } else {
                    Ok(format!("{device} effects include: {}.", list.join(", ")))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMES: &[&str] = &["nanoleaf", "panels"];

    #[test]
    fn light_parsing() {
        assert_eq!(
            parse_command("turn off the nanoleaf", NAMES),
            Some(LightCommand::TurnOff { device: "nanoleaf".into() })
        );
        assert_eq!(
            parse_command("dim the panels to forty percent", NAMES),
            Some(LightCommand::SetBrightness { device: "panels".into(), percent: 40 })
        );
        assert_eq!(
            parse_command("nanoleaf effect northern lights", NAMES),
            Some(LightCommand::SetEffect { device: "nanoleaf".into(), effect: "northern lights".into() })
        );
        assert_eq!(
            parse_command("list nanoleaf effects", NAMES),
            Some(LightCommand::ListEffects { device: "nanoleaf".into() })
        );
        // Not a light command.
        assert_eq!(parse_command("what time is it", NAMES), None);
        assert_eq!(parse_command("turn off the tv", NAMES), None);
    }
}
