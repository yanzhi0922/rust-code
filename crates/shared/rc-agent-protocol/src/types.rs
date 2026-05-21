//! Core type definitions for the Agent protocol.
//!
//! Contains [`AgentType`], [`AgentStatus`], [`AgentCapability`], [`AgentInfo`],
//! and [`AgentConfig`] — the fundamental building blocks used across all
//! adapters and the router.

use std::collections::HashSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Supported Agent types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentType {
    /// In-process Agent — directly calls rc-* crates via callbacks.
    RemoteClaude,
    /// In-process Agent — callback-based adapter (formerly subprocess JSON-RPC).
    RemoteRoo,
    /// In-process Agent — callback-based adapter (formerly subprocess NDJSON).
    RemoteCodex,
}

impl AgentType {
    /// Returns a human-readable display name.
    pub fn display_name(&self) -> &str {
        match self {
            Self::RemoteClaude => "Remote Claude",
            Self::RemoteRoo => "Remote Roo",
            Self::RemoteCodex => "Remote Codex",
        }
    }
}

impl std::fmt::Display for AgentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.display_name())
    }
}

/// Runtime status of an Agent instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    /// Agent is being initialized.
    Starting,
    /// Agent is ready to accept messages.
    Ready,
    /// Agent is currently processing a request.
    Busy,
    /// Agent is idle, waiting for new requests.
    Idle,
    /// Agent has been stopped (gracefully or due to error).
    Stopped,
    /// Agent encountered an unrecoverable error.
    Error,
}

impl std::fmt::Display for AgentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Busy => "busy",
            Self::Idle => "idle",
            Self::Stopped => "stopped",
            Self::Error => "error",
        }
        .fmt(f)
    }
}

/// Capabilities that an Agent may support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentCapability {
    /// Supports streaming text deltas.
    Streaming,
    /// Can invoke tools and return results.
    ToolUse,
    /// Supports MCP (Model Context Protocol) integration.
    McpSupport,
    /// Can spawn and manage subtasks.
    Subtasks,
    /// Requires explicit permission for certain operations.
    Permissions,
}

/// Static information about an Agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    /// Human-readable Agent name.
    pub name: String,
    /// Agent version string (e.g. `"0.1.0"`).
    pub version: String,
    /// Set of capabilities this Agent supports.
    pub capabilities: HashSet<AgentCapability>,
    /// Current Agent status.
    pub status: AgentStatus,
}

/// Configuration for creating and starting an Agent instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// The type of Agent to create.
    pub agent_type: AgentType,
    /// Path to the Agent binary (ignored for in-process Agents).
    #[serde(default)]
    pub binary_path: Option<PathBuf>,
    /// Command-line arguments passed to the Agent binary.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables forwarded to the Agent process.
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// Working directory for the Agent process.
    #[serde(default)]
    pub working_dir: Option<PathBuf>,
    /// Optional model override.
    #[serde(default)]
    pub model: Option<String>,
    /// Optional provider override.
    #[serde(default)]
    pub provider: Option<String>,
    /// Optional API key (passed via environment variable to sub-processes).
    ///
    /// # Security
    ///
    /// This field is **not** serialized to prevent accidental leakage of API
    /// keys in logs, debug output, or persisted configurations.
    #[serde(default, skip_serializing)]
    pub api_key: Option<String>,
    /// Optional base URL override for the provider API.
    #[serde(default)]
    pub base_url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_type_serde_roundtrip() {
        let types = [
            AgentType::RemoteClaude,
            AgentType::RemoteRoo,
            AgentType::RemoteCodex,
        ];
        for t in &types {
            let json = serde_json::to_string(t).expect("serialize");
            let back: AgentType = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*t, back, "roundtrip failed for {t:?}");
        }
    }

    #[test]
    fn agent_type_serde_values() {
        assert_eq!(
            serde_json::to_string(&AgentType::RemoteClaude).expect("serialize"),
            "\"remote_claude\""
        );
        assert_eq!(
            serde_json::to_string(&AgentType::RemoteRoo).expect("serialize"),
            "\"remote_roo\""
        );
        assert_eq!(
            serde_json::to_string(&AgentType::RemoteCodex).expect("serialize"),
            "\"remote_codex\""
        );
    }

    #[test]
    fn agent_status_serde_roundtrip() {
        let statuses = [
            AgentStatus::Starting,
            AgentStatus::Ready,
            AgentStatus::Busy,
            AgentStatus::Idle,
            AgentStatus::Stopped,
            AgentStatus::Error,
        ];
        for s in &statuses {
            let json = serde_json::to_string(s).expect("serialize");
            let back: AgentStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*s, back, "roundtrip failed for {s:?}");
        }
    }

    #[test]
    fn agent_capability_set_serde() {
        let mut caps = HashSet::new();
        caps.insert(AgentCapability::Streaming);
        caps.insert(AgentCapability::ToolUse);
        caps.insert(AgentCapability::McpSupport);

        let json = serde_json::to_string(&caps).expect("serialize");
        let back: HashSet<AgentCapability> = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(caps, back);
    }

    #[test]
    fn agent_info_serde() {
        let mut caps = HashSet::new();
        caps.insert(AgentCapability::Streaming);
        caps.insert(AgentCapability::ToolUse);

        let info = AgentInfo {
            name: "Remote Claude".into(),
            version: "0.1.0".into(),
            capabilities: caps.clone(),
            status: AgentStatus::Ready,
        };

        let json = serde_json::to_string(&info).expect("serialize");
        let back: AgentInfo = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(info.name, back.name);
        assert_eq!(info.version, back.version);
        assert_eq!(info.status, back.status);
        assert_eq!(caps, back.capabilities);
    }

    #[test]
    fn agent_config_serde() {
        let config = AgentConfig {
            agent_type: AgentType::RemoteRoo,
            binary_path: Some(PathBuf::from("/usr/local/bin/roo-server")),
            args: vec!["--verbose".into()],
            env: vec![("ROO_API_KEY".into(), "sk-test".into())],
            working_dir: Some(PathBuf::from("/home/user/project")),
            model: Some("claude-4-sonnet".into()),
            provider: Some("anthropic".into()),
            api_key: Some("sk-test-key".into()),
            base_url: None,
        };

        let json = serde_json::to_string(&config).expect("serialize");
        let back: AgentConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(config.agent_type, back.agent_type);
        assert_eq!(config.binary_path, back.binary_path);
        assert_eq!(config.args, back.args);
        assert_eq!(config.model, back.model);
    }

    #[test]
    fn agent_type_display() {
        assert_eq!(AgentType::RemoteClaude.to_string(), "Remote Claude");
        assert_eq!(AgentType::RemoteRoo.to_string(), "Remote Roo");
        assert_eq!(AgentType::RemoteCodex.to_string(), "Remote Codex");
    }

    #[test]
    fn agent_status_display() {
        assert_eq!(AgentStatus::Starting.to_string(), "starting");
        assert_eq!(AgentStatus::Ready.to_string(), "ready");
        assert_eq!(AgentStatus::Busy.to_string(), "busy");
        assert_eq!(AgentStatus::Idle.to_string(), "idle");
        assert_eq!(AgentStatus::Stopped.to_string(), "stopped");
        assert_eq!(AgentStatus::Error.to_string(), "error");
    }
}
