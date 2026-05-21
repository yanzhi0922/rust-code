//! Batched state update queue and batch operations.
//!
//! Collects multiple server connection updates within a time window and
//! flushes them together, reducing the overhead of frequent individual
//! state updates. Also provides batch tool call and resource fetch
//! operations with result aggregation and error handling.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::connection::McpServerConnection;
use crate::resources::ServerResource;
use crate::types::{McpToolCallResult, McpToolDescriptor};

/// Default flush interval (16 ms ≈ one frame at 60 fps).
const DEFAULT_FLUSH_INTERVAL_MS: u64 = 16;

// ── Batch update queue (original) ─────────────────────────────────────────────

/// A single batched update for a server.
#[derive(Debug, Clone)]
pub struct BatchUpdate {
    /// Server name.
    pub server_name: String,
    /// Updated connection state.
    pub connection: McpServerConnection,
    /// Updated tools (if discovered).
    pub tools: Option<Vec<McpToolDescriptor>>,
    /// Updated resources (if discovered).
    pub resources: Option<Vec<ServerResource>>,
}

/// Batched update queue — merges multiple server updates within a time window.
#[derive(Debug)]
pub struct BatchedUpdateQueue {
    pending: Vec<BatchUpdate>,
    flush_interval: Duration,
}

impl BatchedUpdateQueue {
    /// Create a new queue with the default flush interval (16 ms).
    #[must_use]
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            flush_interval: Duration::from_millis(DEFAULT_FLUSH_INTERVAL_MS),
        }
    }

    /// Create a new queue with a custom flush interval.
    #[must_use]
    pub fn with_flush_interval(flush_interval: Duration) -> Self {
        Self {
            pending: Vec::new(),
            flush_interval,
        }
    }

    /// Enqueue an update. If an update for the same server already exists in
    /// the pending queue, it is replaced (last-write-wins).
    pub fn enqueue(&mut self, update: BatchUpdate) {
        // Replace existing entry for the same server if present.
        if let Some(existing) = self
            .pending
            .iter_mut()
            .find(|u| u.server_name == update.server_name)
        {
            *existing = update;
        } else {
            self.pending.push(update);
        }
    }

    /// Flush and return all pending updates, clearing the queue.
    pub fn flush(&mut self) -> Vec<BatchUpdate> {
        std::mem::take(&mut self.pending)
    }

    /// Return `true` if there are pending updates.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Return the number of pending updates.
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Return the configured flush interval.
    #[must_use]
    pub fn flush_interval(&self) -> Duration {
        self.flush_interval
    }
}

impl Default for BatchedUpdateQueue {
    fn default() -> Self {
        Self::new()
    }
}

// ── Batch tool call operation ─────────────────────────────────────────────────

/// A single tool call in a batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchToolCall {
    /// Server name to call the tool on.
    pub server_name: String,
    /// Tool name.
    pub tool_name: String,
    /// Tool call arguments.
    pub arguments: serde_json::Value,
}

impl BatchToolCall {
    /// Create a new batch tool call.
    #[must_use]
    pub fn new(
        server_name: impl Into<String>,
        tool_name: impl Into<String>,
        arguments: serde_json::Value,
    ) -> Self {
        Self {
            server_name: server_name.into(),
            tool_name: tool_name.into(),
            arguments,
        }
    }
}

/// Result of a single tool call within a batch.
#[derive(Debug, Clone)]
pub struct BatchToolCallResult {
    /// The original call request.
    pub call: BatchToolCall,
    /// The result (if successful).
    pub result: Option<McpToolCallResult>,
    /// Error message (if failed).
    pub error: Option<String>,
}

impl BatchToolCallResult {
    /// Create a successful result.
    #[must_use]
    pub fn success(call: BatchToolCall, result: McpToolCallResult) -> Self {
        Self {
            call,
            result: Some(result),
            error: None,
        }
    }

    /// Create a failed result.
    #[must_use]
    pub fn failure(call: BatchToolCall, error: impl Into<String>) -> Self {
        Self {
            call,
            result: None,
            error: Some(error.into()),
        }
    }

