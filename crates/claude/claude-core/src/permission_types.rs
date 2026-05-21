use serde::{Deserialize, Serialize};

/// Permission behavior used by v2 permission providers and audits.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionBehavior {
    Allow,
    Deny,
    Ask,
}

/// Why a permission decision was made — mirrors the TS `decisionReason` field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PermissionDecisionReason {
    /// Matched an entry in the allow/deny lists from settings.
    SettingsMatch { source: PermissionRuleSource },
    /// Decision driven by permission mode (e.g. bypass, acceptEdits).
    Mode,
    /// Result from a subcommand check (compound bash commands).
    SubcommandResults,
    /// Decision made by the permission prompt tool (headless SDK mode).
    PermissionPromptTool,
    /// Decision made by a hook (PreToolUse hook returned allow/deny).
    Hook,
    /// Decision made by an async/headless agent.
    AsyncAgent,
    /// Classifier-based auto-approval (used in `auto` permission mode).
    Classifier,
    /// Sandbox override — tool is allowed because it runs in sandbox mode.
    SandboxOverride,
    /// Decision based on working directory scope.
    WorkingDir,
    /// Safety check — bypass-immune check for protected paths (.git, .claude, etc).
    SafetyCheck,
    /// User explicitly approved/denied via interactive prompt.
    UserDecision,
    /// Pre-approved in a previous session and remembered.
    PreviouslyApproved,
    /// Default fallback when no rule matched.
    DefaultRule,
    /// Other/miscellaneous reason.
    Other,
}

/// Metadata attached to every permission decision for audit and provenance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PermissionDecisionMeta {
    /// Why this decision was made.
    pub reason: PermissionDecisionReason,
    /// Whether the decision was a passthrough (no prompt shown to the user).
    #[serde(default)]
    pub passthrough: bool,
    /// Optional classifier label when the decision came from a model-based classifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classifier: Option<String>,
}

/// Result returned from a permission check.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum PermissionResult {
    Allow {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        meta: Option<PermissionDecisionMeta>,
    },
    Deny {
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        meta: Option<PermissionDecisionMeta>,
    },
    Ask {
        prompt: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        meta: Option<PermissionDecisionMeta>,
    },
}

/// Source of an active permission rule.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionRuleSource {
    UserSettings,
    ProjectSettings,
    LocalSettings,
    FlagSettings,
    PolicySettings,
    CliArg,
    Command,
    Session,
}

/// Structured permission rule used for provenance-aware evaluation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PermissionRule {
    pub source: PermissionRuleSource,
    pub behavior: PermissionBehavior,
    pub tool_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_content: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{PermissionBehavior, PermissionResult, PermissionRule, PermissionRuleSource};

    #[test]
    fn permission_rule_serializes_with_stable_shape() {
        let rule = PermissionRule {
            source: PermissionRuleSource::ProjectSettings,
            behavior: PermissionBehavior::Ask,
            tool_name: "bash_command".to_owned(),
            rule_content: Some("Bash(git status *)".to_owned()),
        };
        let value = serde_json::to_value(&rule).expect("rule should serialize");
        assert_eq!(value["source"], "project_settings");
        assert_eq!(value["behavior"], "ask");
    }

    #[test]
    fn permission_result_preserves_reason_and_prompt() {
        let deny = PermissionResult::Deny {
            reason: "outside workspace".to_owned(),
            meta: None,
        };
        let ask = PermissionResult::Ask {
            prompt: "Allow write?".to_owned(),
            meta: None,
        };
        assert!(
            serde_json::to_string(&deny)
                .expect("deny should serialize")
                .contains("outside")
        );
        assert!(
            serde_json::to_string(&ask)
                .expect("ask should serialize")
                .contains("Allow")
        );
    }
}
