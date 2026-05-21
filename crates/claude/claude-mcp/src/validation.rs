//! MCP configuration validation utilities.
//!
//! Provides validators for server names, URLs, command safety, and
//! duplicate detection across MCP server configurations.

use std::collections::HashMap;

use crate::config::McpServerConfig;

// ── Validation types ────────────────────────────────────────────────────────

/// Severity level for security warnings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityLevel {
    /// Informational, no action needed.
    Info,
    /// Potential issue, should be reviewed.
    Warning,
    /// Critical issue, should be addressed immediately.
    Critical,
}

/// A security warning about an MCP server configuration.
#[derive(Debug, Clone)]
pub struct SecurityWarning {
    /// Severity level.
    pub level: SecurityLevel,
    /// Human-readable warning message.
    pub message: String,
}

/// Kind of validation warning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationWarningKind {
    /// Server name doesn't match the required pattern.
    InvalidName,
    /// URL format is invalid.
    InvalidUrl,
    /// Command is missing for stdio transport.
    MissingCommand,
    /// URL is missing for HTTP/WebSocket transport.
    MissingUrl,
    /// Using a deprecated transport type.
    DeprecatedTransport,
    /// Potential security risk detected.
    SecurityRisk,
}

/// A validation warning for a specific server configuration.
#[derive(Debug, Clone)]
pub struct ValidationWarning {
    /// Name of the server with the warning.
    pub server_name: String,
    /// Kind of warning.
    pub kind: ValidationWarningKind,
    /// Human-readable warning message.
    pub message: String,
}

/// A pair of duplicate server entries.
#[derive(Debug, Clone)]
pub struct DuplicateEntry {
    /// First duplicate name.
    pub name1: String,
    /// Second duplicate name.
    pub name2: String,
    /// Reason they are considered duplicates.
    pub reason: String,
}

// ── Validator ───────────────────────────────────────────────────────────────

/// MCP configuration validator.
///
/// Provides static methods for validating server names, URLs, commands,
/// and detecting duplicate configurations.
pub struct McpConfigValidator;

impl McpConfigValidator {
    /// Validate a single server configuration.
    ///
    /// Returns a list of warnings (empty if the configuration is valid).
    pub fn validate_server_config(name: &str, config: &McpServerConfig) -> Vec<ValidationWarning> {
        let mut warnings = Vec::new();

        // Validate server name
        if !Self::validate_server_name(name) {
            warnings.push(ValidationWarning {
                server_name: name.to_owned(),
                kind: ValidationWarningKind::InvalidName,
                message: format!("server name '{name}' must match ^[a-zA-Z0-9_-]{{1,64}}$"),
            });
        }

        // Validate transport-specific requirements
        match &config.transport {
            crate::transport::McpTransportConfig::Stdio { command, .. } => {
                if command.is_empty() {
                    warnings.push(ValidationWarning {
                        server_name: name.to_owned(),
                        kind: ValidationWarningKind::MissingCommand,
                        message: "stdio transport requires a non-empty command".to_owned(),
                    });
                }
            }
            crate::transport::McpTransportConfig::Http { url, .. }
            | crate::transport::McpTransportConfig::WebSocket { url, .. }
            | crate::transport::McpTransportConfig::Sse { url, .. }
            | crate::transport::McpTransportConfig::SseIde { url, .. }
            | crate::transport::McpTransportConfig::WsIde { url, .. } => {
                if url.is_empty() {
                    warnings.push(ValidationWarning {
                        server_name: name.to_owned(),
                        kind: ValidationWarningKind::MissingUrl,
                        message: "transport requires a non-empty URL".to_owned(),
                    });
                } else if !Self::validate_url(url) {
                    warnings.push(ValidationWarning {
                        server_name: name.to_owned(),
                        kind: ValidationWarningKind::InvalidUrl,
                        message: format!("invalid URL format: {url}"),
                    });
                }
            }
            crate::transport::McpTransportConfig::Sdk { .. }
            | crate::transport::McpTransportConfig::ClaudeAiProxy { .. } => {}
        }

        warnings
    }

    /// Validate a server name.
    ///
    /// Names must match `^[a-zA-Z0-9_-]{1,64}$`.
    #[must_use]
    pub fn validate_server_name(name: &str) -> bool {
        if name.is_empty() || name.len() > 64 {
            return false;
        }
        name.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    }

    /// Validate a URL format.
    ///
    /// Checks that the URL has a valid scheme (http or https) and a host.
    #[must_use]
    pub fn validate_url(url: &str) -> bool {
        // Basic URL validation: must start with http:// or https://
        let lower = url.to_lowercase();
        if !lower.starts_with("http://") && !lower.starts_with("https://") {
            return false;
        }

        // Must have something after the scheme
        let after_scheme = if lower.starts_with("https://") {
            &url[8..]
        } else {
            &url[7..]
        };

        // Must have at least a host (something before the first / or the whole string)
        let host_part = after_scheme.split('/').next().unwrap_or("");
        if host_part.is_empty() {
            return false;
        }

        // Host must contain a dot or be "localhost"
        if !host_part.contains('.') && !host_part.starts_with("localhost") {
            return false;
        }

        true
    }