    /// Returns `true` if this result is successful.
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.error.is_none() && self.result.is_some()
    }

    /// Returns `true` if this result failed.
    #[must_use]
    pub fn is_failure(&self) -> bool {
        self.error.is_some()
    }
}

/// Batch tool call operation — executes multiple tool calls and aggregates results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpBatchOperation {
    /// Unique operation identifier.
    pub id: String,
    /// Tool calls to execute.
    pub calls: Vec<BatchToolCall>,
}

impl McpBatchOperation {
    /// Create a new batch operation.
    #[must_use]
    pub fn new(id: impl Into<String>, calls: Vec<BatchToolCall>) -> Self {
        Self {
            id: id.into(),
            calls,
        }
    }

    /// Create an empty batch operation.
    #[must_use]
    pub fn empty(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            calls: Vec::new(),
        }
    }

    /// Add a tool call to the batch.
    pub fn add_call(&mut self, call: BatchToolCall) {
        self.calls.push(call);
    }

    /// Return the number of calls in this batch.
    #[must_use]
    pub fn len(&self) -> usize {
        self.calls.len()
    }

    /// Returns `true` if there are no calls.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }

    /// Group calls by server name.
    #[must_use]
    pub fn group_by_server(&self) -> HashMap<&str, Vec<&BatchToolCall>> {
        let mut groups: HashMap<&str, Vec<&BatchToolCall>> = HashMap::new();
        for call in &self.calls {
            groups.entry(&call.server_name).or_default().push(call);
        }
        groups
    }
}

/// Aggregated results of a batch tool call operation.
#[derive(Debug, Clone)]
pub struct BatchOperationResults {
    /// The operation ID.
    pub operation_id: String,
    /// Individual results.
    pub results: Vec<BatchToolCallResult>,
}

impl BatchOperationResults {
    /// Create a new results container.
    #[must_use]
    pub fn new(operation_id: impl Into<String>) -> Self {
        Self {
            operation_id: operation_id.into(),
            results: Vec::new(),
        }
    }

    /// Add a result.
    pub fn add_result(&mut self, result: BatchToolCallResult) {
        self.results.push(result);
    }

    /// Returns `true` if all results are successful.
    #[must_use]
    pub fn all_succeeded(&self) -> bool {
        self.results.iter().all(|r| r.is_success())
    }

    /// Returns the number of successful results.
    #[must_use]
    pub fn success_count(&self) -> usize {
        self.results.iter().filter(|r| r.is_success()).count()
    }

    /// Returns the number of failed results.
    #[must_use]
    pub fn failure_count(&self) -> usize {
        self.results.iter().filter(|r| r.is_failure()).count()
    }

    /// Returns the total number of results.
    #[must_use]
    pub fn total(&self) -> usize {
        self.results.len()
    }

    /// Collect all error messages.
    #[must_use]
    pub fn errors(&self) -> Vec<&str> {
        self.results
            .iter()
            .filter_map(|r| r.error.as_deref())
            .collect()
    }

    /// Collect all successful results.
    #[must_use]
    pub fn successes(&self) -> Vec<&McpToolCallResult> {
        self.results
            .iter()
            .filter_map(|r| r.result.as_ref())
            .collect()
    }

    /// Aggregate all text content from successful results.
    #[must_use]
    pub fn aggregate_text(&self) -> String {
        let mut text = String::new();
        for result in &self.results {
            if let Some(ref tool_result) = result.result {
                for content in &tool_result.content {
                    if content.kind == "text"
                        && let Some(text_val) = content.fields.get("text")
                        && let Some(s) = text_val.as_str()
                    {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(s);
                    }
                }
            }
        }
        text
    }
}

// ── Batch resource fetch ──────────────────────────────────────────────────────

/// A single resource fetch in a batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchResourceFetch {
    /// Server name that owns the resource.
    pub server_name: String,
    /// Resource URI to fetch.
    pub uri: String,
}

