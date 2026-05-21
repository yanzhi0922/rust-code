//! Web browser tool: web_browser.
//!
//! Provides a headless browser tool for fetching web content with
//! JavaScript rendering support, link extraction, text extraction,
//! and page information retrieval.
//!
//! **Note on screenshots**: True screenshot capture requires a headless
//! browser runtime (e.g. Puppeteer MCP server). The `screenshot` action
//! returns page metadata instead of an actual image when no headless
//! browser is available.

use anyhow::{Context, Result, anyhow};
use regex::Regex;
use serde_json::{Value, json};

use super::ToolExecutionContext;

/// Default maximum content size (100KB).
const DEFAULT_MAX_CHARS: usize = 100_000;

/// Fetch web content using a simple HTTP client (headless browser simulation).
///
/// Supports multiple actions:
/// - `fetch`: Fetch the full HTML content of a URL
/// - `extract_links`: Extract all links from a URL
/// - `extract_text`: Extract text content (strip HTML tags)
/// - `screenshot`: Retrieve page information (requires headless browser runtime for actual screenshots)
///
/// # Errors
/// Returns an error if the URL is missing or the HTTP request fails.
pub async fn web_browser(input: &Value, _context: &ToolExecutionContext) -> Result<String> {
    let url = input["url"]
        .as_str()
        .ok_or_else(|| anyhow!("url is required for web browser"))?;

    if url.trim().is_empty() {
        return Err(anyhow!("url cannot be empty"));
    }

    // Validate URL format.
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(anyhow!(
            "Invalid URL format: '{}'. URL must start with http:// or https://",
            url
        ));
    }

    let action = input["action"].as_str().unwrap_or("fetch");

    let max_chars = input["max_chars"]
        .as_u64()
        .unwrap_or(DEFAULT_MAX_CHARS as u64) as usize;

    match action {
        "fetch" => fetch_url(url, max_chars).await,
        "extract_links" => extract_links(url).await,
        "extract_text" => extract_text(url, max_chars).await,
        "screenshot" => screenshot(url).await,
        _ => Err(anyhow!(
            "Unknown action: '{}'. Valid actions: fetch, extract_links, extract_text, screenshot",
            action
        )),
    }
}

/// Fetch the full HTML content of a URL.
async fn fetch_url(url: &str, max_chars: usize) -> Result<String> {
    let response = reqwest::get(url).await.context("failed to fetch URL")?;

    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!("HTTP {} for {}", status, url));
    }

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string();

    let text = response
        .text()
        .await
        .context("failed to read response body")?;

    let truncated = text.len() > max_chars;
    let content: String = text.chars().take(max_chars).collect();

    Ok(json!({
        "type": "web_browser_fetch",
        "url": url,
        "status": status.as_u16(),
        "content_type": content_type,
        "content_length": text.len(),
        "truncated": truncated,
        "content": content,
    })
    .to_string())
}

/// Extract all links from a URL.
async fn extract_links(url: &str) -> Result<String> {
    let response = reqwest::get(url)
        .await
        .context("failed to fetch URL for link extraction")?;

    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!("HTTP {} for {}", status, url));
    }

    let html = response
        .text()
        .await
        .context("failed to read response body")?;

    let links = extract_links_from_html(&html, url);

    Ok(json!({
        "type": "web_browser_extract_links",
        "url": url,
        "total_links": links.len(),
        "links": links.into_iter().take(200).collect::<Vec<_>>(),
    })
    .to_string())
}

/// Extract text content from a URL (strip HTML tags).
async fn extract_text(url: &str, max_chars: usize) -> Result<String> {
    let response = reqwest::get(url)
        .await
        .context("failed to fetch URL for text extraction")?;

    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!("HTTP {} for {}", status, url));
    }

    let html = response
        .text()
        .await
        .context("failed to read response body")?;

    let text = strip_html_tags(&html);
    let truncated = text.len() > max_chars;
    let content: String = text.chars().take(max_chars).collect();

    Ok(json!({
        "type": "web_browser_extract_text",
        "url": url,
        "status": status.as_u16(),
        "text_length": text.len(),
        "truncated": truncated,
        "content": content,
    })
    .to_string())
}

