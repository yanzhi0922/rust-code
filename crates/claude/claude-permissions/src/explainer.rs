//! Permission explanation system.
//!
//! Corresponds to `src/utils/permissions/permissionExplainer.ts`.
//! Generates human-readable explanations for why a permission was granted or denied.

use claude_core::permission_types::PermissionBehavior;

use crate::decision::DecisionReason;

/// Generate a human-readable explanation for a permission decision.
#[must_use]
pub fn explain_permission(
    behavior: PermissionBehavior,
    reason: &Option<DecisionReason>,
    tool_name: &str,
) -> String {
    match behavior {
        PermissionBehavior::Allow => explain_allow(reason, tool_name),
        PermissionBehavior::Deny => explain_deny(reason, tool_name),
        PermissionBehavior::Ask => explain_ask(reason, tool_name),
    }
}

fn explain_allow(reason: &Option<DecisionReason>, tool_name: &str) -> String {
    match reason {
        Some(DecisionReason::Rule { rule }) => format!(
            "✓ {} allowed by {} rule: {} {}",
            tool_name,
            format_source(rule.source),
            rule.value.tool_name,
            rule.value.rule_content.as_deref().unwrap_or("")
        )
        .trim_end()
        .to_string(),
        Some(DecisionReason::Mode { mode }) => {
            format!("✓ {} auto-allowed in {} mode", tool_name, mode.title())
        }
        Some(DecisionReason::Hook {
            hook_name,
            reason: hook_reason,
            ..
        }) => {
            format!(
                "✓ {} allowed by hook '{}': {}",
                tool_name,
                hook_name,
                hook_reason.as_deref().unwrap_or("no reason given")
            )
        }
        Some(DecisionReason::PermissionPromptTool {
            tool_name: pt_name, ..
        }) => {
            format!(
                "✓ {} allowed by permission prompt tool '{}'",
                tool_name, pt_name
            )
        }
        Some(DecisionReason::AsyncAgent {
            reason: agent_reason,
        }) => {
            format!("✓ {} allowed by async agent: {}", tool_name, agent_reason)
        }
        _ => format!("✓ {} allowed", tool_name),
    }
}

fn explain_deny(reason: &Option<DecisionReason>, tool_name: &str) -> String {
    match reason {
        Some(DecisionReason::Rule { rule }) => format!(
            "✗ {} denied by {} rule: {} {}",
            tool_name,
            format_source(rule.source),
            rule.value.tool_name,
            rule.value.rule_content.as_deref().unwrap_or("")
        )
        .trim_end()
        .to_string(),
        Some(DecisionReason::Mode { mode }) => {
            format!("✗ {} denied in {} mode", tool_name, mode.title())
        }
        Some(DecisionReason::Hook {
            hook_name,
            reason: hook_reason,
            ..
        }) => {
            format!(
                "✗ {} denied by hook '{}': {}",
                tool_name,
                hook_name,
                hook_reason.as_deref().unwrap_or("no reason given")
            )
        }
        _ => format!("✗ {} denied", tool_name),
    }
}

fn explain_ask(reason: &Option<DecisionReason>, tool_name: &str) -> String {
    match reason {
        Some(DecisionReason::Mode { mode }) => {
            format!(
                "? {} requires confirmation in {} mode",
                tool_name,
                mode.title()
            )
        }
        Some(DecisionReason::Rule { rule }) => {
            format!(
                "? {} requires confirmation due to {} rule",
                tool_name,
                format_source(rule.source)
            )
        }
        _ => format!("? {} requires confirmation", tool_name),
    }
}

/// Format a permission rule source for display.
fn format_source(source: claude_core::permission_types::PermissionRuleSource) -> &'static str {
    use claude_core::permission_types::PermissionRuleSource::*;
    match source {
        UserSettings => "user settings",
        ProjectSettings => "project settings",
        LocalSettings => "local settings",
        FlagSettings => "flag settings",
        PolicySettings => "policy settings",
        CliArg => "CLI argument",
        Command => "command",
        Session => "session",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode::ExtendedPermissionMode;
    use crate::rule::PermissionRuleV2;
    use claude_core::permission_types::PermissionRuleSource;

    #[test]
    fn explain_allow_default() {
        let msg = explain_permission(PermissionBehavior::Allow, &None, "Read");
        assert!(msg.contains("✓"));
        assert!(msg.contains("Read"));
    }

    #[test]
    fn explain_deny_default() {
        let msg = explain_permission(PermissionBehavior::Deny, &None, "Bash");
        assert!(msg.contains("✗"));
        assert!(msg.contains("Bash"));
    }

    #[test]
    fn explain_ask_default() {
        let msg = explain_permission(PermissionBehavior::Ask, &None, "Write");
        assert!(msg.contains("?"));
        assert!(msg.contains("Write"));
    }

    #[test]
    fn explain_allow_by_rule() {
        let rule = PermissionRuleV2::new(
            PermissionRuleSource::UserSettings,
            PermissionBehavior::Allow,
            "Bash",
            Some("git *".to_string()),
        );
        let reason = DecisionReason::Rule { rule };
        let msg = explain_permission(PermissionBehavior::Allow, &Some(reason), "Bash");
        assert!(msg.contains("user settings"));
        assert!(msg.contains("git *"));
    }

    #[test]
    fn explain_by_mode() {
        let reason = DecisionReason::Mode {
            mode: ExtendedPermissionMode::Auto,
        };
        let msg = explain_permission(PermissionBehavior::Allow, &Some(reason), "Read");
        assert!(msg.contains("Auto mode"));
    }
}