impl BatchResourceFetch {
    /// Create a new batch resource fetch.
    #[must_use]
    pub fn new(server_name: impl Into<String>, uri: impl Into<String>) -> Self {
        Self {
            server_name: server_name.into(),
            uri: uri.into(),
        }
    }
}

/// Result of a single resource fetch within a batch.
#[derive(Debug, Clone)]
pub struct BatchResourceResult {
    /// The original fetch request.
    pub fetch: BatchResourceFetch,
    /// The fetched content (if successful).
    pub content: Option<Vec<u8>>,
    /// MIME type of the content.
    pub mime_type: Option<String>,
    /// Error message (if failed).
    pub error: Option<String>,
}

impl BatchResourceResult {
    /// Create a successful result.
    #[must_use]
    pub fn success(fetch: BatchResourceFetch, content: Vec<u8>, mime_type: Option<String>) -> Self {
        Self {
            fetch,
            content: Some(content),
            mime_type,
            error: None,
        }
    }

    /// Create a failed result.
    #[must_use]
    pub fn failure(fetch: BatchResourceFetch, error: impl Into<String>) -> Self {
        Self {
            fetch,
            content: None,
            mime_type: None,
            error: Some(error.into()),
        }
    }

    /// Returns `true` if this result is successful.
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.error.is_none() && self.content.is_some()
    }

    /// Returns `true` if this result failed.
    #[must_use]
    pub fn is_failure(&self) -> bool {
        self.error.is_some()
    }
}

/// Batch resource fetch operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpBatchResourceFetch {
    /// Unique operation identifier.
    pub id: String,
    /// Resource fetches to execute.
    pub fetches: Vec<BatchResourceFetch>,
}

impl McpBatchResourceFetch {
    /// Create a new batch resource fetch.
    #[must_use]
    pub fn new(id: impl Into<String>, fetches: Vec<BatchResourceFetch>) -> Self {
        Self {
            id: id.into(),
            fetches,
        }
    }

    /// Add a fetch to the batch.
    pub fn add_fetch(&mut self, fetch: BatchResourceFetch) {
        self.fetches.push(fetch);
    }

    /// Return the number of fetches.
    #[must_use]
    pub fn len(&self) -> usize {
        self.fetches.len()
    }

    /// Returns `true` if there are no fetches.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fetches.is_empty()
    }
}

/// Aggregated results of a batch resource fetch.
#[derive(Debug, Clone)]
pub struct BatchResourceResults {
    /// The operation ID.
    pub operation_id: String,
    /// Individual results.
    pub results: Vec<BatchResourceResult>,
}

impl BatchResourceResults {
    /// Create a new results container.
    #[must_use]
    pub fn new(operation_id: impl Into<String>) -> Self {
        Self {
            operation_id: operation_id.into(),
            results: Vec::new(),
        }
    }

    /// Add a result.
    pub fn add_result(&mut self, result: BatchResourceResult) {
        self.results.push(result);
    }

    /// Returns `true` if all results are successful.
    #[must_use]
    pub fn all_succeeded(&self) -> bool {
        self.results.iter().all(|r| r.is_success())
    }

    /// Returns the number of successful results.
    #[must_use]
    pub fn success_count(&self) -> usize {
        self.results.iter().filter(|r| r.is_success()).count()
    }

    /// Returns the number of failed results.
    #[must_use]
    pub fn failure_count(&self) -> usize {
        self.results.iter().filter(|r| r.is_failure()).count()
    }

    /// Total bytes fetched across all successful results.
    #[must_use]
    pub fn total_bytes(&self) -> usize {
        self.results
            .iter()
            .filter_map(|r| r.content.as_ref().map(|c| c.len()))
            .sum()
    }

