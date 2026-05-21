//! Core types for the swarm system.
//!
//! Defines [`TeamFile`], [`TeamMember`], [`TeammateIdentity`],
//! [`SwarmPermissionRequest`], [`MailboxMessage`], and related types.

use chrono::Utc;
use claude_core::PermissionMode;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Path that a team is allowed to access.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamAllowedPath {
    /// Filesystem path.
    pub path: String,
    /// Whether the path is read-only.
    #[serde(default)]
    pub read_only: bool,
}

/// Persistent team data stored in `config.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TeamFile {
    /// Team name (unique identifier).
    pub name: String,
    /// Optional human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Unix timestamp (seconds) when the team was created.
    pub created_at: i64,
    /// Agent ID of the team lead.
    pub lead_agent_id: String,
    /// Session ID of the lead agent (if running).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_session_id: Option<String>,
    /// Team members (excluding the lead).
    #[serde(default)]
    pub members: Vec<TeamMember>,
    /// Pane IDs that are hidden from the user.
    #[serde(default)]
    pub hidden_pane_ids: Vec<String>,
    /// Paths that the team is allowed to access.
    #[serde(default)]
    pub team_allowed_paths: Vec<TeamAllowedPath>,
}

impl TeamFile {
    /// Create a new team file with the given name and lead agent ID.
    #[must_use]
    pub fn new(name: impl Into<String>, lead_agent_id: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            created_at: Utc::now().timestamp(),
            lead_agent_id: lead_agent_id.into(),
            lead_session_id: None,
            members: Vec::new(),
            hidden_pane_ids: Vec::new(),
            team_allowed_paths: Vec::new(),
        }
    }

    /// Find a member by agent name.
    #[must_use]
    pub fn find_member(&self, agent_name: &str) -> Option<&TeamMember> {
        self.members.iter().find(|m| m.name == agent_name)
    }

    /// Find a member by agent name (mutable).
    pub fn find_member_mut(&mut self, agent_name: &str) -> Option<&mut TeamMember> {
        self.members.iter_mut().find(|m| m.name == agent_name)
    }

    /// Check if a member with the given name exists.
    #[must_use]
    pub fn has_member(&self, agent_name: &str) -> bool {
        self.members.iter().any(|m| m.name == agent_name)
    }

    /// Count active members.
    #[must_use]
    pub fn active_member_count(&self) -> usize {
        self.members
            .iter()
            .filter(|m| m.is_active.unwrap_or(false))
            .count()
    }

    /// Remove a member by agent name.
    pub fn remove_member(&mut self, agent_name: &str) -> Option<TeamMember> {
        let idx = self.members.iter().position(|m| m.name == agent_name)?;
        Some(self.members.remove(idx))
    }
}

/// A member of a team (worker agent).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TeamMember {
    /// Unique agent identifier.
    pub agent_id: String,
    /// Human-readable agent name.
    pub name: String,
    /// Agent type (e.g., "worker", "reviewer").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    /// Model identifier the agent uses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Terminal color assigned to this member.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Unix timestamp (seconds) when the member joined.
    pub joined_at: i64,
    /// Pane identifier in the terminal backend.
    pub pane_id: String,
    /// Current working directory.
    pub cwd: String,
    /// Path to the member's worktree (if using git worktrees).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    /// Session ID of the member (if running).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Backend type used by this member.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_type: Option<BackendType>,
    /// Whether the member is currently active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_active: Option<bool>,
    /// Permission mode for this member.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<PermissionMode>,
}

impl TeamMember {
    /// Create a new team member.
    #[must_use]
    pub fn new(
        agent_id: impl Into<String>,
        name: impl Into<String>,
        pane_id: impl Into<String>,
        cwd: impl Into<String>,
    ) -> Self {
        Self {
            agent_id: agent_id.into(),
            name: name.into(),
            agent_type: None,
            model: None,
            color: None,
            joined_at: Utc::now().timestamp(),
            pane_id: pane_id.into(),
            cwd: cwd.into(),
            worktree_path: None,
            session_id: None,
            backend_type: None,
            is_active: Some(true),
            mode: None,
        }
    }
}

/// Identity information for a teammate during initialization.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeammateIdentity {
    /// Unique agent ID.
    pub agent_id: String,
    /// Human-readable name.
    pub name: String,
    /// Team name.
    pub team_name: String,
    /// Whether this agent is the team lead.
    pub is_lead: bool,
    /// Lead agent ID.
    pub lead_agent_id: String,
    /// Backend type.
    pub backend_type: BackendType,
}

/// Terminal backend type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum BackendType {
    /// In-process backend (same process, no terminal splitting).
    InProcess,
    /// Tmux backend (terminal multiplexer).
    Tmux,
    /// iTerm2 backend (macOS terminal).
    ITerm2,
}

