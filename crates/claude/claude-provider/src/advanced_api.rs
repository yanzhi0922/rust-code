//! Advanced API Client features: thinking blocks, deferred tools, task budgets,
//! advisor model, and prompt cache scope.
//!
//! This module extends the base provider with capabilities that mirror upstream
//! Claude Code's advanced API features including:
//!
//! - **Thinking block processing** — extended thinking with budget tokens and streaming
//! - **Deferred tools / ToolSearch** — lazy loading of tool definitions via search
//! - **Task budget parameters** — turn/token/cost limits per request
//! - **Advisor model** — lightweight model for tool-use summaries
//! - **Prompt cache scope** — cache control headers on messages

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

// ===========================================================================
// §1  Thinking Block Processing
// ===========================================================================

/// Configuration for extended thinking in API requests.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThinkingConfig {
    /// Whether thinking is enabled.
    pub enabled: bool,
    /// Budget tokens for thinking (e.g., 10000).
    pub budget_tokens: Option<u32>,
    /// Type of thinking mode.
    pub thinking_mode: ThinkingMode,
}

impl ThinkingConfig {
    /// Create a new thinking configuration.
    #[must_use]
    pub fn new(enabled: bool, budget_tokens: Option<u32>, thinking_mode: ThinkingMode) -> Self {
        Self {
            enabled,
            budget_tokens,
            thinking_mode,
        }
    }

    /// Create a disabled thinking configuration.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            budget_tokens: None,
            thinking_mode: ThinkingMode::None,
        }
    }

    /// Create a streaming thinking configuration with the given budget.
    #[must_use]
    pub fn streaming(budget_tokens: u32) -> Self {
        Self {
            enabled: true,
            budget_tokens: Some(budget_tokens),
            thinking_mode: ThinkingMode::Streaming,
        }
    }

    /// Convert to a JSON value suitable for the API request body.
    #[must_use]
    pub fn to_api_value(&self) -> Value {
        match self.thinking_mode {
            ThinkingMode::None => json!({"type": "disabled"}),
            ThinkingMode::Enabled | ThinkingMode::Streaming => json!({
                "type": "enabled",
                "budget_tokens": self.budget_tokens.unwrap_or(10_000),
            }),
        }
    }
}

impl Default for ThinkingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            budget_tokens: Some(10_000),
            thinking_mode: ThinkingMode::Enabled,
        }
    }
}

/// Thinking mode for extended thinking.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ThinkingMode {
    /// No thinking.
    None,
    /// Standard thinking with budget.
    Enabled,
    /// Streaming thinking (interleaved).
    Streaming,
}

/// A thinking block from the API response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThinkingBlock {
    /// The thinking text content.
    pub text: String,
    /// Optional signature for verification.
    pub signature: Option<String>,
}

impl ThinkingBlock {
    /// Create a new thinking block.
    #[must_use]
    pub fn new(text: String) -> Self {
        Self {
            text,
            signature: None,
        }
    }

    /// Create a thinking block with a signature.
    #[must_use]
    pub fn with_signature(text: String, signature: String) -> Self {
        Self {
            text,
            signature: Some(signature),
        }
    }
}

/// A redacted thinking block (content hidden).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RedactedThinkingBlock {
    /// Opaque data representing the redacted content.
    pub data: String,
}

impl RedactedThinkingBlock {
    /// Create a new redacted thinking block.
    #[must_use]
    pub fn new(data: String) -> Self {
        Self { data }
    }
}

/// A processed thinking block — the result of parsing raw API content blocks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ProcessedThinkingBlock {
    /// A visible thinking block with text and optional signature.
    Thinking(ThinkingBlock),
    /// A redacted thinking block with opaque data.
    Redacted(RedactedThinkingBlock),
}

/// Process thinking blocks from an API response.
///
/// Takes a slice of raw JSON content blocks and extracts thinking-related
/// blocks into typed [`ProcessedThinkingBlock`] variants.
///
/// # Arguments
///
/// * `blocks` — Raw content blocks from the API response.
///
/// # Returns
///
/// A vector of processed thinking blocks (both visible and redacted).
pub fn process_thinking_blocks(blocks: &[Value]) -> Vec<ProcessedThinkingBlock> {
    blocks
        .iter()
        .filter_map(|block| {
            let block_type = block.get("type").and_then(Value::as_str)?;
            match block_type {
                "thinking" => {
                    let text = block
                        .get("thinking")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let signature = block
                        .get("signature")
                        .and_then(Value::as_str)
                        .map(String::from);
                    Some(ProcessedThinkingBlock::Thinking(ThinkingBlock {
                        text,
                        signature,
                    }))
                }
                "redacted_thinking" => {
                    let data = block
                        .get("data")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    Some(ProcessedThinkingBlock::Redacted(
                        RedactedThinkingBlock::new(data),
                    ))
                }
                _ => None,
            }
        })
        .collect()
}

