//! MCP configuration loading, parsing, and saving.
//!
//! Handles TOML- and JSON-based configuration files with support for stdio,
//! HTTP, and WebSocket transports. Includes raw intermediate types for
//! serialization/deserialization.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::McpConfigError;
use crate::tool_policy::McpToolPolicy;
use crate::transport::{McpOAuthConfig, McpTransport, McpTransportConfig, infer_transport_kind};

/// Capability flags reported by an MCP server during initialisation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct McpCapabilityMatrix {
    /// Server supports the `tools` capability.
    #[serde(default)]
    pub supports_tools: bool,
    /// Server supports the `prompts` capability.
    #[serde(default)]
    pub supports_prompts: bool,
    /// Server supports the `resources` capability.
    #[serde(default)]
    pub supports_resources: bool,
    /// Server supports the `sampling` capability.
    #[serde(default)]
    pub supports_sampling: bool,
    /// Server supports the `roots` capability.
    #[serde(default)]
    pub supports_roots: bool,
}

/// Configuration for a single MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Server name (used as a key in the config map).
    pub name: String,
    /// Whether the server is enabled.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Transport configuration.
    pub transport: McpTransportConfig,
    /// Reported capabilities.
    #[serde(default)]
    pub capabilities: McpCapabilityMatrix,
    /// Startup timeout override in seconds.
    #[serde(default)]
    pub startup_timeout_secs: Option<u64>,
    /// Request timeout override in seconds.
    #[serde(default)]
    pub request_timeout_secs: Option<u64>,
    /// Arbitrary metadata.
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    /// OAuth configuration for HTTP/SSE MCP servers.
    #[serde(default)]
    pub oauth: Option<McpOAuthConfig>,
    /// Per-tool policy for filtering which tools this server exposes.
    #[serde(default)]
    pub tool_policy: McpToolPolicy,
}

/// Top-level MCP configuration containing all servers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct McpConfig {
    /// Map of server name → server configuration.
    pub servers: BTreeMap<String, McpServerConfig>,
}

/// An MCP configuration file discovered on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredMcpConfig {
    /// Path to the configuration file.
    pub path: PathBuf,
    /// Parsed configuration.
    pub config: McpConfig,
}

// ── Raw TOML intermediate types ─────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Default)]
pub(crate) struct RawMcpConfig {
    #[serde(
        default,
        rename = "mcp_servers",
        alias = "servers",
        alias = "mcpServers"
    )]
    pub(crate) servers: BTreeMap<String, RawMcpServer>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub(crate) struct RawMcpServer {
    #[serde(default, rename = "transport", alias = "type")]
    pub(crate) transport_kind: Option<String>,
    pub(crate) command: Option<String>,
    #[serde(default)]
    pub(crate) args: Vec<String>,
    pub(crate) cwd: Option<PathBuf>,
    #[serde(default)]
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) url: Option<String>,
    #[serde(default, rename = "http_headers", alias = "headers")]
    pub(crate) http_headers: BTreeMap<String, String>,
    pub(crate) enabled: Option<bool>,
    pub(crate) startup_timeout_secs: Option<u64>,
    pub(crate) request_timeout_secs: Option<u64>,
    #[serde(default)]
    pub(crate) capabilities: RawMcpCapabilities,
    #[serde(default)]
    pub(crate) metadata: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) oauth: Option<McpOAuthConfig>,
    #[serde(default)]
    pub(crate) tool_policy: McpToolPolicy,
    // --- IDE transport fields ---
    /// IDE name for sse-ide and ws-ide transports.
    #[serde(default, rename = "ideName", alias = "ide_name")]
    pub(crate) ide_name: Option<String>,
    /// Auth token for ws-ide transport.
    #[serde(default, rename = "authToken", alias = "auth_token")]
    pub(crate) auth_token: Option<String>,
    /// Whether the IDE is running in a Windows environment (sse-ide, ws-ide).
    #[serde(
        default,
        rename = "ideRunningInWindows",
        alias = "ide_running_in_windows"
    )]
    pub(crate) ide_running_in_windows: Option<bool>,
    /// SDK server name for the `sdk` transport.
    #[serde(default)]
    pub(crate) sdk_name: Option<String>,
    /// Proxy session ID for the `claudeai-proxy` transport.
    #[serde(default, rename = "proxyId", alias = "proxy_id", alias = "id")]
    pub(crate) proxy_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct RawMcpCapabilities {
    #[serde(default, alias = "tools")]
    pub(crate) supports_tools: bool,
    #[serde(default, alias = "prompts")]
    pub(crate) supports_prompts: bool,
    #[serde(default, alias = "resources")]
    pub(crate) supports_resources: bool,
    #[serde(default, alias = "sampling")]
    pub(crate) supports_sampling: bool,
    #[serde(default, alias = "roots")]
    pub(crate) supports_roots: bool,
}

