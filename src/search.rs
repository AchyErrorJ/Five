//! Web search with site whitelisting.
//!
//! Uses DuckDuckGo's HTML search (no API key needed) or falls back to
//! Bing/Brave if an API key is configured. Results are filtered to only
//! allowed domains before fetching page content.

use anyhow::Context;
use serde::Deserialize;
use std::time::Duration;
use tracing::{debug, info, warn};

/// One search result before filtering.
#[derive(Debug, Clone)]
pub struct RawResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// One search result after filtering and content fetch.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    /// First ~2k chars of page content, if fetch succeeded.
    pub content: Option<String>,
}

/// Search configuration: which sites are allowed, how deep to go.
#[derive(Debug, Clone, Default)]
pub struct SearchConfig {
    /// Domains allowed for search results, e.g. ["wikipedia.org", "github.com"].
    /// Empty = allow all (not recommended).
    pub allowed_sites: Vec<String>,
    /// Max results to fetch content from (after filtering).
    pub max_results: usize,
    /// Request timeout for search + fetch.
    pub timeout_sec: u64,
}

pub struct Searcher {
    http: reqwest::Client,
    cfg: SearchConfig,
}

impl Searcher {
    pub fn new(cfg: SearchConfig) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(cfg.timeout_sec.max(5)))
            .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.0")
            .build()
            .context("build search HTTP client")?;
        Ok(Self { http, cfg })
    }

    /// Search DuckDuckGo HTML, filter to allowed sites, fetch content.
    pub async fn search(&self, query: &str) -> anyhow::Result<Vec<SearchResult>> {
        info!(%query, "web search");

        let raw = self.duckduckgo_html(query).await?;
        debug!(count = raw.len(), "raw results from DDG");

        let filtered: Vec<RawResult> = if self.cfg.allowed_sites.is_empty() {
            raw
        } else {
            raw.into_iter()
                .filter(|r| self.is_allowed(&r.url))
                .collect()
        };

        debug!(count = filtered.len(), "after site filter");

        let mut out = Vec::new();
        for r in filtered.into_iter().take(self.cfg.max_results.max(1)) {
            let content = self.fetch_text(&r.url).await.ok();
            if content.is_none() {
                warn!(url = %r.url, "failed to fetch page content");
            }
            out.push(SearchResult {
                title: r.title,
                url: r.url,
                snippet: r.snippet,
                content,
            });
        }

        Ok(out)
    }

    fn is_allowed(&self, url: &str) -> bool {
        let Ok(parsed) = url::Url::parse(url) else {
            return false;
        };
        let Some(host) = parsed.host_str() else {
            return false;
        };
        let host_lower = host.to_lowercase();
        for allowed in &self.cfg.allowed_sites {
            let a = allowed.to_lowercase().trim_start_matches("www.").to_string();
            let h = host_lower.trim_start_matches("www.");
            if h == a || h.ends_with(&format!(".{}" , a)) {
                return true;
            }
        }
        false
    }

    /// DuckDuckGo HTML search — scrapes result titles/links/snippets.
    /// No API key needed; fragile to DDG layout changes.
    async fn duckduckgo_html(&self, query: &str) -> anyhow::Result<Vec<RawResult>> {
        let url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            urlencoding::encode(query)
        );
        let resp = self.http.get(&url).send().await.context("DDG request")?;
        let body = resp.text().await.context("DDG body")?;

        // Parse the classic DDG HTML: each result is in a `.result` div.
        let document = scraper::Html::parse_document(&body);
        let result_sel = scraper::Selector::parse(".result").unwrap();
        let title_sel = scraper::Selector::parse(".result__a").unwrap();
        let snippet_sel = scraper::Selector::parse(".result__snippet").unwrap();

        let mut results = Vec::new();
        for elem in document.select(&result_sel) {
            let title = elem
                .select(&title_sel)
                .next()
                .map(|e| e.text().collect::<String>().trim().to_string())
                .unwrap_or_default();
            let url = elem
                .select(&title_sel)
                .next()
                .and_then(|e| e.value().attr("href"))
                .map(|h| self.resolve_ddg_link(h).unwrap_or_else(|| h.to_string()))
                .unwrap_or_default();
            let snippet = elem
                .select(&snippet_sel)
                .next()
                .map(|e| e.text().collect::<String>().trim().to_string())
                .unwrap_or_default();

            if !url.is_empty() && !title.is_empty() {
                results.push(RawResult { title, url, snippet });
            }
        }

        Ok(results)
    }

    /// DDG HTML uses `/l/?kh=-1&uddg=URL` redirect links; decode them.
    fn resolve_ddg_link(&self, href: &str) -> Option<String> {
        if let Some(pos) = href.find("uddg=") {
            let encoded = &href[pos + 5..];
            return urlencoding::decode(encoded).ok().map(|s| s.into_owned());
        }
        Some(href.to_string())
    }

    /// Fetch a page and extract readable text (best effort).
    async fn fetch_text(&self, url: &str) -> anyhow::Result<String> {
        let resp = self.http.get(url).send().await.context("fetch")?;
        let body = resp.text().await.context("fetch body")?;

        // Very light HTML-to-text: strip tags, collapse whitespace.
        let document = scraper::Html::parse_document(&body);
        let text = document
            .root_element()
            .text()
            .collect::<Vec<_>>()
            .join(" ");

        // Collapse whitespace and trim to ~2k chars.
        let cleaned: String = text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let truncated = if cleaned.len() > 2048 {
            format!("{}...", &cleaned[..2048])
        } else {
            cleaned
        };

        Ok(truncated)
    }
}

