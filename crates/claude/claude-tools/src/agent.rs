//! Agent, send_message, and plan-mode tool implementations.

use std::collections::BTreeSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use claude_agents::fork::{
    ForkContentBlock, ForkMessage, fork_agent_definition, is_fork_subagent_enabled,
    is_in_fork_child,
};
use claude_agents::loader::load_all_agents_with_context;
use claude_agents::{AgentDefinition, AgentIsolation, AgentSource, compose_agent_system_prompt};
use claude_context::RuntimeIdentityContext;
use claude_core::PermissionMode;
use claude_mcp::normalization::mcp_info_from_string;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use claude_core::{SubAgentCompletion, SubAgentExecutionRequest};
use claude_permissions::{PermissionBroker, StaticPermissionBroker};

use super::ToolExecutionContext;
use crate::delegate::{DelegationConfig, DelegationContext, DelegationEngine};
use crate::shell::path_validation::resolve_working_dir;
use crate::tasks::{
    TaskKind, TaskStatus, allocate_task_id, create_background_task, finish_tracked_task,
    mark_task_running, start_tracked_task,
};
use crate::team_runtime::{LiveTeammateRegistration, finish_live_teammate, start_live_teammate};
use crate::{
    ToolSpec, current_runtime_agent_prompt_context, current_runtime_fork_snapshot,
    current_runtime_mcp_cli_state, runtime_provider_tool_specs,
};

const ALL_AGENT_DISALLOWED_TOOLS: &[&str] = &[
    "task_output",
    "exit_plan_mode",
    "enter_plan_mode",
    "agent",
    "ask_user",
    "workflow",
];

const CUSTOM_AGENT_DISALLOWED_TOOLS: &[&str] = ALL_AGENT_DISALLOWED_TOOLS;

const ASYNC_AGENT_ALLOWED_TOOLS: &[&str] = &[
    "read_file",
    "web_search",
    "todo_write",
    "grep",
    "web_fetch",
    "glob",
    "bash_command",
    "edit_file",
    "replace_in_file",
    "write_file",
    "notebook_edit",
    "skill_execute",
    "synthetic_output",
    "tool_search",
    "enter_worktree",
    "exit_worktree",
];

const REQUIRED_MCP_MAX_WAIT_MS: Duration = Duration::from_millis(30_000);
const REQUIRED_MCP_POLL_INTERVAL_MS: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DelegateProgressEvent {
    SubtaskStarted {
        task_id: String,
        parent_task_id: Option<String>,
        description: String,
        depth: u32,
    },
    SubtaskProgress {
        task_id: String,
        turn: u32,
        max_turns: u32,
        summary: String,
    },
    SubtaskCompleted {
        task_id: String,
        success: bool,
        output_preview: String,
        turns_used: u32,
    },
    BatchProgress {
        total: usize,
        completed: usize,
        running: usize,
    },
}

#[must_use]
pub fn parse_delegate_progress_event(message: &str) -> Option<DelegateProgressEvent> {
    serde_json::from_str(message).ok()
}

#[must_use]
pub fn render_delegate_progress_event(event: &DelegateProgressEvent) -> String {
    match event {
        DelegateProgressEvent::SubtaskStarted {
            task_id,
            description,
            depth,
            ..
        } => {
            let indent = "  ".repeat(*depth as usize);
            format!("{indent}🔹 [{task_id}] Started: {description}")
        }
        DelegateProgressEvent::SubtaskProgress {
            task_id,
            turn,
            summary,
            ..
        } => format!("  ⏳ [{task_id}] Turn {turn}: {summary}"),
        DelegateProgressEvent::SubtaskCompleted {
            task_id,
            success,
            turns_used,
            ..
        } => {
            let icon = if *success { "✅" } else { "❌" };
            format!("  {icon} [{task_id}] Completed ({turns_used} turns)")
        }
        DelegateProgressEvent::BatchProgress {
            completed, total, ..
        } => format!("  📊 Batch progress: {completed}/{total}"),
    }
}

/// Returns a boxed, `Send` future to break the recursive async chain:
/// `agent_tool → execute_tool_call → delegate_single → execute_tool_call → agent_tool`.
pub(crate) fn agent_tool<'a>(
    input: &'a Value,
    context: &'a ToolExecutionContext,
) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
    Box::pin(async move { agent_tool_inner(input, context).await })
}

#[derive(Debug, Clone, Deserialize)]
struct AgentToolInput {
    prompt: String,
    description: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    team_name: Option<String>,
    #[serde(default)]
    subagent_type: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    run_in_background: Option<bool>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    isolation: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    tasks: Vec<String>,
}

async fn agent_tool_inner(input: &Value, context: &ToolExecutionContext) -> Result<String> {
    let parsed: AgentToolInput = serde_json::from_value(input.clone())
        .map_err(|error| anyhow!("invalid agent tool input: {error}"))?;

    let mode = parsed.mode.as_deref().unwrap_or("single");

    // Resolve the sub-agent completion provider.
    let sub_agent = match &context.sub_agent {
        Some(provider) => provider.clone(),
        None => {
            let prompt = parsed.prompt.clone();
            return Ok(json!({
                "type": "sub_agent_request",
                "prompt": prompt,
                "description": parsed.description.clone(),
                "subagent_type": parsed.subagent_type.clone(),
                "model": parsed.model.clone(),
                "allowed_tools": parsed.tools.clone(),
                "message": format!(
                    "Sub-agent task: {}. [No provider available for sub-agent execution]",
                    parsed.prompt
                ),
            })
            .to_string());
        }
    };

    let fork_snapshot = implicit_fork_snapshot(&parsed);
    let resolved_definition = if mode == "batch" || !sub_agent.supports_agent_execution() {
        None
    } else if fork_snapshot.is_some() {
        Some(fork_agent_definition())
    } else {
        Some(resolve_agent_definition(
            parsed.subagent_type.as_deref(),
            &context.cwd,
        )?)
    };

    match mode {
        "batch" => run_batch_delegation(input, context, sub_agent, &parsed.tools).await,
        _ if sub_agent.supports_agent_execution()
            && ((fork_snapshot.is_some() && parsed.subagent_type.is_none())
                || parsed.run_in_background == Some(true)
                || resolved_definition
                    .as_ref()
                    .is_some_and(|definition| definition.background)) =>
        {
            run_background_agent_execution(
                &parsed,
                context,
                sub_agent,
                resolved_definition.expect("resolved definition"),
                fork_snapshot,
            )
            .await
        }
        _ if sub_agent.supports_agent_execution() => {
            run_resolved_agent_execution(
                &parsed,
                context,
                sub_agent,
                resolved_definition.expect("resolved definition"),
                fork_snapshot,
            )
            .await
        }
        _ => run_single_delegation(&parsed.prompt, context, sub_agent, &parsed.tools).await,
    }
}

fn implicit_fork_snapshot(input: &AgentToolInput) -> Option<claude_core::SubAgentForkSnapshot> {
    if input
        .subagent_type
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return None;
    }
    let runtime_context = current_runtime_agent_prompt_context().unwrap_or_default();
    if !is_fork_subagent_enabled(
        runtime_context.is_coordinator,
        runtime_context.is_non_interactive,
    ) {
        return None;
    }
    current_runtime_fork_snapshot()
}

fn snapshot_is_fork_child(snapshot: &claude_core::SubAgentForkSnapshot) -> bool {
    let messages = snapshot
        .fork_context_messages
        .iter()
        .filter_map(|message| match message {
            claude_core::Message::User(user) => Some(ForkMessage {
                role: "user".to_owned(),
                content: user
                    .provider_content_blocks()
                    .into_iter()
                    .filter_map(|block| {
                        let block_type = block.get("type").and_then(Value::as_str)?;
                        match block_type {
                            "text" => Some(ForkContentBlock::Text {
                                text: block.get("text").and_then(Value::as_str)?.to_owned(),
                            }),
                            "tool_result" => Some(ForkContentBlock::ToolResult {
                                tool_use_id: block
                                    .get("tool_use_id")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_owned(),
                                content: block
                                    .get("content")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_owned(),
                            }),
                            _ => None,
                        }
                    })
                    .collect(),
            }),
            _ => None,
        })
        .collect::<Vec<_>>();
    is_in_fork_child(&messages)
}