impl BackendType {
    /// Return the string identifier for this backend type.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InProcess => "in_process",
            Self::Tmux => "tmux",
            Self::ITerm2 => "iterm2",
        }
    }

    /// Parse a backend type from a string.
    #[must_use]
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "in_process" | "in-process" | "inprocess" => Some(Self::InProcess),
            "tmux" => Some(Self::Tmux),
            "iterm2" | "iterm" | "iTerm2" | "iTerm" => Some(Self::ITerm2),
            _ => None,
        }
    }
}

impl std::fmt::Display for BackendType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Permission decision for a swarm permission request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecision {
    /// Allow the tool execution.
    Allow,
    /// Deny the tool execution.
    Deny,
}

/// A permission request from a worker to the team lead.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SwarmPermissionRequest {
    /// Unique request identifier.
    pub request_id: String,
    /// Team name.
    pub team_name: String,
    /// Name of the agent making the request.
    pub agent_name: String,
    /// Name of the tool being requested.
    pub tool_name: String,
    /// Tool input parameters.
    pub tool_input: serde_json::Value,
    /// Decision (if resolved).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<PermissionDecision>,
    /// Reason for the decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Unix timestamp (seconds) when the request was created.
    pub created_at: i64,
    /// Unix timestamp (seconds) when the request was resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<i64>,
}

impl SwarmPermissionRequest {
    /// Create a new permission request.
    #[must_use]
    pub fn new(
        team_name: impl Into<String>,
        agent_name: impl Into<String>,
        tool_name: impl Into<String>,
        tool_input: serde_json::Value,
    ) -> Self {
        Self {
            request_id: Uuid::new_v4().to_string(),
            team_name: team_name.into(),
            agent_name: agent_name.into(),
            tool_name: tool_name.into(),
            tool_input,
            decision: None,
            reason: None,
            created_at: Utc::now().timestamp(),
            resolved_at: None,
        }
    }

    /// Check if the request has been resolved.
    #[must_use]
    pub fn is_resolved(&self) -> bool {
        self.decision.is_some()
    }

    /// Resolve the request with a decision.
    pub fn resolve(&mut self, decision: PermissionDecision, reason: Option<String>) {
        self.decision = Some(decision);
        self.reason = reason;
        self.resolved_at = Some(Utc::now().timestamp());
    }
}

/// Type of mailbox message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailboxMessageType {
    /// A text message.
    Text,
    /// A task assignment.
    TaskAssignment,
    /// A task result.
    TaskResult,
    /// A status update.
    StatusUpdate,
    /// A coordination message.
    Coordination,
}

/// A message in the agent mailbox.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MailboxMessage {
    /// Unique message identifier.
    pub id: String,
    /// Name of the sending agent.
    pub from_agent: String,
    /// Name of the receiving agent.
    pub to_agent: String,
    /// Message type.
    pub message_type: MailboxMessageType,
    /// Message content.
    pub content: String,
    /// Optional short preview summary for UI rendering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Optional message priority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    /// Optional correlation identifier for request/response flows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    /// Unix timestamp (seconds) when the message was created.
    pub timestamp: i64,
    /// Whether the message has been read.
    #[serde(default)]
    pub read: bool,
}

impl MailboxMessage {
    /// Create a new mailbox message.
    #[must_use]
    pub fn new(
        from_agent: impl Into<String>,
        to_agent: impl Into<String>,
        message_type: MailboxMessageType,
        content: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            from_agent: from_agent.into(),
            to_agent: to_agent.into(),
            message_type,
            content: content.into(),
            summary: None,
            priority: None,
            correlation_id: None,
            timestamp: Utc::now().timestamp(),
            read: false,
        }
    }

    /// Mark the message as read.
    pub fn mark_read(&mut self) {
        self.read = true;
    }
}

/// Teammate lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeammateState {
    /// Teammate is being initialized.
    Init,
    /// Teammate is reconnecting to an existing session.
    Reconnecting,
    /// Teammate is being laid out in the terminal.
    Layout,
    /// Teammate is being spawned.
    Spawning,
    /// Teammate is running.
    Running,
    /// Teammate has stopped.
    Stopped,
    /// Teammate encountered an error.
    Error,
}

