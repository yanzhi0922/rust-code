use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{Attachment, ConversationEntry, ConversationRole, ToolCall};

/// Provenance of a v2 runtime message.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageOrigin {
    UserInput,
    Provider,
    Tool,
    System,
    Hook,
    Compact,
    Agent,
    Replay,
}

/// Metadata shared by every v2 runtime message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageBase {
    pub uuid: Uuid,
    pub parent_uuid: Option<Uuid>,
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub is_meta: bool,
    #[serde(default)]
    pub is_virtual: bool,
    #[serde(default)]
    pub is_compact_summary: bool,
    #[serde(default)]
    pub is_visible_in_transcript_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<MessageOrigin>,
}

impl Default for MessageBase {
    fn default() -> Self {
        Self {
            uuid: Uuid::new_v4(),
            parent_uuid: None,
            timestamp: Utc::now(),
            is_meta: false,
            is_virtual: false,
            is_compact_summary: false,
            is_visible_in_transcript_only: false,
            origin: None,
        }
    }
}

impl MessageBase {
    /// Create a base with the supplied origin.
    #[must_use]
    pub fn with_origin(origin: MessageOrigin) -> Self {
        Self {
            origin: Some(origin),
            ..Self::default()
        }
    }
}

/// Assistant content blocks aligned with Claude Code's richer message model.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantContentBlock {
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: Value,
    },
    Text {
        text: String,
    },
    RedactedThinking {
        data: String,
    },
    Thinking {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    AdvisorToolResult {
        content: String,
    },
}

/// User-originated message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessage {
    pub base: MessageBase,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    /// Provider-facing user content blocks preserved across compat/runtime
    /// conversions. This is used for system-reminder style meta messages.
    #[serde(default)]
    pub provider_content_blocks: Vec<Value>,
    /// Metadata attached to compact summary messages for UI rendering.
    ///
    /// When `Some`, contains the number of messages summarized, optional
    /// user context (feedback), and the direction of partial compaction.
    /// Mirrors `summarizeMetadata` from the TS reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summarize_metadata: Option<Value>,
}

impl UserMessage {
    #[must_use]
    pub fn provider_content_blocks(&self) -> Vec<Value> {
        if !self.provider_content_blocks.is_empty() {
            return self.provider_content_blocks.clone();
        }

        let mut blocks = vec![json!({
            "type": "text",
            "text": self.text,
        })];
        for attachment in &self.attachments {
            blocks.push(json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": attachment.media_type.mime_type(),
                    "data": attachment.data,
                }
            }));
        }
        blocks
    }
}

/// Assistant-originated message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub base: MessageBase,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub blocks: Vec<AssistantContentBlock>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    /// Provider-facing assistant content blocks preserved for Anthropic-style
    /// replay across runtime bridges.
    #[serde(default)]
    pub provider_content_blocks: Vec<Value>,
}

impl AssistantMessage {
    #[must_use]
    pub fn provider_content_blocks(&self) -> Vec<Value> {
        if !self.provider_content_blocks.is_empty() {
            return self.provider_content_blocks.clone();
        }

        self.blocks
            .iter()
            .filter_map(assistant_block_to_provider_value)
            .collect()
    }
}

fn assistant_block_to_provider_value(block: &AssistantContentBlock) -> Option<Value> {
    match block {
        AssistantContentBlock::ToolUse { id, name, input } => Some(json!({
            "type": "tool_use",
            "id": id,
            "name": name,
            "input": input,
        })),
        AssistantContentBlock::Text { text } => Some(json!({
            "type": "text",
            "text": text,
        })),
        AssistantContentBlock::RedactedThinking { data } => Some(json!({
            "type": "redacted_thinking",
            "data": data,
        })),
        AssistantContentBlock::Thinking { text, signature } => {
            let mut block = json!({
                "type": "thinking",
                "thinking": text,
            });
            if let Some(signature) = signature {
                block["signature"] = Value::String(signature.clone());
            }
            Some(block)
        }
        AssistantContentBlock::AdvisorToolResult { content } => Some(json!({
            "type": "advisor_tool_result",
            "content": content,
        })),
    }
}

