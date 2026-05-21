//! Streaming tool executor for parallel tool execution with progress tracking.
//!
//! Manages concurrent execution of multiple tool calls, emitting progress
//! events via a broadcast channel for real-time monitoring.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use tokio::sync::broadcast;
use tokio::task::JoinSet;

use crate::config::{ProcessUserInputContext, ToolRunResult, ToolRunner};
use crate::tool_progress::{ToolProgressEvent, ToolProgressResult};
use claude_core::ToolCall;

/// Executor that can run multiple tool calls in parallel with streaming progress.
pub struct StreamingToolExecutor {
    /// Maximum number of tools to execute concurrently.
    max_parallel: usize,
    /// Broadcast sender for progress events.
    progress_tx: broadcast::Sender<ToolProgressEvent>,
}

/// Result of executing a batch of tool calls.
#[derive(Debug, Clone)]
pub struct BatchExecutionResult {
    /// Results keyed by tool call ID.
    pub results: HashMap<String, ToolRunResult>,
    /// IDs of tool calls that failed.
    pub failed_ids: Vec<String>,
    /// IDs of tool calls that succeeded.
    pub succeeded_ids: Vec<String>,
}

impl StreamingToolExecutor {
    /// Create a new streaming executor with the given parallelism limit.
    pub fn new(max_parallel: usize) -> Self {
        let (progress_tx, _) = broadcast::channel(256);
        Self {
            max_parallel,
            progress_tx,
        }
    }

    /// Returns the maximum parallelism.
    #[must_use]
    pub fn max_parallel(&self) -> usize {
        self.max_parallel
    }

    /// Subscribe to progress events.
    pub fn subscribe(&self) -> broadcast::Receiver<ToolProgressEvent> {
        self.progress_tx.subscribe()
    }

    /// Execute a batch of tool calls, running up to `max_parallel` concurrently.
    pub async fn execute_batch(
        &self,
        tool_calls: &[ToolCall],
        tool_runner: &Arc<dyn ToolRunner>,
        context: &ProcessUserInputContext,
    ) -> Result<BatchExecutionResult> {
        if tool_calls.is_empty() {
            return Ok(BatchExecutionResult {
                results: HashMap::new(),
                failed_ids: Vec::new(),
                succeeded_ids: Vec::new(),
            });
        }

        let mut join_set = JoinSet::new();
        let semaphore = Arc::new(tokio::sync::Semaphore::new(self.max_parallel));
        let mut results = HashMap::with_capacity(tool_calls.len());
        let mut failed_ids = Vec::new();
        let mut succeeded_ids = Vec::new();

        for tool_call in tool_calls {
            let permit = Arc::clone(&semaphore)
                .acquire_owned()
                .await
                .map_err(|_| anyhow!("semaphore closed"))?;
            let runner = Arc::clone(tool_runner);
            let tc = tool_call.clone();
            let ctx = context.clone();
            let tx = self.progress_tx.clone();

            join_set.spawn(async move {
                let _permit = permit;
                let _ = tx.send(ToolProgressEvent::Started {
                    tool_call_id: tc.id.clone(),
                    tool_name: tc.name.clone(),
                });
                match runner.run_tool(&tc, &ctx).await {
                    Ok(result) => {
                        let progress_result = ToolProgressResult::from(&result);
                        let _ = tx.send(ToolProgressEvent::Completed {
                            tool_call_id: tc.id.clone(),
                            result: progress_result,
                        });
                        (tc.id, Ok(result))
                    }
                    Err(error) => {
                        let _ = tx.send(ToolProgressEvent::Failed {
                            tool_call_id: tc.id.clone(),
                            error: format!("{error:#}"),
                        });
                        (tc.id, Err(error))
                    }
                }
            });
        }

        while let Some(join_result) = join_set.join_next().await {
            match join_result {
                Ok((id, Ok(result))) => {
                    succeeded_ids.push(id.clone());
                    results.insert(id, result);
                }
                Ok((id, Err(error))) => {
                    failed_ids.push(id.clone());
                    results.insert(
                        id,
                        ToolRunResult::from(claude_core::ToolResult {
                            content: format!("Tool execution error: {error:#}"),
                            is_error: true,
                            content_blocks: Vec::new(),
                            follow_up_user_blocks: Vec::new(),
                        }),
                    );
                }
                Err(join_error) => {
                    return Err(anyhow!("task join error: {join_error}"));
                }
            }
        }

        Ok(BatchExecutionResult {
            results,
            failed_ids,
            succeeded_ids,
        })
    }
}

