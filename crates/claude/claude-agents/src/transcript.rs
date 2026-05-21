//! Subagent transcript persistence.
//!
//! When a subagent finishes (successfully or with failure), its conversation
//! transcript is saved to disk so it can be reviewed later. Transcripts are
//! stored as JSON files at `{session_dir}/transcripts/{agent_id}.json`.
//!
//! This module provides:
//! - [`SubagentTranscript`] — Full transcript with metadata and conversation messages
//! - [`TranscriptMessage`] — A single message in the transcript
//! - [`save_transcript`] — Write a transcript to disk
//! - [`load_transcript`] — Read a transcript from disk
//! - [`list_transcripts`] — List all transcripts in a session directory
//! - [`get_transcript_path`] — Get the file path for a given agent ID
//! - [`persist_transcript`] — Convenience: build and save from a `WorkerResult`

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::runner::UsageSummary;
use crate::worker::{WorkerResult, WorkerStatus};

/// Subdirectory within the session directory for transcript files.
const TRANSCRIPT_DIR: &str = "transcripts";

/// A single message within a subagent transcript.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TranscriptMessage {
    /// Role of the message sender (e.g. "user", "assistant", "system").
    pub role: String,
    /// Text content of the message.
    pub content: String,
}

impl TranscriptMessage {
    /// Create a new transcript message.
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
        }
    }

    /// Create a user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self::new("user", content)
    }

    /// Create an assistant message.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::new("assistant", content)
    }

    /// Create a system message.
    pub fn system(content: impl Into<String>) -> Self {
        Self::new("system", content)
    }
}

/// Full transcript of a completed subagent conversation.
///
/// Contains the conversation messages along with metadata about the agent's
/// execution, including status, usage, and timing information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentTranscript {
    /// The agent ID.
    pub agent_id: String,
    /// Human-readable description of the task.
    pub description: String,
    /// Final status of the agent.
    pub status: WorkerStatus,
    /// The conversation messages.
    pub messages: Vec<TranscriptMessage>,
    /// Token usage summary.
    pub usage: UsageSummary,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Number of tool uses.
    pub tool_use_count: u32,
    /// When the transcript was persisted.
    pub saved_at: DateTime<Utc>,
}

impl SubagentTranscript {
    /// Create a new transcript for the given agent.
    pub fn new(agent_id: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            description: String::new(),
            status: WorkerStatus::Idle,
            messages: Vec::new(),
            usage: UsageSummary::default(),
            duration_ms: 0,
            tool_use_count: 0,
            saved_at: Utc::now(),
        }
    }

    /// Add a message to the transcript.
    pub fn add_message(&mut self, role: impl Into<String>, content: impl Into<String>) {
        self.messages.push(TranscriptMessage::new(role, content));
    }

    /// Get the total number of messages.
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }
}

/// Get the directory where transcripts are stored for a session.
///
/// Returns `{session_dir}/transcripts/`.
pub fn get_transcript_dir(session_dir: &Path) -> PathBuf {
    session_dir.join(TRANSCRIPT_DIR)
}

/// Get the file path for a specific agent's transcript.
///
/// Returns `{session_dir}/transcripts/{agent_id}.json`.
pub fn get_transcript_path(session_dir: &Path, agent_id: &str) -> PathBuf {
    get_transcript_dir(session_dir).join(format!("{agent_id}.json"))
}

/// Save a transcript to disk.
///
/// Creates the transcript directory if it doesn't exist. Overwrites any
/// existing transcript for the same agent ID.
pub fn save_transcript(session_dir: &Path, transcript: &SubagentTranscript) -> Result<()> {
    let dir = get_transcript_dir(session_dir);
    fs::create_dir_all(&dir)?;

    let path = get_transcript_path(session_dir, &transcript.agent_id);
    let json = serde_json::to_string_pretty(transcript)?;
    fs::write(path, json)?;

    tracing::debug!(
        agent_id = %transcript.agent_id,
        status = %transcript.status,
        messages = transcript.messages.len(),
        "Persisted subagent transcript"
    );

    Ok(())
}

/// Load a transcript from disk.
///
/// Returns `Ok(None)` if no transcript exists for the given agent ID.
pub fn load_transcript(session_dir: &Path, agent_id: &str) -> Result<Option<SubagentTranscript>> {
    let path = get_transcript_path(session_dir, agent_id);
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(&path)?;
    let transcript: SubagentTranscript = serde_json::from_str(&content)?;
    Ok(Some(transcript))
}

