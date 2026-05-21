//! API microcompact — server-side compaction via an LLM API call.
//!
//! Unlike the local micro-compact (which simply clears old tool results),
//! the API microcompact sends messages to a model endpoint for summarisation.
//! This produces higher-quality compaction at the cost of an API round-trip.
//!
//! # Overview
//!
//! 1. Estimate token savings with [`estimate_savings`].
//! 2. Call [`api_microcompact`] to perform the compaction via an LLM.
//! 3. Receive a [`CompactResult`] with the compacted messages and metadata.

use claude_core::Message;

use crate::prompt::rough_token_count;
use crate::strategy::SummaryProvider;

// ---------------------------------------------------------------------------
// Token savings estimate
// ---------------------------------------------------------------------------

/// Token savings estimate for a compaction pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenSavings {
    /// Estimated token count before compaction.
    pub before: u64,
    /// Estimated token count after compaction.
    pub after: u64,
    /// Tokens saved (`before - after`).
    pub saved: u64,
}

impl TokenSavings {
    /// Compute savings ratio (0.0 – 1.0).
    #[must_use]
    pub fn ratio(&self) -> f64 {
        if self.before == 0 {
            return 0.0;
        }
        (self.saved as f64) / (self.before as f64)
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for API microcompaction.
#[derive(Debug, Clone)]
pub struct ApiMicrocompactConfig {
    /// Maximum output tokens the model may produce for the summary.
    pub max_output_tokens: u64,
    /// Model identifier (e.g., "claude-sonnet-4-20250514").
    pub model: String,
}

impl Default for ApiMicrocompactConfig {
    fn default() -> Self {
        Self {
            max_output_tokens: 4096,
            model: "claude-sonnet-4-20250514".to_owned(),
        }
    }
}

// ---------------------------------------------------------------------------
// Compact result
// ---------------------------------------------------------------------------

/// Result of an API microcompact operation.
#[derive(Debug, Clone)]
pub struct CompactResult {
    /// The compacted message list.
    pub messages: Vec<Message>,
    /// Token savings estimate.
    pub savings: TokenSavings,
    /// The summary text returned by the API (if any).
    pub summary: Option<String>,
}

// ---------------------------------------------------------------------------
// Micro-compact prompts
// ---------------------------------------------------------------------------

/// System prompt for the micro-compact LLM call.
const MICRO_COMPACT_SYSTEM_PROMPT: &str = "\
You are a concise summarizer for tool results in an AI coding assistant conversation.
Your task is to create brief, information-dense summaries of tool results that preserve
all critical information while drastically reducing token count.

Rules:
- Preserve all file paths, function names, error messages, and code snippets that are critical.
- Remove verbose output, repeated content, and decorative formatting.
- Keep the summary factual and precise — do not add interpretations.
- For each tool result, produce a single-line or short-paragraph summary.
- If a tool result is already short (< 100 chars), keep it as-is.";

/// Build the user prompt for micro-compaction.
///
/// Serializes all tool-result messages into a structured format that the LLM
/// can summarize.
fn build_micro_compact_user_prompt(messages: &[Message]) -> String {
    let mut prompt = String::from(
        "Summarize the following tool results for context compaction. \
         For each tool result, provide a concise summary that preserves \
         critical information (file paths, errors, key values).\n\n",
    );

    let mut idx = 0u64;
    for msg in messages {
        if let Message::ToolUseSummary(ts) = msg {
            idx += 1;
            // Truncate very long summaries in the prompt to avoid blowing up context
            let content_preview = if ts.summary.len() > 2000 {
                format!(
                    "{} [... truncated, {} chars total]",
                    &ts.summary[..2000],
                    ts.summary.len()
                )
            } else {
                ts.summary.clone()
            };
            prompt.push_str(&format!(
                "--- Tool Result #{} ---\nTool: {}\nCall ID: {}\nIs Error: {}\nContent:\n{}\n\n",
                idx,
                ts.tool_name,
                &ts.tool_call_id[..8.min(ts.tool_call_id.len())],
                ts.is_error,
                content_preview,
            ));
        }
    }

    if idx == 0 {
        prompt.push_str("(No tool results found in the message list.)\n");
    }

    prompt.push_str("\nProvide your summaries below, one per tool result, in this format:\n");
    prompt.push_str("#1: <summary>\n#2: <summary>\n...\n");

    prompt
}

/// Parse the LLM response to extract per-tool-result summaries.
///
/// Returns a vector of summary strings, one per tool result in order.
/// If parsing fails, returns a single-element vec with the full response.
fn parse_summaries_from_response(response: &str, expected_count: usize) -> Vec<String> {
    let mut summaries = Vec::with_capacity(expected_count.max(1));

    for line in response.lines() {
        let trimmed = line.trim();
        // Match lines like "#1: summary text" or "#1 - summary text"
        if let Some(rest) = trimmed.strip_prefix('#')
            && let Some(colon_pos) = rest.find([':', '-', '.'])
            && let Ok(_num) = rest[..colon_pos].trim().parse::<u64>()
        {
            let summary = rest[colon_pos + 1..].trim();
            if !summary.is_empty() {
                summaries.push(summary.to_owned());
            }
        }
    }

    // If we couldn't parse structured output, use the full response as a single summary
    if summaries.is_empty() && !response.trim().is_empty() {
        summaries.push(response.trim().to_owned());
    }

    summaries
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// Estimate token savings for a given message list.
///
/// This is a heuristic: tool-result messages are assumed to be replaceable
/// with a short placeholder (≈ 10 tokens each). The estimate does **not**
/// call the API.
#[must_use]
pub fn estimate_savings(messages: &[Message]) -> TokenSavings {
    let before = estimate_messages_tokens(messages);
    let tool_result_tokens: u64 = messages
        .iter()
        .map(|m| match m {
            Message::ToolUseSummary(s) => rough_token_count(&s.summary),
            _ => 0,
        })
        .sum();

    // Assume each tool result can be replaced with ~10 tokens.
    let tool_count = messages
        .iter()
        .filter(|m| matches!(m, Message::ToolUseSummary(_)))
        .count() as u64;

    let after = before
        .saturating_sub(tool_result_tokens)
        .saturating_add(tool_count * 10);

    TokenSavings {
        before,
        after,
        saved: before.saturating_sub(after),
    }
}

/// Perform an API microcompact using a real LLM call.
///
/// Sends tool-result messages to the configured LLM via the provided
/// [`SummaryProvider`], receives concise summaries, and replaces the
/// original tool results with the LLM-generated summaries.
///
/// # Errors
///
/// Returns an error if:
/// - The message list is empty
/// - The model name is empty
/// - The LLM API call fails
pub async fn api_microcompact(
    messages: &[Message],
    config: &ApiMicrocompactConfig,
    provider: &dyn SummaryProvider,
) -> anyhow::Result<CompactResult> {
    if messages.is_empty() {
        anyhow::bail!("cannot compact an empty message list");
    }
    if config.model.is_empty() {
        anyhow::bail!("model must not be empty");
    }

    let savings = estimate_savings(messages);

    // Count tool results to decide whether to call the LLM
    let tool_results: Vec<&Message> = messages
        .iter()
        .filter(|m| matches!(m, Message::ToolUseSummary(_)))
        .collect();

    if tool_results.is_empty() {
        // No tool results to compact — return messages unchanged
        return Ok(CompactResult {
            messages: messages.to_vec(),
            savings: TokenSavings {
                before: savings.before,
                after: savings.before,
                saved: 0,
            },
            summary: Some("No tool results to compact.".to_owned()),
        });
    }

    // Build the prompt and call the LLM
    let user_prompt = build_micro_compact_user_prompt(messages);

    tracing::info!(
        model = %config.model,
        max_output_tokens = config.max_output_tokens,
        tool_result_count = tool_results.len(),
        "api_microcompact: calling LLM for tool result summarization"
    );

    let llm_response = provider
        .generate_summary(messages, MICRO_COMPACT_SYSTEM_PROMPT, &user_prompt)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "api_microcompact LLM call failed");
            anyhow::anyhow!("API microcompact LLM call failed: {}", e)
        })?;

