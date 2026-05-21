//! State management for the application runtime.
//!
//! This module provides thread-safe state management with:
//!
//! - [`StateSnapshot`] — immutable point-in-time state snapshot
//! - [`StateUpdate`] — incremental update operations
//! - [`AppStateManager`] — thread-safe state manager with versioning
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────────────┐
//! │  AppStateManager (Arc<RwLock<AppStateInner>>)    │
//! │  ┌────────────────────────────────────────────┐  │
//! │  │  AppState + version + timestamp            │  │
//! │  └────────────────────────────────────────────┘  │
//! │  - snapshot() → StateSnapshot                    │
//! │  - apply(StateUpdate) → Result<()>               │
//! │  - batch_apply(Vec<StateUpdate>) → Result<()>    │
//! └──────────────────────────────────────────────────┘
//! ```

use parking_lot::RwLock;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{AgentId, Message, PermissionMode, SessionId, state::AppState};

// ---------------------------------------------------------------------------
// StateSnapshot — immutable point-in-time snapshot
// ---------------------------------------------------------------------------

/// An immutable snapshot of the application state at a specific version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateSnapshot {
    /// The captured state.
    pub state: AppState,
    /// Monotonically increasing version number.
    pub version: u64,
    /// When this snapshot was taken.
    pub timestamp: DateTime<Utc>,
    /// Snapshot identifier for tracing.
    pub snapshot_id: String,
}

impl StateSnapshot {
    /// Create a new snapshot from the given state.
    #[must_use]
    pub fn new(state: AppState, version: u64) -> Self {
        Self {
            state,
            version,
            timestamp: Utc::now(),
            snapshot_id: Uuid::new_v4().to_string(),
        }
    }

    /// Check if this snapshot is newer than another.
    #[must_use]
    pub fn is_newer_than(&self, other: &Self) -> bool {
        self.version > other.version
    }

    /// Return the number of messages in the snapshot.
    #[must_use]
    pub fn message_count(&self) -> usize {
        self.state.messages.len()
    }

    /// Check if the snapshot has an active session.
    #[must_use]
    pub fn has_session(&self) -> bool {
        self.state.session_id.is_some()
    }
}

// ---------------------------------------------------------------------------
// StateUpdate — incremental update operations
// ---------------------------------------------------------------------------

/// An incremental update to apply to the application state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum StateUpdate {
    /// Set or clear the active session.
    SessionChanged {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<SessionId>,
    },
    /// Set or clear the active agent.
    AgentChanged {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<AgentId>,
    },
    /// Change the permission mode.
    PermissionModeChanged { mode: PermissionMode },
    /// Push a new message.
    MessagePushed { message: Message },
    /// Replace all messages (e.g., after compaction).
    MessagesReplaced { messages: Vec<Message> },
    /// Record a newly discovered skill.
    SkillDiscovered { skill: String },
    /// Activate a tool.
    ToolActivated { tool: String },
    /// Deactivate a tool.
    ToolDeactivated { tool: String },
    /// Change the active model.
    ModelChanged {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    /// Update the queued task count.
    QueueUpdated { count: usize },
    /// Reset the entire state to defaults.
    Reset,
}

impl StateUpdate {
    /// Return a human-readable label for the update type.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::SessionChanged { .. } => "session_changed",
            Self::AgentChanged { .. } => "agent_changed",
            Self::PermissionModeChanged { .. } => "permission_mode_changed",
            Self::MessagePushed { .. } => "message_pushed",
            Self::MessagesReplaced { .. } => "messages_replaced",
            Self::SkillDiscovered { .. } => "skill_discovered",
            Self::ToolActivated { .. } => "tool_activated",
            Self::ToolDeactivated { .. } => "tool_deactivated",
            Self::ModelChanged { .. } => "model_changed",
            Self::QueueUpdated { .. } => "queue_updated",
            Self::Reset => "reset",
        }
    }
}

// ---------------------------------------------------------------------------
// AppStateInner — internal mutable state container
// ---------------------------------------------------------------------------