fn assistant_blocks_from_provider_content_blocks(
    content_blocks: &[Value],
) -> Vec<AssistantContentBlock> {
    content_blocks
        .iter()
        .filter_map(|block| {
            let block_type = block.get("type").and_then(Value::as_str)?;
            match block_type {
                "tool_use" => Some(AssistantContentBlock::ToolUse {
                    id: block.get("id").and_then(Value::as_str)?.to_owned(),
                    name: block.get("name").and_then(Value::as_str)?.to_owned(),
                    input: block.get("input").cloned().unwrap_or_else(|| json!({})),
                }),
                "text" => Some(AssistantContentBlock::Text {
                    text: block.get("text").and_then(Value::as_str)?.to_owned(),
                }),
                "redacted_thinking" => Some(AssistantContentBlock::RedactedThinking {
                    data: block.get("data").and_then(Value::as_str)?.to_owned(),
                }),
                "thinking" => Some(AssistantContentBlock::Thinking {
                    text: block
                        .get("thinking")
                        .or_else(|| block.get("text"))
                        .and_then(Value::as_str)?
                        .to_owned(),
                    signature: block
                        .get("signature")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                }),
                "advisor_tool_result" => Some(AssistantContentBlock::AdvisorToolResult {
                    content: block.get("content").and_then(Value::as_str)?.to_owned(),
                }),
                _ => None,
            }
        })
        .collect()
}

fn assistant_blocks_from_legacy_parts(
    text: &str,
    tool_calls: &[ToolCall],
    provider_content_blocks: &[Value],
) -> Vec<AssistantContentBlock> {
    let mut blocks = assistant_blocks_from_provider_content_blocks(provider_content_blocks);
    if !blocks.is_empty() {
        return blocks;
    }

    if !text.is_empty() {
        blocks.push(AssistantContentBlock::Text {
            text: text.to_owned(),
        });
    }
    blocks.extend(
        tool_calls
            .iter()
            .cloned()
            .map(|call| AssistantContentBlock::ToolUse {
                id: call.id,
                name: call.name,
                input: call.input,
            }),
    );
    blocks
}

/// Progress message emitted while work is underway.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressMessage {
    pub base: MessageBase,
    pub stage: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<u8>,
}

/// System-message subtype aligned with the parity plan.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SystemMessageSubtype {
    LocalCommand,
    BridgeStatus,
    TurnDuration,
    Thinking,
    MemorySaved,
    StopHookSummary,
    Informational,
    CompactBoundary,
    MicrocompactBoundary,
    PermissionRetry,
    ScheduledTaskFire,
    AwaySummary,
    AgentsKilled,
    ApiMetrics,
    ApiError,
    FileSnapshot,
}

/// System-originated message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMessage {
    pub base: MessageBase,
    pub subtype: SystemMessageSubtype,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Attachment-only helper message for UI/event streams.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentMessage {
    pub base: MessageBase,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}

/// Result of a hook execution rendered into the transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookResultMessage {
    pub base: MessageBase,
    pub hook_name: String,
    pub output: String,
    #[serde(default)]
    pub is_error: bool,
}

/// Summary of a tool invocation/result pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUseSummaryMessage {
    pub base: MessageBase,
    pub tool_call_id: String,
    pub tool_name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub is_error: bool,
    /// Provider-facing tool_result content preserved across runtime bridges.
    #[serde(default)]
    pub content_blocks: Vec<Value>,
}

/// Placeholder/tombstone marker used during streaming recovery or compaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TombstoneMessage {
    pub base: MessageBase,
    #[serde(default)]
    pub replaced_message_ids: Vec<Uuid>,
    pub summary: String,
}

/// Grouped rendering of multiple tool uses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupedToolUseMessage {
    pub base: MessageBase,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default)]
    pub summary: Option<String>,
}

/// Collapsed read/search results preserved as a compact summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollapsedReadSearchMessage {
    pub base: MessageBase,
    pub summary: String,
    #[serde(default)]
    pub items: Vec<String>,
}

/// Unified runtime message union for the v2 engine surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Message {
    User(UserMessage),
    Assistant(AssistantMessage),
    Progress(ProgressMessage),
    System(SystemMessage),
    Attachment(AttachmentMessage),
    HookResult(HookResultMessage),
    ToolUseSummary(ToolUseSummaryMessage),
    Tombstone(TombstoneMessage),
    GroupedToolUse(GroupedToolUseMessage),
    CollapsedReadSearch(CollapsedReadSearchMessage),
}