/// Summarize search results into a short spoken paragraph.
pub fn summarize(results: &[SearchResult]) -> String {
    if results.is_empty() {
        return "I searched but didn't find anything matching your sites.".to_string();
    }

    let mut parts = Vec::new();
    parts.push(format!("I found {} result{}.", results.len(), if results.len() == 1 { "" } else { "s" }));

    for (i, r) in results.iter().enumerate().take(3) {
        let mut line = format!("{}: {}", i + 1, r.title);
        if let Some(ref content) = r.content {
            // Pull first sentence or first 120 chars.
            let summary = content
                .split('.')
                .next()
                .unwrap_or(content)
                .trim();
            let clipped = if summary.len() > 120 {
                format!("{}...", &summary[..120])
            } else {
                summary.to_string()
            };
            line.push_str(&format!("。 {}" , clipped));
        } else if !r.snippet.is_empty() {
            let clipped = if r.snippet.len() > 120 {
                format!("{}...", &r.snippet[..120])
            } else {
                r.snippet.clone()
            };
            line.push_str(&format!(". {}", clipped));
        }
        parts.push(line);
    }

    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_filtering() {
        let cfg = SearchConfig {
            allowed_sites: vec!["wikipedia.org".into(), "github.com".into()],
            ..Default::default()
        };
        let s = Searcher::new(cfg).unwrap();
        assert!(s.is_allowed("https://en.wikipedia.org/wiki/Rust"));
        assert!(s.is_allowed("https://github.com/rust-lang/rust"));
        assert!(!s.is_allowed("https://example.com"));
        assert!(!s.is_allowed("not-a-url"));
    }

    #[test]
    fn summarize_empty() {
        let empty: Vec<SearchResult> = vec![];
        assert_eq!(
            summarize(&empty),
            "I searched but didn't find anything matching your sites."
        );
    }

    #[test]
    fn summarize_one() {
        let r = SearchResult {
            title: "Rust Programming Language".into(),
            url: "https://rust-lang.org".into(),
            snippet: "A language empowering everyone to build reliable software.".into(),
            content: Some("Rust is a systems programming language focused on safety.".into()),
        };
        let speech = summarize(&[r]);
        assert!(speech.contains("Rust Programming Language"));
        assert!(speech.contains("1 result"));
    }
}
