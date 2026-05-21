//! Resume Agent matching Claude Code's `AgentTool/resumeAgent.ts`.
//!
//! Provides checkpoint-based agent resume capability. Agents can save their
//! conversation state, turn count, and usage to a checkpoint file, then be
//! restored from that checkpoint in a later session.
//!
//! This module provides:
//! - [`ResumableAgentState`] — Resumable agent state
//! - [`AgentCheckpoint`] — Agent checkpoint with full conversation
//! - [`save_agent_checkpoint`] — Save a checkpoint to disk
//! - [`load_agent_checkpoint`] — Load a checkpoint from disk
//! - [`resume_agent_from_checkpoint`] — Restore agent from checkpoint
//! - [`CheckpointDiff`] — Diff between two checkpoints

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::runner::UsageSummary;

/// File name for the agent checkpoint.
const CHECKPOINT_FILE: &str = "checkpoint.json";

/// Subdirectory within the agent directory for checkpoints.
const CHECKPOINT_DIR: &str = "checkpoints";

/// The state of a resumable agent.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResumableAgentState {
    /// Agent has not been started yet.
    #[default]
    NotStarted,
    /// Agent is actively running.
    Running,
    /// Agent was paused and can be resumed.
    Paused,
    /// Agent completed successfully.
    Completed,
    /// Agent failed.
    Failed,
}

impl std::fmt::Display for ResumableAgentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotStarted => write!(f, "not_started"),
            Self::Running => write!(f, "running"),
            Self::Paused => write!(f, "paused"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

/// A simplified conversation message for checkpoint storage.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckpointMessage {
    /// Role of the message sender (user/assistant/system).
    pub role: String,
    /// Text content of the message.
    pub content: String,
}

impl CheckpointMessage {
    /// Create a new checkpoint message.
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
}

/// An agent checkpoint containing all state needed for resume.
///
/// Captures the conversation history, turn count, usage statistics,
/// agent metadata, and timing information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCheckpoint {
    /// The agent ID this checkpoint belongs to.
    pub agent_id: String,
    /// The agent type (e.g. "worker", "general-purpose").
    pub agent_type: String,
    /// Human-readable description of the agent's task.
    pub description: String,
    /// Current state of the agent.
    pub state: ResumableAgentState,
    /// Conversation messages at checkpoint time.
    pub messages: Vec<CheckpointMessage>,
    /// Number of turns completed.
    pub turn_count: u32,
    /// Token usage at checkpoint time.
    pub usage: UsageSummary,
    /// When the checkpoint was created.
    pub created_at: DateTime<Utc>,
    /// When the agent was originally started.
    pub started_at: Option<DateTime<Utc>>,
    /// Optional working directory for the agent.
    pub working_dir: Option<String>,
    /// Optional worktree path (for isolated agents).
    pub worktree_path: Option<String>,
    /// The prompt that was used to start/resume the agent.
    pub prompt: String,
}

impl AgentCheckpoint {
    /// Create a new checkpoint for the given agent.
    pub fn new(agent_id: impl Into<String>, agent_type: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            agent_type: agent_type.into(),
            description: String::new(),
            state: ResumableAgentState::Paused,
            messages: Vec::new(),
            turn_count: 0,
            usage: UsageSummary::default(),
            created_at: Utc::now(),
            started_at: None,
            working_dir: None,
            worktree_path: None,
            prompt: String::new(),
        }
    }

    /// Add a message to the checkpoint.
    pub fn add_message(&mut self, role: impl Into<String>, content: impl Into<String>) {
        self.messages.push(CheckpointMessage::new(role, content));
    }

    /// Get the total number of messages.
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Get the total tokens used (input + output).
    pub fn total_tokens(&self) -> u64 {
        self.usage.input_tokens + self.usage.output_tokens
    }

    /// Check if the checkpoint can be resumed.
    pub fn can_resume(&self) -> bool {
        matches!(
            self.state,
            ResumableAgentState::Paused | ResumableAgentState::Failed
        )
    }
}

