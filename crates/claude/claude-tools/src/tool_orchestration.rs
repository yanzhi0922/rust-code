//! Tool orchestration – parallel / serial dispatch with dependency analysis.
//!
//! Mirrors the upstream `toolOrchestration.ts` pattern: tool calls are
//! partitioned into *batches* where each batch is either a single
//! non-concurrency-safe tool (run serially) or a group of consecutive
//! concurrency-safe tools (run in parallel).

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::streaming_executor::{
    StreamingExecutorConfig, StreamingToolExecutor, ToolExecutionResult, ToolRunner,
};

// ---------------------------------------------------------------------------
// Batch types
// ---------------------------------------------------------------------------

/// A batch of tool calls that share the same concurrency safety.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolBatch {
    /// Whether every tool in this batch is concurrency-safe.
    pub is_concurrency_safe: bool,
    /// Tool call ids in this batch.
    pub tool_ids: Vec<String>,
}

/// A single tool call for orchestration input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratedToolCall {
    /// Unique tool-call identifier.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// JSON input.
    pub input: Value,
    /// Whether the tool is safe to run concurrently.
    pub is_concurrency_safe: bool,
}

// ---------------------------------------------------------------------------
// Dependency analysis
// ---------------------------------------------------------------------------

/// Describes a dependency edge between two tool calls.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDependency {
    /// Tool that must finish first.
    pub depends_on: String,
    /// Tool that waits.
    pub tool_id: String,
}

/// Analyse tool calls for explicit dependencies based on known patterns.
///
/// Currently this uses a simple heuristic: tools with the same file path
/// are considered dependent (write → read ordering).  This can be extended
/// with more sophisticated analysis.
pub fn analyse_dependencies(calls: &[OrchestratedToolCall]) -> Vec<ToolDependency> {
    let mut deps = Vec::new();

    // Group by file_path input
    let mut file_groups: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, call) in calls.iter().enumerate() {
        if let Some(fp) = call
            .input
            .get("file_path")
            .or_else(|| call.input.get("notebook_path"))
            .and_then(|v| v.as_str())
        {
            file_groups.entry(fp.to_owned()).or_default().push(i);
        }
    }

    for indices in file_groups.values() {
        // If there are both write-like and read-like tools on the same file,
        // the reads depend on the writes finishing first.
        let write_indices: Vec<usize> = indices
            .iter()
            .filter(|&&i| is_write_like(&calls[i].name))
            .copied()
            .collect();

        let read_indices: Vec<usize> = indices
            .iter()
            .filter(|&&i| !is_write_like(&calls[i].name))
            .copied()
            .collect();

        for &w in &write_indices {
            for &r in &read_indices {
                if w < r {
                    deps.push(ToolDependency {
                        depends_on: calls[w].id.clone(),
                        tool_id: calls[r].id.clone(),
                    });
                }
            }
        }
    }

    deps
}

fn is_write_like(name: &str) -> bool {
    matches!(
        name,
        "write_to_file"
            | "apply_diff"
            | "edit_file"
            | "notebook_edit"
            | "create_file"
            | "file_edit"
            | "bash"
            | "shell"
    )
}

// ---------------------------------------------------------------------------
// Partitioning
// ---------------------------------------------------------------------------

/// Partition tool calls into batches of consecutive concurrency-safe or
/// non-concurrency-safe tools.
///
/// Consecutive concurrency-safe tools are grouped into a single batch;
/// each non-concurrency-safe tool forms its own batch.
#[must_use]
pub fn partition_tool_calls(calls: &[OrchestratedToolCall]) -> Vec<ToolBatch> {
    let mut batches: Vec<ToolBatch> = Vec::new();

    for call in calls {
        if call.is_concurrency_safe {
            if let Some(last) = batches.last_mut()
                && last.is_concurrency_safe
            {
                last.tool_ids.push(call.id.clone());
                continue;
            }
            batches.push(ToolBatch {
                is_concurrency_safe: true,
                tool_ids: vec![call.id.clone()],
            });
        } else {
            batches.push(ToolBatch {
                is_concurrency_safe: false,
                tool_ids: vec![call.id.clone()],
            });
        }
    }

    batches
}

