//! Message creation helpers matching Claude Code's `utils/messages.ts`.
//!
//! Provides factory functions for creating various message types used
//! throughout the query engine, along with utility functions for
//! message manipulation and inspection.

use std::collections::BTreeMap;

use claude_core::{
    AssistantContentBlock, AssistantMessage, ConversationEntry, ConversationRole, Message,
    MessageBase, MessageOrigin, SystemMessage, SystemMessageSubtype, ToolCall, ToolResult,
    ToolUseSummaryMessage, UserMessage,
};

/// Create a user text message.
pub fn create_user_message(text: &str) -> Message {
    Message::User(UserMessage {
        base: MessageBase::with_origin(MessageOrigin::UserInput),
        text: text.to_owned(),
        attachments: Vec::new(),
        provider_content_blocks: Vec::new(),
        summarize_metadata: None,
    })
}

/// Create a system message with a specific subtype.
pub fn create_system_message(text: &str, subtype: SystemMessageSubtype) -> Message {
    Message::System(SystemMessage {
        base: MessageBase::with_origin(MessageOrigin::System),
        subtype,
        text: text.to_owned(),
        error: None,
    })
}

/// Create an informational system message.
pub fn create_info_message(text: &str) -> Message {
    create_system_message(text, SystemMessageSubtype::Informational)
}

/// Create an error system message.
pub fn create_error_message(error: &str) -> Message {
    Message::System(SystemMessage {
        base: MessageBase::with_origin(MessageOrigin::System),
        subtype: SystemMessageSubtype::ApiError,
        text: error.to_owned(),
        error: Some(error.to_owned()),
    })
}

/// Create an interruption message (system notification that the query was interrupted).
pub fn create_interruption_message() -> Message {
    create_system_message(
        "Query interrupted by user",
        SystemMessageSubtype::Informational,
    )
}

/// Create a tool use summary message from a tool call and its result.
pub fn create_tool_use_summary(tool_call: &ToolCall, result: &ToolResult) -> Message {
    Message::ToolUseSummary(ToolUseSummaryMessage {
        base: MessageBase::with_origin(MessageOrigin::Tool),
        tool_call_id: tool_call.id.clone(),
        tool_name: tool_call.name.clone(),
        summary: result.content.clone(),
        is_error: result.is_error,
        content_blocks: result.content_blocks.clone(),
    })
}

/// Create a compact boundary message that marks where compaction occurred.
pub fn create_compact_boundary_message(reason: &str) -> Message {
    Message::System(SystemMessage {
        base: MessageBase {
            origin: Some(MessageOrigin::Compact),
            is_compact_summary: true,
            ..MessageBase::default()
        },
        subtype: SystemMessageSubtype::CompactBoundary,
        text: format!("[Context compacted: {reason}]"),
        error: None,
    })
}

/// Create an assistant message with text content.
pub fn create_assistant_text_message(text: &str) -> Message {
    Message::Assistant(AssistantMessage {
        base: MessageBase::with_origin(MessageOrigin::Provider),
        text: text.to_owned(),
        blocks: vec![AssistantContentBlock::Text {
            text: text.to_owned(),
        }],
        tool_calls: Vec::new(),
        provider_content_blocks: Vec::new(),
    })
}

/// Create an assistant message with tool use blocks.
pub fn create_assistant_tool_message(text: &str, tool_calls: Vec<ToolCall>) -> Message {
    let mut blocks = Vec::new();
    if !text.trim().is_empty() {
        blocks.push(AssistantContentBlock::Text {
            text: text.to_owned(),
        });
    }
    for tool_call in &tool_calls {
        blocks.push(AssistantContentBlock::ToolUse {
            id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            input: tool_call.input.clone(),
        });
    }
    Message::Assistant(AssistantMessage {
        base: MessageBase::with_origin(MessageOrigin::Provider),
        text: text.to_owned(),
        blocks,
        tool_calls,
        provider_content_blocks: Vec::new(),
    })
}

/// Strip signature blocks from thinking content in messages.
/// This is used before sending messages to the provider to avoid
/// leaking internal signature data.
pub fn strip_signature_blocks(messages: &mut [Message]) {
    for message in messages.iter_mut() {
        if let Message::Assistant(assistant) = message {
            for block in &mut assistant.blocks {
                if let AssistantContentBlock::Thinking { signature, .. } = block {
                    *signature = None;
                }
            }
        }
    }
}

/// Find the last compact boundary message and return the slice of messages after it.
/// If no compact boundary is found, returns the full slice.
pub fn get_messages_after_compact_boundary(messages: &[Message]) -> &[Message] {
    let last_boundary = messages.iter().rposition(|m| {
        matches!(
            m,
            Message::System(SystemMessage {
                base: MessageBase {
                    is_compact_summary: true,
                    ..
                },
                ..
            })
        ) || matches!(
            m,
            Message::System(SystemMessage {
                base: MessageBase {
                    origin: Some(MessageOrigin::Compact),
                    ..
                },
                ..
            })
        )
    });

    match last_boundary {
        Some(idx) => &messages[idx..],
        None => messages,
    }
}

