//! Multi-agent system with mailbox-based task coordination and Claude Code–style
//! agent definitions.
//!
//! This crate provides two complementary layers:
//!
//! 1. **[`AgentScheduler`]** — A low-level team scheduler that manages agents,
//!    tasks, mailboxes, budgets, and lifecycle events.
//!
//! 2. **Agent definitions & tools** — High-level agent types matching Claude
//!    Code's `AgentTool/` system, including built-in agents, fork subagents,
//!    prompt building, memory, display, coordinator/worker mode, and resume.
//!
//! # Module layout
//!
//! - [`definition`]   — [`AgentDefinition`] struct and related types
//! - [`constants`]    — Agent tool constants
//! - [`builtins`]     — Built-in agent registry (6 agents)
//! - [`prompt`]       — Agent tool prompt builder
//! - [`fork`]         — Fork subagent support
//! - [`runner`]       — Agent execution runner
//! - [`memory`]       — Agent memory management & snapshots
//! - [`display`]      — Agent display/color management
//! - [`loader`]       — Agent directory loader
//! - [`coordinator`]  — Coordinator/Worker mode
//! - [`worker`]       — Worker agent lifecycle
//! - [`resume`]       — Agent checkpoint & resume
//! - [`transcript`]   — Subagent transcript persistence

// ── Modules ──────────────────────────────────────────────────────────────
pub mod builtins;
pub mod constants;
pub mod coordinator;
pub mod definition;
pub mod display;
pub mod fork;
pub mod loader;
pub mod memory;
pub mod prompt;
pub mod resume;
pub mod runner;
pub mod transcript;
pub mod worker;

// ── Existing scheduler types ─────────────────────────────────────────────

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

// Re-export key types from submodules at the crate root for backward compat.
pub use coordinator::{CoordinatorMode, TaskNotificationStatus, TaskUsage};
pub use definition::{AgentDefinition, AgentIsolation, AgentMemoryScope, AgentSource};
pub use resume::{AgentCheckpoint, ResumableAgentState};
pub use runner::{
    AgentExecutionRequest, AgentExecutor, AgentRunConfig, AgentRunResult, AgentRunner,
    ConversationEntry, UsageSummary, compose_agent_system_prompt,
};
pub use transcript::{
    SubagentTranscript, TranscriptMessage, persist_transcript, persist_transcript_from_result,
};
pub use worker::{WorkerAgent, WorkerConfig, WorkerResult, WorkerStatus};

/// Current state of an agent.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// Agent is idle and available for task assignment.
    #[default]
    Idle,
    /// Agent is actively working on a task.
    Busy,
    /// Agent is draining and not accepting new tasks.
    Draining,
    /// Agent is offline.
    Offline,
}

/// Lifecycle state of a task.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// Task is waiting to be assigned.
    #[default]
    Pending,
    /// Task has been assigned to an agent but not started.
    Assigned,
    /// Task is actively running.
    Running,
    /// Task is waiting for a tool call to complete.
    WaitingOnTool,
    /// Task is waiting for user approval.
    WaitingOnApproval,
    /// Task completed successfully.
    Completed,
    /// Task failed.
    Failed,
    /// Task was cancelled.
    Cancelled,
}

/// Kind of message sent between agents via mailboxes.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    /// An instruction to perform work.
    #[default]
    Instruction,
    /// A slice of context data.
    ContextSlice,
    /// A result from a completed sub-task.
    Result,
    /// A lifecycle event notification.
    Event,
    /// An approval request.
    Approval,
    /// A shutdown signal.
    Shutdown,
}

/// Scope for tool budget tracking.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BudgetScope {
    /// Read operations budget.
    #[default]
    Read,
    /// Edit operations budget.
    Edit,
    /// Shell command budget.
    Command,
    /// Network call budget.
    Network,
}

/// Per-task tool call budget with separate counters per scope.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolBudget {
    /// Remaining read calls.
    pub read_calls: u32,
    /// Remaining edit calls.
    pub edit_calls: u32,
    /// Remaining command calls.
    pub command_calls: u32,
    /// Remaining network calls.
    pub network_calls: u32,
}

impl ToolBudget {
    /// Check whether the budget allows a call in the given scope.
    #[must_use]
    pub fn allows(&self, scope: BudgetScope) -> bool {
        self.remaining(scope) > 0
    }

    /// Return the remaining budget for the given scope.
    #[must_use]
    pub fn remaining(&self, scope: BudgetScope) -> u32 {
        match scope {
            BudgetScope::Read => self.read_calls,
            BudgetScope::Edit => self.edit_calls,
            BudgetScope::Command => self.command_calls,
            BudgetScope::Network => self.network_calls,
        }
    }

    /// Consume one unit from the given scope's budget.
    ///
    /// Returns `true` if the budget was decremented, `false` if it was already exhausted.
    pub fn consume(&mut self, scope: BudgetScope) -> bool {
        let counter = match scope {
            BudgetScope::Read => &mut self.read_calls,
            BudgetScope::Edit => &mut self.edit_calls,
            BudgetScope::Command => &mut self.command_calls,
            BudgetScope::Network => &mut self.network_calls,
        };
        if *counter == 0 {
            return false;
        }
        *counter -= 1;
        true
    }
}

/// A slice of context data shared between agents.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextSlice {
    /// Brief summary of the context.
    #[serde(default)]
    pub summary: String,
    /// Paths to artifacts produced by the originating agent.
    #[serde(default)]
    pub artifact_paths: Vec<String>,
    /// Environment hints for the receiving agent.
    #[serde(default)]
    pub environment_hints: BTreeMap<String, String>,
    /// Estimated token count for this slice.
    #[serde(default)]
    pub token_estimate: u32,
}

