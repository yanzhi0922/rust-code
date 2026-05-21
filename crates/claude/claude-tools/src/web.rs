//! Web-related tools: web_fetch, web_search, web_browser.

use std::env;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use regex::Regex;
use serde_json::{Value, json};
use tokio::process::Command;
use tokio::time::{Duration, timeout};
use uuid::Uuid;

use super::ToolExecutionContext;

pub(crate) async fn web_fetch(input: &Value, _context: &ToolExecutionContext) -> Result<String> {
    let url = input
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("web_fetch requires a url"))?;

    // Validate URL format before making the request
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(anyhow!(
            "Invalid URL format: '{}'. URL must start with http:// or https://",
            url
        ));
    }

    let prompt = input
        .get("prompt")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("web_fetch requires a prompt"))?;

    // Build a client that follows up to 10 redirects (the same limit the
    // default client uses) so we can compare the final URL's host against
    // the original and report cross-host redirects to the caller.
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .context("failed to build HTTP client for web_fetch")?;

    // Apply a timeout to prevent hanging on unresponsive servers
    let response = timeout(Duration::from_secs(30), client.get(url).send())
        .await
        .map_err(|_| anyhow!("web_fetch timed out after 30 seconds for {}", url))?
        .context("failed to fetch URL")?;

    // Detect cross-host redirects and report them to the caller so they can
    // re-issue the request with the new URL, matching TS reference behaviour.
    let final_url = response.url().to_string();
    let original_host = reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_owned()));
    let final_host = response.url().host_str().map(|h| h.to_owned());

    if let (Some(orig), Some(final_h)) = (&original_host, &final_host)
        && !orig.eq_ignore_ascii_case(final_h)
    {
        return Ok(format!(
            "The URL redirected to a different host.\n\n\
                 Original URL: {url}\n\
                 Redirect URL: {final_url}\n\n\
                 Please make a new WebFetch request with the redirect URL to fetch the content.",
        ));
    }

    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!("HTTP {} for {}", status, url));
    }

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();

    let is_html = content_type.contains("text/html");
    let text = response
        .text()
        .await
        .context("failed to read response body")?;

    // Convert HTML to readable text, or use raw text for non-HTML
    let readable_content = if is_html {
        html_to_readable_text(&text)
    } else {
        text.clone()
    };

    // Truncate to prevent excessive context usage
    const MAX_CONTENT_LENGTH: usize = 100_000;
    let truncated = if readable_content.len() > MAX_CONTENT_LENGTH {
        format!(
            "{}\n\n[Content truncated due to length...]",
            &readable_content[..MAX_CONTENT_LENGTH]
        )
    } else {
        readable_content
    };

    // Build a result that frames the prompt for the calling model.
    // Since we don't have an embedded LLM, we return the content with the
    // prompt clearly stated so the caller (the model) can apply it.
    let result = format!(
        "<url>{url}</url>\n\
         <prompt>{prompt}</prompt>\n\n\
         The following is the content fetched from the URL above. \
         Use the prompt above to extract or summarize the relevant information from this content:\n\n\
         {truncated}",
    );

    Ok(result)
}

/// Convert HTML to readable plain text by stripping tags and normalizing whitespace.
fn html_to_readable_text(html: &str) -> String {
    // Remove script and style blocks entirely
    let re_script = Regex::new(r"(?is)<script[^>]*>.*?</script>")
        .unwrap_or_else(|_| Regex::new(".").expect("fallback regex"));
    let re_style = Regex::new(r"(?is)<style[^>]*>.*?</style>")
        .unwrap_or_else(|_| Regex::new(".").expect("fallback regex"));
    let no_scripts = re_script.replace_all(html, "");
    let no_style = re_style.replace_all(&no_scripts, "");

    // Convert block-level elements to newlines for structure
    let re_block = Regex::new(r"(?i)</?(p|div|br|h[1-6]|li|tr|hr|blockquote|pre|section|article|header|footer|nav|aside|main|figure|figcaption|details|summary)[^>]*>")
        .unwrap_or_else(|_| Regex::new(".").expect("fallback regex"));
    let with_breaks = re_block.replace_all(&no_style, "\n");

    // Strip remaining tags
    let re_tag =
        Regex::new(r"<[^>]+>").unwrap_or_else(|_| Regex::new(".").expect("fallback regex"));
    let stripped = re_tag.replace_all(&with_breaks, "");

    // Decode common HTML entities
    let decoded = stripped
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("&mdash;", "—")
        .replace("&ndash;", "–")
        .replace("&hellip;", "…");

    // Collapse excessive whitespace while preserving structure
    let lines: Vec<&str> = decoded
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    lines.join("\n")
}

