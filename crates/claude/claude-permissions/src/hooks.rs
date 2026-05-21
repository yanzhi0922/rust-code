//! PreToolUse / PostToolUse hook infrastructure.
//!
//! Mirrors the TS reference's hook system in `src/types/hooks.ts` and
//! `src/services/tools/toolHooks.ts`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::decision::DecisionReason;

/// The hook event names recognised by the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
}

/// Result from executing a single hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocking_error: Option<String>,
    #[serde(default = "default_outcome")]
    pub outcome: String,
    #[serde(default)]
    pub prevent_continuation: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_behavior: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_input: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_context: Option<String>,
}

fn default_outcome() -> String {
    "success".to_owned()
}

/// Aggregated result from running multiple hooks for a single event.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AggregatedHookResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default)]
    pub blocking_errors: Vec<String>,
    #[serde(default)]
    pub prevent_continuation: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_behavior: Option<String>,
    #[serde(default)]
    pub additional_contexts: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_input: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_mcp_tool_output: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_decision: Option<HookPermissionDecision>,
}

/// Permission decision from a hook.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookPermissionDecision {
    Allow,
    Deny,
    Ask,
}

/// Base fields present on all hook inputs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaseHookInput {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
}

/// Input for a PreToolUse hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreToolUseHookInput {
    #[serde(flatten)]
    pub base: BaseHookInput,
    pub hook_event_name: String,
    pub tool_name: String,
    pub tool_input: Value,
    pub tool_use_id: String,
}

/// Input for a PostToolUse hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostToolUseHookInput {
    #[serde(flatten)]
    pub base: BaseHookInput,
    pub hook_event_name: String,
    pub tool_name: String,
    pub tool_input: Value,
    pub tool_response: Value,
    pub tool_use_id: String,
}

/// A configured hook command.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HookCommand {
    Command {
        command: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shell: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout: Option<u64>,
    },
    Http {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        headers: Option<serde_json::Map<String, Value>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout: Option<u64>,
    },
}

/// A hook matcher — matches a tool name pattern and runs hooks.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookMatcher {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    pub hooks: Vec<HookCommand>,
}

/// Full hook configuration, keyed by event name.
pub type HookConfig = std::collections::HashMap<HookEvent, Vec<HookMatcher>>;

/// Resolve the aggregated hook result into a permission decision reason.
pub fn resolve_hook_permission_decision(
    hook_result: &AggregatedHookResult,
) -> Option<DecisionReason> {
    if let Some(ref decision) = hook_result.permission_decision {
        match decision {
            HookPermissionDecision::Deny => Some(DecisionReason::Hook {
                hook_name: "PreToolUse".to_owned(),
                hook_source: None,
                reason: hook_result.blocking_errors.first().cloned(),
            }),
            HookPermissionDecision::Allow => Some(DecisionReason::Hook {
                hook_name: "PreToolUse".to_owned(),
                hook_source: None,
                reason: hook_result.message.clone(),
            }),
            HookPermissionDecision::Ask => None,
        }
    } else if !hook_result.blocking_errors.is_empty() {
        Some(DecisionReason::Hook {
            hook_name: "PreToolUse".to_owned(),
            hook_source: None,
            reason: hook_result.blocking_errors.first().cloned(),
        })
    } else {
        None
    }
}

/// Aggregate multiple hook results into a single result.
pub fn aggregate_hook_results(results: Vec<HookResult>) -> AggregatedHookResult {
    let mut agg = AggregatedHookResult::default();
    for result in results {
        if let Some(msg) = result.message {
            agg.message = Some(match agg.message.take() {
                Some(existing) => format!("{}\n{}", existing, msg),
                None => msg,
            });
        }
        if let Some(err) = result.blocking_error {
            agg.blocking_errors.push(err);
        }
        if result.prevent_continuation {
            agg.prevent_continuation = true;
        }
        if let Some(sr) = result.stop_reason {
            agg.stop_reason = Some(sr);
        }
        if let Some(pb) = result.permission_behavior {
            agg.permission_behavior = Some(pb);
        }
        if let Some(ctx) = result.additional_context {
            agg.additional_contexts.push(ctx);
        }
        if let Some(input) = result.updated_input {
            agg.updated_input = Some(input);
        }
        if result.outcome == "blocking" && agg.permission_decision.is_none() {
            agg.permission_decision = Some(HookPermissionDecision::Deny);
        }
    }
    agg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_collects_blocking_errors() {
        let results = vec![
            HookResult {
                message: Some("ok".into()),
                blocking_error: None,
                outcome: "success".into(),
                prevent_continuation: false,
                stop_reason: None,
                permission_behavior: None,
                updated_input: None,
                additional_context: None,
                system_message: None,
            },
            HookResult {
                message: None,
                blocking_error: Some("denied!".into()),
                outcome: "blocking".into(),
                prevent_continuation: true,
                stop_reason: None,
                permission_behavior: None,
                updated_input: None,
                additional_context: Some("ctx".into()),
                system_message: None,
            },
        ];
        let agg = aggregate_hook_results(results);
        assert_eq!(agg.message.as_deref(), Some("ok"));
        assert_eq!(agg.blocking_errors, vec!["denied!"]);
        assert!(agg.prevent_continuation);
    }

    #[test]
    fn resolve_deny_returns_reason() {
        let agg = AggregatedHookResult {
            blocking_errors: vec!["forbidden".into()],
            permission_decision: Some(HookPermissionDecision::Deny),
            ..Default::default()
        };
        let reason = resolve_hook_permission_decision(&agg);
        assert!(matches!(reason, Some(DecisionReason::Hook { .. })));
    }
}
