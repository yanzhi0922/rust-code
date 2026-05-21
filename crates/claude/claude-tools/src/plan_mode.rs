//! Plan mode runtime integration and tool guarding.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow};
use claude_core::{PermissionMode, ToolCall, ToolResult};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{ToolExecutionContext, ToolSpec};

static PLAN_MODE_RUNTIME: Lazy<Mutex<Option<Arc<dyn PlanModeRuntime>>>> =
    Lazy::new(|| Mutex::new(None));

const PLAN_MODE_SAFE_TOOLS: &[&str] = &[
    "agent",
    "ask_user",
    "broadcast_message",
    "brief",
    "config_read",
    "ctx_inspect",
    "exit_plan_mode",
    "glob",
    "grep",
    "list_directory",
    "list_worktrees",
    "lsp",
    "mcp_server_list",
    "mcp_list_resources",
    "mcp_resource_read",
    "list_peers",
    "read_file",
    "review_artifact",
    "search_text",
    "send_message",
    "snip",
    "task_get",
    "task_list",
    "team_create",
    "team_status",
    "tool_search",
    "verify_plan",
    "web_browser",
    "web_fetch",
    "web_search",
];

const PLAN_MODE_DENIED_READ_CLASS_TOOLS: &[&str] = &[
    "daemon",
    "mcp_auth",
    "mcp_call",
    "remote_trigger",
    "skill_discover",
    "skill_execute",
    "sleep",
    "synthetic_output",
    "task_create",
    "task_update",
    "team_delete",
    "terminal_capture",
    "todo_write",
    "tungsten",
    "workflow",
];

/// Runtime snapshot used to make tool-gating decisions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlanModeRuntimeSnapshot {
    #[serde(default)]
    pub permission_mode: PermissionMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_file_path: Option<PathBuf>,
}

/// Normalized `ExitPlanMode` input after runtime/API-side plan injection.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExitPlanModeInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_file_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_prompts: Vec<AllowedPrompt>,
}

/// Prompt-based permission requested by a plan-mode exit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AllowedPrompt {
    pub tool: String,
    pub prompt: String,
}

/// Host-owned runtime seam for real plan-mode state transitions.
///
/// Implementors **must** provide [`persist_plan_snapshot`] to durably save
/// the current plan state. The default implementation is intentionally
/// absent so that a missing override is caught at compile time.
pub trait PlanModeRuntime: Send + Sync {
    fn enter_plan_mode(&self, objective: &str) -> Result<String>;
    fn exit_plan_mode(&self, input: ExitPlanModeInput) -> Result<String>;
    fn snapshot(&self) -> PlanModeRuntimeSnapshot;

    /// Persist the current plan snapshot to durable storage.
    ///
    /// Called after plan-mode transitions to ensure the plan state survives
    /// process restarts. Implementations should write the snapshot returned
    /// by [`snapshot`](Self::snapshot) to disk or another durable store.
    fn persist_plan_snapshot(&self) -> Result<()>;
}

/// Configure the active process-scoped plan-mode runtime.
///
/// This mirrors the existing process-scoped tool runtime policy: the host
/// configures a single session runtime before entering the main prompt loop.
pub fn configure_plan_mode_runtime(runtime: Option<Arc<dyn PlanModeRuntime>>) -> Result<()> {
    let mut slot = PLAN_MODE_RUNTIME
        .lock()
        .map_err(|_| anyhow!("plan mode runtime lock poisoned"))?;
    *slot = runtime;
    Ok(())
}

fn current_runtime() -> Option<Arc<dyn PlanModeRuntime>> {
    PLAN_MODE_RUNTIME
        .lock()
        .ok()
        .and_then(|runtime| runtime.clone())
}

pub(crate) fn persist_plan_snapshot_if_active() -> Result<()> {
    if let Some(runtime) = current_runtime() {
        runtime.persist_plan_snapshot()?;
    }
    Ok(())
}