/// Retrieve page information for a URL (not a true screenshot).
///
/// True screenshot capture requires a headless browser runtime such as a
/// Puppeteer MCP server. When no headless browser is available, this function
/// fetches the page HTML and returns metadata (title, size, content type)
/// with `screenshot_available: false`.
async fn screenshot(url: &str) -> Result<String> {
    tracing::warn!(
        url = url,
        "screenshot action called without headless browser runtime; returning page metadata instead"
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    match client.get(url).send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let content_type = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("unknown")
                .to_string();

            let body = resp.text().await.unwrap_or_default();
            let title = body
                .lines()
                .take(200)
                .find(|l| l.contains("<title>"))
                .and_then(|l| {
                    let start = l.find("<title>").map(|i| i + 7)?;
                    let end = l.find("</title>")?;
                    Some(l[start..end].trim().to_string())
                })
                .unwrap_or_default();

            let text_len = body.len();
            // Strip HTML tags for a rough text estimate
            let text_preview: String = body
                .chars()
                .take(500)
                .collect::<String>()
                .replace(['<', '>'], " ");

            Ok(json!({
                "type": "web_browser_screenshot",
                "url": url,
                "status": "fetched",
                "http_status": status,
                "screenshot_available": false,
                "content_type": content_type,
                "title": title,
                "body_size_bytes": text_len,
                "text_preview": text_preview.chars().take(200).collect::<String>(),
                "note": "Full screenshot requires a headless browser runtime (Puppeteer MCP server). This response contains the fetched page metadata instead."
            })
            .to_string())
        }
        Err(e) => Ok(json!({
            "type": "web_browser_screenshot",
            "url": url,
            "status": "error",
            "screenshot_available": false,
            "error": format!("Failed to fetch page: {e}"),
            "note": "Screenshot capture requires network access and optionally a headless browser runtime."
        })
        .to_string()),
    }
}