async fn run_resolved_agent_execution(
    input: &AgentToolInput,
    context: &ToolExecutionContext,
    sub_agent: Arc<dyn SubAgentCompletion>,
    definition: AgentDefinition,
    fork_snapshot: Option<claude_core::SubAgentForkSnapshot>,
) -> Result<String> {
    if fork_snapshot.as_ref().is_some_and(snapshot_is_fork_child) {
        return Err(anyhow!(
            "Fork is not available inside a forked worker. Complete your task directly using your tools."
        ));
    }
    ensure_agent_required_mcp_servers(&definition).await?;
    let working_dir = resolve_agent_working_dir(input, context, &definition)?;
    let permission_mode = resolve_agent_permission_mode(input.mode.as_deref(), &definition)?;
    let allowed_tools =
        resolve_agent_allowed_tools(&definition, &input.tools, false, permission_mode).await?;
    let title = if input.description.trim().is_empty() {
        truncate_str(&input.prompt, 80)
    } else {
        input.description.trim().to_owned()
    };
    let (task_id, parent_task_id, depth) = start_agent_tracking(context, &title)?;
    emit_delegate_event(
        context,
        DelegateProgressEvent::SubtaskStarted {
            task_id: task_id.clone(),
            parent_task_id: parent_task_id.clone(),
            description: title,
            depth,
        },
    );

    let live_teammate = match requested_teammate(input) {
        Some((agent_name, team_name)) => Some(
            start_live_teammate(&LiveTeammateRegistration {
                team_name,
                agent_name,
                agent_type: definition.agent_type.clone(),
                model: input.model.clone().or_else(|| definition.model.clone()),
                cwd: working_dir.clone(),
                permission_mode,
                objective: Some(input.description.clone()),
            })
            .await?,
        ),
        None => None,
    };

    let result = sub_agent
        .execute_agent(SubAgentExecutionRequest {
            agent_type: definition.agent_type.clone(),
            agent_name: live_teammate
                .as_ref()
                .map(|teammate| teammate.agent_name.clone()),
            team_name: live_teammate
                .as_ref()
                .map(|teammate| teammate.team_name.clone()),
            task: input.prompt.clone(),
            description: Some(input.description.clone()),
            context: Vec::new(),
            system_prompt: Some(compose_agent_system_prompt(&definition, None, &working_dir)),
            critical_system_reminder: definition.critical_system_reminder_experimental.clone(),
            omit_claude_md: definition.omit_claude_md,
            omit_git_status: matches!(definition.agent_type.as_str(), "Explore" | "Plan"),
            model: if fork_snapshot.is_some() {
                definition.model.clone()
            } else {
                input.model.clone().or_else(|| definition.model.clone())
            },
            max_turns: definition.max_turns,
            allowed_tools,
            permission_mode,
            working_dir,
            additional_working_directories: inherited_additional_working_directories(),
            skip_transcript: false,
            fork_snapshot,
        })
        .await;

    if let Some(handle) = live_teammate.as_ref() {
        finish_live_teammate(handle).await?;
    }

    match result {
        Ok(result) => {
            let output = if result.success {
                result.output
            } else {
                format!(
                    "Sub-agent failed after {} turns: {}",
                    result.turns, result.output
                )
            };
            finish_tracked_task(
                &task_id,
                if result.success {
                    TaskStatus::Completed
                } else {
                    TaskStatus::Failed
                },
                Some(&truncate_str(&output, 200)),
                &output,
                Some(result.turns),
            )?;
            emit_delegate_event(
                context,
                DelegateProgressEvent::SubtaskCompleted {
                    task_id,
                    success: result.success,
                    output_preview: truncate_str(&output, 200),
                    turns_used: result.turns,
                },
            );
            Ok(output)
        }
        Err(error) => {
            let output = error.to_string();
            finish_tracked_task(
                &task_id,
                TaskStatus::Failed,
                Some(&truncate_str(&output, 200)),
                &output,
                None,
            )?;
            emit_delegate_event(
                context,
                DelegateProgressEvent::SubtaskCompleted {
                    task_id,
                    success: false,
                    output_preview: truncate_str(&output, 200),
                    turns_used: 0,
                },
            );
            Err(error)
        }
    }
}

async fn run_background_agent_execution(
    input: &AgentToolInput,
    context: &ToolExecutionContext,
    sub_agent: Arc<dyn SubAgentCompletion>,
    definition: AgentDefinition,
    fork_snapshot: Option<claude_core::SubAgentForkSnapshot>,
) -> Result<String> {
    if requested_teammate(input).is_some() {
        return Err(anyhow!(
            "run_in_background is not supported together with name/team_name in this runtime"
        ));
    }
    if fork_snapshot.as_ref().is_some_and(snapshot_is_fork_child) {
        return Err(anyhow!(
            "Fork is not available inside a forked worker. Complete your task directly using your tools."
        ));
    }
    ensure_agent_required_mcp_servers(&definition).await?;
    let working_dir = resolve_agent_working_dir(input, context, &definition)?;
    let permission_mode = resolve_agent_permission_mode(input.mode.as_deref(), &definition)?;
    let allowed_tools =
        resolve_agent_allowed_tools(&definition, &input.tools, true, permission_mode).await?;
    let title = if input.description.trim().is_empty() {
        truncate_str(&input.prompt, 80)
    } else {
        input.description.trim().to_owned()
    };
    let task = create_background_task(&title)?;
    mark_task_running(&task.id, Some("Launching background agent"))?;

    let task_id = task.id.clone();
    let task_id_for_spawn = task_id.clone();
    let prompt = input.prompt.clone();
    let description = input.description.clone();
    let model = if fork_snapshot.is_some() {
        definition.model.clone()
    } else {
        input.model.clone().or_else(|| definition.model.clone())
    };
    let agent_type = definition.agent_type.clone();
    let additional_working_directories = inherited_additional_working_directories();
    let critical_system_reminder = definition.critical_system_reminder_experimental.clone();
    let max_turns = definition.max_turns;
    let system_prompt = compose_agent_system_prompt(&definition, None, &working_dir);
    tokio::spawn(async move {
        let result = sub_agent
            .execute_agent(SubAgentExecutionRequest {
                agent_type,
                agent_name: None,
                team_name: None,
                task: prompt,
                description: Some(description),
                context: Vec::new(),
                system_prompt: Some(system_prompt),
                critical_system_reminder,
                omit_claude_md: definition.omit_claude_md,
                omit_git_status: matches!(definition.agent_type.as_str(), "Explore" | "Plan"),
                model,
                max_turns,
                allowed_tools,
                permission_mode,
                working_dir,
                additional_working_directories,
                skip_transcript: false,
                fork_snapshot,
            })
            .await;

        match result {
            Ok(result) => {
                let output = if result.success {
                    result.output
                } else {
                    format!(
                        "Sub-agent failed after {} turns: {}",
                        result.turns, result.output
                    )
                };
                let status = if result.success {
                    TaskStatus::Completed
                } else {
                    TaskStatus::Failed
                };
                let _ = finish_tracked_task(
                    &task_id_for_spawn,
                    status,
                    Some(&truncate_str(&output, 200)),
                    &output,
                    Some(result.turns),
                );
            }
            Err(error) => {
                let output = error.to_string();
                let _ = finish_tracked_task(
                    &task_id_for_spawn,
                    TaskStatus::Failed,
                    Some(&truncate_str(&output, 200)),
                    &output,
                    None,
                );
            }
        }
    });

    Ok(json!({
        "status": "async_launched",
        "task_id": task_id,
        "description": title,
        "prompt": input.prompt.clone(),
        "message": "Background agent launched. Continue with other work and only check task output if the user explicitly asks for progress."
    })
    .to_string())
}

fn requested_teammate(input: &AgentToolInput) -> Option<(String, String)> {
    let agent_name = input
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let team_name = input
        .team_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("default-team");
    Some((agent_name.to_owned(), team_name.to_owned()))
}

fn resolve_agent_working_dir(
    input: &AgentToolInput,
    context: &ToolExecutionContext,
    definition: &AgentDefinition,
) -> Result<std::path::PathBuf> {
    let requested_isolation = input.isolation.as_deref().map(str::trim);
    let effective_isolation = match requested_isolation {
        Some("") | None => definition.isolation,
        Some("worktree") => AgentIsolation::Worktree,
        Some(other) => {
            return Err(anyhow!(
                "unsupported agent isolation `{other}`; expected worktree"
            ));
        }
    };

    if matches!(effective_isolation, AgentIsolation::Worktree) {
        if input.cwd.as_deref().is_some() {
            return Err(anyhow!(
                "`cwd` cannot be combined with isolation `worktree`"
            ));
        }
        return Err(anyhow!(
            "agent isolation `worktree` is not yet supported by this runtime; create or choose a dedicated working directory first"
        ));
    }

    resolve_working_dir(&context.cwd, input.cwd.as_deref())
}

fn resolve_agent_permission_mode(
    mode: Option<&str>,
    definition: &AgentDefinition,
) -> Result<Option<PermissionMode>> {
    match mode.unwrap_or("single") {
        "single" | "batch"
            if definition
                .permission_mode
                .as_deref()
                .is_some_and(|raw_mode| raw_mode.trim().eq_ignore_ascii_case("bubble")) =>
        {
            Ok(None)
        }
        "single" | "batch" => Ok(Some(agent_definition_permission_mode(definition)?)),
        "default" => Ok(Some(PermissionMode::Default)),
        "plan" => Ok(Some(PermissionMode::Plan)),
        other => Err(anyhow!(
            "unsupported agent mode `{other}`; expected single, batch, default, or plan"
        )),
    }
}

fn agent_definition_permission_mode(definition: &AgentDefinition) -> Result<PermissionMode> {
    let Some(raw_mode) = definition.permission_mode.as_deref() else {
        return Ok(PermissionMode::AcceptEdits);
    };
    parse_agent_permission_mode(raw_mode).ok_or_else(|| {
        anyhow!(
            "unsupported permissionMode `{}` for agent `{}`; expected default, acceptEdits, bypassPermissions, dontAsk, or plan",
            raw_mode,
            definition.agent_type
        )
    })
}

