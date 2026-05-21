//! CLI state serialization types.
//!
//! Provides types for serializing MCP client state to JSON for CLI output
//! and inter-process communication.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::config::McpCapabilityMatrix;
use crate::resources::ServerResource;
use crate::scope::ScopedMcpServerConfig;

/// A serialized tool for CLI output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SerializedTool {
    /// Tool name (fully qualified or local).
    pub name: String,
    /// Tool description.
    pub description: String,
    /// JSON Schema for tool input.
    #[serde(default)]
    pub input_json_schema: Option<serde_json::Value>,
    /// Whether this tool comes from an MCP server.
    #[serde(default)]
    pub is_mcp: Option<bool>,
    /// Original tool name before normalization.
    #[serde(default)]
    pub original_tool_name: Option<String>,
}

/// A serialized MCP client for CLI output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SerializedClient {
    /// Client/server name.
    pub name: String,
    /// Connection type string.
    pub connection_type: String,
    /// Negotiated capabilities (if connected).
    #[serde(default)]
    pub capabilities: Option<McpCapabilityMatrix>,
}

/// Full MCP CLI state for serialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct McpCliState {
    /// Connected/known clients.
    #[serde(default)]
    pub clients: Vec<SerializedClient>,
    /// Server configurations keyed by name.
    #[serde(default)]
    pub configs: HashMap<String, ScopedMcpServerConfig>,
    /// Available tools.
    #[serde(default)]
    pub tools: Vec<SerializedTool>,
    /// Resources keyed by server name.
    #[serde(default)]
    pub resources: HashMap<String, Vec<ServerResource>>,
    /// Normalized name mapping (original → normalized).
    #[serde(default)]
    pub normalized_names: Option<HashMap<String, String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialized_tool_minimal() {
        let tool = SerializedTool {
            name: "search".to_owned(),
            description: "Search docs".to_owned(),
            input_json_schema: None,
            is_mcp: Some(true),
            original_tool_name: None,
        };
        let json = serde_json::to_string(&tool).expect("serialize");
        let back: SerializedTool = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.name, "search");
        assert_eq!(back.is_mcp, Some(true));
    }

    #[test]
    fn serialized_client_connection_types() {
        for ct in &["connected", "failed", "needs-auth", "pending", "disabled"] {
            let client = SerializedClient {
                name: "test".to_owned(),
                connection_type: ct.to_string(),
                capabilities: None,
            };
            let json = serde_json::to_string(&client).expect("serialize");
            let back: SerializedClient = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back.connection_type, *ct);
        }
    }

    #[test]
    fn mcp_cli_state_default_is_empty() {
        let state = McpCliState::default();
        assert!(state.clients.is_empty());
        assert!(state.configs.is_empty());
        assert!(state.tools.is_empty());
        assert!(state.resources.is_empty());
        assert!(state.normalized_names.is_none());
    }

    #[test]
    fn mcp_cli_state_serde_roundtrip() {
        let state = McpCliState {
            clients: vec![SerializedClient {
                name: "srv".to_owned(),
                connection_type: "connected".to_owned(),
                capabilities: Some(McpCapabilityMatrix {
                    supports_tools: true,
                    ..McpCapabilityMatrix::default()
                }),
            }],
            configs: HashMap::new(),
            tools: vec![SerializedTool {
                name: "t".to_owned(),
                description: "d".to_owned(),
                input_json_schema: None,
                is_mcp: None,
                original_tool_name: None,
            }],
            resources: HashMap::new(),
            normalized_names: Some(HashMap::from([("a b".to_owned(), "a_b".to_owned())])),
        };
        let json = serde_json::to_string(&state).expect("serialize");
        let back: McpCliState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.clients.len(), 1);
        assert_eq!(back.tools.len(), 1);
        assert_eq!(
            back.normalized_names
                .as_ref()
                .map(|m| m.get("a b").cloned()),
            Some(Some("a_b".to_owned()))
        );
    }

    #[test]
    fn serialized_tool_with_schema() {
        let tool = SerializedTool {
            name: "fetch".to_owned(),
            description: "Fetch URL".to_owned(),
            input_json_schema: Some(serde_json::json!({"type": "object"})),
            is_mcp: Some(true),
            original_tool_name: Some("original_fetch".to_owned()),
        };
        assert!(tool.input_json_schema.is_some());
        assert_eq!(tool.original_tool_name.as_deref(), Some("original_fetch"));
    }
}
