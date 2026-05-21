//! Permission decision types — Allow, Ask, Deny, Passthrough.
//!
//! Corresponds to `src/types/permissions.ts` (PermissionDecision, PermissionResult,
//! PermissionAllowDecision, PermissionAskDecision, PermissionDenyDecision).

use claude_core::permission_types::PermissionBehavior;
use serde::{Deserialize, Serialize};

use crate::mode::ExtendedPermissionMode;
use crate::rule::PermissionRuleV2;

/// Reason why a permission decision was made.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum DecisionReason {
    /// Decision based on a specific rule.
    Rule { rule: PermissionRuleV2 },
    /// Decision based on the current permission mode.
    Mode { mode: ExtendedPermissionMode },
    /// Decision based on subcommand results (e.g., compound bash commands).
    SubcommandResults,
    /// Decision from a permission prompt tool.
    PermissionPromptTool {
        tool_name: String,
        result: serde_json::Value,
    },
    /// Decision from a hook.
    Hook {
        hook_name: String,
        hook_source: Option<String>,
        reason: Option<String>,
    },
    /// Decision from an async agent.
    AsyncAgent { reason: String },
    /// Decision due to sandbox override (excluded command or disabled sandbox).
    SandboxOverride { reason: SandboxOverrideReason },
    /// Decision from a bash classifier.
    Classifier { classifier: String, reason: String },
    /// Decision based on working directory scope.
    WorkingDir { reason: String },
    /// Decision from a safety check (sensitive paths, Windows bypass, bridge).
    SafetyCheck {
        reason: String,
        classifier_approvable: bool,
    },
    /// Other / uncategorized reason.
    Other { reason: String },
    /// Default fallback reason.
    Default,
}

/// Reason for a sandbox override decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SandboxOverrideReason {
    ExcludedCommand,
    DangerouslyDisableSandbox,
}

/// Metadata for a permission command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionCommandMetadata {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Metadata attached to permission decisions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionMetadata {
    pub command: PermissionCommandMetadata,
}

/// Result when permission is granted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllowDecision {
    pub behavior: PermissionBehavior,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_input: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_modified: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_reason: Option<DecisionReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accept_feedback: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_blocks: Option<Vec<serde_json::Value>>,
}

/// Metadata for a pending classifier check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingClassifierCheck {
    pub command: String,
    pub cwd: String,
    pub descriptions: Vec<String>,
}

/// Result when user should be prompted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskDecision {
    pub behavior: PermissionBehavior,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_input: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_reason: Option<DecisionReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestions: Option<Vec<PermissionUpdate>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<PermissionMetadata>,
    #[serde(default)]
    pub is_bash_security_check_for_misparsing: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_classifier_check: Option<PendingClassifierCheck>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_blocks: Option<Vec<serde_json::Value>>,
}

/// Result when permission is denied.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DenyDecision {
    pub behavior: PermissionBehavior,
    pub message: String,
    pub decision_reason: DecisionReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
}

/// Result when permission is passed through to the caller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassthroughDecision {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_reason: Option<DecisionReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestions: Option<Vec<PermissionUpdate>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_classifier_check: Option<PendingClassifierCheck>,
}

/// The full permission decision — one of Allow, Ask, Deny, or Passthrough.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "camelCase")]
pub enum PermissionDecisionV2 {
    Allow(AllowDecision),
    Ask(AskDecision),
    Deny(DenyDecision),
    Passthrough(PassthroughDecision),
}

impl PermissionDecisionV2 {
    /// Create an allow decision.
    pub fn allow(reason: Option<DecisionReason>) -> Self {
        Self::Allow(AllowDecision {
            behavior: PermissionBehavior::Allow,
            updated_input: None,
            user_modified: None,
            decision_reason: reason,
            tool_use_id: None,
            accept_feedback: None,
            content_blocks: None,
        })
    }

    /// Create a deny decision.
    pub fn deny(message: impl Into<String>, reason: DecisionReason) -> Self {
        Self::Deny(DenyDecision {
            behavior: PermissionBehavior::Deny,
            message: message.into(),
            decision_reason: reason,
            tool_use_id: None,
        })
    }