impl Message {
    /// Borrow the message base metadata.
    #[must_use]
    pub fn base(&self) -> &MessageBase {
        match self {
            Self::User(message) => &message.base,
            Self::Assistant(message) => &message.base,
            Self::Progress(message) => &message.base,
            Self::System(message) => &message.base,
            Self::Attachment(message) => &message.base,
            Self::HookResult(message) => &message.base,
            Self::ToolUseSummary(message) => &message.base,
            Self::Tombstone(message) => &message.base,
            Self::GroupedToolUse(message) => &message.base,
            Self::CollapsedReadSearch(message) => &message.base,
        }
    }

    /// Return the primary UUID for the message.
    #[must_use]
    pub fn uuid(&self) -> Uuid {
        self.base().uuid
    }

    /// Convert the message back into the legacy conversation format when possible.
    #[must_use]
    pub fn as_conversation_entry(&self) -> Option<ConversationEntry> {
        match self {
            Self::User(message) => Some(ConversationEntry {
                uuid: message.base.uuid,
                role: ConversationRole::User,
                text: message.text.clone(),
                history_text: None,
                content_blocks: message.provider_content_blocks(),
                tool_calls: Vec::new(),
                attachments: message.attachments.clone(),
                tool_call_id: None,
                name: None,
                is_error: false,
            }),
            Self::Assistant(message) => Some(ConversationEntry {
                uuid: message.base.uuid,
                role: ConversationRole::Assistant,
                text: message.text.clone(),
                history_text: None,
                content_blocks: message.provider_content_blocks(),
                tool_calls: message.tool_calls.clone(),
                attachments: Vec::new(),
                tool_call_id: None,
                name: None,
                is_error: false,
            }),
            Self::System(message) => Some(ConversationEntry {
                uuid: message.base.uuid,
                role: ConversationRole::System,
                text: message.text.clone(),
                history_text: None,
                content_blocks: Vec::new(),
                tool_calls: Vec::new(),
                attachments: Vec::new(),
                tool_call_id: None,
                name: system_subtype_name(&message.subtype).map(ToOwned::to_owned),
                is_error: matches!(message.subtype, SystemMessageSubtype::ApiError),
            }),
            Self::ToolUseSummary(message) => Some(ConversationEntry {
                uuid: message.base.uuid,
                role: ConversationRole::Tool,
                text: message.summary.clone(),
                history_text: None,
                content_blocks: message.content_blocks.clone(),
                tool_calls: Vec::new(),
                attachments: Vec::new(),
                tool_call_id: Some(message.tool_call_id.clone()),
                name: Some(message.tool_name.clone()),
                is_error: message.is_error,
            }),
            Self::Attachment(message) => Some(ConversationEntry::user_with_attachments(
                message.label.clone().unwrap_or_default(),
                message.attachments.clone(),
            )),
            Self::Progress(_)
            | Self::HookResult(_)
            | Self::Tombstone(_)
            | Self::GroupedToolUse(_)
            | Self::CollapsedReadSearch(_) => None,
        }
    }
}

impl From<ConversationEntry> for Message {
    fn from(value: ConversationEntry) -> Self {
        match value.role {
            ConversationRole::System => Self::System(SystemMessage {
                base: MessageBase {
                    uuid: value.uuid,
                    origin: Some(MessageOrigin::System),
                    ..MessageBase::default()
                },
                subtype: system_subtype_from_name(value.name.as_deref()),
                text: value.text,
                error: value.is_error.then_some("system_error".to_owned()),
            }),
            ConversationRole::User => Self::User(UserMessage {
                base: MessageBase::with_origin(MessageOrigin::UserInput),
                text: value.text,
                attachments: value.attachments,
                provider_content_blocks: value.content_blocks,
                summarize_metadata: None,
            }),
            ConversationRole::Assistant => Self::Assistant(AssistantMessage {
                base: MessageBase::with_origin(MessageOrigin::Provider),
                text: value.text.clone(),
                blocks: assistant_blocks_from_legacy_parts(
                    &value.text,
                    &value.tool_calls,
                    &value.content_blocks,
                ),
                tool_calls: value.tool_calls,
                provider_content_blocks: value.content_blocks,
            }),
            ConversationRole::Tool => Self::ToolUseSummary(ToolUseSummaryMessage {
                base: MessageBase::with_origin(MessageOrigin::Tool),
                tool_call_id: value
                    .tool_call_id
                    .unwrap_or_else(|| "unknown-tool-call".to_owned()),
                tool_name: value.name.unwrap_or_else(|| "unknown".to_owned()),
                summary: value.text,
                is_error: value.is_error,
                content_blocks: value.content_blocks,
            }),
        }
    }
}

