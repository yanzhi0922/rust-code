//! Thinking Blocks support for extended thinking and signature deltas.
//!
//! Handles parsing of `thinking` and `signature` content block deltas from
//! Anthropic streaming responses, as well as redaction of thinking blocks
//! for display purposes.
//!
//! Based on upstream Claude Code's thinking + signature delta parsing.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for extended thinking support.
///
/// Controls the budget tokens allocated for thinking and whether thinking
/// is enabled for the current request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThinkingConfig {
    /// Number of tokens to allocate for the thinking budget.
    pub budget_tokens: u32,
    /// Whether thinking is enabled — always `"enabled"` when present.
    #[serde(rename = "type")]
    pub thinking_type: String,
}

impl ThinkingConfig {
    /// Create a new thinking configuration with the specified budget.
    ///
    /// The `thinking_type` is always set to `"enabled"`.
    #[must_use]
    pub fn new(budget_tokens: u32) -> Self {
        Self {
            budget_tokens,
            thinking_type: "enabled".to_string(),
        }
    }

    /// Create a disabled thinking configuration (zero budget).
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            budget_tokens: 0,
            thinking_type: "disabled".to_string(),
        }
    }

    /// Create an adaptive thinking configuration.
    ///
    /// Adaptive thinking lets the model decide how much thinking to do.
    /// This is the default for Claude 4+ models.
    #[must_use]
    pub fn adaptive() -> Self {
        Self {
            budget_tokens: 0,
            thinking_type: "adaptive".to_string(),
        }
    }

    /// Convert to a JSON value suitable for the API request body.
    #[must_use]
    pub fn to_api_value(&self) -> Value {
        if self.thinking_type == "adaptive" {
            json!({
                "type": "adaptive"
            })
        } else if self.thinking_type == "disabled" {
            json!({
                "type": "disabled"
            })
        } else {
            json!({
                "type": "enabled",
                "budget_tokens": self.budget_tokens,
            })
        }
    }
}

impl Default for ThinkingConfig {
    fn default() -> Self {
        Self::new(10_000)
    }
}

// ---------------------------------------------------------------------------
// Block types
// ---------------------------------------------------------------------------

/// A thinking content block containing the model's chain-of-thought.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThinkingBlock {
    /// The block type identifier — always `"thinking"`.
    #[serde(rename = "type")]
    pub block_type: String,
    /// The thinking text content.
    pub thinking: String,
}

impl ThinkingBlock {
    /// Create a new thinking block with the given text.
    #[must_use]
    pub fn new(thinking: String) -> Self {
        Self {
            block_type: "thinking".to_string(),
            thinking,
        }
    }
}

/// A signature content block used to verify thinking integrity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignatureBlock {
    /// The block type identifier — always `"signature"`.
    #[serde(rename = "type")]
    pub block_type: String,
    /// The signature data.
    pub signature: String,
}

impl SignatureBlock {
    /// Create a new signature block with the given signature data.
    #[must_use]
    pub fn new(signature: String) -> Self {
        Self {
            block_type: "signature".to_string(),
            signature,
        }
    }
}

// ---------------------------------------------------------------------------
// Delta parsing
// ---------------------------------------------------------------------------

/// Parsed result of a thinking content block delta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThinkingDelta {
    /// Incremental thinking text.
    pub thinking: String,
}

/// Parsed result of a signature content block delta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureDelta {
    /// Incremental signature data.
    pub signature: String,
}

/// Parse a thinking content block delta from a streaming SSE event.
///
/// Expects the `delta` object from a content block with `type: "thinking_delta"`.
///
/// # Arguments
///
/// * `block` — The content block JSON value from the streaming response.
///
/// # Returns
///
/// The parsed `ThinkingDelta`, or `None` if the block is not a valid thinking delta.
pub fn parse_thinking_delta(block: &Value) -> Option<ThinkingDelta> {
    let block_type = block.get("type").and_then(Value::as_str)?;
    if block_type != "thinking_delta" {
        return None;
    }
    let thinking = block.get("thinking").and_then(Value::as_str)?.to_string();
    Some(ThinkingDelta { thinking })
}