pub(crate) async fn web_search(input: &Value, _context: &ToolExecutionContext) -> Result<String> {
    let query = input
        .get("query")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("web_search requires a query"))?;
    let allowed_domains: Vec<String> = input
        .get("allowed_domains")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(|s| s.to_owned())
                .collect()
        })
        .unwrap_or_default();
    let blocked_domains: Vec<String> = input
        .get("blocked_domains")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(|s| s.to_owned())
                .collect()
        })
        .unwrap_or_default();

    // Mutual exclusion: allowed_domains and blocked_domains cannot both be non-empty.
    if !allowed_domains.is_empty() && !blocked_domains.is_empty() {
        return Err(anyhow!(
            "allowed_domains and blocked_domains are mutually exclusive — \
             specify one or the other, not both"
        ));
    }

    // Try multiple search backends for better results.
    let search_backends = get_search_backends();

    let mut last_error: Option<String> = None;
    for backend in &search_backends {
        match backend.search(query, 10).await {
            Ok(Some(results)) if !results.is_empty() => {
                return Ok(filter_results_by_domains(
                    &results,
                    &allowed_domains,
                    &blocked_domains,
                ));
            }
            Ok(_) => continue,
            Err(e) => {
                last_error = Some(e.to_string());
                continue;
            }
        }
    }

    // All backends failed or returned no results.
    if let Some(error) = last_error {
        Ok(format!(
            "No search results found for '{}' (last error: {error}). Try a more specific query.",
            query
        ))
    } else {
        Ok(format!(
            "No search results found for '{}'. Try a more specific query.",
            query
        ))
    }
}

fn filter_results_by_domains(
    results: &str,
    allowed_domains: &[String],
    blocked_domains: &[String],
) -> String {
    if allowed_domains.is_empty() && blocked_domains.is_empty() {
        return results.to_owned();
    }
    let filtered_lines: Vec<String> = results
        .lines()
        .filter(|line| {
            let line_lower = line.to_ascii_lowercase();
            let matches_allowed = if allowed_domains.is_empty() {
                true
            } else {
                allowed_domains
                    .iter()
                    .any(|domain| line_lower.contains(&domain.to_ascii_lowercase()))
            };
            let matches_blocked = blocked_domains
                .iter()
                .any(|domain| line_lower.contains(&domain.to_ascii_lowercase()));
            matches_allowed && !matches_blocked
        })
        .map(|s| s.to_owned())
        .collect();
    if filtered_lines.is_empty() {
        "No results found after domain filtering.".to_owned()
    } else {
        filtered_lines.join("\n")
    }
}

/// Get the ordered list of search backends to try.
fn get_search_backends() -> Vec<Box<dyn SearchBackend>> {
    // Check for custom search backend configuration via env var.
    let backend_env = std::env::var("REMOTE_CODE_SEARCH_BACKEND")
        .unwrap_or_default()
        .to_lowercase();

    if backend_env == "duckduckgo_html" {
        vec![Box::new(DuckDuckGoHtmlBackend)]
    } else if backend_env == "duckduckgo_api" {
        vec![Box::new(DuckDuckGoApiBackend)]
    } else {
        // Default: try HTML scraping first (better results), fall back to API.
        vec![
            Box::new(DuckDuckGoHtmlBackend),
            Box::new(DuckDuckGoApiBackend),
        ]
    }
}

