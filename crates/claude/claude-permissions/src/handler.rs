//! Permission handler system.
//!
//! Corresponds to `src/utils/permissions/permissions.ts` handler logic.
//! Provides different permission handling strategies:
//! - Interactive: prompts the user
//! - Coordinator: manages multi-agent permission coordination
//! - SwarmWorker: delegates to swarm leader

use async_trait::async_trait;
use claude_core::permission_types::PermissionBehavior;
use serde_json::Value;

use crate::decision::{DecisionReason, PermissionDecisionV2};
use crate::mode::ExtendedPermissionMode;
use crate::rule::PermissionRuleV2;

/// Context for a permission check.
#[derive(Debug, Clone)]
pub struct PermissionCheckContext {
    /// Current working directory.
    pub cwd: String,
    /// Current permission mode.
    pub mode: ExtendedPermissionMode,
    /// Tool name being checked.
    pub tool_name: String,
    /// Tool input.
    pub input: Value,
    /// Tool use ID.
    pub tool_use_id: Option<String>,
    /// Additional allowed directories.
    pub additional_directories: Vec<String>,
}

/// Trait for permission handlers.
#[async_trait]
pub trait PermissionHandler: Send + Sync {
    /// Handle a permission check.
    async fn check_permission(
        &self,
        ctx: &PermissionCheckContext,
        rules: &[PermissionRuleV2],
    ) -> PermissionDecisionV2;

    /// Name of this handler.
    fn name(&self) -> &str;
}

/// Interactive permission handler — prompts the user for decisions.
///
/// This is the default handler that evaluates rules first, then falls back
/// to prompting the user if no rule matches.
pub struct InteractiveHandler {
    /// Whether to auto-accept edits.
    auto_accept_edits: bool,
}

impl InteractiveHandler {
    /// Create a new interactive handler.
    #[must_use]
    pub fn new(auto_accept_edits: bool) -> Self {
        Self { auto_accept_edits }
    }
}

#[async_trait]
impl PermissionHandler for InteractiveHandler {
    async fn check_permission(
        &self,
        ctx: &PermissionCheckContext,
        rules: &[PermissionRuleV2],
    ) -> PermissionDecisionV2 {
        // Check mode-based decisions first
        match ctx.mode {
            ExtendedPermissionMode::BypassPermissions => {
                return PermissionDecisionV2::allow(Some(DecisionReason::Mode { mode: ctx.mode }));
            }
            ExtendedPermissionMode::Plan => {
                return PermissionDecisionV2::deny(
                    "Tool execution not allowed in plan mode",
                    DecisionReason::Mode { mode: ctx.mode },
                );
            }
            ExtendedPermissionMode::DontAsk => {
                // Auto-deny if no explicit allow rule
                let has_allow = rules.iter().any(|r| {
                    r.behavior == PermissionBehavior::Allow
                        && r.matches(
                            &ctx.tool_name,
                            ctx.input.get("command").and_then(|v| v.as_str()),
                        )
                });
                if has_allow {
                    return PermissionDecisionV2::allow(Some(DecisionReason::Mode {
                        mode: ctx.mode,
                    }));
                }
                return PermissionDecisionV2::deny(
                    "No allow rule found in dontAsk mode",
                    DecisionReason::Mode { mode: ctx.mode },
                );
            }
            ExtendedPermissionMode::AcceptEdits if self.auto_accept_edits => {
                if ctx.tool_name == "Write"
                    || ctx.tool_name == "Edit"
                    || ctx.tool_name == "MultiEdit"
                {
                    return PermissionDecisionV2::allow(Some(DecisionReason::Mode {
                        mode: ctx.mode,
                    }));
                }
            }
            _ => {}
        }

        // Check rules
        for rule in rules {
            let content = match ctx.tool_name.as_str() {
                "Bash" | "BashCommand" => ctx
                    .input
                    .get("command")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                "Read" => ctx
                    .input
                    .get("file_path")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                "Write" | "Edit" => ctx
                    .input
                    .get("file_path")
                    .or_else(|| ctx.input.get("path"))
                    .and_then(|v| v.as_str())
                    .map(String::from),
                _ => None,
            };

            if rule.matches(&ctx.tool_name, content.as_deref()) {
                return match rule.behavior {
                    PermissionBehavior::Allow => {
                        PermissionDecisionV2::allow(Some(DecisionReason::Rule {
                            rule: rule.clone(),
                        }))
                    }
                    PermissionBehavior::Deny => PermissionDecisionV2::deny(
                        format!("Denied by {} rule", rule.value.tool_name),
                        DecisionReason::Rule { rule: rule.clone() },
                    ),
                    PermissionBehavior::Ask => PermissionDecisionV2::ask(
                        format!("Confirmation required by {} rule", rule.value.tool_name),
                        Some(DecisionReason::Rule { rule: rule.clone() }),
                    ),
                };
            }
        }

        // Default: ask the user
        PermissionDecisionV2::ask(
            format!("Allow {} to execute?", ctx.tool_name),
            Some(DecisionReason::Mode { mode: ctx.mode }),
        )
    }