/// Diff between two checkpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointDiff {
    /// Number of new messages added.
    pub new_messages: usize,
    /// Difference in turn count.
    pub turn_delta: i64,
    /// Difference in input tokens.
    pub input_token_delta: i64,
    /// Difference in output tokens.
    pub output_token_delta: i64,
    /// Whether the state changed.
    pub state_changed: bool,
}

/// Get the checkpoint directory for an agent.
///
/// Returns `<base>/checkpoints/<agent_id>/`.
pub fn get_checkpoint_dir(base: &Path, agent_id: &str) -> PathBuf {
    base.join(CHECKPOINT_DIR).join(agent_id)
}

/// Get the checkpoint file path.
pub fn get_checkpoint_path(base: &Path, agent_id: &str) -> PathBuf {
    get_checkpoint_dir(base, agent_id).join(CHECKPOINT_FILE)
}

/// Save an agent checkpoint to disk.
///
/// Creates the checkpoint directory if it doesn't exist.
pub fn save_agent_checkpoint(base: &Path, checkpoint: &AgentCheckpoint) -> Result<()> {
    let dir = get_checkpoint_dir(base, &checkpoint.agent_id);
    fs::create_dir_all(&dir)?;

    let path = dir.join(CHECKPOINT_FILE);
    let json = serde_json::to_string_pretty(checkpoint)?;
    fs::write(path, json)?;

    Ok(())
}

/// Load an agent checkpoint from disk.
///
/// Returns `Ok(None)` if no checkpoint exists for the given agent ID.
pub fn load_agent_checkpoint(base: &Path, agent_id: &str) -> Result<Option<AgentCheckpoint>> {
    let path = get_checkpoint_path(base, agent_id);
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(&path)?;
    let checkpoint: AgentCheckpoint = serde_json::from_str(&content)?;
    Ok(Some(checkpoint))
}

/// Delete an agent checkpoint from disk.
///
/// Returns `Ok(true)` if the checkpoint was deleted, `Ok(false)` if it
/// didn't exist.
pub fn delete_agent_checkpoint(base: &Path, agent_id: &str) -> Result<bool> {
    let path = get_checkpoint_path(base, agent_id);
    if !path.exists() {
        return Ok(false);
    }

    fs::remove_file(&path)?;

    // Try to clean up empty directory
    let dir = get_checkpoint_dir(base, agent_id);
    let _ = fs::remove_dir(&dir);

    Ok(true)
}

/// List all agent IDs that have checkpoints in the given base directory.
pub fn list_checkpoints(base: &Path) -> Result<Vec<String>> {
    let checkpoint_dir = base.join(CHECKPOINT_DIR);
    if !checkpoint_dir.exists() {
        return Ok(Vec::new());
    }

    let mut agent_ids = Vec::new();
    for entry in fs::read_dir(&checkpoint_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let checkpoint_file = entry.path().join(CHECKPOINT_FILE);
            if checkpoint_file.exists()
                && let Some(name) = entry.file_name().to_str()
            {
                agent_ids.push(name.to_owned());
            }
        }
    }

    agent_ids.sort();
    Ok(agent_ids)
}

/// Resume an agent from a checkpoint.
///
/// Loads the checkpoint, validates it can be resumed, and returns the
/// checkpoint with updated state ready for execution.
pub fn resume_agent_from_checkpoint(
    base: &Path,
    agent_id: &str,
    resume_prompt: &str,
) -> Result<AgentCheckpoint> {
    let Some(mut checkpoint) = load_agent_checkpoint(base, agent_id)? else {
        anyhow::bail!("No checkpoint found for agent ID: {agent_id}");
    };

    if !checkpoint.can_resume() {
        anyhow::bail!(
            "Agent {} is in state {} and cannot be resumed",
            agent_id,
            checkpoint.state
        );
    }

    // Add the resume prompt as a new user message
    checkpoint
        .messages
        .push(CheckpointMessage::user(resume_prompt));
    checkpoint.prompt = resume_prompt.to_owned();
    checkpoint.state = ResumableAgentState::Running;
    checkpoint.created_at = Utc::now();

    // Save updated checkpoint
    save_agent_checkpoint(base, &checkpoint)?;

    Ok(checkpoint)
}