fn system_subtype_name(subtype: &SystemMessageSubtype) -> Option<&'static str> {
    let name = match subtype {
        SystemMessageSubtype::LocalCommand => "local_command",
        SystemMessageSubtype::BridgeStatus => "bridge_status",
        SystemMessageSubtype::TurnDuration => "turn_duration",
        SystemMessageSubtype::Thinking => "thinking",
        SystemMessageSubtype::MemorySaved => "memory_saved",
        SystemMessageSubtype::StopHookSummary => "stop_hook_summary",
        SystemMessageSubtype::Informational => return None,
        SystemMessageSubtype::CompactBoundary => "compact_boundary",
        SystemMessageSubtype::MicrocompactBoundary => "microcompact_boundary",
        SystemMessageSubtype::PermissionRetry => "permission_retry",
        SystemMessageSubtype::ScheduledTaskFire => "scheduled_task_fire",
        SystemMessageSubtype::AwaySummary => "away_summary",
        SystemMessageSubtype::AgentsKilled => "agents_killed",
        SystemMessageSubtype::ApiMetrics => "api_metrics",
        SystemMessageSubtype::ApiError => "api_error",
        SystemMessageSubtype::FileSnapshot => "file_snapshot",
    };
    Some(name)
}

fn system_subtype_from_name(name: Option<&str>) -> SystemMessageSubtype {
    match name {
        Some("local_command") => SystemMessageSubtype::LocalCommand,
        Some("bridge_status") => SystemMessageSubtype::BridgeStatus,
        Some("turn_duration") => SystemMessageSubtype::TurnDuration,
        Some("thinking") => SystemMessageSubtype::Thinking,
        Some("memory_saved") => SystemMessageSubtype::MemorySaved,
        Some("stop_hook_summary") => SystemMessageSubtype::StopHookSummary,
        Some("compact_boundary") => SystemMessageSubtype::CompactBoundary,
        Some("microcompact_boundary") => SystemMessageSubtype::MicrocompactBoundary,
        Some("permission_retry") => SystemMessageSubtype::PermissionRetry,
        Some("scheduled_task_fire") => SystemMessageSubtype::ScheduledTaskFire,
        Some("away_summary") => SystemMessageSubtype::AwaySummary,
        Some("agents_killed") => SystemMessageSubtype::AgentsKilled,
        Some("api_metrics") => SystemMessageSubtype::ApiMetrics,
        Some("api_error") => SystemMessageSubtype::ApiError,
        Some("file_snapshot") => SystemMessageSubtype::FileSnapshot,
        _ => SystemMessageSubtype::Informational,
    }
}

#[cfg(test)]
mod tests {
    use super::{Message, MessageOrigin, SystemMessageSubtype};
    use crate::{ConversationEntry, ConversationRole, ToolCall};
    use serde_json::json;

    #[test]
    fn user_conversation_entry_round_trips_via_message() {
        let entry = ConversationEntry::user("ship it");
        let message = Message::from(entry.clone());
        let restored = message
            .as_conversation_entry()
            .expect("user message should down-convert");
        assert_eq!(restored.text, entry.text);
        assert_eq!(restored.role, entry.role);
        assert_eq!(restored.content_blocks.len(), 1);
        assert_eq!(restored.content_blocks[0]["type"], "text");
        assert_eq!(restored.content_blocks[0]["text"], "ship it");
    }