    fn name(&self) -> &str {
        "interactive"
    }
}

/// Coordinator handler — manages permission decisions for multi-agent scenarios.
pub struct CoordinatorHandler {
    /// The underlying handler for local decisions.
    inner: InteractiveHandler,
}

impl CoordinatorHandler {
    /// Create a new coordinator handler.
    #[must_use]
    pub fn new(auto_accept_edits: bool) -> Self {
        Self {
            inner: InteractiveHandler::new(auto_accept_edits),
        }
    }
}

#[async_trait]
impl PermissionHandler for CoordinatorHandler {
    async fn check_permission(
        &self,
        ctx: &PermissionCheckContext,
        rules: &[PermissionRuleV2],
    ) -> PermissionDecisionV2 {
        // Coordinator delegates to inner handler with additional context
        self.inner.check_permission(ctx, rules).await
    }

    fn name(&self) -> &str {
        "coordinator"
    }
}

/// Swarm worker handler — delegates decisions to the swarm leader.
pub struct SwarmWorkerHandler;

impl Default for SwarmWorkerHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl SwarmWorkerHandler {
    /// Create a new swarm worker handler.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl PermissionHandler for SwarmWorkerHandler {
    async fn check_permission(
        &self,
        ctx: &PermissionCheckContext,
        _rules: &[PermissionRuleV2],
    ) -> PermissionDecisionV2 {
        // In swarm mode, worker delegates to leader via passthrough
        PermissionDecisionV2::passthrough(format!(
            "Swarm worker: delegate {} permission to leader",
            ctx.tool_name
        ))
    }

    fn name(&self) -> &str {
        "swarm_worker"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use claude_core::permission_types::PermissionRuleSource;

    fn make_ctx(mode: ExtendedPermissionMode, tool: &str) -> PermissionCheckContext {
        PermissionCheckContext {
            cwd: "/tmp".to_string(),
            mode,
            tool_name: tool.to_string(),
            input: serde_json::json!({"command": "git status"}),
            tool_use_id: None,
            additional_directories: vec![],
        }
    }

    #[tokio::test]
    async fn bypass_mode_allows_all() {
        let handler = InteractiveHandler::new(false);
        let ctx = make_ctx(ExtendedPermissionMode::BypassPermissions, "Bash");
        let result = handler.check_permission(&ctx, &[]).await;
        assert!(result.is_allowed());
    }

    #[tokio::test]
    async fn plan_mode_denies_all() {
        let handler = InteractiveHandler::new(false);
        let ctx = make_ctx(ExtendedPermissionMode::Plan, "Bash");
        let result = handler.check_permission(&ctx, &[]).await;
        assert!(result.is_denied());
    }

    #[tokio::test]
    async fn rule_allow_overrides() {
        let handler = InteractiveHandler::new(false);
        let ctx = make_ctx(ExtendedPermissionMode::Default, "Bash");
        let rules = vec![PermissionRuleV2::new(
            PermissionRuleSource::UserSettings,
            PermissionBehavior::Allow,
            "Bash",
            Some("git *".to_string()),
        )];
        let result = handler.check_permission(&ctx, &rules).await;
        assert!(result.is_allowed());
    }

    #[tokio::test]
    async fn rule_deny_blocks() {
        let handler = InteractiveHandler::new(false);
        let ctx = make_ctx(ExtendedPermissionMode::Default, "Bash");
        let rules = vec![PermissionRuleV2::new(
            PermissionRuleSource::PolicySettings,
            PermissionBehavior::Deny,
            "Bash",
            Some("rm *".to_string()),
        )];
        let result = handler.check_permission(&ctx, &rules).await;
        // "git status" doesn't match "rm *", so should ask
        assert!(!result.is_denied());
    }

    #[tokio::test]
    async fn default_mode_asks_when_no_rules() {
        let handler = InteractiveHandler::new(false);
        let ctx = make_ctx(ExtendedPermissionMode::Default, "Bash");
        let result = handler.check_permission(&ctx, &[]).await;
        assert_eq!(result.behavior(), PermissionBehavior::Ask);
    }

    #[tokio::test]
    async fn swarm_worker_passthrough() {
        let handler = SwarmWorkerHandler::new();
        let ctx = make_ctx(ExtendedPermissionMode::Default, "Bash");
        let result = handler.check_permission(&ctx, &[]).await;
        assert_eq!(result.behavior(), PermissionBehavior::Ask); // passthrough maps to Ask
    }

    #[tokio::test]
    async fn coordinator_delegates() {
        let handler = CoordinatorHandler::new(false);
        let ctx = make_ctx(ExtendedPermissionMode::BypassPermissions, "Bash");
        let result = handler.check_permission(&ctx, &[]).await;
        assert!(result.is_allowed());
        assert_eq!(handler.name(), "coordinator");
    }
}
