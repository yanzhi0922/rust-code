use std::fmt;

use chrono::{DateTime, Utc};
use claude_core::{ConversationEntry, Message, StoredEvent};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::boundary::CompactBoundary;

/// Shared metadata present on every transcript record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptRecordMeta {
    pub timestamp: DateTime<Utc>,
    pub session_id: Uuid,
}

impl TranscriptRecordMeta {
    #[must_use]
    pub fn new(session_id: Uuid, timestamp: DateTime<Utc>) -> Self {
        Self {
            timestamp,
            session_id,
        }
    }
}

/// Transcript record categories required by the Phase 1 transcript store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TranscriptEntryKind {
    Conversation,
    NamedEvent,
    CompactBoundary,
    RuntimeMessage,
}

/// JSONL record written by `TranscriptStorage`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum TranscriptEntry {
    Conversation {
        #[serde(flatten)]
        meta: TranscriptRecordMeta,
        entry: ConversationEntry,
    },
    NamedEvent {
        #[serde(flatten)]
        meta: TranscriptRecordMeta,
        event_type: String,
        #[serde(default)]
        payload: Option<Value>,
    },
    CompactBoundary {
        #[serde(flatten)]
        meta: TranscriptRecordMeta,
        boundary: CompactBoundary,
    },
    RuntimeMessage {
        #[serde(flatten)]
        meta: TranscriptRecordMeta,
        message: Message,
    },
}

impl TranscriptEntry {
    #[must_use]
    pub fn conversation(
        session_id: Uuid,
        timestamp: DateTime<Utc>,
        entry: ConversationEntry,
    ) -> Self {
        Self::Conversation {
            meta: TranscriptRecordMeta::new(session_id, timestamp),
            entry,
        }
    }

    #[must_use]
    pub fn conversation_now(session_id: Uuid, entry: ConversationEntry) -> Self {
        Self::conversation(session_id, Utc::now(), entry)
    }

    #[must_use]
    pub fn named_event(
        session_id: Uuid,
        timestamp: DateTime<Utc>,
        event_type: impl Into<String>,
        payload: Option<Value>,
    ) -> Self {
        Self::NamedEvent {
            meta: TranscriptRecordMeta::new(session_id, timestamp),
            event_type: event_type.into(),
            payload,
        }
    }

    #[must_use]
    pub fn named_event_now(
        session_id: Uuid,
        event_type: impl Into<String>,
        payload: Option<Value>,
    ) -> Self {
        Self::named_event(session_id, Utc::now(), event_type, payload)
    }

    #[must_use]
    pub fn compact_boundary(
        session_id: Uuid,
        timestamp: DateTime<Utc>,
        boundary: CompactBoundary,
    ) -> Self {
        Self::CompactBoundary {
            meta: TranscriptRecordMeta::new(session_id, timestamp),
            boundary,
        }
    }

    #[must_use]
    pub fn compact_boundary_now(session_id: Uuid, boundary: CompactBoundary) -> Self {
        Self::compact_boundary(session_id, Utc::now(), boundary)
    }

    #[must_use]
    pub fn runtime_message(session_id: Uuid, timestamp: DateTime<Utc>, message: Message) -> Self {
        Self::RuntimeMessage {
            meta: TranscriptRecordMeta::new(session_id, timestamp),
            message,
        }
    }

    #[must_use]
    pub fn runtime_message_now(session_id: Uuid, message: Message) -> Self {
        Self::runtime_message(session_id, Utc::now(), message)
    }

    #[must_use]
    pub fn kind(&self) -> TranscriptEntryKind {
        match self {
            Self::Conversation { .. } => TranscriptEntryKind::Conversation,
            Self::NamedEvent { .. } => TranscriptEntryKind::NamedEvent,
            Self::CompactBoundary { .. } => TranscriptEntryKind::CompactBoundary,
            Self::RuntimeMessage { .. } => TranscriptEntryKind::RuntimeMessage,
        }
    }

    #[must_use]
    pub fn meta(&self) -> &TranscriptRecordMeta {
        match self {
            Self::Conversation { meta, .. }
            | Self::NamedEvent { meta, .. }
            | Self::CompactBoundary { meta, .. }
            | Self::RuntimeMessage { meta, .. } => meta,
        }
    }