/// Internal state container with versioning.
#[derive(Debug)]
struct AppStateInner {
    state: AppState,
    version: u64,
    last_updated: DateTime<Utc>,
}

impl AppStateInner {
    fn new() -> Self {
        Self {
            state: AppState::default(),
            version: 0,
            last_updated: Utc::now(),
        }
    }

    fn apply(&mut self, update: StateUpdate) {
        match update {
            StateUpdate::SessionChanged { session_id } => {
                self.state.session_id = session_id;
            }
            StateUpdate::AgentChanged { agent_id } => {
                self.state.active_agent_id = agent_id;
            }
            StateUpdate::PermissionModeChanged { mode } => {
                self.state.permission_mode = mode;
            }
            StateUpdate::MessagePushed { message } => {
                self.state.push_message(message);
            }
            StateUpdate::MessagesReplaced { messages } => {
                self.state.messages = messages;
            }
            StateUpdate::SkillDiscovered { skill } => {
                self.state.note_skill(skill);
            }
            StateUpdate::ToolActivated { tool } => {
                self.state.note_tool(tool);
            }
            StateUpdate::ToolDeactivated { tool } => {
                self.state.active_tools.remove(&tool);
            }
            StateUpdate::ModelChanged { model } => {
                self.state.model = model;
            }
            StateUpdate::QueueUpdated { count } => {
                self.state.queued_task_count = count;
            }
            StateUpdate::Reset => {
                self.state = AppState::default();
            }
        }
        self.version = self.version.saturating_add(1);
        self.last_updated = Utc::now();
    }
}

// ---------------------------------------------------------------------------
// AppStateManager — thread-safe state manager
// ---------------------------------------------------------------------------

/// Thread-safe application state manager.
///
/// Provides atomic read/write access to the global application state
/// through versioned snapshots and incremental updates.
///
/// # Thread Safety
///
/// Uses `std::sync::RwLock` internally, allowing concurrent reads
/// but exclusive writes. This is suitable for the typical read-heavy
/// pattern of UI state access.
pub struct AppStateManager {
    inner: RwLock<AppStateInner>,
}

impl AppStateManager {
    /// Create a new state manager with default state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(AppStateInner::new()),
        }
    }

    /// Create a state manager initialized with the given state.
    #[must_use]
    pub fn with_state(state: AppState) -> Self {
        let inner = AppStateInner {
            state,
            version: 1,
            last_updated: Utc::now(),
        };
        Self {
            inner: RwLock::new(inner),
        }
    }

    /// Take a snapshot of the current state.
    ///
    /// Returns an immutable snapshot that can be held without locking
    /// the manager.
    pub fn snapshot(&self) -> StateSnapshot {
        let guard = self.inner.read();
        StateSnapshot::new(guard.state.clone(), guard.version)
    }

    /// Apply a single state update.
    ///
    /// Increments the version counter and updates the timestamp.
    pub fn apply(&self, update: StateUpdate) {
        let mut guard = self.inner.write();
        guard.apply(update);
    }

    /// Apply multiple state updates atomically.
    ///
    /// All updates are applied under a single write lock, ensuring
    /// no reader sees an intermediate state.
    pub fn batch_apply(&self, updates: Vec<StateUpdate>) {
        let mut guard = self.inner.write();
        for update in updates {
            guard.apply(update);
        }
    }

    /// Return the current version number.
    pub fn version(&self) -> u64 {
        let guard = self.inner.read();
        guard.version
    }

    /// Return the last update timestamp.
    pub fn last_updated(&self) -> DateTime<Utc> {
        let guard = self.inner.read();
        guard.last_updated
    }

    /// Check if the manager has an active session.
    pub fn has_session(&self) -> bool {
        let guard = self.inner.read();
        guard.state.session_id.is_some()
    }

    /// Return the current message count.
    pub fn message_count(&self) -> usize {
        let guard = self.inner.read();
        guard.state.messages.len()
    }

    /// Reset the state to defaults.
    pub fn reset(&self) {
        self.apply(StateUpdate::Reset);
    }
}