/// Parse a signature content block delta from a streaming SSE event.
///
/// Expects the `delta` object from a content block with `type: "signature_delta"`.
///
/// # Arguments
///
/// * `block` — The content block JSON value from the streaming response.
///
/// # Returns
///
/// The parsed `SignatureDelta`, or `None` if the block is not a valid signature delta.
pub fn parse_signature_delta(block: &Value) -> Option<SignatureDelta> {
    let block_type = block.get("type").and_then(Value::as_str)?;
    if block_type != "signature_delta" {
        return None;
    }
    let signature = block.get("signature").and_then(Value::as_str)?.to_string();
    Some(SignatureDelta { signature })
}

// ---------------------------------------------------------------------------
// Redaction
// ---------------------------------------------------------------------------

/// Redact thinking and signature blocks from a message's content blocks.
///
/// This is used before displaying messages to the user, as thinking blocks
/// are internal and should not be shown.
///
/// # Arguments
///
/// * `blocks` — The content blocks array from an assistant message.
///
/// # Returns
///
/// A new vector of content blocks with thinking and signature blocks removed.
pub fn redact_thinking_blocks(blocks: &[Value]) -> Vec<Value> {
    blocks
        .iter()
        .filter(|block| {
            let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
            !matches!(block_type, "thinking" | "signature" | "redacted_thinking")
        })
        .cloned()
        .collect()
}

/// Check whether a content block is a thinking-related block.
///
/// # Arguments
///
/// * `block` — The content block to check.
///
/// # Returns
///
/// `true` if the block is a thinking, signature, or redacted_thinking block.
#[must_use]
pub fn is_thinking_block(block: &Value) -> bool {
    let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
    matches!(block_type, "thinking" | "signature" | "redacted_thinking")
}

/// Extract all thinking text from a message's content blocks.
///
/// Concatenates the text from all thinking blocks into a single string.
///
/// # Arguments
///
/// * `blocks` — The content blocks array from an assistant message.
///
/// # Returns
///
/// The concatenated thinking text, or an empty string if none found.
#[must_use]
pub fn extract_thinking_text(blocks: &[Value]) -> String {
    blocks
        .iter()
        .filter(|block| {
            block
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|t| t == "thinking")
        })
        .filter_map(|block| block.get("thinking").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("")
}

