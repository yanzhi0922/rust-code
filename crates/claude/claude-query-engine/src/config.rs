use std::collections::{BTreeMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use claude_core::{
    AgentId, FileHistoryState, Message, PermissionMode, SessionId, ToolPermissionContext,
    ToolResult,
};
use claude_provider::ConversationBackend;
use claude_provider::context::ContextWindowManager;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::observer::{NoopQueryObserver, QueryObserver};
use crate::stop_hooks::{ReplHookContext, StopHookOutcome, StopHookPipeline, StopHookRequest};

pub type PostCompactTransform = dyn Fn(
        Vec<claude_core::ConversationEntry>,
    ) -> Pin<Box<dyn Future<Output = Vec<claude_core::ConversationEntry>> + Send>>
    + Send
    + Sync;

pub type CompactConversationHandler = dyn Fn(
        Vec<claude_core::ConversationEntry>,
        ContextWindowManager,
    ) -> Pin<
        Box<dyn Future<Output = Option<(Vec<claude_core::ConversationEntry>, String)>> + Send>,
    > + Send
    + Sync;

pub type PostSamplingHook =
    dyn Fn(ReplHookContext) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync;

pub type StopHook = dyn Fn(
        ReplHookContext,
        StopHookRequest,
    ) -> Pin<Box<dyn Future<Output = Result<StopHookOutcome>> + Send>>
    + Send
    + Sync;

/// Re-export EffortLevel from the canonical definition in claude-context.
pub use claude_context::effort::EffortLevel;

/// Source that initiated a query.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuerySource {
    #[default]
    User,
    ReplMainThread,
    Sdk,
    Compact,
    SessionMemory,
    Agent,
    ExtractMemories,
    BackgroundTask,
}

/// Provider invocation mode for a compat query run.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderInvocationMode {
    #[default]
    Buffered,
    Streaming,
}

/// Thinking/extended reasoning controls.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThinkingConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub adaptive: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u32>,
}

/// Optional task budget limits injected per query.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskBudget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_total_tokens: Option<u64>,
    /// Tokens already consumed by completed sub-agents.
    /// When a sub-agent finishes, its `usage.output_tokens` are added here.
    /// The remaining budget for the parent is `max_total_tokens - consumed_tokens`.
    #[serde(default)]
    pub consumed_tokens: u64,
    /// Maximum USD budget for this query.
    /// Mirrors TS `maxBudgetUsd`. When set, the query loop calculates
    /// accumulated cost after each provider response and stops if exceeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_budget_usd: Option<f64>,
}

impl TaskBudget {
    /// Returns the remaining token budget, or `None` if no budget was set.
    #[must_use]
    pub fn remaining(&self) -> Option<u64> {
        self.max_total_tokens
            .map(|total| total.saturating_sub(self.consumed_tokens))
    }

    /// Record tokens consumed by a completed sub-agent.
    pub fn record_sub_agent_usage(&mut self, tokens: u64) {
        self.consumed_tokens = self.consumed_tokens.saturating_add(tokens);
    }

    /// Returns `true` if a USD budget cap is configured.
    #[must_use]
    pub fn has_usd_budget(&self) -> bool {
        self.max_budget_usd.is_some()
    }
}

/// Host-side context passed into a query run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessUserInputContext {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<AgentId>,
    #[serde(default)]
    pub permission_mode: PermissionMode,
    #[serde(default)]
    pub tool_permission_context: ToolPermissionContext,
    #[serde(default)]
    pub file_history: FileHistoryState,
    #[serde(default)]
    pub thinking_config: ThinkingConfig,
    #[serde(default)]
    pub effort: EffortLevel,
    /// Effort explicitly configured by the runtime. `effort` above has a UI
    /// default, while this stays `None` unless the user/settings supplied one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_effort: Option<String>,
    #[serde(default)]
    pub fast_mode: bool,
    #[serde(default)]
    pub query_source: QuerySource,
    pub model: String,
    /// Per-request provider model override used by fallback retries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_model_override: Option<String>,
    /// Per-request provider output token limit override used by truncation recovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens_override: Option<u32>,
    #[serde(skip)]
    pub task_budget: Arc<parking_lot::Mutex<Option<TaskBudget>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_instructions: Option<String>,
    #[serde(default)]
    pub discovered_skills: HashSet<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub user_context: BTreeMap<String, String>,
    #[serde(default)]
    pub system_context: BTreeMap<String, String>,
}

impl ProcessUserInputContext {
    #[must_use]
    pub fn new(
        session_id: SessionId,
        permission_mode: PermissionMode,
        model: impl Into<String>,
    ) -> Self {
        Self {
            session_id,
            agent_id: None,
            permission_mode,
            tool_permission_context: ToolPermissionContext::default(),
            file_history: FileHistoryState::default(),
            thinking_config: ThinkingConfig::default(),
            effort: EffortLevel::default(),
            requested_effort: None,
            fast_mode: false,
            query_source: QuerySource::default(),
            model: model.into(),
            provider_model_override: None,
            max_output_tokens_override: None,
            task_budget: Arc::new(parking_lot::Mutex::new(None)),
            memory_content: None,
            mcp_instructions: None,
            discovered_skills: HashSet::new(),
            system_prompt: None,
            user_context: BTreeMap::new(),
            system_context: BTreeMap::new(),
        }
    }
}