    /// Create an ask decision.
    pub fn ask(message: impl Into<String>, reason: Option<DecisionReason>) -> Self {
        Self::Ask(AskDecision {
            behavior: PermissionBehavior::Ask,
            message: message.into(),
            updated_input: None,
            decision_reason: reason,
            suggestions: None,
            blocked_path: None,
            metadata: None,
            is_bash_security_check_for_misparsing: false,
            pending_classifier_check: None,
            content_blocks: None,
        })
    }

    /// Create a passthrough decision.
    pub fn passthrough(message: impl Into<String>) -> Self {
        Self::Passthrough(PassthroughDecision {
            message: message.into(),
            decision_reason: None,
            suggestions: None,
            blocked_path: None,
            pending_classifier_check: None,
        })
    }

    /// Get the behavior of this decision.
    #[must_use]
    pub fn behavior(&self) -> PermissionBehavior {
        match self {
            Self::Allow(_) => PermissionBehavior::Allow,
            Self::Ask(_) => PermissionBehavior::Ask,
            Self::Deny(_) => PermissionBehavior::Deny,
            Self::Passthrough(_) => PermissionBehavior::Ask, // Passthrough acts like Ask
        }
    }

    /// Whether this decision allows the action.
    #[must_use]
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allow(_))
    }

    /// Whether this decision denies the action.
    #[must_use]
    pub fn is_denied(&self) -> bool {
        matches!(self, Self::Deny(_))
    }
}

/// Where a permission update should be persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionUpdateDestination {
    UserSettings,
    ProjectSettings,
    LocalSettings,
    Session,
    CliArg,
}

/// Update operations for permission configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PermissionUpdate {
    AddRules {
        destination: PermissionUpdateDestination,
        rules: Vec<crate::rule::PermissionRuleValue>,
        behavior: PermissionBehavior,
    },
    ReplaceRules {
        destination: PermissionUpdateDestination,
        rules: Vec<crate::rule::PermissionRuleValue>,
        behavior: PermissionBehavior,
    },
    RemoveRules {
        destination: PermissionUpdateDestination,
        rules: Vec<crate::rule::PermissionRuleValue>,
        behavior: PermissionBehavior,
    },
    SetMode {
        destination: PermissionUpdateDestination,
        mode: ExtendedPermissionMode,
    },
    AddDirectories {
        destination: PermissionUpdateDestination,
        directories: Vec<String>,
    },
    RemoveDirectories {
        destination: PermissionUpdateDestination,
        directories: Vec<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_decision_is_allowed() {
        let d = PermissionDecisionV2::allow(None);
        assert!(d.is_allowed());
        assert!(!d.is_denied());
        assert_eq!(d.behavior(), PermissionBehavior::Allow);
    }

    #[test]
    fn deny_decision_is_denied() {
        let d = PermissionDecisionV2::deny("forbidden", DecisionReason::Default);
        assert!(d.is_denied());
        assert!(!d.is_allowed());
    }

    #[test]
    fn ask_decision_behavior() {
        let d = PermissionDecisionV2::ask("confirm?", None);
        assert_eq!(d.behavior(), PermissionBehavior::Ask);
    }

    #[test]
    fn passthrough_behavior_is_ask() {
        let d = PermissionDecisionV2::passthrough("pass along");
        assert_eq!(d.behavior(), PermissionBehavior::Ask);
    }

    #[test]
    fn decision_serialization_roundtrip() {
        let d = PermissionDecisionV2::allow(Some(DecisionReason::Default));
        let json = serde_json::to_string(&d).expect("serialize");
        let back: PermissionDecisionV2 = serde_json::from_str(&json).expect("deserialize");
        assert!(back.is_allowed());
    }

    #[test]
    fn permission_update_serialization() {
        let update = PermissionUpdate::AddRules {
            destination: PermissionUpdateDestination::UserSettings,
            rules: vec![crate::rule::PermissionRuleValue::tool_only("Read")],
            behavior: PermissionBehavior::Allow,
        };
        let json = serde_json::to_string(&update).expect("serialize");
        assert!(json.contains("addRules"));
    }
}