fn parse_agent_permission_mode(value: &str) -> Option<PermissionMode> {
    match value.trim() {
        "default" => Some(PermissionMode::Default),
        "acceptEdits" | "accept-edits" => Some(PermissionMode::AcceptEdits),
        "bypassPermissions" | "bypass-permissions" => Some(PermissionMode::BypassPermissions),
        "dontAsk" | "dont-ask" => Some(PermissionMode::DontAsk),
        "plan" => Some(PermissionMode::Plan),
        _ => None,
    }
}

/// Delegate a single task using the [`DelegationEngine`].
///
/// Uses the [`TaskStack`] from the execution context to track delegation
/// depth and enforce nesting limits.
async fn run_single_delegation(
    prompt: &str,
    context: &ToolExecutionContext,
    sub_agent: Arc<dyn SubAgentCompletion>,
    allowed_tools: &[String],
) -> Result<String> {
    // Determine current delegation depth from the task stack.
    let depth = {
        let stack = context.task_stack.lock();
        stack.depth()
    };

    let engine = DelegationEngine::new(DelegationConfig::default());

    let broker: Arc<dyn PermissionBroker> = Arc::new(StaticPermissionBroker::new(true));

    let delegation_ctx = DelegationContext {
        task: prompt.to_owned(),
        cwd: context.cwd.clone(),
        parent_conversation: Vec::new(),
        depth,
        task_metadata: None,
        allowed_tools: allowed_tools.to_vec(),
        tool_context: context.clone(),
        broker,
    };

    // Build a progress callback that prints to the frontend.
    let progress_cb = build_progress_callback(context);

    let result = engine
        .delegate_single(delegation_ctx, sub_agent, progress_cb)
        .await?;

    if result.success {
        Ok(result.output)
    } else {
        Ok(format!(
            "Sub-agent failed after {} turns: {}",
            result.turns_used, result.output
        ))
    }
}

/// Delegate multiple tasks in parallel using [`DelegationEngine::delegate_batch`].
async fn run_batch_delegation(
    input: &Value,
    context: &ToolExecutionContext,
    sub_agent: Arc<dyn SubAgentCompletion>,
    allowed_tools: &[String],
) -> Result<String> {
    let tasks: Vec<String> = input
        .get("tasks")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    if tasks.is_empty() {
        return Err(anyhow!("batch mode requires a non-empty 'tasks' array"));
    }

    let engine = DelegationEngine::new(DelegationConfig::default());
    let progress_cb = build_progress_callback(context);

    let broker: Arc<dyn PermissionBroker> = Arc::new(StaticPermissionBroker::new(true));

    let (batch_depth, parent_task_id) = {
        let stack = context.task_stack.lock();
        if let Some(frame) = stack.current() {
            (frame.depth.saturating_add(1), Some(frame.task_id.clone()))
        } else {
            (0, None)
        }
    };

    let results: Vec<crate::delegate::DelegationResult> = engine
        .delegate_batch(
            &tasks,
            sub_agent,
            &context.cwd,
            allowed_tools,
            batch_depth,
            parent_task_id,
            progress_cb,
            context.clone(),
            broker,
        )
        .await?;

    // Format results as a summary JSON.
    let summary: Vec<Value> = results
        .iter()
        .map(|r| {
            json!({
                "task": r.task,
                "success": r.success,
                "turns_used": r.turns_used,
                "output_preview": truncate_str(&r.output, 200),
            })
        })
        .collect();

    let succeeded = results.iter().filter(|r| r.success).count();
    Ok(json!({
        "type": "batch_delegation_result",
        "total": results.len(),
        "succeeded": succeeded,
        "failed": results.len() - succeeded,
        "results": summary,
    })
    .to_string())
}

/// Build an optional progress callback from the tool execution context.
///
/// Wraps the frontend-provided callback to format progress events as
/// human-readable strings suitable for the [`DelegationEngine`].
fn build_progress_callback(
    context: &ToolExecutionContext,
) -> Option<Arc<dyn Fn(claude_ui_bridge::UiEvent) + Send + Sync>> {
    context.progress_cb.as_ref().map(|cb| {
        let cb = cb.clone();
        Arc::new(move |event: claude_ui_bridge::UiEvent| {
            if let Some(serialized) = serialize_delegate_event(&event) {
                cb(&serialized);
            }
        }) as Arc<dyn Fn(claude_ui_bridge::UiEvent) + Send + Sync>
    })
}

fn serialize_delegate_event(event: &claude_ui_bridge::UiEvent) -> Option<String> {
    let envelope = match event {
        claude_ui_bridge::UiEvent::SubtaskStarted {
            task_id,
            parent_task_id,
            description,
            depth,
        } => DelegateProgressEvent::SubtaskStarted {
            task_id: task_id.clone(),
            parent_task_id: parent_task_id.clone(),
            description: description.clone(),
            depth: *depth,
        },
        claude_ui_bridge::UiEvent::SubtaskProgress {
            task_id,
            turn,
            max_turns,
            summary,
        } => DelegateProgressEvent::SubtaskProgress {
            task_id: task_id.clone(),
            turn: *turn,
            max_turns: *max_turns,
            summary: summary.clone(),
        },
        claude_ui_bridge::UiEvent::SubtaskCompleted {
            task_id,
            success,
            output_preview,
            turns_used,
        } => DelegateProgressEvent::SubtaskCompleted {
            task_id: task_id.clone(),
            success: *success,
            output_preview: output_preview.clone(),
            turns_used: *turns_used,
        },
        claude_ui_bridge::UiEvent::BatchProgress {
            total,
            completed,
            running,
        } => DelegateProgressEvent::BatchProgress {
            total: *total,
            completed: *completed,
            running: *running,
        },
        _ => return None,
    };
    serde_json::to_string(&envelope).ok()
}

fn emit_delegate_event(context: &ToolExecutionContext, event: DelegateProgressEvent) {
    if let Some(cb) = context.progress_cb.as_ref()
        && let Ok(serialized) = serde_json::to_string(&event)
    {
        cb(&serialized);
    }
}

fn resolve_agent_definition(subagent_type: Option<&str>, cwd: &Path) -> Result<AgentDefinition> {
    let runtime_context = current_runtime_agent_prompt_context();
    let user_agents_dir = runtime_context
        .as_ref()
        .and_then(|context| context.user_agents_dir.clone());
    let project_agents_dir = runtime_context
        .as_ref()
        .and_then(|context| context.project_agents_dir.clone())
        .unwrap_or_else(|| cwd.join(".claude").join("agents"));
    resolve_agent_definition_from_dirs(
        subagent_type,
        user_agents_dir.as_deref(),
        Some(project_agents_dir.as_path()),
    )
}

fn resolve_agent_definition_from_dirs(
    subagent_type: Option<&str>,
    user_dir: Option<&Path>,
    project_dir: Option<&Path>,
) -> Result<AgentDefinition> {
    let requested_type = subagent_type.unwrap_or("general-purpose");
    let runtime_identity = current_runtime_agent_prompt_context()
        .map(|context| context.runtime_identity)
        .unwrap_or_else(RuntimeIdentityContext::from_legacy_env);
    let definitions = load_all_agents_with_context(user_dir, project_dir, &runtime_identity);
    if let Some(definition) = find_agent_definition(&definitions.active_agents, requested_type) {
        return Ok(definition);
    }

    let mut available_agents = definitions
        .all_agents
        .into_iter()
        .map(|definition| definition.agent_type)
        .collect::<Vec<_>>();
    available_agents.sort();
    available_agents.dedup();

    let mut error = format!(
        "unknown subagent_type `{requested_type}`; available agents: {}",
        available_agents.join(", ")
    );
    if !definitions.failed_files.is_empty() {
        let failures = definitions
            .failed_files
            .into_iter()
            .map(|(path, reason)| format!("{path}: {reason}"))
            .collect::<Vec<_>>()
            .join("; ");
        error.push_str(&format!(". failed agent files: {failures}"));
    }

    Err(anyhow!(error))
}

fn find_agent_definition(
    definitions: &[AgentDefinition],
    requested_type: &str,
) -> Option<AgentDefinition> {
    definitions
        .iter()
        .find(|definition| definition.agent_type == requested_type)
        .cloned()
        .or_else(|| {
            let requested_key = normalize_agent_type_key(requested_type);
            definitions
                .iter()
                .find(|definition| {
                    normalize_agent_type_key(&definition.agent_type) == requested_key
                })
                .cloned()
        })
        .or_else(|| {
            agent_type_aliases(requested_type).iter().find_map(|alias| {
                definitions
                    .iter()
                    .find(|definition| {
                        normalize_agent_type_key(&definition.agent_type)
                            == normalize_agent_type_key(alias)
                    })
                    .cloned()
            })
        })
}

fn normalize_agent_type_key(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect()
}

fn agent_type_aliases(requested_type: &str) -> &'static [&'static str] {
    match normalize_agent_type_key(requested_type).as_str() {
        "explore" | "explorer" => &["Explore"],
        "plan" | "planner" => &["Plan"],
        "verify" | "verifier" => &["verification"],
        "worker" => &["worker", "general-purpose"],
        "review" | "reviewer" => &["code-reviewer", "project-reviewer", "verification"],
        "general" | "generalpurpose" | "default" => &["general-purpose"],
        _ => &[],
    }
}