/// Host-provided tool execution seam for the compat query engine.
#[derive(Debug, Clone)]
pub struct ToolRunResult {
    pub result: ToolResult,
    pub pre_messages: Vec<Message>,
    pub post_messages: Vec<Message>,
    pub permission_denial: Option<Value>,
    /// Output tokens consumed by this tool invocation (e.g. sub-agent).
    /// When set, the query engine records these against the task budget
    /// so the parent's remaining budget is reduced accordingly.
    pub output_tokens_consumed: Option<u64>,
}

impl From<ToolResult> for ToolRunResult {
    fn from(result: ToolResult) -> Self {
        Self {
            result,
            pre_messages: Vec::new(),
            post_messages: Vec::new(),
            permission_denial: None,
            output_tokens_consumed: None,
        }
    }
}

/// Host-provided tool execution seam for the compat query engine.
#[async_trait]
pub trait ToolRunner: Send + Sync {
    async fn run_tool(
        &self,
        tool_call: &claude_core::ToolCall,
        context: &ProcessUserInputContext,
    ) -> Result<ToolRunResult>;
}

/// Immutable configuration for the compat query engine.
pub struct QueryEngineConfig {
    pub session_id: SessionId,
    pub model: String,
    pub backend: Arc<dyn ConversationBackend>,
    pub tool_runner: Arc<dyn ToolRunner>,
    pub observer: Arc<dyn QueryObserver>,
    pub event_stream: rc_engine_events::EventStream,
    pub provider_invocation_mode: ProviderInvocationMode,
    pub max_turns: u32,
    pub context_manager: ContextWindowManager,
    pub failure_threshold: usize,
    /// Maximum number of parallel tool executions.
    pub max_parallel_tools: usize,
    /// Optional JSON Schema for structured output enforcement.
    pub structured_output_schema: Option<Value>,
    /// Maximum retries for stop hooks.
    pub stop_hook_max_retries: usize,
    /// Optional fallback model for runtime model switching.
    pub fallback_model: Option<String>,
    /// Whether to enable tool result summarization.
    pub enable_tool_summarization: bool,
    /// Maximum tool result length before summarization.
    pub tool_result_max_length: usize,
    /// Maximum chain nesting depth for sub-queries.
    pub max_chain_depth: u32,
    pub compact_conversation_handler: Option<Arc<CompactConversationHandler>>,
    pub post_compact_transform: Option<Arc<PostCompactTransform>>,
    pub post_sampling_hooks: Vec<Arc<PostSamplingHook>>,
    pub stop_hook: Option<Arc<StopHook>>,
    pub stop_hook_pipeline: Option<Arc<StopHookPipeline>>,
    #[allow(dead_code)]
    pub metadata: Value,
}

impl QueryEngineConfig {
    #[must_use]
    pub fn new(
        session_id: SessionId,
        model: impl Into<String>,
        backend: Arc<dyn ConversationBackend>,
        tool_runner: Arc<dyn ToolRunner>,
        event_stream: rc_engine_events::EventStream,
    ) -> Self {
        let model = model.into();
        Self {
            session_id,
            context_manager: ContextWindowManager::for_model(&model),
            model,
            backend,
            tool_runner,
            observer: Arc::new(NoopQueryObserver),
            event_stream,
            provider_invocation_mode: ProviderInvocationMode::Buffered,
            max_turns: 8,
            failure_threshold: 3,
            max_parallel_tools: 4,
            structured_output_schema: None,
            stop_hook_max_retries: 3,
            fallback_model: None,
            enable_tool_summarization: true,
            tool_result_max_length: 10_000,
            max_chain_depth: 4,
            compact_conversation_handler: None,
            post_compact_transform: None,
            post_sampling_hooks: Vec::new(),
            stop_hook: None,
            stop_hook_pipeline: None,
            metadata: Value::Null,
        }
    }

    #[must_use]
    pub fn with_observer(mut self, observer: Arc<dyn QueryObserver>) -> Self {
        self.observer = observer;
        self
    }

    #[must_use]
    pub fn with_provider_invocation_mode(mut self, mode: ProviderInvocationMode) -> Self {
        self.provider_invocation_mode = mode;
        self
    }

    #[must_use]
    pub fn with_fallback_model(mut self, model: impl Into<String>) -> Self {
        self.fallback_model = Some(model.into());
        self
    }

    #[must_use]
    pub fn with_structured_output_schema(mut self, schema: Value) -> Self {
        self.structured_output_schema = Some(schema);
        self
    }

    #[must_use]
    pub fn with_max_parallel_tools(mut self, max: usize) -> Self {
        self.max_parallel_tools = max;
        self
    }

    #[must_use]
    pub fn with_post_compact_transform(mut self, transform: Arc<PostCompactTransform>) -> Self {
        self.post_compact_transform = Some(transform);
        self
    }

    #[must_use]
    pub fn with_compact_conversation_handler(
        mut self,
        handler: Arc<CompactConversationHandler>,
    ) -> Self {
        self.compact_conversation_handler = Some(handler);
        self
    }

    #[must_use]
    pub fn with_post_sampling_hook(mut self, hook: Arc<PostSamplingHook>) -> Self {
        self.post_sampling_hooks.push(hook);
        self
    }

    #[must_use]
    pub fn with_stop_hook(mut self, hook: Arc<StopHook>) -> Self {
        self.stop_hook = Some(hook);
        self
    }

    #[must_use]
    pub fn with_stop_hook_pipeline(mut self, pipeline: Arc<StopHookPipeline>) -> Self {
        self.stop_hook_pipeline = Some(pipeline);
        self
    }
}