    #[must_use]
    pub fn session_id(&self) -> Uuid {
        self.meta().session_id
    }

    #[must_use]
    pub fn timestamp(&self) -> DateTime<Utc> {
        self.meta().timestamp
    }

    #[must_use]
    pub fn event_type(&self) -> &str {
        match self {
            Self::Conversation { .. } => "conversation",
            Self::NamedEvent { event_type, .. } => event_type,
            Self::CompactBoundary { .. } => "compact_boundary",
            Self::RuntimeMessage { .. } => "runtime_message",
        }
    }

    #[must_use]
    pub fn as_conversation(&self) -> Option<&ConversationEntry> {
        match self {
            Self::Conversation { entry, .. } => Some(entry),
            Self::NamedEvent { .. }
            | Self::CompactBoundary { .. }
            | Self::RuntimeMessage { .. } => None,
        }
    }

    #[must_use]
    pub fn as_named_event(&self) -> Option<(&str, Option<&Value>)> {
        match self {
            Self::NamedEvent {
                event_type,
                payload,
                ..
            } => Some((event_type.as_str(), payload.as_ref())),
            Self::Conversation { .. }
            | Self::CompactBoundary { .. }
            | Self::RuntimeMessage { .. } => None,
        }
    }

    #[must_use]
    pub fn as_compact_boundary(&self) -> Option<&CompactBoundary> {
        match self {
            Self::CompactBoundary { boundary, .. } => Some(boundary),
            Self::Conversation { .. } | Self::NamedEvent { .. } | Self::RuntimeMessage { .. } => {
                None
            }
        }
    }

    #[must_use]
    pub fn as_runtime_message(&self) -> Option<&Message> {
        match self {
            Self::RuntimeMessage { message, .. } => Some(message),
            Self::Conversation { .. } | Self::NamedEvent { .. } | Self::CompactBoundary { .. } => {
                None
            }
        }
    }
}

impl From<&TranscriptEntry> for StoredEvent {
    fn from(value: &TranscriptEntry) -> Self {
        match value {
            TranscriptEntry::Conversation { meta, entry } => Self {
                timestamp: meta.timestamp,
                session_id: meta.session_id,
                event_type: "conversation".to_owned(),
                conversation: Some(entry.clone()),
                payload: None,
            },
            TranscriptEntry::NamedEvent {
                meta,
                event_type,
                payload,
            } => Self {
                timestamp: meta.timestamp,
                session_id: meta.session_id,
                event_type: event_type.clone(),
                conversation: None,
                payload: payload.clone(),
            },
            TranscriptEntry::CompactBoundary { meta, boundary } => Self {
                timestamp: meta.timestamp,
                session_id: meta.session_id,
                event_type: "compact_boundary".to_owned(),
                conversation: None,
                payload: Some(
                    serde_json::to_value(boundary)
                        .expect("CompactBoundary serialization should not fail"),
                ),
            },
            TranscriptEntry::RuntimeMessage { meta, message } => Self {
                timestamp: meta.timestamp,
                session_id: meta.session_id,
                event_type: "runtime_message".to_owned(),
                conversation: None,
                payload: Some(
                    serde_json::to_value(message)
                        .expect("runtime message serialization should not fail"),
                ),
            },
        }
    }
}

impl From<TranscriptEntry> for StoredEvent {
    fn from(value: TranscriptEntry) -> Self {
        Self::from(&value)
    }
}

impl TryFrom<StoredEvent> for TranscriptEntry {
    type Error = TranscriptEntryConversionError;