    tracing::debug!(
        response_len = llm_response.len(),
        "api_microcompact: received LLM response"
    );

    // Parse the structured summaries from the LLM response
    let parsed_summaries = parse_summaries_from_response(&llm_response, tool_results.len());

    // Apply summaries to the messages
    let mut summary_idx = 0usize;
    let compacted: Vec<Message> = messages
        .iter()
        .map(|msg| match msg {
            Message::ToolUseSummary(ts) => {
                let new_summary = if summary_idx < parsed_summaries.len() {
                    let s = parsed_summaries[summary_idx].clone();
                    summary_idx += 1;
                    s
                } else {
                    // Fallback: use a short placeholder if we ran out of parsed summaries
                    format!(
                        "[Compact: {} call {} — {}]",
                        ts.tool_name,
                        &ts.tool_call_id[..8.min(ts.tool_call_id.len())],
                        if ts.is_error { "error" } else { "ok" }
                    )
                };
                Message::ToolUseSummary(claude_core::ToolUseSummaryMessage {
                    summary: new_summary,
                    ..ts.clone()
                })
            }
            other => other.clone(),
        })
        .collect();

    let summary_text = format!(
        "Compacted {} tool results using model {} (max_output_tokens={})",
        tool_results.len(),
        config.model,
        config.max_output_tokens,
    );

