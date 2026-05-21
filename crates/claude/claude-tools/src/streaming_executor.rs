//! Streaming tool executor with concurrency control.
//!
//! Mirrors the upstream `StreamingToolExecutor` pattern: tools are added as
//! they stream in from the LLM response and are dispatched immediately when
//! concurrency rules allow.  Concurrent-safe tools may run in parallel;
//! non-safe tools require exclusive access.  Results are buffered and emitted
//! in the order tools were received.

use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::tool_progress::{ProgressCallback, ProgressStream};

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// Status of a tracked tool within the executor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    /// Waiting for concurrency slot.
    Queued,
    /// Currently running.
    Executing,
    /// Finished – results ready to yield.
    Completed,
    /// Results already yielded to consumer.
    Yielded,
}

/// A lightweight description of a tool call being tracked.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedToolCall {
    /// Unique tool-call identifier.
    pub id: String,
    /// Tool name (e.g. `"read_file"`).
    pub name: String,
    /// JSON input for the tool.
    pub input: Value,
    /// Whether the tool is safe to run alongside other concurrent-safe tools.
    pub is_concurrency_safe: bool,
    /// Current execution status.
    pub status: ToolStatus,
}

/// The result of executing a single tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecutionResult {
    /// The tool-call id this result belongs to.
    pub tool_call_id: String,
    /// Output content (may be truncated).
    pub content: String,
    /// Whether the tool returned an error.
    pub is_error: bool,
    /// Execution duration.
    pub duration: Duration,
}

// ---------------------------------------------------------------------------
// Tool executor trait (injectable for testing)
// ---------------------------------------------------------------------------

/// A synchronous-or-async function that actually runs a tool.
/// Implementations are provided by the caller of [`StreamingToolExecutor`].
pub trait ToolRunner: Send + Sync + 'static {
    /// Execute the tool and return its result.
    fn run(
        &self,
        tool_call_id: &str,
        name: &str,
        input: &Value,
        progress: &ProgressStream,
    ) -> JoinHandle<ToolExecutionResult>;
}

/// Signature for a tool execution function.
type ToolExecFn = dyn Fn(&str, &str, &Value, &ProgressStream) -> ToolExecutionResult + Send + Sync;

/// A simple function-pointer-based [`ToolRunner`].
pub struct FnToolRunner {
    pub f: Arc<ToolExecFn>,
}

impl ToolRunner for FnToolRunner {
    fn run(
        &self,
        tool_call_id: &str,
        name: &str,
        input: &Value,
        progress: &ProgressStream,
    ) -> JoinHandle<ToolExecutionResult> {
        let id = tool_call_id.to_owned();
        let n = name.to_owned();
        let inp = input.clone();
        let p = progress.clone();
        let f = Arc::clone(&self.f);
        tokio::spawn(async move { f(&id, &n, &inp, &p) })
    }
}

// ---------------------------------------------------------------------------
// StreamingToolExecutor
// ---------------------------------------------------------------------------

/// Internal tracked state for each tool in the executor.
struct TrackedTool {
    call: TrackedToolCall,
    /// Shared reference to the tool input.
    ///
    /// Mirrors `call.input` but in `Arc<Value>` form so dispatch can hand a
    /// cheap refcount-bump to the spawned task instead of deep-cloning the
    /// JSON tree on every retry of `dispatch_queued`.
    input_arc: Arc<Value>,
    result: Option<ToolExecutionResult>,
}

impl std::fmt::Debug for TrackedTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrackedTool")
            .field("call", &self.call)
            .field("result", &self.result)
            .finish()
    }
}

/// Configuration for the streaming executor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingExecutorConfig {
    /// Maximum number of tools that can execute concurrently.
    pub max_concurrency: usize,
    /// Per-tool timeout.  `None` means no timeout.
    pub timeout: Option<Duration>,
    /// Maximum bytes for a tool result before truncation.
    pub max_result_bytes: usize,
}

impl Default for StreamingExecutorConfig {
    fn default() -> Self {
        Self {
            max_concurrency: 10,
            timeout: Some(Duration::from_secs(300)),
            max_result_bytes: 100_000,
        }
    }
}

/// Shared state behind the `Arc<Mutex<>>`.
struct SharedState {
    tools: Vec<TrackedTool>,
    discarded: bool,
    has_errored: bool,
}