// ===========================================================================
// §2  Deferred Tools / ToolSearch API Integration
// ===========================================================================

/// Configuration for deferred tool loading.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeferredToolConfig {
    /// Whether ToolSearch is enabled.
    pub enabled: bool,
    /// Tools that are always loaded (never deferred).
    pub always_load_tools: Vec<String>,
    /// Tools that should be deferred.
    pub deferred_tools: Vec<String>,
}

impl DeferredToolConfig {
    /// Create a disabled deferred tool configuration.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            always_load_tools: Vec::new(),
            deferred_tools: Vec::new(),
        }
    }

    /// Create a new deferred tool configuration.
    #[must_use]
    pub fn new(enabled: bool, always_load_tools: Vec<String>, deferred_tools: Vec<String>) -> Self {
        Self {
            enabled,
            always_load_tools,
            deferred_tools,
        }
    }

    /// Check if a specific tool name is in the deferred list.
    #[must_use]
    pub fn is_deferred(&self, tool_name: &str) -> bool {
        self.enabled && self.deferred_tools.iter().any(|t| t == tool_name)
    }

    /// Check if a specific tool name is in the always-load list.
    #[must_use]
    pub fn is_always_loaded(&self, tool_name: &str) -> bool {
        self.always_load_tools.iter().any(|t| t == tool_name)
    }
}

impl Default for DeferredToolConfig {
    fn default() -> Self {
        Self::disabled()
    }
}

/// A ToolSearch request to load deferred tools.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolSearchRequest {
    /// The search query from the model.
    pub query: String,
    /// Maximum number of results to return.
    pub max_results: Option<usize>,
}

impl ToolSearchRequest {
    /// Create a new tool search request.
    #[must_use]
    pub fn new(query: String) -> Self {
        Self {
            query,
            max_results: None,
        }
    }

    /// Create a tool search request with a max results limit.
    #[must_use]
    pub fn with_max_results(query: String, max_results: usize) -> Self {
        Self {
            query,
            max_results: Some(max_results),
        }
    }

    /// Convert to a JSON value suitable for a tool_use input.
    #[must_use]
    pub fn to_input_value(&self) -> Value {
        let mut obj = json!({"query": self.query});
        if let Some(max) = self.max_results {
            obj["max_results"] = json!(max);
        }
        obj
    }
}

/// Result of a ToolSearch operation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolSearchResult {
    /// Tools that matched the search.
    pub found_tools: Vec<FoundTool>,
    /// Whether the search was exhaustive.
    pub exhaustive: bool,
}

impl ToolSearchResult {
    /// Create an empty search result.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            found_tools: Vec::new(),
            exhaustive: true,
        }
    }

    /// Create a search result with found tools.
    #[must_use]
    pub fn new(found_tools: Vec<FoundTool>, exhaustive: bool) -> Self {
        Self {
            found_tools,
            exhaustive,
        }
    }

    /// Check if no tools were found.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.found_tools.is_empty()
    }
}

/// A tool found via ToolSearch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FoundTool {
    /// Tool name.
    pub name: String,
    /// Tool description.
    pub description: String,
    /// Hint for search matching.
    pub search_hint: String,
    /// Relevance score (0.0–1.0).
    pub relevance_score: f64,
}

impl FoundTool {
    /// Create a new found tool.
    #[must_use]
    pub fn new(
        name: String,
        description: String,
        search_hint: String,
        relevance_score: f64,
    ) -> Self {
        Self {
            name,
            description,
            search_hint,
            relevance_score,
        }
    }
}

/// The canonical tool name used for deferred tool search calls.
pub const TOOL_SEARCH_TOOL_NAME: &str = "toolsearch";