// ---------------------------------------------------------------------------
// Execution strategies
// ---------------------------------------------------------------------------

/// Strategy for dispatching tool batches.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchStrategy {
    /// Run batches in order: concurrent batches in parallel, serial batches one-by-one.
    #[default]
    Auto,
    /// Force all tools to run serially.
    SerialOnly,
    /// Force all tools to run in parallel (ignoring safety).
    ForceParallel,
}

/// Result of orchestrating a set of tool calls.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestrationResult {
    /// All tool execution results, keyed by tool-call id.
    pub results: HashMap<String, ToolExecutionResult>,
    /// Total wall-clock time.
    pub total_duration_ms: u64,
    /// Number of batches executed.
    pub batch_count: usize,
    /// Strategy used.
    pub strategy: DispatchStrategy,
}

// ---------------------------------------------------------------------------
// ToolOrchestrator
// ---------------------------------------------------------------------------

/// Orchestrates the execution of multiple tool calls using configurable
/// dispatch strategies.
pub struct ToolOrchestrator {
    config: StreamingExecutorConfig,
    strategy: DispatchStrategy,
    runner: Arc<dyn ToolRunner>,
}

impl ToolOrchestrator {
    /// Create a new orchestrator with the given runner and config.
    pub fn new(runner: Arc<dyn ToolRunner>, config: StreamingExecutorConfig) -> Self {
        Self {
            config,
            strategy: DispatchStrategy::default(),
            runner,
        }
    }

