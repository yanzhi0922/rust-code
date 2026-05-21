//! Configuration scope for MCP servers.
//!
//! Defines where a particular MCP server configuration originates from,
//! following the precedence model used by the reference implementation.

use serde::{Deserialize, Serialize};

use crate::config::McpServerConfig;

/// The source scope of an MCP server configuration.
///
/// Scopes are ordered by precedence (highest first):
/// `Managed > Enterprise > Claudeai > Dynamic > Project > User > Local`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigScope {
    /// Local project-level configuration (e.g. `.mcp.toml`).
    Local,
    /// User-level configuration (`~/.config/remote-code/mcp.toml`).
    User,
    /// Project-level configuration from workspace settings.
    Project,
    /// Dynamically discovered configuration (e.g. from plugins).
    Dynamic,
    /// Enterprise-managed configuration.
    Enterprise,
    /// Claude.ai-specific configuration scope.
    Claudeai,
    /// Managed runtime configuration.
    Managed,
}

impl ConfigScope {
    /// Return the precedence rank (higher = more authoritative).
    #[must_use]
    pub const fn precedence(self) -> u8 {
        match self {
            Self::Local => 0,
            Self::User => 1,
            Self::Project => 2,
            Self::Dynamic => 3,
            Self::Enterprise => 4,
            Self::Claudeai => 5,
            Self::Managed => 6,
        }
    }

    /// Return the kebab-case string representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::User => "user",
            Self::Project => "project",
            Self::Dynamic => "dynamic",
            Self::Enterprise => "enterprise",
            Self::Claudeai => "claudeai",
            Self::Managed => "managed",
        }
    }
}

impl std::fmt::Display for ConfigScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An MCP server configuration annotated with its scope and optional plugin source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopedMcpServerConfig {
    /// The inner server configuration.
    pub inner: McpServerConfig,
    /// The scope from which this configuration originates.
    pub scope: ConfigScope,
    /// If this config was loaded from a plugin, the plugin identifier.
    #[serde(default)]
    pub plugin_source: Option<String>,
}

impl ScopedMcpServerConfig {
    /// Create a new scoped configuration.
    #[must_use]
    pub fn new(inner: McpServerConfig, scope: ConfigScope) -> Self {
        Self {
            inner,
            scope,
            plugin_source: None,
        }
    }

    /// Attach a plugin source to this scoped configuration.
    #[must_use]
    pub fn with_plugin_source(mut self, source: impl Into<String>) -> Self {
        self.plugin_source = Some(source.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_precedence_ordering() {
        assert!(ConfigScope::Managed.precedence() > ConfigScope::Enterprise.precedence());
        assert!(ConfigScope::Claudeai.precedence() > ConfigScope::Enterprise.precedence());
        assert!(ConfigScope::Claudeai.precedence() > ConfigScope::Dynamic.precedence());
        assert!(ConfigScope::Dynamic.precedence() > ConfigScope::Project.precedence());
        assert!(ConfigScope::Project.precedence() > ConfigScope::User.precedence());
        assert!(ConfigScope::User.precedence() > ConfigScope::Local.precedence());
    }

    #[test]
    fn scope_as_str_kebab_case() {
        assert_eq!(ConfigScope::Local.as_str(), "local");
        assert_eq!(ConfigScope::Claudeai.as_str(), "claudeai");
        assert_eq!(ConfigScope::Managed.as_str(), "managed");
    }

    #[test]
    fn scope_display_matches_as_str() {
        assert_eq!(ConfigScope::Project.to_string(), "project");
        assert_eq!(ConfigScope::Enterprise.to_string(), "enterprise");
    }

    #[test]
    fn scope_serde_roundtrip() {
        for scope in [
            ConfigScope::Local,
            ConfigScope::User,
            ConfigScope::Project,
            ConfigScope::Dynamic,
            ConfigScope::Enterprise,
            ConfigScope::Claudeai,
            ConfigScope::Managed,
        ] {
            let json = serde_json::to_string(&scope).expect("serialize");
            let back: ConfigScope = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(scope, back, "roundtrip failed for {scope:?}");
        }
    }

    #[test]
    fn scoped_config_new_and_with_plugin() {
        let inner = McpServerConfig {
            name: "test".to_owned(),
            enabled: true,
            transport: crate::transport::McpTransportConfig::Stdio {
                command: "echo".to_owned(),
                args: vec![],
                cwd: None,
                env: std::collections::BTreeMap::new(),
            },
            capabilities: crate::config::McpCapabilityMatrix::default(),
            startup_timeout_secs: None,
            request_timeout_secs: None,
            metadata: std::collections::BTreeMap::new(),
            oauth: None,
            tool_policy: crate::tool_policy::McpToolPolicy::default(),
        };
        let scoped = ScopedMcpServerConfig::new(inner.clone(), ConfigScope::User)
            .with_plugin_source("my-plugin");
        assert_eq!(scoped.scope, ConfigScope::User);
        assert_eq!(scoped.plugin_source.as_deref(), Some("my-plugin"));
        assert_eq!(scoped.inner.name, "test");
    }
}