/// Delete a transcript from disk.
///
/// Returns `Ok(true)` if the transcript was deleted, `Ok(false)` if it
/// didn't exist.
pub fn delete_transcript(session_dir: &Path, agent_id: &str) -> Result<bool> {
    let path = get_transcript_path(session_dir, agent_id);
    if !path.exists() {
        return Ok(false);
    }

    fs::remove_file(&path)?;
    Ok(true)
}

/// List all agent IDs that have transcripts in the given session directory.
///
/// Returns agent IDs sorted alphabetically.
pub fn list_transcripts(session_dir: &Path) -> Result<Vec<String>> {
    let transcript_dir = get_transcript_dir(session_dir);
    if !transcript_dir.exists() {
        return Ok(Vec::new());
    }

    let mut agent_ids = Vec::new();
    for entry in fs::read_dir(&transcript_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name_str = match name.to_str() {
            Some(s) => s,
            None => continue,
        };
        if let Some(agent_id) = name_str.strip_suffix(".json") {
            agent_ids.push(agent_id.to_owned());
        }
    }

    agent_ids.sort();
    Ok(agent_ids)
}

/// Build and save a transcript from a worker result and conversation messages.
///
/// This is the primary convenience function for persisting a subagent's
/// transcript when it finishes execution. The caller supplies the
/// [`WorkerResult`] (which contains status, usage, and timing) and the
/// full conversation as a slice of [`TranscriptMessage`].
///
/// The transcript is written to `{session_dir}/transcripts/{agent_id}.json`.
pub fn persist_transcript(
    session_dir: &Path,
    result: &WorkerResult,
    messages: &[TranscriptMessage],
) -> Result<()> {
    let transcript = SubagentTranscript {
        agent_id: result.agent_id.clone(),
        description: result.description.clone(),
        status: result.status,
        messages: messages.to_owned(),
        usage: result.usage.clone(),
        duration_ms: result.duration_ms,
        tool_use_count: result.tool_use_count,
        saved_at: Utc::now(),
    };
    save_transcript(session_dir, &transcript)
}

