use std::collections::BTreeSet;

use anyhow::Result;
use claude_core::{ConversationEntry, Message, StoredEvent, SystemMessageSubtype};
use serde::de::DeserializeOwned;
use serde_json::Value;
use uuid::Uuid;

use crate::SessionUsageSummary;
use crate::plan_state::PlanModeState;
use crate::resume_state::ResumeState;

/// Read-only semantic view over a session transcript.
#[derive(Debug, Clone)]
pub struct SessionTranscript {
    session_id: Uuid,
    events: Vec<StoredEvent>,
}

impl SessionTranscript {
    pub(crate) fn new(session_id: Uuid, events: Vec<StoredEvent>) -> Self {
        Self { session_id, events }
    }

    /// Return the session identifier associated with this transcript.
    #[must_use]
    pub fn session_id(&self) -> Uuid {
        self.session_id
    }

    /// Borrow all raw stored events in transcript order.
    #[must_use]
    pub fn events(&self) -> &[StoredEvent] {
        &self.events
    }

    /// Iterate over raw stored events in transcript order.
    pub fn iter_events(&self) -> impl DoubleEndedIterator<Item = &StoredEvent> {
        self.events.iter()
    }

    /// Project the transcript into conversation entries only.
    #[must_use]
    pub fn conversation_entries(&self) -> Vec<ConversationEntry> {
        let events = if let Some(boundary_index) = self
            .events
            .iter()
            .rposition(|event| event.event_type == "compact_boundary")
        {
            let suffix = &self.events[boundary_index + 1..];
            if suffix.iter().any(|event| event.conversation.is_some()) {
                suffix
            } else {
                &self.events
            }
        } else {
            &self.events
        };

        events
            .iter()
            .filter_map(|event| event.conversation.clone())
            .collect()
    }

    /// Collect tool names carried forward on compact-boundary metadata.
    #[must_use]
    pub fn carried_discovered_tool_names(&self) -> BTreeSet<String> {
        let mut discovered = BTreeSet::new();

        for event in &self.events {
            if event.event_type != "compact_boundary" {
                continue;
            }

            let Some(payload) = event.payload.clone() else {
                continue;
            };

            let Ok(boundary) =
                serde_json::from_value::<claude_transcript::CompactBoundary>(payload)
            else {
                continue;
            };

            discovered.extend(boundary.pre_compact_discovered_tools);
        }

        discovered
    }

    /// Return the latest payload stored for a named event type.
    #[must_use]
    pub fn latest_named_event_payload(&self, event_type: &str) -> Option<&Value> {
        self.events.iter().rev().find_map(|event| {
            (event.event_type == event_type)
                .then_some(event.payload.as_ref())
                .flatten()
        })
    }

    /// Deserialize the latest named event payload into a concrete type.
    ///
    /// # Errors
    /// Returns an error if the latest payload exists but cannot be decoded.
    pub fn latest_named_event_as<T>(&self, event_type: &str) -> Result<Option<T>>
    where
        T: DeserializeOwned,
    {
        self.latest_named_event_payload(event_type)
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(Into::into)
    }