impl Default for AppStateManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Thread-safe shared handle
// ---------------------------------------------------------------------------

/// A shared, thread-safe handle to an [`AppStateManager`].
pub type SharedStateManager = Arc<AppStateManager>;

/// Extension trait for creating shared state managers.
pub trait StateManagerExt {
    /// Create a shared state manager.
    fn shared() -> SharedStateManager;
    /// Create a shared state manager with initial state.
    fn shared_with_state(state: AppState) -> SharedStateManager;
}

impl StateManagerExt for AppStateManager {
    fn shared() -> SharedStateManager {
        Arc::new(Self::new())
    }

    fn shared_with_state(state: AppState) -> SharedStateManager {
        Arc::new(Self::with_state(state))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConversationEntry, PermissionMode};

    #[test]
    fn state_snapshot_creation() {
        let state = AppState::default();
        let snapshot = StateSnapshot::new(state, 1);
        assert_eq!(snapshot.version, 1);
        assert!(!snapshot.snapshot_id.is_empty());
    }

    #[test]
    fn state_snapshot_version_comparison() {
        let snap_v1 = StateSnapshot::new(AppState::default(), 1);
        let snap_v2 = StateSnapshot::new(AppState::default(), 2);
        assert!(snap_v2.is_newer_than(&snap_v1));
        assert!(!snap_v1.is_newer_than(&snap_v2));
    }

    #[test]
    fn state_snapshot_message_count() {
        let mut state = AppState::default();
        state.push_message(Message::from(ConversationEntry::user("hello")));
        state.push_message(Message::from(ConversationEntry::user("world")));
        let snapshot = StateSnapshot::new(state, 1);
        assert_eq!(snapshot.message_count(), 2);
    }

    #[test]
    fn state_snapshot_has_session() {
        let mut state = AppState::default();
        assert!(!StateSnapshot::new(state.clone(), 1).has_session());

        state.session_id = Some(SessionId::new());
        assert!(StateSnapshot::new(state, 1).has_session());
    }

    #[test]
    fn state_update_labels() {
        assert_eq!(StateUpdate::Reset.label(), "reset");
        assert_eq!(
            StateUpdate::PermissionModeChanged {
                mode: PermissionMode::Default,
            }
            .label(),
            "permission_mode_changed"
        );
        assert_eq!(
            StateUpdate::MessagePushed {
                message: Message::from(ConversationEntry::user("hi")),
            }
            .label(),
            "message_pushed"
        );
    }

    #[test]
    fn state_update_serialization() {
        let updates = vec![
            StateUpdate::SessionChanged {
                session_id: Some(SessionId::new()),
            },
            StateUpdate::PermissionModeChanged {
                mode: PermissionMode::BypassPermissions,
            },
            StateUpdate::SkillDiscovered {
                skill: "test-skill".to_owned(),
            },
            StateUpdate::ToolActivated {
                tool: "bash".to_owned(),
            },
            StateUpdate::QueueUpdated { count: 5 },
            StateUpdate::Reset,
        ];
        for update in updates {
            let json = serde_json::to_string(&update).expect("serialize should succeed");
            let parsed: StateUpdate =
                serde_json::from_str(&json).expect("deserialize should succeed");
            assert_eq!(json, serde_json::to_string(&parsed).expect("re-serialize"));
        }
    }

    #[test]
    fn app_state_manager_new() {
        let manager = AppStateManager::new();
        assert_eq!(manager.version(), 0);
        assert_eq!(manager.message_count(), 0);
        assert!(!manager.has_session());
    }

    #[test]
    fn app_state_manager_apply_session() {
        let manager = AppStateManager::new();
        let session_id = SessionId::new();
        manager.apply(StateUpdate::SessionChanged {
            session_id: Some(session_id),
        });
        assert!(manager.has_session());
        assert_eq!(manager.version(), 1);
    }