/// Enter plan mode through the host runtime when available.
pub fn enter_plan_mode(input: &Value, _context: &ToolExecutionContext) -> Result<String> {
    let objective = input
        .get("objective")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("implementation task");

    if let Some(runtime) = current_runtime() {
        return runtime.enter_plan_mode(objective);
    }

    Ok(format!(
        "Entered plan mode.\n\nObjective: {objective}\n\nPlan mode is active. Stay read-only, inspect the codebase, and design an implementation approach before exiting plan mode."
    ))
}

/// Exit plan mode through the host runtime when available.
///
/// # Errors
/// Returns an error if the host runtime rejects the transition.
pub fn exit_plan_mode(input: &Value, _context: &ToolExecutionContext) -> Result<String> {
    let normalized = parse_exit_plan_mode_input(input);

    if let Some(runtime) = current_runtime() {
        return runtime.exit_plan_mode(normalized);
    }

    Ok(render_exit_plan_mode_result(&ExitPlanModeToolResult {
        plan: normalized.plan,
        file_path: normalized
            .plan_file_path
            .map(|path| path.display().to_string()),
        is_agent: false,
        has_task_tool: false,
        plan_was_edited: false,
        awaiting_leader_approval: false,
        request_id: None,
    }))
}

/// Inject the current plan content into any `exit_plan_mode` tool call before
/// it is persisted or executed. This mirrors the upstream runtime's
/// normalized-tool-input behavior closely enough for resume recovery.
pub fn normalize_exit_plan_mode_tool_calls(tool_calls: &mut [ToolCall]) {
    let Some(plan_file_path) = current_plan_file_path() else {
        return;
    };
    let plan_file_path_string = plan_file_path.display().to_string();
    let plan_content = fs::read_to_string(&plan_file_path).ok();

    for call in tool_calls
        .iter_mut()
        .filter(|call| call.name == "exit_plan_mode")
    {
        if !call.input.is_object() {
            call.input = Value::Object(serde_json::Map::new());
        }
        let Some(input) = call.input.as_object_mut() else {
            continue;
        };
        input
            .entry("plan_file_path".to_owned())
            .or_insert_with(|| Value::String(plan_file_path_string.clone()));
        input
            .entry("planFilePath".to_owned())
            .or_insert_with(|| Value::String(plan_file_path_string.clone()));
        if let Some(plan_content) = plan_content.as_ref() {
            input
                .entry("plan".to_owned())
                .or_insert_with(|| Value::String(plan_content.clone()));
        }
    }
}

/// Apply plan-mode write restrictions before any tool executes.
#[must_use]
pub fn plan_mode_guard(
    spec: &ToolSpec,
    call: &ToolCall,
    context: &ToolExecutionContext,
    mode: Option<PermissionMode>,
) -> Option<ToolResult> {
    if mode != Some(PermissionMode::Plan) {
        return None;
    }

    if is_plan_file_edit(spec.name.as_str(), call, context)
        || is_safe_plan_mode_tool(spec.name.as_str())
    {
        return None;
    }

    Some(ToolResult {
        content: blocked_tool_message(spec.name.as_str(), current_plan_file_path()),
        is_error: true,
        content_blocks: Vec::new(),
        follow_up_user_blocks: Vec::new(),
    })
}

fn is_safe_plan_mode_tool(tool_name: &str) -> bool {
    PLAN_MODE_SAFE_TOOLS.contains(&tool_name)
        || (is_read_class_tool(tool_name)
            && !PLAN_MODE_DENIED_READ_CLASS_TOOLS.contains(&tool_name))
}

fn is_read_class_tool(tool_name: &str) -> bool {
    matches!(
        claude_permissions::classify_tool(tool_name),
        claude_permissions::PermissionClass::Read
    )
}

fn is_plan_file_edit(tool_name: &str, call: &ToolCall, context: &ToolExecutionContext) -> bool {
    if !matches!(tool_name, "write_file" | "replace_in_file" | "edit_file") {
        return false;
    }

    let Some(path) = call
        .input
        .get("path")
        .or_else(|| call.input.get("file_path"))
        .and_then(Value::as_str)
    else {
        return false;
    };

    let Some(plan_file_path) = current_plan_file_path() else {
        return false;
    };

    normalize_joined_path(path, &context.cwd) == normalize_path(plan_file_path)
}