    /// Find duplicate server configurations.
    ///
    /// Servers are considered duplicates if they have the same transport
    /// target (command for stdio, URL for HTTP/WebSocket).
    pub fn find_duplicates(configs: &HashMap<String, McpServerConfig>) -> Vec<DuplicateEntry> {
        let mut duplicates = Vec::new();
        let entries: Vec<(&String, &McpServerConfig)> = configs.iter().collect();

        for i in 0..entries.len() {
            for j in (i + 1)..entries.len() {
                let (name1, config1) = entries[i];
                let (name2, config2) = entries[j];

                if let Some(reason) = Self::check_duplicate_pair(config1, config2) {
                    duplicates.push(DuplicateEntry {
                        name1: name1.clone(),
                        name2: name2.clone(),
                        reason,
                    });
                }
            }
        }

        duplicates
    }

    /// Check if two configurations are duplicates.
    fn check_duplicate_pair(a: &McpServerConfig, b: &McpServerConfig) -> Option<String> {
        match (&a.transport, &b.transport) {
            (
                crate::transport::McpTransportConfig::Stdio {
                    command: cmd_a,
                    args: args_a,
                    ..
                },
                crate::transport::McpTransportConfig::Stdio {
                    command: cmd_b,
                    args: args_b,
                    ..
                },
            ) => {
                if cmd_a == cmd_b && args_a == args_b {
                    Some(format!("same stdio command: {cmd_a}"))
                } else {
                    None
                }
            }
            (
                crate::transport::McpTransportConfig::Http { url: url_a, .. },
                crate::transport::McpTransportConfig::Http { url: url_b, .. },
            ) => {
                if url_a == url_b {
                    Some(format!("same HTTP URL: {url_a}"))
                } else {
                    None
                }
            }
            (
                crate::transport::McpTransportConfig::WebSocket { url: url_a, .. },
                crate::transport::McpTransportConfig::WebSocket { url: url_b, .. },
            ) => {
                if url_a == url_b {
                    Some(format!("same WebSocket URL: {url_a}"))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Check command safety.
    ///
    /// Returns warnings for potentially dangerous commands or arguments.
    pub fn check_command_safety(command: &str, args: &[String]) -> Vec<SecurityWarning> {
        let mut warnings = Vec::new();

        // Check for dangerous commands
        let dangerous_commands = [
            "rm", "rmdir", "del", "format", "mkfs", "dd", "shred", "sudo", "su", "chmod", "chown",
            "curl", "wget",
        ];

        let cmd_lower = command.to_lowercase();
        // Extract just the binary name from the path
        let binary = cmd_lower
            .rsplit('/')
            .next()
            .unwrap_or(&cmd_lower)
            .rsplit('\\')
            .next()
            .unwrap_or(&cmd_lower);

        if dangerous_commands.contains(&binary) {
            warnings.push(SecurityWarning {
                level: SecurityLevel::Critical,
                message: format!("command '{command}' may be dangerous"),
            });
        }

        // Check for shell injection patterns in arguments
        for arg in args {
            if arg.contains("..") {
                warnings.push(SecurityWarning {
                    level: SecurityLevel::Warning,
                    message: format!("argument contains path traversal: {arg}"),
                });
            }
            if arg.contains('$') || arg.contains('`') {
                warnings.push(SecurityWarning {
                    level: SecurityLevel::Warning,
                    message: format!("argument may contain shell expansion: {arg}"),
                });
            }
        }

        warnings
    }
}

use serde::{Deserialize, Serialize};

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::McpTransportConfig;
    use std::collections::BTreeMap;

    fn make_stdio_config(name: &str, command: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.to_owned(),
            enabled: true,
            transport: McpTransportConfig::Stdio {
                command: command.to_owned(),
                args: vec![],
                cwd: None,
                env: BTreeMap::new(),
            },
            capabilities: Default::default(),
            startup_timeout_secs: None,
            request_timeout_secs: None,
            metadata: BTreeMap::new(),
            oauth: None,
            tool_policy: crate::tool_policy::McpToolPolicy::default(),
        }
    }

    fn make_http_config(name: &str, url: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.to_owned(),
            enabled: true,
            transport: McpTransportConfig::Http {
                url: url.to_owned(),
                headers: BTreeMap::new(),
                headers_helper: None,
            },
            capabilities: Default::default(),
            startup_timeout_secs: None,
            request_timeout_secs: None,
            metadata: BTreeMap::new(),
            oauth: None,
            tool_policy: crate::tool_policy::McpToolPolicy::default(),
        }
    }

    #[test]
    fn validate_server_name_valid() {
        assert!(McpConfigValidator::validate_server_name("my-server"));
        assert!(McpConfigValidator::validate_server_name("server_1"));
        assert!(McpConfigValidator::validate_server_name("ABC"));
        assert!(McpConfigValidator::validate_server_name("a"));
    }

    #[test]
    fn validate_server_name_invalid() {
        assert!(!McpConfigValidator::validate_server_name(""));
        assert!(!McpConfigValidator::validate_server_name(
            "server with spaces"
        ));
        assert!(!McpConfigValidator::validate_server_name("server.name"));
        assert!(!McpConfigValidator::validate_server_name("server@name"));
        assert!(!McpConfigValidator::validate_server_name(&"x".repeat(65)));
    }

    #[test]
    fn validate_url_valid() {
        assert!(McpConfigValidator::validate_url("https://example.com"));
        assert!(McpConfigValidator::validate_url("http://localhost:8080"));
        assert!(McpConfigValidator::validate_url(
            "https://api.example.com/v1/mcp"
        ));
    }

    #[test]
    fn validate_url_invalid() {
        assert!(!McpConfigValidator::validate_url(""));
        assert!(!McpConfigValidator::validate_url("ftp://example.com"));
        assert!(!McpConfigValidator::validate_url("not-a-url"));
        assert!(!McpConfigValidator::validate_url("http://"));
    }

    #[test]
    fn validate_server_config_valid_stdio() {
        let config = make_stdio_config("test-server", "npx");
        let warnings = McpConfigValidator::validate_server_config("test-server", &config);
        assert!(warnings.is_empty());
    }

    #[test]
    fn validate_server_config_invalid_name() {
        let config = make_stdio_config("bad name", "npx");
        let warnings = McpConfigValidator::validate_server_config("bad name", &config);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].kind, ValidationWarningKind::InvalidName);
    }

    #[test]
    fn validate_server_config_empty_command() {
        let config = make_stdio_config("test", "");
        let warnings = McpConfigValidator::validate_server_config("test", &config);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].kind, ValidationWarningKind::MissingCommand);
    }

    #[test]
    fn validate_server_config_invalid_url() {
        let config = make_http_config("test", "not-a-url");
        let warnings = McpConfigValidator::validate_server_config("test", &config);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].kind, ValidationWarningKind::InvalidUrl);
    }

    #[test]
    fn find_duplicates_detects_same_command() {
        let mut configs = HashMap::new();
        configs.insert("srv1".to_owned(), make_stdio_config("srv1", "npx"));
        configs.insert("srv2".to_owned(), make_stdio_config("srv2", "npx"));

        let dups = McpConfigValidator::find_duplicates(&configs);
        assert_eq!(dups.len(), 1);
        assert_eq!(dups[0].reason, "same stdio command: npx");
    }

    #[test]
    fn find_duplicates_no_duplicates() {
        let mut configs = HashMap::new();
        configs.insert("srv1".to_owned(), make_stdio_config("srv1", "npx"));
        configs.insert("srv2".to_owned(), make_stdio_config("srv2", "node"));

        let dups = McpConfigValidator::find_duplicates(&configs);
        assert!(dups.is_empty());
    }

    #[test]
    fn find_duplicates_detects_same_url() {
        let mut configs = HashMap::new();
        configs.insert(
            "srv1".to_owned(),
            make_http_config("srv1", "https://api.example.com"),
        );
        configs.insert(
            "srv2".to_owned(),
            make_http_config("srv2", "https://api.example.com"),
        );

        let dups = McpConfigValidator::find_duplicates(&configs);
        assert_eq!(dups.len(), 1);
        assert!(dups[0].reason.contains("same HTTP URL"));
    }

    #[test]
    fn check_command_safety_dangerous() {
        let warnings = McpConfigValidator::check_command_safety("rm", &[]);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].level, SecurityLevel::Critical);
    }

    #[test]
    fn check_command_safety_safe() {
        let warnings = McpConfigValidator::check_command_safety("node", &["server.js".to_owned()]);
        assert!(warnings.is_empty());
    }

    #[test]
    fn check_command_safety_path_traversal() {
        let warnings =
            McpConfigValidator::check_command_safety("cat", &["../../../etc/passwd".to_owned()]);
        assert!(
            warnings
                .iter()
                .any(|w| w.message.contains("path traversal"))
        );
    }

    #[test]
    fn check_command_safety_shell_expansion() {
        let warnings = McpConfigValidator::check_command_safety("echo", &["$(whoami)".to_owned()]);
        assert!(
            warnings
                .iter()
                .any(|w| w.message.contains("shell expansion"))
        );
    }

    #[test]
    fn security_level_serde_roundtrip() {
        let levels = vec![
            SecurityLevel::Info,
            SecurityLevel::Warning,
            SecurityLevel::Critical,
        ];
        for level in levels {
            let json = serde_json::to_string(&level).expect("serialize");
            let back: SecurityLevel = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, level);
        }
    }
}