/// Determine whether a model should use adaptive thinking.
///
/// Returns true for Claude 4+ models that support adaptive thinking.
#[must_use]
pub fn should_use_adaptive_thinking(model: &str) -> bool {
    let model_lower = model.to_ascii_lowercase();
    model_lower.contains("claude-opus-4")
        || model_lower.contains("claude-sonnet-4")
        || model_lower.contains("claude-haiku-4")
        || model_lower.contains("claude-4")
        || model_lower.contains("minimax")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- ThinkingConfig ---

    #[test]
    fn thinking_config_new() {
        let config = ThinkingConfig::new(5000);
        assert_eq!(config.budget_tokens, 5000);
        assert_eq!(config.thinking_type, "enabled");
    }

    #[test]
    fn thinking_config_disabled() {
        let config = ThinkingConfig::disabled();
        assert_eq!(config.budget_tokens, 0);
        assert_eq!(config.thinking_type, "disabled");
    }

    #[test]
    fn thinking_config_default() {
        let config = ThinkingConfig::default();
        assert_eq!(config.budget_tokens, 10_000);
        assert_eq!(config.thinking_type, "enabled");
    }

    #[test]
    fn thinking_config_to_api_value() {
        let config = ThinkingConfig::new(8000);
        let val = config.to_api_value();
        assert_eq!(val["type"], "enabled");
        assert_eq!(val["budget_tokens"], 8000);
    }

    #[test]
    fn thinking_config_adaptive() {
        let config = ThinkingConfig::adaptive();
        assert_eq!(config.budget_tokens, 0);
        assert_eq!(config.thinking_type, "adaptive");
    }

    #[test]
    fn thinking_config_adaptive_to_api_value() {
        let config = ThinkingConfig::adaptive();
        let val = config.to_api_value();
        assert_eq!(val["type"], "adaptive");
        assert!(val.get("budget_tokens").is_none());
    }

    #[test]
    fn should_use_adaptive_thinking_claude4() {
        assert!(should_use_adaptive_thinking("claude-opus-4-7"));
        assert!(should_use_adaptive_thinking("claude-sonnet-4-6"));
        assert!(should_use_adaptive_thinking("claude-haiku-4-5"));
        assert!(should_use_adaptive_thinking("minimax-m2.7"));
        assert!(!should_use_adaptive_thinking("gpt-4o"));
        assert!(!should_use_adaptive_thinking("claude-3-5-sonnet"));
    }

    #[test]
    fn thinking_config_serialization_roundtrip() {
        let config = ThinkingConfig::new(1234);
        let json = serde_json::to_string(&config).expect("serialize");
        let deserialized: ThinkingConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(config, deserialized);
    }

    // --- ThinkingBlock ---

    #[test]
    fn thinking_block_new() {
        let block = ThinkingBlock::new("I need to think about this".to_string());
        assert_eq!(block.block_type, "thinking");
        assert_eq!(block.thinking, "I need to think about this");
    }

    #[test]
    fn thinking_block_serialization() {
        let block = ThinkingBlock::new("test thinking".to_string());
        let json = serde_json::to_string(&block).expect("serialize");
        assert!(json.contains("\"type\":\"thinking\""));
        assert!(json.contains("\"thinking\":\"test thinking\""));
    }

    // --- SignatureBlock ---

    #[test]
    fn signature_block_new() {
        let block = SignatureBlock::new("sig_data_123".to_string());
        assert_eq!(block.block_type, "signature");
        assert_eq!(block.signature, "sig_data_123");
    }

    #[test]
    fn signature_block_serialization() {
        let block = SignatureBlock::new("sig".to_string());
        let json = serde_json::to_string(&block).expect("serialize");
        assert!(json.contains("\"type\":\"signature\""));
    }

    // --- parse_thinking_delta ---

    #[test]
    fn parse_thinking_delta_valid() {
        let block = json!({
            "type": "thinking_delta",
            "thinking": "Let me reason about"
        });
        let delta = parse_thinking_delta(&block).expect("should parse");
        assert_eq!(delta.thinking, "Let me reason about");
    }

    #[test]
    fn parse_thinking_delta_wrong_type() {
        let block = json!({
            "type": "text_delta",
            "text": "hello"
        });
        assert!(parse_thinking_delta(&block).is_none());
    }

    #[test]
    fn parse_thinking_delta_missing_thinking() {
        let block = json!({
            "type": "thinking_delta"
        });
        assert!(parse_thinking_delta(&block).is_none());
    }

    #[test]
    fn parse_thinking_delta_missing_type() {
        let block = json!({
            "thinking": "some text"
        });
        assert!(parse_thinking_delta(&block).is_none());
    }

    #[test]
    fn parse_thinking_delta_empty_thinking() {
        let block = json!({
            "type": "thinking_delta",
            "thinking": ""
        });
        let delta = parse_thinking_delta(&block).expect("should parse");
        assert!(delta.thinking.is_empty());
    }

    // --- parse_signature_delta ---

    #[test]
    fn parse_signature_delta_valid() {
        let block = json!({
            "type": "signature_delta",
            "signature": "abc123"
        });
        let delta = parse_signature_delta(&block).expect("should parse");
        assert_eq!(delta.signature, "abc123");
    }

    #[test]
    fn parse_signature_delta_wrong_type() {
        let block = json!({
            "type": "thinking_delta",
            "thinking": "text"
        });
        assert!(parse_signature_delta(&block).is_none());
    }

    #[test]
    fn parse_signature_delta_missing_signature() {
        let block = json!({
            "type": "signature_delta"
        });
        assert!(parse_signature_delta(&block).is_none());
    }

    // --- redact_thinking_blocks ---

    #[test]
    fn redact_removes_thinking_blocks() {
        let blocks = vec![
            json!({"type": "thinking", "thinking": "internal"}),
            json!({"type": "text", "text": "hello"}),
            json!({"type": "signature", "signature": "sig"}),
        ];
        let redacted = redact_thinking_blocks(&blocks);
        assert_eq!(redacted.len(), 1);
        assert_eq!(redacted[0]["type"], "text");
    }

    #[test]
    fn redact_removes_redacted_thinking() {
        let blocks = vec![
            json!({"type": "redacted_thinking", "data": "xxx"}),
            json!({"type": "text", "text": "visible"}),
        ];
        let redacted = redact_thinking_blocks(&blocks);
        assert_eq!(redacted.len(), 1);
        assert_eq!(redacted[0]["type"], "text");
    }

    #[test]
    fn redact_preserves_tool_use() {
        let blocks = vec![
            json!({"type": "tool_use", "id": "t1", "name": "bash", "input": {}}),
            json!({"type": "thinking", "thinking": "hidden"}),
        ];
        let redacted = redact_thinking_blocks(&blocks);
        assert_eq!(redacted.len(), 1);
        assert_eq!(redacted[0]["type"], "tool_use");
    }

    #[test]
    fn redact_empty_input() {
        let blocks: Vec<Value> = vec![];
        let redacted = redact_thinking_blocks(&blocks);
        assert!(redacted.is_empty());
    }

    #[test]
    fn redact_no_thinking_blocks() {
        let blocks = vec![
            json!({"type": "text", "text": "a"}),
            json!({"type": "text", "text": "b"}),
        ];
        let redacted = redact_thinking_blocks(&blocks);
        assert_eq!(redacted.len(), 2);
    }

    // --- is_thinking_block ---

    #[test]
    fn is_thinking_block_thinking() {
        let block = json!({"type": "thinking", "thinking": "text"});
        assert!(is_thinking_block(&block));
    }

    #[test]
    fn is_thinking_block_signature() {
        let block = json!({"type": "signature", "signature": "sig"});
        assert!(is_thinking_block(&block));
    }

    #[test]
    fn is_thinking_block_redacted() {
        let block = json!({"type": "redacted_thinking", "data": "xxx"});
        assert!(is_thinking_block(&block));
    }

    #[test]
    fn is_thinking_block_text() {
        let block = json!({"type": "text", "text": "hello"});
        assert!(!is_thinking_block(&block));
    }

    #[test]
    fn is_thinking_block_no_type() {
        let block = json!({"text": "hello"});
        assert!(!is_thinking_block(&block));
    }

    // --- extract_thinking_text ---

    #[test]
    fn extract_thinking_text_multiple() {
        let blocks = vec![
            json!({"type": "thinking", "thinking": "Part 1. "}),
            json!({"type": "text", "text": "visible"}),
            json!({"type": "thinking", "thinking": "Part 2."}),
        ];
        assert_eq!(extract_thinking_text(&blocks), "Part 1. Part 2.");
    }

    #[test]
    fn extract_thinking_text_none() {
        let blocks = vec![
            json!({"type": "text", "text": "hello"}),
            json!({"type": "tool_use", "id": "t1", "name": "bash", "input": {}}),
        ];
        assert!(extract_thinking_text(&blocks).is_empty());
    }

    #[test]
    fn extract_thinking_text_empty() {
        let blocks: Vec<Value> = vec![];
        assert!(extract_thinking_text(&blocks).is_empty());
    }
}
