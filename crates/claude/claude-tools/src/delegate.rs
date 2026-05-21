//! Enhanced subtask delegation engine with parallel execution support.
//!
//! Inspired by hermes-agent's delegate_tool with depth limits, blocked tools,
//! and concurrent execution via tokio::JoinSet.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Result, anyhow};
use claude_core::{ConversationEntry, SubAgentCompletion};
use claude_permissions::PermissionBroker;
use claude_ui_bridge::UiEvent;
use tokio::sync::Semaphore;

use crate::tasks::{
    TaskKind, TaskStatus, allocate_task_id, finish_tracked_task, start_tracked_task,
    update_task_progress,
};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Tools that sub-agents are never allowed to use.
pub const DEFAULT_BLOCKED_TOOLS: &[&str] = &["agent", "send_message"];

/// Configuration for the delegation engine.
pub struct DelegationConfig {
    /// Maximum number of concurrent sub-agents in batch mode.
    pub max_concurrent: usize,
    /// Maximum delegation nesting depth.
    pub max_depth: u32,
    /// Tool names that sub-agents cannot invoke.
    pub blocked_tools: Vec<String>,
    /// Maximum turns per sub-agent.
    pub max_turns: u32,
    /// Timeout per sub-agent completion call in seconds.
    pub timeout_secs: u64,
}

impl Default for DelegationConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 3,
            max_depth: 3,
            blocked_tools: DEFAULT_BLOCKED_TOOLS
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            max_turns: 10,
            timeout_secs: 120,
        }
    }
}

// ---------------------------------------------------------------------------
// Context & Result types
// ---------------------------------------------------------------------------

/// Input context for a single delegation.
pub struct DelegationContext {
    /// The task description for the sub-agent.
    pub task: String,
    /// Working directory for the sub-agent.
    pub cwd: PathBuf,
    /// Parent conversation snapshot (may be empty for root delegation).
    pub parent_conversation: Vec<ConversationEntry>,
    /// Current delegation depth (0 = root).
    pub depth: u32,
    /// Preallocated task metadata used for batch or externally managed tasks.
    pub task_metadata: Option<DelegationTaskMetadata>,
    /// Tools the sub-agent is allowed to use (empty = all non-blocked).
    pub allowed_tools: Vec<String>,
    /// Tool execution context for real tool calls.
    pub tool_context: crate::ToolExecutionContext,
    /// Permission broker for tool execution.
    pub broker: Arc<dyn PermissionBroker>,
}

#[derive(Debug, Clone)]
pub struct DelegationTaskMetadata {
    pub task_id: String,
    pub parent_task_id: Option<String>,
    pub depth: u32,
    pub manage_stack: bool,
}

/// Result of a single delegation.
pub struct DelegationResult {
    /// The task that was delegated.
    pub task: String,
    /// Output text from the sub-agent.
    pub output: String,
    /// Whether the task succeeded.
    pub success: bool,
    /// Number of turns used.
    pub turns_used: u32,
    /// Tool call trace for debugging.
    pub tool_trace: Vec<ToolTraceEntry>,
}

/// A single tool call trace entry.
pub struct ToolTraceEntry {
    /// Tool name.
    pub tool_name: String,
    /// Whether the tool call succeeded.
    pub success: bool,
    /// Duration in milliseconds.
    pub duration_ms: u64,
}

/// Callback type for delegation progress events.
type ProgressCallback = dyn Fn(UiEvent) + Send + Sync;

#[derive(Debug, Clone, Copy)]
enum StackCleanupMode {
    Root,
    Child,
    None,
}

#[derive(Debug, Clone)]
struct ActiveDelegationTask {
    task_id: String,
    parent_task_id: Option<String>,
    depth: u32,
    cleanup_mode: StackCleanupMode,
}

// ---------------------------------------------------------------------------
// Delegation Engine
// ---------------------------------------------------------------------------