    /// Override the dispatch strategy.
    pub fn with_strategy(mut self, strategy: DispatchStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    /// Execute a slice of tool calls and return the orchestration result.
    pub async fn execute(&self, calls: &[OrchestratedToolCall]) -> Result<OrchestrationResult> {
        let start = std::time::Instant::now();
        let mut results: HashMap<String, ToolExecutionResult> = HashMap::new();

        match self.strategy {
            DispatchStrategy::SerialOnly => {
                self.execute_serial(calls, &mut results).await?;
            }
            DispatchStrategy::ForceParallel => {
                self.execute_parallel(calls, &mut results).await?;
            }
            DispatchStrategy::Auto => {
                let batches = partition_tool_calls(calls);
                let call_map: HashMap<&str, &OrchestratedToolCall> =
                    calls.iter().map(|c| (c.id.as_str(), c)).collect();

                for batch in &batches {
                    let batch_calls: Vec<&OrchestratedToolCall> = batch
                        .tool_ids
                        .iter()
                        .filter_map(|id| call_map.get(id.as_str()))
                        .copied()
                        .collect();

                    if batch.is_concurrency_safe {
                        self.execute_batch_parallel(&batch_calls, &mut results)
                            .await?;
                    } else {
                        self.execute_batch_serial(&batch_calls, &mut results)
                            .await?;
                    }
                }
            }
        }

        Ok(OrchestrationResult {
            results,
            total_duration_ms: start.elapsed().as_millis() as u64,
            batch_count: partition_tool_calls(calls).len(),
            strategy: self.strategy,
        })
    }

    // -- Serial execution ---------------------------------------------------

    async fn execute_serial(
        &self,
        calls: &[OrchestratedToolCall],
        results: &mut HashMap<String, ToolExecutionResult>,
    ) -> Result<()> {
        for call in calls {
            let executor =
                StreamingToolExecutor::new(Arc::clone(&self.runner), self.config.clone(), None);
            executor.add_tool(
                &call.id,
                &call.name,
                &call.input,
                false, // force non-concurrent
            );
            let mut batch_results = executor.wait_for_remaining().await;
            for r in batch_results.drain(..) {
                results.insert(r.tool_call_id.clone(), r);
            }
        }
        Ok(())
    }

    // -- Parallel execution -------------------------------------------------

    async fn execute_parallel(
        &self,
        calls: &[OrchestratedToolCall],
        results: &mut HashMap<String, ToolExecutionResult>,
    ) -> Result<()> {
        let executor =
            StreamingToolExecutor::new(Arc::clone(&self.runner), self.config.clone(), None);
        for call in calls {
            executor.add_tool(
                &call.id,
                &call.name,
                &call.input,
                true, // force concurrent
            );
        }
        let batch_results = executor.wait_for_remaining().await;
        for r in batch_results {
            results.insert(r.tool_call_id.clone(), r);
        }
        Ok(())
    }

    // -- Batch helpers ------------------------------------------------------

    async fn execute_batch_serial(
        &self,
        calls: &[&OrchestratedToolCall],
        results: &mut HashMap<String, ToolExecutionResult>,
    ) -> Result<()> {
        for call in calls {
            let executor =
                StreamingToolExecutor::new(Arc::clone(&self.runner), self.config.clone(), None);
            executor.add_tool(&call.id, &call.name, &call.input, false);
            let mut batch_results = executor.wait_for_remaining().await;
            for r in batch_results.drain(..) {
                results.insert(r.tool_call_id.clone(), r);
            }
        }
        Ok(())
    }

    async fn execute_batch_parallel(
        &self,
        calls: &[&OrchestratedToolCall],
        results: &mut HashMap<String, ToolExecutionResult>,
    ) -> Result<()> {
        let executor =
            StreamingToolExecutor::new(Arc::clone(&self.runner), self.config.clone(), None);
        for call in calls {
            executor.add_tool(&call.id, &call.name, &call.input, true);
        }
        let batch_results = executor.wait_for_remaining().await;
        for r in batch_results {
            results.insert(r.tool_call_id.clone(), r);
        }
        Ok(())
    }

    /// Analyse dependencies in the given tool calls.
    pub fn analyse_deps(&self, calls: &[OrchestratedToolCall]) -> Vec<ToolDependency> {
        analyse_dependencies(calls)
    }

    /// Partition calls into batches based on concurrency safety.
    pub fn partition(&self, calls: &[OrchestratedToolCall]) -> Vec<ToolBatch> {
        partition_tool_calls(calls)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_progress::ProgressStream;
    use std::sync::Arc;
    use std::time::Duration;

    struct OkRunner;
    impl ToolRunner for OkRunner {
        fn run(
            &self,
            id: &str,
            _name: &str,
            _input: &Value,
            _progress: &ProgressStream,
        ) -> tokio::task::JoinHandle<ToolExecutionResult> {
            let id = id.to_owned();
            tokio::spawn(async move {
                ToolExecutionResult {
                    tool_call_id: id,
                    content: "ok".into(),
                    is_error: false,
                    duration: Duration::from_millis(1),
                }
            })
        }
    }

    fn config() -> StreamingExecutorConfig {
        StreamingExecutorConfig {
            max_concurrency: 4,
            timeout: Some(Duration::from_secs(5)),
            max_result_bytes: 10_000,
        }
    }

    fn call(id: &str, name: &str, safe: bool) -> OrchestratedToolCall {
        OrchestratedToolCall {
            id: id.into(),
            name: name.into(),
            input: Value::Null,
            is_concurrency_safe: safe,
        }
    }

    fn call_with_file(id: &str, name: &str, file: &str, safe: bool) -> OrchestratedToolCall {
        let mut input = serde_json::Map::new();
        input.insert("file_path".into(), Value::String(file.into()));
        OrchestratedToolCall {
            id: id.into(),
            name: name.into(),
            input: Value::Object(input),
            is_concurrency_safe: safe,
        }
    }

    // -- partition_tool_calls tests -----------------------------------------

    #[test]
    fn partition_empty() {
        assert!(partition_tool_calls(&[]).is_empty());
    }

    #[test]
    fn partition_single_safe() {
        let batches = partition_tool_calls(&[call("1", "read", true)]);
        assert_eq!(batches.len(), 1);
        assert!(batches[0].is_concurrency_safe);
        assert_eq!(batches[0].tool_ids, vec!["1"]);
    }

    #[test]
    fn partition_single_unsafe() {
        let batches = partition_tool_calls(&[call("1", "bash", false)]);
        assert_eq!(batches.len(), 1);
        assert!(!batches[0].is_concurrency_safe);
    }

    #[test]
    fn partition_consecutive_safe_grouped() {
        let calls = vec![
            call("1", "read", true),
            call("2", "search", true),
            call("3", "read", true),
        ];
        let batches = partition_tool_calls(&calls);
        assert_eq!(batches.len(), 1);
        assert!(batches[0].is_concurrency_safe);
        assert_eq!(batches[0].tool_ids.len(), 3);
    }

    #[test]
    fn partition_mixed() {
        let calls = vec![
            call("1", "read", true),
            call("2", "bash", false),
            call("3", "read", true),
        ];
        let batches = partition_tool_calls(&calls);
        assert_eq!(batches.len(), 3);
        assert!(batches[0].is_concurrency_safe);
        assert!(!batches[1].is_concurrency_safe);
        assert!(batches[2].is_concurrency_safe);
    }

    #[test]
    fn partition_two_unsafe_not_grouped() {
        let calls = vec![call("1", "bash", false), call("2", "bash", false)];
        let batches = partition_tool_calls(&calls);
        assert_eq!(batches.len(), 2);
        assert!(!batches[0].is_concurrency_safe);
        assert!(!batches[1].is_concurrency_safe);
    }

    // -- analyse_dependencies tests -----------------------------------------

    #[test]
    fn analyse_deps_no_deps() {
        let calls = vec![call("1", "read", true), call("2", "read", true)];
        assert!(analyse_dependencies(&calls).is_empty());
    }

    #[test]
    fn analyse_deps_write_read_same_file() {
        let calls = vec![
            call_with_file("1", "write_to_file", "a.rs", false),
            call_with_file("2", "read_file", "a.rs", true),
        ];
        let deps = analyse_dependencies(&calls);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].depends_on, "1");
        assert_eq!(deps[0].tool_id, "2");
    }