/// Streaming tool executor with concurrency control.
///
/// Tools are added via [`add_tool`](Self::add_tool) and dispatched
/// automatically when concurrency rules allow.  Completed results are
/// yielded in order via [`completed_results`](Self::completed_results) or
/// [`wait_for_remaining`](Self::wait_for_remaining).
pub struct StreamingToolExecutor {
    state: Arc<Mutex<SharedState>>,
    config: StreamingExecutorConfig,
    runner: Arc<dyn ToolRunner>,
    progress: ProgressStream,
    notify: Arc<Notify>,
}

impl StreamingToolExecutor {
    /// Create a new executor with the given runner and configuration.
    pub fn new(
        runner: Arc<dyn ToolRunner>,
        config: StreamingExecutorConfig,
        progress_callback: Option<ProgressCallback>,
    ) -> Self {
        let progress = match progress_callback {
            Some(cb) => ProgressStream::with_callback(cb),
            None => ProgressStream::new(),
        };
        Self {
            state: Arc::new(Mutex::new(SharedState {
                tools: Vec::new(),
                discarded: false,
                has_errored: false,
            })),
            config,
            runner,
            progress,
            notify: Arc::new(Notify::new()),
        }
    }

    /// Create with default config and a simple runner.
    pub fn with_runner(runner: Arc<dyn ToolRunner>) -> Self {
        Self::new(runner, StreamingExecutorConfig::default(), None)
    }

    /// The shared progress stream.
    #[must_use]
    pub fn progress_stream(&self) -> &ProgressStream {
        &self.progress
    }

    // -- add / discard ------------------------------------------------------

    /// Add a tool call to the execution queue.
    ///
    /// The tool is dispatched immediately if concurrency rules allow.
    pub fn add_tool(&self, id: &str, name: &str, input: &Value, is_concurrency_safe: bool) {
        let mut state = self.state.lock();
        if state.discarded {
            return;
        }

        // Single deep-clone of the JSON tree at queue time. The dispatch
        // path then reuses an Arc<Value> wrapper to hand the input to each
        // spawned task without re-cloning.
        let input_arc: Arc<Value> = Arc::new(input.clone());

        let call = TrackedToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            // Public record's `input` is filled lazily on `tracked_calls()`
            // from the shared `input_arc`. We avoid populating it eagerly to
            // keep the queue cheap; serializing a `TrackedToolCall` directly
            // out of the queue is not a supported path.
            input: Value::Null,
            is_concurrency_safe,
            status: ToolStatus::Queued,
        };

        state.tools.push(TrackedTool {
            call,
            input_arc,
            result: None,
        });