/// Persist a transcript from a worker result when no separate conversation
/// messages are available.
///
/// Constructs a minimal transcript from the worker result's output field.
pub fn persist_transcript_from_result(session_dir: &Path, result: &WorkerResult) -> Result<()> {
    let mut messages = Vec::new();

    // Include the task description as a user message if available.
    if !result.description.is_empty() {
        messages.push(TranscriptMessage::user(&result.description));
    }

    // Include the final output as an assistant message if available.
    if let Some(output) = &result.output
        && !output.is_empty()
    {
        messages.push(TranscriptMessage::assistant(output));
    }

    // Add a status summary as a system message.
    let status_msg = match result.status {
        WorkerStatus::Completed => format!(
            "Agent completed after {}ms ({} tool uses)",
            result.duration_ms, result.tool_use_count
        ),
        WorkerStatus::Failed => format!(
            "Agent failed after {}ms ({} tool uses)",
            result.duration_ms, result.tool_use_count
        ),
        WorkerStatus::Killed => "Agent was killed".to_owned(),
        _ => format!("Agent in state {}", result.status),
    };
    messages.push(TranscriptMessage::system(&status_msg));

    persist_transcript(session_dir, result, &messages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::UsageSummary;

    fn test_result(status: WorkerStatus, output: Option<&str>) -> WorkerResult {
        WorkerResult {
            agent_id: "worker-test-123".to_owned(),
            description: "Test task description".to_owned(),
            status,
            output: output.map(|s| s.to_owned()),
            usage: UsageSummary {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
            duration_ms: 5000,
            tool_use_count: 3,
        }
    }

    #[test]
    fn transcript_message_constructors() {
        let user = TranscriptMessage::user("hello");
        assert_eq!(user.role, "user");
        assert_eq!(user.content, "hello");

        let assistant = TranscriptMessage::assistant("response");
        assert_eq!(assistant.role, "assistant");

        let system = TranscriptMessage::system("instruction");
        assert_eq!(system.role, "system");
    }

    #[test]
    fn transcript_message_new() {
        let msg = TranscriptMessage::new("custom", "content");
        assert_eq!(msg.role, "custom");
        assert_eq!(msg.content, "content");
    }

    #[test]
    fn subagent_transcript_new() {
        let t = SubagentTranscript::new("agent-1");
        assert_eq!(t.agent_id, "agent-1");
        assert!(t.messages.is_empty());
        assert_eq!(t.status, WorkerStatus::Idle);
    }

    #[test]
    fn subagent_transcript_add_message() {
        let mut t = SubagentTranscript::new("agent-1");
        t.add_message("user", "Hello");
        t.add_message("assistant", "Hi there");
        assert_eq!(t.message_count(), 2);
    }

    #[test]
    fn get_transcript_dir_path() {
        let session_dir = PathBuf::from("/tmp/session");
        let dir = get_transcript_dir(&session_dir);
        assert_eq!(dir, PathBuf::from("/tmp/session/transcripts"));
    }

    #[test]
    fn get_transcript_path_format() {
        let session_dir = PathBuf::from("/tmp/session");
        let path = get_transcript_path(&session_dir, "worker-abc");
        assert_eq!(
            path,
            PathBuf::from("/tmp/session/transcripts/worker-abc.json")
        );
    }

    #[test]
    fn save_and_load_transcript() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut transcript = SubagentTranscript::new("test-agent-1");
        transcript.description = "Test task".to_owned();
        transcript.status = WorkerStatus::Completed;
        transcript.add_message("user", "Do the thing");
        transcript.add_message("assistant", "Done");
        transcript.usage.input_tokens = 100;
        transcript.usage.output_tokens = 50;

        save_transcript(dir.path(), &transcript).expect("save");

        let loaded = load_transcript(dir.path(), "test-agent-1")
            .expect("load")
            .expect("some");

        assert_eq!(loaded.agent_id, "test-agent-1");
        assert_eq!(loaded.description, "Test task");
        assert_eq!(loaded.status, WorkerStatus::Completed);
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.messages[0].role, "user");
        assert_eq!(loaded.messages[0].content, "Do the thing");
        assert_eq!(loaded.messages[1].role, "assistant");
        assert_eq!(loaded.messages[1].content, "Done");
        assert_eq!(loaded.usage.input_tokens, 100);
        assert_eq!(loaded.usage.output_tokens, 50);
    }

    #[test]
    fn load_nonexistent_returns_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = load_transcript(dir.path(), "nonexistent").expect("load");
        assert!(result.is_none());
    }

    #[test]
    fn delete_transcript_removes_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let transcript = SubagentTranscript::new("del-agent");
        save_transcript(dir.path(), &transcript).expect("save");

        let deleted = delete_transcript(dir.path(), "del-agent").expect("delete");
        assert!(deleted);

        let loaded = load_transcript(dir.path(), "del-agent").expect("load");
        assert!(loaded.is_none());
    }

    #[test]
    fn delete_nonexistent_returns_false() {
        let dir = tempfile::tempdir().expect("tempdir");
        let deleted = delete_transcript(dir.path(), "nonexistent").expect("delete");
        assert!(!deleted);
    }

    #[test]
    fn list_transcripts_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let list = list_transcripts(dir.path()).expect("list");
        assert!(list.is_empty());
    }

    #[test]
    fn list_transcripts_returns_ids() {
        let dir = tempfile::tempdir().expect("tempdir");
        let t1 = SubagentTranscript::new("agent-alpha");
        let t2 = SubagentTranscript::new("agent-beta");
        save_transcript(dir.path(), &t1).expect("save");
        save_transcript(dir.path(), &t2).expect("save");

        let list = list_transcripts(dir.path()).expect("list");
        assert_eq!(list, vec!["agent-alpha", "agent-beta"]);
    }

    #[test]
    fn list_transcripts_ignores_non_json() {
        let dir = tempfile::tempdir().expect("tempdir");
        let transcript_dir = get_transcript_dir(dir.path());
        fs::create_dir_all(&transcript_dir).expect("dir");
        fs::write(transcript_dir.join("readme.txt"), "not a transcript").expect("write");

        let list = list_transcripts(dir.path()).expect("list");
        assert!(list.is_empty());
    }

    #[test]
    fn persist_transcript_saves_full_conversation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = test_result(WorkerStatus::Completed, Some("Task done"));
        let messages = vec![
            TranscriptMessage::user("Please fix the bug"),
            TranscriptMessage::assistant("I found the bug in auth.rs"),
            TranscriptMessage::assistant("Fixed and tested"),
        ];

        persist_transcript(dir.path(), &result, &messages).expect("persist");

        let loaded = load_transcript(dir.path(), "worker-test-123")
            .expect("load")
            .expect("some");

        assert_eq!(loaded.agent_id, "worker-test-123");
        assert_eq!(loaded.status, WorkerStatus::Completed);
        assert_eq!(loaded.messages.len(), 3);
        assert_eq!(loaded.messages[0].content, "Please fix the bug");
        assert_eq!(loaded.messages[2].content, "Fixed and tested");
        assert_eq!(loaded.usage.input_tokens, 100);
        assert_eq!(loaded.duration_ms, 5000);
        assert_eq!(loaded.tool_use_count, 3);
    }

    #[test]
    fn persist_transcript_from_result_completed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = test_result(WorkerStatus::Completed, Some("Done output"));

        persist_transcript_from_result(dir.path(), &result).expect("persist");

        let loaded = load_transcript(dir.path(), "worker-test-123")
            .expect("load")
            .expect("some");

        assert_eq!(loaded.status, WorkerStatus::Completed);
        // Should have: description (user), output (assistant), status (system)
        assert_eq!(loaded.messages.len(), 3);
        assert_eq!(loaded.messages[0].role, "user");
        assert_eq!(loaded.messages[0].content, "Test task description");
        assert_eq!(loaded.messages[1].role, "assistant");
        assert_eq!(loaded.messages[1].content, "Done output");
        assert_eq!(loaded.messages[2].role, "system");
        assert!(loaded.messages[2].content.contains("completed"));
    }

    #[test]
    fn persist_transcript_from_result_failed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = test_result(WorkerStatus::Failed, Some("Error: file not found"));

        persist_transcript_from_result(dir.path(), &result).expect("persist");

        let loaded = load_transcript(dir.path(), "worker-test-123")
            .expect("load")
            .expect("some");

        assert_eq!(loaded.status, WorkerStatus::Failed);
        assert_eq!(loaded.messages[1].role, "assistant");
        assert_eq!(loaded.messages[1].content, "Error: file not found");
        assert!(loaded.messages[2].content.contains("failed"));
    }

    #[test]
    fn persist_transcript_from_result_killed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = test_result(WorkerStatus::Killed, None);

        persist_transcript_from_result(dir.path(), &result).expect("persist");

        let loaded = load_transcript(dir.path(), "worker-test-123")
            .expect("load")
            .expect("some");

        assert_eq!(loaded.status, WorkerStatus::Killed);
        // Should have: description (user), status (system) -- no output
        assert_eq!(loaded.messages.len(), 2);
        assert!(loaded.messages[1].content.contains("killed"));
    }

    #[test]
    fn persist_transcript_creates_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_dir = dir.path().join("nested").join("session");
        // session_dir doesn't exist yet
        assert!(!session_dir.exists());

        let result = test_result(WorkerStatus::Completed, Some("Done"));
        persist_transcript_from_result(&session_dir, &result).expect("persist");

        // Directory and file should now exist
        assert!(session_dir.join("transcripts").exists());
        assert!(
            session_dir
                .join("transcripts")
                .join("worker-test-123.json")
                .exists()
        );
    }

    #[test]
    fn save_transcript_overwrites_existing() {
        let dir = tempfile::tempdir().expect("tempdir");

        let mut t1 = SubagentTranscript::new("agent-1");
        t1.add_message("user", "First run");
        t1.status = WorkerStatus::Failed;
        save_transcript(dir.path(), &t1).expect("save");

        let mut t2 = SubagentTranscript::new("agent-1");
        t2.add_message("user", "Second run");
        t2.add_message("assistant", "Success");
        t2.status = WorkerStatus::Completed;
        save_transcript(dir.path(), &t2).expect("save");

        let loaded = load_transcript(dir.path(), "agent-1")
            .expect("load")
            .expect("some");
        assert_eq!(loaded.status, WorkerStatus::Completed);
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.messages[0].content, "Second run");
    }

    #[test]
    fn transcript_serde_roundtrip() {
        let mut transcript = SubagentTranscript::new("serde-agent");
        transcript.description = "Serde test".to_owned();
        transcript.status = WorkerStatus::Completed;
        transcript.add_message("user", "Test prompt");
        transcript.add_message("assistant", "Test response");
        transcript.usage.input_tokens = 200;
        transcript.usage.output_tokens = 100;
        transcript.duration_ms = 3000;
        transcript.tool_use_count = 5;

        let json = serde_json::to_string_pretty(&transcript).expect("serialize");
        let parsed: SubagentTranscript = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(parsed.agent_id, "serde-agent");
        assert_eq!(parsed.description, "Serde test");
        assert_eq!(parsed.status, WorkerStatus::Completed);
        assert_eq!(parsed.messages.len(), 2);
        assert_eq!(parsed.usage.input_tokens, 200);
        assert_eq!(parsed.duration_ms, 3000);
        assert_eq!(parsed.tool_use_count, 5);
    }
}