/// Check if a tool name indicates a deferred tool search.
///
/// Returns `true` when the tool name is `"toolsearch"` (case-insensitive),
/// which is the canonical name Claude Code uses for the ToolSearch built-in.
///
/// # Arguments
///
/// * `tool_name` — The name of the tool being called.
/// * `_input` — The input to the tool call (reserved for future use).
#[must_use]
pub fn is_deferred_tool_call(tool_name: &str, _input: &Value) -> bool {
    tool_name.eq_ignore_ascii_case(TOOL_SEARCH_TOOL_NAME)
}

/// Build the initial tool list with deferred tools marked.
///
/// Tools in the `deferred_tools` list are replaced with a lightweight placeholder
/// that includes only the name and a `deferred: true` flag. Tools in the
/// `always_load_tools` list always keep their full definition.
///
/// # Arguments
///
/// * `all_tools` — Full tool definitions (each must have a `"name"` field).
/// * `config` — Deferred tool configuration.
///
/// # Returns
///
/// A vector of tool JSON values where deferred tools are replaced with placeholders.
pub fn build_deferred_tool_list(all_tools: &[Value], config: &DeferredToolConfig) -> Vec<Value> {
    if !config.enabled {
        return all_tools.to_vec();
    }

    all_tools
        .iter()
        .map(|tool| {
            let name = tool
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("");

            if config.is_always_loaded(name) || !config.is_deferred(name) {
                tool.clone()
            } else {
                // Replace with a lightweight deferred placeholder
                json!({
                    "name": name,
                    "description": format!("(deferred) Use {TOOL_SEARCH_TOOL_NAME} to load this tool."),
                    "deferred": true,
                })
            }
        })
        .collect()
}

// ===========================================================================
// §3  Task Budget API Parameters
// ===========================================================================

/// Task budget parameters sent with API requests.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskBudgetParams {
    /// Maximum number of turns for this task.
    pub max_turns: Option<u32>,
    /// Maximum total tokens allowed.
    pub max_total_tokens: Option<u64>,
    /// Maximum cost in USD.
    pub max_budget_usd: Option<f64>,
}

impl TaskBudgetParams {
    /// Create an empty (unlimited) budget.
    #[must_use]
    pub fn unlimited() -> Self {
        Self {
            max_turns: None,
            max_total_tokens: None,
            max_budget_usd: None,
        }
    }

    /// Create a budget with a turn limit.
    #[must_use]
    pub fn with_max_turns(max_turns: u32) -> Self {
        Self {
            max_turns: Some(max_turns),
            max_total_tokens: None,
            max_budget_usd: None,
        }
    }

    /// Create a budget with a token limit.
    #[must_use]
    pub fn with_max_tokens(max_total_tokens: u64) -> Self {
        Self {
            max_turns: None,
            max_total_tokens: Some(max_total_tokens),
            max_budget_usd: None,
        }
    }

    /// Create a budget with a USD cost limit.
    #[must_use]
    pub fn with_budget_usd(max_budget_usd: f64) -> Self {
        Self {
            max_turns: None,
            max_total_tokens: None,
            max_budget_usd: Some(max_budget_usd),
        }
    }

    /// Check if any budget limit is set.
    #[must_use]
    pub fn has_limit(&self) -> bool {
        self.max_turns.is_some() || self.max_total_tokens.is_some() || self.max_budget_usd.is_some()
    }

    /// Merge another budget into this one, taking the more restrictive values.
    #[must_use]
    pub fn merge_restrictive(&self, other: &TaskBudgetParams) -> TaskBudgetParams {
        TaskBudgetParams {
            max_turns: match (self.max_turns, other.max_turns) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            },
            max_total_tokens: match (self.max_total_tokens, other.max_total_tokens) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            },
            max_budget_usd: match (self.max_budget_usd, other.max_budget_usd) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            },
        }
    }
}

impl Default for TaskBudgetParams {
    fn default() -> Self {
        Self::unlimited()
    }
}

// ===========================================================================
// §4  Advisor Model Support
// ===========================================================================

/// Configuration for the advisor model (lightweight model for tool-use summaries).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdvisorModelConfig {
    /// Whether the advisor model is enabled.
    pub enabled: bool,
    /// Model identifier for the advisor.
    pub model: String,
    /// Maximum tokens for advisor responses.
    pub max_tokens: u32,
}

impl AdvisorModelConfig {
    /// Create a disabled advisor model configuration.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            model: String::new(),
            max_tokens: 0,
        }
    }

    /// Create a new advisor model configuration.
    #[must_use]
    pub fn new(model: String, max_tokens: u32) -> Self {
        Self {
            enabled: true,
            model,
            max_tokens,
        }
    }
}