/// Engine for delegating tasks to sub-agents.
///
/// Supports both single-task and batch (parallel) delegation with:
/// - Depth-limited recursion prevention
/// - Blocked tool filtering
/// - Progress callbacks via [`UiEvent`]
/// - Configurable concurrency via semaphore
pub struct DelegationEngine {
    config: DelegationConfig,
}

impl DelegationEngine {
    /// Create a new engine with the given configuration.
    pub fn new(config: DelegationConfig) -> Self {
        Self { config }
    }

    /// Delegate a single task to a sub-agent.
    ///
    /// The sub-agent runs in a loop: call the completion provider, execute any
    /// tool calls, feed results back, until the agent produces a text-only
    /// response or exceeds `max_turns`.
    pub async fn delegate_single(
        &self,
        context: DelegationContext,
        executor: Arc<dyn SubAgentCompletion>,
        progress_cb: Option<Arc<ProgressCallback>>,
    ) -> Result<DelegationResult> {
        let task_desc = context.task.clone();
        let cwd = context.cwd.clone();
        let active_task = self.initialize_task(&context)?;
        let task_id = active_task.task_id.clone();
        let depth = active_task.depth;

        start_tracked_task(
            task_id.clone(),
            &task_desc,
            active_task.parent_task_id.clone(),
            active_task.depth,
            TaskKind::Delegation,
            Some("started"),
        )?;

        // Emit started event.
        if let Some(ref cb) = progress_cb {
            cb(UiEvent::SubtaskStarted {
                task_id: task_id.clone(),
                parent_task_id: active_task.parent_task_id.clone(),
                description: task_desc.clone(),
                depth,
            });
        }

        // Build the child conversation.
        let system_prompt = self.build_child_system_prompt(&context.task, depth, &cwd);
        let mut conversation = vec![ConversationEntry::system(&system_prompt)];
        conversation.push(ConversationEntry::user(&context.task));

        let timeout = std::time::Duration::from_secs(self.config.timeout_secs);
        let mut turns_used = 0u32;
        let mut tool_trace: Vec<ToolTraceEntry> = Vec::new();
        let mut last_error: Option<anyhow::Error> = None;

        for _ in 0..self.config.max_turns {
            turns_used += 1;

            let response =
                match tokio::time::timeout(timeout, executor.complete(&conversation)).await {
                    Ok(Ok(resp)) => resp,
                    Ok(Err(e)) => {
                        last_error = Some(e);
                        break;
                    }
                    Err(_) => {
                        last_error = Some(anyhow!("Timeout after {}s", timeout.as_secs()));
                        break;
                    }
                };

            let assistant_text = response.text.clone();
            let mut assistant_entry = ConversationEntry::assistant(&assistant_text);
            assistant_entry.history_text = response.history_text.clone();
            assistant_entry.content_blocks = response.content_blocks.clone();
            assistant_entry.tool_calls = response.tool_calls.clone();
            conversation.push(assistant_entry);

            // No tool calls → child is done.
            if response.tool_calls.is_empty() {
                let output = truncate_output(&assistant_text, 10_000);
                finish_tracked_task(
                    &task_id,
                    TaskStatus::Completed,
                    Some(&truncate_output(&assistant_text, 200)),
                    &output,
                    Some(turns_used),
                )?;
                emit_completed(&progress_cb, &task_id, true, &output, turns_used);
                self.complete_task(&context, &active_task, true);
                return Ok(DelegationResult {
                    task: context.task.to_owned(),
                    output,
                    success: true,
                    turns_used,
                    tool_trace,
                });
            }

            // Execute each tool call.
            for tool_call in &response.tool_calls {
                let tool_name = tool_call.name.clone();
                let start = std::time::Instant::now();

                // Check blocked tools.
                if self.config.blocked_tools.contains(&tool_name) {
                    let duration_ms = start.elapsed().as_millis() as u64;
                    conversation.push(ConversationEntry::tool(
                        &tool_call.id,
                        &tool_name,
                        "Tool is blocked in sub-agent context",
                        true,
                    ));
                    tool_trace.push(ToolTraceEntry {
                        tool_name: tool_name.clone(),
                        success: false,
                        duration_ms,
                    });
                    continue;
                }

                // Check allowed tools.
                if !context.allowed_tools.is_empty() && !context.allowed_tools.contains(&tool_name)
                {
                    let duration_ms = start.elapsed().as_millis() as u64;
                    conversation.push(ConversationEntry::tool(
                        &tool_call.id,
                        &tool_name,
                        "Tool not in allowed list",
                        true,
                    ));
                    tool_trace.push(ToolTraceEntry {
                        tool_name: tool_name.clone(),
                        success: false,
                        duration_ms,
                    });
                    continue;
                }

                // Execute the tool call via the real tool execution pipeline.
                // Box::pin breaks the recursive async cycle (agent → delegate → execute_tool_call → agent).
                let tool_result = Box::pin(crate::execute_tool_call(
                    tool_call,
                    &context.tool_context,
                    &*context.broker,
                ))
                .await;
                let duration_ms = start.elapsed().as_millis() as u64;

                match tool_result {
                    Ok(result) => {
                        let summary_preview = if result.is_error {
                            let trunc = result
                                .content
                                .char_indices()
                                .take_while(|(i, _)| *i < 200)
                                .map(|(_, c)| c)
                                .collect::<String>();
                            format!("{tool_name}: {trunc}")
                        } else {
                            format!("Called {tool_name}")
                        };
                        conversation.push(ConversationEntry::tool(
                            &tool_call.id,
                            &tool_name,
                            &result.content,
                            result.is_error,
                        ));
                        tool_trace.push(ToolTraceEntry {
                            tool_name: tool_name.clone(),
                            success: !result.is_error,
                            duration_ms,
                        });
                        let _ = update_task_progress(&task_id, &summary_preview);
                        // Emit progress.
                        if let Some(ref cb) = progress_cb {
                            cb(UiEvent::SubtaskProgress {
                                task_id: task_id.clone(),
                                turn: turns_used,
                                max_turns: self.config.max_turns,
                                summary: summary_preview,
                            });
                        }
                    }
                    Err(e) => {
                        let error_msg = format!("Error executing {tool_name}: {e}");
                        conversation.push(ConversationEntry::tool(
                            &tool_call.id,
                            &tool_name,
                            &error_msg,
                            true,
                        ));
                        tool_trace.push(ToolTraceEntry {
                            tool_name: tool_name.clone(),
                            success: false,
                            duration_ms,
                        });
                        let summary = format!("{tool_name}: error");
                        let _ = update_task_progress(&task_id, &summary);
                        if let Some(ref cb) = progress_cb {
                            cb(UiEvent::SubtaskProgress {
                                task_id: task_id.clone(),
                                turn: turns_used,
                                max_turns: self.config.max_turns,
                                summary,
                            });
                        }
                    }
                }
            }
        }

        // Exhausted turns or errored out.
        let final_text = conversation
            .last()
            .map(|e| e.text.clone())
            .unwrap_or_default();
        let success = last_error.is_none();
        let output = if let Some(ref e) = last_error {
            truncate_output(&format!("Error: {e}"), 10_000)
        } else {
            truncate_output(&final_text, 10_000)
        };

        finish_tracked_task(
            &task_id,
            if success {
                TaskStatus::Completed
            } else {
                TaskStatus::Failed
            },
            Some(&truncate_output(&output, 200)),
            &output,
            Some(turns_used),
        )?;
        emit_completed(&progress_cb, &task_id, success, &output, turns_used);
        self.complete_task(&context, &active_task, success);

        Ok(DelegationResult {
            task: context.task.to_owned(),
            output,
            success,
            turns_used,
            tool_trace,
        })
    }