    Ok(CompactResult {
        messages: compacted,
        savings,
        summary: Some(summary_text),
    })
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Estimate total tokens for a slice of messages.
fn estimate_messages_tokens(messages: &[Message]) -> u64 {
    messages.iter().map(estimate_single_message_tokens).sum()
}

/// Estimate tokens for a single message.
fn estimate_single_message_tokens(msg: &Message) -> u64 {
    match msg {
        Message::User(u) => rough_token_count(&u.text),
        Message::Assistant(a) => rough_token_count(&a.text),
        Message::ToolUseSummary(s) => rough_token_count(&s.summary),
        Message::System(s) => rough_token_count(&s.text),
        Message::HookResult(h) => rough_token_count(&h.output),
        Message::Tombstone(t) => rough_token_count(&t.summary),
        Message::Progress(p) => rough_token_count(&p.stage) + rough_token_count(&p.status),
        Message::Attachment(a) => a.label.as_ref().map(|l| rough_token_count(l)).unwrap_or(5),
        Message::GroupedToolUse(g) => g
            .summary
            .as_ref()
            .map(|s| rough_token_count(s))
            .unwrap_or(10),
        Message::CollapsedReadSearch(c) => rough_token_count(&c.summary),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use claude_core::{MessageBase, MessageOrigin, ToolUseSummaryMessage};
    use uuid::Uuid;

    /// Helper: create a tool-use-summary message.
    fn tool_summary(id: &str, content: &str) -> Message {
        Message::ToolUseSummary(ToolUseSummaryMessage {
            base: MessageBase {
                uuid: Uuid::new_v4(),
                parent_uuid: None,
                timestamp: chrono::Utc::now(),
                is_meta: false,
                is_virtual: false,
                is_compact_summary: false,
                is_visible_in_transcript_only: false,
                origin: Some(MessageOrigin::Tool),
            },
            tool_call_id: id.to_owned(),
            tool_name: "bash".to_owned(),
            summary: content.to_owned(),
            is_error: false,
            content_blocks: Vec::new(),
        })
    }

    /// Helper: create a user message.
    fn user_msg(text: &str) -> Message {
        Message::User(claude_core::UserMessage {
            base: MessageBase::with_origin(MessageOrigin::UserInput),
            text: text.to_owned(),
            attachments: Vec::new(),
            provider_content_blocks: Vec::new(),
            summarize_metadata: None,
        })
    }

    /// A mock SummaryProvider that returns a fixed response.
    struct MockProvider {
        response: String,
    }

    #[async_trait::async_trait]
    impl SummaryProvider for MockProvider {
        async fn generate_summary(
            &self,
            _messages: &[Message],
            _system_prompt: &str,
            _user_prompt: &str,
        ) -> Result<String, anyhow::Error> {
            Ok(self.response.clone())
        }
    }

    /// A mock SummaryProvider that always fails.
    struct FailingProvider;

    #[async_trait::async_trait]
    impl SummaryProvider for FailingProvider {
        async fn generate_summary(
            &self,
            _messages: &[Message],
            _system_prompt: &str,
            _user_prompt: &str,
        ) -> Result<String, anyhow::Error> {
            anyhow::bail!("mock provider failure")
        }
    }

    #[test]
    fn estimate_savings_returns_zero_for_empty() {
        let savings = estimate_savings(&[]);
        assert_eq!(savings.before, 0);
        assert_eq!(savings.after, 0);
        assert_eq!(savings.saved, 0);
        assert_eq!(savings.ratio(), 0.0);
    }

    #[test]
    fn estimate_savings_detects_tool_result_savings() {
        let long_content = "x".repeat(5000);
        let messages = vec![user_msg("short"), tool_summary("1", &long_content)];
        let savings = estimate_savings(&messages);
        assert!(savings.before > 0, "before should be positive");
        assert!(savings.saved > 0, "should detect savings from tool result");
        assert!(savings.ratio() > 0.0, "ratio should be positive");
    }

    #[tokio::test]
    async fn api_microcompact_rejects_empty_messages() {
        let config = ApiMicrocompactConfig::default();
        let provider = MockProvider {
            response: String::new(),
        };
        let result = api_microcompact(&[], &config, &provider).await;
        assert!(result.is_err(), "should reject empty message list");
    }

    #[tokio::test]
    async fn api_microcompact_rejects_empty_model() {
        let config = ApiMicrocompactConfig {
            model: String::new(),
            ..ApiMicrocompactConfig::default()
        };
        let provider = MockProvider {
            response: String::new(),
        };
        let messages = vec![user_msg("test")];
        let result = api_microcompact(&messages, &config, &provider).await;
        assert!(result.is_err(), "should reject empty model");
    }

    #[tokio::test]
    async fn api_microcompact_compacts_tool_results() {
        let config = ApiMicrocompactConfig {
            max_output_tokens: 2048,
            model: "test-model".to_owned(),
        };
        let messages = vec![
            user_msg("run this"),
            tool_summary("tc-12345678", &"very long tool output ".repeat(100)),
        ];

        let provider = MockProvider {
            response: "#1: Short summary of bash output".to_owned(),
        };

        let result = api_microcompact(&messages, &config, &provider)
            .await
            .expect("should succeed");
        assert_eq!(result.messages.len(), 2);
        assert!(result.summary.is_some());

        // Tool result should be replaced with the LLM-generated summary.
        match &result.messages[1] {
            Message::ToolUseSummary(s) => {
                assert_eq!(s.summary, "Short summary of bash output");
                assert!(s.summary.len() < 200, "summary should be short");
            }
            other => panic!("expected ToolUseSummary, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn api_microcompact_preserves_user_messages() {
        let config = ApiMicrocompactConfig::default();
        let provider = MockProvider {
            response: String::new(),
        };
        let messages = vec![user_msg("hello world")];
        let result = api_microcompact(&messages, &config, &provider)
            .await
            .expect("should succeed");

        // No tool results, so messages should be unchanged
        match &result.messages[0] {
            Message::User(u) => assert_eq!(u.text, "hello world"),
            other => panic!("expected User, got {other:?}"),
        }
        assert_eq!(result.savings.saved, 0);
    }

    #[tokio::test]
    async fn api_microcompact_handles_llm_failure() {
        let config = ApiMicrocompactConfig {
            max_output_tokens: 2048,
            model: "test-model".to_owned(),
        };
        let messages = vec![
            user_msg("run this"),
            tool_summary("tc-12345678", &"very long tool output ".repeat(100)),
        ];

        let result = api_microcompact(&messages, &config, &FailingProvider).await;
        assert!(result.is_err(), "should propagate LLM failure");
    }

    #[tokio::test]
    async fn api_microcompact_handles_multiple_tool_results() {
        let config = ApiMicrocompactConfig {
            max_output_tokens: 4096,
            model: "test-model".to_owned(),
        };
        let messages = vec![
            user_msg("do things"),
            tool_summary("tc-11111111", &"output a ".repeat(200)),
            tool_summary("tc-22222222", &"output b ".repeat(200)),
            tool_summary("tc-33333333", &"output c ".repeat(200)),
        ];

        let provider = MockProvider {
            response: "#1: Summary of tool A\n#2: Summary of tool B\n#3: Summary of tool C"
                .to_owned(),
        };

        let result = api_microcompact(&messages, &config, &provider)
            .await
            .expect("should succeed");

        // Verify all three tool results got their summaries
        let summaries: Vec<&str> = result
            .messages
            .iter()
            .filter_map(|m| match m {
                Message::ToolUseSummary(s) => Some(s.summary.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(summaries.len(), 3);
        assert_eq!(summaries[0], "Summary of tool A");
        assert_eq!(summaries[1], "Summary of tool B");
        assert_eq!(summaries[2], "Summary of tool C");
    }

    #[test]
    fn token_savings_ratio_handles_zero_before() {
        let savings = TokenSavings {
            before: 0,
            after: 0,
            saved: 0,
        };
        assert_eq!(savings.ratio(), 0.0);
    }

    #[test]
    fn token_savings_ratio_computes_correctly() {
        let savings = TokenSavings {
            before: 1000,
            after: 400,
            saved: 600,
        };
        let ratio = savings.ratio();
        assert!(
            (ratio - 0.6).abs() < 0.001,
            "ratio should be ~0.6, got {ratio}"
        );
    }

    #[test]
    fn parse_summaries_extracts_numbered_items() {
        let response = "#1: First summary\n#2: Second summary\n#3: Third summary";
        let summaries = parse_summaries_from_response(response, 3);
        assert_eq!(summaries.len(), 3);
        assert_eq!(summaries[0], "First summary");
        assert_eq!(summaries[1], "Second summary");
        assert_eq!(summaries[2], "Third summary");
    }

    #[test]
    fn parse_summaries_fallback_to_full_response() {
        let response = "This is a plain text summary without structured output.";
        let summaries = parse_summaries_from_response(response, 1);
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0], response.trim());
    }

    #[test]
    fn build_micro_compact_prompt_includes_tool_content() {
        let messages = vec![
            user_msg("hello"),
            tool_summary("tc-abc", "file contents here"),
        ];
        let prompt = build_micro_compact_user_prompt(&messages);
        assert!(prompt.contains("file contents here"));
        assert!(prompt.contains("bash"));
        assert!(prompt.contains("tc-abc"));
    }
}
