//! JSON-RPC types for MCP communication.
//!
//! Internal types used for serializing requests/notifications and deserializing
//! responses from MCP servers over stdio.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::{
    McpClientInfo, McpPeerInfo, McpPromptDescriptor, McpPromptGetResult, McpToolDescriptor,
};

/// A JSON-RPC request (with method and params).
#[derive(Debug, Serialize)]
pub(crate) struct JsonRpcRequest<T> {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: &'static str,
    pub params: T,
}

/// A JSON-RPC notification (no id, method and params).
#[derive(Debug, Serialize)]
pub(crate) struct JsonRpcNotification<T> {
    pub jsonrpc: &'static str,
    pub method: &'static str,
    pub params: T,
}

/// A JSON-RPC response envelope.
#[derive(Debug, Deserialize)]
pub(crate) struct JsonRpcEnvelope {
    #[serde(default)]
    pub id: Option<Value>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<JsonRpcErrorPayload>,
}

/// A JSON-RPC error payload.
#[derive(Debug, Deserialize)]
pub(crate) struct JsonRpcErrorPayload {
    pub code: i64,
    pub message: String,
}

/// Parameters for the MCP `initialize` method.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InitializeParams<'a> {
    pub protocol_version: &'a str,
    pub capabilities: Value,
    pub client_info: &'a McpClientInfo,
}

/// Result of the MCP `initialize` method.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpInitializeResult {
    pub protocol_version: String,
    #[serde(default)]
    pub capabilities: Value,
    #[serde(default)]
    pub server_info: Option<McpPeerInfo>,
    #[serde(default)]
    pub instructions: Option<String>,
}

/// Result of the MCP `tools/list` method.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpToolsListResult {
    #[serde(default)]
    pub tools: Vec<McpToolDescriptor>,
}

/// Parameters for the MCP `tools/call` method.
#[derive(Debug, Serialize)]
pub(crate) struct ToolCallParams<'a> {
    pub name: &'a str,
    pub arguments: Value,
}

/// Result of the MCP `prompts/list` method.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpPromptsListResult {
    #[serde(default)]
    pub prompts: Vec<McpPromptDescriptor>,
}

/// Parameters for the MCP `prompts/get` method.
#[derive(Debug, Serialize)]
pub(crate) struct PromptGetParams {
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

/// Result of the MCP `prompts/get` method.
pub(crate) type McpPromptGetRpcResult = McpPromptGetResult;

/// Result of the MCP `resources/list` method.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpResourcesListResult {
    #[serde(default)]
    pub resources: Vec<McpResourceEntry>,
}

/// A single resource returned by `resources/list`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpResourceEntry {
    pub uri: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
}

/// Parameters for the MCP `resources/read` method.
#[derive(Debug, Serialize)]
pub(crate) struct ResourceReadParams {
    pub uri: String,
}

/// A single content item returned by `resources/read`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpResourceContent {
    pub uri: String,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub blob: Option<String>,
}

/// Result of the MCP `resources/read` method.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpResourceReadResult {
    #[serde(default)]
    pub contents: Vec<McpResourceContent>,
}

/// Check whether a JSON-RPC response ID matches the expected request ID.
pub(crate) fn rpc_id_matches(id: &Value, request_id: u64) -> bool {
    id.as_u64() == Some(request_id)
        || id.as_i64() == Some(request_id as i64)
        || id
            .as_str()
            .is_some_and(|value| value == request_id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpc_id_matches_u64() {
        assert!(rpc_id_matches(&Value::from(1), 1));
        assert!(rpc_id_matches(&Value::from(42), 42));
    }

    #[test]
    fn rpc_id_matches_i64() {
        assert!(!rpc_id_matches(&Value::from(-1i64), 0));
        assert!(rpc_id_matches(&Value::from(2i64), 2));
    }

    #[test]
    fn rpc_id_matches_string() {
        assert!(rpc_id_matches(&Value::String("3".to_owned()), 3));
        assert!(!rpc_id_matches(&Value::String("abc".to_owned()), 3));
    }

    #[test]
    fn rpc_id_matches_null() {
        assert!(!rpc_id_matches(&Value::Null, 1));
    }

    #[test]
    fn envelope_deserialize_with_result() {
        let json = r#"{"id":1,"result":{"protocolVersion":"2025-03-26"}}"#;
        let env: JsonRpcEnvelope = serde_json::from_str(json).expect("deserialize");
        assert!(env.result.is_some());
        assert!(env.error.is_none());
    }

    #[test]
    fn envelope_deserialize_with_error() {
        let json = r#"{"id":2,"error":{"code":-32600,"message":"invalid"}}"#;
        let env: JsonRpcEnvelope = serde_json::from_str(json).expect("deserialize");
        assert!(env.result.is_none());
        let err = env.error.expect("should have error");
        assert_eq!(err.code, -32600);
        assert_eq!(err.message, "invalid");
    }

    #[test]
    fn initialize_result_deserialize() {
        let json = r#"{"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"test","version":"1.0"},"instructions":"Use wisely"}"#;
        let result: McpInitializeResult = serde_json::from_str(json).expect("deserialize");
        assert_eq!(result.protocol_version, "2025-03-26");
        assert_eq!(
            result.server_info.as_ref().map(|i| i.name.as_str()),
            Some("test")
        );
        assert_eq!(result.instructions.as_deref(), Some("Use wisely"));
    }

    #[test]
    fn tools_list_result_deserialize() {
        let json = r#"{"tools":[{"name":"search","inputSchema":{},"annotations":{}}]}"#;
        let result: McpToolsListResult = serde_json::from_str(json).expect("deserialize");
        assert_eq!(result.tools.len(), 1);
        assert_eq!(result.tools[0].name, "search");
    }

    #[test]
    fn prompts_list_result_deserialize() {
        let json = r#"{"prompts":[{"name":"plan","description":"Plan","arguments":[{"name":"topic","required":true}]}]}"#;
        let result: McpPromptsListResult = serde_json::from_str(json).expect("deserialize");
        assert_eq!(result.prompts.len(), 1);
        assert_eq!(result.prompts[0].name, "plan");
        assert_eq!(result.prompts[0].arguments[0].name, "topic");
        assert!(result.prompts[0].arguments[0].required);
    }
}