// ---------------------------------------------------------------------------
// Search backend trait and implementations
// ---------------------------------------------------------------------------

/// A search backend that can execute web searches.
trait SearchBackend: Send + Sync {
    /// Execute a search and return formatted results.
    fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<String>>> + Send>>;
}

/// DuckDuckGo HTML search backend — scrapes the lite version for better results.
struct DuckDuckGoHtmlBackend;

impl SearchBackend for DuckDuckGoHtmlBackend {
    fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<String>>> + Send>> {
        let query = query.to_owned();
        Box::pin(async move {
            let url = format!(
                "https://lite.duckduckgo.com/lite/?q={}",
                urlencoding::encode(&query)
            );

            let client = reqwest::Client::builder()
                .user_agent(
                    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
                     AppleWebKit/537.36 (KHTML, like Gecko) \
                     Chrome/120.0.0.0 Safari/537.36",
                )
                .build()
                .context("failed to build HTTP client for DuckDuckGo HTML search")?;

            let response = client
                .get(&url)
                .timeout(Duration::from_secs(15))
                .send()
                .await
                .context("failed to query DuckDuckGo HTML search")?;

            let status = response.status();
            if !status.is_success() {
                return Err(anyhow!("DuckDuckGo HTML search returned HTTP {status}"));
            }

            let html = response
                .text()
                .await
                .context("failed to read DuckDuckGo HTML response")?;

            // Parse results from the HTML.
            let results = parse_ddg_html_results(&html, max_results);

            if results.is_empty() {
                Ok(None)
            } else {
                Ok(Some(format!(
                    "Search results for '{}':\n{}",
                    query,
                    results.join("\n")
                )))
            }
        })
    }
}