/// Identity and configuration of a single agent in the team.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentIdentity {
    /// Unique agent identifier.
    pub agent_id: Uuid,
    /// Human-readable agent name.
    pub name: String,
    /// Role description (e.g. "lead", "worker").
    pub role: String,
    /// File paths this agent has ownership over.
    #[serde(default)]
    pub ownership_paths: Vec<String>,
    /// Maximum number of tasks this agent can handle concurrently.
    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: usize,
    /// Arbitrary key-value labels for task matching.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    /// Current agent state.
    #[serde(default)]
    pub state: AgentState,
}

fn default_max_concurrency() -> usize {
    1
}

impl AgentIdentity {
    /// Create a new agent identity with a random UUID.
    #[must_use]
    pub fn new(name: impl Into<String>, role: impl Into<String>) -> Self {
        Self {
            agent_id: Uuid::new_v4(),
            name: name.into(),
            role: role.into(),
            ownership_paths: Vec::new(),
            max_concurrency: default_max_concurrency(),
            labels: BTreeMap::new(),
            state: AgentState::Idle,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentTask {
    pub id: Uuid,
    pub title: String,
    pub owner: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub state: TaskState,
    /// ID of the parent task, if this task was spawned as a sub-task.
    #[serde(default)]
    pub parent_task_id: Option<Uuid>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub ownership_paths: Vec<String>,
    #[serde(default)]
    pub required_labels: BTreeMap<String, String>,
    #[serde(default)]
    pub context: ContextSlice,
    #[serde(default)]
    pub budget: ToolBudget,
    #[serde(default)]
    pub result_summary: Option<String>,
    #[serde(default)]
    pub failure_message: Option<String>,
}

impl AgentTask {
    #[must_use]
    pub fn new(title: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            title: title.into(),
            owner: None,
            created_at: now,
            updated_at: now,
            state: TaskState::Pending,
            parent_task_id: None,
            description: String::new(),
            ownership_paths: Vec::new(),
            required_labels: BTreeMap::new(),
            context: ContextSlice::default(),
            budget: ToolBudget::default(),
            result_summary: None,
            failure_message: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentMailboxMessage {
    pub message_id: Uuid,
    pub team_id: Uuid,
    pub from: String,
    pub to: Uuid,
    pub kind: MessageKind,
    pub subject: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamPlan {
    pub team_id: Uuid,
    pub lead: String,
    pub objective: String,
    pub tasks: Vec<AgentTask>,
}

impl TeamPlan {
    #[must_use]
    pub fn new(lead: impl Into<String>, objective: impl Into<String>) -> Self {
        Self {
            team_id: Uuid::new_v4(),
            lead: lead.into(),
            objective: objective.into(),
            tasks: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamSnapshot {
    pub team_id: Uuid,
    pub lead: String,
    pub objective: String,
    pub agents: Vec<AgentIdentity>,
    pub tasks: Vec<AgentTask>,
    pub pending_messages: usize,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SchedulerSummary {
    pub team_id: Uuid,
    pub total_agents: usize,
    pub total_tasks: usize,
    pub pending_tasks: usize,
    pub active_tasks: usize,
    pub completed_tasks: usize,
    pub failed_tasks: usize,
    pub pending_messages: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentLifecycleEvent {
    pub event_id: Uuid,
    pub team_id: Uuid,
    pub kind: String,
    pub created_at: DateTime<Utc>,
    pub actor: String,
    #[serde(default)]
    pub task_id: Option<Uuid>,
    #[serde(default)]
    pub agent_id: Option<Uuid>,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct AgentScheduler {
    team_id: Uuid,
    lead: String,
    objective: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    agents: BTreeMap<Uuid, AgentIdentity>,
    tasks: BTreeMap<Uuid, AgentTask>,
    mailboxes: BTreeMap<Uuid, VecDeque<AgentMailboxMessage>>,
    active_task_count: BTreeMap<Uuid, usize>,
    events: Vec<AgentLifecycleEvent>,
    /// Cancellation tokens for tasks that have been started.
    /// When a task is cancelled, its token is fired so that any async work
    /// listening on it will terminate cooperatively.
    cancel_tokens: Arc<std::sync::Mutex<BTreeMap<Uuid, CancellationToken>>>,
}

impl AgentScheduler {
    #[must_use]
    pub fn new(lead: impl Into<String>, objective: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            team_id: Uuid::new_v4(),
            lead: lead.into(),
            objective: objective.into(),
            created_at: now,
            updated_at: now,
            agents: BTreeMap::new(),
            tasks: BTreeMap::new(),
            mailboxes: BTreeMap::new(),
            active_task_count: BTreeMap::new(),
            events: Vec::new(),
            cancel_tokens: Arc::new(std::sync::Mutex::new(BTreeMap::new())),
        }
    }

    #[must_use]
    pub fn team_id(&self) -> Uuid {
        self.team_id
    }

    pub fn register_agent(&mut self, mut agent: AgentIdentity) -> Uuid {
        agent.state = AgentState::Idle;
        let agent_id = agent.agent_id;
        self.mailboxes.entry(agent_id).or_default();
        self.active_task_count.entry(agent_id).or_insert(0);
        self.agents.insert(agent_id, agent.clone());
        self.record_event(
            "agent_registered",
            "scheduler",
            None,
            Some(agent_id),
            agent.name,
        );
        agent_id
    }

    pub fn add_task(&mut self, task: AgentTask) -> Uuid {
        let task_id = task.id;
        self.tasks.insert(task_id, task.clone());
        self.updated_at = Utc::now();
        self.record_event("task_added", "scheduler", Some(task_id), None, task.title);
        task_id
    }

    pub fn queue_instruction(
        &mut self,
        to: Uuid,
        from: impl Into<String>,
        subject: impl Into<String>,
        body: impl Into<String>,
    ) -> Option<Uuid> {
        self.send_message(
            to,
            from,
            MessageKind::Instruction,
            subject,
            body,
            BTreeMap::new(),
        )
    }

    pub fn send_message(
        &mut self,
        to: Uuid,
        from: impl Into<String>,
        kind: MessageKind,
        subject: impl Into<String>,
        body: impl Into<String>,
        metadata: BTreeMap<String, String>,
    ) -> Option<Uuid> {
        if !self.agents.contains_key(&to) {
            return None;
        }
        let subject = subject.into();
        let message = AgentMailboxMessage {
            message_id: Uuid::new_v4(),
            team_id: self.team_id,
            from: from.into(),
            to,
            kind,
            subject: subject.clone(),
            body: body.into(),
            created_at: Utc::now(),
            metadata,
        };
        let message_id = message.message_id;
        self.mailboxes.entry(to).or_default().push_back(message);
        self.updated_at = Utc::now();
        self.record_event(
            "message_queued",
            "scheduler",
            None,
            Some(to),
            format!("queued `{subject}`"),
        );
        Some(message_id)
    }

    pub fn drain_mailbox(&mut self, agent_id: Uuid) -> Vec<AgentMailboxMessage> {
        let messages = self
            .mailboxes
            .entry(agent_id)
            .or_default()
            .drain(..)
            .collect::<Vec<_>>();
        if !messages.is_empty() {
            self.updated_at = Utc::now();
            self.record_event(
                "mailbox_drained",
                "scheduler",
                None,
                Some(agent_id),
                format!("delivered {} messages", messages.len()),
            );
        }
        messages
    }

    pub fn assign_next_task(&mut self) -> Option<(Uuid, Uuid)> {
        let task_id = self.next_assignable_task_id()?;
        let agent_id = self.select_agent_for_task(task_id)?;
        self.assign_task(task_id, agent_id)?;
        Some((task_id, agent_id))
    }

    pub fn assign_task(&mut self, task_id: Uuid, agent_id: Uuid) -> Option<()> {
        let agent = self.agents.get_mut(&agent_id)?;
        let task = self.tasks.get_mut(&task_id)?;
        if !matches!(task.state, TaskState::Pending | TaskState::Assigned) {
            return None;
        }
        let title = task.title.clone();
        task.owner = Some(agent_id);
        task.state = TaskState::Assigned;
        task.updated_at = Utc::now();
        agent.state = AgentState::Busy;
        self.updated_at = Utc::now();
        self.record_event(
            "task_assigned",
            "scheduler",
            Some(task_id),
            Some(agent_id),
            format!("assigned `{title}`"),
        );
        Some(())
    }

    pub fn start_task(&mut self, task_id: Uuid) -> Option<()> {
        let task = self.tasks.get_mut(&task_id)?;
        let agent_id = task.owner?;
        if !matches!(
            task.state,
            TaskState::Assigned | TaskState::WaitingOnTool | TaskState::WaitingOnApproval
        ) {
            return None;
        }
        task.state = TaskState::Running;
        task.updated_at = Utc::now();
        *self.active_task_count.entry(agent_id).or_insert(0) += 1;
        if let Some(agent) = self.agents.get_mut(&agent_id) {
            agent.state = AgentState::Busy;
        }
        // Register a cancellation token for this task so that callers can
        // cancel it cooperatively via [`cancel_task_with_propagation`].
        self.cancel_tokens
            .lock()
            .expect("cancel_tokens lock")
            .insert(task_id, CancellationToken::new());
        self.updated_at = Utc::now();
        self.record_event(
            "task_started",
            "scheduler",
            Some(task_id),
            Some(agent_id),
            String::new(),
        );
        Some(())
    }

    pub fn mark_waiting_on_tool(&mut self, task_id: Uuid) -> Option<()> {
        self.transition_waiting(task_id, TaskState::WaitingOnTool, "task_waiting_on_tool")
    }

    pub fn mark_waiting_on_approval(&mut self, task_id: Uuid) -> Option<()> {
        self.transition_waiting(
            task_id,
            TaskState::WaitingOnApproval,
            "task_waiting_on_approval",
        )
    }

    pub fn complete_task(&mut self, task_id: Uuid, summary: impl Into<String>) -> Option<()> {
        self.finish_task(
            task_id,
            TaskState::Completed,
            Some(summary.into()),
            None,
            "task_completed",
        )
    }

    pub fn fail_task(&mut self, task_id: Uuid, message: impl Into<String>) -> Option<()> {
        self.finish_task(
            task_id,
            TaskState::Failed,
            None,
            Some(message.into()),
            "task_failed",
        )
    }

    /// Cancel a task and propagate cancellation to all child sub-tasks recursively.
    ///
    /// Sets the task state to [`TaskState::Cancelled`], fires the associated
    /// cancellation token (if one exists) so that any async work listening on
    /// it will terminate cooperatively, and then recursively cancels all child
    /// tasks whose `parent_task_id` points to this task.
    ///
    /// Returns the total number of tasks cancelled (including the root).
    pub fn cancel_task_with_propagation(
        &mut self,
        task_id: Uuid,
        message: impl Into<String>,
    ) -> Option<usize> {
        let msg = message.into();
        self.cancel_task_recursive(task_id, msg)
    }

    /// Legacy single-task cancel without propagation.
    ///
    /// Prefer [`cancel_task_with_propagation`] which also cancels child tasks.
    pub fn cancel_task(&mut self, task_id: Uuid, message: impl Into<String>) -> Option<()> {
        self.finish_task(
            task_id,
            TaskState::Cancelled,
            None,
            Some(message.into()),
            "task_cancelled",
        )
    }

    /// Internal recursive cancellation implementation.
    ///
    /// Collects all descendant task IDs first to avoid borrow-checker issues,
    /// then cancels each one in turn.
    fn cancel_task_recursive(&mut self, task_id: Uuid, message: String) -> Option<usize> {
        // Verify the root task exists.
        if !self.tasks.contains_key(&task_id) {
            return None;
        }

        // Collect all descendant task IDs (BFS order, parent first).
        let mut to_cancel = vec![task_id];
        let mut queue = std::collections::VecDeque::from([task_id]);

        while let Some(parent_id) = queue.pop_front() {
            for task in self.tasks.values() {
                if task.parent_task_id == Some(parent_id) && !to_cancel.contains(&task.id) {
                    to_cancel.push(task.id);
                    queue.push_back(task.id);
                }
            }
        }

        let total = to_cancel.len();
        for tid in to_cancel {
            // Fire the cancellation token if one exists.
            if let Some(token) = self
                .cancel_tokens
                .lock()
                .expect("cancel_tokens lock")
                .remove(&tid)
            {
                token.cancel();
            }
            // Mark the task as cancelled.
            self.finish_task(
                tid,
                TaskState::Cancelled,
                None,
                Some(message.clone()),
                "task_cancelled",
            );
        }

        Some(total)
    }

    /// Spawn a child sub-task under an existing parent task.
    ///
    /// Creates a new [`AgentTask`] linked to `parent_task_id` via the
    /// [`AgentTask::parent_task_id`] field. The child inherits context
    /// from the parent (ownership paths and required labels are cloned).
    ///
    /// Returns the new task's ID, or `None` if the parent task does not exist.
    pub fn spawn_child_task(
        &mut self,
        parent_task_id: Uuid,
        title: impl Into<String>,
    ) -> Option<Uuid> {
        let (owner, ownership_paths, required_labels, parent_title) = {
            let parent = self.tasks.get(&parent_task_id)?;
            (
                parent.owner,
                parent.ownership_paths.clone(),
                parent.required_labels.clone(),
                parent.title.chars().take(60).collect::<String>(),
            )
        };

        let now = Utc::now();
        let child = AgentTask {
            id: Uuid::new_v4(),
            title: title.into(),
            owner,
            created_at: now,
            updated_at: now,
            state: TaskState::Pending,
            parent_task_id: Some(parent_task_id),
            description: String::new(),
            ownership_paths,
            required_labels,
            context: ContextSlice::default(),
            budget: ToolBudget::default(),
            result_summary: None,
            failure_message: None,
        };

        let child_id = child.id;
        self.tasks.insert(child_id, child.clone());
        self.updated_at = Utc::now();
        self.record_event(
            "child_task_spawned",
            "scheduler",
            Some(child_id),
            self.tasks
                .get(&parent_task_id)
                .map(|p| p.owner)
                .unwrap_or_default(),
            format!("spawned child of task {}", parent_title),
        );
        Some(child_id)
    }

    /// Get the cancellation token for a task, if one has been registered.
    ///
    /// Returns a cloned [`CancellationToken`] that the caller can poll or
    /// select on. The token is registered when [`start_task`] is called and
    /// removed when the task finishes or is cancelled.
    pub fn cancellation_token(&self, task_id: Uuid) -> Option<CancellationToken> {
        self.cancel_tokens
            .lock()
            .expect("cancel_tokens lock")
            .get(&task_id)
            .cloned()
    }

    /// Return the IDs of all direct children of the given task.
    pub fn child_task_ids(&self, parent_task_id: Uuid) -> Vec<Uuid> {
        self.tasks
            .values()
            .filter(|t| t.parent_task_id == Some(parent_task_id))
            .map(|t| t.id)
            .collect()
    }

    pub fn consume_budget(&mut self, task_id: Uuid, scope: BudgetScope) -> bool {
        let Some(task) = self.tasks.get_mut(&task_id) else {
            return false;
        };
        let consumed = task.budget.consume(scope);
        if consumed {
            let owner = task.owner;
            task.updated_at = Utc::now();
            self.updated_at = Utc::now();
            let message = format!("consumed {scope:?}");
            self.record_event(
                "task_budget_consumed",
                "scheduler",
                Some(task_id),
                owner,
                message,
            );
        }
        consumed
    }

    #[must_use]
    pub fn snapshot(&self) -> TeamSnapshot {
        TeamSnapshot {
            team_id: self.team_id,
            lead: self.lead.clone(),
            objective: self.objective.clone(),
            agents: self.agents.values().cloned().collect(),
            tasks: self.tasks.values().cloned().collect(),
            pending_messages: self.pending_message_count(),
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }

    #[must_use]
    pub fn summary(&self) -> SchedulerSummary {
        SchedulerSummary {
            team_id: self.team_id,
            total_agents: self.agents.len(),
            total_tasks: self.tasks.len(),
            pending_tasks: self
                .tasks
                .values()
                .filter(|task| matches!(task.state, TaskState::Pending | TaskState::Assigned))
                .count(),
            active_tasks: self
                .tasks
                .values()
                .filter(|task| {
                    matches!(
                        task.state,
                        TaskState::Running
                            | TaskState::WaitingOnApproval
                            | TaskState::WaitingOnTool
                    )
                })
                .count(),
            completed_tasks: self
                .tasks
                .values()
                .filter(|task| matches!(task.state, TaskState::Completed))
                .count(),
            failed_tasks: self
                .tasks
                .values()
                .filter(|task| matches!(task.state, TaskState::Failed))
                .count(),
            pending_messages: self.pending_message_count(),
        }
    }

    #[must_use]
    pub fn tasks(&self) -> Vec<AgentTask> {
        self.tasks.values().cloned().collect()
    }

    #[must_use]
    pub fn agents(&self) -> Vec<AgentIdentity> {
        self.agents.values().cloned().collect()
    }

    #[must_use]
    pub fn events(&self) -> &[AgentLifecycleEvent] {
        &self.events
    }

    fn pending_message_count(&self) -> usize {
        self.mailboxes.values().map(VecDeque::len).sum()
    }

    fn next_assignable_task_id(&self) -> Option<Uuid> {
        self.tasks
            .values()
            .filter(|task| matches!(task.state, TaskState::Pending))
            .map(|task| task.id)
            .next()
    }

    fn select_agent_for_task(&self, task_id: Uuid) -> Option<Uuid> {
        let task = self.tasks.get(&task_id)?;
        self.agents
            .values()
            .filter(|agent| self.agent_matches_task(agent, task))
            .min_by_key(|agent| {
                (
                    ownership_rank(&agent.ownership_paths, &task.ownership_paths),
                    self.active_task_count
                        .get(&agent.agent_id)
                        .copied()
                        .unwrap_or_default(),
                    agent.name.as_str(),
                )
            })
            .map(|agent| agent.agent_id)
    }

    fn agent_matches_task(&self, agent: &AgentIdentity, task: &AgentTask) -> bool {
        if !matches!(agent.state, AgentState::Idle | AgentState::Busy) {
            return false;
        }
        let active = self
            .active_task_count
            .get(&agent.agent_id)
            .copied()
            .unwrap_or_default();
        if active >= agent.max_concurrency {
            return false;
        }
        if !task.required_labels.is_empty()
            && !task
                .required_labels
                .iter()
                .all(|(key, value)| agent.labels.get(key) == Some(value))
        {
            return false;
        }
        if task.ownership_paths.is_empty() {
            return true;
        }
        has_ownership_match(&agent.ownership_paths, &task.ownership_paths)
    }

    fn transition_waiting(&mut self, task_id: Uuid, state: TaskState, kind: &str) -> Option<()> {
        let task = self.tasks.get_mut(&task_id)?;
        if !matches!(task.state, TaskState::Assigned | TaskState::Running) {
            return None;
        }
        let owner = task.owner;
        task.state = state;
        task.updated_at = Utc::now();
        self.updated_at = Utc::now();
        self.record_event(kind, "scheduler", Some(task_id), owner, String::new());
        Some(())
    }

    fn finish_task(
        &mut self,
        task_id: Uuid,
        state: TaskState,
        result_summary: Option<String>,
        failure_message: Option<String>,
        kind: &str,
    ) -> Option<()> {
        let task = self.tasks.get_mut(&task_id)?;
        let agent_id = task.owner;
        let title = task.title.clone();
        task.state = state;
        task.updated_at = Utc::now();
        task.result_summary = result_summary;
        task.failure_message = failure_message;
        // Clean up cancellation token when a task finishes normally.
        self.cancel_tokens
            .lock()
            .expect("cancel_tokens lock")
            .remove(&task_id);
        if let Some(agent_id) = agent_id {
            let counter = self.active_task_count.entry(agent_id).or_insert(0);
            if *counter > 0 {
                *counter -= 1;
            }
            if let Some(agent) = self.agents.get_mut(&agent_id) {
                agent.state = if *counter == 0 {
                    AgentState::Idle
                } else {
                    AgentState::Busy
                };
            }
        }
        self.updated_at = Utc::now();
        self.record_event(kind, "scheduler", Some(task_id), agent_id, title);
        Some(())
    }

    fn record_event(
        &mut self,
        kind: impl Into<String>,
        actor: impl Into<String>,
        task_id: Option<Uuid>,
        agent_id: Option<Uuid>,
        message: impl Into<String>,
    ) {
        self.events.push(AgentLifecycleEvent {
            event_id: Uuid::new_v4(),
            team_id: self.team_id,
            kind: kind.into(),
            created_at: Utc::now(),
            actor: actor.into(),
            task_id,
            agent_id,
            message: message.into(),
        });
    }

    /// Execute multiple pending tasks in parallel using tokio.
    ///
    /// Each task is spawned as an independent tokio task. The `executor` closure
    /// receives the task ID and title, and returns a `Result<String>`.
    pub async fn execute_parallel<F, Fut>(
        &self,
        task_ids: &[Uuid],
        executor: F,
    ) -> Vec<Result<String>>
    where
        F: Fn(Uuid, String) -> Fut + Clone + Send + 'static,
        Fut: std::future::Future<Output = Result<String>> + Send,
    {
        let mut handles = Vec::new();
        for &task_id in task_ids {
            if let Some(task) = self.tasks.get(&task_id)
                && task.state == TaskState::Pending
            {
                let title = task.title.clone();
                let exec = executor.clone();
                handles.push(tokio::spawn(async move { exec(task_id, title).await }));
            }
        }
        let mut results = Vec::new();
        for handle in handles {
            match handle.await {
                Ok(result) => results.push(result),
                Err(e) => results.push(Err(anyhow::anyhow!("Task panicked: {}", e))),
            }
        }
        results
    }

    /// Produce a team status report with per-agent active task counts.
    #[must_use]
    pub fn team_status(&self) -> TeamStatusReport {
        TeamStatusReport {
            team_id: self.team_id,
            lead: self.lead.clone(),
            objective: self.objective.clone(),
            agents: self
                .agents
                .iter()
                .map(|(id, agent)| AgentStatusEntry {
                    id: *id,
                    name: agent.name.clone(),
                    role: agent.role.clone(),
                    state: agent.state,
                    active_tasks: self
                        .tasks
                        .values()
                        .filter(|t| {
                            t.owner == Some(*id)
                                && matches!(
                                    t.state,
                                    TaskState::Assigned
                                        | TaskState::Running
                                        | TaskState::WaitingOnTool
                                        | TaskState::WaitingOnApproval
                                )
                        })
                        .count(),
                })
                .collect(),
            pending_tasks: self
                .tasks
                .values()
                .filter(|t| t.state == TaskState::Pending)
                .count(),
            completed_tasks: self
                .tasks
                .values()
                .filter(|t| t.state == TaskState::Completed)
                .count(),
            failed_tasks: self
                .tasks
                .values()
                .filter(|t| t.state == TaskState::Failed)
                .count(),
        }
    }
}

/// A snapshot of the team's current status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamStatusReport {
    pub team_id: Uuid,
    pub lead: String,
    pub objective: String,
    pub agents: Vec<AgentStatusEntry>,
    pub pending_tasks: usize,
    pub completed_tasks: usize,
    pub failed_tasks: usize,
}

/// Per-agent status within a team report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStatusEntry {
    pub id: Uuid,
    pub name: String,
    pub role: String,
    pub state: AgentState,
    pub active_tasks: usize,
}

fn has_ownership_match(agent_paths: &[String], task_paths: &[String]) -> bool {
    let task_paths = task_paths.iter().collect::<BTreeSet<_>>();
    agent_paths.iter().any(|agent_path| {
        task_paths.iter().any(|task_path| {
            task_path.starts_with(agent_path)
                || agent_path.starts_with(task_path.as_str())
                || task_path.contains(agent_path)
        })
    })
}

fn ownership_rank(agent_paths: &[String], task_paths: &[String]) -> usize {
    if task_paths.is_empty() {
        return usize::MAX / 2;
    }
    task_paths
        .iter()
        .flat_map(|task_path| {
            agent_paths.iter().filter_map(move |agent_path| {
                if task_path == agent_path {
                    Some(0usize)
                } else if task_path.starts_with(agent_path) {
                    Some(task_path.len().saturating_sub(agent_path.len()) + 1)
                } else if agent_path.starts_with(task_path) {
                    Some(agent_path.len().saturating_sub(task_path.len()) + 50)
                } else if task_path.contains(agent_path) {
                    Some(100)
                } else {
                    None
                }
            })
        })
        .min()
        .unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(name: &str, ownership_paths: &[&str]) -> AgentIdentity {
        let mut identity = AgentIdentity::new(name, "worker");
        identity.ownership_paths = ownership_paths
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        identity
    }

    #[test]
    fn scheduler_prefers_owned_paths() {
        let mut scheduler = AgentScheduler::new("lead", "Ship feature");
        let rust_agent = agent("rust", &["crates/"]);
        let docs_agent = agent("docs", &["docs/"]);
        let rust_agent_id = scheduler.register_agent(rust_agent);
        let _docs_agent_id = scheduler.register_agent(docs_agent);

        let mut task = AgentTask::new("Implement crate");
        task.ownership_paths = vec!["crates/rc-agents/src/lib.rs".to_owned()];
        let task_id = scheduler.add_task(task);

        let assigned = scheduler.assign_next_task();
        assert_eq!(assigned, Some((task_id, rust_agent_id)));
        let task = scheduler
            .tasks()
            .into_iter()
            .find(|task| task.id == task_id)
            .expect("task should exist");
        assert_eq!(task.owner, Some(rust_agent_id));
        assert_eq!(task.state, TaskState::Assigned);
    }

    #[test]
    fn mailbox_drains_messages_in_order() {
        let mut scheduler = AgentScheduler::new("lead", "Coordinate work");
        let agent_id = scheduler.register_agent(agent("curie", &[]));
        let first = scheduler.queue_instruction(agent_id, "lead", "Plan", "Please plan");
        let second = scheduler.queue_instruction(agent_id, "lead", "Patch", "Please patch");
        assert!(first.is_some());
        assert!(second.is_some());

        let messages = scheduler.drain_mailbox(agent_id);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].subject, "Plan");
        assert_eq!(messages[1].subject, "Patch");
        assert!(scheduler.drain_mailbox(agent_id).is_empty());
    }

    #[test]
    fn completing_task_releases_capacity() {
        let mut scheduler = AgentScheduler::new("lead", "Finish work");
        let agent_id = scheduler.register_agent(agent("laplace", &["src/"]));

        let mut task = AgentTask::new("Implement");
        task.ownership_paths = vec!["src/main.rs".to_owned()];
        task.budget.command_calls = 2;
        let task_id = scheduler.add_task(task);

        assert_eq!(scheduler.assign_next_task(), Some((task_id, agent_id)));
        assert_eq!(scheduler.start_task(task_id), Some(()));
        assert!(scheduler.consume_budget(task_id, BudgetScope::Command));
        assert!(scheduler.consume_budget(task_id, BudgetScope::Command));
        assert!(!scheduler.consume_budget(task_id, BudgetScope::Command));
        assert_eq!(scheduler.complete_task(task_id, "done"), Some(()));

        let summary = scheduler.summary();
        assert_eq!(summary.completed_tasks, 1);
        assert_eq!(summary.active_tasks, 0);
        let agent = scheduler
            .agents()
            .into_iter()
            .find(|agent| agent.agent_id == agent_id)
            .expect("agent should exist");
        assert_eq!(agent.state, AgentState::Idle);
    }

    #[test]
    fn label_requirements_filter_agents() {
        let mut scheduler = AgentScheduler::new("lead", "Mixed team");
        let mut windows_agent = agent("windows", &["apps/"]);
        windows_agent
            .labels
            .insert("os".to_owned(), "windows".to_owned());
        let mut linux_agent = agent("linux", &["apps/"]);
        linux_agent
            .labels
            .insert("os".to_owned(), "linux".to_owned());

        let windows_id = scheduler.register_agent(windows_agent);
        let _linux_id = scheduler.register_agent(linux_agent);

        let mut task = AgentTask::new("Fix PowerShell path");
        task.ownership_paths = vec!["apps/remote-code-runner/src/main.rs".to_owned()];
        task.required_labels
            .insert("os".to_owned(), "windows".to_owned());
        let task_id = scheduler.add_task(task);

        assert_eq!(scheduler.assign_next_task(), Some((task_id, windows_id)));
    }

    #[test]
    fn snapshot_and_events_reflect_scheduler_activity() {
        let mut scheduler = AgentScheduler::new("lead", "Observe state");
        let agent_id = scheduler.register_agent(agent("mendel", &["crates/"]));
        let task_id = scheduler.add_task(AgentTask::new("Write tests"));
        let _ = scheduler.assign_next_task();
        let _ = scheduler.start_task(task_id);
        let _ = scheduler.mark_waiting_on_approval(task_id);
        let _ = scheduler.fail_task(task_id, "approval denied");
        let _ = scheduler.queue_instruction(agent_id, "lead", "Cleanup", "Collect artifacts");

        let snapshot = scheduler.snapshot();
        assert_eq!(snapshot.agents.len(), 1);
        assert_eq!(snapshot.tasks.len(), 1);
        assert_eq!(snapshot.pending_messages, 1);
        assert!(scheduler.events().len() >= 5);
    }

    // ── Cancellation propagation tests ─────────────────────────────────────

    #[test]
    fn cancel_task_single_task() {
        let mut scheduler = AgentScheduler::new("lead", "Cancel test");
        let _agent_id = scheduler.register_agent(agent("worker-1", &["src/"]));

        let mut task = AgentTask::new("Task to cancel");
        task.ownership_paths = vec!["src/main.rs".to_owned()];
        let task_id = scheduler.add_task(task);
        scheduler.assign_next_task();
        scheduler.start_task(task_id);

        // Cancel via legacy method
        assert_eq!(scheduler.cancel_task(task_id, "user requested"), Some(()));
        let task = scheduler.tasks().into_iter().next().unwrap();
        assert_eq!(task.state, TaskState::Cancelled);
        assert_eq!(task.failure_message, Some("user requested".to_owned()));

        // Agent should be idle again
        let agent = scheduler.agents().into_iter().next().unwrap();
        assert_eq!(agent.state, AgentState::Idle);
    }

    #[test]
    fn cancel_task_with_propagation_cancels_children() {
        let mut scheduler = AgentScheduler::new("lead", "Propagation test");
        let agent_id = scheduler.register_agent(agent("worker-1", &[]));

        // Create parent task
        let parent_id = scheduler.add_task(AgentTask::new("Parent task"));
        scheduler.assign_task(parent_id, agent_id);
        scheduler.start_task(parent_id);

        // Spawn children under parent
        let child_a = scheduler.spawn_child_task(parent_id, "Child A").unwrap();
        let child_b = scheduler.spawn_child_task(parent_id, "Child B").unwrap();

        // Start children so they get cancellation tokens
        scheduler.assign_task(child_a, agent_id);
        scheduler.start_task(child_a);
        scheduler.assign_task(child_b, agent_id);
        scheduler.start_task(child_b);

        // Spawn a grandchild under child_a
        let grandchild = scheduler.spawn_child_task(child_a, "Grandchild").unwrap();
        scheduler.assign_task(grandchild, agent_id);
        scheduler.start_task(grandchild);

        // Cancel the parent with propagation
        let cancelled_count = scheduler
            .cancel_task_with_propagation(parent_id, "parent cancelled")
            .unwrap();

        // All 4 tasks should be cancelled
        assert_eq!(cancelled_count, 4);

        for tid in &[parent_id, child_a, child_b, grandchild] {
            let task = scheduler
                .tasks()
                .into_iter()
                .find(|t| t.id == *tid)
                .unwrap();
            assert_eq!(task.state, TaskState::Cancelled);
        }
    }

    #[test]
    fn cancel_propagation_fires_cancellation_tokens() {
        let mut scheduler = AgentScheduler::new("lead", "Token test");
        let agent_id = scheduler.register_agent(agent("worker-1", &[]));

        let parent_id = scheduler.add_task(AgentTask::new("Parent"));
        scheduler.assign_task(parent_id, agent_id);
        scheduler.start_task(parent_id);

        let child_id = scheduler.spawn_child_task(parent_id, "Child").unwrap();
        scheduler.assign_task(child_id, agent_id);
        scheduler.start_task(child_id);

        // Grab tokens before cancellation
        let parent_token = scheduler.cancellation_token(parent_id).unwrap();
        let child_token = scheduler.cancellation_token(child_id).unwrap();

        assert!(!parent_token.is_cancelled());
        assert!(!child_token.is_cancelled());

        // Cancel parent with propagation
        let count = scheduler
            .cancel_task_with_propagation(parent_id, "test")
            .unwrap();
        assert_eq!(count, 2);

        // Tokens should be cancelled
        assert!(parent_token.is_cancelled());
        assert!(child_token.is_cancelled());
    }

    #[test]
    fn cancel_propagation_no_children() {
        let mut scheduler = AgentScheduler::new("lead", "Leaf cancel");
        let agent_id = scheduler.register_agent(agent("w1", &[]));

        let task_id = scheduler.add_task(AgentTask::new("Leaf task"));
        scheduler.assign_task(task_id, agent_id);
        scheduler.start_task(task_id);

        let count = scheduler
            .cancel_task_with_propagation(task_id, "cancelled")
            .unwrap();
        assert_eq!(count, 1);

        let task = scheduler.tasks().into_iter().next().unwrap();
        assert_eq!(task.state, TaskState::Cancelled);
    }

    #[test]
    fn cancel_propagation_nonexistent_task() {
        let mut scheduler = AgentScheduler::new("lead", "Nothing");
        let result = scheduler.cancel_task_with_propagation(Uuid::new_v4(), "gone");
        assert!(result.is_none());
    }

    #[test]
    fn spawn_child_task_links_parent() {
        let mut scheduler = AgentScheduler::new("lead", "Child test");
        let agent_id = scheduler.register_agent(agent("w1", &[]));

        let mut parent = AgentTask::new("Parent task");
        parent.ownership_paths = vec!["src/".to_owned()];
        parent
            .required_labels
            .insert("lang".to_owned(), "rust".to_owned());
        let parent_id = scheduler.add_task(parent);
        scheduler.assign_task(parent_id, agent_id);

        let child_id = scheduler.spawn_child_task(parent_id, "Child task").unwrap();

        let child = scheduler
            .tasks()
            .into_iter()
            .find(|t| t.id == child_id)
            .unwrap();
        assert_eq!(child.parent_task_id, Some(parent_id));
        assert_eq!(child.owner, Some(agent_id));
        assert_eq!(child.ownership_paths, vec!["src/".to_owned()]);
        assert_eq!(
            child.required_labels.get("lang").map(|s| s.as_str()),
            Some("rust")
        );
        assert_eq!(child.state, TaskState::Pending);
    }

    #[test]
    fn spawn_child_task_nonexistent_parent() {
        let mut scheduler = AgentScheduler::new("lead", "Orphan test");
        let result = scheduler.spawn_child_task(Uuid::new_v4(), "Orphan");
        assert!(result.is_none());
    }

    #[test]
    fn child_task_ids_returns_direct_children() {
        let mut scheduler = AgentScheduler::new("lead", "Children test");
        let agent_id = scheduler.register_agent(agent("w1", &[]));

        let parent_id = scheduler.add_task(AgentTask::new("Parent"));
        scheduler.assign_task(parent_id, agent_id);

        let child_a = scheduler.spawn_child_task(parent_id, "A").unwrap();
        let child_b = scheduler.spawn_child_task(parent_id, "B").unwrap();
        let _child_c = scheduler.spawn_child_task(child_a, "Grandchild").unwrap();

        let children = scheduler.child_task_ids(parent_id);
        assert_eq!(children.len(), 2);
        assert!(children.contains(&child_a));
        assert!(children.contains(&child_b));
    }

    #[test]
    fn cancellation_token_none_for_unstarted_task() {
        let scheduler = AgentScheduler::new("lead", "Token test");
        assert!(scheduler.cancellation_token(Uuid::new_v4()).is_none());
    }

    #[test]
    fn cancel_task_creates_lifecycle_events() {
        let mut scheduler = AgentScheduler::new("lead", "Events test");
        let agent_id = scheduler.register_agent(agent("w1", &[]));

        let parent_id = scheduler.add_task(AgentTask::new("Parent"));
        scheduler.assign_task(parent_id, agent_id);
        scheduler.start_task(parent_id);

        let child_id = scheduler.spawn_child_task(parent_id, "Child").unwrap();
        scheduler.assign_task(child_id, agent_id);
        scheduler.start_task(child_id);

        let event_count_before = scheduler.events().len();

        scheduler
            .cancel_task_with_propagation(parent_id, "testing")
            .unwrap();

        // Should have events for: child_task_spawned, cancel (parent), cancel (child)
        let events = scheduler.events();
        let new_events = &events[event_count_before..];

        // At least the two cancel events plus the spawn event
        let cancel_events: Vec<_> = new_events
            .iter()
            .filter(|e| e.kind == "task_cancelled")
            .collect();
        assert_eq!(cancel_events.len(), 2);
    }

    #[test]
    fn agent_task_new_has_no_parent() {
        let task = AgentTask::new("Standalone");
        assert_eq!(task.parent_task_id, None);
        assert_eq!(task.state, TaskState::Pending);
    }
}