/// Compute the diff between two checkpoints.
pub fn diff_checkpoints(old: &AgentCheckpoint, new: &AgentCheckpoint) -> CheckpointDiff {
    CheckpointDiff {
        new_messages: new.messages.len().saturating_sub(old.messages.len()),
        turn_delta: new.turn_count as i64 - old.turn_count as i64,
        input_token_delta: new.usage.input_tokens as i64 - old.usage.input_tokens as i64,
        output_token_delta: new.usage.output_tokens as i64 - old.usage.output_tokens as i64,
        state_changed: old.state != new.state,
    }
}

/// Filter messages suitable for resume.
///
/// Removes whitespace-only assistant messages and unresolved tool uses
/// that would cause issues when resuming.
pub fn filter_messages_for_resume(messages: &[CheckpointMessage]) -> Vec<CheckpointMessage> {
    messages
        .iter()
        .filter(|msg| {
            // Keep all non-assistant messages
            if msg.role != "assistant" {
                return true;
            }
            // Filter out whitespace-only assistant messages
            !msg.content.trim().is_empty()
        })
        .cloned()
        .collect()
}

/// Build the resume context from a checkpoint.
///
/// Returns the conversation messages formatted for injection into a new
/// agent run, plus the resume prompt.
pub fn build_resume_context(checkpoint: &AgentCheckpoint) -> Vec<CheckpointMessage> {
    let filtered = filter_messages_for_resume(&checkpoint.messages);
    let mut context = filtered;

    // Add a system message indicating this is a resumed session
    if !context.is_empty() {
        context.insert(
            0,
            CheckpointMessage::new(
                "system",
                format!(
                    "Resuming agent '{}' (type: {}, {} turns completed, {} tokens used)",
                    checkpoint.agent_id,
                    checkpoint.agent_type,
                    checkpoint.turn_count,
                    checkpoint.total_tokens()
                ),
            ),
        );
    }

    context
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resumable_agent_state_default() {
        assert_eq!(
            ResumableAgentState::default(),
            ResumableAgentState::NotStarted
        );
    }

    #[test]
    fn resumable_agent_state_display() {
        assert_eq!(ResumableAgentState::NotStarted.to_string(), "not_started");
        assert_eq!(ResumableAgentState::Running.to_string(), "running");
        assert_eq!(ResumableAgentState::Paused.to_string(), "paused");
        assert_eq!(ResumableAgentState::Completed.to_string(), "completed");
        assert_eq!(ResumableAgentState::Failed.to_string(), "failed");
    }

    #[test]
    fn resumable_agent_state_serde() {
        let state = ResumableAgentState::Paused;
        let json = serde_json::to_string(&state).expect("serialize");
        let parsed: ResumableAgentState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, state);
    }

    #[test]
    fn checkpoint_message_new() {
        let msg = CheckpointMessage::new("user", "Hello");
        assert_eq!(msg.role, "user");
        assert_eq!(msg.content, "Hello");
    }

    #[test]
    fn checkpoint_message_user() {
        let msg = CheckpointMessage::user("Test prompt");
        assert_eq!(msg.role, "user");
        assert_eq!(msg.content, "Test prompt");
    }

    #[test]
    fn checkpoint_message_assistant() {
        let msg = CheckpointMessage::assistant("Response");
        assert_eq!(msg.role, "assistant");
        assert_eq!(msg.content, "Response");
    }

    #[test]
    fn checkpoint_message_equality() {
        let m1 = CheckpointMessage::user("test");
        let m2 = CheckpointMessage::user("test");
        assert_eq!(m1, m2);
    }

    #[test]
    fn agent_checkpoint_new() {
        let cp = AgentCheckpoint::new("agent-1", "worker");
        assert_eq!(cp.agent_id, "agent-1");
        assert_eq!(cp.agent_type, "worker");
        assert_eq!(cp.state, ResumableAgentState::Paused);
        assert!(cp.messages.is_empty());
        assert_eq!(cp.turn_count, 0);
    }

    #[test]
    fn agent_checkpoint_add_message() {
        let mut cp = AgentCheckpoint::new("agent-1", "worker");
        cp.add_message("user", "Hello");
        cp.add_message("assistant", "Hi there");
        assert_eq!(cp.message_count(), 2);
    }

    #[test]
    fn agent_checkpoint_total_tokens() {
        let mut cp = AgentCheckpoint::new("agent-1", "worker");
        cp.usage.input_tokens = 100;
        cp.usage.output_tokens = 50;
        assert_eq!(cp.total_tokens(), 150);
    }

    #[test]
    fn agent_checkpoint_can_resume_paused() {
        let cp = AgentCheckpoint::new("agent-1", "worker");
        assert!(cp.can_resume());
    }

    #[test]
    fn agent_checkpoint_can_resume_failed() {
        let mut cp = AgentCheckpoint::new("agent-1", "worker");
        cp.state = ResumableAgentState::Failed;
        assert!(cp.can_resume());
    }

    #[test]
    fn agent_checkpoint_cannot_resume_running() {
        let mut cp = AgentCheckpoint::new("agent-1", "worker");
        cp.state = ResumableAgentState::Running;
        assert!(!cp.can_resume());
    }

    #[test]
    fn agent_checkpoint_cannot_resume_completed() {
        let mut cp = AgentCheckpoint::new("agent-1", "worker");
        cp.state = ResumableAgentState::Completed;
        assert!(!cp.can_resume());
    }

    #[test]
    fn save_and_load_checkpoint() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut cp = AgentCheckpoint::new("test-agent", "worker");
        cp.add_message("user", "Hello");
        cp.add_message("assistant", "Hi");
        cp.turn_count = 1;
        cp.usage.input_tokens = 50;
        cp.usage.output_tokens = 25;

        save_agent_checkpoint(dir.path(), &cp).expect("save");
        let loaded = load_agent_checkpoint(dir.path(), "test-agent")
            .expect("load")
            .expect("some");

        assert_eq!(loaded.agent_id, "test-agent");
        assert_eq!(loaded.agent_type, "worker");
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.turn_count, 1);
        assert_eq!(loaded.usage.input_tokens, 50);
    }

    #[test]
    fn load_nonexistent_returns_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = load_agent_checkpoint(dir.path(), "nonexistent").expect("load");
        assert!(result.is_none());
    }

    #[test]
    fn delete_checkpoint() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cp = AgentCheckpoint::new("del-agent", "worker");
        save_agent_checkpoint(dir.path(), &cp).expect("save");

        let deleted = delete_agent_checkpoint(dir.path(), "del-agent").expect("delete");
        assert!(deleted);

        let loaded = load_agent_checkpoint(dir.path(), "del-agent").expect("load");
        assert!(loaded.is_none());
    }

    #[test]
    fn delete_nonexistent_returns_false() {
        let dir = tempfile::tempdir().expect("tempdir");
        let deleted = delete_agent_checkpoint(dir.path(), "nonexistent").expect("delete");
        assert!(!deleted);
    }

    #[test]
    fn list_checkpoints_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let list = list_checkpoints(dir.path()).expect("list");
        assert!(list.is_empty());
    }

    #[test]
    fn list_checkpoints_returns_ids() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cp1 = AgentCheckpoint::new("agent-a", "worker");
        let cp2 = AgentCheckpoint::new("agent-b", "worker");
        save_agent_checkpoint(dir.path(), &cp1).expect("save");
        save_agent_checkpoint(dir.path(), &cp2).expect("save");

        let list = list_checkpoints(dir.path()).expect("list");
        assert_eq!(list, vec!["agent-a", "agent-b"]);
    }

    #[test]
    fn resume_from_checkpoint() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut cp = AgentCheckpoint::new("resume-agent", "worker");
        cp.add_message("user", "Initial prompt");
        cp.add_message("assistant", "Working on it");
        cp.state = ResumableAgentState::Paused;
        save_agent_checkpoint(dir.path(), &cp).expect("save");

        let resumed = resume_agent_from_checkpoint(dir.path(), "resume-agent", "Continue please")
            .expect("resume");

        assert_eq!(resumed.state, ResumableAgentState::Running);
        assert_eq!(resumed.messages.len(), 3); // 2 original + 1 resume prompt
        assert_eq!(resumed.prompt, "Continue please");
    }

    #[test]
    fn resume_nonexistent_fails() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = resume_agent_from_checkpoint(dir.path(), "nonexistent", "Resume");
        assert!(result.is_err());
    }

    #[test]
    fn resume_completed_fails() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut cp = AgentCheckpoint::new("completed-agent", "worker");
        cp.state = ResumableAgentState::Completed;
        save_agent_checkpoint(dir.path(), &cp).expect("save");

        let result = resume_agent_from_checkpoint(dir.path(), "completed-agent", "Resume");
        assert!(result.is_err());
    }

    #[test]
    fn test_diff_checkpoints_added() {
        let old = AgentCheckpoint::new("agent-1", "worker");
        let mut new = AgentCheckpoint::new("agent-1", "worker");
        new.messages.push(CheckpointMessage::user("test"));
        new.turn_count = 5;
        new.usage.input_tokens = 100;
        new.usage.output_tokens = 50;
        new.state = ResumableAgentState::Running;

        let diff = diff_checkpoints(&old, &new);
        assert_eq!(diff.new_messages, 1);
        assert_eq!(diff.turn_delta, 5);
        assert_eq!(diff.input_token_delta, 100);
        assert_eq!(diff.output_token_delta, 50);
        assert!(diff.state_changed);
    }

    #[test]
    fn test_diff_checkpoints_no_change() {
        let cp = AgentCheckpoint::new("agent-1", "worker");
        let diff = diff_checkpoints(&cp, &cp);
        assert_eq!(diff.new_messages, 0);
        assert_eq!(diff.turn_delta, 0);
        assert!(!diff.state_changed);
    }

    #[test]
    fn filter_messages_removes_empty_assistant() {
        let messages = vec![
            CheckpointMessage::user("Hello"),
            CheckpointMessage::assistant(""),
            CheckpointMessage::assistant("   "),
            CheckpointMessage::assistant("Real response"),
        ];
        let filtered = filter_messages_for_resume(&messages);
        assert_eq!(filtered.len(), 2); // user + real response
    }

    #[test]
    fn filter_messages_keeps_all_user() {
        let messages = vec![
            CheckpointMessage::user(""),
            CheckpointMessage::user("Hello"),
        ];
        let filtered = filter_messages_for_resume(&messages);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn build_resume_context_adds_system_message() {
        let mut cp = AgentCheckpoint::new("agent-1", "worker");
        cp.add_message("user", "Hello");
        cp.turn_count = 3;
        cp.usage.input_tokens = 100;
        cp.usage.output_tokens = 50;

        let context = build_resume_context(&cp);
        assert_eq!(context.len(), 2); // system + user
        assert_eq!(context[0].role, "system");
        assert!(context[0].content.contains("agent-1"));
        assert!(context[0].content.contains("worker"));
    }

    #[test]
    fn build_resume_context_empty_messages() {
        let cp = AgentCheckpoint::new("agent-1", "worker");
        let context = build_resume_context(&cp);
        assert!(context.is_empty());
    }

    #[test]
    fn checkpoint_serde_roundtrip() {
        let mut cp = AgentCheckpoint::new("serde-agent", "worker");
        cp.add_message("user", "test");
        cp.turn_count = 10;
        cp.usage.input_tokens = 500;
        cp.usage.output_tokens = 200;
        cp.working_dir = Some("/tmp/work".to_owned());

        let json = serde_json::to_string_pretty(&cp).expect("serialize");
        let parsed: AgentCheckpoint = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(parsed.agent_id, "serde-agent");
        assert_eq!(parsed.turn_count, 10);
        assert_eq!(parsed.usage.input_tokens, 500);
        assert_eq!(parsed.working_dir, Some("/tmp/work".to_owned()));
    }
}
