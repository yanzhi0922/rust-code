//! Marketplace helpers — utility functions for marketplace operations.
//!
//! Provides source resolution, display formatting, failure formatting,
//! and index parsing.

use serde::{Deserialize, Serialize};

use super::manager::MarketplaceIndex;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A marketplace source resolved from user input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "lowercase")]
pub enum ResolvedMarketplaceSource {
    /// GitHub repository.
    Github { repo: String },
    /// URL to marketplace index JSON.
    Url { url: String },
    /// Git repository URL.
    Git {
        url: String,
        #[serde(default)]
        ref_: Option<String>,
    },
    /// Local directory path.
    Directory { path: String },
    /// Local file path.
    File { path: String },
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// Resolve a marketplace source from a user-provided string.
///
/// Handles GitHub shorthand (`owner/repo`), URLs, and local paths.
pub fn resolve_marketplace_source(input: &str) -> Option<ResolvedMarketplaceSource> {
    let trimmed = input.trim();

    // Handle URLs
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        if trimmed.contains("github.com") {
            // Try to extract owner/repo
            if let Some(repo) = extract_github_repo(trimmed) {
                return Some(ResolvedMarketplaceSource::Github { repo });
            }
        }
        return Some(ResolvedMarketplaceSource::Url {
            url: trimmed.to_owned(),
        });
    }

    // Handle git@ SSH URLs
    if trimmed.starts_with("git@") {
        return Some(ResolvedMarketplaceSource::Git {
            url: trimmed.to_owned(),
            ref_: None,
        });
    }

    // Handle GitHub shorthand (owner/repo)
    if let Some(slash_pos) = trimmed.find('/') {
        let owner = &trimmed[..slash_pos];
        let repo = &trimmed[slash_pos + 1..];
        if !owner.is_empty() && !repo.is_empty() && !repo.contains('/') && !owner.contains('.') {
            return Some(ResolvedMarketplaceSource::Github {
                repo: trimmed.to_owned(),
            });
        }
    }

    // Handle local paths
    if trimmed.starts_with('.') || trimmed.starts_with('/') {
        if trimmed.ends_with(".json") {
            return Some(ResolvedMarketplaceSource::File {
                path: trimmed.to_owned(),
            });
        }
        return Some(ResolvedMarketplaceSource::Directory {
            path: trimmed.to_owned(),
        });
    }

    None
}

/// Extract a GitHub `owner/repo` from a URL.
fn extract_github_repo(url: &str) -> Option<String> {
    let re = regex::Regex::new(r"github\.com/([^/]+/[^/?#]+)").ok()?;
    let caps = re.captures(url)?;
    let repo = caps.get(1)?.as_str();
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    Some(repo.to_owned())
}

/// Format plugin failure details for user display.
pub fn format_failure_details(failures: &[FailureEntry], include_reasons: bool) -> String {
    let max_show = 2;
    let details: Vec<String> = failures
        .iter()
        .take(max_show)
        .map(|f| {
            if include_reasons {
                let reason = f.reason.as_deref().unwrap_or("unknown error");
                format!("{} ({})", f.name, reason)
            } else {
                f.name.clone()
            }
        })
        .collect();

    let details_str = if include_reasons {
        details.join("; ")
    } else {
        details.join(", ")
    };

    let remaining = failures.len().saturating_sub(max_show);
    if remaining > 0 {
        format!("{details_str} and {remaining} more")
    } else {
        details_str
    }
}

/// A failure entry for formatting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureEntry {
    /// Name of the failed item.
    pub name: String,
    /// Reason for failure.
    pub reason: Option<String>,
}

/// Get a display string for a marketplace source.
pub fn get_marketplace_source_display(source: &ResolvedMarketplaceSource) -> String {
    match source {
        ResolvedMarketplaceSource::Github { repo } => repo.clone(),
        ResolvedMarketplaceSource::Url { url } => url.clone(),
        ResolvedMarketplaceSource::Git { url, .. } => url.clone(),
        ResolvedMarketplaceSource::Directory { path } => path.clone(),
        ResolvedMarketplaceSource::File { path } => path.clone(),
    }
}

/// Create a plugin ID from plugin name and marketplace name.
pub fn create_plugin_id(plugin_name: &str, marketplace_name: &str) -> String {
    format!("{plugin_name}@{marketplace_name}")
}

/// Fetch a marketplace index from a URL.
///
/// In a real implementation, this would make an HTTP request.
/// Here it returns an error to indicate the operation is not available.
pub fn fetch_marketplace_index(_url: &str) -> Result<MarketplaceIndex, String> {
    Err("network fetch not available in this build".to_owned())
}

