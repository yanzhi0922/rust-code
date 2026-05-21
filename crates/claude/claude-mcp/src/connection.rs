//! MCP server connection state machine.
//!
//! Models the lifecycle of an MCP server connection with five states:
//! connected, failed, needs-auth, pending, and disabled.

use serde::{Deserialize, Serialize};

use crate::config::McpCapabilityMatrix;
use crate::scope::ScopedMcpServerConfig;
use crate::types::McpPeerInfo;

/// Server identification returned during MCP initialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerInfo {
    /// Server name.
    pub name: String,
    /// Server version.
    #[serde(default)]
    pub version: Option<String>,
}

/// MCP server connection state.
///
/// Each variant captures the relevant data for that state, including
/// the scoped configuration that led to the connection attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum McpServerConnection {
    /// Successfully connected and initialized.
    #[serde(rename = "connected")]
    Connected(ConnectedServer),
    /// Connection or initialization failed.
    #[serde(rename = "failed")]
    Failed(FailedServer),
    /// Server requires authentication (e.g. OAuth).
    #[serde(rename = "needs-auth")]
    NeedsAuth(NeedsAuthServer),
    /// Connection is pending (not yet attempted or reconnecting).
    #[serde(rename = "pending")]
    Pending(PendingServer),
    /// Server is explicitly disabled.
    #[serde(rename = "disabled")]
    Disabled(DisabledServer),
}

impl McpServerConnection {
    /// Return the server name regardless of state.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Connected(s) => &s.name,
            Self::Failed(s) => &s.name,
            Self::NeedsAuth(s) => &s.name,
            Self::Pending(s) => &s.name,
            Self::Disabled(s) => &s.name,
        }
    }

    /// Return the connection type as a string.
    #[must_use]
    pub fn connection_type(&self) -> &'static str {
        match self {
            Self::Connected(_) => "connected",
            Self::Failed(_) => "failed",
            Self::NeedsAuth(_) => "needs-auth",
            Self::Pending(_) => "pending",
            Self::Disabled(_) => "disabled",
        }
    }

    /// Return true if this connection is in the Connected state.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        matches!(self, Self::Connected(_))
    }
}

/// A successfully connected MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectedServer {
    /// Server name.
    pub name: String,
    /// Negotiated capabilities.
    pub capabilities: McpCapabilityMatrix,
    /// Server identification from initialization.
    pub server_info: Option<ServerInfo>,
    /// Server instructions for the client.
    pub instructions: Option<String>,
    /// The scoped configuration that established this connection.
    pub config: ScopedMcpServerConfig,
}

/// A failed MCP server connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailedServer {
    /// Server name.
    pub name: String,
    /// The scoped configuration that was used.
    pub config: ScopedMcpServerConfig,
    /// Error message describing the failure.
    #[serde(default)]
    pub error: Option<String>,
}

/// An MCP server that requires authentication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeedsAuthServer {
    /// Server name.
    pub name: String,
    /// The scoped configuration that requires auth.
    pub config: ScopedMcpServerConfig,
}

/// A pending MCP server connection (not yet attempted or reconnecting).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingServer {
    /// Server name.
    pub name: String,
    /// The scoped configuration for this server.
    pub config: ScopedMcpServerConfig,
    /// Current reconnect attempt number (0 = initial attempt).
    #[serde(default)]
    pub reconnect_attempt: Option<u32>,
    /// Maximum number of reconnect attempts.
    #[serde(default)]
    pub max_reconnect_attempts: Option<u32>,
}

/// A disabled MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisabledServer {
    /// Server name.
    pub name: String,
    /// The scoped configuration (disabled).
    pub config: ScopedMcpServerConfig,
}

/// Convert from `McpPeerInfo` to `ServerInfo`.
impl From<McpPeerInfo> for ServerInfo {
    fn from(peer: McpPeerInfo) -> Self {
        Self {
            name: peer.name,
            version: peer.version,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{McpCapabilityMatrix, McpServerConfig};
    use crate::scope::ConfigScope;
    use crate::transport::McpTransportConfig;
    use std::collections::BTreeMap;

    fn test_scoped_config() -> ScopedMcpServerConfig {
        ScopedMcpServerConfig::new(
            McpServerConfig {
                name: "test".to_owned(),
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
                tool_policy: crate::tool_policy::McpToolPolicy::default(),
            },
            ConfigScope::Local,
        )
    }

    #[test]
    fn connected_state_name_and_type() {
        let conn = McpServerConnection::Connected(ConnectedServer {
            name: "my-server".to_owned(),
            capabilities: McpCapabilityMatrix::default(),
            server_info: None,
            instructions: None,
            config: test_scoped_config(),
        });
        assert_eq!(conn.name(), "my-server");
        assert_eq!(conn.connection_type(), "connected");
        assert!(conn.is_connected());
    }

    #[test]
    fn failed_state_name_and_type() {
        let conn = McpServerConnection::Failed(FailedServer {
            name: "bad-server".to_owned(),
            config: test_scoped_config(),
            error: Some("connection refused".to_owned()),
        });
        assert_eq!(conn.name(), "bad-server");
        assert_eq!(conn.connection_type(), "failed");
        assert!(!conn.is_connected());
    }

    #[test]
    fn needs_auth_state() {
        let conn = McpServerConnection::NeedsAuth(NeedsAuthServer {
            name: "auth-server".to_owned(),
            config: test_scoped_config(),
        });
        assert_eq!(conn.connection_type(), "needs-auth");
    }

    #[test]
    fn pending_state_with_reconnect() {
        let conn = McpServerConnection::Pending(PendingServer {
            name: "slow-server".to_owned(),
            config: test_scoped_config(),
            reconnect_attempt: Some(3),
            max_reconnect_attempts: Some(5),
        });
        assert_eq!(conn.connection_type(), "pending");
    }

    #[test]
    fn disabled_state() {
        let conn = McpServerConnection::Disabled(DisabledServer {
            name: "off-server".to_owned(),
            config: test_scoped_config(),
        });
        assert_eq!(conn.connection_type(), "disabled");
    }

    #[test]
    fn server_info_from_peer_info() {
        let peer = McpPeerInfo {
            name: "test".to_owned(),
            title: None,
            version: Some("1.0".to_owned()),
        };
        let info = ServerInfo::from(peer);
        assert_eq!(info.name, "test");
        assert_eq!(info.version.as_deref(), Some("1.0"));
    }

    #[test]
    fn connection_serde_roundtrip() {
        let conn = McpServerConnection::Connected(ConnectedServer {
            name: "serde-test".to_owned(),
            capabilities: McpCapabilityMatrix {
                supports_tools: true,
                ..McpCapabilityMatrix::default()
            },
            server_info: Some(ServerInfo {
                name: "test".to_owned(),
                version: Some("2.0".to_owned()),
            }),
            instructions: Some("Be careful".to_owned()),
            config: test_scoped_config(),
        });
        let json = serde_json::to_string(&conn).expect("serialize");
        let back: McpServerConnection = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.name(), "serde-test");
        assert_eq!(back.connection_type(), "connected");
    }
}
