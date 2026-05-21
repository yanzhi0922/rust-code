//! Core MCP type definitions for client info, tool descriptors, and server inspection.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::resources::ServerResource;

// ---------------------------------------------------------------------------
// Tool result truncation
// ---------------------------------------------------------------------------

/// Default maximum number of characters allowed in MCP tool result content
/// before truncation is applied.
const DEFAULT_MCP_TOOL_RESULT_MAX_CHARS: usize = 50_000;

/// Environment variable name for configuring the maximum MCP output size in
/// tokens. When set, the value is multiplied by 4 to estimate a character
/// budget (matching the TS `mcpValidation.ts:77-79` approach).
const MAX_MCP_OUTPUT_TOKENS_ENV: &str = "MAX_MCP_OUTPUT_TOKENS";

/// Return the effective maximum character budget for MCP tool results.
///
/// If the `MAX_MCP_OUTPUT_TOKENS` environment variable is set, its value is
/// parsed as a `usize` and multiplied by 4 to produce a character budget.
/// Otherwise the default of 50,000 characters is used.
#[must_use]
pub fn mcp_tool_result_max_chars() -> usize {
    if let Ok(val) = std::env::var(MAX_MCP_OUTPUT_TOKENS_ENV)
        && let Ok(tokens) = val.parse::<usize>()
    {
        return tokens.saturating_mul(4);
    }
    DEFAULT_MCP_TOOL_RESULT_MAX_CHARS
}

/// Maximum number of characters allowed in MCP tool result content before
/// truncation is applied. Large results are truncated to prevent context
/// window overflow and excessive memory usage.
///
/// NOTE: Prefer using [`mcp_tool_result_max_chars()`] which respects the
/// `MAX_MCP_OUTPUT_TOKENS` environment variable. This constant is kept for
/// backward compatibility.
pub const MCP_TOOL_RESULT_MAX_CHARS: usize = 50_000;

/// Truncation notice appended to tool results that exceed the character limit.
pub const MCP_TOOL_RESULT_TRUNCATION_NOTICE: &str =
    "\n\n[Output truncated: exceeded 50,000 character limit]";

/// Truncate a tool result string if it exceeds the configured character budget.
///
/// Uses [`mcp_tool_result_max_chars()`] to determine the effective limit, which
/// respects the `MAX_MCP_OUTPUT_TOKENS` environment variable.
///
/// If the content exceeds the limit, it is sliced to the budget minus the
/// notice length, then the truncation notice is appended.
#[must_use]
pub fn truncate_tool_result_content(content: &str) -> String {
    let max_chars = mcp_tool_result_max_chars();
    if content.len() <= max_chars {
        return content.to_owned();
    }
    let truncate_at = max_chars.saturating_sub(MCP_TOOL_RESULT_TRUNCATION_NOTICE.len());
    let mut truncated = content.chars().take(truncate_at).collect::<String>();
    truncated.push_str(MCP_TOOL_RESULT_TRUNCATION_NOTICE);
    truncated
}

/// Truncate all text content blocks in a [`McpToolCallResult`] that exceed
/// the character limit.
///
/// This modifies the result in-place by replacing oversized text content
/// with a truncated version plus the notice.
pub fn truncate_tool_call_result(result: &mut McpToolCallResult) {
    let max_chars = mcp_tool_result_max_chars();

    // Truncate the legacy tool_result field.
    if let Some(Value::String(s)) = &result.tool_result
        && s.len() > max_chars
    {
        result.tool_result = Some(Value::String(truncate_tool_result_content(s)));
    }

    // Truncate text content blocks.
    for content in &mut result.content {
        if content.kind == "text"
            && let Some(Value::String(s)) = content.fields.get_mut("text")
            && s.len() > max_chars
        {
            *s = truncate_tool_result_content(s);
        }
    }
}

/// Client identification sent during MCP initialisation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpClientInfo {
    /// Client name.
    pub name: String,
    /// Client version.
    pub version: String,
}

impl McpClientInfo {
    /// Create a new client info with the given name and version.
    #[must_use]
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
        }
    }
}

impl Default for McpClientInfo {
    fn default() -> Self {
        Self::new("remote-code-rust", env!("CARGO_PKG_VERSION"))
    }
}

/// Peer (server) identification returned during MCP initialisation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpPeerInfo {
    /// Server name.
    pub name: String,
    /// Human-readable title.
    #[serde(default)]
    pub title: Option<String>,
    /// Server version.
    #[serde(default)]
    pub version: Option<String>,
}