async fn resolve_agent_allowed_tools(
    definition: &AgentDefinition,
    requested_tools: &[String],
    is_async: bool,
    permission_mode: Option<PermissionMode>,
) -> Result<Vec<String>> {
    let specs = runtime_provider_tool_specs().await;
    let filtered_by_definition =
        filter_tools_for_agent_runtime(&specs, definition, is_async, permission_mode)?;
    let filtered_by_definition =
        apply_agent_tool_allowlist(&filtered_by_definition, &definition.tools, true)?;
    let filtered_by_request = if requested_tools.is_empty() {
        filtered_by_definition
    } else {
        apply_agent_tool_allowlist(&filtered_by_definition, requested_tools, false)?
    };

    let denied = collect_matching_tool_names(&filtered_by_request, &definition.disallowed_tools);
    Ok(collect_selected_tool_names(filtered_by_request, &denied))
}

async fn ensure_agent_required_mcp_servers(definition: &AgentDefinition) -> Result<()> {
    if definition.required_mcp_servers.is_empty() {
        return Ok(());
    }

    if current_runtime_mcp_cli_state().is_some() {
        return ensure_agent_required_mcp_servers_with_live_state(definition).await;
    }

    let specs = runtime_provider_tool_specs().await;
    ensure_agent_required_mcp_servers_with_specs(definition, &specs)
}

async fn ensure_agent_required_mcp_servers_with_live_state(
    definition: &AgentDefinition,
) -> Result<()> {
    let mut state = current_runtime_mcp_cli_state().unwrap_or_default();
    if has_pending_required_mcp_servers(&state, &definition.required_mcp_servers) {
        let deadline = tokio::time::Instant::now() + REQUIRED_MCP_MAX_WAIT_MS;
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(REQUIRED_MCP_POLL_INTERVAL_MS).await;
            let Some(current) = current_runtime_mcp_cli_state() else {
                break;
            };
            state = current;
            if has_failed_required_mcp_servers(&state, &definition.required_mcp_servers)
                || !has_pending_required_mcp_servers(&state, &definition.required_mcp_servers)
            {
                break;
            }
        }
    }

    ensure_agent_required_mcp_servers_with_server_names(
        definition,
        &mcp_servers_with_tools_from_cli_state(&state),
    )
}

fn ensure_agent_required_mcp_servers_with_specs(
    definition: &AgentDefinition,
    specs: &[ToolSpec],
) -> Result<()> {
    if definition.required_mcp_servers.is_empty() {
        return Ok(());
    }

    ensure_agent_required_mcp_servers_with_server_names(definition, &mcp_servers_with_tools(specs))
}

fn ensure_agent_required_mcp_servers_with_server_names(
    definition: &AgentDefinition,
    servers_with_tools: &[String],
) -> Result<()> {
    let missing =
        missing_required_mcp_servers(&definition.required_mcp_servers, servers_with_tools);
    if missing.is_empty() {
        return Ok(());
    }

    Err(anyhow!(
        "Agent '{}' requires MCP servers matching: {}. MCP servers with tools: {}. Use /mcp to configure and authenticate the required MCP servers.",
        definition.agent_type,
        missing.join(", "),
        if servers_with_tools.is_empty() {
            "none".to_owned()
        } else {
            servers_with_tools.join(", ")
        }
    ))
}

fn mcp_servers_with_tools(specs: &[ToolSpec]) -> Vec<String> {
    let mut servers = Vec::new();
    for spec in specs {
        let Some(info) = mcp_info_from_string(&spec.name) else {
            continue;
        };
        if servers.iter().any(|server| server == &info.server_name) {
            continue;
        }
        servers.push(info.server_name);
    }
    servers
}

fn mcp_servers_with_tools_from_cli_state(state: &claude_mcp::McpCliState) -> Vec<String> {
    let mut servers = Vec::new();
    for tool in &state.tools {
        let Some(info) = mcp_info_from_string(&tool.name) else {
            continue;
        };
        if servers.iter().any(|server| server == &info.server_name) {
            continue;
        }
        servers.push(info.server_name);
    }
    servers
}

fn has_pending_required_mcp_servers(
    state: &claude_mcp::McpCliState,
    required_patterns: &[String],
) -> bool {
    state.clients.iter().any(|client| {
        client.connection_type == "pending"
            && required_patterns
                .iter()
                .any(|pattern| server_matches_required_pattern(&client.name, pattern))
    })
}

fn has_failed_required_mcp_servers(
    state: &claude_mcp::McpCliState,
    required_patterns: &[String],
) -> bool {
    state.clients.iter().any(|client| {
        client.connection_type == "failed"
            && required_patterns
                .iter()
                .any(|pattern| server_matches_required_pattern(&client.name, pattern))
    })
}

fn missing_required_mcp_servers(required: &[String], servers_with_tools: &[String]) -> Vec<String> {
    required
        .iter()
        .filter(|pattern| {
            !servers_with_tools
                .iter()
                .any(|server| server_matches_required_pattern(server, pattern))
        })
        .cloned()
        .collect()
}

fn server_matches_required_pattern(server_name: &str, pattern: &str) -> bool {
    server_name
        .to_ascii_lowercase()
        .contains(&pattern.to_ascii_lowercase())
}

fn filter_tools_for_agent_runtime(
    specs: &[ToolSpec],
    definition: &AgentDefinition,
    is_async: bool,
    permission_mode: Option<PermissionMode>,
) -> Result<Vec<ToolSpec>> {
    let exit_plan_mode_allowed = permission_mode == Some(PermissionMode::Plan);
    let globally_denied = collect_matching_tool_names(
        specs,
        &disallowed_agent_tool_patterns(ALL_AGENT_DISALLOWED_TOOLS, exit_plan_mode_allowed),
    );
    let mut filtered = specs
        .iter()
        .filter(|spec| !globally_denied.contains(&spec.name))
        .cloned()
        .collect::<Vec<_>>();

    if definition.source != AgentSource::BuiltIn {
        let custom_denied = collect_matching_tool_names(
            &filtered,
            &disallowed_agent_tool_patterns(CUSTOM_AGENT_DISALLOWED_TOOLS, exit_plan_mode_allowed),
        );
        filtered.retain(|spec| !custom_denied.contains(&spec.name));
    }

    if is_async {
        let async_allowed = collect_matching_tool_names(
            &filtered,
            &ASYNC_AGENT_ALLOWED_TOOLS
                .iter()
                .map(|tool| (*tool).to_owned())
                .collect::<Vec<_>>(),
        );
        filtered.retain(|spec| {
            spec.name.starts_with("mcp__")
                || async_allowed.contains(&spec.name)
                || (exit_plan_mode_allowed && spec.name == "exit_plan_mode")
        });
    }

    Ok(filtered)
}

fn disallowed_agent_tool_patterns(patterns: &[&str], exit_plan_mode_allowed: bool) -> Vec<String> {
    patterns
        .iter()
        .filter(|tool| !(exit_plan_mode_allowed && **tool == "exit_plan_mode"))
        .map(|tool| (*tool).to_owned())
        .collect()
}

fn collect_selected_tool_names(specs: Vec<ToolSpec>, denied: &BTreeSet<String>) -> Vec<String> {
    let mut selected = Vec::new();
    let mut seen = BTreeSet::new();

    for spec in specs {
        if denied.contains(&spec.name) {
            continue;
        }
        if seen.insert(spec.name.clone()) {
            selected.push(spec.name);
        }
    }

    selected
}

fn apply_agent_tool_allowlist(
    specs: &[ToolSpec],
    allowlist: &[String],
    wildcard_on_empty: bool,
) -> Result<Vec<ToolSpec>> {
    if allowlist.is_empty() && wildcard_on_empty {
        return Ok(specs.to_vec());
    }
    if allowlist.len() == 1 && allowlist[0] == "*" {
        return Ok(specs.to_vec());
    }

    let matched = collect_matching_tool_names(specs, allowlist);
    let unknown = allowlist
        .iter()
        .filter(|requested| !matches_any_tool_alias(specs, requested))
        .cloned()
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        return Err(anyhow!(
            "unknown agent tool name(s): {}",
            unknown.join(", ")
        ));
    }

    Ok(specs
        .iter()
        .filter(|spec| matched.contains(&spec.name))
        .cloned()
        .collect())
}