/// Count the number of tool calls in a message.
pub fn count_tool_calls(messages: &[Message]) -> usize {
    messages
        .iter()
        .map(|m| match m {
            Message::Assistant(assistant) => assistant.tool_calls.len(),
            _ => 0,
        })
        .sum()
}

/// Extract all tool call IDs from a slice of messages.
pub fn collect_tool_call_ids(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .flat_map(|m| match m {
            Message::Assistant(assistant) => assistant
                .tool_calls
                .iter()
                .map(|tc| tc.id.clone())
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect()
}

/// Check if a message is an error message.
pub fn is_error_message(message: &Message) -> bool {
    matches!(
        message,
        Message::System(SystemMessage {
            subtype: SystemMessageSubtype::ApiError,
            ..
        })
    )
}

/// Check if a message is a tool use summary.
pub fn is_tool_summary(message: &Message) -> bool {
    matches!(message, Message::ToolUseSummary(_))
}

/// Generate synthetic tool result messages for tool calls that lack results.
///
/// Mirrors TS `yieldMissingToolResultBlocks`. When a model errors out after
/// emitting tool_use blocks but before receiving tool_result blocks, the
/// conversation has orphaned tool_use entries. This function finds all
/// assistant tool_use IDs that don't have a corresponding tool result and
/// creates synthetic error tool results for them.
pub fn prepend_user_context_to_conversation(
    conversation: Vec<ConversationEntry>,
    user_context: &BTreeMap<String, String>,
) -> Vec<ConversationEntry> {
    if user_context.is_empty() {
        return conversation;
    }
    let context_lines: Vec<String> = user_context
        .iter()
        .map(|(key, value)| format!("# {key}\n{value}"))
        .collect();
    let content = format!(
        "<system-reminder>\nAs you answer the user's questions, you can use the following context:\n{}\n\nIMPORTANT: this context may or may not be relevant to your tasks. You should not respond to this context unless it is highly relevant to your task.\n</system-reminder>",
        context_lines.join("\n")
    );
    let mut result = vec![ConversationEntry::user(&content)];
    result.extend(conversation);
    result
}

pub fn append_system_context_to_conversation(
    conversation: &mut Vec<ConversationEntry>,
    system_context: &BTreeMap<String, String>,
) {
    if system_context.is_empty() {
        return;
    }
    let context_text = system_context
        .iter()
        .map(|(key, value)| format!("{key}: {value}"))
        .collect::<Vec<_>>()
        .join("\n");
    if let Some(entry) = conversation
        .iter_mut()
        .find(|e| matches!(e.role, ConversationRole::System))
    {
        entry.text = format!("{}\n{}", entry.text, context_text);
    } else {
        conversation.push(ConversationEntry::system(&context_text));
    }
}

pub fn yield_missing_tool_result_messages(messages: &[Message], error_text: &str) -> Vec<Message> {
    // Collect all tool_call IDs from assistant messages
    let mut tool_call_ids: Vec<String> = Vec::new();
    for msg in messages {
        if let Message::Assistant(assistant) = msg {
            for tc in &assistant.tool_calls {
                tool_call_ids.push(tc.id.clone());
            }
            for block in &assistant.blocks {
                if let AssistantContentBlock::ToolUse { id, .. } = block
                    && !tool_call_ids.contains(id)
                {
                    tool_call_ids.push(id.clone());
                }
            }
        }
    }

    // Collect all tool_call IDs that already have results
    let mut responded_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for msg in messages {
        if let Message::ToolUseSummary(summary) = msg {
            responded_ids.insert(summary.tool_call_id.clone());
        }
    }

    // Create synthetic results for missing tool calls
    let mut results = Vec::new();
    for id in tool_call_ids {
        if !responded_ids.contains(&id) {
            let entry = ConversationEntry::tool(&id, "unknown", error_text, true);
            results.push(Message::from(entry));
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use claude_core::{SystemMessageSubtype, ToolCall, ToolResult};
    use serde_json::json;

    use super::*;

    #[test]
    fn create_user_message_works() {
        let msg = create_user_message("hello");
        assert!(matches!(msg, Message::User(u) if u.text == "hello"));
    }

    #[test]
    fn create_system_message_works() {
        let msg = create_system_message("info", SystemMessageSubtype::Informational);
        assert!(matches!(
            msg,
            Message::System(SystemMessage {
                subtype: SystemMessageSubtype::Informational,
                text,
                ..
            }) if text == "info"
        ));
    }

    #[test]
    fn create_error_message_works() {
        let msg = create_error_message("something broke");
        assert!(matches!(
            msg,
            Message::System(SystemMessage {
                subtype: SystemMessageSubtype::ApiError,
                error: Some(e),
                ..
            }) if e == "something broke"
        ));
    }

    #[test]
    fn create_tool_use_summary_works() {
        let tool_call = ToolCall {
            id: "tc-1".into(),
            name: "bash".into(),
            input: json!({"command": "ls"}),
        };
        let result = ToolResult {
            content: "file1.txt".into(),
            is_error: false,
            content_blocks: Vec::new(),
            follow_up_user_blocks: Vec::new(),
        };
        let msg = create_tool_use_summary(&tool_call, &result);
        assert!(matches!(
            msg,
            Message::ToolUseSummary(ToolUseSummaryMessage {
                tool_call_id,
                tool_name,
                summary,
                is_error: false,
                ..
            }) if tool_call_id == "tc-1" && tool_name == "bash" && summary == "file1.txt"
        ));
    }

    #[test]
    fn create_compact_boundary_message_works() {
        let msg = create_compact_boundary_message("context limit");
        assert!(matches!(
            msg,
            Message::System(SystemMessage {
                base: MessageBase {
                    is_compact_summary: true,
                    ..
                },
                ..
            })
        ));
    }

    #[test]
    fn strip_signature_blocks_clears_signatures() {
        let mut messages = vec![create_assistant_text_message("hi")];
        // Manually inject a thinking block with signature
        if let Message::Assistant(ref mut assistant) = messages[0] {
            assistant.blocks.push(AssistantContentBlock::Thinking {
                text: "thoughts".into(),
                signature: Some("sig123".into()),
            });
        }
        strip_signature_blocks(&mut messages);
        if let Message::Assistant(assistant) = &messages[0] {
            for block in &assistant.blocks {
                if let AssistantContentBlock::Thinking { signature, .. } = block {
                    assert!(signature.is_none());
                }
            }
        }
    }

    #[test]
    fn get_messages_after_compact_boundary_finds_boundary() {
        let messages = vec![
            create_user_message("before"),
            create_compact_boundary_message("test"),
            create_user_message("after1"),
            create_user_message("after2"),
        ];
        // Mark the compact boundary properly
        let slice = get_messages_after_compact_boundary(&messages);
        assert_eq!(slice.len(), 3); // boundary + after1 + after2
    }

    #[test]
    fn get_messages_after_compact_boundary_returns_all_when_no_boundary() {
        let messages = vec![create_user_message("a"), create_user_message("b")];
        let slice = get_messages_after_compact_boundary(&messages);
        assert_eq!(slice.len(), 2);
    }

    #[test]
    fn count_tool_calls_works() {
        let messages = vec![
            create_assistant_tool_message(
                "",
                vec![
                    ToolCall {
                        id: "tc-1".into(),
                        name: "bash".into(),
                        input: json!({}),
                    },
                    ToolCall {
                        id: "tc-2".into(),
                        name: "read".into(),
                        input: json!({}),
                    },
                ],
            ),
            create_user_message("hello"),
            create_assistant_tool_message(
                "",
                vec![ToolCall {
                    id: "tc-3".into(),
                    name: "write".into(),
                    input: json!({}),
                }],
            ),
        ];
        assert_eq!(count_tool_calls(&messages), 3);
    }

    #[test]
    fn collect_tool_call_ids_works() {
        let messages = vec![create_assistant_tool_message(
            "",
            vec![
                ToolCall {
                    id: "tc-1".into(),
                    name: "bash".into(),
                    input: json!({}),
                },
                ToolCall {
                    id: "tc-2".into(),
                    name: "read".into(),
                    input: json!({}),
                },
            ],
        )];
        let ids = collect_tool_call_ids(&messages);
        assert_eq!(ids, vec!["tc-1", "tc-2"]);
    }

    #[test]
    fn is_error_message_and_is_tool_summary() {
        let error = create_error_message("fail");
        assert!(is_error_message(&error));
        let user = create_user_message("hello");
        assert!(!is_error_message(&user));

        let tool_call = ToolCall {
            id: "tc-1".into(),
            name: "bash".into(),
            input: json!({}),
        };
        let result = ToolResult {
            content: "ok".into(),
            is_error: false,
            content_blocks: Vec::new(),
            follow_up_user_blocks: Vec::new(),
        };
        let summary = create_tool_use_summary(&tool_call, &result);
        assert!(is_tool_summary(&summary));
        assert!(!is_tool_summary(&user));
    }

    #[test]
    fn create_interruption_message_works() {
        let msg = create_interruption_message();
        assert!(matches!(msg, Message::System(_)));
    }

    #[test]
    fn create_assistant_text_message_works() {
        let msg = create_assistant_text_message("response");
        assert!(matches!(
            msg,
            Message::Assistant(AssistantMessage { text, .. }) if text == "response"
        ));
    }
}