/// Spawn configuration for a new teammate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpawnConfig {
    /// Agent ID for the new teammate.
    pub agent_id: String,
    /// Human-readable name.
    pub agent_name: String,
    /// Team name.
    pub team_name: String,
    /// Model to use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Working directory.
    pub cwd: String,
    /// Backend type to use.
    pub backend_type: BackendType,
    /// Environment variables to set.
    #[serde(default)]
    pub env_vars: Vec<(String, String)>,
    /// Permission mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
    /// Optional worktree path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_file_new() {
        let tf = TeamFile::new("test-team", "lead-123");
        assert_eq!(tf.name, "test-team");
        assert_eq!(tf.lead_agent_id, "lead-123");
        assert!(tf.members.is_empty());
        assert!(tf.description.is_none());
    }

    #[test]
    fn team_file_add_and_find_member() {
        let mut tf = TeamFile::new("test-team", "lead-123");
        let member = TeamMember::new("agent-1", "worker-1", "pane-1", "/tmp/work");
        tf.members.push(member);
        assert!(tf.has_member("worker-1"));
        assert!(!tf.has_member("worker-2"));
        let found = tf.find_member("worker-1").expect("should find member");
        assert_eq!(found.agent_id, "agent-1");
    }

    #[test]
    fn team_file_remove_member() {
        let mut tf = TeamFile::new("test-team", "lead-123");
        tf.members.push(TeamMember::new("a1", "w1", "p1", "/tmp"));
        tf.members.push(TeamMember::new("a2", "w2", "p2", "/tmp"));
        let removed = tf.remove_member("w1").expect("should remove");
        assert_eq!(removed.agent_id, "a1");
        assert_eq!(tf.members.len(), 1);
        assert!(tf.remove_member("nonexistent").is_none());
    }

    #[test]
    fn team_file_find_member_mut() {
        let mut tf = TeamFile::new("test-team", "lead-123");
        tf.members.push(TeamMember::new("a1", "w1", "p1", "/tmp"));
        let m = tf.find_member_mut("w1").expect("should find");
        m.color = Some("red".to_owned());
        assert_eq!(
            tf.find_member("w1").expect("found").color.as_deref(),
            Some("red")
        );
    }

    #[test]
    fn team_file_active_member_count() {
        let mut tf = TeamFile::new("test-team", "lead-123");
        let mut m1 = TeamMember::new("a1", "w1", "p1", "/tmp");
        m1.is_active = Some(true);
        let mut m2 = TeamMember::new("a2", "w2", "p2", "/tmp");
        m2.is_active = Some(false);
        tf.members.push(m1);
        tf.members.push(m2);
        assert_eq!(tf.active_member_count(), 1);
    }

    #[test]
    fn team_member_new() {
        let m = TeamMember::new("id-1", "worker-1", "pane-1", "/home/user/project");
        assert_eq!(m.agent_id, "id-1");
        assert_eq!(m.name, "worker-1");
        assert_eq!(m.pane_id, "pane-1");
        assert_eq!(m.cwd, "/home/user/project");
        assert_eq!(m.is_active, Some(true));
        assert!(m.model.is_none());
        assert!(m.color.is_none());
    }

    #[test]
    fn backend_type_roundtrip() {
        assert_eq!(
            BackendType::from_str_opt("in_process"),
            Some(BackendType::InProcess)
        );
        assert_eq!(BackendType::from_str_opt("tmux"), Some(BackendType::Tmux));
        assert_eq!(
            BackendType::from_str_opt("iterm2"),
            Some(BackendType::ITerm2)
        );
        assert_eq!(BackendType::from_str_opt("unknown"), None);
    }

    #[test]
    fn backend_type_display() {
        assert_eq!(BackendType::InProcess.to_string(), "in_process");
        assert_eq!(BackendType::Tmux.to_string(), "tmux");
        assert_eq!(BackendType::ITerm2.to_string(), "iterm2");
    }

    #[test]
    fn backend_type_as_str() {
        assert_eq!(BackendType::InProcess.as_str(), "in_process");
        assert_eq!(BackendType::Tmux.as_str(), "tmux");
        assert_eq!(BackendType::ITerm2.as_str(), "iterm2");
    }

    #[test]
    fn permission_request_new() {
        let req = SwarmPermissionRequest::new(
            "team-1",
            "worker-1",
            "bash",
            serde_json::json!({"command": "ls"}),
        );
        assert_eq!(req.team_name, "team-1");
        assert_eq!(req.agent_name, "worker-1");
        assert_eq!(req.tool_name, "bash");
        assert!(!req.is_resolved());
        assert!(req.decision.is_none());
    }

    #[test]
    fn permission_request_resolve() {
        let mut req = SwarmPermissionRequest::new(
            "team-1",
            "worker-1",
            "bash",
            serde_json::json!({"command": "ls"}),
        );
        req.resolve(PermissionDecision::Allow, None);
        assert!(req.is_resolved());
        assert_eq!(req.decision, Some(PermissionDecision::Allow));
        assert!(req.resolved_at.is_some());
    }

    #[test]
    fn permission_request_resolve_with_reason() {
        let mut req = SwarmPermissionRequest::new(
            "team-1",
            "worker-1",
            "bash",
            serde_json::json!({"command": "rm -rf /"}),
        );
        req.resolve(
            PermissionDecision::Deny,
            Some("dangerous command".to_owned()),
        );
        assert_eq!(req.decision, Some(PermissionDecision::Deny));
        assert_eq!(req.reason.as_deref(), Some("dangerous command"));
    }

    #[test]
    fn mailbox_message_new() {
        let msg = MailboxMessage::new(
            "lead",
            "worker-1",
            MailboxMessageType::TaskAssignment,
            "Fix the bug in module X",
        );
        assert_eq!(msg.from_agent, "lead");
        assert_eq!(msg.to_agent, "worker-1");
        assert!(!msg.read);
        assert_eq!(msg.message_type, MailboxMessageType::TaskAssignment);
    }

    #[test]
    fn mailbox_message_mark_read() {
        let mut msg = MailboxMessage::new("a", "b", MailboxMessageType::Text, "hello");
        assert!(!msg.read);
        msg.mark_read();
        assert!(msg.read);
    }

    #[test]
    fn team_allowed_path() {
        let p = TeamAllowedPath {
            path: "/home/user/project".to_owned(),
            read_only: true,
        };
        assert_eq!(p.path, "/home/user/project");
        assert!(p.read_only);
    }

    #[test]
    fn teammate_identity() {
        let id = TeammateIdentity {
            agent_id: "a1".to_owned(),
            name: "worker-1".to_owned(),
            team_name: "team-1".to_owned(),
            is_lead: false,
            lead_agent_id: "lead-1".to_owned(),
            backend_type: BackendType::InProcess,
        };
        assert!(!id.is_lead);
        assert_eq!(id.backend_type, BackendType::InProcess);
    }

    #[test]
    fn teammate_state_values() {
        assert_ne!(TeammateState::Init, TeammateState::Running);
        assert_ne!(TeammateState::Spawning, TeammateState::Stopped);
        assert_ne!(TeammateState::Error, TeammateState::Reconnecting);
    }

    #[test]
    fn spawn_config() {
        let config = SpawnConfig {
            agent_id: "a1".to_owned(),
            agent_name: "worker-1".to_owned(),
            team_name: "team-1".to_owned(),
            model: Some("gpt-4".to_owned()),
            cwd: "/tmp".to_owned(),
            backend_type: BackendType::Tmux,
            env_vars: vec![("KEY".to_owned(), "VALUE".to_owned())],
            permission_mode: Some(PermissionMode::Default),
            worktree_path: None,
        };
        assert_eq!(config.agent_name, "worker-1");
        assert_eq!(config.env_vars.len(), 1);
    }

    #[test]
    fn team_file_serialization_roundtrip() {
        let tf = TeamFile::new("test-team", "lead-123");
        let json = serde_json::to_string(&tf).expect("should serialize");
        let tf2: TeamFile = serde_json::from_str(&json).expect("should deserialize");
        assert_eq!(tf, tf2);
    }

    #[test]
    fn permission_request_serialization_roundtrip() {
        let req = SwarmPermissionRequest::new(
            "team-1",
            "worker-1",
            "bash",
            serde_json::json!({"command": "ls"}),
        );
        let json = serde_json::to_string(&req).expect("should serialize");
        let req2: SwarmPermissionRequest = serde_json::from_str(&json).expect("should deserialize");
        assert_eq!(req, req2);
    }

    #[test]
    fn mailbox_message_serialization_roundtrip() {
        let msg = MailboxMessage::new("a", "b", MailboxMessageType::Text, "hello");
        let json = serde_json::to_string(&msg).expect("should serialize");
        let msg2: MailboxMessage = serde_json::from_str(&json).expect("should deserialize");
        assert_eq!(msg, msg2);
    }

    #[test]
    fn backend_type_from_str_opt_aliases() {
        assert_eq!(
            BackendType::from_str_opt("in-process"),
            Some(BackendType::InProcess)
        );
        assert_eq!(
            BackendType::from_str_opt("inprocess"),
            Some(BackendType::InProcess)
        );
        assert_eq!(
            BackendType::from_str_opt("iterm"),
            Some(BackendType::ITerm2)
        );
        assert_eq!(
            BackendType::from_str_opt("iTerm2"),
            Some(BackendType::ITerm2)
        );
    }

    #[test]
    fn mailbox_message_type_serialization() {
        let mt = MailboxMessageType::TaskAssignment;
        let json = serde_json::to_string(&mt).expect("should serialize");
        assert!(json.contains("task_assignment"));
    }
}