    /// Delegate multiple tasks in parallel.
    ///
    /// Uses a [`Semaphore`] to limit concurrency to `max_concurrent`.
    /// Results are returned in the same order as the input tasks.
    #[allow(clippy::too_many_arguments)]
    pub async fn delegate_batch(
        &self,
        tasks: &[String],
        executor: Arc<dyn SubAgentCompletion>,
        cwd: &Path,
        allowed_tools: &[String],
        depth: u32,
        parent_task_id: Option<String>,
        progress_cb: Option<Arc<ProgressCallback>>,
        tool_context: crate::ToolExecutionContext,
        broker: Arc<dyn PermissionBroker>,
    ) -> Result<Vec<DelegationResult>> {
        if tasks.is_empty() {
            return Ok(Vec::new());
        }

        let semaphore = Arc::new(Semaphore::new(self.config.max_concurrent));
        let mut join_set = tokio::task::JoinSet::new();

        for (idx, task) in tasks.iter().enumerate() {
            let task = task.clone();
            let executor = executor.clone();
            let cwd = cwd.to_path_buf();
            let allowed_tools = allowed_tools.to_vec();
            let semaphore = semaphore.clone();
            let progress_cb = progress_cb.clone();
            let mut tool_context = tool_context.clone();
            tool_context.task_stack = Arc::new(parking_lot::Mutex::new(
                claude_core::task_stack::TaskStack::default(),
            ));
            let broker = broker.clone();
            let parent_task_id = parent_task_id.clone();
            let config = DelegationConfig {
                max_concurrent: self.config.max_concurrent,
                max_depth: self.config.max_depth,
                blocked_tools: self.config.blocked_tools.clone(),
                max_turns: self.config.max_turns,
                timeout_secs: self.config.timeout_secs,
            };

            join_set.spawn(async move {
                let _permit = semaphore.acquire().await.expect("semaphore closed");
                let engine = DelegationEngine::new(config);
                let ctx = DelegationContext {
                    task: task.clone(),
                    cwd,
                    parent_conversation: Vec::new(),
                    depth,
                    task_metadata: Some(DelegationTaskMetadata {
                        task_id: allocate_task_id(),
                        parent_task_id,
                        depth,
                        manage_stack: false,
                    }),
                    allowed_tools,
                    tool_context,
                    broker,
                };
                let result = engine.delegate_single(ctx, executor, progress_cb).await;

                // Emit batch progress.
                // (Individual task progress is emitted by delegate_single.)

                (idx, result)
            });
        }

        let mut results: Vec<Option<DelegationResult>> = (0..tasks.len()).map(|_| None).collect();
        let mut completed = 0usize;
        let total = tasks.len();

        while let Some(res) = join_set.join_next().await {
            match res {
                Ok((idx, Ok(delegation_result))) => {
                    results[idx] = Some(delegation_result);
                }
                Ok((idx, Err(e))) => {
                    results[idx] = Some(DelegationResult {
                        task: tasks[idx].clone(),
                        output: format!("Delegation error: {e}"),
                        success: false,
                        turns_used: 0,
                        tool_trace: Vec::new(),
                    });
                }
                Err(e) => {
                    return Err(anyhow!("Join error: {e}"));
                }
            }
            completed += 1;
            if let Some(ref cb) = progress_cb {
                cb(UiEvent::BatchProgress {
                    total,
                    completed,
                    running: total - completed,
                });
            }
        }

        Ok(results
            .into_iter()
            .map(|r| r.expect("all batch slots should be filled"))
            .collect())
    }