fn default_enabled() -> bool {
    true
}

fn expand_env_vars(value: &str) -> String {
    let mut result = value.to_string();
    while let Some(start) = result.find("${") {
        let end = result[start..].find('}').map(|i| start + i);
        if let Some(end) = end {
            let var_name = &result[start + 2..end];
            let replacement = std::env::var(var_name).unwrap_or_default();
            result = format!("{}{}{}", &result[..start], replacement, &result[end + 1..]);
        } else {
            break;
        }
    }
    result
}

fn expand_env_map(env: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    env.iter()
        .map(|(k, v)| (k.clone(), expand_env_vars(v)))
        .collect()
}

fn should_parse_as_json(path: &Path, content: &str) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        || content.trim_start().starts_with('{')
}

fn infer_transport_kind_with_override(url: &str, transport_kind: Option<&str>) -> McpTransport {
    match transport_kind
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("http") | Some("sse") | Some("sse-ide") => McpTransport::Http,
        Some("ws") | Some("websocket") | Some("ws-ide") => McpTransport::WebSocket,
        Some("stdio") => McpTransport::Stdio,
        _ => infer_transport_kind(url),
    }
}

impl From<RawMcpCapabilities> for McpCapabilityMatrix {
    fn from(value: RawMcpCapabilities) -> Self {
        Self {
            supports_tools: value.supports_tools,
            supports_prompts: value.supports_prompts,
            supports_resources: value.supports_resources,
            supports_sampling: value.supports_sampling,
            supports_roots: value.supports_roots,
        }
    }
}

impl McpConfig {
    /// Parse an MCP configuration from a TOML string.
    pub fn from_toml_str(input: &str) -> Result<Self, McpConfigError> {
        let raw: RawMcpConfig =
            toml::from_str(input).map_err(|source| McpConfigError::ParseToml {
                path: PathBuf::from("<memory>"),
                source,
            })?;
        Self::from_raw(raw)
    }

    /// Parse an MCP configuration from a JSON string.
    pub fn from_json_str(input: &str) -> Result<Self, McpConfigError> {
        let raw: RawMcpConfig =
            serde_json::from_str(input).map_err(|source| McpConfigError::ParseJson {
                path: PathBuf::from("<memory>"),
                source,
            })?;
        Self::from_raw(raw)
    }

    /// Load an MCP configuration from a file on disk.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, McpConfigError> {
        let path = path.as_ref();
        let content = fs::read_to_string(path).map_err(|source| McpConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let raw = if should_parse_as_json(path, &content) {
            serde_json::from_str(&content).map_err(|source| McpConfigError::ParseJson {
                path: path.to_path_buf(),
                source,
            })?
        } else {
            toml::from_str(&content).map_err(|source| McpConfigError::ParseToml {
                path: path.to_path_buf(),
                source,
            })?
        };
        Self::from_raw(raw)
    }