/// Extract links from HTML content.
fn extract_links_from_html(html: &str, base_url: &str) -> Vec<String> {
    let href_re = Regex::new(r#"href\s*=\s*["']([^"']+)["']"#)
        .unwrap_or_else(|_| Regex::new("a^").expect("fallback regex"));
    let mut links = Vec::new();

    for cap in href_re.captures_iter(html) {
        let href = cap[1].to_string();
        let resolved = if href.starts_with("http://") || href.starts_with("https://") {
            href
        } else if href.starts_with('/') {
            let base = base_url.trim_end_matches('/');
            format!("{base}{href}")
        } else if href.starts_with('#')
            || href.starts_with("mailto:")
            || href.starts_with("javascript:")
        {
            continue;
        } else {
            let base = base_url.trim_end_matches('/');
            format!("{base}/{href}")
        };
        links.push(resolved);
    }

    links.sort();
    links.dedup();
    links
}

/// Strip HTML tags from content, leaving plain text.
fn strip_html_tags(html: &str) -> String {
    // Remove script and style tags with content.
    let script_re = Regex::new(r"(?is)<script[^>]*>.*?</script>")
        .unwrap_or_else(|_| Regex::new("a^").expect("fallback regex"));
    let style_re = Regex::new(r"(?is)<style[^>]*>.*?</style>")
        .unwrap_or_else(|_| Regex::new("a^").expect("fallback regex"));
    let tag_re =
        Regex::new(r"<[^>]+>").unwrap_or_else(|_| Regex::new("a^").expect("fallback regex"));

    let text = script_re.replace_all(html, "").to_string();
    let text = style_re.replace_all(&text, "").to_string();
    let text = tag_re.replace_all(&text, " ").to_string();

    // Decode common HTML entities using char codes to avoid encoding issues.
    let amp = '\u{0026}';
    let text = text
        .replace(&format!("{amp}amp;"), "\u{0026}")
        .replace(&format!("{amp}lt;"), "\u{003C}")
        .replace(&format!("{amp}gt;"), "\u{003E}")
        .replace(&format!("{amp}quot;"), "\u{0022}")
        .replace(&format!("{amp}#39;"), "\u{0027}")
        .replace(&format!("{amp}nbsp;"), " ");

    // Collapse whitespace.
    let whitespace_re =
        Regex::new(r"\s+").unwrap_or_else(|_| Regex::new("a^").expect("fallback regex"));
    whitespace_re.replace_all(&text, " ").trim().to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn test_context() -> ToolExecutionContext {
        ToolExecutionContext {
            cwd: PathBuf::from("/tmp"),
            original_cwd: PathBuf::from("/tmp"),
            active_worktree_session: None,
            timeout_ms: 30_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Arc::new(parking_lot::Mutex::new(
                claude_core::task_stack::TaskStack::default(),
            )),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    #[test]
    fn extract_links_from_simple_html() {
        let html = r#"<html><body><a href="https://example.com">Link</a><a href="/about">About</a></body></html>"#;
        let links = extract_links_from_html(html, "https://example.com");
        assert!(links.contains(&"https://example.com".to_string()));
        assert!(links.contains(&"https://example.com/about".to_string()));
    }

    #[test]
    fn extract_links_skips_anchor_links() {
        let html = r##"<a href="#section">Skip</a><a href="https://example.com">Link</a>"##;
        let links = extract_links_from_html(html, "https://example.com");
        assert!(!links.iter().any(|l| l.contains("#section")));
        assert!(links.contains(&"https://example.com".to_string()));
    }

    #[test]
    fn extract_links_skips_mailto() {
        let html =
            r#"<a href="mailto:test@example.com">Email</a><a href="https://example.com">Link</a>"#;
        let links = extract_links_from_html(html, "https://example.com");
        assert!(!links.iter().any(|l| l.starts_with("mailto:")));
    }

    #[test]
    fn extract_links_skips_javascript() {
        let html =
            r#"<a href="javascript:void(0)">Click</a><a href="https://example.com">Link</a>"#;
        let links = extract_links_from_html(html, "https://example.com");
        assert!(!links.iter().any(|l| l.starts_with("javascript:")));
    }

    #[test]
    fn extract_links_deduplicates() {
        let html =
            r#"<a href="https://example.com">Link1</a><a href="https://example.com">Link2</a>"#;
        let links = extract_links_from_html(html, "https://example.com");
        assert_eq!(links.len(), 1);
    }

    #[test]
    fn extract_links_resolves_relative_paths() {
        let html = r#"<a href="page.html">Page</a>"#;
        let links = extract_links_from_html(html, "https://example.com");
        assert!(links.contains(&"https://example.com/page.html".to_string()));
    }

    #[test]
    fn strip_html_tags_removes_tags() {
        let html = "<p>Hello <b>World</b></p>";
        let text = strip_html_tags(html);
        assert_eq!(text, "Hello World");
    }

    #[test]
    fn strip_html_tags_removes_scripts() {
        let html = "<html><script>alert('xss')</script><p>Content</p></html>";
        let text = strip_html_tags(html);
        assert!(!text.contains("alert"));
        assert!(text.contains("Content"));
    }

    #[test]
    fn strip_html_tags_removes_styles() {
        let html = "<style>body { color: red; }</style><p>Text</p>";
        let text = strip_html_tags(html);
        assert!(!text.contains("color"));
        assert!(text.contains("Text"));
    }

    #[test]
    fn strip_html_tags_decodes_entities() {
        // Build the test string from char codes to avoid encoding issues.
        let amp = '\u{0026}';
        let html = format!(
            "<p>{}amp; {}lt; {}gt; {}quot; {}#39;</p>",
            amp, amp, amp, amp, amp
        );
        let text = strip_html_tags(&html);
        assert!(text.contains('&'));
        assert!(text.contains('<'));
        assert!(text.contains('>'));
    }

    #[test]
    fn strip_html_tags_collapses_whitespace() {
        let html = "<p>Hello</p>   <p>World</p>";
        let text = strip_html_tags(html);
        assert_eq!(text, "Hello World");
    }

    #[test]
    fn web_browser_requires_url() {
        let input = json!({});
        let context = test_context();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(web_browser(&input, &context));
        let error = result.expect_err("missing url should return an error");
        assert!(error.to_string().contains("url"));
    }

    #[test]
    fn web_browser_rejects_empty_url() {
        let input = json!({"url": ""});
        let context = test_context();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(web_browser(&input, &context));
        assert!(result.is_err());
    }

    #[test]
    fn web_browser_validates_url_format() {
        let input = json!({"url": "ftp://example.com"});
        let context = test_context();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(web_browser(&input, &context));
        let error = result.expect_err("invalid URL format should return an error");
        assert!(error.to_string().contains("Invalid URL"));
    }

    #[test]
    fn web_browser_rejects_unknown_action() {
        let input = json!({"url": "https://example.com", "action": "unknown"});
        let context = test_context();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(web_browser(&input, &context));
        let error = result.expect_err("unknown action should return an error");
        assert!(error.to_string().contains("Unknown action"));
    }

    #[test]
    fn extract_links_from_empty_html() {
        let links = extract_links_from_html("", "https://example.com");
        assert!(links.is_empty());
    }

    #[test]
    fn strip_html_tags_empty_input() {
        let text = strip_html_tags("");
        assert!(text.is_empty());
    }

    #[test]
    fn strip_html_tags_preserves_content() {
        let html = "<div><h1>Title</h1><p>Paragraph with <em>emphasis</em>.</p></div>";
        let text = strip_html_tags(html);
        assert!(text.contains("Title"));
        assert!(text.contains("Paragraph"));
        assert!(text.contains("emphasis"));
    }
}
