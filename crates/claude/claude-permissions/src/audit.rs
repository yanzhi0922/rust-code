use serde::{Deserialize, Serialize};

use crate::rules::{RuleAction, RuleSource};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionAuditRecord {
    pub tool_name: String,
    pub tool_use_id: String,
    pub source: Option<RuleSource>,
    pub matched_pattern: Option<String>,
    pub action: RuleAction,
    pub final_allowed: bool,
    pub reason: Option<String>,
}