fn collect_matching_tool_names(specs: &[ToolSpec], requested: &[String]) -> BTreeSet<String> {
    let requested = requested
        .iter()
        .map(|tool| tool.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    specs
        .iter()
        .filter(|spec| {
            tool_aliases(spec)
                .iter()
                .any(|alias| requested.contains(&alias.to_ascii_lowercase()))
        })
        .map(|spec| spec.name.clone())
        .collect()
}

fn matches_any_tool_alias(specs: &[ToolSpec], requested: &str) -> bool {
    let requested = requested.to_ascii_lowercase();
    specs.iter().any(|spec| {
        tool_aliases(spec)
            .iter()
            .any(|alias| alias.eq_ignore_ascii_case(&requested))
    })
}

fn tool_aliases(spec: &ToolSpec) -> BTreeSet<String> {
    let mut aliases = BTreeSet::from([
        spec.name.clone(),
        spec.protocol_name.clone(),
        spec.permission_tool_name.clone(),
        spec.provider_wire_name().to_owned(),
    ]);
    match spec.name.as_str() {
        "read_file" => {
            aliases.insert("Read".to_owned());
        }
        "write_file" => {
            aliases.insert("Write".to_owned());
        }
        "edit_file" | "replace_in_file" => {
            aliases.insert("Edit".to_owned());
        }
        "bash_command" => {
            aliases.insert("Bash".to_owned());
        }
        "glob" => {
            aliases.insert("Glob".to_owned());
        }
        "grep" => {
            aliases.insert("Grep".to_owned());
        }
        "agent" => {
            aliases.insert("Agent".to_owned());
        }
        "send_message" => {
            aliases.insert("SendMessage".to_owned());
        }
        "synthetic_output" => {
            aliases.insert("SyntheticOutput".to_owned());
        }
        "ask_user" => {
            aliases.insert("AskUserQuestion".to_owned());
        }
        "enter_plan_mode" => {
            aliases.insert("EnterPlanMode".to_owned());
        }
        "exit_plan_mode" => {
            aliases.insert("ExitPlanMode".to_owned());
        }
        _ => {}
    }
    aliases
}

fn start_agent_tracking(
    context: &ToolExecutionContext,
    title: &str,
) -> Result<(String, Option<String>, u32)> {
    let (parent_task_id, depth) = {
        let stack = context.task_stack.lock();
        if let Some(frame) = stack.current() {
            (Some(frame.task_id.clone()), frame.depth.saturating_add(1))
        } else {
            (None, 0)
        }
    };
    let task_id = allocate_task_id();
    start_tracked_task(
        task_id.clone(),
        title,
        parent_task_id.clone(),
        depth,
        TaskKind::Delegation,
        Some("started"),
    )?;
    Ok((task_id, parent_task_id, depth))
}

fn inherited_additional_working_directories() -> Vec<PathBuf> {
    current_runtime_agent_prompt_context()
        .map(|context| context.additional_working_directories)
        .unwrap_or_default()
}

/// Truncate a string to `max_bytes` bytes, respecting UTF-8 boundaries.
fn truncate_str(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        text.to_owned()
    } else {
        let boundary = text
            .char_indices()
            .take_while(|(i, _)| *i < max_bytes)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(max_bytes.min(text.len()));
        format!("{}...", &text[..boundary])
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex as StdMutex;
    use std::path::PathBuf;
    use std::sync::Arc;

    use claude_core::{ProviderResponse, UsageSummary};
    use claude_mcp::serialization::{McpCliState, SerializedClient, SerializedTool};
    use claude_swarm::team_helpers;
    use tempfile::tempdir;

    #[derive(Clone)]
    struct RecordingAgentRuntime {
        requests: Arc<StdMutex<Vec<SubAgentExecutionRequest>>>,
        result: claude_core::SubAgentExecutionResult,
    }

    #[async_trait::async_trait]
    impl SubAgentCompletion for RecordingAgentRuntime {
        async fn complete(
            &self,
            _conversation: &[claude_core::ConversationEntry],
        ) -> Result<ProviderResponse> {
            panic!("complete() should not be used when execute_agent is supported")
        }

        fn supports_agent_execution(&self) -> bool {
            true
        }

        async fn execute_agent(
            &self,
            request: SubAgentExecutionRequest,
        ) -> Result<claude_core::SubAgentExecutionResult> {
            self.requests.lock().push(request);
            Ok(self.result.clone())
        }
    }

    fn test_context_with_cwd(
        cwd: PathBuf,
        sub_agent: Option<Arc<dyn SubAgentCompletion>>,
    ) -> ToolExecutionContext {
        ToolExecutionContext {
            original_cwd: cwd.clone(),
            cwd,
            active_worktree_session: None,
            timeout_ms: 5_000,
            sub_agent,
            progress_cb: None,
            task_stack: Default::default(),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    fn test_context(sub_agent: Option<Arc<dyn SubAgentCompletion>>) -> ToolExecutionContext {
        let tempdir = tempdir().expect("tempdir");
        let cwd = tempdir.path().to_path_buf();
        std::mem::forget(tempdir);
        test_context_with_cwd(cwd, sub_agent)
    }

    #[test]
    fn format_started_event() {
        let event = DelegateProgressEvent::SubtaskStarted {
            task_id: "t1".into(),
            parent_task_id: Some("root".into()),
            description: "fix bug".into(),
            depth: 1,
        };
        assert!(render_delegate_progress_event(&event).contains("Started"));
    }

    #[test]
    fn format_completed_event() {
        let event = DelegateProgressEvent::SubtaskCompleted {
            task_id: "t1".into(),
            success: true,
            turns_used: 3,
            output_preview: "done".into(),
        };
        let label = render_delegate_progress_event(&event);
        assert!(label.contains("✅"));
        assert!(label.contains("3 turns"));
    }

    #[test]
    fn format_batch_progress_event() {
        let event = DelegateProgressEvent::BatchProgress {
            completed: 2,
            total: 5,
            running: 1,
        };
        let label = render_delegate_progress_event(&event);
        assert!(label.contains("2/5"));
    }

    #[test]
    fn delegate_progress_round_trips_json() {
        let event = DelegateProgressEvent::SubtaskStarted {
            task_id: "t1".into(),
            parent_task_id: Some("root".into()),
            description: "fix bug".into(),
            depth: 1,
        };
        let json = serde_json::to_string(&event).expect("serialize event");
        let parsed = parse_delegate_progress_event(&json).expect("parse event");
        match parsed {
            DelegateProgressEvent::SubtaskStarted { parent_task_id, .. } => {
                assert_eq!(parent_task_id.as_deref(), Some("root"));
            }
            _ => panic!("expected started event"),
        }
    }

    #[tokio::test]
    async fn verification_agent_runs_in_background_by_default() {
        let requests = Arc::new(StdMutex::new(Vec::new()));
        let runtime: Arc<dyn SubAgentCompletion> = Arc::new(RecordingAgentRuntime {
            requests: Arc::clone(&requests),
            result: claude_core::SubAgentExecutionResult {
                output: "verified".to_owned(),
                success: true,
                turns: 4,
                usage: UsageSummary::default(),
            },
        });
        let context = test_context(Some(runtime));
        let runtime_context = crate::RuntimeAgentPromptContext {
            runtime_identity: RuntimeIdentityContext {
                features: claude_context::RuntimeFeatureGates {
                    verification_agent_enabled: true,
                    ..claude_context::RuntimeFeatureGates::default()
                },
                ..RuntimeIdentityContext::from_legacy_env()
            },
            ..crate::RuntimeAgentPromptContext::default()
        };

        let result = crate::with_runtime_agent_prompt_context_provider(
            Arc::new(move || runtime_context.clone()),
            async {
                agent_tool_inner(
                    &json!({
                        "prompt": "Review the recent Rust refactor for regressions.",
                        "description": "Verify refactor",
                        "subagent_type": "verification"
                    }),
                    &context,
                )
                .await
            },
        )
        .await
        .expect("agent tool should succeed");

        let payload: Value = serde_json::from_str(&result).expect("background payload");
        assert_eq!(payload["status"], "async_launched");
        for _ in 0..20 {
            if requests.lock().len() == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let requests = requests.lock();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.agent_type, "verification");
        assert_eq!(request.max_turns, 200);
        assert!(!request.skip_transcript);
        assert!(
            request
                .system_prompt
                .as_deref()
                .unwrap_or_default()
                .contains("verification specialist")
        );
        assert!(request.allowed_tools.contains(&"read_file".to_owned()));
        assert!(!request.allowed_tools.contains(&"write_file".to_owned()));
        assert!(!request.allowed_tools.contains(&"edit_file".to_owned()));
        assert!(
            !request
                .allowed_tools
                .contains(&"replace_in_file".to_owned())
        );
        assert!(!request.allowed_tools.contains(&"agent".to_owned()));
    }

    #[tokio::test]
    async fn resolved_agent_execution_defaults_to_general_purpose_when_fork_disabled() {
        let requests = Arc::new(StdMutex::new(Vec::new()));
        let runtime: Arc<dyn SubAgentCompletion> = Arc::new(RecordingAgentRuntime {
            requests: Arc::clone(&requests),
            result: claude_core::SubAgentExecutionResult {
                output: "implemented".to_owned(),
                success: true,
                turns: 2,
                usage: UsageSummary::default(),
            },
        });
        let context = test_context(Some(runtime));
        let runtime_context = crate::RuntimeAgentPromptContext {
            is_non_interactive: true,
            ..crate::RuntimeAgentPromptContext::default()
        };

        let result = crate::with_runtime_agent_prompt_context_provider(
            Arc::new(move || runtime_context.clone()),
            async {
                agent_tool_inner(
                    &json!({
                        "prompt": "Investigate the code path and make the required change.",
                        "description": "Implement fix"
                    }),
                    &context,
                )
                .await
            },
        )
        .await
        .expect("agent tool should succeed");

        assert_eq!(result, "implemented");
        let requests = requests.lock();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.agent_type, "general-purpose");
        assert_eq!(request.max_turns, 200);
        assert!(!request.skip_transcript);
        assert_eq!(request.permission_mode, Some(PermissionMode::AcceptEdits));
        assert!(
            request
                .system_prompt
                .as_deref()
                .unwrap_or_default()
                .contains("Complete the task fully")
        );
        assert!(!request.allowed_tools.contains(&"agent".to_owned()));
        assert!(request.allowed_tools.contains(&"write_file".to_owned()));
    }

    #[tokio::test]
    async fn resolved_agent_execution_omits_subagent_type_to_fork_when_enabled() {
        let requests = Arc::new(StdMutex::new(Vec::new()));
        let runtime: Arc<dyn SubAgentCompletion> = Arc::new(RecordingAgentRuntime {
            requests: Arc::clone(&requests),
            result: claude_core::SubAgentExecutionResult {
                output: "implemented".to_owned(),
                success: true,
                turns: 2,
                usage: UsageSummary::default(),
            },
        });
        let context = test_context(Some(runtime));
        let runtime_context = crate::RuntimeAgentPromptContext::default();
        let fork_snapshot = claude_core::SubAgentForkSnapshot {
            fork_context_messages: vec![
                claude_core::Message::from(claude_core::ConversationEntry::system(
                    "Parent system prompt.",
                )),
                claude_core::Message::from(claude_core::ConversationEntry::user(
                    "Investigate the code path and make the required change.",
                )),
            ],
            system_prompt: Some("Parent system prompt.".to_owned()),
            user_context: std::collections::BTreeMap::new(),
            system_context: std::collections::BTreeMap::new(),
        };

        let result = crate::with_runtime_agent_prompt_context_provider(
            Arc::new(move || runtime_context.clone()),
            async {
                crate::with_runtime_fork_snapshot_provider(
                    Arc::new(move || fork_snapshot.clone()),
                    async {
                        agent_tool_inner(
                            &json!({
                                "prompt": "Implement the change directly.",
                                "description": "Implement fix"
                            }),
                            &context,
                        )
                        .await
                    },
                )
                .await
            },
        )
        .await
        .expect("agent tool should succeed");

        let payload: Value = serde_json::from_str(&result).expect("background payload");
        assert_eq!(payload["status"], "async_launched");
        for _ in 0..20 {
            if requests.lock().len() == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let requests = requests.lock();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.agent_type, "fork");
        assert!(request.fork_snapshot.is_some());
        assert_eq!(request.model.as_deref(), Some("inherit"));
        assert_eq!(request.permission_mode, None);
    }

    #[tokio::test]
    async fn implicit_fork_ignores_model_override_and_inherits_parent_model() {
        let requests = Arc::new(StdMutex::new(Vec::new()));
        let runtime: Arc<dyn SubAgentCompletion> = Arc::new(RecordingAgentRuntime {
            requests: Arc::clone(&requests),
            result: claude_core::SubAgentExecutionResult {
                output: "forked".to_owned(),
                success: true,
                turns: 1,
                usage: UsageSummary::default(),
            },
        });
        let context = test_context(Some(runtime));
        let fork_snapshot = claude_core::SubAgentForkSnapshot {
            fork_context_messages: vec![claude_core::Message::from(
                claude_core::ConversationEntry::user("Parent prompt."),
            )],
            system_prompt: Some("Parent system prompt.".to_owned()),
            user_context: std::collections::BTreeMap::new(),
            system_context: std::collections::BTreeMap::new(),
        };

        let result = crate::with_runtime_fork_snapshot_provider(
            Arc::new(move || fork_snapshot.clone()),
            async {
                agent_tool_inner(
                    &json!({
                        "prompt": "Implement the change directly.",
                        "description": "Implement fix",
                        "model": "sonnet"
                    }),
                    &context,
                )
                .await
            },
        )
        .await
        .expect("implicit fork should ignore model override");

        let payload: Value = serde_json::from_str(&result).expect("background payload");
        assert_eq!(payload["status"], "async_launched");
        for _ in 0..20 {
            if requests.lock().len() == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let requests = requests.lock();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.agent_type, "fork");
        assert_eq!(request.model.as_deref(), Some("inherit"));
    }

    #[tokio::test]
    async fn implicit_fork_rejects_recursive_forking_inside_fork_child() {
        let requests = Arc::new(StdMutex::new(Vec::new()));
        let runtime: Arc<dyn SubAgentCompletion> = Arc::new(RecordingAgentRuntime {
            requests,
            result: claude_core::SubAgentExecutionResult {
                output: String::new(),
                success: true,
                turns: 1,
                usage: UsageSummary::default(),
            },
        });
        let context = test_context(Some(runtime));
        let fork_snapshot = claude_core::SubAgentForkSnapshot {
            fork_context_messages: vec![claude_core::Message::from(
                claude_core::ConversationEntry {
                    uuid: uuid::Uuid::new_v4(),
                    role: claude_core::ConversationRole::User,
                    text: String::new(),
                    history_text: None,
                    content_blocks: vec![json!({
                        "type": "text",
                        "text": "<fork-boilerplate>already a fork</fork-boilerplate>\n\nYour directive: stay focused"
                    })],
                    tool_calls: Vec::new(),
                    attachments: Vec::new(),
                    tool_call_id: None,
                    name: None,
                    is_error: false,
                },
            )],
            system_prompt: Some("Parent system prompt.".to_owned()),
            user_context: std::collections::BTreeMap::new(),
            system_context: std::collections::BTreeMap::new(),
        };

        let error = crate::with_runtime_fork_snapshot_provider(
            Arc::new(move || fork_snapshot.clone()),
            async {
                agent_tool_inner(
                    &json!({
                        "prompt": "Try to fork again.",
                        "description": "Recursive fork"
                    }),
                    &context,
                )
                .await
            },
        )
        .await
        .expect_err("recursive implicit fork should fail");

        assert!(
            error
                .to_string()
                .contains("Fork is not available inside a forked worker")
        );
    }

    #[tokio::test]
    async fn unknown_subagent_type_is_rejected() {
        let requests = Arc::new(StdMutex::new(Vec::new()));
        let runtime: Arc<dyn SubAgentCompletion> = Arc::new(RecordingAgentRuntime {
            requests,
            result: claude_core::SubAgentExecutionResult {
                output: String::new(),
                success: true,
                turns: 1,
                usage: UsageSummary::default(),
            },
        });
        let context = test_context(Some(runtime));

        let error = agent_tool_inner(
            &json!({
                "prompt": "Do work.",
                "description": "Unknown agent",
                "subagent_type": "unknown-agent"
            }),
            &context,
        )
        .await
        .expect_err("unknown agent type should fail");

        assert!(error.to_string().contains("unknown subagent_type"));
    }

    #[test]
    fn agent_definition_permission_mode_defaults_to_accept_edits_like_research() {
        let definition = AgentDefinition::new("worker", "Do work");

        assert_eq!(
            resolve_agent_permission_mode(Some("single"), &definition)
                .expect("default agent permission mode"),
            Some(PermissionMode::AcceptEdits)
        );
    }

    #[test]
    fn agent_definition_permission_mode_uses_agent_frontmatter() {
        let mut definition = AgentDefinition::new("guide", "Guide");
        definition.permission_mode = Some("dontAsk".to_owned());

        assert_eq!(
            resolve_agent_permission_mode(Some("single"), &definition)
                .expect("definition permission mode"),
            Some(PermissionMode::DontAsk)
        );
        assert_eq!(
            resolve_agent_permission_mode(Some("plan"), &definition)
                .expect("explicit spawn mode should still override"),
            Some(PermissionMode::Plan)
        );
    }

    #[test]
    fn filter_tools_for_agent_runtime_keeps_exit_plan_mode_for_plan_agents() {
        let definition = AgentDefinition::new("planner", "Plan work");
        let specs = vec![
            fake_tool_spec("exit_plan_mode"),
            fake_tool_spec("read_file"),
            fake_tool_spec("agent"),
        ];

        let filtered =
            filter_tools_for_agent_runtime(&specs, &definition, false, Some(PermissionMode::Plan))
                .expect("filter tools");

        let names = filtered
            .into_iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>();
        assert!(names.contains(&"exit_plan_mode".to_owned()));
        assert!(!names.contains(&"agent".to_owned()));
    }

    #[test]
    fn filter_tools_for_agent_runtime_removes_discover_skills_for_async_agents() {
        let definition = AgentDefinition::new("worker", "Do work");
        let specs = vec![
            fake_tool_spec("discover_skills"),
            fake_tool_spec("skill_execute"),
            fake_tool_spec("mcp__context7__query_docs"),
        ];

        let filtered = filter_tools_for_agent_runtime(
            &specs,
            &definition,
            true,
            Some(PermissionMode::AcceptEdits),
        )
        .expect("filter tools");

        let names = filtered
            .into_iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>();
        assert!(!names.contains(&"discover_skills".to_owned()));
        assert!(names.contains(&"skill_execute".to_owned()));
        assert!(names.contains(&"mcp__context7__query_docs".to_owned()));
    }

    #[test]
    fn collect_selected_tool_names_preserves_first_seen_order() {
        let specs = vec![
            fake_tool_spec("write_file"),
            fake_tool_spec("read_file"),
            fake_tool_spec("write_file"),
        ];

        let selected = collect_selected_tool_names(specs, &BTreeSet::new());
        assert_eq!(selected, vec!["write_file", "read_file"]);
    }

    #[test]
    fn required_mcp_servers_match_mcp_tool_server_names_like_research() {
        let mut definition = AgentDefinition::new("docs-agent", "Use docs");
        definition.required_mcp_servers = vec!["context".to_owned(), "MINI".to_owned()];
        let specs = vec![
            fake_tool_spec("mcp__context7__query_docs"),
            fake_tool_spec("mcp__MiniMax__plan"),
            fake_tool_spec("read_file"),
        ];

        ensure_agent_required_mcp_servers_with_specs(&definition, &specs)
            .expect("case-insensitive substring MCP requirements should pass");
    }

    #[test]
    fn required_mcp_servers_error_matches_research_wording() {
        let mut definition = AgentDefinition::new("docs-agent", "Use docs");
        definition.required_mcp_servers = vec!["MiniMax".to_owned(), "context".to_owned()];
        let specs = vec![fake_tool_spec("mcp__context7__query_docs")];

        let error = ensure_agent_required_mcp_servers_with_specs(&definition, &specs)
            .expect_err("missing required MCP server should fail before agent launch")
            .to_string();

        assert_eq!(
            error,
            "Agent 'docs-agent' requires MCP servers matching: MiniMax. MCP servers with tools: context7. Use /mcp to configure and authenticate the required MCP servers."
        );
    }

    #[test]
    fn required_mcp_servers_report_none_when_no_mcp_tools_are_visible() {
        let mut definition = AgentDefinition::new("docs-agent", "Use docs");
        definition.required_mcp_servers = vec!["context7".to_owned()];
        let specs = vec![fake_tool_spec("read_file"), fake_tool_spec("bash_command")];

        let error = ensure_agent_required_mcp_servers_with_specs(&definition, &specs)
            .expect_err("missing required MCP server should fail before agent launch")
            .to_string();

        assert_eq!(
            error,
            "Agent 'docs-agent' requires MCP servers matching: context7. MCP servers with tools: none. Use /mcp to configure and authenticate the required MCP servers."
        );
    }

    #[test]
    fn required_mcp_servers_ignore_malformed_mcp_tool_names() {
        let mut definition = AgentDefinition::new("docs-agent", "Use docs");
        definition.required_mcp_servers = vec!["srv".to_owned()];
        let specs = vec![
            fake_tool_spec("mcp__srv"),
            fake_tool_spec("mcp____lookup"),
            fake_tool_spec("mcp__srv__"),
        ];

        let error = ensure_agent_required_mcp_servers_with_specs(&definition, &specs)
            .expect_err("malformed MCP tool names must not satisfy server requirements")
            .to_string();

        assert!(error.contains("MCP servers with tools: none"));
    }

    #[tokio::test]
    async fn required_mcp_servers_wait_for_pending_live_state_then_pass() {
        let mut definition = AgentDefinition::new("docs-agent", "Use docs");
        definition.required_mcp_servers = vec!["context7".to_owned()];
        let state = Arc::new(StdMutex::new(McpCliState {
            clients: vec![serialized_client("context7", "pending")],
            ..McpCliState::default()
        }));
        let state_for_update = Arc::clone(&state);

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(25)).await;
            {
                let mut snapshot = state_for_update.lock();
                snapshot.clients = vec![serialized_client("context7", "connected")];
                snapshot.tools = vec![serialized_tool("mcp__context7__query_docs")];
            }
        });

        crate::with_runtime_mcp_state_provider(live_mcp_provider(state), async {
            ensure_agent_required_mcp_servers(&definition).await
        })
        .await
        .expect("pending required MCP should succeed after tools appear");
    }

    #[tokio::test]
    async fn required_mcp_servers_stop_waiting_when_live_state_fails() {
        let mut definition = AgentDefinition::new("docs-agent", "Use docs");
        definition.required_mcp_servers = vec!["context7".to_owned()];
        let state = Arc::new(StdMutex::new(McpCliState {
            clients: vec![serialized_client("context7", "pending")],
            ..McpCliState::default()
        }));
        let state_for_update = Arc::clone(&state);

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(25)).await;
            {
                let mut snapshot = state_for_update.lock();
                snapshot.clients = vec![serialized_client("context7", "failed")];
            }
        });

        let error = tokio::time::timeout(
            Duration::from_secs(1),
            crate::with_runtime_mcp_state_provider(live_mcp_provider(state), async {
                ensure_agent_required_mcp_servers(&definition).await
            }),
        )
        .await
        .expect("failed required MCP should stop waiting early")
        .expect_err("failed live MCP should still use missing-tools error")
        .to_string();

        assert_eq!(
            error,
            "Agent 'docs-agent' requires MCP servers matching: context7. MCP servers with tools: none. Use /mcp to configure and authenticate the required MCP servers."
        );
    }

    #[tokio::test]
    async fn required_mcp_servers_needs_auth_live_state_uses_standard_missing_error() {
        let mut definition = AgentDefinition::new("docs-agent", "Use docs");
        definition.required_mcp_servers = vec!["context7".to_owned()];
        let state = Arc::new(StdMutex::new(McpCliState {
            clients: vec![serialized_client("context7", "needs-auth")],
            ..McpCliState::default()
        }));

        let error = crate::with_runtime_mcp_state_provider(live_mcp_provider(state), async {
            ensure_agent_required_mcp_servers(&definition).await
        })
        .await
        .expect_err("needs-auth without tools should fail")
        .to_string();

        assert_eq!(
            error,
            "Agent 'docs-agent' requires MCP servers matching: context7. MCP servers with tools: none. Use /mcp to configure and authenticate the required MCP servers."
        );
    }

    #[test]
    fn resolve_agent_definition_prefers_project_override_over_user() {
        let temp = tempdir().expect("tempdir");
        let user_dir = temp.path().join("user-agents");
        let project_dir = temp.path().join("project-agents");
        std::fs::create_dir_all(&user_dir).expect("user agents dir");
        std::fs::create_dir_all(&project_dir).expect("project agents dir");
        std::fs::write(
            user_dir.join("reviewer.md"),
            "---\nname: reviewer\ndescription: User reviewer\ntools: [Read]\n---\nUse the user reviewer prompt.\n",
        )
        .expect("write user reviewer");
        std::fs::write(
            project_dir.join("reviewer.md"),
            "---\nname: reviewer\ndescription: Project reviewer\ntools: [Read, Grep]\n---\nUse the project reviewer prompt.\n",
        )
        .expect("write project reviewer");

        let definition = resolve_agent_definition_from_dirs(
            Some("reviewer"),
            Some(&user_dir),
            Some(&project_dir),
        )
        .expect("project override should resolve");

        assert_eq!(definition.agent_type, "reviewer");
        assert_eq!(definition.when_to_use, "Project reviewer");
        assert_eq!(definition.tools, vec!["Read", "Grep"]);
        assert_eq!(
            definition.system_prompt.as_deref(),
            Some("Use the project reviewer prompt.")
        );
        assert_eq!(definition.source, claude_agents::AgentSource::Project);
    }

    fn fake_tool_spec(name: &str) -> ToolSpec {
        ToolSpec {
            name: name.to_owned(),
            protocol_name: name.to_owned(),
            permission_tool_name: name.to_owned(),
            description: String::new(),
            requires_permission: false,
            input_schema: json!({"type": "object"}),
        }
    }

    fn live_mcp_provider(state: Arc<StdMutex<McpCliState>>) -> Arc<crate::RuntimeMcpStateProvider> {
        Arc::new(move || state.lock().clone())
    }

    fn serialized_client(name: &str, connection_type: &str) -> SerializedClient {
        SerializedClient {
            name: name.to_owned(),
            connection_type: connection_type.to_owned(),
            capabilities: None,
        }
    }

    fn serialized_tool(name: &str) -> SerializedTool {
        SerializedTool {
            name: name.to_owned(),
            description: String::new(),
            input_json_schema: Some(json!({"type": "object"})),
            is_mcp: Some(true),
            original_tool_name: None,
        }
    }

    #[test]
    fn resolve_agent_definition_matches_builtin_aliases_case_insensitively() {
        let definition = resolve_agent_definition_from_dirs(Some("planner"), None, None)
            .expect("planner alias should resolve");
        assert_eq!(definition.agent_type, "Plan");

        let definition = resolve_agent_definition_from_dirs(Some("worker"), None, None)
            .expect("worker alias should resolve");
        assert_eq!(definition.agent_type, "general-purpose");
    }

    #[test]
    fn resolve_agent_definition_uses_reviewer_alias_when_review_agent_exists() {
        let temp = tempdir().expect("tempdir");
        let project_dir = temp.path().join("project-agents");
        std::fs::create_dir_all(&project_dir).expect("project agents dir");
        std::fs::write(
            project_dir.join("code-reviewer.md"),
            "---\nname: code-reviewer\ndescription: Code reviewer\ntools: [Read]\n---\nUse the code reviewer prompt.\n",
        )
        .expect("write code reviewer");

        let definition =
            resolve_agent_definition_from_dirs(Some("reviewer"), None, Some(&project_dir))
                .expect("reviewer alias should resolve");

        assert_eq!(definition.agent_type, "code-reviewer");
        assert_eq!(definition.when_to_use, "Code reviewer");
    }

    #[tokio::test]
    async fn resolved_agent_execution_loads_project_agent_definition() {
        let temp = tempdir().expect("tempdir");
        let project_agents_dir = temp.path().join(".claude").join("agents");
        std::fs::create_dir_all(&project_agents_dir).expect("project agents dir");
        std::fs::write(
            project_agents_dir.join("reviewer.md"),
            "---\nname: reviewer\ndescription: Project reviewer\ntools: [Read]\nmodel: inherit\n---\nUse the project reviewer prompt.\n",
        )
        .expect("write project reviewer");

        let requests = Arc::new(StdMutex::new(Vec::new()));
        let runtime: Arc<dyn SubAgentCompletion> = Arc::new(RecordingAgentRuntime {
            requests: Arc::clone(&requests),
            result: claude_core::SubAgentExecutionResult {
                output: "reviewed".to_owned(),
                success: true,
                turns: 5,
                usage: UsageSummary::default(),
            },
        });
        let context = test_context_with_cwd(temp.path().to_path_buf(), Some(runtime));

        let result = agent_tool_inner(
            &json!({
                "prompt": "Review the custom project agent path.",
                "description": "Project review",
                "subagent_type": "reviewer"
            }),
            &context,
        )
        .await
        .expect("custom project agent should succeed");

        assert_eq!(result, "reviewed");
        let requests = requests.lock();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.agent_type, "reviewer");
        assert_eq!(request.model.as_deref(), Some("inherit"));
        assert!(
            request
                .system_prompt
                .as_deref()
                .unwrap_or_default()
                .contains("project reviewer prompt")
        );
        assert!(request.allowed_tools.contains(&"read_file".to_owned()));
        assert!(!request.allowed_tools.contains(&"write_file".to_owned()));
    }

    #[tokio::test]
    async fn resolved_agent_execution_inherits_runtime_prompt_context_and_agent_memory() {
        let temp = tempdir().expect("tempdir");
        let project_agents_dir = temp.path().join("project-agents");
        let extra_dir = temp.path().join("extra");
        let memory_dir = temp
            .path()
            .join(".claude")
            .join("agent-memory")
            .join("reviewer");
        std::fs::create_dir_all(&project_agents_dir).expect("project agents dir");
        std::fs::create_dir_all(&extra_dir).expect("extra dir");
        std::fs::create_dir_all(&memory_dir).expect("memory dir");
        std::fs::write(
            project_agents_dir.join("reviewer.md"),
            "---\nname: reviewer\ndescription: Project reviewer\ntools: [Read]\nmemory: project\n---\nUse the project reviewer prompt.\n",
        )
        .expect("write project reviewer");
        std::fs::write(
            memory_dir.join("MEMORY.md"),
            "- [Review preference](review.md) — prefer focused diffs\n",
        )
        .expect("write memory entrypoint");

        let requests = Arc::new(StdMutex::new(Vec::new()));
        let runtime: Arc<dyn SubAgentCompletion> = Arc::new(RecordingAgentRuntime {
            requests: Arc::clone(&requests),
            result: claude_core::SubAgentExecutionResult {
                output: "reviewed".to_owned(),
                success: true,
                turns: 2,
                usage: UsageSummary::default(),
            },
        });
        let context = test_context_with_cwd(temp.path().to_path_buf(), Some(runtime));
        let runtime_context = crate::RuntimeAgentPromptContext {
            project_agents_dir: Some(project_agents_dir),
            additional_working_directories: vec![extra_dir.clone()],
            ..crate::RuntimeAgentPromptContext::default()
        };

        let result = crate::with_runtime_agent_prompt_context_provider(
            Arc::new(move || runtime_context.clone()),
            async {
                agent_tool_inner(
                    &json!({
                        "prompt": "Review the custom project agent path.",
                        "description": "Project review",
                        "subagent_type": "reviewer"
                    }),
                    &context,
                )
                .await
            },
        )
        .await
        .expect("custom project agent should succeed");

        assert_eq!(result, "reviewed");
        let requests = requests.lock();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert!(!request.skip_transcript);
        assert_eq!(request.additional_working_directories, vec![extra_dir]);
        let system_prompt = request.system_prompt.as_deref().unwrap_or_default();
        assert!(system_prompt.contains("project reviewer prompt"));
        assert!(system_prompt.contains("# Persistent Agent Memory"));
        assert!(system_prompt.contains("prefer focused diffs"));
    }

    #[tokio::test]
    async fn resolved_agent_execution_registers_named_plan_teammate() {
        let temp = tempdir().expect("tempdir");
        let teams_dir = temp.path().join("teams");
        team_helpers::set_base_dir_override(Some(teams_dir.clone()));

        let requests = Arc::new(StdMutex::new(Vec::new()));
        let runtime: Arc<dyn SubAgentCompletion> = Arc::new(RecordingAgentRuntime {
            requests: Arc::clone(&requests),
            result: claude_core::SubAgentExecutionResult {
                output: "planned".to_owned(),
                success: true,
                turns: 3,
                usage: UsageSummary::default(),
            },
        });
        let context = test_context_with_cwd(temp.path().to_path_buf(), Some(runtime));

        let result = agent_tool_inner(
            &json!({
                "prompt": "Audit the project and draft the plan.",
                "description": "Plan teammate",
                "name": "planner",
                "team_name": "alpha-team",
                "mode": "plan"
            }),
            &context,
        )
        .await
        .expect("named teammate should succeed");

        assert_eq!(result, "planned");
        {
            let requests = requests.lock();
            let request = &requests[0];
            assert_eq!(request.agent_name.as_deref(), Some("planner"));
            assert_eq!(request.team_name.as_deref(), Some("alpha-team"));
            assert_eq!(request.permission_mode, Some(PermissionMode::Plan));
        }

        let team = team_helpers::read_team("alpha-team")
            .await
            .expect("team should exist");
        let member = team
            .find_member("planner")
            .expect("planner member should exist");
        assert_eq!(member.mode, Some(PermissionMode::Plan));
        assert_eq!(
            member.backend_type,
            Some(claude_swarm::BackendType::InProcess)
        );
        assert_eq!(member.is_active, Some(false));

        team_helpers::set_base_dir_override(None);
    }

    #[tokio::test]
    async fn resolved_agent_execution_honors_cwd_override() {
        let temp = tempdir().expect("tempdir");
        let nested = temp.path().join("nested");
        std::fs::create_dir_all(&nested).expect("create nested dir");

        let requests = Arc::new(StdMutex::new(Vec::new()));
        let runtime: Arc<dyn SubAgentCompletion> = Arc::new(RecordingAgentRuntime {
            requests: Arc::clone(&requests),
            result: claude_core::SubAgentExecutionResult {
                output: "done".to_owned(),
                success: true,
                turns: 1,
                usage: UsageSummary::default(),
            },
        });
        let context = test_context_with_cwd(temp.path().to_path_buf(), Some(runtime));

        let result = agent_tool_inner(
            &json!({
                "prompt": "Inspect the nested workspace.",
                "description": "Nested audit",
                "cwd": "nested"
            }),
            &context,
        )
        .await
        .expect("agent run should succeed");

        assert_eq!(result, "done");
        let requests = requests.lock();
        let request = &requests[0];
        assert_eq!(request.working_dir, nested);
    }

    #[tokio::test]
    async fn background_agent_execution_creates_runtime_task() {
        let temp = tempdir().expect("tempdir");
        let runtime: Arc<dyn SubAgentCompletion> = Arc::new(RecordingAgentRuntime {
            requests: Arc::new(StdMutex::new(Vec::new())),
            result: claude_core::SubAgentExecutionResult {
                output: "background-complete".to_owned(),
                success: true,
                turns: 2,
                usage: UsageSummary::default(),
            },
        });
        let context = test_context_with_cwd(temp.path().to_path_buf(), Some(runtime));

        let result = agent_tool_inner(
            &json!({
                "prompt": "Review the repository in the background.",
                "description": "Background review",
                "run_in_background": true
            }),
            &context,
        )
        .await
        .expect("background agent should launch");

        let payload: Value = serde_json::from_str(&result).expect("background payload");
        assert_eq!(payload["status"], "async_launched");
        let task_id = payload["task_id"]
            .as_str()
            .expect("task id should be present")
            .to_owned();

        for _ in 0..20 {
            if crate::tasks::task_snapshots().iter().any(|task| {
                task.id == task_id
                    && matches!(task.status, crate::tasks::TaskStatus::Completed)
                    && task.output.contains("background-complete")
            }) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        panic!("background task did not finish in time");
    }

    #[tokio::test]
    async fn worktree_isolation_requires_explicit_runtime_support() {
        let requests = Arc::new(StdMutex::new(Vec::new()));
        let runtime: Arc<dyn SubAgentCompletion> = Arc::new(RecordingAgentRuntime {
            requests,
            result: claude_core::SubAgentExecutionResult {
                output: "noop".to_owned(),
                success: true,
                turns: 1,
                usage: UsageSummary::default(),
            },
        });
        let context = test_context(Some(runtime));

        let error = agent_tool_inner(
            &json!({
                "prompt": "Inspect the worktree copy.",
                "description": "Worktree review",
                "isolation": "worktree"
            }),
            &context,
        )
        .await
        .expect_err("worktree isolation should be rejected for now");

        assert!(error.to_string().contains("not yet supported"));
    }
}
