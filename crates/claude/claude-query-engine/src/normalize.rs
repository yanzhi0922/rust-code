//! Message normalization pipeline for Anthropic API compliance.
//!
//! Runs a multi-pass pipeline to ensure messages are well-formed before sending to
//! the Anthropic Messages API. Mirrors the TS reference's
//! `normalizeMessagesForAPI` from `src/utils/messages.ts`.
//!
//! The implementation lives in `claude_provider::normalize`, which is the crate
//! that builds HTTP request bodies.  This module re-exports the public entry
//! point so that the query engine can call it without reaching through the
//! provider crate's namespace.

use serde_json::Value;

/// Re-export of the normalize config struct from the provider crate.
pub use claude_provider::normalize::NormalizeConfig;

/// Normalize a conversation (array of messages) for the Anthropic API.
///
/// Each message is a JSON object with `"role"` and `"content"` fields.
/// Delegates to the provider crate's full normalization pipeline.
pub fn normalize_messages_for_api(messages: &mut Vec<Value>) {
    claude_provider::normalize::normalize_messages_for_api(messages);
}

/// Normalize a conversation with tool-search configuration.
pub fn normalize_messages_for_api_with_config(
    messages: &mut Vec<Value>,
    config: NormalizeConfig<'_>,
) {
    claude_provider::normalize::normalize_messages_for_api_with_config(messages, config);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_orphaned_tool_use_gets_synthetic_result() {
        let mut messages = vec![
            json!({"role": "user", "content": "do something"}),
            json!({"role": "assistant", "content": [
                {"type": "tool_use", "id": "tool-1", "name": "Read", "input": {"path": "/foo"}}
            ]}),
            json!({"role": "user", "content": "next message"}),
        ];
        normalize_messages_for_api(&mut messages);

        // Synthetic tool_result is merged into the existing user message at index 2.
        let user_msg = &messages[2];
        assert_eq!(user_msg["role"], "user");
        let content = user_msg["content"]
            .as_array()
            .expect("normalized user content should be an array");
        assert!(
            content
                .iter()
                .any(|b| b["type"].as_str() == Some("tool_result")
                    && b["tool_use_id"].as_str() == Some("tool-1"))
        );
    }

    #[test]
    fn test_already_paired_no_insertion() {
        let mut messages = vec![
            json!({"role": "user", "content": "check this"}),
            json!({"role": "assistant", "content": [
                {"type": "tool_use", "id": "tool-1", "name": "Read", "input": {}}
            ]}),
            json!({"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "tool-1", "content": "ok"}
            ]}),
        ];
        normalize_messages_for_api(&mut messages);
        assert_eq!(messages.len(), 3);
    }

    #[test]
    fn test_consecutive_user_messages_merged() {
        let mut messages = vec![
            json!({"role": "user", "content": [{"type": "text", "text": "hello"}]}),
            json!({"role": "user", "content": [{"type": "text", "text": "world"}]}),
        ];
        normalize_messages_for_api(&mut messages);
        assert_eq!(messages.len(), 1);
        let content = messages[0]["content"]
            .as_array()
            .expect("merged user content should be an array");
        assert_eq!(content.len(), 2);
    }

    #[test]
    fn test_empty_text_blocks_stripped() {
        let mut messages = vec![json!({
            "role": "user",
            "content": [
                {"type": "text", "text": ""},
                {"type": "text", "text": "keep me"}
            ]
        })];
        normalize_messages_for_api(&mut messages);
        let content = messages[0]["content"]
            .as_array()
            .expect("stripped user content should be an array");
        assert!(content.iter().all(|b| {
            b["type"].as_str() != Some("text") || b["text"].as_str().is_none_or(|s| !s.is_empty())
        }));
    }

    #[test]
    fn test_first_message_is_user() {
        let mut messages = vec![json!({
            "role": "assistant",
            "content": [{"type": "text", "text": "hi"}]
        })];
        normalize_messages_for_api(&mut messages);
        assert_eq!(messages[0]["role"], "user");
    }

    #[test]
    fn test_unknown_block_type_replaced() {
        let mut messages = vec![json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "hello"},
                {"type": "unknown_type", "data": "x"}
            ]
        })];
        normalize_messages_for_api(&mut messages);
        let content = messages[0]["content"]
            .as_array()
            .expect("normalized user content should be an array");
        // unknown_type should be replaced with a text block
        assert!(
            content
                .iter()
                .all(|b| b["type"].as_str() != Some("unknown_type"))
        );
    }

    #[test]
    fn test_full_pipeline_role_alternation() {
        let mut messages = vec![
            json!({"role": "assistant", "content": [
                {"type": "text", "text": "let me check"},
                {"type": "tool_use", "id": "t1", "name": "Read", "input": {"path": "/x"}}
            ]}),
            json!({"role": "user", "content": [
                {"type": "text", "text": ""},
                {"type": "text", "text": "please help"}
            ]}),
            json!({"role": "user", "content": "thanks"}),
        ];
        normalize_messages_for_api(&mut messages);

        // First message should be user
        assert_eq!(messages[0]["role"], "user");

        // Verify role alternation
        for i in 1..messages.len() {
            assert_ne!(
                messages[i]["role"].as_str(),
                messages[i - 1]["role"].as_str(),
                "Consecutive messages at index {} have same role",
                i
            );
        }

        // Verify tool_use has matching tool_result
        let tool_use_ids: Vec<&str> = messages
            .iter()
            .flat_map(|m| m["content"].as_array().into_iter().flatten())
            .filter(|b| b["type"].as_str() == Some("tool_use"))
            .filter_map(|b| b["id"].as_str())
            .collect();
        let tool_result_ids: Vec<&str> = messages
            .iter()
            .flat_map(|m| m["content"].as_array().into_iter().flatten())
            .filter(|b| b["type"].as_str() == Some("tool_result"))
            .filter_map(|b| b["tool_use_id"].as_str())
            .collect();
        for id in &tool_use_ids {
            assert!(
                tool_result_ids.contains(id),
                "Missing tool_result for tool_use id {}",
                id
            );
        }
    }

    #[test]
    fn test_empty_conversation_is_unchanged() {
        let mut messages: Vec<Value> = vec![];
        normalize_messages_for_api(&mut messages);
        assert!(messages.is_empty());
    }
}