    #[test]
    fn analyse_deps_different_files_no_dep() {
        let calls = vec![
            call_with_file("1", "write_to_file", "a.rs", false),
            call_with_file("2", "read_file", "b.rs", true),
        ];
        assert!(analyse_dependencies(&calls).is_empty());
    }

    // -- ToolOrchestrator integration tests ---------------------------------

    #[tokio::test]
    async fn orchestrator_auto_strategy() {
        let orch = ToolOrchestrator::new(Arc::new(OkRunner), config());
        let calls = vec![
            call("1", "read", true),
            call("2", "search", true),
            call("3", "bash", false),
        ];
        let result = orch.execute(&calls).await.expect("execute");
        assert_eq!(result.results.len(), 3);
        assert_eq!(result.strategy, DispatchStrategy::Auto);
        assert!(!result.results.get("1").expect("r1").is_error);
    }

    #[tokio::test]
    async fn orchestrator_serial_strategy() {
        let orch = ToolOrchestrator::new(Arc::new(OkRunner), config())
            .with_strategy(DispatchStrategy::SerialOnly);
        let calls = vec![call("1", "read", true), call("2", "bash", false)];
        let result = orch.execute(&calls).await.expect("execute");
        assert_eq!(result.results.len(), 2);
        assert_eq!(result.strategy, DispatchStrategy::SerialOnly);
    }

    #[tokio::test]
    async fn orchestrator_force_parallel_strategy() {
        let orch = ToolOrchestrator::new(Arc::new(OkRunner), config())
            .with_strategy(DispatchStrategy::ForceParallel);
        let calls = vec![call("1", "bash", false), call("2", "bash", false)];
        let result = orch.execute(&calls).await.expect("execute");
        assert_eq!(result.results.len(), 2);
        assert_eq!(result.strategy, DispatchStrategy::ForceParallel);
    }

    #[tokio::test]
    async fn orchestrator_empty_calls() {
        let orch = ToolOrchestrator::new(Arc::new(OkRunner), config());
        let result = orch.execute(&[]).await.expect("execute");
        assert!(result.results.is_empty());
        assert_eq!(result.batch_count, 0);
    }

    #[test]
    fn dispatch_strategy_default() {
        assert_eq!(DispatchStrategy::default(), DispatchStrategy::Auto);
    }

    #[test]
    fn orchestration_result_serialization() {
        let result = OrchestrationResult {
            results: HashMap::new(),
            total_duration_ms: 42,
            batch_count: 2,
            strategy: DispatchStrategy::Auto,
        };
        let json = serde_json::to_string(&result).expect("serialize");
        assert!(json.contains("\"auto\""));
        let back: OrchestrationResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.total_duration_ms, 42);
    }
}