/// Parse DuckDuckGo Lite HTML search results.
fn parse_ddg_html_results(html: &str, max_results: usize) -> Vec<String> {
    let mut results = Vec::new();
    let link_re = Regex::new(r#"<a[^>]*class="result-link"[^>]*>(.*?)</a>"#).unwrap_or_else(|_| {
        Regex::new(r#"<a[^>]*>(.*?)</a>"#).expect("fallback link regex is valid")
    });
    let snippet_re = Regex::new(r#"<td[^>]*class="result-snippet"[^>]*>(.*?)</td>"#)
        .unwrap_or_else(|_| {
            Regex::new(r#"<td[^>]*>(.*?)</td>"#).expect("fallback snippet regex is valid")
        });
    let tag_re = Regex::new(r"<[^>]+>").expect("tag stripping regex is valid");

    // Try to find result links and snippets in the DDG Lite HTML.
    let link_captures: Vec<String> = link_re
        .captures_iter(html)
        .filter_map(|cap| {
            cap.get(1)
                .map(|m| tag_re.replace_all(m.as_str(), "").to_string())
        })
        .filter(|s| !s.trim().is_empty())
        .take(max_results)
        .collect();

    let snippet_captures: Vec<String> = snippet_re
        .captures_iter(html)
        .filter_map(|cap| {
            cap.get(1)
                .map(|m| tag_re.replace_all(m.as_str(), "").to_string())
        })
        .filter(|s| !s.trim().is_empty())
        .take(max_results)
        .collect();

    for (i, title) in link_captures.iter().enumerate() {
        let snippet = snippet_captures.get(i).map(|s| s.as_str()).unwrap_or("");
        let entry = if snippet.is_empty() {
            format!("{}. {title}", i + 1)
        } else {
            format!("{}. {title}\n   {snippet}", i + 1)
        };
        results.push(entry);
    }

    // Fallback: try to extract any meaningful text blocks from the HTML.
    if results.is_empty() {
        // Try a simpler approach: extract text between <tr> blocks.
        let row_re = Regex::new(r"<tr[^>]*>(.*?)</tr>")
            .unwrap_or_else(|_| Regex::new(".").expect("fallback row regex is valid"));
        let mut count = 0;
        for cap in row_re.captures_iter(html) {
            let row_text = tag_re.replace_all(&cap[1], " ");
            let cleaned = row_text
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .trim()
                .to_string();
            if cleaned.len() > 20 && !cleaned.contains("DuckDuckGo") && !cleaned.contains("cookie")
            {
                results.push(cleaned);
                count += 1;
                if count >= max_results {
                    break;
                }
            }
        }
    }

    results
}

/// DuckDuckGo Instant Answer API backend — returns instant answers and related topics.
struct DuckDuckGoApiBackend;

impl SearchBackend for DuckDuckGoApiBackend {
    fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<String>>> + Send>> {
        let query = query.to_owned();
        Box::pin(async move {
            let url = format!(
                "https://api.duckduckgo.com/?q={}&format=json&no_html=1",
                urlencoding::encode(&query)
            );

            let response = reqwest::get(&url)
                .await
                .context("failed to query DuckDuckGo API")?;

            let body = response
                .text()
                .await
                .context("failed to read DuckDuckGo API response")?;

            let parsed: Value = serde_json::from_str(&body).unwrap_or_default();

            // Extract the abstract text (instant answer summary).
            let abstract_text = parsed["AbstractText"].as_str().unwrap_or("");
            let abstract_source = parsed["AbstractSource"].as_str().unwrap_or("");
            let abstract_url = parsed["AbstractURL"].as_str().unwrap_or("");

            if !abstract_text.is_empty() {
                let source_info = if abstract_source.is_empty() {
                    String::new()
                } else {
                    let url_info = if abstract_url.is_empty() {
                        String::new()
                    } else {
                        format!(" ({abstract_url})")
                    };
                    format!(" (source: {abstract_source}{url_info})")
                };
                return Ok(Some(format!(
                    "Search results for '{}':\n{abstract_text}{source_info}",
                    query
                )));
            }

            // Try to extract related topics.
            let related: Vec<String> = parsed
                .get("RelatedTopics")
                .and_then(Value::as_array)
                .map(|topics| {
                    topics
                        .iter()
                        .take(max_results)
                        .filter_map(|topic| {
                            // Handle both simple topics and nested topics.
                            if let Some(text) = topic.get("Text").and_then(Value::as_str) {
                                let url =
                                    topic.get("FirstURL").and_then(Value::as_str).unwrap_or("");
                                if url.is_empty() {
                                    Some(text.to_owned())
                                } else {
                                    Some(format!("{text} ({url})"))
                                }
                            } else if let Some(nested) =
                                topic.get("Topics").and_then(Value::as_array)
                            {
                                // Nested topics (category groups).
                                nested
                                    .iter()
                                    .filter_map(|t| {
                                        let text = t.get("Text").and_then(Value::as_str)?;
                                        let url =
                                            t.get("FirstURL").and_then(Value::as_str).unwrap_or("");
                                        Some(if url.is_empty() {
                                            text.to_owned()
                                        } else {
                                            format!("{text} ({url})")
                                        })
                                    })
                                    .next()
                            } else {
                                None
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();

            if related.is_empty() {
                Ok(None)
            } else {
                Ok(Some(format!(
                    "Related topics for '{}':\n{}",
                    query,
                    related.join("\n")
                )))
            }
        })
    }
}

pub(crate) async fn web_browser_tool(
    input: &Value,
    _context: &ToolExecutionContext,
) -> Result<String> {
    let url = input
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("url is required"))?;
    let action = input
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("fetch");
    match action {
        "fetch" => {
            let response = reqwest::get(url).await.context("failed to fetch URL")?;
            let status = response.status();
            if !status.is_success() {
                return Err(anyhow!("HTTP {} for {}", status, url));
            }
            let text = response
                .text()
                .await
                .context("failed to read response body")?;
            // Truncate to 50K chars
            let truncated: String = text.chars().take(50_000).collect();
            Ok(truncated)
        }
        "screenshot" => capture_visual_screenshot(url, _context).await,
        "extract_links" => {
            let response = reqwest::get(url)
                .await
                .context("failed to fetch URL for link extraction")?;
            let status = response.status();
            if !status.is_success() {
                return Err(anyhow!("HTTP {} for {}", status, url));
            }
            let text = response
                .text()
                .await
                .context("failed to read response body")?;
            let re = Regex::new(r#"href\s*=\s*"([^"]+)""#).expect("valid href regex");
            let links: Vec<String> = re
                .captures_iter(&text)
                .filter_map(|cap| cap.get(1).map(|m| m.as_str().to_owned()))
                .take(200)
                .collect();
            Ok(json!({
                "url": url,
                "links": links,
                "count": links.len(),
            })
            .to_string())
        }
        "extract_text" => {
            let response = reqwest::get(url)
                .await
                .context("failed to fetch URL for text extraction")?;
            let status = response.status();
            if !status.is_success() {
                return Err(anyhow!("HTTP {} for {}", status, url));
            }
            let text = response
                .text()
                .await
                .context("failed to read response body")?;
            // Strip HTML tags for a plain-text approximation.
            let re = Regex::new(r"<[^>]+>").expect("valid html-stripping regex");
            let plain = re.replace_all(&text, " ");
            // Collapse whitespace.
            let collapsed: String = plain.split_whitespace().collect::<Vec<_>>().join(" ");
            let truncated: String = collapsed.chars().take(50_000).collect();
            Ok(truncated)
        }
        _ => Err(anyhow!(
            "action must be 'fetch', 'extract_links', 'extract_text', or 'screenshot'"
        )),
    }
}

async fn capture_visual_screenshot(url: &str, context: &ToolExecutionContext) -> Result<String> {
    let browser = detect_headless_browser().await.ok_or_else(|| {
        anyhow!("no compatible Chromium-based browser found for screenshot capture")
    })?;

    let screenshots_dir = env::temp_dir()
        .join("remote-code-rust")
        .join("web-screenshots");
    fs::create_dir_all(&screenshots_dir).context("failed to create screenshot directory")?;

    let browser_profile_dir = env::temp_dir()
        .join("remote-code-rust")
        .join("web-browser-profiles")
        .join(Uuid::new_v4().to_string());
    fs::create_dir_all(&browser_profile_dir)
        .context("failed to create browser profile directory")?;

    let screenshot_path = screenshots_dir.join(format!("{}.png", Uuid::new_v4()));
    let timeout_secs = (context.timeout_ms / 1000).clamp(5, 60);

    let mut last_error: Option<String> = None;
    for headless_flag in ["--headless=new", "--headless"] {
        let mut command = Command::new(&browser);
        command.args([
            headless_flag,
            "--disable-gpu",
            "--hide-scrollbars",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-background-networking",
            "--window-size=1440,1024",
        ]);
        command.arg(format!(
            "--user-data-dir={}",
            browser_profile_dir.to_string_lossy()
        ));
        command.arg(format!(
            "--screenshot={}",
            screenshot_path.to_string_lossy()
        ));
        command.arg(url);

        let output = timeout(Duration::from_secs(timeout_secs), command.output())
            .await
            .with_context(|| {
                format!(
                    "timed out after {timeout_secs}s while launching {}",
                    browser.display()
                )
            })?
            .with_context(|| format!("failed to launch browser at {}", browser.display()))?;

        if output.status.success() && screenshot_path.exists() {
            let size_bytes = fs::metadata(&screenshot_path)
                .context("failed to read screenshot metadata")?
                .len();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            let _ = fs::remove_dir_all(&browser_profile_dir);
            return Ok(json!({
                "type": "web_browser_screenshot",
                "url": url,
                "path": screenshot_path.to_string_lossy(),
                "mime_type": "image/png",
                "size_bytes": size_bytes,
                "browser": browser.to_string_lossy(),
                "stderr": if stderr.is_empty() { Value::Null } else { Value::String(stderr) },
            })
            .to_string());
        }

        last_error = Some(build_browser_failure(&output, &browser, headless_flag));
        let _ = fs::remove_file(&screenshot_path);
    }

    let _ = fs::remove_dir_all(&browser_profile_dir);
    Err(anyhow!(last_error.unwrap_or_else(|| {
        "browser exited without creating a screenshot".to_owned()
    })))
}

fn build_browser_failure(
    output: &std::process::Output,
    browser: &std::path::Path,
    headless_flag: &str,
) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let detail = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        "no browser output captured".to_owned()
    };
    format!(
        "failed to capture screenshot with {} {}: {}",
        browser.display(),
        headless_flag,
        detail
    )
}

async fn detect_headless_browser() -> Option<PathBuf> {
    if let Some(path) = browser_from_env() {
        return Some(path);
    }
    if let Some(path) = browser_from_path().await {
        return Some(path);
    }
    browser_from_known_locations()
}

fn browser_from_env() -> Option<PathBuf> {
    for key in ["REMOTE_CODE_BROWSER", "BROWSER"] {
        let candidate = PathBuf::from(env::var_os(key)?);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

async fn browser_from_path() -> Option<PathBuf> {
    #[cfg(windows)]
    let resolver = "where";
    #[cfg(not(windows))]
    let resolver = "which";

    for name in browser_binary_names() {
        let output = Command::new(resolver).arg(name).output().await.ok()?;
        if !output.status.success() {
            continue;
        }
        let candidate = String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        if let Some(path) = candidate
            && path.exists()
        {
            return Some(path);
        }
    }
    None
}

fn browser_from_known_locations() -> Option<PathBuf> {
    browser_known_locations()
        .into_iter()
        .find(|candidate| candidate.exists())
}

fn browser_binary_names() -> &'static [&'static str] {
    #[cfg(windows)]
    {
        &["msedge.exe", "chrome.exe"]
    }
    #[cfg(target_os = "macos")]
    {
        &[
            "Google Chrome",
            "Microsoft Edge",
            "Chromium",
            "google-chrome",
            "microsoft-edge",
            "chromium",
        ]
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        &[
            "google-chrome",
            "google-chrome-stable",
            "microsoft-edge",
            "chromium",
            "chromium-browser",
        ]
    }
}

fn browser_known_locations() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let mut candidates = Vec::new();
        if let Some(program_files_x86) = env::var_os("ProgramFiles(x86)") {
            candidates.push(
                PathBuf::from(&program_files_x86)
                    .join("Microsoft")
                    .join("Edge")
                    .join("Application")
                    .join("msedge.exe"),
            );
            candidates.push(
                PathBuf::from(&program_files_x86)
                    .join("Google")
                    .join("Chrome")
                    .join("Application")
                    .join("chrome.exe"),
            );
        }
        if let Some(program_files) = env::var_os("ProgramFiles") {
            candidates.push(
                PathBuf::from(&program_files)
                    .join("Microsoft")
                    .join("Edge")
                    .join("Application")
                    .join("msedge.exe"),
            );
            candidates.push(
                PathBuf::from(&program_files)
                    .join("Google")
                    .join("Chrome")
                    .join("Application")
                    .join("chrome.exe"),
            );
        }
        if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
            candidates.push(
                PathBuf::from(local_app_data)
                    .join("Google")
                    .join("Chrome")
                    .join("Application")
                    .join("chrome.exe"),
            );
        }
        candidates
    }
    #[cfg(target_os = "macos")]
    {
        vec![
            PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
            PathBuf::from("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"),
            PathBuf::from("/Applications/Chromium.app/Contents/MacOS/Chromium"),
        ]
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        vec![
            PathBuf::from("/usr/bin/google-chrome"),
            PathBuf::from("/usr/bin/google-chrome-stable"),
            PathBuf::from("/usr/bin/microsoft-edge"),
            PathBuf::from("/usr/bin/chromium"),
            PathBuf::from("/usr/bin/chromium-browser"),
        ]
    }
}