    fn from_raw(raw: RawMcpConfig) -> Result<Self, McpConfigError> {
        let mut servers = BTreeMap::new();

        for (name, raw_server) in raw.servers {
            let transport =
                match raw_server
                    .transport_kind
                    .as_deref()
                    .map(str::trim)
                    .map(str::to_ascii_lowercase)
                    .as_deref()
                {
                    // Explicit sdk transport — no command or url needed.
                    Some("sdk") => McpTransportConfig::Sdk {
                        name: raw_server.sdk_name,
                    },
                    // Explicit claudeai-proxy transport.
                    Some("claudeai-proxy") => McpTransportConfig::ClaudeAiProxy {
                        url: raw_server.url,
                        id: raw_server.proxy_id,
                    },
                    // Explicit sse-ide transport.
                    Some("sse-ide") => {
                        let url = raw_server.url.as_deref().ok_or_else(|| {
                            McpConfigError::MissingTransport { name: name.clone() }
                        })?;
                        McpTransportConfig::SseIde {
                            url: url.to_owned(),
                            ide_name: raw_server.ide_name,
                            ide_running_in_windows: raw_server.ide_running_in_windows,
                        }
                    }
                    // Explicit ws-ide transport.
                    Some("ws-ide") => {
                        let url = raw_server.url.as_deref().ok_or_else(|| {
                            McpConfigError::MissingTransport { name: name.clone() }
                        })?;
                        McpTransportConfig::WsIde {
                            url: url.to_owned(),
                            ide_name: raw_server.ide_name,
                            auth_token: raw_server.auth_token,
                            ide_running_in_windows: raw_server.ide_running_in_windows,
                        }
                    }
                    // Other explicit or inferred transports.
                    _ => match (&raw_server.command, &raw_server.url) {
                        (Some(_), Some(_)) => {
                            return Err(McpConfigError::AmbiguousTransport { name });
                        }
                        (None, None) => return Err(McpConfigError::MissingTransport { name }),
                        (Some(command), None) => McpTransportConfig::Stdio {
                            command: command.clone(),
                            args: raw_server.args,
                            cwd: raw_server.cwd,
                            env: expand_env_map(&raw_server.env),
                        },
                        (None, Some(url)) => {
                            let headers = raw_server.http_headers;
                            // If the transport kind was explicitly "sse", use SSE config.
                            match raw_server.transport_kind.as_deref() {
                                Some(t) if t.trim().eq_ignore_ascii_case("sse") => {
                                    McpTransportConfig::Sse {
                                        url: url.clone(),
                                        headers,
                                        headers_helper: None,
                                    }
                                }
                                _ => match infer_transport_kind_with_override(
                                    url,
                                    raw_server.transport_kind.as_deref(),
                                ) {
                                    McpTransport::Http => McpTransportConfig::Http {
                                        url: url.clone(),
                                        headers,
                                        headers_helper: None,
                                    },
                                    McpTransport::WebSocket => McpTransportConfig::WebSocket {
                                        url: url.clone(),
                                        headers,
                                        headers_helper: None,
                                    },
                                    McpTransport::Stdio => {
                                        let scheme = url
                                            .split(':')
                                            .next()
                                            .map_or_else(String::new, str::to_owned);
                                        return Err(McpConfigError::UnsupportedUrlScheme {
                                            name,
                                            scheme,
                                        });
                                    }
                                },
                            }
                        }
                    },
                };

            let server_name = name.clone();
            servers.insert(
                name,
                McpServerConfig {
                    name: server_name,
                    enabled: raw_server.enabled.unwrap_or(true),
                    transport,
                    capabilities: raw_server.capabilities.into(),
                    startup_timeout_secs: raw_server.startup_timeout_secs,
                    request_timeout_secs: raw_server.request_timeout_secs,
                    metadata: raw_server.metadata,
                    oauth: raw_server.oauth,
                    tool_policy: raw_server.tool_policy,
                },
            );
        }

        Ok(Self { servers })
    }

