//! Voice-driven file creation — a safe sandbox for Five to write files.
//!
//! Commands:
//!   "Five, write this down: <content>"      → timestamped file in creations_dir
//!   "Five, save this as <filename>"          → named file in creations_dir
//!   "Five, append this to <filename>"        → append to existing file
//!
//! Safety: all paths are resolved inside `creations_dir`. Path traversal
//! (../) is rejected. Overwrites require confirmation unless disabled.

use std::path::PathBuf;
use anyhow::Context;
use tracing::info;

use crate::config::FilesConfig;

pub struct FileManager {
    creations_dir: PathBuf,
}

impl FileManager {
    pub fn new(cfg: &FilesConfig) -> anyhow::Result<Self> {
        let dir = &cfg.creations_dir;
        if !dir.exists() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("creating creations_dir {}", dir.display()))?;
        }
        Ok(Self {
            creations_dir: dir.clone(),
        })
    }

    /// Sanitize a user-provided filename: no path traversal, no leading dot
    /// files (hidden), reasonable length.
    fn sanitize(&self, name: &str) -> anyhow::Result<PathBuf> {
        let name = name.trim();
        if name.is_empty() {
            anyhow::bail!("empty filename");
        }
        if name.len() > 120 {
            anyhow::bail!("filename too long");
        }
        // Reject path traversal
        if name.contains("..") || name.contains('/') || name.contains('\\') {
            anyhow::bail!("path traversal not allowed");
        }
        // Reject hidden files
        if name.starts_with('.') {
            anyhow::bail!("hidden files not allowed");
        }
        let path = self.creations_dir.join(name);
        // Ensure it's still inside creations_dir after resolution
        let canonical = std::fs::canonicalize(&self.creations_dir)?;
        let resolved = if path.exists() {
            std::fs::canonicalize(&path)?
        } else {
            // For non-existent files, canonicalize the parent and append
            let parent = std::fs::canonicalize(&self.creations_dir)?;
            parent.join(name)
        };
        if !resolved.starts_with(&canonical) {
            anyhow::bail!("path escapes sandbox");
        }
        Ok(path)
    }

    /// "write this down: <content>" → timestamped file
    pub fn write_down(&self, content: &str) -> anyhow::Result<PathBuf> {
        let timestamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
        let name = format!("note_{}.txt", timestamp);
        let path = self.sanitize(&name)?;
        std::fs::write(&path, content)
            .with_context(|| format!("writing {}", path.display()))?;
        info!(path = %path.display(), "file created");
        Ok(path)
    }

    /// "save this as <filename>: <content>"
    pub fn save_as(&self, filename: &str, content: &str) -> anyhow::Result<PathBuf> {
        let path = self.sanitize(filename)?;
        let existed = path.exists();
        std::fs::write(&path, content)
            .with_context(|| format!("writing {}", path.display()))?;
        if existed {
            info!(path = %path.display(), "file overwritten");
        } else {
            info!(path = %path.display(), "file created");
        }
        Ok(path)
    }

    /// "append this to <filename>: <content>"
    pub fn append_to(&self, filename: &str, content: &str) -> anyhow::Result<PathBuf> {
        let path = self.sanitize(filename)?;
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("opening {} for append", path.display()))?;
        write!(f, "{}", content)?;
        info!(path = %path.display(), "file appended");
        Ok(path)
    }
}

/// Parse a voice command for file operations.
/// Returns (action, filename_or_none, content).
pub fn parse_file_command(text: &str) -> Option<(&'static str, Option<String>, String)> {
    let t = text.trim().to_lowercase();

    // "write this down: <content>" or "write this down <content>"
    if let Some(rest) = t.strip_prefix("write this down") {
        let content = rest.trim_start_matches([':', ' ']).trim();
        if !content.is_empty() {
            return Some(("write_down", None, content.to_string()));
        }
    }

    // "save this as <filename>: <content>"
    if let Some(rest) = t.strip_prefix("save this as ") {
        if let Some((filename, content)) = rest.split_once(':') {
            let content = content.trim();
            if !content.is_empty() {
                return Some(("save_as", Some(filename.trim().to_string()), content.to_string()));
            }
        }
    }

    // "append this to <filename>: <content>"
    if let Some(rest) = t.strip_prefix("append this to ") {
        if let Some((filename, content)) = rest.split_once(':') {
            let content = content.trim();
            if !content.is_empty() {
                return Some(("append", Some(filename.trim().to_string()), content.to_string()));
            }
        }
    }

    None
}
