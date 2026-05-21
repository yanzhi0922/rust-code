//! Official MCP server registry.
//!
//! Maintains a set of known official MCP server URLs. This can be used
//! to distinguish between official/trusted servers and third-party ones,
//! enabling different permission or security policies.

use std::collections::HashSet;

// ── Official MCP server registry ────────────────────────────────────────────

/// Registry of known official MCP server URLs.
///
/// URLs are normalized (lowercased, trailing slashes removed, query
/// parameters stripped) before comparison to ensure consistent matching.
#[derive(Debug, Clone, Default)]
pub struct OfficialMcpRegistry {
    /// Normalized URLs of official servers.
    official_urls: HashSet<String>,
    /// Whether the registry has been loaded.
    loaded: bool,
}

impl OfficialMcpRegistry {
    /// Create a new empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            official_urls: HashSet::new(),
            loaded: false,
        }
    }

    /// Load the registry from a list of URL strings.
    ///
    /// Each URL is normalized before being added to the set.
    pub fn load_from_urls(&mut self, urls: &[&str]) {
        for url in urls {
            if let Some(normalized) = Self::normalize_url(url) {
                self.official_urls.insert(normalized);
            }
        }
        self.loaded = true;
    }

    /// Check if a URL belongs to an official server.
    ///
    /// The URL is normalized before checking.
    #[must_use]
    pub fn is_official(&self, url: &str) -> bool {
        match Self::normalize_url(url) {
            Some(normalized) => self.official_urls.contains(&normalized),
            None => false,
        }
    }

    /// Normalize a URL for consistent comparison.
    ///
    /// - Converts to lowercase
    /// - Removes trailing slashes
    /// - Strips query parameters
    /// - Strips fragment identifiers
    ///
    /// Returns `None` if the input is empty or clearly invalid.
    #[must_use]
    pub fn normalize_url(url: &str) -> Option<String> {
        let trimmed = url.trim();
        if trimmed.is_empty() {
            return None;
        }

        let mut result = trimmed.to_lowercase();

        // Remove fragment
        if let Some(idx) = result.find('#') {
            result.truncate(idx);
        }

        // Remove query parameters
        if let Some(idx) = result.find('?') {
            result.truncate(idx);
        }

        // Remove trailing slashes
        while result.ends_with('/') {
            result.pop();
        }

        if result.is_empty() {
            return None;
        }

        Some(result)
    }

    /// Get the number of registered official servers.
    #[must_use]
    pub fn count(&self) -> usize {
        self.official_urls.len()
    }

    /// Check if the registry has been loaded.
    #[must_use]
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// Clear all entries from the registry.
    pub fn clear(&mut self) {
        self.official_urls.clear();
        self.loaded = false;
    }

    /// Get an iterator over the registered URLs.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.official_urls.iter().map(|s| s.as_str())
    }
}

// ── Well-known official servers ─────────────────────────────────────────────

/// Returns a list of well-known official MCP server URLs.
///
/// This is a static list that can be used as a fallback when the
/// registry API is unavailable.
#[must_use]
pub fn well_known_official_servers() -> Vec<&'static str> {
    vec![
        "https://mcp.server.github.com",
        "https://mcp.server.anthropic.com",
        "https://mcp.server.openai.com",
        "https://mcp.server.cursor.com",
    ]
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_registry_is_empty() {
        let reg = OfficialMcpRegistry::new();
        assert_eq!(reg.count(), 0);
        assert!(!reg.is_loaded());
    }

    #[test]
    fn load_from_urls_adds_entries() {
        let mut reg = OfficialMcpRegistry::new();
        reg.load_from_urls(&["https://example.com/mcp", "https://other.com/mcp"]);
        assert_eq!(reg.count(), 2);
        assert!(reg.is_loaded());
    }

    #[test]
    fn is_official_checks_normalized_url() {
        let mut reg = OfficialMcpRegistry::new();
        reg.load_from_urls(&["https://Example.COM/MCP/"]);

        assert!(reg.is_official("https://example.com/mcp"));
        assert!(reg.is_official("https://example.com/mcp/"));
        assert!(reg.is_official("https://EXAMPLE.COM/MCP"));
        assert!(!reg.is_official("https://other.com/mcp"));
    }

    #[test]
    fn normalize_url_removes_trailing_slash() {
        assert_eq!(
            OfficialMcpRegistry::normalize_url("https://example.com/mcp/"),
            Some("https://example.com/mcp".to_owned())
        );
    }

    #[test]
    fn normalize_url_removes_query_params() {
        assert_eq!(
            OfficialMcpRegistry::normalize_url("https://example.com/mcp?token=abc"),
            Some("https://example.com/mcp".to_owned())
        );
    }

    #[test]
    fn normalize_url_removes_fragment() {
        assert_eq!(
            OfficialMcpRegistry::normalize_url("https://example.com/mcp#section"),
            Some("https://example.com/mcp".to_owned())
        );
    }

    #[test]
    fn normalize_url_lowercases() {
        assert_eq!(
            OfficialMcpRegistry::normalize_url("HTTPS://EXAMPLE.COM/MCP"),
            Some("https://example.com/mcp".to_owned())
        );
    }

    #[test]
    fn normalize_url_returns_none_for_empty() {
        assert_eq!(OfficialMcpRegistry::normalize_url(""), None);
        assert_eq!(OfficialMcpRegistry::normalize_url("   "), None);
    }

    #[test]
    fn clear_resets_registry() {
        let mut reg = OfficialMcpRegistry::new();
        reg.load_from_urls(&["https://example.com"]);
        assert!(reg.is_loaded());
        reg.clear();
        assert_eq!(reg.count(), 0);
        assert!(!reg.is_loaded());
    }

    #[test]
    fn duplicate_urls_count_once() {
        let mut reg = OfficialMcpRegistry::new();
        reg.load_from_urls(&[
            "https://example.com/mcp",
            "https://example.com/mcp/",
            "HTTPS://EXAMPLE.COM/MCP",
        ]);
        assert_eq!(reg.count(), 1);
    }

    #[test]
    fn iter_returns_all_urls() {
        let mut reg = OfficialMcpRegistry::new();
        reg.load_from_urls(&["https://a.com", "https://b.com"]);
        let urls: Vec<&str> = reg.iter().collect();
        assert_eq!(urls.len(), 2);
    }

    #[test]
    fn well_known_official_servers_not_empty() {
        let servers = well_known_official_servers();
        assert!(!servers.is_empty());
        for url in &servers {
            assert!(url.starts_with("https://"));
        }
    }

    #[test]
    fn load_from_well_known() {
        let mut reg = OfficialMcpRegistry::new();
        let servers = well_known_official_servers();
        reg.load_from_urls(&servers);
        assert!(reg.count() > 0);
        assert!(reg.is_official("https://mcp.server.github.com"));
    }
}