    fn try_from(value: StoredEvent) -> Result<Self, Self::Error> {
        if value.event_type == "conversation" {
            let entry = value
                .conversation
                .ok_or(TranscriptEntryConversionError::MissingConversation)?;
            return Ok(Self::conversation(value.session_id, value.timestamp, entry));
        }

        if value.event_type == "compact_boundary" {
            let payload = value
                .payload
                .ok_or(TranscriptEntryConversionError::MissingPayload)?;
            let boundary = serde_json::from_value(payload)
                .map_err(TranscriptEntryConversionError::InvalidCompactBoundary)?;
            return Ok(Self::compact_boundary(
                value.session_id,
                value.timestamp,
                boundary,
            ));
        }

        if value.event_type == "runtime_message" {
            let payload = value
                .payload
                .ok_or(TranscriptEntryConversionError::MissingPayload)?;
            let message = serde_json::from_value(payload)
                .map_err(TranscriptEntryConversionError::InvalidRuntimeMessage)?;
            return Ok(Self::runtime_message(
                value.session_id,
                value.timestamp,
                message,
            ));
        }

        if value.conversation.is_some() {
            return Err(TranscriptEntryConversionError::MixedStoredEvent {
                event_type: value.event_type,
            });
        }

        Ok(Self::named_event(
            value.session_id,
            value.timestamp,
            value.event_type,
            value.payload,
        ))
    }
}

impl TryFrom<&TranscriptEntry> for ConversationEntry {
    type Error = TranscriptEntryConversionError;

    fn try_from(value: &TranscriptEntry) -> Result<Self, Self::Error> {
        value
            .as_conversation()
            .cloned()
            .ok_or(TranscriptEntryConversionError::UnexpectedKind {
                expected: TranscriptEntryKind::Conversation,
                found: value.kind(),
            })
    }
}

impl TryFrom<TranscriptEntry> for ConversationEntry {
    type Error = TranscriptEntryConversionError;

    fn try_from(value: TranscriptEntry) -> Result<Self, Self::Error> {
        Self::try_from(&value)
    }
}

/// Conversion failures between `StoredEvent`, `ConversationEntry`, and `TranscriptEntry`.
#[derive(Debug)]
pub enum TranscriptEntryConversionError {
    MissingConversation,
    MissingPayload,
    MixedStoredEvent {
        event_type: String,
    },
    InvalidCompactBoundary(serde_json::Error),
    InvalidRuntimeMessage(serde_json::Error),
    UnexpectedKind {
        expected: TranscriptEntryKind,
        found: TranscriptEntryKind,
    },
}

impl fmt::Display for TranscriptEntryConversionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingConversation => {
                write!(f, "stored conversation event is missing conversation data")
            }
            Self::MissingPayload => write!(f, "stored event is missing payload data"),
            Self::MixedStoredEvent { event_type } => write!(
                f,
                "stored event `{event_type}` contains conversation data and cannot be mapped to a named-event record"
            ),
            Self::InvalidCompactBoundary(err) => {
                write!(f, "failed to decode compact boundary payload: {err}")
            }
            Self::InvalidRuntimeMessage(err) => {
                write!(f, "failed to decode runtime message payload: {err}")
            }
            Self::UnexpectedKind { expected, found } => write!(
                f,
                "expected transcript entry kind {:?}, found {:?}",
                expected, found
            ),
        }
    }
}

