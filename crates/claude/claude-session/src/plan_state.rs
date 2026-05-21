use std::path::PathBuf;

use chrono::{DateTime, Utc};
use claude_core::PermissionMode;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Persisted plan-mode state reconstructed across resume / re-entry flows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanModeState {
    pub updated_at: DateTime<Utc>,
    pub current_permission_mode: PermissionMode,
    pub pre_plan_permission_mode: Option<PermissionMode>,
    pub has_exited_plan_mode: bool,
    pub needs_plan_mode_exit_attachment: bool,
    pub plan_id: Option<String>,
    pub plan_slug: Option<String>,
    pub plan_objective: Option<String>,
    pub plan_file_path: Option<PathBuf>,
    pub parent_session_id: Option<Uuid>,
}

impl Default for PlanModeState {
    fn default() -> Self {
        Self {
            updated_at: Utc::now(),
            current_permission_mode: PermissionMode::Default,
            pre_plan_permission_mode: None,
            has_exited_plan_mode: false,
            needs_plan_mode_exit_attachment: false,
            plan_id: None,
            plan_slug: None,
            plan_objective: None,
            plan_file_path: None,
            parent_session_id: None,
        }
    }
}

impl PlanModeState {
    #[must_use]
    pub fn is_plan_mode(&self) -> bool {
        self.current_permission_mode == PermissionMode::Plan
    }

    #[must_use]
    pub fn touch(mut self) -> Self {
        self.updated_at = Utc::now();
        self
    }
}