    pub fn named_event_payloads<'a>(
        &'a self,
        event_type: &'a str,
    ) -> impl Iterator<Item = &'a Value> {
        self.events.iter().filter_map(move |event| {
            (event.event_type == event_type)
                .then_some(event.payload.as_ref())
                .flatten()
        })
    }

    #[must_use]
    pub fn runtime_messages(&self) -> Vec<Message> {
        self.events
            .iter()
            .filter(|event| event.event_type == "runtime_message")
            .filter_map(|event| event.payload.clone())
            .filter_map(|payload| serde_json::from_value::<Message>(payload).ok())
            .collect()
    }

    #[must_use]
    pub fn memory_saved_messages(&self) -> Vec<Message> {
        self.events
            .iter()
            .filter_map(|event| {
                if event.event_type == "conversation"
                    && let Some(entry) = event.conversation.as_ref()
                    && entry.role == claude_core::ConversationRole::System
                    && entry.name.as_deref() == Some("memory_saved")
                {
                    return Some(Message::from(entry.clone()));
                }

                if event.event_type == "runtime_message"
                    && let Some(payload) = event.payload.clone()
                    && let Ok(message) = serde_json::from_value::<Message>(payload)
                    && matches!(
                        message,
                        Message::System(ref system)
                            if system.subtype == SystemMessageSubtype::MemorySaved
                    )
                {
                    return Some(message);
                }

                None
            })
            .collect()
    }

    /// Load the latest persisted resume-state snapshot from the transcript.
    ///
    /// # Errors
    /// Returns an error if the persisted snapshot exists but cannot be decoded.
    pub fn latest_resume_state(&self) -> Result<Option<ResumeState>> {
        self.latest_named_event_as("resume_state")
    }

    /// Load the latest persisted plan-mode snapshot from the transcript.
    ///
    /// # Errors
    /// Returns an error if the persisted snapshot exists but cannot be decoded.
    pub fn latest_plan_mode_state(&self) -> Result<Option<PlanModeState>> {
        self.latest_named_event_as("plan_mode_state")
    }

    /// Count named events whose payload is marked as an error.
    #[must_use]
    pub fn named_event_error_count(&self) -> usize {
        self.events
            .iter()
            .filter_map(|event| event.payload.as_ref())
            .filter(|payload| {
                payload
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            })
            .count()
    }

    /// Aggregate input/output token usage from named event payloads.
    #[must_use]
    pub fn accumulated_usage(&self) -> SessionUsageSummary {
        let mut usage = SessionUsageSummary::default();
        for payload in self
            .events
            .iter()
            .filter_map(|event| event.payload.as_ref())
        {
            if let Some(event_usage) = payload.get("usage") {
                usage.input_tokens += event_usage
                    .get("input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
                usage.output_tokens += event_usage
                    .get("output_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
            }
        }
        usage
    }

    /// Return the most recent stop reason recorded in the transcript.
    #[must_use]
    pub fn last_stop_reason(&self) -> Option<String> {
        self.events.iter().rev().find_map(|event| {
            event.payload.as_ref().and_then(|payload| {
                payload
                    .get("stop_reason")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
        })
    }

    /// Collect all once-only hook ids that have already been consumed.
    #[must_use]
    pub fn consumed_once_hook_ids(&self) -> BTreeSet<String> {
        self.events
            .iter()
            .filter(|event| event.event_type == "hook_execution")
            .filter_map(|event| event.payload.as_ref())
            .filter(|payload| payload.get("once").and_then(Value::as_bool) == Some(true))
            .filter_map(|payload| payload.get("hook_id").and_then(Value::as_str))
            .map(str::to_owned)
            .collect()
    }

    /// Check whether a hook phase event has already been recorded.
    #[must_use]
    pub fn has_hook_phase(&self, phase: &str) -> bool {
        self.events
            .iter()
            .filter(|event| event.event_type == "hook_phase")
            .filter_map(|event| event.payload.as_ref())
            .any(|payload| payload.get("phase").and_then(Value::as_str) == Some(phase))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use chrono::Utc;
    use claude_core::{Message, MessageBase, MessageOrigin, SystemMessage, SystemMessageSubtype};
    use uuid::Uuid;

    use super::SessionTranscript;
    use crate::resume_state::{PendingToolCall, ResumeState};

    #[test]
    fn transcript_projects_conversation_entries() {
        let session_id = Uuid::new_v4();
        let transcript = SessionTranscript::new(
            session_id,
            vec![
                claude_core::StoredEvent {
                    timestamp: Utc::now(),
                    session_id,
                    event_type: "conversation".to_owned(),
                    conversation: Some(claude_core::ConversationEntry::user("hello")),
                    payload: None,
                },
                claude_core::StoredEvent {
                    timestamp: Utc::now(),
                    session_id,
                    event_type: "result".to_owned(),
                    conversation: None,
                    payload: Some(serde_json::json!({"ok": true})),
                },
            ],
        );

        let conversation = transcript.conversation_entries();
        assert_eq!(conversation.len(), 1);
        assert_eq!(conversation[0].text, "hello");
        assert_eq!(transcript.session_id(), session_id);
    }

    #[test]
    fn transcript_projects_latest_compacted_suffix() {
        let session_id = Uuid::new_v4();
        let transcript = SessionTranscript::new(
            session_id,
            vec![
                claude_core::StoredEvent {
                    timestamp: Utc::now(),
                    session_id,
                    event_type: "conversation".to_owned(),
                    conversation: Some(claude_core::ConversationEntry::user("old")),
                    payload: None,
                },
                claude_core::StoredEvent {
                    timestamp: Utc::now(),
                    session_id,
                    event_type: "compact_boundary".to_owned(),
                    conversation: None,
                    payload: Some(serde_json::json!({
                        "trigger": "auto",
                        "pre_tokens": 100,
                    })),
                },
                claude_core::StoredEvent {
                    timestamp: Utc::now(),
                    session_id,
                    event_type: "conversation".to_owned(),
                    conversation: Some(claude_core::ConversationEntry::system("summary")),
                    payload: None,
                },
                claude_core::StoredEvent {
                    timestamp: Utc::now(),
                    session_id,
                    event_type: "conversation".to_owned(),
                    conversation: Some(claude_core::ConversationEntry::user("tail")),
                    payload: None,
                },
            ],
        );

        let conversation = transcript.conversation_entries();
        assert_eq!(conversation.len(), 2);
        assert_eq!(conversation[0].text, "summary");
        assert_eq!(conversation[1].text, "tail");
    }

    #[test]
    fn transcript_keeps_full_conversation_when_boundary_has_no_suffix() {
        let session_id = Uuid::new_v4();
        let transcript = SessionTranscript::new(
            session_id,
            vec![
                claude_core::StoredEvent {
                    timestamp: Utc::now(),
                    session_id,
                    event_type: "conversation".to_owned(),
                    conversation: Some(claude_core::ConversationEntry::user("old")),
                    payload: None,
                },
                claude_core::StoredEvent {
                    timestamp: Utc::now(),
                    session_id,
                    event_type: "compact_boundary".to_owned(),
                    conversation: None,
                    payload: Some(serde_json::json!({
                        "trigger": "auto",
                        "pre_tokens": 100,
                    })),
                },
            ],
        );

        let conversation = transcript.conversation_entries();
        assert_eq!(conversation.len(), 1);
        assert_eq!(conversation[0].text, "old");
    }

    #[test]
    fn transcript_reads_latest_resume_state() {
        let session_id = Uuid::new_v4();
        let transcript = SessionTranscript::new(
            session_id,
            vec![
                claude_core::StoredEvent {
                    timestamp: Utc::now(),
                    session_id,
                    event_type: "resume_state".to_owned(),
                    conversation: None,
                    payload: Some(serde_json::to_value(ResumeState::empty()).expect("serialize")),
                },
                claude_core::StoredEvent {
                    timestamp: Utc::now(),
                    session_id,
                    event_type: "resume_state".to_owned(),
                    conversation: None,
                    payload: Some(
                        serde_json::to_value(ResumeState::from_pending_calls(vec![
                            PendingToolCall {
                                id: "tool-1".to_owned(),
                                name: "bash".to_owned(),
                                input: serde_json::json!({"command": "pwd"}),
                            },
                        ]))
                        .expect("serialize"),
                    ),
                },
            ],
        );

        let state = transcript
            .latest_resume_state()
            .expect("resume state should decode")
            .expect("resume state should exist");
        assert_eq!(state.pending_tool_calls.len(), 1);
        assert_eq!(state.pending_tool_calls[0].name, "bash");
    }

    #[test]
    fn transcript_reads_carried_discovered_tool_names_from_boundaries() {
        let session_id = Uuid::new_v4();
        let transcript = SessionTranscript::new(
            session_id,
            vec![
                claude_core::StoredEvent {
                    timestamp: Utc::now(),
                    session_id,
                    event_type: "compact_boundary".to_owned(),
                    conversation: None,
                    payload: Some(serde_json::json!({
                        "trigger": "auto",
                        "pre_tokens": 100,
                        "pre_compact_discovered_tools": ["web_fetch", "mcp__context7__query_docs"],
                    })),
                },
                claude_core::StoredEvent {
                    timestamp: Utc::now(),
                    session_id,
                    event_type: "compact_boundary".to_owned(),
                    conversation: None,
                    payload: Some(serde_json::json!({
                        "trigger": "auto",
                        "pre_tokens": 120,
                        "pre_compact_discovered_tools": ["task_create"],
                    })),
                },
            ],
        );

        let discovered = transcript.carried_discovered_tool_names();
        assert!(discovered.contains("web_fetch"));
        assert!(discovered.contains("mcp__context7__query_docs"));
        assert!(discovered.contains("task_create"));
    }

    #[test]
    fn transcript_tracks_hook_state_usage_and_stop_reason() {
        let session_id = Uuid::new_v4();
        let transcript = SessionTranscript::new(
            session_id,
            vec![
                claude_core::StoredEvent {
                    timestamp: Utc::now(),
                    session_id,
                    event_type: "hook_execution".to_owned(),
                    conversation: None,
                    payload: Some(serde_json::json!({
                        "hook_id": "hook-1",
                        "once": true,
                    })),
                },
                claude_core::StoredEvent {
                    timestamp: Utc::now(),
                    session_id,
                    event_type: "hook_phase".to_owned(),
                    conversation: None,
                    payload: Some(serde_json::json!({
                        "phase": "session_start",
                    })),
                },
                claude_core::StoredEvent {
                    timestamp: Utc::now(),
                    session_id,
                    event_type: "result".to_owned(),
                    conversation: None,
                    payload: Some(serde_json::json!({
                        "is_error": true,
                        "stop_reason": "max_turns",
                        "usage": {
                            "input_tokens": 12,
                            "output_tokens": 34,
                        }
                    })),
                },
            ],
        );

        assert_eq!(
            transcript.consumed_once_hook_ids(),
            BTreeSet::from([String::from("hook-1")])
        );
        assert!(transcript.has_hook_phase("session_start"));
        assert_eq!(transcript.named_event_error_count(), 1);
        assert_eq!(transcript.last_stop_reason().as_deref(), Some("max_turns"));
        let usage = transcript.accumulated_usage();
        assert_eq!(usage.input_tokens, 12);
        assert_eq!(usage.output_tokens, 34);
    }

    #[test]
    fn transcript_reads_conversation_memory_saved_messages() {
        let session_id = Uuid::new_v4();
        let mut entry = claude_core::ConversationEntry::system(
            r#"{"writtenPaths":["C:/mem.md"],"teamCount":1}"#,
        );
        entry.name = Some("memory_saved".to_owned());
        let transcript = SessionTranscript::new(
            session_id,
            vec![claude_core::StoredEvent {
                timestamp: Utc::now(),
                session_id,
                event_type: "conversation".to_owned(),
                conversation: Some(entry),
                payload: None,
            }],
        );

        let messages = transcript.memory_saved_messages();
        assert_eq!(messages.len(), 1);
        assert!(matches!(
            &messages[0],
            Message::System(SystemMessage {
                subtype: SystemMessageSubtype::MemorySaved,
                ..
            })
        ));
    }

    #[test]
    fn transcript_keeps_legacy_runtime_memory_saved_messages() {
        let session_id = Uuid::new_v4();
        let transcript = SessionTranscript::new(
            session_id,
            vec![claude_core::StoredEvent {
                timestamp: Utc::now(),
                session_id,
                event_type: "runtime_message".to_owned(),
                conversation: None,
                payload: Some(
                    serde_json::to_value(Message::System(SystemMessage {
                        base: MessageBase::with_origin(MessageOrigin::System),
                        subtype: SystemMessageSubtype::MemorySaved,
                        text: "Saved 1 memory".to_owned(),
                        error: None,
                    }))
                    .expect("serialize message"),
                ),
            }],
        );

        let messages = transcript.memory_saved_messages();
        assert_eq!(messages.len(), 1);
        assert!(matches!(
            &messages[0],
            Message::System(SystemMessage {
                subtype: SystemMessageSubtype::MemorySaved,
                ..
            })
        ));
    }
}
