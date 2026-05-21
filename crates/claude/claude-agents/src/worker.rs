//! Worker Agent matching Claude Code's `coordinator/workerAgent.ts`.
//!
//! Workers are autonomous agents spawned by a coordinator to execute tasks
//! independently. Each worker has its own lifecycle (idle → running →
//! completed/failed) and reports results back via task notifications.
//!
//! This module provides:
//! - [`WorkerConfig`] — Worker configuration
//! - [`WorkerResult`] — Worker execution result
//! - [`WorkerStatus`] — Worker lifecycle status
//! - [`WorkerAgent`] — Worker agent struct with lifecycle management
//! - [`spawn_worker`] — Create a new worker
//! - [`format_task_notification_xml`] — Format task notification XML
//! - [`parse_worker_tools`] — Parse tools from the allowed set

use std::path::PathBuf;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::coordinator::{
    TaskNotificationParams, TaskNotificationStatus, TaskUsage, async_agent_allowed_tools_set,
    format_task_notification, internal_worker_tools_set,
};
use crate::definition::AgentDefinition;
use crate::runner::{AgentRunConfig, AgentRunResult, UsageSummary};
use crate::transcript::{TranscriptMessage, persist_transcript, persist_transcript_from_result};

/// The worker agent type identifier.
pub const WORKER_AGENT: &str = "worker";

/// Default maximum turns for a worker.
const DEFAULT_WORKER_MAX_TURNS: u32 = 200;

/// Configuration for spawning a worker agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerConfig {
    /// Human-readable description of the worker's task.
    pub description: String,
    /// The prompt/task to send to the worker.
    pub prompt: String,
    /// Optional model override for the worker.
    pub model: Option<String>,
    /// Maximum number of turns.
    pub max_turns: u32,
    /// Whether to run in simple mode (restricted tool set).
    pub simple_mode: bool,
    /// Optional working directory override.
    pub working_dir: Option<String>,
    /// Optional session directory for transcript persistence.
    #[serde(default)]
    pub session_dir: Option<PathBuf>,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            description: String::new(),
            prompt: String::new(),
            model: None,
            max_turns: DEFAULT_WORKER_MAX_TURNS,
            simple_mode: false,
            working_dir: None,
            session_dir: None,
        }
    }
}

/// Result of a worker execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerResult {
    /// The worker's agent ID.
    pub agent_id: String,
    /// Human-readable description.
    pub description: String,
    /// Final status of the worker.
    pub status: WorkerStatus,
    /// The worker's output text.
    pub output: Option<String>,
    /// Token usage summary.
    pub usage: UsageSummary,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Number of tool uses.
    pub tool_use_count: u32,
}

/// Lifecycle status of a worker agent.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkerStatus {
    /// Worker is idle and ready to accept tasks.
    #[default]
    Idle,
    /// Worker is actively running.
    Running,
    /// Worker completed successfully.
    Completed,
    /// Worker failed with an error.
    Failed,
    /// Worker was killed/stopped.
    Killed,
}

impl std::fmt::Display for WorkerStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => write!(f, "idle"),
            Self::Running => write!(f, "running"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Killed => write!(f, "killed"),
        }
    }
}

/// A worker agent with lifecycle management.
///
/// Tracks the state of a single worker from spawn through completion,
/// including timing, usage, and status transitions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerAgent {
    /// Unique agent ID.
    pub agent_id: String,
    /// Human-readable description.
    pub description: String,
    /// Current status.
    pub status: WorkerStatus,
    /// Configuration used to spawn this worker.
    pub config: WorkerConfig,
    /// When the worker was started.
    #[serde(skip)]
    started_at: Option<Instant>,
    /// Token usage so far.
    pub usage: UsageSummary,
    /// Number of tool uses.
    pub tool_use_count: u32,
    /// Final output text.
    pub output: Option<String>,
    /// Accumulated conversation messages for transcript persistence.
    #[serde(default)]
    pub transcript_messages: Vec<TranscriptMessage>,
}