        self.try_dispatch(&mut state);
    }

    /// Discard all pending and in-progress tools.
    pub fn discard(&self) {
        let mut state = self.state.lock();
        state.discarded = true;
        for tool in state.tools.iter_mut() {
            if tool.call.status == ToolStatus::Queued {
                tool.call.status = ToolStatus::Completed;
                tool.result = Some(ToolExecutionResult {
                    tool_call_id: tool.call.id.clone(),
                    content: "<tool_use_error>Error: Streaming fallback - tool execution discarded</tool_use_error>".into(),
                    is_error: true,
                    duration: Duration::ZERO,
                });
            }
        }
        self.notify.notify_waiters();
    }

    // -- result retrieval ---------------------------------------------------

    /// Return completed (but not yet yielded) results in order.
    pub fn completed_results(&self) -> Vec<ToolExecutionResult> {
        let mut state = self.state.lock();
        let mut out = Vec::new();

        for tool in state.tools.iter_mut() {
            if tool.call.status == ToolStatus::Completed {
                tool.call.status = ToolStatus::Yielded;
                if let Some(r) = tool.result.take() {
                    out.push(r);
                }
            } else if tool.call.status == ToolStatus::Executing && !tool.call.is_concurrency_safe {
                // Must preserve order for non-concurrent tools.
                break;
            }
        }
        out
    }

    /// Wait until all tools have finished and return all remaining results.
    pub async fn wait_for_remaining(&self) -> Vec<ToolExecutionResult> {
        loop {
            {
                let state = self.state.lock();
                let all_done = state
                    .tools
                    .iter()
                    .all(|t| matches!(t.call.status, ToolStatus::Completed | ToolStatus::Yielded));
                if all_done {
                    break;
                }
            }
            self.notify.notified().await;
        }
        self.completed_results()
    }

    /// Whether any tool is still executing or queued.
    #[must_use]
    pub fn has_unfinished_tools(&self) -> bool {
        let state = self.state.lock();
        state
            .tools
            .iter()
            .any(|t| !matches!(t.call.status, ToolStatus::Yielded))
    }

    /// Whether any tool is currently executing.
    #[must_use]
    pub fn has_executing_tools(&self) -> bool {
        let state = self.state.lock();
        state
            .tools
            .iter()
            .any(|t| t.call.status == ToolStatus::Executing)
    }

    /// Number of tools in the executor (all statuses).
    #[must_use]
    pub fn tool_count(&self) -> usize {
        self.state.lock().tools.len()
    }

    /// Snapshot of all tracked tool calls and their statuses.
    pub fn tracked_calls(&self) -> Vec<TrackedToolCall> {
        self.state
            .lock()
            .tools
            .iter()
            .map(|t| {
                let mut snapshot = t.call.clone();
                // Lazily populate `input` from the shared Arc — see add_tool.
                snapshot.input = (*t.input_arc).clone();
                snapshot
            })
            .collect()
    }

    /// Mark a tool as errored, which cancels sibling bash-like tools.
    pub fn mark_error(&self, _tool_description: &str) {
        self.state.lock().has_errored = true;
    }

    /// Whether any tool has errored.
    #[must_use]
    pub fn has_errored(&self) -> bool {
        self.state.lock().has_errored
    }

    // -- internal dispatch --------------------------------------------------

    fn can_execute(is_concurrency_safe: bool, tools: &[TrackedTool]) -> bool {
        let executing: Vec<&TrackedTool> = tools
            .iter()
            .filter(|t| t.call.status == ToolStatus::Executing)
            .collect();
        if executing.is_empty() {
            return true;
        }
        is_concurrency_safe && executing.iter().all(|t| t.call.is_concurrency_safe)
    }

    fn try_dispatch(&self, state: &mut SharedState) {
        let max = self.config.max_concurrency;
        let executing_count = state
            .tools
            .iter()
            .filter(|t| t.call.status == ToolStatus::Executing)
            .count();

        // Collect indices of tools to dispatch
        let mut to_dispatch: Vec<usize> = Vec::new();
        let mut current_executing = executing_count;

        for (i, tool) in state.tools.iter().enumerate() {
            if tool.call.status != ToolStatus::Queued {
                continue;
            }
            if current_executing >= max {
                break;
            }
            if !Self::can_execute(tool.call.is_concurrency_safe, &state.tools) {
                if !tool.call.is_concurrency_safe {
                    break;
                }
                continue;
            }
            to_dispatch.push(i);
            current_executing += 1;
        }

        // Dispatch each tool
        for idx in to_dispatch {
            let tool = &mut state.tools[idx];
            tool.call.status = ToolStatus::Executing;

            let id = tool.call.id.clone();
            let name = tool.call.name.clone();
            // Cheap Arc clone instead of deep-cloning the JSON tree.
            let input_arc: Arc<Value> = Arc::clone(&tool.input_arc);
            let runner = Arc::clone(&self.runner);
            let progress = self.progress.clone();
            let notify = Arc::clone(&self.notify);
            let timeout = self.config.timeout;
            let max_bytes = self.config.max_result_bytes;
            let max_concurrency = self.config.max_concurrency;

            let state_arc = Arc::clone(&self.state);

            tokio::spawn(async move {
                let start = std::time::Instant::now();

                // Check preconditions
                let discarded = state_arc.lock().discarded;
                if discarded {
                    let r = ToolExecutionResult {
                        tool_call_id: id.clone(),
                        content: "<tool_use_error>Discarded</tool_use_error>".into(),
                        is_error: true,
                        duration: start.elapsed(),
                    };
                    let mut s = state_arc.lock();
                    if let Some(tool) = s.tools.iter_mut().find(|t| t.call.id == id) {
                        tool.call.status = ToolStatus::Completed;
                        tool.result = Some(r);
                    }
                    drop(s);
                    Self::dispatch_queued(
                        &state_arc,
                        &runner,
                        &progress,
                        &notify,
                        max_concurrency,
                        timeout,
                        max_bytes,
                    );
                    notify.notify_waiters();
                    return;
                }

                let has_errored = state_arc.lock().has_errored;
                if has_errored {
                    let r = ToolExecutionResult {
                        tool_call_id: id.clone(),
                        content:
                            "<tool_use_error>Cancelled: parallel tool call errored</tool_use_error>"
                                .into(),
                        is_error: true,
                        duration: start.elapsed(),
                    };
                    let mut s = state_arc.lock();
                    if let Some(tool) = s.tools.iter_mut().find(|t| t.call.id == id) {
                        tool.call.status = ToolStatus::Completed;
                        tool.result = Some(r);
                    }
                    drop(s);
                    Self::dispatch_queued(
                        &state_arc,
                        &runner,
                        &progress,
                        &notify,
                        max_concurrency,
                        timeout,
                        max_bytes,
                    );
                    notify.notify_waiters();
                    return;
                }

                // Run the tool
                let handle = runner.run(&id, &name, input_arc.as_ref(), &progress);

                let result = if let Some(dur) = timeout {
                    match tokio::time::timeout(dur, handle).await {
                        Ok(res) => res,
                        Err(_) => {
                            let r = ToolExecutionResult {
                                tool_call_id: id.clone(),
                                content: format!(
                                    "<tool_use_error>Timeout after {}s</tool_use_error>",
                                    dur.as_secs()
                                ),
                                is_error: true,
                                duration: start.elapsed(),
                            };
                            let mut s = state_arc.lock();
                            if let Some(tool) = s.tools.iter_mut().find(|t| t.call.id == id) {
                                tool.call.status = ToolStatus::Completed;
                                tool.result = Some(r);
                            }
                            drop(s);
                            Self::dispatch_queued(
                                &state_arc,
                                &runner,
                                &progress,
                                &notify,
                                max_concurrency,
                                timeout,
                                max_bytes,
                            );
                            notify.notify_waiters();
                            return;
                        }
                    }
                } else {
                    handle.await
                };

                let mut r = match result {
                    Ok(r) => r,
                    Err(e) => ToolExecutionResult {
                        tool_call_id: id.clone(),
                        content: format!("<tool_use_error>Join error: {e}</tool_use_error>"),
                        is_error: true,
                        duration: start.elapsed(),
                    },
                };

                // Apply result budget
                if r.content.len() > max_bytes {
                    r.content = apply_tool_result_budget(&r.content, max_bytes);
                }

                // Store result
                {
                    let mut s = state_arc.lock();
                    if let Some(tool) = s.tools.iter_mut().find(|t| t.call.id == id) {
                        tool.call.status = ToolStatus::Completed;
                        tool.result = Some(r);
                    }
                }
                // After completing, try to dispatch any queued tools that
                // were blocked by this non-concurrent tool.
                Self::dispatch_queued(
                    &state_arc,
                    &runner,
                    &progress,
                    &notify,
                    max_concurrency,
                    timeout,
                    max_bytes,
                );
                notify.notify_waiters();
            });
        }
    }

    /// After a tool finishes, check whether any queued tools can now be
    /// dispatched (e.g. a non-concurrent tool that was blocked).
    fn dispatch_queued(
        state_arc: &Arc<Mutex<SharedState>>,
        runner: &Arc<dyn ToolRunner>,
        progress: &ProgressStream,
        notify: &Arc<Notify>,
        max_concurrency: usize,
        timeout: Option<Duration>,
        max_bytes: usize,
    ) {
        let mut state = state_arc.lock();
        let executing_count = state
            .tools
            .iter()
            .filter(|t| t.call.status == ToolStatus::Executing)
            .count();
        let mut to_dispatch: Vec<usize> = Vec::new();
        let mut current_executing = executing_count;

        for (i, tool) in state.tools.iter().enumerate() {
            if tool.call.status != ToolStatus::Queued {
                continue;
            }
            if current_executing >= max_concurrency {
                break;
            }
            if !Self::can_execute(tool.call.is_concurrency_safe, &state.tools) {
                if !tool.call.is_concurrency_safe {
                    break;
                }
                continue;
            }
            to_dispatch.push(i);
            current_executing += 1;
        }

        for idx in to_dispatch {
            let tool = &mut state.tools[idx];
            tool.call.status = ToolStatus::Executing;

            let id = tool.call.id.clone();
            let name = tool.call.name.clone();
            // Cheap Arc clone instead of deep-cloning the JSON tree.
            let input_arc: Arc<Value> = Arc::clone(&tool.input_arc);
            let runner = Arc::clone(runner);
            let progress = progress.clone();
            let notify = Arc::clone(notify);
            let state_arc = Arc::clone(state_arc);

            tokio::spawn(async move {
                let start = std::time::Instant::now();
                let handle = runner.run(&id, &name, input_arc.as_ref(), &progress);

                let result = if let Some(dur) = timeout {
                    match tokio::time::timeout(dur, handle).await {
                        Ok(res) => res,
                        Err(_) => {
                            let r = ToolExecutionResult {
                                tool_call_id: id.clone(),
                                content: format!(
                                    "<tool_use_error>Timeout after {}s</tool_use_error>",
                                    dur.as_secs()
                                ),
                                is_error: true,
                                duration: start.elapsed(),
                            };
                            let mut s = state_arc.lock();
                            if let Some(tool) = s.tools.iter_mut().find(|t| t.call.id == id) {
                                tool.call.status = ToolStatus::Completed;
                                tool.result = Some(r);
                            }
                            drop(s);
                            Self::dispatch_queued(
                                &state_arc,
                                &runner,
                                &progress,
                                &notify,
                                max_concurrency,
                                timeout,
                                max_bytes,
                            );
                            notify.notify_waiters();
                            return;
                        }
                    }
                } else {
                    handle.await
                };

                let mut r = match result {
                    Ok(r) => r,
                    Err(e) => ToolExecutionResult {
                        tool_call_id: id.clone(),
                        content: format!("<tool_use_error>Join error: {e}</tool_use_error>"),
                        is_error: true,
                        duration: start.elapsed(),
                    },
                };

                if r.content.len() > max_bytes {
                    r.content = apply_tool_result_budget(&r.content, max_bytes);
                }

                {
                    let mut s = state_arc.lock();
                    if let Some(tool) = s.tools.iter_mut().find(|t| t.call.id == id) {
                        tool.call.status = ToolStatus::Completed;
                        tool.result = Some(r);
                    }
                }
                Self::dispatch_queued(
                    &state_arc,
                    &runner,
                    &progress,
                    &notify,
                    max_concurrency,
                    timeout,
                    max_bytes,
                );
                notify.notify_waiters();
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Result budget
// ---------------------------------------------------------------------------

/// Truncate `content` to fit within `budget` bytes, keeping a head and tail
/// with a marker in between.
#[must_use]
pub fn apply_tool_result_budget(content: &str, budget: usize) -> String {
    if content.len() <= budget {
        return content.to_owned();
    }
    let marker = "\n\n... [truncated] ...\n\n";
    let available = budget.saturating_sub(marker.len());
    let head_size = available / 2;
    let tail_size = available.saturating_sub(head_size);

    // Find a valid char boundary near head_size
    let head_end = content
        .char_indices()
        .take_while(|(i, _)| *i < head_size)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);

    // Find a valid char boundary near content.len() - tail_size
    let tail_start_byte = content.len().saturating_sub(tail_size);
    let tail_start = content
        .char_indices()
        .take_while(|(i, _)| *i < tail_start_byte)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(tail_start_byte);

    let mut out = String::with_capacity(budget);
    out.push_str(&content[..head_end]);
    out.push_str(marker);
    if tail_start < content.len() {
        out.push_str(&content[tail_start..]);
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_progress::ToolProgressData;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A simple runner that immediately returns a fixed result.
    struct InstantRunner {
        content: String,
        is_error: bool,
    }

    impl ToolRunner for InstantRunner {
        fn run(
            &self,
            tool_call_id: &str,
            _name: &str,
            _input: &Value,
            _progress: &ProgressStream,
        ) -> JoinHandle<ToolExecutionResult> {
            let id = tool_call_id.to_owned();
            let content = self.content.clone();
            let is_error = self.is_error;
            tokio::spawn(async move {
                ToolExecutionResult {
                    tool_call_id: id,
                    content,
                    is_error,
                    duration: Duration::from_millis(1),
                }
            })
        }
    }

    fn make_executor() -> StreamingToolExecutor {
        StreamingToolExecutor::new(
            Arc::new(InstantRunner {
                content: "ok".into(),
                is_error: false,
            }),
            StreamingExecutorConfig {
                max_concurrency: 4,
                timeout: Some(Duration::from_secs(5)),
                max_result_bytes: 10_000,
            },
            None,
        )
    }

    #[tokio::test]
    async fn add_and_wait_for_single_tool() {
        let ex = make_executor();
        ex.add_tool("tc-1", "read_file", &Value::Null, true);
        let results = ex.wait_for_remaining().await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tool_call_id, "tc-1");
        assert!(!results[0].is_error);
    }

    #[tokio::test]
    async fn multiple_concurrent_safe_tools() {
        let ex = make_executor();
        ex.add_tool("tc-1", "read_file", &Value::Null, true);
        ex.add_tool("tc-2", "search", &Value::Null, true);
        ex.add_tool("tc-3", "web_fetch", &Value::Null, true);
        let results = ex.wait_for_remaining().await;
        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn non_concurrent_tools_run_serially() {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = Arc::clone(&counter);
        let runner = Arc::new(FnToolRunner {
            f: Arc::new(move |id, _name, _input, _progress| {
                counter_clone.fetch_add(1, Ordering::SeqCst);
                ToolExecutionResult {
                    tool_call_id: id.to_owned(),
                    content: "ok".into(),
                    is_error: false,
                    duration: Duration::from_millis(10),
                }
            }),
        });
        let ex = StreamingToolExecutor::new(
            runner,
            StreamingExecutorConfig {
                max_concurrency: 4,
                timeout: Some(Duration::from_secs(5)),
                max_result_bytes: 10_000,
            },
            None,
        );
        ex.add_tool("tc-1", "bash", &Value::Null, false);
        ex.add_tool("tc-2", "bash", &Value::Null, false);
        let results = ex.wait_for_remaining().await;
        assert_eq!(results.len(), 2);
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn discard_cancels_queued_tools() {
        let ex = make_executor();
        ex.add_tool("tc-1", "read_file", &Value::Null, true);
        ex.add_tool("tc-2", "read_file", &Value::Null, true);
        ex.discard();
        let results = ex.wait_for_remaining().await;
        // tc-1 may already be completed, tc-2 should be errored
        assert!(results.iter().any(|r| r.is_error));
    }

    #[test]
    fn completed_results_returns_empty_initially() {
        let ex = make_executor();
        assert!(ex.completed_results().is_empty());
    }

    #[tokio::test]
    async fn tool_count_tracks_additions() {
        let ex = make_executor();
        assert_eq!(ex.tool_count(), 0);
        ex.add_tool("tc-1", "read", &Value::Null, true);
        assert_eq!(ex.tool_count(), 1);
        ex.add_tool("tc-2", "write", &Value::Null, false);
        assert_eq!(ex.tool_count(), 2);
    }

    #[tokio::test]
    async fn tracked_calls_snapshot() {
        let ex = make_executor();
        ex.add_tool("tc-1", "read_file", &Value::Null, true);
        let calls = ex.tracked_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "tc-1");
        assert_eq!(calls[0].name, "read_file");
        assert!(calls[0].is_concurrency_safe);
    }

    #[test]
    fn has_errored_flag() {
        let ex = make_executor();
        assert!(!ex.has_errored());
        ex.mark_error("bash(command)");
        assert!(ex.has_errored());
    }

    #[test]
    fn apply_tool_result_budget_no_truncation_needed() {
        let result = apply_tool_result_budget("hello", 100);
        assert_eq!(result, "hello");
    }

    #[test]
    fn apply_tool_result_budget_truncates() {
        let long = "a".repeat(200);
        let result = apply_tool_result_budget(&long, 50);
        assert!(result.contains("[truncated]"));
        assert!(result.len() <= 200);
    }

    #[test]
    fn apply_tool_result_budget_exact_fit() {
        let content = "x".repeat(100);
        let result = apply_tool_result_budget(&content, 100);
        assert_eq!(result, content);
    }

    #[test]
    fn config_default_values() {
        let config = StreamingExecutorConfig::default();
        assert_eq!(config.max_concurrency, 10);
        assert_eq!(config.timeout, Some(Duration::from_secs(300)));
        assert_eq!(config.max_result_bytes, 100_000);
    }

    #[test]
    fn progress_stream_accessible() {
        let ex = make_executor();
        ex.progress_stream().emit(
            "tc-1",
            ToolProgressData::Spinner {
                message: Some("loading".into()),
            },
        );
        assert_eq!(ex.progress_stream().len(), 1);
    }

    #[tokio::test]
    async fn wait_for_remaining_empty() {
        let ex = make_executor();
        let results = ex.wait_for_remaining().await;
        assert!(results.is_empty());
    }

    #[test]
    fn tool_status_serialization() {
        let status = ToolStatus::Executing;
        let json = serde_json::to_string(&status).expect("serialize");
        assert_eq!(json, "\"executing\"");
        let back: ToolStatus = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, ToolStatus::Executing);
    }

    #[test]
    fn tracked_tool_call_serialization() {
        let call = TrackedToolCall {
            id: "tc-1".into(),
            name: "bash".into(),
            input: Value::Null,
            is_concurrency_safe: false,
            status: ToolStatus::Queued,
        };
        let json = serde_json::to_string(&call).expect("serialize");
        assert!(json.contains("\"tc-1\""));
        let back: TrackedToolCall = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.id, "tc-1");
    }
}