    /// Build the system prompt for a child agent.
    fn build_child_system_prompt(&self, task: &str, depth: u32, cwd: &Path) -> String {
        let depth_warning = if depth > 0 {
            format!(
                "\n\nWARNING: You are a nested sub-agent at depth {}. \
                 Keep responses concise and avoid further delegation.",
                depth
            )
        } else {
            String::new()
        };

        format!(
            "You are a sub-agent tasked with: {task}\n\
             Working directory: {}\n\
             You have at most {} turns to complete the task.\
             {depth_warning}\n\n\
             Complete the task and return the result. \
             If you cannot complete it, explain why.",
            cwd.display(),
            self.config.max_turns,
        )
    }

    /// Filter a list of tool names, removing blocked tools.
    #[allow(dead_code)]
    fn filter_tools(&self, allowed_tools: &[String]) -> Vec<String> {
        if allowed_tools.is_empty() {
            return Vec::new();
        }
        allowed_tools
            .iter()
            .filter(|t| !self.config.blocked_tools.contains(t))
            .cloned()
            .collect()
    }

    fn initialize_task(&self, context: &DelegationContext) -> Result<ActiveDelegationTask> {
        if let Some(metadata) = &context.task_metadata {
            if metadata.depth >= self.config.max_depth {
                return Err(anyhow!(
                    "Maximum delegation depth ({}) exceeded. Requested depth: {}",
                    self.config.max_depth,
                    metadata.depth
                ));
            }
            return Ok(ActiveDelegationTask {
                task_id: metadata.task_id.clone(),
                parent_task_id: metadata.parent_task_id.clone(),
                depth: metadata.depth,
                cleanup_mode: if metadata.manage_stack {
                    StackCleanupMode::Child
                } else {
                    StackCleanupMode::None
                },
            });
        }

        let mut stack = context.tool_context.task_stack.lock();
        if stack.current().is_none() {
            let task_id = stack.push_root(context.parent_conversation.clone());
            let frame = stack.current().cloned().expect("root frame should exist");
            return Ok(ActiveDelegationTask {
                task_id,
                parent_task_id: frame.parent_task_id,
                depth: frame.depth,
                cleanup_mode: StackCleanupMode::Root,
            });
        }

        let task_id = stack.push_child(
            context.parent_conversation.clone(),
            context.allowed_tools.clone(),
        )?;
        let frame = stack.current().cloned().expect("child frame should exist");
        Ok(ActiveDelegationTask {
            task_id,
            parent_task_id: frame.parent_task_id,
            depth: frame.depth,
            cleanup_mode: StackCleanupMode::Child,
        })
    }

