//! On-screen captions: while Five speaks, show the spoken text as a desktop
//! notification for exactly the duration of the speech.
//!
//! Linux: shells out to `gdbus` (glib, always present on Pop!_OS) to call
//! org.freedesktop.Notifications directly, falling back to `notify-send`.
//! Consecutive captions share a replace-id so they update in place instead
//! of stacking.
//!
//! Windows: not implemented yet — captions are skipped (the text is always
//! printed to the terminal anyway). A toast-notification backend can land
//! later without touching callers.

use std::time::Duration;

#[cfg(target_os = "linux")]
use std::process::{Command, Stdio};
#[cfg(target_os = "linux")]
use tracing::debug;
use tracing::warn;

/// Notification replace-id: every caption replaces the previous one.
#[cfg(target_os = "linux")]
const REPLACE_ID: u32 = 5005;

/// Captions longer than this get truncated with an ellipsis — a notification
/// is a glance, not a transcript. (The full text is always in the terminal.)
#[cfg(target_os = "linux")]
const MAX_CHARS: usize = 200;

/// Show `text` on screen for the duration of `speech` (plus a small tail so
/// the caption doesn't vanish the instant the last phoneme lands).
#[cfg(target_os = "linux")]
pub fn show(text: &str, speech: Duration) {
    let hold_ms = (speech.as_millis() as u64 + 800).max(3000);
    let body: String = match text.char_indices().nth(MAX_CHARS) {
        Some((idx, _)) => format!("{}…", &text[..idx]),
        None => text.to_string(),
    };

    if try_gdbus(&body, hold_ms) || try_notify_send(&body, hold_ms) {
        debug!(hold_ms, "caption shown");
    } else {
        warn!("no caption backend worked (tried gdbus, notify-send)");
    }
}

/// Windows caption stub: log once per call and move on.
#[cfg(target_os = "windows")]
pub fn show(_text: &str, _speech: Duration) {
    warn!("captions not supported on Windows yet — skipping");
}

/// org.freedesktop.Notifications.Notify via gdbus (no libnotify needed).
#[cfg(target_os = "linux")]
fn try_gdbus(body: &str, hold_ms: u64) -> bool {
    run(
        Command::new("gdbus")
            .arg("call")
            .arg("--session")
            .arg("--dest")
            .arg("org.freedesktop.Notifications")
            .arg("--object-path")
            .arg("/org/freedesktop/Notifications")
            .arg("--method")
            .arg("org.freedesktop.Notifications.Notify")
            .arg("Five") // app name
            .arg(REPLACE_ID.to_string())
            .arg("") // icon
            .arg("Five") // summary
            .arg(body)
            .arg("[]") // actions
            .arg("{'transient': <true>}") // hints: don't log to history
            .arg(hold_ms.to_string()),
    )
}

/// libnotify's notify-send, if installed.
#[cfg(target_os = "linux")]
fn try_notify_send(body: &str, hold_ms: u64) -> bool {
    run(
        Command::new("notify-send")
            .arg("--app-name=Five")
            .arg("--urgency=low")
            .arg(format!("--expire-time={hold_ms}"))
            .arg(format!("--replace-id={REPLACE_ID}"))
            .arg("--hint=int:transient:1")
            .arg("Five")
            .arg(body),
    )
}

#[cfg(target_os = "linux")]
fn run(cmd: &mut Command) -> bool {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