/// A tool descriptor returned by an MCP server via `tools/list`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolDescriptor {
    /// Tool name (unique within the server).
    pub name: String,
    /// Human-readable title.
    #[serde(default)]
    pub title: Option<String>,
    /// Tool description.
    #[serde(default)]
    pub description: Option<String>,
    /// JSON Schema for tool input.
    #[serde(default)]
    pub input_schema: Value,
    /// Tool annotations (e.g. `readOnlyHint`).
    #[serde(default)]
    pub annotations: Value,
}

impl<'de> Deserialize<'de> for McpToolDescriptor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct RawMcpToolDescriptor {
            name: String,
            #[serde(default)]
            title: Option<String>,
            #[serde(default)]
            description: Option<String>,
            #[serde(default)]
            input_schema: Value,
            #[serde(default)]
            annotations: Value,
            #[serde(default, rename = "_meta")]
            meta: Value,
        }

        let raw = RawMcpToolDescriptor::deserialize(deserializer)?;
        let annotations = match (raw.annotations, raw.meta) {
            (Value::Object(mut annotations), Value::Object(meta)) if !meta.is_empty() => {
                annotations.insert("_meta".to_owned(), Value::Object(meta));
                Value::Object(annotations)
            }
            (annotations, _) => annotations,
        };
        Ok(Self {
            name: raw.name,
            title: raw.title,
            description: raw.description,
            input_schema: raw.input_schema,
            annotations,
        })
    }
}

/// A prompt argument descriptor returned by an MCP server via `prompts/list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpPromptArgument {
    /// Argument name.
    pub name: String,
    /// Human-readable title.
    #[serde(default)]
    pub title: Option<String>,
    /// Argument description.
    #[serde(default)]
    pub description: Option<String>,
    /// Whether the argument is required.
    #[serde(default)]
    pub required: bool,
}

/// A prompt descriptor returned by an MCP server via `prompts/list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpPromptDescriptor {
    /// Prompt name.
    pub name: String,
    /// Human-readable title.
    #[serde(default)]
    pub title: Option<String>,
    /// Prompt description.
    #[serde(default)]
    pub description: Option<String>,
    /// Prompt arguments.
    #[serde(default)]
    pub arguments: Vec<McpPromptArgument>,
}

/// A prompt message returned by `prompts/get`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpPromptMessage {
    /// Message role.
    pub role: String,
    /// MCP content block payload.
    pub content: Value,
}

/// Result payload from `prompts/get`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpPromptGetResult {
    /// Optional description from the server.
    #[serde(default)]
    pub description: Option<String>,
    /// Prompt messages.
    #[serde(default)]
    pub messages: Vec<McpPromptMessage>,
}

/// Full response from a `prompts/get` invocation including server metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpPromptGetResponse {
    /// Server name from the config key.
    pub server_name: String,
    /// Prompt name that was invoked.
    pub prompt_name: String,
    /// Negotiated protocol version.
    pub protocol_version: String,
    /// Server identification.
    #[serde(default)]
    pub server_info: Option<McpPeerInfo>,
    /// Prompt result.
    pub result: McpPromptGetResult,
}

/// Full inspection result from an MCP server (initialize + tools/list + prompts/list + resources/list).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerInspection {
    /// Server name from the config key.
    pub server_name: String,
    /// Negotiated protocol version.
    pub protocol_version: String,
    /// Server identification.
    #[serde(default)]
    pub server_info: Option<McpPeerInfo>,
    /// Server capabilities (raw JSON).
    #[serde(default)]
    pub capabilities: Value,
    /// Server instructions for the client.
    #[serde(default)]
    pub instructions: Option<String>,
    /// Available tools.
    #[serde(default)]
    pub tools: Vec<McpToolDescriptor>,
    /// Available prompts.
    #[serde(default)]
    pub prompts: Vec<McpPromptDescriptor>,
    /// Available resources.
    #[serde(default)]
    pub resources: Vec<ServerResource>,
}

/// A single content block in a tool call result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpToolCallContent {
    /// Content block type (e.g. `"text"`, `"image"`).
    #[serde(rename = "type")]
    pub kind: String,
    /// Additional fields for the content block.
    #[serde(flatten)]
    pub fields: BTreeMap<String, Value>,
}

/// The result payload of a `tools/call` invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolCallResult {
    /// Legacy/simple MCP result field. Claude Code gives this precedence over
    /// structuredContent and content when servers return it.
    #[serde(default)]
    pub tool_result: Option<Value>,
    /// Content blocks returned by the tool.
    #[serde(default)]
    pub content: Vec<McpToolCallContent>,
    /// Optional structured content (JSON).
    #[serde(default)]
    pub structured_content: Option<Value>,
    /// Whether the tool invocation resulted in an error.
    #[serde(default)]
    pub is_error: bool,
}

