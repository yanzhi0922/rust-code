//! Dynamic MCP server header resolution.
//!
//! Merges static headers from configuration with dynamic headers obtained
//! by executing a `headersHelper` script. This allows MCP servers that
//! require short-lived tokens (e.g. AWS SigV4) to inject fresh headers
//! at connection time.

use std::collections::HashMap;
use std::process::Stdio;

use tokio::process::Command;

use crate::error::McpRuntimeError;
use crate::transport::TransportConfig;

/// Dynamic MCP server header resolver.
pub struct McpHeadersResolver;

impl McpHeadersResolver {
    /// Resolve headers for a server by merging static config headers with
    /// dynamic headers from a `headersHelper` script.
    ///
    /// Static headers take precedence: if both static and dynamic headers
    /// define the same key, the static value wins.
    pub async fn resolve_headers(
        server_name: &str,
        config: &TransportConfig,
        env_lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<HashMap<String, String>, McpRuntimeError> {
        let mut headers = HashMap::new();

        // 1. Collect static headers from config.
        let (static_headers, helper_path, url) = match config {
            TransportConfig::Sse {
                url,
                headers,
                headers_helper,
                ..
            }
            | TransportConfig::Http {
                url,
                headers,
                headers_helper,
                ..
            } => (headers, headers_helper, url),
            TransportConfig::WebSocket {
                url,
                headers,
                headers_helper,
            } => (headers, headers_helper, url),
            _ => return Ok(headers),
        };

        // Add static headers first.
        if let Some(static_h) = static_headers {
            for (k, v) in static_h {
                // Expand environment variables in header values.
                let expanded = expand_env_in_value(v, &env_lookup);
                headers.insert(k.clone(), expanded);
            }
        }

        // 2. Execute headersHelper script and merge results.
        if let Some(helper) = helper_path {
            let expanded_helper = expand_env_in_value(helper, &env_lookup);
            let dynamic = Self::execute_headers_helper(&expanded_helper, server_name, url).await?;
            // Dynamic headers fill in gaps; static headers win on conflict.
            for (k, v) in dynamic {
                headers.entry(k).or_insert(v);
            }
        }

        Ok(headers)
    }

    /// Execute a `headersHelper` script and parse its JSON output as headers.
    ///
    /// The helper script should print a JSON object of `{"key": "value"}` to stdout.
    pub async fn execute_headers_helper(
        helper_path: &str,
        server_name: &str,
        server_url: &str,
    ) -> Result<HashMap<String, String>, McpRuntimeError> {
        let output = Command::new(helper_path)
            .arg(server_name)
            .arg(server_url)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| McpRuntimeError::Spawn {
                server: server_name.to_owned(),
                command: helper_path.to_owned(),
                source: e,
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(McpRuntimeError::Protocol {
                server: server_name.to_owned(),
                phase: "headersHelper",
                message: format!(
                    "helper script failed with status {}: {stderr}",
                    output.status
                ),
            });
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed: HashMap<String, String> =
            serde_json::from_str(stdout.trim()).map_err(|e| McpRuntimeError::Decode {
                server: server_name.to_owned(),
                phase: "headersHelper output",
                source: e,
            })?;

        Ok(parsed)
    }
}

/// Expand `${VAR}` and `$VAR` patterns in a value using the provided lookup.
fn expand_env_in_value(value: &str, lookup: &impl Fn(&str) -> Option<String>) -> String {
    let mut result = value.to_owned();
    // Expand ${VAR} patterns.
    while let Some(start) = result.find("${") {
        let end = result[start..].find('}').map(|i| start + i);
        if let Some(end) = end {
            let var_name = &result[start + 2..end];
            if let Some(replacement) = lookup(var_name) {
                result = format!("{}{}{}", &result[..start], replacement, &result[end + 1..]);
            } else {
                break;
            }
        } else {
            break;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::TransportConfig;
    use std::collections::BTreeMap;

    fn lookup(key: &str) -> Option<String> {
        match key {
            "API_KEY" => Some("secret123".to_owned()),
            "BEARER_TOKEN" => Some("tok_abc".to_owned()),
            _ => None,
        }
    }

    #[test]
    fn expand_env_braces_found() {
        let result = expand_env_in_value("Bearer ${API_KEY}", &lookup);
        assert_eq!(result, "Bearer secret123");
    }

    #[test]
    fn expand_env_braces_not_found() {
        let result = expand_env_in_value("Bearer ${UNKNOWN}", &lookup);
        assert_eq!(result, "Bearer ${UNKNOWN}");
    }

    #[test]
    fn expand_env_multiple_vars() {
        let result = expand_env_in_value("${API_KEY}:${BEARER_TOKEN}", &lookup);
        assert_eq!(result, "secret123:tok_abc");
    }

    #[test]
    fn expand_env_no_vars() {
        let result = expand_env_in_value("plain-text", &lookup);
        assert_eq!(result, "plain-text");
    }

    #[tokio::test]
    async fn resolve_headers_stdio_returns_empty() {
        let config = TransportConfig::Stdio {
            command: "echo".to_owned(),
            args: vec![],
            env: None,
            cwd: None,
        };
        let headers = McpHeadersResolver::resolve_headers("test", &config, lookup)
            .await
            .expect("resolve");
        assert!(headers.is_empty());
    }

    #[tokio::test]
    async fn resolve_headers_http_static() {
        let mut h = BTreeMap::new();
        h.insert("X-Custom".to_owned(), "value".to_owned());
        let config = TransportConfig::Http {
            url: "https://example.com".to_owned(),
            headers: Some(h),
            headers_helper: None,
            oauth: None,
        };
        let headers = McpHeadersResolver::resolve_headers("test", &config, lookup)
            .await
            .expect("resolve");
        assert_eq!(headers.get("X-Custom").map(String::as_str), Some("value"));
    }

    #[tokio::test]
    async fn resolve_headers_expands_env_vars() {
        let mut h = BTreeMap::new();
        h.insert("Authorization".to_owned(), "Bearer ${API_KEY}".to_owned());
        let config = TransportConfig::Http {
            url: "https://example.com".to_owned(),
            headers: Some(h),
            headers_helper: None,
            oauth: None,
        };
        let headers = McpHeadersResolver::resolve_headers("test", &config, lookup)
            .await
            .expect("resolve");
        assert_eq!(
            headers.get("Authorization").map(String::as_str),
            Some("Bearer secret123")
        );
    }

    #[tokio::test]
    async fn execute_headers_helper_invalid_path() {
        let result = McpHeadersResolver::execute_headers_helper(
            "/nonexistent/script.sh",
            "test",
            "https://example.com",
        )
        .await;
        assert!(result.is_err());
    }
}