/// Parse a marketplace index from JSON content.
pub fn parse_marketplace_index(
    content: &str,
    marketplace_name: &str,
) -> Result<MarketplaceIndex, String> {
    let raw: serde_json::Value =
        serde_json::from_str(content).map_err(|e| format!("invalid JSON: {e}"))?;

    let entries = if let Some(arr) = raw.get("plugins").and_then(|v| v.as_array()) {
        arr.iter()
            .filter_map(|v| serde_json::from_value(v.clone()).ok())
            .collect()
    } else {
        Vec::new()
    };

    Ok(MarketplaceIndex {
        name: marketplace_name.to_owned(),
        entries,
        fetched_at: None,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_github_shorthand() {
        let result = resolve_marketplace_source("anthropics/claude-plugins-official");
        assert!(matches!(
            result,
            Some(ResolvedMarketplaceSource::Github { repo }) if repo == "anthropics/claude-plugins-official"
        ));
    }

    #[test]
    fn resolve_url() {
        let result = resolve_marketplace_source("https://example.com/marketplace.json");
        assert!(matches!(
            result,
            Some(ResolvedMarketplaceSource::Url { url }) if url == "https://example.com/marketplace.json"
        ));
    }

    #[test]
    fn resolve_github_url() {
        let result = resolve_marketplace_source("https://github.com/org/repo");
        assert!(matches!(
            result,
            Some(ResolvedMarketplaceSource::Github { repo }) if repo == "org/repo"
        ));
    }

    #[test]
    fn resolve_git_ssh() {
        let result = resolve_marketplace_source("git@github.com:org/repo.git");
        assert!(matches!(
            result,
            Some(ResolvedMarketplaceSource::Git { url, .. }) if url == "git@github.com:org/repo.git"
        ));
    }

    #[test]
    fn resolve_local_directory() {
        let result = resolve_marketplace_source("./my-plugins");
        assert!(matches!(
            result,
            Some(ResolvedMarketplaceSource::Directory { path }) if path == "./my-plugins"
        ));
    }

    #[test]
    fn resolve_local_file() {
        let result = resolve_marketplace_source("./marketplace.json");
        assert!(matches!(
            result,
            Some(ResolvedMarketplaceSource::File { path }) if path == "./marketplace.json"
        ));
    }

    #[test]
    fn resolve_unrecognized() {
        let result = resolve_marketplace_source("just-a-name");
        assert!(result.is_none());
    }

    #[test]
    fn format_failure_details_basic() {
        let failures = vec![
            FailureEntry {
                name: "plugin-a".to_owned(),
                reason: Some("timeout".to_owned()),
            },
            FailureEntry {
                name: "plugin-b".to_owned(),
                reason: Some("not found".to_owned()),
            },
        ];
        let result = format_failure_details(&failures, true);
        assert!(result.contains("plugin-a (timeout)"));
        assert!(result.contains("plugin-b (not found)"));
    }

    #[test]
    fn format_failure_details_with_more() {
        let failures = vec![
            FailureEntry {
                name: "a".to_owned(),
                reason: None,
            },
            FailureEntry {
                name: "b".to_owned(),
                reason: None,
            },
            FailureEntry {
                name: "c".to_owned(),
                reason: None,
            },
        ];
        let result = format_failure_details(&failures, false);
        assert!(result.contains("and 1 more"));
    }

    #[test]
    fn create_plugin_id_works() {
        assert_eq!(
            create_plugin_id("my-plugin", "my-marketplace"),
            "my-plugin@my-marketplace"
        );
    }

    #[test]
    fn parse_marketplace_index_basic() {
        let content = r#"{
            "plugins": [
                {"name": "test", "version": "1.0.0"}
            ]
        }"#;
        let result = parse_marketplace_index(content, "test-mkt");
        assert!(result.is_ok());
        let index = result.expect("index");
        assert_eq!(index.name, "test-mkt");
        assert_eq!(index.entries.len(), 1);
    }

    #[test]
    fn parse_marketplace_index_invalid() {
        let result = parse_marketplace_index("not json", "test-mkt");
        assert!(result.is_err());
    }

    #[test]
    fn get_marketplace_source_display_works() {
        let source = ResolvedMarketplaceSource::Github {
            repo: "org/repo".to_owned(),
        };
        assert_eq!(get_marketplace_source_display(&source), "org/repo");
    }
}