impl WorkerAgent {
    /// Create a new worker agent with the given ID and configuration.
    pub fn new(agent_id: impl Into<String>, config: WorkerConfig) -> Self {
        Self {
            agent_id: agent_id.into(),
            description: config.description.clone(),
            status: WorkerStatus::Idle,
            config,
            started_at: None,
            usage: UsageSummary::default(),
            tool_use_count: 0,
            output: None,
            transcript_messages: Vec::new(),
        }
    }

    /// Start the worker, transitioning from Idle to Running.
    ///
    /// Returns `Ok(())` if the transition was successful, or an error
    /// message if the worker is not in Idle state.
    pub fn start(&mut self) -> std::result::Result<(), String> {
        if self.status != WorkerStatus::Idle {
            return Err(format!("Cannot start worker in {} state", self.status));
        }
        self.status = WorkerStatus::Running;
        self.started_at = Some(Instant::now());
        Ok(())
    }

    /// Mark the worker as completed with the given output.
    ///
    /// If a session directory is configured, the transcript is automatically
    /// persisted to disk.
    pub fn complete(&mut self, output: String) -> std::result::Result<(), String> {
        if self.status != WorkerStatus::Running {
            return Err(format!("Cannot complete worker in {} state", self.status));
        }
        self.status = WorkerStatus::Completed;
        self.output = Some(output);
        let _ = self.try_persist_transcript();
        Ok(())
    }

    /// Mark the worker as failed with an error message.
    ///
    /// If a session directory is configured, the transcript is automatically
    /// persisted to disk.
    pub fn fail(&mut self, error: String) -> std::result::Result<(), String> {
        if self.status != WorkerStatus::Running {
            return Err(format!("Cannot fail worker in {} state", self.status));
        }
        self.status = WorkerStatus::Failed;
        self.output = Some(error);
        let _ = self.try_persist_transcript();
        Ok(())
    }

    /// Kill the worker.
    ///
    /// If a session directory is configured, the transcript is automatically
    /// persisted to disk.
    pub fn kill(&mut self) -> std::result::Result<(), String> {
        if self.status != WorkerStatus::Running {
            return Err(format!("Cannot kill worker in {} state", self.status));
        }
        self.status = WorkerStatus::Killed;
        let _ = self.try_persist_transcript();
        Ok(())
    }

    /// Update usage statistics from a run result.
    pub fn update_usage(&mut self, result: &AgentRunResult) {
        self.usage.input_tokens += result.usage.input_tokens;
        self.usage.output_tokens += result.usage.output_tokens;
        self.usage.cache_creation_tokens += result.usage.cache_creation_tokens;
        self.usage.cache_read_tokens += result.usage.cache_read_tokens;
        self.tool_use_count += result.turns;
    }

    /// Get the elapsed time since the worker was started.
    pub fn elapsed_ms(&self) -> u64 {
        self.started_at
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(0)
    }

    /// Check if the worker is still active (running or idle).
    pub fn is_active(&self) -> bool {
        matches!(self.status, WorkerStatus::Idle | WorkerStatus::Running)
    }

    /// Check if the worker has finished (completed, failed, or killed).
    pub fn is_finished(&self) -> bool {
        matches!(
            self.status,
            WorkerStatus::Completed | WorkerStatus::Failed | WorkerStatus::Killed
        )
    }

    /// Convert to a worker result.
    pub fn to_result(&self) -> WorkerResult {
        WorkerResult {
            agent_id: self.agent_id.clone(),
            description: self.description.clone(),
            status: self.status,
            output: self.output.clone(),
            usage: self.usage.clone(),
            duration_ms: self.elapsed_ms(),
            tool_use_count: self.tool_use_count,
        }
    }