    #[test]
    fn app_state_manager_apply_message() {
        let manager = AppStateManager::new();
        manager.apply(StateUpdate::MessagePushed {
            message: Message::from(ConversationEntry::user("hello")),
        });
        assert_eq!(manager.message_count(), 1);

        manager.apply(StateUpdate::MessagePushed {
            message: Message::from(ConversationEntry::user("world")),
        });
        assert_eq!(manager.message_count(), 2);
    }

    #[test]
    fn app_state_manager_apply_model() {
        let manager = AppStateManager::new();
        manager.apply(StateUpdate::ModelChanged {
            model: Some("gpt-4".to_owned()),
        });
        let snapshot = manager.snapshot();
        assert_eq!(snapshot.state.model.as_deref(), Some("gpt-4"));
    }

    #[test]
    fn app_state_manager_batch_apply() {
        let manager = AppStateManager::new();
        let updates = vec![
            StateUpdate::SessionChanged {
                session_id: Some(SessionId::new()),
            },
            StateUpdate::ModelChanged {
                model: Some("glm-5".to_owned()),
            },
            StateUpdate::MessagePushed {
                message: Message::from(ConversationEntry::user("hello")),
            },
        ];
        manager.batch_apply(updates);
        assert_eq!(manager.version(), 3);
        assert!(manager.has_session());
        assert_eq!(manager.message_count(), 1);
    }

    #[test]
    fn app_state_manager_version_increments() {
        let manager = AppStateManager::new();
        assert_eq!(manager.version(), 0);

        manager.apply(StateUpdate::QueueUpdated { count: 1 });
        assert_eq!(manager.version(), 1);

        manager.apply(StateUpdate::QueueUpdated { count: 2 });
        assert_eq!(manager.version(), 2);
    }

    #[test]
    fn app_state_manager_reset() {
        let manager = AppStateManager::new();
        manager.apply(StateUpdate::SessionChanged {
            session_id: Some(SessionId::new()),
        });
        manager.apply(StateUpdate::MessagePushed {
            message: Message::from(ConversationEntry::user("hello")),
        });
        assert!(manager.has_session());
        assert_eq!(manager.message_count(), 1);

        manager.reset();
        assert!(!manager.has_session());
        assert_eq!(manager.message_count(), 0);
    }

    #[test]
    fn app_state_manager_tool_activation() {
        let manager = AppStateManager::new();
        manager.apply(StateUpdate::ToolActivated {
            tool: "bash".to_owned(),
        });
        let snapshot = manager.snapshot();
        assert!(snapshot.state.active_tools.contains("bash"));

        manager.apply(StateUpdate::ToolDeactivated {
            tool: "bash".to_owned(),
        });
        let snapshot = manager.snapshot();
        assert!(!snapshot.state.active_tools.contains("bash"));
    }

    #[test]
    fn state_snapshot_serialization() {
        let mut state = AppState::default();
        state.push_message(Message::from(ConversationEntry::user("hello")));
        let snapshot = StateSnapshot::new(state, 42);
        let json = serde_json::to_string(&snapshot).expect("serialize should succeed");
        let parsed: StateSnapshot =
            serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(parsed.version, 42);
        assert_eq!(parsed.message_count(), 1);
    }

    #[test]
    fn shared_state_manager() {
        let shared = AppStateManager::shared();
        shared.apply(StateUpdate::QueueUpdated { count: 10 });

        // Clone the Arc and verify both references see the same state
        let cloned = Arc::clone(&shared);
        assert_eq!(cloned.version(), 1);
        let snapshot = cloned.snapshot();
        assert_eq!(snapshot.state.queued_task_count, 10);
    }

    #[test]
    fn state_manager_concurrent_reads() {
        let manager = AppStateManager::new();
        manager.apply(StateUpdate::ModelChanged {
            model: Some("test-model".to_owned()),
        });

        // Multiple snapshots should all see the same state
        let snap1 = manager.snapshot();
        let snap2 = manager.snapshot();
        assert_eq!(snap1.version, snap2.version);
        assert_eq!(snap1.state.model, snap2.state.model);
    }
}
