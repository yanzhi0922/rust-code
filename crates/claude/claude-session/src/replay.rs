//! Session replay and event summarisation.
//!
//! Converts raw [`StoredEvent`] sequences into
//! human-readable [`ReplayEvent`] entries for display in the TUI or export.

use std::collections::HashMap;

use anyhow::Result;

use claude_core::{ConversationRole, StoredEvent};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::SessionStore;
use crate::transcript::SessionTranscript;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ReplayEvent {
    PromptStarted {
        prompt: String,
        provider: String,
        model: Option<String>,
    },
    AssistantTurn {
        turn: usize,
        text_preview: String,
        stop_reason: String,
        tool_calls: usize,
    },
    ToolResult {
        tool_name: String,
        tool_use_id: String,
        is_error: bool,
        content_preview: String,
    },
    HookRun {
        hook_id: String,
        event: String,
        status: String,
    },
    Result {
        is_error: bool,
        stop_reason: String,
        duration_ms: u64,
    },
    SessionContext {
        cwd: String,
        provider: String,
    },
    Other {
        event_type: String,
        payload: Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayVerification {
    pub ok: bool,
    pub total_events: usize,
    pub prompt_count: usize,
    pub tool_call_count: usize,
    pub issues: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplaySummary {
    pub session_id: Uuid,
    pub duration_ms: u64,
    pub total_turns: usize,
    pub total_tool_calls: usize,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub errors: usize,
    pub events: Vec<ReplayEvent>,
}

pub struct SessionReplay {
    events: Vec<ReplayEvent>,
}

impl SessionReplay {
    /// # Errors
    /// Returns an error if session events cannot be loaded from the store.
    pub fn load(session_id: Uuid, store: &SessionStore) -> Result<Self> {
        let transcript = store.load_transcript(session_id)?;
        Ok(Self::from_transcript(&transcript))
    }

    #[must_use]
    pub fn from_transcript(transcript: &SessionTranscript) -> Self {
        let events = transcript.iter_events().map(convert_event).collect();
        Self { events }
    }

    #[must_use]
    pub fn events(&self) -> &[ReplayEvent] {
        &self.events
    }

    #[must_use]
    pub fn verify(&self) -> ReplayVerification {
        let mut issues = Vec::new();
        let mut prompt_count = 0usize;
        let mut result_count = 0usize;
        let mut tool_call_count = 0usize;
        let mut tool_result_ids = HashMap::<String, usize>::new();
        let mut seen_turn_numbers = Vec::<usize>::new();

        for event in &self.events {
            match event {
                ReplayEvent::PromptStarted { .. } => {
                    prompt_count += 1;
                }
                ReplayEvent::AssistantTurn {
                    turn, tool_calls, ..
                } => {
                    seen_turn_numbers.push(*turn);
                    tool_call_count += tool_calls;
                }
                ReplayEvent::ToolResult { tool_use_id, .. } => {
                    *tool_result_ids.entry(tool_use_id.clone()).or_insert(0) += 1;
                }
                ReplayEvent::Result { .. } => {
                    result_count += 1;
                }
                _ => {}
            }
        }

        if prompt_count > 0 && result_count != prompt_count {
            issues.push(format!(
                "prompt/result mismatch: {prompt_count} prompts but {result_count} results"
            ));
        }

        for (id, count) in &tool_result_ids {
            if *count > 1 {
                issues.push(format!(
                    "duplicate tool result for id '{id}' ({count} occurrences)"
                ));
            }
        }

        if !seen_turn_numbers.is_empty() {
            let mut sorted = seen_turn_numbers.clone();
            sorted.sort_unstable();
            sorted.dedup();
            for (i, &turn) in sorted.iter().enumerate() {
                let expected = i + 1;
                if turn != expected {
                    issues.push(format!(
                        "non-sequential turn number: found {turn} but expected {expected}"
                    ));
                }
            }
        }

        let ok = issues.is_empty();
        ReplayVerification {
            ok,
            total_events: self.events.len(),
            prompt_count,
            tool_call_count,
            issues,
        }
    }
}

/// # Errors
/// Returns an error if the session cannot be loaded or replay fails.
pub fn replay_session(session_id: Uuid, store: &SessionStore) -> Result<ReplaySummary> {
    let replay = SessionReplay::load(session_id, store)?;
    let events = replay.events.clone();

    let mut total_turns = 0usize;
    let mut total_tool_calls = 0usize;
    let total_input_tokens = 0u64;
    let total_output_tokens = 0u64;
    let mut errors = 0usize;
    let mut duration_ms = 0u64;

    for event in &events {
        match event {
            ReplayEvent::AssistantTurn {
                turn, tool_calls, ..
            } => {
                total_turns = total_turns.max(*turn);
                total_tool_calls += tool_calls;
            }
            ReplayEvent::Result {
                is_error,
                duration_ms: dur,
                ..
            } => {
                if *is_error {
                    errors += 1;
                }
                duration_ms += dur;
            }
            ReplayEvent::ToolResult { is_error, .. } => {
                if *is_error {
                    errors += 1;
                }
                total_tool_calls += 1;
            }
            _ => {}
        }
    }

    Ok(ReplaySummary {
        session_id,
        duration_ms,
        total_turns,
        total_tool_calls,
        total_input_tokens,
        total_output_tokens,
        errors,
        events,
    })
}

#[allow(clippy::too_many_lines)]
fn convert_event(raw: &StoredEvent) -> ReplayEvent {
    if let Some(conv) = &raw.conversation {
        return convert_conversation(conv);
    }

    let payload = raw.payload.clone().unwrap_or(Value::Null);
    match raw.event_type.as_str() {
        "prompt_started" | "prompt" => ReplayEvent::PromptStarted {
            prompt: payload
                .get("prompt")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            provider: payload
                .get("provider")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            model: payload
                .get("model")
                .and_then(Value::as_str)
                .map(String::from),
        },
        "assistant_turn" | "assistant" => ReplayEvent::AssistantTurn {
            turn: {
                #[allow(clippy::cast_possible_truncation)]
                {
                    payload.get("turn").and_then(Value::as_u64).unwrap_or(0) as usize
                }
            },
            text_preview: payload
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .chars()
                .take(200)
                .collect(),
            stop_reason: payload
                .get("stop_reason")
                .and_then(Value::as_str)
                .unwrap_or("end_turn")
                .to_owned(),
            tool_calls: payload
                .get("tool_calls")
                .and_then(|v| v.as_array())
                .map_or(0, Vec::len),
        },
        "tool_result" | "tool" => ReplayEvent::ToolResult {
            tool_name: payload
                .get("tool_name")
                .or_else(|| payload.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            tool_use_id: payload
                .get("tool_use_id")
                .or_else(|| payload.get("tool_call_id"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            is_error: payload
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            content_preview: payload
                .get("content")
                .or_else(|| payload.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .chars()
                .take(200)
                .collect(),
        },
        "hook_run" | "hook" => ReplayEvent::HookRun {
            hook_id: payload
                .get("hook_id")
                .or_else(|| payload.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            event: payload
                .get("event")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            status: payload
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
        },
        "result" => ReplayEvent::Result {
            is_error: payload
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            stop_reason: payload
                .get("stop_reason")
                .and_then(Value::as_str)
                .unwrap_or("end_turn")
                .to_owned(),
            duration_ms: payload
                .get("duration_ms")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        },
        "session_context" => ReplayEvent::SessionContext {
            cwd: payload
                .get("cwd")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            provider: payload
                .get("provider")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
        },
        _ => ReplayEvent::Other {
            event_type: raw.event_type.clone(),
            payload,
        },
    }
}

fn convert_conversation(conv: &claude_core::ConversationEntry) -> ReplayEvent {
    match conv.role {
        ConversationRole::User => ReplayEvent::PromptStarted {
            prompt: conv.text.clone(),
            provider: String::new(),
            model: None,
        },
        ConversationRole::Assistant => ReplayEvent::AssistantTurn {
            turn: 0,
            text_preview: conv.text.chars().take(200).collect(),
            stop_reason: String::from("end_turn"),
            tool_calls: conv.tool_calls.len(),
        },
        ConversationRole::Tool => ReplayEvent::ToolResult {
            tool_name: conv.name.clone().unwrap_or_default(),
            tool_use_id: conv.tool_call_id.clone().unwrap_or_default(),
            is_error: conv.is_error,
            content_preview: conv.text.chars().take(200).collect(),
        },
        ConversationRole::System => ReplayEvent::Other {
            event_type: "system_message".to_owned(),
            payload: Value::String(conv.text.clone()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use claude_config::AppPaths;
    use claude_core::ConversationEntry;
    use serde_json::json;
    use tempfile::tempdir;

    fn setup_store() -> (SessionStore, tempfile::TempDir) {
        let dir = tempdir().expect("tempdir should succeed");
        let paths = AppPaths::discover(Some(dir.path().join(".remote-code-rust")))
            .expect("paths should discover");
        let store = SessionStore::open(paths).expect("store should open");
        (store, dir)
    }

    #[test]
    fn replay_loads_empty_session() {
        let (store, _dir) = setup_store();
        let session_id = Uuid::new_v4();
        store
            .ensure_session(session_id, std::path::Path::new("/tmp"), "mock", None, None)
            .expect("ensure should work");

        let replay = SessionReplay::load(session_id, &store).expect("load should work");
        assert!(replay.events().is_empty());

        let verification = replay.verify();
        assert!(verification.ok);
        assert_eq!(verification.total_events, 0);
    }

    #[test]
    fn replay_verifies_prompt_result_pairing() {
        let (store, _dir) = setup_store();
        let session_id = Uuid::new_v4();
        store
            .ensure_session(session_id, std::path::Path::new("/tmp"), "mock", None, None)
            .expect("ensure should work");

        store
            .append_named_event(
                session_id,
                "prompt_started",
                json!({"prompt": "hello", "provider": "mock"}),
            )
            .expect("append should work");
        store
            .append_named_event(
                session_id,
                "result",
                json!({"is_error": false, "stop_reason": "end_turn", "duration_ms": 100}),
            )
            .expect("append should work");

        let replay = SessionReplay::load(session_id, &store).expect("load should work");
        let verification = replay.verify();
        assert!(verification.ok);
        assert_eq!(verification.prompt_count, 1);
    }

    #[test]
    fn replay_detects_mismatched_prompt_result() {
        let (store, _dir) = setup_store();
        let session_id = Uuid::new_v4();
        store
            .ensure_session(session_id, std::path::Path::new("/tmp"), "mock", None, None)
            .expect("ensure should work");

        store
            .append_named_event(
                session_id,
                "prompt_started",
                json!({"prompt": "hello", "provider": "mock"}),
            )
            .expect("append should work");
        store
            .append_named_event(
                session_id,
                "prompt_started",
                json!({"prompt": "world", "provider": "mock"}),
            )
            .expect("append should work");
        store
            .append_named_event(
                session_id,
                "result",
                json!({"is_error": false, "stop_reason": "end_turn", "duration_ms": 100}),
            )
            .expect("append should work");

        let replay = SessionReplay::load(session_id, &store).expect("load should work");
        let verification = replay.verify();
        assert!(!verification.ok);
        assert!(verification.issues.iter().any(|i| i.contains("mismatch")));
    }

    #[test]
    fn replay_session_returns_summary() {
        let (store, _dir) = setup_store();
        let session_id = Uuid::new_v4();
        store
            .ensure_session(session_id, std::path::Path::new("/tmp"), "mock", None, None)
            .expect("ensure should work");

        store
            .append_conversation_entry(session_id, &ConversationEntry::user("test prompt"))
            .expect("append should work");
        store
            .append_named_event(
                session_id,
                "result",
                json!({"is_error": false, "stop_reason": "end_turn", "duration_ms": 250}),
            )
            .expect("append should work");

        let summary = replay_session(session_id, &store).expect("replay_session should work");
        assert_eq!(summary.session_id, session_id);
        assert_eq!(summary.duration_ms, 250);
        assert_eq!(summary.errors, 0);
        assert!(!summary.events.is_empty());
    }

    #[test]
    fn replay_handles_conversation_events() {
        let (store, _dir) = setup_store();
        let session_id = Uuid::new_v4();
        store
            .ensure_session(session_id, std::path::Path::new("/tmp"), "mock", None, None)
            .expect("ensure should work");

        store
            .append_conversation_entry(session_id, &ConversationEntry::user("hello"))
            .expect("append should work");
        store
            .append_conversation_entry(session_id, &ConversationEntry::assistant("world"))
            .expect("append should work");
        store
            .append_conversation_entry(
                session_id,
                &ConversationEntry::tool("call-1", "bash", "output", false),
            )
            .expect("append should work");

        let replay = SessionReplay::load(session_id, &store).expect("load should work");
        assert_eq!(replay.events().len(), 3);

        let tool_results: Vec<_> = replay
            .events()
            .iter()
            .filter_map(|e| match e {
                ReplayEvent::ToolResult {
                    tool_name,
                    tool_use_id,
                    ..
                } => Some((tool_name.clone(), tool_use_id.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(tool_results.len(), 1);
        assert_eq!(tool_results[0].0, "bash");
        assert_eq!(tool_results[0].1, "call-1");
    }

    #[test]
    fn replay_handles_unknown_event_types() {
        let (store, _dir) = setup_store();
        let session_id = Uuid::new_v4();
        store
            .ensure_session(session_id, std::path::Path::new("/tmp"), "mock", None, None)
            .expect("ensure should work");

        store
            .append_named_event(session_id, "custom_event", json!({"foo": "bar"}))
            .expect("append should work");

        let replay = SessionReplay::load(session_id, &store).expect("load should work");
        assert_eq!(replay.events().len(), 1);
        assert!(matches!(
            &replay.events()[0],
            ReplayEvent::Other { event_type, .. } if event_type == "custom_event"
        ));
    }
}