    #[test]
    fn assistant_tool_entry_becomes_tool_summary_message() {
        let message = Message::from(ConversationEntry::tool("tool-1", "bash", "ok", false));
        match message {
            Message::ToolUseSummary(summary) => {
                assert_eq!(summary.tool_call_id, "tool-1");
                assert_eq!(summary.tool_name, "bash");
                assert_eq!(summary.summary, "ok");
                assert_eq!(summary.base.origin, Some(MessageOrigin::Tool));
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[test]
    fn system_memory_saved_conversation_entry_preserves_subtype() {
        let mut entry = ConversationEntry::system(r#"{"writtenPaths":["C:/mem.md"]}"#);
        entry.name = Some("memory_saved".to_owned());
        let message = Message::from(entry.clone());
        assert!(matches!(
            message,
            Message::System(ref system) if system.subtype == SystemMessageSubtype::MemorySaved
        ));
        let restored = message
            .as_conversation_entry()
            .expect("system message should down-convert");
        assert_eq!(restored.uuid, entry.uuid);
        assert_eq!(restored.name.as_deref(), Some("memory_saved"));
        assert_eq!(restored.text, entry.text);
    }

    #[test]
    fn system_messages_mark_api_errors() {
        let message = Message::System(super::SystemMessage {
            base: super::MessageBase::with_origin(MessageOrigin::System),
            subtype: SystemMessageSubtype::ApiError,
            text: "api failed".to_owned(),
            error: Some("bad request".to_owned()),
        });
        let entry = message
            .as_conversation_entry()
            .expect("system message should down-convert");
        assert!(entry.is_error);
    }

    #[test]
    fn assistant_conversation_entry_round_trips_provider_content_blocks() {
        let entry = ConversationEntry {
            uuid: uuid::Uuid::new_v4(),
            role: ConversationRole::Assistant,
            text: String::new(),
            history_text: None,
            content_blocks: vec![
                json!({"type": "thinking", "thinking": "reasoning", "signature": "sig"}),
                json!({"type": "tool_use", "id": "call-1", "name": "read_file", "input": {"path": "src/lib.rs"}}),
            ],
            tool_calls: vec![ToolCall {
                id: "call-1".to_owned(),
                name: "read_file".to_owned(),
                input: json!({"path": "src/lib.rs"}),
            }],
            attachments: Vec::new(),
            tool_call_id: None,
            name: None,
            is_error: false,
        };

        let restored = Message::from(entry)
            .as_conversation_entry()
            .expect("assistant message should down-convert");

        assert_eq!(restored.content_blocks.len(), 2);
        assert_eq!(restored.content_blocks[0]["type"], "thinking");
        assert_eq!(restored.content_blocks[0]["thinking"], "reasoning");
        assert_eq!(restored.content_blocks[0]["signature"], "sig");
        assert_eq!(restored.content_blocks[1]["type"], "tool_use");
        assert_eq!(restored.content_blocks[1]["id"], "call-1");
    }

    #[test]
    fn user_conversation_entry_round_trips_provider_content_blocks() {
        let entry = ConversationEntry {
            uuid: uuid::Uuid::new_v4(),
            role: ConversationRole::User,
            text: String::new(),
            history_text: Some("meta".to_owned()),
            content_blocks: vec![
                json!({"type": "text", "text": "<system-reminder>\nmeta\n</system-reminder>"}),
            ],
            tool_calls: Vec::new(),
            attachments: Vec::new(),
            tool_call_id: None,
            name: None,
            is_error: false,
        };

        let restored = Message::from(entry)
            .as_conversation_entry()
            .expect("user message should down-convert");
        assert_eq!(restored.content_blocks.len(), 1);
        assert_eq!(restored.content_blocks[0]["type"], "text");
        assert!(
            restored.content_blocks[0]["text"]
                .as_str()
                .expect("text block")
                .contains("system-reminder")
        );
    }

    #[test]
    fn tool_conversation_entry_round_trips_provider_content_blocks() {
        let entry = ConversationEntry {
            uuid: uuid::Uuid::new_v4(),
            role: ConversationRole::Tool,
            text: "tool-search".to_owned(),
            history_text: None,
            content_blocks: vec![
                json!({"type": "tool_reference", "tool_name": "read_mcp_resource"}),
            ],
            tool_calls: Vec::new(),
            attachments: Vec::new(),
            tool_call_id: Some("call-1".to_owned()),
            name: Some("tool_search".to_owned()),
            is_error: false,
        };

        let restored = Message::from(entry)
            .as_conversation_entry()
            .expect("tool message should down-convert");
        assert_eq!(restored.content_blocks.len(), 1);
        assert_eq!(restored.content_blocks[0]["type"], "tool_reference");
        assert_eq!(restored.content_blocks[0]["tool_name"], "read_mcp_resource");
    }
}