    /// Serialize the configuration to a TOML string.
    pub fn to_toml_string(&self) -> Result<String, McpConfigError> {
        toml::to_string_pretty(&RawMcpConfig::from(self))
            .map_err(|source| McpConfigError::Serialize { source })
    }

    /// Save the configuration to a file on disk.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), McpConfigError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| McpConfigError::Write {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let contents = self.to_toml_string()?;
        fs::write(path, contents).map_err(|source| McpConfigError::Write {
            path: path.to_path_buf(),
            source,
        })
    }
}

impl From<&McpConfig> for RawMcpConfig {
    fn from(value: &McpConfig) -> Self {
        let servers = value
            .servers
            .iter()
            .map(|(name, server)| {
                let mut raw = RawMcpServer {
                    enabled: Some(server.enabled),
                    startup_timeout_secs: server.startup_timeout_secs,
                    request_timeout_secs: server.request_timeout_secs,
                    capabilities: RawMcpCapabilities {
                        supports_tools: server.capabilities.supports_tools,
                        supports_prompts: server.capabilities.supports_prompts,
                        supports_resources: server.capabilities.supports_resources,
                        supports_sampling: server.capabilities.supports_sampling,
                        supports_roots: server.capabilities.supports_roots,
                    },
                    metadata: server.metadata.clone(),
                    oauth: server.oauth.clone(),
                    tool_policy: server.tool_policy.clone(),
                    ..Default::default()
                };

                match &server.transport {
                    McpTransportConfig::Stdio {
                        command,
                        args,
                        cwd,
                        env,
                    } => {
                        raw.command = Some(command.clone());
                        raw.args = args.clone();
                        raw.cwd = cwd.clone();
                        raw.env = env.clone();
                    }
                    McpTransportConfig::Sse { url, headers, .. } => {
                        raw.transport_kind = Some("sse".to_owned());
                        raw.url = Some(url.clone());
                        raw.http_headers = headers.clone();
                    }
                    McpTransportConfig::SseIde {
                        url,
                        ide_name,
                        ide_running_in_windows,
                    } => {
                        raw.transport_kind = Some("sse-ide".to_owned());
                        raw.url = Some(url.clone());
                        raw.ide_name = ide_name.clone();
                        raw.ide_running_in_windows = *ide_running_in_windows;
                    }
                    McpTransportConfig::Http { url, headers, .. } => {
                        raw.url = Some(url.clone());
                        raw.http_headers = headers.clone();
                    }
                    McpTransportConfig::WebSocket { url, headers, .. } => {
                        raw.url = Some(url.clone());
                        raw.http_headers = headers.clone();
                    }
                    McpTransportConfig::WsIde {
                        url,
                        ide_name,
                        auth_token,
                        ide_running_in_windows,
                    } => {
                        raw.transport_kind = Some("ws-ide".to_owned());
                        raw.url = Some(url.clone());
                        raw.ide_name = ide_name.clone();
                        raw.auth_token = auth_token.clone();
                        raw.ide_running_in_windows = *ide_running_in_windows;
                    }
                    McpTransportConfig::Sdk { name: sdk_name } => {
                        raw.transport_kind = Some("sdk".to_owned());
                        raw.sdk_name = sdk_name.clone();
                    }
                    McpTransportConfig::ClaudeAiProxy { url, id } => {
                        raw.transport_kind = Some("claudeai-proxy".to_owned());
                        raw.url = url.clone();
                        raw.proxy_id = id.clone();
                    }
                }

                (name.clone(), raw)
            })
            .collect();
        Self { servers }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_matrix_default() {
        let caps = McpCapabilityMatrix::default();
        assert!(!caps.supports_tools);
        assert!(!caps.supports_prompts);
        assert!(!caps.supports_resources);
        assert!(!caps.supports_sampling);
        assert!(!caps.supports_roots);
    }

    #[test]
    fn config_default_is_empty() {
        let config = McpConfig::default();
        assert!(config.servers.is_empty());
    }

    #[test]
    fn parses_minimal_stdio() {
        let config = McpConfig::from_toml_str(
            r#"[mcp_servers.echo]
command = "echo""#,
        )
        .expect("should parse");
        assert_eq!(config.servers.len(), 1);
        let echo = &config.servers["echo"];
        assert!(echo.enabled);
        assert_eq!(echo.transport.kind(), McpTransport::Stdio);
    }

    #[test]
    fn rejects_ambiguous_transport() {
        let err = McpConfig::from_toml_str(
            r#"[mcp_servers.bad]
command = "echo"
url = "https://example.com""#,
        )
        .expect_err("should fail");
        assert!(matches!(err, McpConfigError::AmbiguousTransport { .. }));
    }

    #[test]
    fn parses_json_mcp_servers_shape() {
        let config = McpConfig::from_json_str(
            r#"{
                "mcpServers": {
                    "context7": {
                        "type": "stdio",
                        "command": "npx",
                        "args": ["-y", "@upstash/context7-mcp"]
                    },
                    "relay": {
                        "type": "ws",
                        "url": "https://example.com/mcp",
                        "headers": {
                            "Authorization": "Bearer token"
                        }
                    }
                }
            }"#,
        )
        .expect("should parse");

        assert_eq!(config.servers.len(), 2);
        assert_eq!(
            config.servers["context7"].transport.kind(),
            McpTransport::Stdio
        );
        assert_eq!(
            config.servers["relay"].transport.kind(),
            McpTransport::WebSocket
        );
    }