    /// Format this worker's result as a task notification XML.
    pub fn to_task_notification(&self) -> String {
        let notification_status = match self.status {
            WorkerStatus::Completed => TaskNotificationStatus::Completed,
            WorkerStatus::Failed => TaskNotificationStatus::Failed,
            WorkerStatus::Killed => TaskNotificationStatus::Killed,
            _ => TaskNotificationStatus::Completed, // Default for idle/running
        };

        let summary = match self.status {
            WorkerStatus::Completed => {
                format!("Agent \"{}\" completed", self.description)
            }
            WorkerStatus::Failed => {
                format!("Agent \"{}\" failed", self.description)
            }
            WorkerStatus::Killed => {
                format!("Agent \"{}\" was stopped", self.description)
            }
            _ => format!("Agent \"{}\" {}", self.description, self.status),
        };

        let usage = TaskUsage {
            total_tokens: self.usage.input_tokens + self.usage.output_tokens,
            tool_uses: self.tool_use_count,
            duration_ms: self.elapsed_ms(),
        };

        let params = TaskNotificationParams {
            task_id: self.agent_id.clone(),
            status: notification_status,
            summary,
            result: self.output.clone(),
            usage: Some(usage),
        };

        format_task_notification(&params)
    }

    /// Add a message to the transcript log.
    ///
    /// Call this as messages arrive during the worker's execution so that
    /// the transcript is available when the worker finishes.
    pub fn add_transcript_message(&mut self, role: impl Into<String>, content: impl Into<String>) {
        self.transcript_messages
            .push(TranscriptMessage::new(role, content));
    }

    /// Try to persist the transcript to disk.
    ///
    /// Called automatically by [`complete`](Self::complete),
    /// [`fail`](Self::fail), and [`kill`](Self::kill) when a session
    /// directory is configured. Returns `Ok(())` if no session directory is
    /// set (no-op), or the result of the write attempt.
    pub fn try_persist_transcript(&self) -> anyhow::Result<()> {
        let Some(ref session_dir) = self.config.session_dir else {
            return Ok(());
        };

        let result = self.to_result();
        if self.transcript_messages.is_empty() {
            // No accumulated messages -- persist from the result alone.
            persist_transcript_from_result(session_dir, &result)
        } else {
            persist_transcript(session_dir, &result, &self.transcript_messages)
        }
    }

    /// Manually persist the transcript with an explicit session directory.
    ///
    /// Use this when you want to persist outside the auto-persist flow,
    /// or when the session directory was not set at construction time.
    pub fn persist_transcript_to(&self, session_dir: &std::path::Path) -> anyhow::Result<()> {
        let result = self.to_result();
        if self.transcript_messages.is_empty() {
            persist_transcript_from_result(session_dir, &result)
        } else {
            persist_transcript(session_dir, &result, &self.transcript_messages)
        }
    }
}

/// Spawn a new worker agent with a generated ID.
///
/// Creates a [`WorkerAgent`] in Idle state, ready to be started.
/// The agent ID is generated from the description.
pub fn spawn_worker(config: WorkerConfig) -> WorkerAgent {
    let agent_id = generate_worker_id();
    WorkerAgent::new(agent_id, config)
}

/// Generate a unique worker agent ID.
fn generate_worker_id() -> String {
    let uuid = uuid::Uuid::new_v4();
    format!("worker-{uuid}")
}

/// Get the worker agent definition.
///
/// Workers have access to all async-agent-allowed tools and run with
/// the default model.
pub fn worker_agent_definition() -> AgentDefinition {
    AgentDefinition {
        agent_type: WORKER_AGENT.to_owned(),
        when_to_use: "General-purpose worker that executes tasks autonomously.".to_owned(),
        tools: vec!["*".to_owned()],
        disallowed_tools: Vec::new(),
        max_turns: DEFAULT_WORKER_MAX_TURNS,
        model: None,
        effort: None,
        permission_mode: None,
        source: crate::definition::AgentSource::BuiltIn,
        base_dir: "built-in".to_owned(),
        system_prompt: Some(String::new()),
        skills: Vec::new(),
        mcp_servers: Vec::new(),
        hooks: None,
        color: None,
        critical_system_reminder_experimental: None,
        required_mcp_servers: Vec::new(),
        memory: None,
        background: false,
        isolation: crate::definition::AgentIsolation::None,
        initial_prompt: None,
        omit_claude_md: false,
        filename: None,
    }
}

