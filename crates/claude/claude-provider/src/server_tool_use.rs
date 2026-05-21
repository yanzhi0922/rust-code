//! Server Tool Use blocks for server-side tools like web search.
//!
//! Handles parsing of `server_tool_use` and `server_tool_result` content blocks
//! that represent tools executed by the API server rather than the client.
//!
//! Based on upstream Claude Code's server tool use handling.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Block types
// ---------------------------------------------------------------------------

/// A server-side tool use block.
///
/// Represents a tool invocation performed by the API server (e.g. web search).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerToolUseBlock {
    /// The block type identifier — always `"server_tool_use"`.
    #[serde(rename = "type")]
    pub block_type: String,
    /// The tool name (e.g. `"web_search"`).
    pub name: String,
    /// Unique identifier for this tool invocation.
    pub id: String,
    /// The tool input parameters.
    #[serde(default)]
    pub input: Value,
}

impl ServerToolUseBlock {
    /// Create a new server tool use block.
    #[must_use]
    pub fn new(name: String, id: String, input: Value) -> Self {
        Self {
            block_type: "server_tool_use".to_string(),
            name,
            id,
            input,
        }
    }

    /// Convert to a JSON value for API serialization.
    #[must_use]
    pub fn to_value(&self) -> Value {
        json!({
            "type": self.block_type,
            "name": self.name,
            "id": self.id,
            "input": self.input,
        })
    }
}

/// A server-side tool result block.
///
/// Contains the output of a server-side tool invocation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerToolResultBlock {
    /// The block type identifier — always `"server_tool_result"`.
    #[serde(rename = "type")]
    pub block_type: String,
    /// The tool use ID this result corresponds to.
    pub tool_use_id: String,
    /// The result content.
    pub content: Value,
}

impl ServerToolResultBlock {
    /// Create a new server tool result block.
    #[must_use]
    pub fn new(tool_use_id: String, content: Value) -> Self {
        Self {
            block_type: "server_tool_result".to_string(),
            tool_use_id,
            content,
        }
    }