impl std::error::Error for TranscriptEntryConversionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidCompactBoundary(err) => Some(err),
            Self::InvalidRuntimeMessage(err) => Some(err),
            Self::MissingConversation
            | Self::MissingPayload
            | Self::MixedStoredEvent { .. }
            | Self::UnexpectedKind { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use claude_core::{
        ConversationEntry, Message, MessageBase, MessageOrigin, StoredEvent, SystemMessage,
        SystemMessageSubtype,
    };
    use serde_json::json;
    use uuid::Uuid;

    use super::{TranscriptEntry, TranscriptEntryConversionError, TranscriptEntryKind};
    use crate::boundary::{CompactBoundary, CompactTrigger};

    #[test]
    fn stored_conversation_round_trips() {
        let session_id = Uuid::new_v4();
        let timestamp = Utc::now();
        let stored = StoredEvent {
            timestamp,
            session_id,
            event_type: "conversation".to_owned(),
            conversation: Some(ConversationEntry::assistant("hello")),
            payload: None,
        };

        let entry = TranscriptEntry::try_from(stored.clone()).expect("convert to transcript");
        assert_eq!(entry.kind(), TranscriptEntryKind::Conversation);
        assert_eq!(entry.as_conversation().expect("conversation").text, "hello");

        let restored: StoredEvent = (&entry).into();
        assert_eq!(restored.event_type, stored.event_type);
        assert_eq!(
            restored.conversation.expect("conversation").text,
            stored.conversation.expect("conversation").text
        );
    }

    #[test]
    fn stored_named_event_round_trips() {
        let session_id = Uuid::new_v4();
        let timestamp = Utc::now();
        let stored = StoredEvent {
            timestamp,
            session_id,
            event_type: "tool_result".to_owned(),
            conversation: None,
            payload: Some(json!({ "ok": true })),
        };

        let entry = TranscriptEntry::try_from(stored.clone()).expect("convert to transcript");
        let (event_type, payload) = entry.as_named_event().expect("named event");
        assert_eq!(event_type, "tool_result");
        assert_eq!(payload, Some(&json!({ "ok": true })));

        let restored: StoredEvent = entry.into();
        assert_eq!(restored.event_type, "tool_result");
        assert_eq!(restored.payload, stored.payload);
    }

    #[test]
    fn compact_boundary_round_trips_through_stored_event() {
        let session_id = Uuid::new_v4();
        let timestamp = Utc::now();
        let boundary = CompactBoundary {
            trigger: CompactTrigger::Manual,
            pre_tokens: 2048,
            user_context: Some("manual compact".to_owned()),
            messages_summarized: Some(4),
            pre_compact_discovered_tools: vec!["web_fetch".to_owned()],
            preserved_segment: None,
        };
        let stored = StoredEvent {
            timestamp,
            session_id,
            event_type: "compact_boundary".to_owned(),
            conversation: None,
            payload: Some(serde_json::to_value(&boundary).expect("serialize boundary")),
        };

        let entry = TranscriptEntry::try_from(stored).expect("convert to transcript");
        assert_eq!(entry.as_compact_boundary().expect("boundary"), &boundary);

        let restored: StoredEvent = (&entry).into();
        assert_eq!(restored.event_type, "compact_boundary");
        assert_eq!(
            restored.payload.expect("payload"),
            serde_json::to_value(boundary).expect("serialize boundary")
        );
    }

    #[test]
    fn runtime_message_round_trips_through_stored_event() {
        let session_id = Uuid::new_v4();
        let timestamp = Utc::now();
        let message = Message::System(SystemMessage {
            base: MessageBase::with_origin(MessageOrigin::System),
            subtype: SystemMessageSubtype::MemorySaved,
            text: "Saved 1 memory".to_owned(),
            error: None,
        });
        let stored = StoredEvent {
            timestamp,
            session_id,
            event_type: "runtime_message".to_owned(),
            conversation: None,
            payload: Some(serde_json::to_value(&message).expect("serialize message")),
        };

        let entry = TranscriptEntry::try_from(stored).expect("convert to transcript");
        let restored = entry.as_runtime_message().expect("runtime message");
        assert!(matches!(
            restored,
            Message::System(SystemMessage {
                subtype: SystemMessageSubtype::MemorySaved,
                ..
            })
        ));
    }

    #[test]
    fn conversation_conversion_rejects_non_conversation_entries() {
        let entry = TranscriptEntry::named_event_now(Uuid::new_v4(), "status", None);
        let error = ConversationEntry::try_from(entry).expect_err("should reject named event");

        assert!(matches!(
            error,
            TranscriptEntryConversionError::UnexpectedKind {
                expected: TranscriptEntryKind::Conversation,
                found: TranscriptEntryKind::NamedEvent,
            }
        ));
    }

    #[test]
    fn mixed_stored_event_is_rejected() {
        let error = TranscriptEntry::try_from(StoredEvent {
            timestamp: Utc::now(),
            session_id: Uuid::new_v4(),
            event_type: "tool_result".to_owned(),
            conversation: Some(ConversationEntry::assistant("unexpected")),
            payload: Some(json!({ "ok": true })),
        })
        .expect_err("mixed stored event should fail");

        assert!(matches!(
            error,
            TranscriptEntryConversionError::MixedStoredEvent { .. }
        ));
    }
}