    fn complete_task(
        &self,
        context: &DelegationContext,
        task: &ActiveDelegationTask,
        success: bool,
    ) {
        let mut stack = context.tool_context.task_stack.lock();
        match task.cleanup_mode {
            StackCleanupMode::Root => {
                if success {
                    stack.mark_completed();
                } else {
                    stack.mark_failed();
                }
                let _ = stack.pop();
            }
            StackCleanupMode::Child => {
                if success {
                    stack.mark_completed();
                } else {
                    stack.mark_failed();
                }
                let _ = stack.resume_parent();
            }
            StackCleanupMode::None => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn truncate_output(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        text.to_owned()
    } else {
        // Find a valid UTF-8 boundary to avoid slicing mid-character.
        let boundary = text
            .char_indices()
            .take_while(|(i, _)| *i < max_chars)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(max_chars.min(text.len()));
        format!("{}...[truncated]", &text[..boundary])
    }
}

fn emit_completed(
    progress_cb: &Option<Arc<ProgressCallback>>,
    task_id: &str,
    success: bool,
    output: &str,
    turns_used: u32,
) {
    if let Some(cb) = progress_cb {
        cb(UiEvent::SubtaskCompleted {
            task_id: task_id.to_owned(),
            success,
            output_preview: truncate_output(output, 500),
            turns_used,
        });
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::ToolExecutionContext;
    use claude_core::{ProviderResponse, ToolCall};
    use claude_permissions::StaticPermissionBroker;
    use parking_lot::Mutex;
    use serde_json::json;

    // -- Mock completer -------------------------------------------------------

    struct MockCompleter {
        response: String,
    }

    #[async_trait::async_trait]
    impl SubAgentCompletion for MockCompleter {
        async fn complete(
            &self,
            _conversation: &[ConversationEntry],
        ) -> anyhow::Result<ProviderResponse> {
            Ok(ProviderResponse {
                text: self.response.clone(),
                tool_calls: Vec::new(),
                stop_reason: "stop".to_owned(),
                usage: Default::default(),
                history_text: Some(String::new()),
                content_blocks: Vec::new(),
                thinking: None,
                request_id: None,
                research: None,
            })
        }
    }

    /// Multi-turn mock: returns tool calls first, then text.
    #[allow(dead_code)]
    struct MultiTurnMock {
        responses: Vec<ProviderResponse>,
        call_count: Mutex<usize>,
    }

    #[allow(dead_code)]
    impl MultiTurnMock {
        fn new(responses: Vec<ProviderResponse>) -> Self {
            Self {
                responses,
                call_count: Mutex::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl SubAgentCompletion for MultiTurnMock {
        async fn complete(
            &self,
            _conversation: &[ConversationEntry],
        ) -> anyhow::Result<ProviderResponse> {
            let mut count = self.call_count.lock();
            let idx = *count;
            *count += 1;
            if idx < self.responses.len() {
                Ok(self.responses[idx].clone())
            } else {
                Ok(ProviderResponse {
                    text: "done".to_owned(),
                    tool_calls: Vec::new(),
                    stop_reason: "stop".to_owned(),
                    usage: Default::default(),
                    history_text: Some(String::new()),
                    content_blocks: Vec::new(),
                    thinking: None,
                    request_id: None,
                    research: None,
                })
            }
        }
    }

    struct InspectingMultiTurnMock {
        responses: Vec<ProviderResponse>,
        call_count: Mutex<usize>,
        seen_conversations: Mutex<Vec<Vec<ConversationEntry>>>,
    }

    impl InspectingMultiTurnMock {
        fn new(responses: Vec<ProviderResponse>) -> Self {
            Self {
                responses,
                call_count: Mutex::new(0),
                seen_conversations: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl SubAgentCompletion for InspectingMultiTurnMock {
        async fn complete(
            &self,
            conversation: &[ConversationEntry],
        ) -> anyhow::Result<ProviderResponse> {
            self.seen_conversations.lock().push(conversation.to_vec());

            let mut count = self.call_count.lock();
            let idx = *count;
            *count += 1;
            if idx < self.responses.len() {
                Ok(self.responses[idx].clone())
            } else {
                Ok(ProviderResponse {
                    text: "done".to_owned(),
                    tool_calls: Vec::new(),
                    stop_reason: "stop".to_owned(),
                    usage: Default::default(),
                    history_text: Some(String::new()),
                    content_blocks: Vec::new(),
                    thinking: None,
                    request_id: None,
                    research: None,
                })
            }
        }
    }

    #[allow(dead_code)]
    fn tool_context() -> ToolExecutionContext {
        ToolExecutionContext {
            cwd: PathBuf::from("."),
            original_cwd: PathBuf::from("."),
            active_worktree_session: None,
            timeout_ms: 30_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: std::sync::Arc::new(parking_lot::Mutex::new(
                claude_core::task_stack::TaskStack::default(),
            )),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    /// Helper: create a default broker for tests (bypass all permissions).
    fn test_broker() -> Arc<dyn PermissionBroker> {
        Arc::new(StaticPermissionBroker::new(true))
    }

    #[tokio::test]
    async fn delegation_single_task_succeeds() {
        let engine = DelegationEngine::new(DelegationConfig::default());
        let completer = Arc::new(MockCompleter {
            response: "Task completed successfully!".to_owned(),
        });

        let ctx = DelegationContext {
            task: "Fix the bug".to_owned(),
            cwd: PathBuf::from("."),
            parent_conversation: Vec::new(),
            depth: 0,
            task_metadata: None,
            allowed_tools: Vec::new(),
            tool_context: tool_context(),
            broker: test_broker(),
        };

        let result = engine.delegate_single(ctx, completer, None).await.unwrap();

        assert!(result.success);
        assert_eq!(result.output, "Task completed successfully!");
        assert_eq!(result.turns_used, 1);
    }

    #[tokio::test]
    async fn delegation_depth_limit_enforced() {
        let config = DelegationConfig {
            max_depth: 1,
            ..Default::default()
        };
        let engine = DelegationEngine::new(config);
        let completer = Arc::new(MockCompleter {
            response: "done".to_owned(),
        });

        let ctx = DelegationContext {
            task: "nested".to_owned(),
            cwd: PathBuf::from("."),
            parent_conversation: Vec::new(),
            depth: 2, // exceeds max_depth
            task_metadata: Some(DelegationTaskMetadata {
                task_id: "nested-1".to_owned(),
                parent_task_id: Some("root".to_owned()),
                depth: 2,
                manage_stack: false,
            }),
            allowed_tools: Vec::new(),
            tool_context: tool_context(),
            broker: test_broker(),
        };

        let result = engine.delegate_single(ctx, completer, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn delegation_blocked_tools_filtered() {
        let config = DelegationConfig::default();
        let engine = DelegationEngine::new(config);
        let filtered = engine.filter_tools(&[
            "read_file".to_owned(),
            "agent".to_owned(),
            "bash".to_owned(),
        ]);
        assert!(filtered.contains(&"read_file".to_owned()));
        assert!(!filtered.contains(&"agent".to_owned()));
    }

    #[test]
    fn delegation_filter_tools_removes_blocked() {
        let config = DelegationConfig::default();
        let engine = DelegationEngine::new(config);
        let filtered = engine.filter_tools(&["agent".to_owned(), "send_message".to_owned()]);
        assert!(filtered.is_empty());
    }

    #[test]
    fn delegation_builds_system_prompt() {
        let config = DelegationConfig::default();
        let engine = DelegationEngine::new(config);
        let prompt = engine.build_child_system_prompt("test task", 0, Path::new("/tmp"));
        assert!(prompt.contains("test task"));
        assert!(prompt.contains("/tmp"));
    }

    #[tokio::test]
    async fn delegation_batch_empty_tasks() {
        let engine = DelegationEngine::new(DelegationConfig::default());
        let completer = Arc::new(MockCompleter {
            response: "done".to_owned(),
        });
        let results = engine
            .delegate_batch(
                &[],
                completer,
                Path::new("."),
                &[],
                0,
                None,
                None,
                tool_context(),
                test_broker(),
            )
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn delegation_batch_parallel_execution() {
        let engine = DelegationEngine::new(DelegationConfig::default());
        let completer = Arc::new(MockCompleter {
            response: "done".to_owned(),
        });
        let tasks: Vec<String> = vec!["task1".to_owned(), "task2".to_owned(), "task3".to_owned()];
        let results = engine
            .delegate_batch(
                &tasks,
                completer,
                Path::new("."),
                &[],
                0,
                None,
                None,
                tool_context(),
                test_broker(),
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 3);
        for result in &results {
            assert!(result.success);
            assert_eq!(result.output, "done");
        }
    }

    #[tokio::test]
    async fn delegation_progress_callback_fires() {
        let engine = DelegationEngine::new(DelegationConfig::default());
        let completer = Arc::new(MockCompleter {
            response: "done".to_owned(),
        });

        let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        let cb = Arc::new(move |event: UiEvent| {
            let label = match &event {
                UiEvent::SubtaskStarted { task_id, .. } => format!("started:{task_id}"),
                UiEvent::SubtaskCompleted {
                    task_id, success, ..
                } => {
                    format!("completed:{task_id}:success={success}")
                }
                _ => "other".to_owned(),
            };
            events_clone.lock().push(label);
        }) as Arc<ProgressCallback>;

        let ctx = DelegationContext {
            task: "test".to_owned(),
            cwd: PathBuf::from("."),
            parent_conversation: Vec::new(),
            depth: 0,
            task_metadata: None,
            allowed_tools: Vec::new(),
            tool_context: tool_context(),
            broker: test_broker(),
        };

        engine
            .delegate_single(ctx, completer, Some(cb))
            .await
            .unwrap();

        let captured = events.lock();
        assert!(captured.len() >= 2);
        assert!(captured[0].starts_with("started:"));
        assert!(captured[1].starts_with("completed:"));
    }

    #[tokio::test]
    async fn delegation_preserves_assistant_tool_calls_across_turns() {
        let engine = DelegationEngine::new(DelegationConfig::default());
        let completer = Arc::new(InspectingMultiTurnMock::new(vec![
            ProviderResponse {
                text: "Need to delegate".to_owned(),
                tool_calls: vec![ToolCall {
                    id: "call-1".to_owned(),
                    name: "agent".to_owned(),
                    input: json!({"task":"inspect"}),
                }],
                stop_reason: "tool_use".to_owned(),
                usage: Default::default(),
                history_text: Some("Need to delegate".to_owned()),
                content_blocks: vec![
                    json!({"type":"text","text":"Need to delegate"}),
                    json!({
                        "type":"tool_use",
                        "id":"call-1",
                        "name":"agent",
                        "input":{"task":"inspect"}
                    }),
                ],
                thinking: None,
                request_id: None,
                research: None,
            },
            ProviderResponse {
                text: "done".to_owned(),
                tool_calls: Vec::new(),
                stop_reason: "stop".to_owned(),
                usage: Default::default(),
                history_text: Some("done".to_owned()),
                content_blocks: vec![json!({"type":"text","text":"done"})],
                thinking: None,
                request_id: None,
                research: None,
            },
        ]));

        let ctx = DelegationContext {
            task: "inspect".to_owned(),
            cwd: PathBuf::from("."),
            parent_conversation: Vec::new(),
            depth: 0,
            task_metadata: None,
            allowed_tools: Vec::new(),
            tool_context: tool_context(),
            broker: test_broker(),
        };

        let result = engine
            .delegate_single(ctx, completer.clone(), None)
            .await
            .unwrap();
        assert!(result.success);
        assert_eq!(result.output, "done");

        let seen = completer.seen_conversations.lock();
        assert_eq!(seen.len(), 2);

        let second_turn = &seen[1];
        let assistant_entry = second_turn
            .iter()
            .find(|entry| {
                matches!(entry.role, claude_core::ConversationRole::Assistant)
                    && entry.tool_calls.iter().any(|call| call.id == "call-1")
            })
            .expect("assistant entry with original tool call should be preserved");
        assert_eq!(
            assistant_entry.history_text.as_deref(),
            Some("Need to delegate")
        );
        assert!(
            assistant_entry
                .content_blocks
                .iter()
                .any(|block| block.get("type").and_then(|value| value.as_str()) == Some("tool_use"))
        );

        let tool_entry = second_turn
            .iter()
            .find(|entry| entry.tool_call_id.as_deref() == Some("call-1"))
            .expect("tool result should still reference the original tool call id");
        assert_eq!(tool_entry.name.as_deref(), Some("agent"));
        assert!(tool_entry.is_error);
    }

    #[test]
    fn truncate_output_short_text_unchanged() {
        assert_eq!(truncate_output("hello", 100), "hello");
    }

    #[test]
    fn truncate_output_long_text_truncated() {
        let long = "x".repeat(200);
        let result = truncate_output(&long, 100);
        assert!(result.ends_with("...[truncated]"));
        assert_eq!(result.len(), "x".repeat(100).len() + "...[truncated]".len());
    }
}