impl Default for AdvisorModelConfig {
    fn default() -> Self {
        Self::disabled()
    }
}

/// Result from the advisor model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdvisorResult {
    /// The advisor's summary/recommendation.
    pub summary: String,
    /// Whether the advisor recommends proceeding.
    pub should_proceed: bool,
    /// Confidence score (0.0–1.0).
    pub confidence: f64,
}

impl AdvisorResult {
    /// Create a new advisor result recommending to proceed.
    #[must_use]
    pub fn proceed(summary: String, confidence: f64) -> Self {
        Self {
            summary,
            should_proceed: true,
            confidence,
        }
    }

    /// Create a new advisor result recommending to stop.
    #[must_use]
    pub fn stop(summary: String, confidence: f64) -> Self {
        Self {
            summary,
            should_proceed: false,
            confidence,
        }
    }

    /// Check if the confidence is above a threshold.
    #[must_use]
    pub fn is_confident(&self, threshold: f64) -> bool {
        self.confidence >= threshold
    }
}

// ===========================================================================
// §5  Prompt Cache Scope
// ===========================================================================

/// Prompt cache scope configuration.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum PromptCacheScope {
    /// No caching.
    None,
    /// 5-minute cache (default).
    #[default]
    Short,
    /// 1-hour cache.
    Hour,
}

impl PromptCacheScope {
    /// Return the cache TTL in seconds for this scope.
    #[must_use]
    pub fn ttl_seconds(&self) -> Option<u64> {
        match self {
            PromptCacheScope::None => None,
            PromptCacheScope::Short => Some(300), // 5 minutes
            PromptCacheScope::Hour => Some(3600), // 1 hour
        }
    }

    /// Return the Anthropic cache control value for this scope.
    #[must_use]
    pub fn to_cache_control(&self) -> Option<Value> {
        match self {
            PromptCacheScope::None => None,
            PromptCacheScope::Short | PromptCacheScope::Hour => Some(json!({"type": "ephemeral"})),
        }
    }
}