    /// Convert to a JSON value for API serialization.
    #[must_use]
    pub fn to_value(&self) -> Value {
        json!({
            "type": self.block_type,
            "tool_use_id": self.tool_use_id,
            "content": self.content,
        })
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Parse a server tool use content block from a streaming or non-streaming response.
///
/// # Arguments
///
/// * `block` — The content block JSON value from the response.
///
/// # Returns
///
/// The parsed `ServerToolUseBlock`, or `None` if the block is not a valid server tool use block.
pub fn parse_server_tool_use(block: &Value) -> Option<ServerToolUseBlock> {
    let block_type = block.get("type").and_then(Value::as_str)?;
    if block_type != "server_tool_use" {
        return None;
    }
    let name = block.get("name").and_then(Value::as_str)?.to_string();
    let id = block.get("id").and_then(Value::as_str)?.to_string();
    let input = block.get("input").cloned().unwrap_or(json!({}));
    Some(ServerToolUseBlock {
        block_type: "server_tool_use".to_string(),
        name,
        id,
        input,
    })
}

/// Parse a server tool result content block from a response.
///
/// # Arguments
///
/// * `block` — The content block JSON value from the response.
///
/// # Returns
///
/// The parsed `ServerToolResultBlock`, or `None` if invalid.
pub fn parse_server_tool_result(block: &Value) -> Option<ServerToolResultBlock> {
    let block_type = block.get("type").and_then(Value::as_str)?;
    if block_type != "server_tool_result" {
        return None;
    }
    let tool_use_id = block
        .get("tool_use_id")
        .and_then(Value::as_str)?
        .to_string();
    let content = block.get("content").cloned().unwrap_or(json!(null));
    Some(ServerToolResultBlock {
        block_type: "server_tool_result".to_string(),
        tool_use_id,
        content,
    })
}

/// Check whether a content block is any type of server tool block.
///
/// # Arguments
///
/// * `block` — The content block to check.
///
/// # Returns
///
/// `true` if the block is a `server_tool_use` or `server_tool_result`.
#[must_use]
pub fn is_server_tool_block(block: &Value) -> bool {
    let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
    matches!(block_type, "server_tool_use" | "server_tool_result")
}

/// Well-known server tool names.
pub mod tools {
    /// Web search tool.
    pub const WEB_SEARCH: &str = "web_search";
    /// Code execution tool.
    pub const CODE_EXECUTION: &str = "code_execution";
    /// Computer use tool.
    pub const COMPUTER_USE: &str = "computer_use";
    /// MCP tool prefix.
    pub const MCP_PREFIX: &str = "mcp__";
}

/// Check if a tool name is a well-known server tool.
///
/// # Arguments
///
/// * `name` — The tool name to check.
///
/// # Returns
///
/// `true` if the name matches a known server tool or has the MCP prefix.
#[must_use]
pub fn is_known_server_tool(name: &str) -> bool {
    matches!(
        name,
        tools::WEB_SEARCH | tools::CODE_EXECUTION | tools::COMPUTER_USE
    ) || name.starts_with(tools::MCP_PREFIX)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- ServerToolUseBlock ---

    #[test]
    fn server_tool_use_block_new() {
        let block = ServerToolUseBlock::new(
            "web_search".to_string(),
            "srv_123".to_string(),
            json!({"query": "rust programming"}),
        );
        assert_eq!(block.block_type, "server_tool_use");
        assert_eq!(block.name, "web_search");
        assert_eq!(block.id, "srv_123");
        assert_eq!(block.input["query"], "rust programming");
    }

    #[test]
    fn server_tool_use_block_to_value() {
        let block = ServerToolUseBlock::new(
            "web_search".to_string(),
            "srv_456".to_string(),
            json!({"query": "test"}),
        );
        let val = block.to_value();
        assert_eq!(val["type"], "server_tool_use");
        assert_eq!(val["name"], "web_search");
        assert_eq!(val["id"], "srv_456");
    }

    #[test]
    fn server_tool_use_block_serialization_roundtrip() {
        let block = ServerToolUseBlock::new(
            "code_execution".to_string(),
            "srv_789".to_string(),
            json!({"code": "print(1)"}),
        );
        let json_str = serde_json::to_string(&block).expect("serialize");
        let deserialized: ServerToolUseBlock =
            serde_json::from_str(&json_str).expect("deserialize");
        assert_eq!(block, deserialized);
    }

    // --- ServerToolResultBlock ---

    #[test]
    fn server_tool_result_block_new() {
        let block = ServerToolResultBlock::new(
            "srv_123".to_string(),
            json!({"results": ["item1", "item2"]}),
        );
        assert_eq!(block.block_type, "server_tool_result");
        assert_eq!(block.tool_use_id, "srv_123");
    }

    #[test]
    fn server_tool_result_block_to_value() {
        let block = ServerToolResultBlock::new("srv_456".to_string(), json!({"output": "result"}));
        let val = block.to_value();
        assert_eq!(val["type"], "server_tool_result");
        assert_eq!(val["tool_use_id"], "srv_456");
    }

    #[test]
    fn server_tool_result_block_serialization_roundtrip() {
        let block = ServerToolResultBlock::new("srv_abc".to_string(), json!({"data": 42}));
        let json_str = serde_json::to_string(&block).expect("serialize");
        let deserialized: ServerToolResultBlock =
            serde_json::from_str(&json_str).expect("deserialize");
        assert_eq!(block, deserialized);
    }

    // --- parse_server_tool_use ---

    #[test]
    fn parse_server_tool_use_valid() {
        let block = json!({
            "type": "server_tool_use",
            "name": "web_search",
            "id": "srv_001",
            "input": {"query": "hello"}
        });
        let parsed = parse_server_tool_use(&block).expect("should parse");
        assert_eq!(parsed.name, "web_search");
        assert_eq!(parsed.id, "srv_001");
        assert_eq!(parsed.input["query"], "hello");
    }

    #[test]
    fn parse_server_tool_use_missing_input() {
        let block = json!({
            "type": "server_tool_use",
            "name": "web_search",
            "id": "srv_002"
        });
        let parsed = parse_server_tool_use(&block).expect("should parse");
        assert_eq!(parsed.input, json!({}));
    }

    #[test]
    fn parse_server_tool_use_wrong_type() {
        let block = json!({
            "type": "tool_use",
            "name": "bash",
            "id": "t1"
        });
        assert!(parse_server_tool_use(&block).is_none());
    }

    #[test]
    fn parse_server_tool_use_missing_name() {
        let block = json!({
            "type": "server_tool_use",
            "id": "srv_003"
        });
        assert!(parse_server_tool_use(&block).is_none());
    }

    #[test]
    fn parse_server_tool_use_missing_id() {
        let block = json!({
            "type": "server_tool_use",
            "name": "web_search"
        });
        assert!(parse_server_tool_use(&block).is_none());
    }

    // --- parse_server_tool_result ---

    #[test]
    fn parse_server_tool_result_valid() {
        let block = json!({
            "type": "server_tool_result",
            "tool_use_id": "srv_001",
            "content": {"results": ["a"]}
        });
        let parsed = parse_server_tool_result(&block).expect("should parse");
        assert_eq!(parsed.tool_use_id, "srv_001");
        assert_eq!(parsed.content["results"][0], "a");
    }

    #[test]
    fn parse_server_tool_result_missing_content() {
        let block = json!({
            "type": "server_tool_result",
            "tool_use_id": "srv_002"
        });
        let parsed = parse_server_tool_result(&block).expect("should parse");
        assert_eq!(parsed.content, json!(null));
    }

    #[test]
    fn parse_server_tool_result_wrong_type() {
        let block = json!({
            "type": "tool_result",
            "tool_use_id": "srv_003"
        });
        assert!(parse_server_tool_result(&block).is_none());
    }

    // --- is_server_tool_block ---

    #[test]
    fn is_server_tool_block_use() {
        let block = json!({"type": "server_tool_use"});
        assert!(is_server_tool_block(&block));
    }

    #[test]
    fn is_server_tool_block_result() {
        let block = json!({"type": "server_tool_result"});
        assert!(is_server_tool_block(&block));
    }

    #[test]
    fn is_server_tool_block_text() {
        let block = json!({"type": "text"});
        assert!(!is_server_tool_block(&block));
    }

    #[test]
    fn is_server_tool_block_no_type() {
        let block = json!({});
        assert!(!is_server_tool_block(&block));
    }

    // --- is_known_server_tool ---

    #[test]
    fn is_known_server_tool_web_search() {
        assert!(is_known_server_tool("web_search"));
    }

    #[test]
    fn is_known_server_tool_code_execution() {
        assert!(is_known_server_tool("code_execution"));
    }

    #[test]
    fn is_known_server_tool_computer_use() {
        assert!(is_known_server_tool("computer_use"));
    }

    #[test]
    fn is_known_server_tool_mcp_prefix() {
        assert!(is_known_server_tool("mcp__my_server__my_tool"));
    }

    #[test]
    fn is_known_server_tool_unknown() {
        assert!(!is_known_server_tool("bash"));
        assert!(!is_known_server_tool("read_file"));
    }
}
