//! Shared JSON-RPC 2.0 types.
//!
//! These types are used by both [`RooCodeAdapter`](crate::adapters::RooCodeAdapter) and
//! [`CodexAdapter`](crate::adapters::CodexAdapter) for JSON-RPC 2.0 communication over
//! stdio. Extracting them into a shared module avoids duplication.

use serde::{Deserialize, Serialize};

/// JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    /// Error code.
    pub code: i64,
    /// Human-readable error message.
    pub message: String,
    /// Optional additional data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// JSON-RPC 2.0 request (host → agent).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    /// Protocol version — always `"2.0"`.
    pub jsonrpc: &'static str,
    /// Request identifier.
    pub id: u64,
    /// Method name.
    pub method: String,
    /// Method parameters.
    #[serde(default)]
    pub params: serde_json::Value,
}

/// JSON-RPC 2.0 response (agent → host).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonRpcResponse {
    /// Protocol version — always `"2.0"`.
    pub jsonrpc: String,
    /// Request identifier this response corresponds to.
    pub id: Option<u64>,
    /// Result payload (present on success).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// Error payload (present on failure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 notification (agent → host, no id).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    /// Protocol version — always `"2.0"`.
    pub jsonrpc: String,
    /// Notification method name.
    pub method: String,
    /// Notification parameters.
    #[serde(default)]
    pub params: serde_json::Value,
}

/// Envelope that can represent any incoming JSON-RPC message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcMessage {
    /// A response to a previously sent request.
    Response(JsonRpcResponse),
    /// An unsolicited notification from the agent.
    Notification(JsonRpcNotification),
}