/// Apply cache control headers to messages.
///
/// Adds `cache_control` breakpoints to strategic positions in the message list:
/// - The last user message (to cache the system + conversation prefix).
/// - Optionally the system prompt (if present as a system message).
///
/// # Arguments
///
/// * `messages` — The mutable message list to annotate.
/// * `scope` — The cache scope determining TTL.
pub fn apply_cache_control(messages: &mut [Value], scope: PromptCacheScope) {
    if scope == PromptCacheScope::None {
        return;
    }

    let cache_value = scope
        .to_cache_control()
        .expect("non-None scope always produces a cache_control value");

    // Apply to the last user message (most impactful for prefix caching)
    for msg in messages.iter_mut().rev() {
        let role = msg.get("role").and_then(Value::as_str).unwrap_or("");
        if role == "user" {
            msg["cache_control"] = cache_value.clone();
            break;
        }
    }

    // Apply to the first system message if present
    for msg in messages.iter_mut() {
        let role = msg.get("role").and_then(Value::as_str).unwrap_or("");
        if role == "system" {
            msg["cache_control"] = cache_value;
            break;
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // ThinkingConfig tests
    // -----------------------------------------------------------------------

    #[test]
    fn thinking_config_new_enabled() {
        let config = ThinkingConfig::new(true, Some(10_000), ThinkingMode::Enabled);
        assert!(config.enabled);
        assert_eq!(config.budget_tokens, Some(10_000));
        assert_eq!(config.thinking_mode, ThinkingMode::Enabled);
    }

    #[test]
    fn thinking_config_disabled() {
        let config = ThinkingConfig::disabled();
        assert!(!config.enabled);
        assert_eq!(config.budget_tokens, None);
        assert_eq!(config.thinking_mode, ThinkingMode::None);
    }

    #[test]
    fn thinking_config_streaming() {
        let config = ThinkingConfig::streaming(20_000);
        assert!(config.enabled);
        assert_eq!(config.budget_tokens, Some(20_000));
        assert_eq!(config.thinking_mode, ThinkingMode::Streaming);
    }

    #[test]
    fn thinking_config_default() {
        let config = ThinkingConfig::default();
        assert!(config.enabled);
        assert_eq!(config.budget_tokens, Some(10_000));
        assert_eq!(config.thinking_mode, ThinkingMode::Enabled);
    }

    #[test]
    fn thinking_config_to_api_value_enabled() {
        let config = ThinkingConfig::new(true, Some(5000), ThinkingMode::Enabled);
        let val = config.to_api_value();
        assert_eq!(val["type"], "enabled");
        assert_eq!(val["budget_tokens"], 5000);
    }

    #[test]
    fn thinking_config_to_api_value_disabled() {
        let config = ThinkingConfig::new(false, None, ThinkingMode::None);
        let val = config.to_api_value();
        assert_eq!(val["type"], "disabled");
    }

    #[test]
    fn thinking_config_serialization_roundtrip() {
        let config = ThinkingConfig::streaming(15_000);
        let json_str = serde_json::to_string(&config).expect("serialize");
        let deserialized: ThinkingConfig = serde_json::from_str(&json_str).expect("deserialize");
        assert_eq!(config, deserialized);
    }

    // -----------------------------------------------------------------------
    // ThinkingBlock / RedactedThinkingBlock tests
    // -----------------------------------------------------------------------

    #[test]
    fn thinking_block_new() {
        let block = ThinkingBlock::new("I am thinking".to_string());
        assert_eq!(block.text, "I am thinking");
        assert!(block.signature.is_none());
    }

    #[test]
    fn thinking_block_with_signature() {
        let block = ThinkingBlock::with_signature("reasoning".to_string(), "sig_abc".to_string());
        assert_eq!(block.text, "reasoning");
        assert_eq!(block.signature.as_deref(), Some("sig_abc"));
    }

    #[test]
    fn thinking_block_serialization_roundtrip() {
        let block = ThinkingBlock::with_signature("text".into(), "sig".into());
        let json_str = serde_json::to_string(&block).expect("serialize");
        let back: ThinkingBlock = serde_json::from_str(&json_str).expect("deserialize");
        assert_eq!(block, back);
    }

    #[test]
    fn redacted_thinking_block_new() {
        let block = RedactedThinkingBlock::new("opaque_data".to_string());
        assert_eq!(block.data, "opaque_data");
    }

    #[test]
    fn redacted_thinking_block_serialization_roundtrip() {
        let block = RedactedThinkingBlock::new("data123".into());
        let json_str = serde_json::to_string(&block).expect("serialize");
        let back: RedactedThinkingBlock = serde_json::from_str(&json_str).expect("deserialize");
        assert_eq!(block, back);
    }

    // -----------------------------------------------------------------------
    // process_thinking_blocks tests
    // -----------------------------------------------------------------------

    #[test]
    fn process_thinking_blocks_extracts_thinking() {
        let blocks = vec![
            json!({"type": "thinking", "thinking": "Let me reason"}),
            json!({"type": "text", "text": "Hello"}),
        ];
        let result = process_thinking_blocks(&blocks);
        assert_eq!(result.len(), 1);
        match &result[0] {
            ProcessedThinkingBlock::Thinking(tb) => {
                assert_eq!(tb.text, "Let me reason");
                assert!(tb.signature.is_none());
            }
            other => panic!("expected Thinking, got {other:?}"),
        }
    }

    #[test]
    fn process_thinking_blocks_extracts_redacted() {
        let blocks = vec![
            json!({"type": "redacted_thinking", "data": "secret"}),
            json!({"type": "text", "text": "visible"}),
        ];
        let result = process_thinking_blocks(&blocks);
        assert_eq!(result.len(), 1);
        match &result[0] {
            ProcessedThinkingBlock::Redacted(rb) => {
                assert_eq!(rb.data, "secret");
            }
            other => panic!("expected Redacted, got {other:?}"),
        }
    }

    #[test]
    fn process_thinking_blocks_with_signature() {
        let blocks = vec![json!({
            "type": "thinking",
            "thinking": "reasoning",
            "signature": "sig_xyz"
        })];
        let result = process_thinking_blocks(&blocks);
        assert_eq!(result.len(), 1);
        match &result[0] {
            ProcessedThinkingBlock::Thinking(tb) => {
                assert_eq!(tb.signature.as_deref(), Some("sig_xyz"));
            }
            other => panic!("expected Thinking, got {other:?}"),
        }
    }

    #[test]
    fn process_thinking_blocks_empty() {
        let blocks: Vec<Value> = vec![];
        let result = process_thinking_blocks(&blocks);
        assert!(result.is_empty());
    }

    #[test]
    fn process_thinking_blocks_no_thinking_blocks() {
        let blocks = vec![
            json!({"type": "text", "text": "a"}),
            json!({"type": "tool_use", "id": "t1", "name": "bash", "input": {}}),
        ];
        let result = process_thinking_blocks(&blocks);
        assert!(result.is_empty());
    }

    #[test]
    fn process_thinking_blocks_mixed() {
        let blocks = vec![
            json!({"type": "thinking", "thinking": "part1"}),
            json!({"type": "text", "text": "visible"}),
            json!({"type": "redacted_thinking", "data": "redacted"}),
            json!({"type": "thinking", "thinking": "part2", "signature": "sig"}),
        ];
        let result = process_thinking_blocks(&blocks);
        assert_eq!(result.len(), 3);
    }

    // -----------------------------------------------------------------------
    // DeferredToolConfig tests
    // -----------------------------------------------------------------------

    #[test]
    fn deferred_tool_config_disabled() {
        let config = DeferredToolConfig::disabled();
        assert!(!config.enabled);
        assert!(config.always_load_tools.is_empty());
        assert!(config.deferred_tools.is_empty());
    }

    #[test]
    fn deferred_tool_config_is_deferred() {
        let config = DeferredToolConfig::new(
            true,
            vec!["bash".to_string()],
            vec!["rare_tool".to_string()],
        );
        assert!(config.is_deferred("rare_tool"));
        assert!(!config.is_deferred("bash"));
    }

    #[test]
    fn deferred_tool_config_is_deferred_when_disabled() {
        let config = DeferredToolConfig::new(false, vec![], vec!["rare_tool".to_string()]);
        // Even though "rare_tool" is in deferred_tools, config is disabled
        assert!(!config.is_deferred("rare_tool"));
    }

    #[test]
    fn deferred_tool_config_is_always_loaded() {
        let config =
            DeferredToolConfig::new(true, vec!["bash".to_string(), "read".to_string()], vec![]);
        assert!(config.is_always_loaded("bash"));
        assert!(config.is_always_loaded("read"));
        assert!(!config.is_always_loaded("write"));
    }

    #[test]
    fn deferred_tool_config_serialization_roundtrip() {
        let config = DeferredToolConfig::new(true, vec!["bash".into()], vec!["rare".into()]);
        let json_str = serde_json::to_string(&config).expect("serialize");
        let back: DeferredToolConfig = serde_json::from_str(&json_str).expect("deserialize");
        assert_eq!(config, back);
    }

    // -----------------------------------------------------------------------
    // ToolSearchRequest / ToolSearchResult / FoundTool tests
    // -----------------------------------------------------------------------

    #[test]
    fn tool_search_request_new() {
        let req = ToolSearchRequest::new("file editing".to_string());
        assert_eq!(req.query, "file editing");
        assert!(req.max_results.is_none());
    }

    #[test]
    fn tool_search_request_with_max_results() {
        let req = ToolSearchRequest::with_max_results("search query".into(), 5);
        assert_eq!(req.max_results, Some(5));
    }

    #[test]
    fn tool_search_request_to_input_value() {
        let req = ToolSearchRequest::with_max_results("query".into(), 10);
        let val = req.to_input_value();
        assert_eq!(val["query"], "query");
        assert_eq!(val["max_results"], 10);
    }

    #[test]
    fn tool_search_result_empty() {
        let result = ToolSearchResult::empty();
        assert!(result.is_empty());
        assert!(result.exhaustive);
    }

    #[test]
    fn tool_search_result_with_tools() {
        let tools = vec![
            FoundTool::new("bash".into(), "Run commands".into(), "shell".into(), 0.95),
            FoundTool::new("read".into(), "Read files".into(), "file".into(), 0.80),
        ];
        let result = ToolSearchResult::new(tools, false);
        assert_eq!(result.found_tools.len(), 2);
        assert!(!result.exhaustive);
        assert!(!result.is_empty());
    }

    #[test]
    fn found_tool_serialization_roundtrip() {
        let tool = FoundTool::new("bash".into(), "desc".into(), "hint".into(), 0.9);
        let json_str = serde_json::to_string(&tool).expect("serialize");
        let back: FoundTool = serde_json::from_str(&json_str).expect("deserialize");
        assert_eq!(tool, back);
    }

    // -----------------------------------------------------------------------
    // is_deferred_tool_call tests
    // -----------------------------------------------------------------------

    #[test]
    fn is_deferred_tool_call_exact_match() {
        assert!(is_deferred_tool_call("toolsearch", &json!({})));
    }

    #[test]
    fn is_deferred_tool_call_case_insensitive() {
        assert!(is_deferred_tool_call("ToolSearch", &json!({})));
        assert!(is_deferred_tool_call("TOOLSEARCH", &json!({})));
    }

    #[test]
    fn is_deferred_tool_call_not_match() {
        assert!(!is_deferred_tool_call("bash", &json!({})));
        assert!(!is_deferred_tool_call("read_file", &json!({})));
    }

    // -----------------------------------------------------------------------
    // build_deferred_tool_list tests
    // -----------------------------------------------------------------------

    #[test]
    fn build_deferred_tool_list_disabled_returns_all() {
        let tools = vec![
            json!({"name": "bash", "description": "Run commands"}),
            json!({"name": "rare", "description": "Rare tool"}),
        ];
        let config = DeferredToolConfig::disabled();
        let result = build_deferred_tool_list(&tools, &config);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0]["description"], "Run commands");
    }

    #[test]
    fn build_deferred_tool_list_defers_matching() {
        let tools = vec![
            json!({"name": "bash", "description": "Run commands", "input_schema": {}}),
            json!({"name": "rare_tool", "description": "Rare tool", "input_schema": {}}),
        ];
        let config = DeferredToolConfig::new(true, vec!["bash".into()], vec!["rare_tool".into()]);
        let result = build_deferred_tool_list(&tools, &config);
        assert_eq!(result.len(), 2);

        // bash is always loaded — full definition preserved
        assert_eq!(result[0]["name"], "bash");
        assert_eq!(result[0]["description"], "Run commands");
        assert!(result[0].get("deferred").is_none());

        // rare_tool is deferred — replaced with placeholder
        assert_eq!(result[1]["name"], "rare_tool");
        assert_eq!(result[1]["deferred"], true);
        assert!(
            result[1]["description"]
                .as_str()
                .expect("deferred tool should keep a description")
                .contains("deferred")
        );
    }

    // -----------------------------------------------------------------------
    // TaskBudgetParams tests
    // -----------------------------------------------------------------------

    #[test]
    fn task_budget_unlimited() {
        let budget = TaskBudgetParams::unlimited();
        assert!(!budget.has_limit());
        assert!(budget.max_turns.is_none());
        assert!(budget.max_total_tokens.is_none());
        assert!(budget.max_budget_usd.is_none());
    }

    #[test]
    fn task_budget_with_turns() {
        let budget = TaskBudgetParams::with_max_turns(10);
        assert!(budget.has_limit());
        assert_eq!(budget.max_turns, Some(10));
    }

    #[test]
    fn task_budget_with_tokens() {
        let budget = TaskBudgetParams::with_max_tokens(100_000);
        assert_eq!(budget.max_total_tokens, Some(100_000));
    }

    #[test]
    fn task_budget_with_usd() {
        let budget = TaskBudgetParams::with_budget_usd(5.0);
        assert_eq!(budget.max_budget_usd, Some(5.0));
    }

    #[test]
    fn task_budget_merge_restrictive_takes_min() {
        let a = TaskBudgetParams::with_max_turns(10);
        let b = TaskBudgetParams::with_max_turns(5);
        let merged = a.merge_restrictive(&b);
        assert_eq!(merged.max_turns, Some(5));
    }

    #[test]
    fn task_budget_merge_restrictive_fills_none() {
        let a = TaskBudgetParams::unlimited();
        let b = TaskBudgetParams::with_max_turns(3);
        let merged = a.merge_restrictive(&b);
        assert_eq!(merged.max_turns, Some(3));
    }

    #[test]
    fn task_budget_serialization_roundtrip() {
        let budget = TaskBudgetParams {
            max_turns: Some(10),
            max_total_tokens: Some(500_000),
            max_budget_usd: Some(2.5),
        };
        let json_str = serde_json::to_string(&budget).expect("serialize");
        let back: TaskBudgetParams = serde_json::from_str(&json_str).expect("deserialize");
        assert_eq!(budget, back);
    }

    // -----------------------------------------------------------------------
    // AdvisorModelConfig / AdvisorResult tests
    // -----------------------------------------------------------------------

    #[test]
    fn advisor_model_config_disabled() {
        let config = AdvisorModelConfig::disabled();
        assert!(!config.enabled);
        assert!(config.model.is_empty());
    }

    #[test]
    fn advisor_model_config_new() {
        let config = AdvisorModelConfig::new("claude-3-haiku".into(), 1024);
        assert!(config.enabled);
        assert_eq!(config.model, "claude-3-haiku");
        assert_eq!(config.max_tokens, 1024);
    }

    #[test]
    fn advisor_result_proceed() {
        let result = AdvisorResult::proceed("Looks good".into(), 0.9);
        assert!(result.should_proceed);
        assert_eq!(result.summary, "Looks good");
        assert!((result.confidence - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn advisor_result_stop() {
        let result = AdvisorResult::stop("Too risky".into(), 0.85);
        assert!(!result.should_proceed);
    }

    #[test]
    fn advisor_result_is_confident() {
        let result = AdvisorResult::proceed("ok".into(), 0.8);
        assert!(result.is_confident(0.7));
        assert!(!result.is_confident(0.9));
    }

    #[test]
    fn advisor_result_serialization_roundtrip() {
        let result = AdvisorResult::proceed("summary".into(), 0.75);
        let json_str = serde_json::to_string(&result).expect("serialize");
        let back: AdvisorResult = serde_json::from_str(&json_str).expect("deserialize");
        assert_eq!(result, back);
    }

    // -----------------------------------------------------------------------
    // PromptCacheScope tests
    // -----------------------------------------------------------------------

    #[test]
    fn prompt_cache_scope_ttl() {
        assert_eq!(PromptCacheScope::None.ttl_seconds(), None);
        assert_eq!(PromptCacheScope::Short.ttl_seconds(), Some(300));
        assert_eq!(PromptCacheScope::Hour.ttl_seconds(), Some(3600));
    }

    #[test]
    fn prompt_cache_scope_to_cache_control() {
        assert!(PromptCacheScope::None.to_cache_control().is_none());
        let cc = PromptCacheScope::Short.to_cache_control().expect("short");
        assert_eq!(cc["type"], "ephemeral");
        let cc2 = PromptCacheScope::Hour.to_cache_control().expect("hour");
        assert_eq!(cc2["type"], "ephemeral");
    }

    #[test]
    fn prompt_cache_scope_default() {
        assert_eq!(PromptCacheScope::default(), PromptCacheScope::Short);
    }

    #[test]
    fn prompt_cache_scope_serialization_roundtrip() {
        let scope = PromptCacheScope::Hour;
        let json_str = serde_json::to_string(&scope).expect("serialize");
        let back: PromptCacheScope = serde_json::from_str(&json_str).expect("deserialize");
        assert_eq!(scope, back);
    }

    // -----------------------------------------------------------------------
    // apply_cache_control tests
    // -----------------------------------------------------------------------

    #[test]
    fn apply_cache_control_none_does_nothing() {
        let mut messages = vec![json!({"role": "user", "content": "hello"})];
        apply_cache_control(&mut messages, PromptCacheScope::None);
        assert!(messages[0].get("cache_control").is_none());
    }

    #[test]
    fn apply_cache_control_short_adds_to_last_user() {
        let mut messages = vec![
            json!({"role": "system", "content": "You are helpful."}),
            json!({"role": "user", "content": "hello"}),
            json!({"role": "assistant", "content": "hi"}),
            json!({"role": "user", "content": "how are you?"}),
        ];
        apply_cache_control(&mut messages, PromptCacheScope::Short);

        // Last user message should have cache_control
        assert_eq!(messages[3]["cache_control"]["type"], "ephemeral");
        // First system message should have cache_control
        assert_eq!(messages[0]["cache_control"]["type"], "ephemeral");
        // Assistant message should NOT have cache_control
        assert!(messages[2].get("cache_control").is_none());
        // First user message should NOT have cache_control (only last)
        assert!(messages[1].get("cache_control").is_none());
    }

    #[test]
    fn apply_cache_control_no_user_message() {
        let mut messages = vec![
            json!({"role": "system", "content": "system"}),
            json!({"role": "assistant", "content": "hi"}),
        ];
        apply_cache_control(&mut messages, PromptCacheScope::Hour);
        // System should still get cache_control
        assert_eq!(messages[0]["cache_control"]["type"], "ephemeral");
        // Assistant should not
        assert!(messages[1].get("cache_control").is_none());
    }

    #[test]
    fn apply_cache_control_empty_messages() {
        let mut messages: Vec<Value> = vec![];
        // Should not panic
        apply_cache_control(&mut messages, PromptCacheScope::Short);
        assert!(messages.is_empty());
    }
}