pub(crate) fn current_plan_file_path() -> Option<PathBuf> {
    current_runtime().and_then(|runtime| runtime.snapshot().plan_file_path)
}

fn blocked_tool_message(tool_name: &str, plan_file_path: Option<PathBuf>) -> String {
    let plan_file_hint = plan_file_path
        .map(|path| {
            format!(
                "\n\nThe only file you may edit right now is:\n{}",
                path.display()
            )
        })
        .unwrap_or_default();

    format!(
        "Plan mode is active. `{tool_name}` is not allowed right now.\n\nUse read-only tools to inspect the project, update the plan file as needed, and call `exit_plan_mode` when the plan is ready.{plan_file_hint}"
    )
}

fn normalize_joined_path(path: &str, cwd: &Path) -> PathBuf {
    let candidate = PathBuf::from(path);
    if candidate.is_absolute() {
        normalize_path(candidate)
    } else {
        normalize_path(cwd.join(candidate))
    }
}

fn normalize_path(path: impl Into<PathBuf>) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.into().components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn parse_exit_plan_mode_input(input: &Value) -> ExitPlanModeInput {
    let plan = input
        .get("plan")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let plan_file_path = input
        .get("planFilePath")
        .or_else(|| input.get("plan_file_path"))
        .and_then(Value::as_str)
        .map(PathBuf::from);
    let allowed_prompts = input
        .get("allowedPrompts")
        .or_else(|| input.get("allowed_prompts"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let tool = item.get("tool")?.as_str()?.trim();
                    let prompt = item.get("prompt")?.as_str()?.trim();
                    if tool.is_empty() || prompt.is_empty() {
                        None
                    } else {
                        Some(AllowedPrompt {
                            tool: tool.to_owned(),
                            prompt: prompt.to_owned(),
                        })
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    ExitPlanModeInput {
        plan,
        plan_file_path,
        allowed_prompts,
    }
}

/// Data returned by the host runtime and rendered like the upstream
/// `mapToolResultToToolResultBlockParam` contract.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExitPlanModeToolResult {
    pub plan: Option<String>,
    pub file_path: Option<String>,
    #[serde(default)]
    pub is_agent: bool,
    #[serde(default)]
    pub has_task_tool: bool,
    #[serde(default)]
    pub plan_was_edited: bool,
    #[serde(default)]
    pub awaiting_leader_approval: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

#[must_use]
pub fn render_exit_plan_mode_result(result: &ExitPlanModeToolResult) -> String {
    if result.awaiting_leader_approval {
        let file_path = result.file_path.as_deref().unwrap_or("(missing)");
        let request_id = result.request_id.as_deref().unwrap_or("(missing)");
        return format!(
            "Your plan has been submitted to the team lead for approval.\n\nPlan file: {file_path}\n\n**What happens next:**\n1. Wait for the team lead to review your plan\n2. You will receive a message in your inbox with approval/rejection\n3. If approved, you can proceed with implementation\n4. If rejected, refine your plan based on the feedback\n\n**Important:** Do NOT proceed until you receive approval. Check your inbox for response.\n\nRequest ID: {request_id}"
        );
    }

    if result.is_agent {
        return "User has approved the plan. There is nothing else needed from you now. Please respond with \"ok\"".to_owned();
    }

    let Some(plan) = result
        .plan
        .as_deref()
        .filter(|plan| !plan.trim().is_empty())
    else {
        return "User has approved exiting plan mode. You can now proceed.".to_owned();
    };

    let file_path = result.file_path.as_deref().unwrap_or("(missing)");
    let team_hint = if result.has_task_tool {
        "\n\nIf this plan can be broken down into multiple independent tasks, consider using the TeamCreate tool to create a team and parallelize the work."
    } else {
        ""
    };
    let plan_label = if result.plan_was_edited {
        "Approved Plan (edited by user)"
    } else {
        "Approved Plan"
    };

    format!(
        "User has approved your plan. You can now start coding. Start with updating your todo list if applicable\n\nYour plan has been saved to: {file_path}\nYou can refer back to it if needed during implementation.{team_hint}\n\n## {plan_label}:\n{plan}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use serde_json::json;

    static PLAN_MODE_TEST_MUTEX: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    #[derive(Debug)]
    struct StubPlanRuntime {
        plan_file_path: Option<PathBuf>,
    }

    impl PlanModeRuntime for StubPlanRuntime {
        fn enter_plan_mode(&self, objective: &str) -> Result<String> {
            Ok(format!(
                "Entered plan mode for `{objective}`.\nPlan file: {}",
                self.plan_file_path
                    .as_ref()
                    .expect("plan file path")
                    .display()
            ))
        }

        fn exit_plan_mode(&self, input: ExitPlanModeInput) -> Result<String> {
            Ok(format!(
                "Exited plan mode.\nSummary: {}",
                input.plan.as_deref().unwrap_or("(none)")
            ))
        }

        fn snapshot(&self) -> PlanModeRuntimeSnapshot {
            PlanModeRuntimeSnapshot {
                permission_mode: PermissionMode::Plan,
                plan_file_path: self.plan_file_path.clone(),
            }
        }

        fn persist_plan_snapshot(&self) -> Result<()> {
            // Stub: no-op for tests
            Ok(())
        }
    }

    fn test_context() -> ToolExecutionContext {
        ToolExecutionContext {
            cwd: PathBuf::from("/tmp/workspace"),
            original_cwd: PathBuf::from("/tmp/workspace"),
            active_worktree_session: None,
            timeout_ms: 30_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Arc::new(parking_lot::Mutex::new(
                claude_core::task_stack::TaskStack::default(),
            )),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    #[test]
    fn enter_plan_mode_accepts_empty_input_like_research_schema() {
        let _guard = PLAN_MODE_TEST_MUTEX.lock().expect("test mutex");
        let input = json!({});
        let result = enter_plan_mode(&input, &test_context());
        assert!(
            result
                .expect("enter plan mode")
                .contains("Entered plan mode")
        );
    }

    #[test]
    fn enter_plan_mode_uses_runtime_when_configured() {
        let _guard = PLAN_MODE_TEST_MUTEX.lock().expect("test mutex");
        configure_plan_mode_runtime(Some(Arc::new(StubPlanRuntime {
            plan_file_path: Some(PathBuf::from(
                "/tmp/workspace/.remote-code-rust/plans/demo.md",
            )),
        })))
        .expect("configure");

        let result = enter_plan_mode(&json!({"objective": "Refactor auth"}), &test_context())
            .expect("enter plan mode");
        assert!(result.contains("Plan file:"));

        configure_plan_mode_runtime(None).expect("clear runtime");
    }

    #[test]
    fn plan_mode_guard_allows_only_plan_file_edits() {
        let _guard = PLAN_MODE_TEST_MUTEX.lock().expect("test mutex");
        configure_plan_mode_runtime(Some(Arc::new(StubPlanRuntime {
            plan_file_path: Some(PathBuf::from(
                "/tmp/workspace/.remote-code-rust/plans/demo.md",
            )),
        })))
        .expect("configure");

        let spec = ToolSpec {
            name: "write_file".to_owned(),
            protocol_name: "WriteFile".to_owned(),
            permission_tool_name: "Edit".to_owned(),
            description: "write".to_owned(),
            requires_permission: true,
            input_schema: Value::Null,
        };
        let allowed = plan_mode_guard(
            &spec,
            &ToolCall {
                id: "tool-1".to_owned(),
                name: "write_file".to_owned(),
                input: json!({"path": ".remote-code-rust/plans/demo.md", "content": "plan"}),
            },
            &test_context(),
            Some(PermissionMode::Plan),
        );
        assert!(allowed.is_none());

        let blocked = plan_mode_guard(
            &spec,
            &ToolCall {
                id: "tool-2".to_owned(),
                name: "write_file".to_owned(),
                input: json!({"path": "src/main.rs", "content": "oops"}),
            },
            &test_context(),
            Some(PermissionMode::Plan),
        )
        .expect("blocked result");
        assert!(blocked.is_error);
        assert!(blocked.content.contains("only file you may edit"));

        configure_plan_mode_runtime(None).expect("clear runtime");
    }

    #[test]
    fn plan_mode_guard_blocks_todo_write_even_though_it_is_permissionless() {
        let result = plan_mode_guard(
            &ToolSpec {
                name: "todo_write".to_owned(),
                protocol_name: "TodoWrite".to_owned(),
                permission_tool_name: "TodoWrite".to_owned(),
                description: "write todos".to_owned(),
                requires_permission: false,
                input_schema: Value::Null,
            },
            &ToolCall {
                id: "tool-3".to_owned(),
                name: "todo_write".to_owned(),
                input: json!({"todos": []}),
            },
            &test_context(),
            Some(PermissionMode::Plan),
        )
        .expect("blocked");
        assert!(result.is_error);
        assert!(result.content.contains("Plan mode is active"));
    }

    #[test]
    fn plan_mode_guard_allows_team_coordination_tools() {
        for tool_name in [
            "agent",
            "team_create",
            "team_status",
            "send_message",
            "list_peers",
        ] {
            let allowed = plan_mode_guard(
                &ToolSpec {
                    name: tool_name.to_owned(),
                    protocol_name: tool_name.to_owned(),
                    permission_tool_name: tool_name.to_owned(),
                    description: "coordination".to_owned(),
                    requires_permission: false,
                    input_schema: Value::Null,
                },
                &ToolCall {
                    id: format!("call-{tool_name}"),
                    name: tool_name.to_owned(),
                    input: json!({}),
                },
                &test_context(),
                Some(PermissionMode::Plan),
            );
            assert!(
                allowed.is_none(),
                "{tool_name} should be allowed in plan mode for teammate coordination"
            );
        }
    }

    #[test]
    fn normalize_exit_plan_mode_tool_calls_injects_plan_context() {
        let _guard = PLAN_MODE_TEST_MUTEX.lock().expect("test mutex");
        let tempdir = tempfile::tempdir().expect("tempdir");
        let plan_path = tempdir.path().join("demo-plan.md");
        fs::write(&plan_path, "# Plan\n- inspect\n").expect("write plan");
        configure_plan_mode_runtime(Some(Arc::new(StubPlanRuntime {
            plan_file_path: Some(plan_path.clone()),
        })))
        .expect("configure");

        let mut tool_calls = vec![ToolCall {
            id: "tool-1".to_owned(),
            name: "exit_plan_mode".to_owned(),
            input: json!({}),
        }];
        normalize_exit_plan_mode_tool_calls(&mut tool_calls);

        assert_eq!(
            tool_calls[0].input["plan_file_path"].as_str(),
            Some(plan_path.to_string_lossy().as_ref())
        );
        assert_eq!(
            tool_calls[0].input["planFilePath"].as_str(),
            Some(plan_path.to_string_lossy().as_ref())
        );
        assert_eq!(
            tool_calls[0].input["plan"].as_str(),
            Some("# Plan\n- inspect\n")
        );

        configure_plan_mode_runtime(None).expect("clear runtime");
    }

    #[test]
    fn exit_plan_mode_result_matches_research_empty_plan_contract() {
        let content = render_exit_plan_mode_result(&ExitPlanModeToolResult::default());
        assert_eq!(
            content,
            "User has approved exiting plan mode. You can now proceed."
        );
    }

    #[test]
    fn exit_plan_mode_result_matches_research_approved_plan_contract() {
        let content = render_exit_plan_mode_result(&ExitPlanModeToolResult {
            plan: Some("# Plan\n- implement".to_owned()),
            file_path: Some("/tmp/plan.md".to_owned()),
            is_agent: false,
            has_task_tool: true,
            plan_was_edited: true,
            awaiting_leader_approval: false,
            request_id: None,
        });
        assert!(content.contains("User has approved your plan. You can now start coding."));
        assert!(content.contains("Your plan has been saved to: /tmp/plan.md"));
        assert!(content.contains("consider using the TeamCreate tool"));
        assert!(content.contains("## Approved Plan (edited by user):\n# Plan\n- implement"));
    }
}