/// Full response from a `tools/call` invocation including server metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolCallResponse {
    /// Server name from the config key.
    pub server_name: String,
    /// Tool name that was invoked.
    pub tool_name: String,
    /// Negotiated protocol version.
    pub protocol_version: String,
    /// Server identification.
    #[serde(default)]
    pub server_info: Option<McpPeerInfo>,
    /// Tool call result.
    pub result: McpToolCallResult,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_info_default_uses_crate_name() {
        let info = McpClientInfo::default();
        assert_eq!(info.name, "remote-code-rust");
        assert!(!info.version.is_empty());
    }

    #[test]
    fn client_info_new_custom() {
        let info = McpClientInfo::new("my-app", "1.0.0");
        assert_eq!(info.name, "my-app");
        assert_eq!(info.version, "1.0.0");
    }

    #[test]
    fn peer_info_serialization_roundtrip() {
        let peer = McpPeerInfo {
            name: "test-server".to_owned(),
            title: Some("Test Server".to_owned()),
            version: Some("2.0".to_owned()),
        };
        let json = serde_json::to_string(&peer).expect("serialize");
        let back: McpPeerInfo = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(peer, back);
    }

    #[test]
    fn tool_descriptor_deserialization() {
        let json = r#"{"name":"search","description":"Search","inputSchema":{},"annotations":{}}"#;
        let tool: McpToolDescriptor = serde_json::from_str(json).expect("deserialize");
        assert_eq!(tool.name, "search");
        assert_eq!(tool.description.as_deref(), Some("Search"));
    }

    #[test]
    fn tool_descriptor_preserves_meta_in_annotations() {
        let json = r#"{"name":"search","inputSchema":{},"annotations":{"readOnlyHint":true},"_meta":{"anthropic/searchHint":"docs lookup","anthropic/alwaysLoad":true}}"#;
        let tool: McpToolDescriptor = serde_json::from_str(json).expect("deserialize");
        assert_eq!(tool.annotations["readOnlyHint"], true);
        assert_eq!(
            tool.annotations["_meta"]["anthropic/searchHint"],
            "docs lookup"
        );
        assert_eq!(tool.annotations["_meta"]["anthropic/alwaysLoad"], true);
    }

    #[test]
    fn tool_call_content_kind_and_fields() {
        let content = McpToolCallContent {
            kind: "text".to_owned(),
            fields: BTreeMap::from([("text".to_owned(), Value::String("hello".to_owned()))]),
        };
        let json = serde_json::to_string(&content).expect("serialize");
        assert!(json.contains("\"type\":\"text\""));
        assert!(json.contains("\"text\":\"hello\""));
    }

    #[test]
    fn tool_call_result_is_error_default_false() {
        let result = McpToolCallResult {
            tool_result: None,
            content: vec![],
            structured_content: None,
            is_error: false,
        };
        assert!(!result.is_error);
    }

    #[test]
    fn server_inspection_serialization() {
        let inspection = McpServerInspection {
            server_name: "test".to_owned(),
            protocol_version: "2025-03-26".to_owned(),
            server_info: None,
            capabilities: serde_json::json!({}),
            instructions: Some("Use carefully".to_owned()),
            tools: vec![],
            prompts: vec![],
            resources: vec![],
        };
        let json = serde_json::to_string(&inspection).expect("serialize");
        let back: McpServerInspection = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(inspection, back);
    }

    #[test]
    fn server_inspection_deserializes_without_resources_for_compatibility() {
        let json = r#"{"serverName":"test","protocolVersion":"2025-03-26","tools":[]}"#;
        let inspection: McpServerInspection = serde_json::from_str(json).expect("deserialize");
        assert!(inspection.resources.is_empty());
        assert!(inspection.prompts.is_empty());
    }

    #[test]
    fn mcp_tool_result_max_chars_default() {
        // Ensure the env var is not set for this test.
        unsafe {
            std::env::remove_var("MAX_MCP_OUTPUT_TOKENS");
        }
        assert_eq!(super::mcp_tool_result_max_chars(), 50_000);
    }

    #[test]
    fn truncate_tool_result_content_uses_env_var() {
        // Set the env var to 100 tokens => 400 chars budget.
        unsafe {
            std::env::set_var("MAX_MCP_OUTPUT_TOKENS", "100");
        }
        let long = "a".repeat(500);
        let truncated = super::truncate_tool_result_content(&long);
        assert!(truncated.len() <= 500);
        assert!(truncated.contains("[Output truncated"));
        // Clean up.
        unsafe {
            std::env::remove_var("MAX_MCP_OUTPUT_TOKENS");
        }
    }

    #[test]
    fn truncate_tool_result_content_short_passthrough() {
        unsafe {
            std::env::remove_var("MAX_MCP_OUTPUT_TOKENS");
        }
        let short = "hello";
        assert_eq!(super::truncate_tool_result_content(short), short);
    }
}