    /// Collect all error messages.
    #[must_use]
    pub fn errors(&self) -> Vec<&str> {
        self.results
            .iter()
            .filter_map(|r| r.error.as_deref())
            .collect()
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{McpCapabilityMatrix, McpServerConfig};
    use crate::connection::{DisabledServer, PendingServer};
    use crate::scope::{ConfigScope, ScopedMcpServerConfig};
    use crate::transport::McpTransportConfig;
    use crate::types::McpToolCallContent;
    use std::collections::BTreeMap;

    fn test_scoped_config(name: &str) -> ScopedMcpServerConfig {
        ScopedMcpServerConfig::new(
            McpServerConfig {
                name: name.to_owned(),
                enabled: true,
                transport: McpTransportConfig::Stdio {
                    command: "echo".to_owned(),
                    args: vec![],
                    cwd: None,
                    env: BTreeMap::new(),
                },
                capabilities: McpCapabilityMatrix::default(),
                startup_timeout_secs: None,
                request_timeout_secs: None,
                metadata: BTreeMap::new(),
                oauth: None,
                tool_policy: crate::tool_policy::McpToolPolicy::default(),
            },
            ConfigScope::Local,
        )
    }

    fn pending_connection(name: &str) -> McpServerConnection {
        McpServerConnection::Pending(PendingServer {
            name: name.to_owned(),
            config: test_scoped_config(name),
            reconnect_attempt: None,
            max_reconnect_attempts: None,
        })
    }

    fn disabled_connection(name: &str) -> McpServerConnection {
        McpServerConnection::Disabled(DisabledServer {
            name: name.to_owned(),
            config: test_scoped_config(name),
        })
    }

    // ── BatchedUpdateQueue tests (original) ───────────────────────────────

    #[test]
    fn new_queue_is_empty() {
        let queue = BatchedUpdateQueue::new();
        assert!(!queue.has_pending());
        assert_eq!(queue.pending_count(), 0);
    }

    #[test]
    fn enqueue_adds_update() {
        let mut queue = BatchedUpdateQueue::new();
        queue.enqueue(BatchUpdate {
            server_name: "test".to_owned(),
            connection: pending_connection("test"),
            tools: None,
            resources: None,
        });
        assert!(queue.has_pending());
        assert_eq!(queue.pending_count(), 1);
    }

    #[test]
    fn enqueue_replaces_existing_server() {
        let mut queue = BatchedUpdateQueue::new();
        queue.enqueue(BatchUpdate {
            server_name: "test".to_owned(),
            connection: pending_connection("test"),
            tools: None,
            resources: None,
        });
        queue.enqueue(BatchUpdate {
            server_name: "test".to_owned(),
            connection: disabled_connection("test"),
            tools: None,
            resources: None,
        });
        assert_eq!(queue.pending_count(), 1);
        let updates = queue.flush();
        assert!(matches!(
            updates[0].connection,
            McpServerConnection::Disabled(_)
        ));
    }

    #[test]
    fn flush_clears_queue() {
        let mut queue = BatchedUpdateQueue::new();
        queue.enqueue(BatchUpdate {
            server_name: "a".to_owned(),
            connection: pending_connection("a"),
            tools: None,
            resources: None,
        });
        queue.enqueue(BatchUpdate {
            server_name: "b".to_owned(),
            connection: pending_connection("b"),
            tools: None,
            resources: None,
        });
        let updates = queue.flush();
        assert_eq!(updates.len(), 2);
        assert!(!queue.has_pending());
        assert_eq!(queue.pending_count(), 0);
    }

    #[test]
    fn flush_on_empty_returns_empty() {
        let mut queue = BatchedUpdateQueue::new();
        let updates = queue.flush();
        assert!(updates.is_empty());
    }

    #[test]
    fn custom_flush_interval() {
        let queue = BatchedUpdateQueue::with_flush_interval(Duration::from_millis(50));
        assert_eq!(queue.flush_interval(), Duration::from_millis(50));
    }

    #[test]
    fn default_impl() {
        let queue = BatchedUpdateQueue::default();
        assert_eq!(queue.flush_interval(), Duration::from_millis(16));
        assert!(!queue.has_pending());
    }

    // ── McpBatchOperation tests ───────────────────────────────────────────

    #[test]
    fn batch_operation_new() {
        let calls = vec![
            BatchToolCall::new("srv1", "tool1", serde_json::json!({})),
            BatchToolCall::new("srv2", "tool2", serde_json::json!({"key": "value"})),
        ];
        let op = McpBatchOperation::new("op-1", calls);
        assert_eq!(op.id, "op-1");
        assert_eq!(op.len(), 2);
        assert!(!op.is_empty());
    }

    #[test]
    fn batch_operation_empty() {
        let op = McpBatchOperation::empty("op-2");
        assert_eq!(op.id, "op-2");
        assert!(op.is_empty());
        assert_eq!(op.len(), 0);
    }

    #[test]
    fn batch_operation_add_call() {
        let mut op = McpBatchOperation::empty("op-3");
        op.add_call(BatchToolCall::new("srv", "tool", serde_json::json!({})));
        assert_eq!(op.len(), 1);
    }

    #[test]
    fn batch_operation_group_by_server() {
        let calls = vec![
            BatchToolCall::new("srv1", "tool1", serde_json::json!({})),
            BatchToolCall::new("srv1", "tool2", serde_json::json!({})),
            BatchToolCall::new("srv2", "tool3", serde_json::json!({})),
        ];
        let op = McpBatchOperation::new("op-4", calls);
        let groups = op.group_by_server();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups.get("srv1").map(|v| v.len()), Some(2));
        assert_eq!(groups.get("srv2").map(|v| v.len()), Some(1));
    }