    #[test]
    fn parses_http_oauth_config_with_reference_camel_case() {
        let config = McpConfig::from_json_str(
            r#"{
                "mcpServers": {
                    "remote": {
                        "type": "http",
                        "url": "https://example.com/mcp",
                        "oauth": {
                            "clientId": "client-123",
                            "callbackPort": 4567,
                            "authServerMetadataUrl": "https://auth.example.com/.well-known/oauth-authorization-server"
                        }
                    }
                }
            }"#,
        )
        .expect("should parse");

        let oauth = config.servers["remote"]
            .oauth
            .as_ref()
            .expect("oauth config");
        assert_eq!(oauth.client_id.as_deref(), Some("client-123"));
        assert_eq!(oauth.callback_port, Some(4567));
        assert_eq!(
            oauth.auth_server_metadata_url.as_deref(),
            Some("https://auth.example.com/.well-known/oauth-authorization-server")
        );
    }

    #[test]
    fn load_detects_json_by_extension() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".mcp.json");
        fs::write(
            &path,
            r#"{
                "mcpServers": {
                    "echo": {
                        "command": "python"
                    }
                }
            }"#,
        )
        .expect("write");

        let config = McpConfig::load(&path).expect("load");
        assert_eq!(config.servers.len(), 1);
        assert_eq!(config.servers["echo"].transport.kind(), McpTransport::Stdio);
    }

    #[test]
    fn toml_roundtrip() {
        let config = McpConfig {
            servers: BTreeMap::from([(
                "demo".to_owned(),
                McpServerConfig {
                    name: "demo".to_owned(),
                    enabled: true,
                    transport: McpTransportConfig::Stdio {
                        command: "echo".to_owned(),
                        args: vec![],
                        cwd: None,
                        env: BTreeMap::new(),
                    },
                    capabilities: McpCapabilityMatrix::default(),
                    startup_timeout_secs: None,
                    request_timeout_secs: None,
                    metadata: BTreeMap::new(),
                    oauth: None,
                    tool_policy: McpToolPolicy::default(),
                },
            )]),
        };
        let toml_str = config.to_toml_string().expect("serialize");
        let back = McpConfig::from_toml_str(&toml_str).expect("deserialize");
        assert_eq!(config, back);
    }
}