/// Parse the effective tool set for a worker.
///
/// Filters the full async-agent-allowed tools by removing internal tools.
/// Returns a sorted list of tool names available to workers.
pub fn parse_worker_tools(simple_mode: bool) -> Vec<String> {
    let internal = internal_worker_tools_set();
    let all_tools = async_agent_allowed_tools_set();

    if simple_mode {
        // Simple mode: only Bash, Read, Edit
        let mut tools = vec!["Bash".to_owned(), "Read".to_owned(), "Edit".to_owned()];
        tools.sort();
        tools
    } else {
        let mut tools: Vec<String> = all_tools
            .into_iter()
            .filter(|t| !internal.contains(t))
            .collect();
        tools.sort();
        tools
    }
}

/// Build a run config for a worker from its configuration.
pub fn worker_run_config(worker_config: &WorkerConfig) -> AgentRunConfig {
    let tools = parse_worker_tools(worker_config.simple_mode);
    let working_dir = worker_config
        .working_dir
        .as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
        });

    AgentRunConfig {
        max_turns: worker_config.max_turns,
        model: worker_config.model.clone().unwrap_or_default(),
        tools,
        system_prompt: None,
        working_dir,
        additional_working_directories: Vec::new(),
    }
}

/// Format a task notification XML for a completed worker.
pub fn format_task_notification_xml(
    agent_id: &str,
    description: &str,
    status: TaskNotificationStatus,
    result: Option<&str>,
    usage: Option<&UsageSummary>,
) -> String {
    let summary = match status {
        TaskNotificationStatus::Completed => format!("Agent \"{description}\" completed"),
        TaskNotificationStatus::Failed => format!(
            "Agent \"{description}\" failed: {}",
            result.unwrap_or("unknown error")
        ),
        TaskNotificationStatus::Killed => format!("Agent \"{description}\" was stopped"),
    };

    let task_usage = usage.map(|u| TaskUsage {
        total_tokens: u.input_tokens + u.output_tokens,
        tool_uses: 0,
        duration_ms: 0,
    });

    let params = TaskNotificationParams {
        task_id: agent_id.to_owned(),
        status,
        summary,
        result: result.map(|s| s.to_owned()),
        usage: task_usage,
    };

    format_task_notification(&params)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_config_default() {
        let config = WorkerConfig::default();
        assert!(config.description.is_empty());
        assert!(config.prompt.is_empty());
        assert!(config.model.is_none());
        assert_eq!(config.max_turns, 200);
        assert!(!config.simple_mode);
    }

    #[test]
    fn worker_status_default_is_idle() {
        assert_eq!(WorkerStatus::default(), WorkerStatus::Idle);
    }

    #[test]
    fn worker_status_display() {
        assert_eq!(WorkerStatus::Idle.to_string(), "idle");
        assert_eq!(WorkerStatus::Running.to_string(), "running");
        assert_eq!(WorkerStatus::Completed.to_string(), "completed");
        assert_eq!(WorkerStatus::Failed.to_string(), "failed");
        assert_eq!(WorkerStatus::Killed.to_string(), "killed");
    }

    #[test]
    fn worker_status_serde() {
        let status = WorkerStatus::Running;
        let json = serde_json::to_string(&status).expect("serialize");
        let parsed: WorkerStatus = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, status);
    }

    #[test]
    fn worker_agent_new() {
        let config = WorkerConfig {
            description: "Test worker".to_owned(),
            prompt: "Do something".to_owned(),
            ..Default::default()
        };
        let worker = WorkerAgent::new("worker-1", config);
        assert_eq!(worker.agent_id, "worker-1");
        assert_eq!(worker.description, "Test worker");
        assert_eq!(worker.status, WorkerStatus::Idle);
        assert!(worker.is_active());
        assert!(!worker.is_finished());
    }

    #[test]
    fn worker_agent_lifecycle() {
        let config = WorkerConfig {
            description: "Test".to_owned(),
            prompt: "Test".to_owned(),
            ..Default::default()
        };
        let mut worker = WorkerAgent::new("w-1", config);

        // Start
        assert!(worker.start().is_ok());
        assert_eq!(worker.status, WorkerStatus::Running);

        // Cannot start again
        assert!(worker.start().is_err());

        // Complete
        assert!(worker.complete("Done".to_owned()).is_ok());
        assert_eq!(worker.status, WorkerStatus::Completed);
        assert_eq!(worker.output, Some("Done".to_owned()));
        assert!(worker.is_finished());
    }

    #[test]
    fn worker_agent_fail() {
        let config = WorkerConfig::default();
        let mut worker = WorkerAgent::new("w-2", config);
        assert!(worker.start().is_ok());
        assert!(worker.fail("Error occurred".to_owned()).is_ok());
        assert_eq!(worker.status, WorkerStatus::Failed);
        assert_eq!(worker.output, Some("Error occurred".to_owned()));
    }

    #[test]
    fn worker_agent_kill() {
        let config = WorkerConfig::default();
        let mut worker = WorkerAgent::new("w-3", config);
        assert!(worker.start().is_ok());
        assert!(worker.kill().is_ok());
        assert_eq!(worker.status, WorkerStatus::Killed);
    }

    #[test]
    fn worker_cannot_complete_from_idle() {
        let config = WorkerConfig::default();
        let mut worker = WorkerAgent::new("w-4", config);
        assert!(worker.complete("Done".to_owned()).is_err());
    }

    #[test]
    fn worker_cannot_fail_from_idle() {
        let config = WorkerConfig::default();
        let mut worker = WorkerAgent::new("w-5", config);
        assert!(worker.fail("Error".to_owned()).is_err());
    }

    #[test]
    fn worker_cannot_kill_from_completed() {
        let config = WorkerConfig::default();
        let mut worker = WorkerAgent::new("w-6", config);
        assert!(worker.start().is_ok());
        assert!(worker.complete("Done".to_owned()).is_ok());
        assert!(worker.kill().is_err());
    }

    #[test]
    fn spawn_worker_creates_valid_agent() {
        let config = WorkerConfig {
            description: "Research task".to_owned(),
            prompt: "Find all auth files".to_owned(),
            ..Default::default()
        };
        let worker = spawn_worker(config);
        assert!(worker.agent_id.starts_with("worker-"));
        assert_eq!(worker.status, WorkerStatus::Idle);
    }

    #[test]
    fn worker_agent_definition_type() {
        let def = worker_agent_definition();
        assert_eq!(def.agent_type, "worker");
        assert_eq!(def.tools, vec!["*"]);
    }

    #[test]
    fn parse_worker_tools_normal_mode() {
        let tools = parse_worker_tools(false);
        assert!(tools.contains(&"Bash".to_owned()));
        assert!(tools.contains(&"Read".to_owned()));
        assert!(tools.contains(&"Edit".to_owned()));
        // Internal tools should be filtered
        assert!(!tools.contains(&"SendMessage".to_owned()));
        assert!(!tools.contains(&"SyntheticOutput".to_owned()));
    }

    #[test]
    fn parse_worker_tools_simple_mode() {
        let tools = parse_worker_tools(true);
        assert_eq!(tools, vec!["Bash", "Edit", "Read"]);
    }

    #[test]
    fn worker_run_config_builds_correctly() {
        let config = WorkerConfig {
            description: "Test".to_owned(),
            prompt: "Test".to_owned(),
            max_turns: 50,
            model: Some("haiku".to_owned()),
            simple_mode: false,
            working_dir: Some("/tmp/test".to_owned()),
            session_dir: None,
        };
        let run_config = worker_run_config(&config);
        assert_eq!(run_config.max_turns, 50);
        assert_eq!(run_config.model, "haiku");
        assert_eq!(
            run_config.working_dir,
            std::path::PathBuf::from("/tmp/test")
        );
    }

    #[test]
    fn worker_to_result() {
        let config = WorkerConfig {
            description: "Test".to_owned(),
            prompt: "Test".to_owned(),
            ..Default::default()
        };
        let mut worker = WorkerAgent::new("w-result", config);
        assert!(worker.start().is_ok());
        assert!(worker.complete("Done".to_owned()).is_ok());

        let result = worker.to_result();
        assert_eq!(result.agent_id, "w-result");
        assert_eq!(result.status, WorkerStatus::Completed);
        assert_eq!(result.output, Some("Done".to_owned()));
    }

    #[test]
    fn worker_to_task_notification() {
        let config = WorkerConfig {
            description: "Research auth".to_owned(),
            prompt: "Find auth files".to_owned(),
            ..Default::default()
        };
        let mut worker = WorkerAgent::new("w-notif", config);
        assert!(worker.start().is_ok());
        assert!(worker.complete("Found 3 files".to_owned()).is_ok());

        let xml = worker.to_task_notification();
        assert!(xml.contains("<task-notification>"));
        assert!(xml.contains("<task-id>w-notif</task-id>"));
        assert!(xml.contains("<status>completed</status>"));
        assert!(xml.contains("Research auth"));
        assert!(xml.contains("Found 3 files"));
    }

    #[test]
    fn format_task_notification_xml_helper() {
        let xml = format_task_notification_xml(
            "agent-1",
            "Test task",
            TaskNotificationStatus::Completed,
            Some("Result text"),
            None,
        );
        assert!(xml.contains("<task-id>agent-1</task-id>"));
        assert!(xml.contains("<status>completed</status>"));
        assert!(xml.contains("Result text"));
    }

    #[test]
    fn worker_update_usage() {
        let config = WorkerConfig::default();
        let mut worker = WorkerAgent::new("w-usage", config);
        let result = AgentRunResult {
            output: "test".to_owned(),
            success: true,
            turns: 3,
            usage: UsageSummary {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_tokens: 10,
                cache_read_tokens: 20,
            },
        };
        worker.update_usage(&result);
        assert_eq!(worker.usage.input_tokens, 100);
        assert_eq!(worker.usage.output_tokens, 50);
        assert_eq!(worker.tool_use_count, 3);
    }

    #[test]
    fn worker_result_serde() {
        let result = WorkerResult {
            agent_id: "w-1".to_owned(),
            description: "Test".to_owned(),
            status: WorkerStatus::Completed,
            output: Some("Done".to_owned()),
            usage: UsageSummary::default(),
            duration_ms: 1000,
            tool_use_count: 5,
        };
        let json = serde_json::to_string(&result).expect("serialize");
        let parsed: WorkerResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.agent_id, "w-1");
        assert_eq!(parsed.status, WorkerStatus::Completed);
    }

    // -- Transcript persistence tests ------------------------------------

    #[test]
    fn worker_config_session_dir_default_none() {
        let config = WorkerConfig::default();
        assert!(config.session_dir.is_none());
    }

    #[test]
    fn complete_auto_persists_transcript() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = WorkerConfig {
            description: "Auto-persist test".to_owned(),
            prompt: "Do work".to_owned(),
            session_dir: Some(dir.path().to_path_buf()),
            ..Default::default()
        };
        let mut worker = WorkerAgent::new("w-auto", config);
        worker.add_transcript_message("user", "Fix the bug");
        worker.add_transcript_message("assistant", "Fixed it");

        assert!(worker.start().is_ok());
        assert!(worker.complete("All done".to_owned()).is_ok());

        // Transcript file should exist
        let transcript_path = dir.path().join("transcripts").join("w-auto.json");
        assert!(
            transcript_path.exists(),
            "transcript file should be created"
        );

        let loaded: crate::transcript::SubagentTranscript =
            serde_json::from_str(&std::fs::read_to_string(&transcript_path).expect("read"))
                .expect("parse");
        assert_eq!(loaded.agent_id, "w-auto");
        assert_eq!(loaded.status, WorkerStatus::Completed);
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.messages[0].content, "Fix the bug");
    }

    #[test]
    fn fail_auto_persists_transcript() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = WorkerConfig {
            description: "Fail test".to_owned(),
            prompt: "Do work".to_owned(),
            session_dir: Some(dir.path().to_path_buf()),
            ..Default::default()
        };
        let mut worker = WorkerAgent::new("w-fail", config);

        assert!(worker.start().is_ok());
        assert!(worker.fail("Something went wrong".to_owned()).is_ok());

        let transcript_path = dir.path().join("transcripts").join("w-fail.json");
        assert!(transcript_path.exists());

        let loaded: crate::transcript::SubagentTranscript =
            serde_json::from_str(&std::fs::read_to_string(&transcript_path).expect("read"))
                .expect("parse");
        assert_eq!(loaded.status, WorkerStatus::Failed);
    }

    #[test]
    fn kill_auto_persists_transcript() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = WorkerConfig {
            description: "Kill test".to_owned(),
            prompt: "Do work".to_owned(),
            session_dir: Some(dir.path().to_path_buf()),
            ..Default::default()
        };
        let mut worker = WorkerAgent::new("w-kill", config);

        assert!(worker.start().is_ok());
        assert!(worker.kill().is_ok());

        let transcript_path = dir.path().join("transcripts").join("w-kill.json");
        assert!(transcript_path.exists());
    }

    #[test]
    fn no_session_dir_is_noop_on_complete() {
        let config = WorkerConfig {
            description: "No dir test".to_owned(),
            prompt: "Test".to_owned(),
            session_dir: None,
            ..Default::default()
        };
        let mut worker = WorkerAgent::new("w-nodir", config);
        assert!(worker.start().is_ok());
        // Should succeed even though no session_dir is set
        assert!(worker.complete("Done".to_owned()).is_ok());
    }

    #[test]
    fn persist_transcript_to_explicit_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = WorkerConfig {
            description: "Manual persist".to_owned(),
            prompt: "Test".to_owned(),
            session_dir: None,
            ..Default::default()
        };
        let mut worker = WorkerAgent::new("w-manual", config);
        worker.add_transcript_message("user", "Do the thing");
        worker.add_transcript_message("assistant", "Done");

        assert!(worker.start().is_ok());
        assert!(worker.complete("Finished".to_owned()).is_ok());

        // Manually persist to an explicit directory
        worker.persist_transcript_to(dir.path()).expect("persist");

        let transcript_path = dir.path().join("transcripts").join("w-manual.json");
        assert!(transcript_path.exists());

        let loaded: crate::transcript::SubagentTranscript =
            serde_json::from_str(&std::fs::read_to_string(&transcript_path).expect("read"))
                .expect("parse");
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.messages[0].content, "Do the thing");
    }

    #[test]
    fn add_transcript_message_accumulates() {
        let config = WorkerConfig::default();
        let mut worker = WorkerAgent::new("w-msgs", config);
        assert!(worker.transcript_messages.is_empty());

        worker.add_transcript_message("user", "Hello");
        worker.add_transcript_message("assistant", "Hi");
        worker.add_transcript_message("assistant", "How can I help?");

        assert_eq!(worker.transcript_messages.len(), 3);
    }
}