    #[test]
    fn batch_tool_call_result_success() {
        let call = BatchToolCall::new("srv", "tool", serde_json::json!({}));
        let tool_result = McpToolCallResult {
            tool_result: None,
            content: vec![McpToolCallContent {
                kind: "text".to_owned(),
                fields: {
                    let mut m = BTreeMap::new();
                    m.insert("text".to_owned(), serde_json::json!("hello"));
                    m
                },
            }],
            structured_content: None,
            is_error: false,
        };
        let result = BatchToolCallResult::success(call, tool_result);
        assert!(result.is_success());
        assert!(!result.is_failure());
        assert!(result.result.is_some());
        assert!(result.error.is_none());
    }

    #[test]
    fn batch_tool_call_result_failure() {
        let call = BatchToolCall::new("srv", "tool", serde_json::json!({}));
        let result = BatchToolCallResult::failure(call, "connection refused");
        assert!(!result.is_success());
        assert!(result.is_failure());
        assert!(result.error.is_some());
        assert!(result.result.is_none());
    }

    #[test]
    fn batch_operation_results_aggregation() {
        let mut results = BatchOperationResults::new("op-1");

        // Add a success
        let call1 = BatchToolCall::new("srv1", "tool1", serde_json::json!({}));
        let tool_result = McpToolCallResult {
            tool_result: None,
            content: vec![McpToolCallContent {
                kind: "text".to_owned(),
                fields: {
                    let mut m = BTreeMap::new();
                    m.insert("text".to_owned(), serde_json::json!("output 1"));
                    m
                },
            }],
            structured_content: None,
            is_error: false,
        };
        results.add_result(BatchToolCallResult::success(call1, tool_result));

        // Add a failure
        let call2 = BatchToolCall::new("srv2", "tool2", serde_json::json!({}));
        results.add_result(BatchToolCallResult::failure(call2, "timeout"));

        assert!(!results.all_succeeded());
        assert_eq!(results.success_count(), 1);
        assert_eq!(results.failure_count(), 1);
        assert_eq!(results.total(), 2);
        assert_eq!(results.errors(), vec!["timeout"]);
    }

    #[test]
    fn batch_operation_results_aggregate_text() {
        let mut results = BatchOperationResults::new("op-2");

        for i in 0..3 {
            let call = BatchToolCall::new("srv", "tool", serde_json::json!({}));
            let tool_result = McpToolCallResult {
                tool_result: None,
                content: vec![McpToolCallContent {
                    kind: "text".to_owned(),
                    fields: {
                        let mut m = BTreeMap::new();
                        m.insert("text".to_owned(), serde_json::json!(format!("line {i}")));
                        m
                    },
                }],
                structured_content: None,
                is_error: false,
            };
            results.add_result(BatchToolCallResult::success(call, tool_result));
        }

        let text = results.aggregate_text();
        assert!(text.contains("line 0"));
        assert!(text.contains("line 1"));
        assert!(text.contains("line 2"));
        assert_eq!(text.matches('\n').count(), 2);
    }