impl Default for StreamingToolExecutor {
    fn default() -> Self {
        Self::new(4)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use anyhow::Result;
    use async_trait::async_trait;
    use claude_core::{PermissionMode, SessionId, ToolCall, ToolResult};
    use serde_json::json;

    use super::StreamingToolExecutor;
    use crate::config::{ProcessUserInputContext, ToolRunResult, ToolRunner};
    use crate::tool_progress::ToolProgressEvent;

    struct ImmediateToolRunner;

    #[async_trait]
    impl ToolRunner for ImmediateToolRunner {
        async fn run_tool(
            &self,
            tool_call: &ToolCall,
            _context: &ProcessUserInputContext,
        ) -> Result<ToolRunResult> {
            Ok(ToolRunResult::from(ToolResult {
                content: format!("result:{}", tool_call.name),
                is_error: false,
                content_blocks: Vec::new(),
                follow_up_user_blocks: Vec::new(),
            }))
        }
    }

    struct FailingToolRunner;

    #[async_trait]
    impl ToolRunner for FailingToolRunner {
        async fn run_tool(
            &self,
            tool_call: &ToolCall,
            _context: &ProcessUserInputContext,
        ) -> Result<ToolRunResult> {
            Err(anyhow::anyhow!("tool {} failed", tool_call.name))
        }
    }

    fn test_context() -> ProcessUserInputContext {
        ProcessUserInputContext::new(SessionId::new(), PermissionMode::Default, "test-model")
    }

    #[tokio::test]
    async fn streaming_executor_runs_tools_in_parallel() {
        let executor = StreamingToolExecutor::new(4);
        let runner: Arc<dyn ToolRunner> = Arc::new(ImmediateToolRunner);
        let context = test_context();
        let mut progress_rx = executor.subscribe();

        let tool_calls = vec![
            ToolCall {
                id: "tc-1".into(),
                name: "tool_a".into(),
                input: json!({}),
            },
            ToolCall {
                id: "tc-2".into(),
                name: "tool_b".into(),
                input: json!({}),
            },
        ];

        let result = executor
            .execute_batch(&tool_calls, &runner, &context)
            .await
            .expect("batch should succeed");

        assert_eq!(result.succeeded_ids.len(), 2);
        assert!(result.failed_ids.is_empty());

        // Collect progress events
        let mut events = Vec::new();
        while let Ok(event) = progress_rx.try_recv() {
            events.push(event);
        }
        assert!(events.iter().any(|e| matches!(e, ToolProgressEvent::Started { tool_call_id, .. } if tool_call_id == "tc-1")));
        assert!(events.iter().any(|e| matches!(e, ToolProgressEvent::Completed { tool_call_id, .. } if tool_call_id == "tc-2")));
    }

    #[tokio::test]
    async fn streaming_executor_handles_failures() {
        let executor = StreamingToolExecutor::new(2);
        let runner: Arc<dyn ToolRunner> = Arc::new(FailingToolRunner);
        let context = test_context();

        let tool_calls = vec![ToolCall {
            id: "tc-fail".into(),
            name: "bad_tool".into(),
            input: json!({}),
        }];

        let result = executor
            .execute_batch(&tool_calls, &runner, &context)
            .await
            .expect("batch should complete even with failures");

        assert!(result.failed_ids.contains(&"tc-fail".to_string()));
        assert!(result.succeeded_ids.is_empty());
    }

    #[tokio::test]
    async fn streaming_executor_empty_batch() {
        let executor = StreamingToolExecutor::new(2);
        let runner: Arc<dyn ToolRunner> = Arc::new(ImmediateToolRunner);
        let context = test_context();

        let result = executor
            .execute_batch(&[], &runner, &context)
            .await
            .expect("empty batch should succeed");

        assert!(result.results.is_empty());
        assert!(result.failed_ids.is_empty());
        assert!(result.succeeded_ids.is_empty());
    }

    #[tokio::test]
    async fn streaming_executor_respects_parallelism() {
        let executor = StreamingToolExecutor::new(1);
        assert_eq!(executor.max_parallel(), 1);

        let runner: Arc<dyn ToolRunner> = Arc::new(ImmediateToolRunner);
        let context = test_context();

        let tool_calls: Vec<ToolCall> = (0..3)
            .map(|i| ToolCall {
                id: format!("tc-{i}"),
                name: format!("tool-{i}"),
                input: json!({}),
            })
            .collect();

        let result = executor
            .execute_batch(&tool_calls, &runner, &context)
            .await
            .expect("should succeed");

        assert_eq!(result.succeeded_ids.len(), 3);
    }
}