    #[test]
    fn batch_operation_results_all_succeeded() {
        let mut results = BatchOperationResults::new("op-3");
        let call = BatchToolCall::new("srv", "tool", serde_json::json!({}));
        let tool_result = McpToolCallResult {
            tool_result: None,
            content: vec![],
            structured_content: None,
            is_error: false,
        };
        results.add_result(BatchToolCallResult::success(call, tool_result));
        assert!(results.all_succeeded());
    }

    // ── McpBatchResourceFetch tests ───────────────────────────────────────

    #[test]
    fn batch_resource_fetch_new() {
        let fetches = vec![
            BatchResourceFetch::new("srv1", "file:///a"),
            BatchResourceFetch::new("srv2", "file:///b"),
        ];
        let op = McpBatchResourceFetch::new("fetch-1", fetches);
        assert_eq!(op.id, "fetch-1");
        assert_eq!(op.len(), 2);
        assert!(!op.is_empty());
    }

    #[test]
    fn batch_resource_fetch_add() {
        let mut op = McpBatchResourceFetch::new("fetch-2", vec![]);
        assert!(op.is_empty());
        op.add_fetch(BatchResourceFetch::new("srv", "file:///x"));
        assert_eq!(op.len(), 1);
    }

    #[test]
    fn batch_resource_result_success() {
        let fetch = BatchResourceFetch::new("srv", "file:///data");
        let result = BatchResourceResult::success(
            fetch,
            vec![1, 2, 3, 4],
            Some("application/octet-stream".to_owned()),
        );
        assert!(result.is_success());
        assert!(!result.is_failure());
        assert_eq!(result.content.as_ref().map(|c| c.len()), Some(4));
        assert_eq!(
            result.mime_type.as_deref(),
            Some("application/octet-stream")
        );
    }

    #[test]
    fn batch_resource_result_failure() {
        let fetch = BatchResourceFetch::new("srv", "file:///missing");
        let result = BatchResourceResult::failure(fetch, "not found");
        assert!(!result.is_success());
        assert!(result.is_failure());
    }

    #[test]
    fn batch_resource_results_aggregation() {
        let mut results = BatchResourceResults::new("fetch-3");

        let fetch1 = BatchResourceFetch::new("srv1", "file:///a");
        results.add_result(BatchResourceResult::success(
            fetch1,
            vec![1, 2, 3],
            Some("text/plain".to_owned()),
        ));

        let fetch2 = BatchResourceFetch::new("srv2", "file:///b");
        results.add_result(BatchResourceResult::failure(fetch2, "permission denied"));

        let fetch3 = BatchResourceFetch::new("srv3", "file:///c");
        results.add_result(BatchResourceResult::success(
            fetch3,
            vec![4, 5, 6, 7, 8],
            None,
        ));

        assert!(!results.all_succeeded());
        assert_eq!(results.success_count(), 2);
        assert_eq!(results.failure_count(), 1);
        assert_eq!(results.total_bytes(), 8); // 3 + 5
        assert_eq!(results.errors(), vec!["permission denied"]);
    }

    #[test]
    fn batch_tool_call_serde() {
        let call = BatchToolCall::new("srv", "tool", serde_json::json!({"key": "val"}));
        let json = serde_json::to_string(&call).expect("serialize");
        let back: BatchToolCall = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.server_name, "srv");
        assert_eq!(back.tool_name, "tool");
        assert_eq!(back.arguments["key"], "val");
    }

    #[test]
    fn batch_operation_serde() {
        let op = McpBatchOperation::new(
            "op-serde",
            vec![BatchToolCall::new("s", "t", serde_json::json!({}))],
        );
        let json = serde_json::to_string(&op).expect("serialize");
        let back: McpBatchOperation = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.id, "op-serde");
        assert_eq!(back.calls.len(), 1);
    }
}
