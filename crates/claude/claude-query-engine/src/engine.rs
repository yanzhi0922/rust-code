use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use claude_core::{
    AssistantContentBlock, AssistantMessage, ConversationEntry, Message, MessageBase,
    MessageOrigin, SessionId, SystemMessage, SystemMessageSubtype, ToolCall, ToolUseSummaryMessage,
    UsageAccumulator,
};
use rc_engine_events::{EngineEvent, Usage};
use serde_json::{Value, json};
use thiserror::Error;
use uuid::Uuid;

use crate::chain::{ChainManager, QueryChain};
use crate::config::{ProcessUserInputContext, QueryEngineConfig};
use crate::failure_tracker::FailureTracker;
use crate::model_switch::ModelSwitcher;
use crate::observer::QueryObserverEvent;
use crate::query_loop::run_query_loop;
use crate::state_machine::{EnginePhase, StateMachine};
use crate::stop_hooks::StopHookManager;
use crate::structured_output::StructuredOutputEnforcer;
use crate::token_budget::BudgetTracker;
use crate::tool_summary::ToolResultSummarizer;

/// Runtime error returned by the compat query engine.
#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Other(#[from] anyhow::Error),
    #[error("query stopped: {0}")]
    Stopped(String),
    /// Prompt exceeds the model's context window.
    #[error("prompt too long: {reason:?}")]
    PromptTooLong {
        reason: crate::query_loop::PromptTooLongReason,
    },
    /// Model is temporarily overloaded (503 / rate-limit).
    #[error("model overloaded")]
    ModelOverloaded,
    /// Max output tokens reached — response was truncated.
    #[error("max output tokens reached")]
    MaxTokensReached,
}

/// Mutable state carried across query turns.
#[derive(Debug, Clone)]
pub struct EngineState {
    pub turn: u32,
    pub messages: Vec<Message>,
    pub usage: UsageAccumulator,
    pub budget_tracker: BudgetTracker,
    pub stop_reason: Option<String>,
    pub consecutive_failures: usize,
    pub permission_denials: Vec<Value>,
    /// Explicit state machine tracking the engine's current phase.
    pub state_machine: StateMachine,
    /// Chain tracking for nested query execution.
    pub current_chain: Option<QueryChain>,
    /// Failure tracker with circuit breaker.
    pub failure_tracker: FailureTracker,
    /// Model switcher for runtime model changes.
    pub model_switcher: ModelSwitcher,
    /// Stop hook manager for graceful termination.
    pub stop_hook_manager: StopHookManager,
    /// Structured output enforcer.
    pub structured_output: StructuredOutputEnforcer,
    /// Tool result summarizer.
    pub tool_summarizer: ToolResultSummarizer,
    /// Accumulated USD cost across all turns.
    /// Mirrors TS `costUsd` tracking in the query loop.
    pub accumulated_usd_cost: f64,
}

impl EngineState {
    #[must_use]
    pub fn new(messages: Vec<Message>, budget_tracker: BudgetTracker) -> Self {
        Self {
            turn: 1,
            messages,
            usage: UsageAccumulator::default(),
            budget_tracker,
            stop_reason: None,
            consecutive_failures: 0,
            permission_denials: Vec::new(),
            state_machine: StateMachine::new(),
            current_chain: None,
            failure_tracker: FailureTracker::default(),
            model_switcher: ModelSwitcher::new("unknown"),
            stop_hook_manager: StopHookManager::default(),
            structured_output: StructuredOutputEnforcer::new(),
            tool_summarizer: ToolResultSummarizer::default(),
            accumulated_usd_cost: 0.0,
        }
    }

    /// Convert the current v2 message state into the legacy provider transcript format.
    #[must_use]
    pub fn legacy_conversation(&self) -> Vec<ConversationEntry> {
        self.messages
            .iter()
            .filter_map(Message::as_conversation_entry)
            .collect()
    }

    pub(crate) fn replace_from_legacy(&mut self, conversation: &[ConversationEntry]) {
        self.messages = conversation.iter().cloned().map(Message::from).collect();
    }

    /// Returns the current engine phase.
    #[must_use]
    pub fn phase(&self) -> EnginePhase {
        self.state_machine.phase()
    }
}

/// Final result of a compat query engine run.
#[derive(Debug, Clone)]
pub struct QueryResult {
    pub state: EngineState,
    pub final_text: Option<String>,
    pub stop_reason: String,
    pub turns: u32,
    pub permission_denials: Vec<Value>,
}

/// Minimal query engine that owns state/config and delegates loop execution.
pub struct QueryEngine {
    config: QueryEngineConfig,
    state: EngineState,
    chain_manager: ChainManager,
    /// Shared interrupt flag. When set to `true`, the query loop will abort
    /// at the next cancellation point (mirrors TS `AbortController.abort()`).
    interrupted: Arc<AtomicBool>,
}

impl QueryEngine {
    #[must_use]
    pub fn new(config: QueryEngineConfig, existing_messages: Vec<Message>) -> Self {
        let budget_tracker = BudgetTracker::new(config.max_turns, None);
        let mut model_switcher = ModelSwitcher::new(&config.model);
        if let Some(fallback_model) = config.fallback_model.as_ref() {
            model_switcher = model_switcher.with_fallback(fallback_model.clone());
        }
        let chain_manager = ChainManager::new(config.max_chain_depth);
        let failure_tracker =
            FailureTracker::new(config.failure_threshold, std::time::Duration::from_secs(30));
        let stop_hook_manager = StopHookManager::new(config.stop_hook_max_retries);
        let structured_output = match &config.structured_output_schema {
            Some(schema) => StructuredOutputEnforcer::with_schema(schema.clone()),
            None => StructuredOutputEnforcer::new(),
        };
        let tool_summarizer = if config.enable_tool_summarization {
            ToolResultSummarizer::new(config.tool_result_max_length, 2_000)
        } else {
            let mut s = ToolResultSummarizer::new(config.tool_result_max_length, 2_000);
            s.disable();
            s
        };

        let mut state = EngineState::new(existing_messages, budget_tracker);
        state.model_switcher = model_switcher;
        state.failure_tracker = failure_tracker;
        state.stop_hook_manager = stop_hook_manager;
        state.structured_output = structured_output;
        state.tool_summarizer = tool_summarizer;

        Self {
            config,
            state,
            chain_manager,
            interrupted: Arc::new(AtomicBool::new(false)),
        }
    }

    #[must_use]
    pub fn state(&self) -> &EngineState {
        &self.state
    }

    /// Signal the running query loop to abort at the next cancellation point.
    ///
    /// Mirrors TS `QueryEngine.interrupt()` → `this.abortController.abort()`.
    pub fn interrupt(&self) {
        self.interrupted.store(true, Ordering::SeqCst);
    }

    /// Check whether an interrupt has been requested.
    pub fn is_interrupted(&self) -> bool {
        self.interrupted.load(Ordering::SeqCst)
    }

    /// Returns a reference to the chain manager.
    #[must_use]
    pub fn chain_manager(&self) -> &ChainManager {
        &self.chain_manager
    }

    /// Submit new user input and execute the compat query loop to completion.
    pub async fn submit_message(
        &mut self,
        user_input: Vec<Message>,
        context: ProcessUserInputContext,
    ) -> Result<QueryResult, EngineError> {
        // Reset interrupt flag for the new query
        self.interrupted.store(false, Ordering::SeqCst);
        let started = Instant::now();
        let existing_messages = self.state.messages.len();
        let new_messages = user_input.len();

        // Start a new chain for this query
        let chain = self.chain_manager.start_root(context.query_source);
        self.state.current_chain = Some(chain);

        // Transition state machine to Initializing
        let _ = self
            .state
            .state_machine
            .transition(EnginePhase::Initializing);

        self.config.event_stream.emit(EngineEvent::QueryStarted {
            session_id: event_session_id(&context.session_id),
        });
        let _ = self
            .config
            .observer
            .on_event(QueryObserverEvent::QueryStarted {
                session_id: context.session_id.clone(),
                existing_messages,
                new_messages,
            })
            .await;
        let result = run_query_loop(
            &self.config,
            &mut self.state,
            user_input,
            &context,
            self.interrupted.clone(),
        )
        .await;
        match &result {
            Ok(query_result) => {
                // Transition to Idle via Finalizing
                let _ = self.state.state_machine.transition(EnginePhase::Finalizing);
                let _ = self.state.state_machine.transition(EnginePhase::Idle);

                self.config.event_stream.emit(EngineEvent::QueryCompleted {
                    session_id: event_session_id(&context.session_id),
                    duration_ms: started.elapsed().as_millis() as u64,
                });
                let _ = self
                    .config
                    .observer
                    .on_event(QueryObserverEvent::QueryFinished {
                        stop_reason: query_result.stop_reason.clone(),
                        turns: query_result.turns,
                        final_text: query_result.final_text.clone(),
                        usage: usage_from_accumulator(&query_result.state.usage),
                    })
                    .await;
            }
            Err(error) => {
                // Transition to Failed
                self.state.state_machine.force_set(EnginePhase::Failed);

                self.config.event_stream.emit(EngineEvent::QueryAborted {
                    session_id: event_session_id(&context.session_id),
                });
                let _ = self
                    .config
                    .observer
                    .on_event(QueryObserverEvent::QueryFailed {
                        error: error.to_string(),
                        turns: self.state.turn,
                        consecutive_failures: self.state.consecutive_failures,
                        usage: usage_from_accumulator(&self.state.usage),
                    })
                    .await;
            }
        }

        // End the chain
        if let Some(ref chain) = self.state.current_chain {
            self.chain_manager.end_chain(chain.id());
        }
        self.state.current_chain = None;

        result
    }
}

fn event_session_id(session_id: &SessionId) -> Uuid {
    session_id.try_as_uuid().unwrap_or_else(|_| Uuid::nil())
}

pub(crate) fn usage_from_accumulator(accumulator: &UsageAccumulator) -> Usage {
    Usage {
        input_tokens: accumulator.input_tokens,
        output_tokens: accumulator.output_tokens,
        cache_creation_input_tokens: accumulator.cache_creation_input_tokens,
        cache_read_input_tokens: accumulator.cache_read_input_tokens,
        total_tokens: accumulator.total_tokens(),
        server_tool_use_web_search_requests: accumulator.server_tool_use_web_search_requests,
        server_tool_use_web_fetch_requests: accumulator.server_tool_use_web_fetch_requests,
        cache_creation_ephemeral_5m_input_tokens: accumulator
            .cache_creation_ephemeral_5m_input_tokens,
        cache_creation_ephemeral_1h_input_tokens: accumulator
            .cache_creation_ephemeral_1h_input_tokens,
    }
}

#[allow(dead_code)]
pub(crate) fn assistant_message_from_response(response: &claude_core::ProviderResponse) -> Message {
    assistant_message_from_response_with_parent(response, None)
}

pub(crate) fn assistant_message_from_response_with_parent(
    response: &claude_core::ProviderResponse,
    parent_uuid: Option<Uuid>,
) -> Message {
    let mut blocks = Vec::new();
    if !response.text.trim().is_empty() {
        blocks.push(AssistantContentBlock::Text {
            text: response.text.clone(),
        });
    }
    if let Some(thinking) = response.thinking.clone()
        && !thinking.trim().is_empty()
    {
        blocks.push(AssistantContentBlock::Thinking {
            text: thinking,
            signature: None,
        });
    }
    for tool_call in &response.tool_calls {
        blocks.push(AssistantContentBlock::ToolUse {
            id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            input: tool_call.input.clone(),
        });
    }

    let has_non_tool_content_blocks = response.content_blocks.iter().any(|block| {
        !matches!(
            block.get("type").and_then(Value::as_str),
            Some("tool_use" | "server_tool_use")
        )
    });
    let provider_content_blocks = if has_non_tool_content_blocks {
        response.content_blocks.clone()
    } else {
        let mut provider_content_blocks = Vec::new();
        if let Some(thinking) = response.thinking.clone()
            && !thinking.trim().is_empty()
        {
            provider_content_blocks.push(json!({
                "type": "thinking",
                "thinking": thinking,
            }));
        }
        if !response.text.trim().is_empty() {
            provider_content_blocks.push(json!({
                "type": "text",
                "text": response.text,
            }));
        }
        if response.content_blocks.is_empty() {
            provider_content_blocks.extend(response.tool_calls.iter().map(|tool_call| {
                json!({
                    "type": "tool_use",
                    "id": tool_call.id,
                    "name": tool_call.name,
                    "input": tool_call.input,
                })
            }));
        } else {
            provider_content_blocks.extend(response.content_blocks.clone());
        }
        provider_content_blocks
    };

    let mut base = MessageBase::with_origin(MessageOrigin::Provider);
    base.parent_uuid = parent_uuid;
    Message::Assistant(AssistantMessage {
        base,
        text: response.text.clone(),
        blocks,
        tool_calls: response.tool_calls.clone(),
        provider_content_blocks,
    })
}

#[allow(dead_code)]
pub(crate) fn tool_result_message(
    tool_call: &ToolCall,
    result: &claude_core::ToolResult,
) -> Message {
    tool_result_message_with_parent(tool_call, result, None)
}

pub(crate) fn tool_result_message_with_parent(
    tool_call: &ToolCall,
    result: &claude_core::ToolResult,
    parent_uuid: Option<Uuid>,
) -> Message {
    let mut base = MessageBase::with_origin(MessageOrigin::Tool);
    base.parent_uuid = parent_uuid;
    Message::ToolUseSummary(ToolUseSummaryMessage {
        base,
        tool_call_id: tool_call.id.clone(),
        tool_name: tool_call.name.clone(),
        summary: result.content.clone(),
        is_error: result.is_error,
        content_blocks: result.content_blocks.clone(),
    })
}

pub(crate) fn budget_stop_message(reason: impl Into<String>) -> Message {
    Message::System(SystemMessage {
        base: MessageBase::with_origin(MessageOrigin::System),
        subtype: SystemMessageSubtype::Informational,
        text: reason.into(),
        error: None,
    })
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;
    use std::collections::VecDeque;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use anyhow::{Result, anyhow};
    use async_trait::async_trait;
    use claude_core::{
        ConversationEntry, PermissionMode, ProviderResponse, SessionId, SubAgentCompletion,
        ToolCall, ToolResult, UsageSummary,
    };
    use claude_provider::context::ContextWindowManager;
    use claude_provider::{ConversationBackend, StreamingCallbacks};
    use rc_engine_events::EngineEvent;
    use serde_json::json;
    use tokio::sync::broadcast::Receiver;

    use super::{QueryEngine, assistant_message_from_response};
    use crate::config::{
        ProcessUserInputContext, ProviderInvocationMode, QueryEngineConfig, ToolRunResult,
        ToolRunner,
    };
    use crate::observer::{QueryCheckpointKind, QueryObserver, QueryObserverEvent};
    use crate::stop_hooks::{ReplHookContext, StopHookOutcome, StopHookRequest};

    struct DummyCompletion;

    #[async_trait]
    impl SubAgentCompletion for DummyCompletion {
        async fn complete(
            &self,
            _conversation: &[ConversationEntry],
        ) -> anyhow::Result<ProviderResponse> {
            Ok(ProviderResponse::default())
        }
    }

    #[derive(Debug, Clone)]
    enum MockStreamingEvent {
        TextDelta(&'static str),
        ToolCallStart(&'static str, &'static str),
        ToolCallDelta(&'static str, &'static str),
        Usage(u64, u64),
    }

    struct MockBackend {
        responses: Mutex<VecDeque<ProviderResponse>>,
        stream_scripts: Mutex<VecDeque<Vec<MockStreamingEvent>>>,
        errors: Mutex<VecDeque<String>>,
        contexts: Mutex<Vec<claude_provider::query_source::ProviderRequestContext>>,
        complete_calls: AtomicUsize,
        complete_streaming_calls: AtomicUsize,
    }

    impl MockBackend {
        fn new(responses: Vec<ProviderResponse>) -> Self {
            Self {
                responses: Mutex::new(VecDeque::from(responses)),
                stream_scripts: Mutex::new(VecDeque::new()),
                errors: Mutex::new(VecDeque::new()),
                contexts: Mutex::new(Vec::new()),
                complete_calls: AtomicUsize::new(0),
                complete_streaming_calls: AtomicUsize::new(0),
            }
        }

        fn with_errors(errors: Vec<&'static str>, responses: Vec<ProviderResponse>) -> Self {
            Self {
                responses: Mutex::new(VecDeque::from(responses)),
                stream_scripts: Mutex::new(VecDeque::new()),
                errors: Mutex::new(errors.into_iter().map(str::to_owned).collect()),
                contexts: Mutex::new(Vec::new()),
                complete_calls: AtomicUsize::new(0),
                complete_streaming_calls: AtomicUsize::new(0),
            }
        }

        fn with_stream_scripts(
            responses: Vec<ProviderResponse>,
            stream_scripts: Vec<Vec<MockStreamingEvent>>,
        ) -> Self {
            Self {
                responses: Mutex::new(VecDeque::from(responses)),
                stream_scripts: Mutex::new(VecDeque::from(stream_scripts)),
                errors: Mutex::new(VecDeque::new()),
                contexts: Mutex::new(Vec::new()),
                complete_calls: AtomicUsize::new(0),
                complete_streaming_calls: AtomicUsize::new(0),
            }
        }

        fn contexts(&self) -> Vec<claude_provider::query_source::ProviderRequestContext> {
            self.contexts.lock().clone()
        }
    }

    #[test]
    fn assistant_message_from_response_preserves_provider_blocks_for_replay() {
        let response = ProviderResponse {
            text: String::new(),
            history_text: None,
            thinking: Some("reasoning".to_owned()),
            content_blocks: vec![json!({
                "type": "tool_use",
                "id": "call-1",
                "name": "read_file",
                "input": {"path": "src/lib.rs"},
            })],
            tool_calls: vec![ToolCall {
                id: "call-1".to_owned(),
                name: "read_file".to_owned(),
                input: json!({"path":"src/lib.rs"}),
            }],
            request_id: None,
            usage: UsageSummary::default(),
            stop_reason: "tool_use".to_owned(),
            research: None,
        };

        let message = assistant_message_from_response(&response);
        let entry = message
            .as_conversation_entry()
            .expect("assistant should down-convert");

        assert_eq!(entry.content_blocks.len(), 2);
        assert_eq!(entry.content_blocks[0]["type"], "thinking");
        assert_eq!(entry.content_blocks[0]["thinking"], "reasoning");
        assert_eq!(entry.content_blocks[1]["type"], "tool_use");
        assert_eq!(entry.content_blocks[1]["id"], "call-1");
    }

    #[async_trait]
    impl ConversationBackend for MockBackend {
        async fn complete(&self, _conversation: &[ConversationEntry]) -> Result<ProviderResponse> {
            self.complete_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(error) = self.errors.lock().pop_front() {
                return Err(anyhow!(error));
            }
            self.responses
                .lock()
                .pop_front()
                .ok_or_else(|| anyhow!("no more responses"))
        }

        async fn complete_with_context(
            &self,
            conversation: &[ConversationEntry],
            context: &claude_provider::query_source::ProviderRequestContext,
        ) -> Result<ProviderResponse> {
            self.contexts.lock().push(context.clone());
            self.complete(conversation).await
        }

        async fn complete_streaming(
            &self,
            conversation: &[ConversationEntry],
            callbacks: Option<StreamingCallbacks>,
        ) -> Result<ProviderResponse> {
            self.complete_streaming_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(script) = self.stream_scripts.lock().pop_front()
                && let Some(callbacks) = callbacks.as_ref()
            {
                for event in script {
                    match event {
                        MockStreamingEvent::TextDelta(delta) => {
                            if let Some(callback) = callbacks.on_text_delta.as_ref() {
                                callback(delta);
                            }
                        }
                        MockStreamingEvent::ToolCallStart(tool_call_id, tool_name) => {
                            if let Some(callback) = callbacks.on_tool_call_start.as_ref() {
                                callback(tool_call_id, tool_name);
                            }
                        }
                        MockStreamingEvent::ToolCallDelta(tool_call_id, delta) => {
                            if let Some(callback) = callbacks.on_tool_call_delta.as_ref() {
                                callback(tool_call_id, delta);
                            }
                        }
                        MockStreamingEvent::Usage(input_tokens, output_tokens) => {
                            if let Some(callback) = callbacks.on_usage.as_ref() {
                                callback(claude_provider::streaming::StreamingUsageUpdate {
                                    input_tokens,
                                    output_tokens,
                                    ..Default::default()
                                });
                            }
                        }
                    }
                }
            }
            self.responses
                .lock()
                .pop_front()
                .ok_or_else(|| anyhow!("no more responses for streaming call {conversation:?}"))
        }

        async fn complete_streaming_with_context(
            &self,
            conversation: &[ConversationEntry],
            callbacks: Option<StreamingCallbacks>,
            context: &claude_provider::query_source::ProviderRequestContext,
        ) -> Result<ProviderResponse> {
            self.contexts.lock().push(context.clone());
            self.complete_streaming(conversation, callbacks).await
        }

        fn sub_agent_completion(&self) -> Arc<dyn SubAgentCompletion> {
            Arc::new(DummyCompletion)
        }
    }

    struct MockToolRunner;

    #[async_trait]
    impl ToolRunner for MockToolRunner {
        async fn run_tool(
            &self,
            tool_call: &ToolCall,
            _context: &ProcessUserInputContext,
        ) -> Result<ToolRunResult> {
            Ok(ToolRunResult::from(ToolResult {
                content: format!("tool:{} ok", tool_call.name),
                is_error: false,
                content_blocks: Vec::new(),
                follow_up_user_blocks: Vec::new(),
            }))
        }
    }

    #[derive(Default)]
    struct RecordingObserver {
        events: Mutex<Vec<QueryObserverEvent>>,
    }

    impl RecordingObserver {
        fn snapshot(&self) -> Vec<QueryObserverEvent> {
            self.events.lock().clone()
        }
    }

    #[async_trait]
    impl QueryObserver for RecordingObserver {
        async fn on_event(&self, event: QueryObserverEvent) -> Result<()> {
            self.events.lock().push(event);
            Ok(())
        }
    }

    fn drain_engine_events(receiver: &mut Receiver<EngineEvent>) -> Vec<EngineEvent> {
        let mut events = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            events.push(event);
        }
        events
    }

    #[tokio::test]
    async fn query_engine_completes_basic_tool_round_trip() {
        let session_id = SessionId::new();
        let observer = Arc::new(RecordingObserver::default());
        let backend = Arc::new(MockBackend::new(vec![
            ProviderResponse {
                text: String::new(),
                history_text: None,
                thinking: None,
                content_blocks: Vec::new(),
                tool_calls: vec![ToolCall {
                    id: "tool-1".to_owned(),
                    name: "bash_command".to_owned(),
                    input: serde_json::json!({"command": "echo hi"}),
                }],
                request_id: None,
                usage: UsageSummary {
                    input_tokens: 10,
                    output_tokens: 5,
                    cache_read_input_tokens: 0,
                    cache_creation_input_tokens: 0,
                    ..Default::default()
                },
                stop_reason: "tool_use".to_owned(),
                research: None,
            },
            ProviderResponse {
                text: "done".to_owned(),
                history_text: None,
                thinking: None,
                content_blocks: Vec::new(),
                tool_calls: Vec::new(),
                request_id: None,
                usage: UsageSummary {
                    input_tokens: 3,
                    output_tokens: 7,
                    cache_read_input_tokens: 0,
                    cache_creation_input_tokens: 0,
                    ..Default::default()
                },
                stop_reason: "end_turn".to_owned(),
                research: None,
            },
        ]));
        let config = QueryEngineConfig::new(
            session_id.clone(),
            "mock-model",
            backend,
            Arc::new(MockToolRunner),
            rc_engine_events::EventStream::new(64),
        )
        .with_observer(observer.clone());
        let mut engine_events = config.event_stream.subscribe();
        let mut engine = QueryEngine::new(
            config,
            vec![claude_core::Message::from(ConversationEntry::system("sys"))],
        );
        let context =
            ProcessUserInputContext::new(session_id, PermissionMode::Default, "mock-model");
        let result = engine
            .submit_message(
                vec![claude_core::Message::from(ConversationEntry::user("hello"))],
                context,
            )
            .await
            .expect("query engine should succeed");

        assert_eq!(result.final_text.as_deref(), Some("done"));
        assert_eq!(result.stop_reason, "end_turn");
        assert_eq!(result.turns, 3);
        assert_eq!(result.state.usage.input_tokens, 13);
        assert_eq!(result.state.usage.output_tokens, 12);
        assert!(
            result
                .state
                .messages
                .iter()
                .filter_map(claude_core::Message::as_conversation_entry)
                .any(|entry| entry.role == claude_core::ConversationRole::Tool)
        );

        let observer_events = observer.snapshot();
        assert!(observer_events.iter().any(|event| matches!(
            event,
            QueryObserverEvent::MessagesAppended { appended, .. } if appended.len() == 1
        )));
        assert!(observer_events.iter().any(|event| matches!(
            event,
            QueryObserverEvent::AssistantMessageCommitted { stop_reason, .. }
                if stop_reason == "tool_use"
        )));
        assert!(observer_events.iter().any(|event| matches!(
            event,
            QueryObserverEvent::ToolCallStarted { tool_call, .. } if tool_call.id == "tool-1"
        )));
        assert!(observer_events.iter().any(|event| matches!(
            event,
            QueryObserverEvent::ToolResultCommitted { tool_call, result, .. }
                if tool_call.id == "tool-1" && result.content == "tool:bash_command ok"
        )));
        assert!(observer_events.iter().any(|event| matches!(
            event,
            QueryObserverEvent::CheckpointCreated { checkpoint }
                if checkpoint.kind == QueryCheckpointKind::ResumeBoundary
                    && checkpoint.tool_use_ids == vec!["tool-1".to_owned()]
        )));
        assert!(observer_events.iter().any(|event| matches!(
            event,
            QueryObserverEvent::CheckpointCleared { checkpoint }
                if checkpoint.kind == QueryCheckpointKind::ToolBatch
        )));
        assert!(observer_events.iter().any(|event| matches!(
            event,
            QueryObserverEvent::QueryFinished { stop_reason, final_text, .. }
                if stop_reason == "end_turn" && final_text.as_deref() == Some("done")
        )));

        let engine_events = drain_engine_events(&mut engine_events);
        assert!(
            engine_events
                .iter()
                .any(|event| matches!(event, EngineEvent::QueryStarted { .. }))
        );
        assert!(engine_events.iter().any(|event| matches!(
            event,
            EngineEvent::ToolUseStarted { tool_use_id, .. } if tool_use_id.as_ref() == "tool-1"
        )));
        assert!(
            engine_events
                .iter()
                .any(|event| matches!(event, EngineEvent::QueryCompleted { .. }))
        );
    }

    #[tokio::test]
    async fn query_engine_reports_budget_stop_to_observer_and_event_stream() {
        let session_id = SessionId::new();
        let observer = Arc::new(RecordingObserver::default());
        let backend = Arc::new(MockBackend::new(Vec::new()));
        let config = QueryEngineConfig::new(
            session_id.clone(),
            "mock-model",
            backend,
            Arc::new(MockToolRunner),
            rc_engine_events::EventStream::new(16),
        )
        .with_observer(observer.clone());
        let mut engine_events = config.event_stream.subscribe();
        let mut engine = QueryEngine::new(
            config,
            vec![claude_core::Message::from(ConversationEntry::system("sys"))],
        );
        let mut context =
            ProcessUserInputContext::new(session_id, PermissionMode::Default, "mock-model");
        context.task_budget =
            std::sync::Arc::new(parking_lot::Mutex::new(Some(crate::TaskBudget {
                max_turns: Some(0),
                max_total_tokens: None,
                consumed_tokens: 0,
                max_budget_usd: None,
            })));

        let error = engine
            .submit_message(
                vec![claude_core::Message::from(ConversationEntry::user("hello"))],
                context,
            )
            .await
            .expect_err("budget stop should abort before provider call");

        match error {
            crate::EngineError::Stopped(reason) => {
                assert_eq!(reason, "turn budget exceeded (0)");
            }
            other => panic!("unexpected error: {other:?}"),
        }

        let observer_events = observer.snapshot();
        assert!(observer_events.iter().any(|event| matches!(
            event,
            QueryObserverEvent::BudgetExceeded { reason, .. }
                if reason == "turn budget exceeded (0)"
        )));
        assert!(observer_events.iter().any(|event| matches!(
            event,
            QueryObserverEvent::QueryFailed { error, .. }
                if error == "query stopped: turn budget exceeded (0)"
        )));

        let engine_events = drain_engine_events(&mut engine_events);
        assert!(
            engine_events
                .iter()
                .any(|event| matches!(event, EngineEvent::QueryAborted { .. }))
        );
    }

    #[tokio::test]
    async fn query_engine_emits_streaming_observer_events_when_opted_in() {
        let session_id = SessionId::new();
        let observer = Arc::new(RecordingObserver::default());
        let backend = Arc::new(MockBackend::with_stream_scripts(
            vec![
                ProviderResponse {
                    text: String::new(),
                    history_text: None,
                    thinking: None,
                    content_blocks: Vec::new(),
                    tool_calls: vec![ToolCall {
                        id: "tool-1".to_owned(),
                        name: "bash_command".to_owned(),
                        input: serde_json::json!({"command": "echo hi"}),
                    }],
                    request_id: None,
                    usage: UsageSummary {
                        input_tokens: 10,
                        output_tokens: 5,
                        cache_read_input_tokens: 0,
                        cache_creation_input_tokens: 0,
                        ..Default::default()
                    },
                    stop_reason: "tool_use".to_owned(),
                    research: None,
                },
                ProviderResponse {
                    text: "done".to_owned(),
                    history_text: None,
                    thinking: None,
                    content_blocks: Vec::new(),
                    tool_calls: Vec::new(),
                    request_id: None,
                    usage: UsageSummary {
                        input_tokens: 2,
                        output_tokens: 4,
                        cache_read_input_tokens: 0,
                        cache_creation_input_tokens: 0,
                        ..Default::default()
                    },
                    stop_reason: "end_turn".to_owned(),
                    research: None,
                },
            ],
            vec![
                vec![
                    MockStreamingEvent::ToolCallStart("tool-1", "bash_command"),
                    MockStreamingEvent::ToolCallDelta("tool-1", "{\"command\":\"echo"),
                    MockStreamingEvent::ToolCallDelta("tool-1", " hi\"}"),
                    MockStreamingEvent::Usage(10, 5),
                ],
                vec![
                    MockStreamingEvent::TextDelta("do"),
                    MockStreamingEvent::TextDelta("ne"),
                    MockStreamingEvent::Usage(2, 4),
                ],
            ],
        ));
        let config = QueryEngineConfig::new(
            session_id.clone(),
            "mock-model",
            Arc::clone(&backend) as Arc<dyn ConversationBackend>,
            Arc::new(MockToolRunner),
            rc_engine_events::EventStream::new(32),
        )
        .with_observer(observer.clone())
        .with_provider_invocation_mode(ProviderInvocationMode::Streaming);
        let mut engine = QueryEngine::new(
            config,
            vec![claude_core::Message::from(ConversationEntry::system("sys"))],
        );
        let context =
            ProcessUserInputContext::new(session_id, PermissionMode::Default, "mock-model");

        let result = engine
            .submit_message(
                vec![claude_core::Message::from(ConversationEntry::user("hello"))],
                context,
            )
            .await
            .expect("streaming query should succeed");

        assert_eq!(result.final_text.as_deref(), Some("done"));
        assert_eq!(backend.complete_calls.load(Ordering::SeqCst), 0);
        assert_eq!(backend.complete_streaming_calls.load(Ordering::SeqCst), 2);

        let observer_events = observer.snapshot();
        assert!(observer_events.iter().any(|event| matches!(
            event,
            QueryObserverEvent::StreamingToolCallStarted {
                tool_call_id,
                tool_name,
                ..
            } if tool_call_id == "tool-1" && tool_name == "bash_command"
        )));
        assert!(observer_events.iter().any(|event| matches!(
            event,
            QueryObserverEvent::StreamingToolCallDelta {
                tool_call_id,
                delta,
                ..
            } if tool_call_id == "tool-1" && delta.contains("echo")
        )));
        assert!(observer_events.iter().any(|event| matches!(
            event,
            QueryObserverEvent::StreamingTextDelta {
                delta,
                accumulated_text,
                ..
            } if delta == "ne" && accumulated_text == "done"
        )));
        assert!(observer_events.iter().any(|event| matches!(
            event,
            QueryObserverEvent::StreamingUsageUpdated { usage, .. }
                if usage.input_tokens == 2 && usage.output_tokens == 4
        )));
    }

    #[tokio::test]
    async fn query_engine_fallback_model_reaches_provider_context() {
        let session_id = SessionId::new();
        let backend = Arc::new(MockBackend::with_errors(
            vec!["provider overloaded"],
            vec![ProviderResponse {
                text: "fallback ok".to_owned(),
                history_text: None,
                thinking: None,
                content_blocks: Vec::new(),
                tool_calls: Vec::new(),
                request_id: None,
                usage: UsageSummary::default(),
                stop_reason: "end_turn".to_owned(),
                research: None,
            }],
        ));
        let config = QueryEngineConfig::new(
            session_id.clone(),
            "primary-model",
            Arc::clone(&backend) as Arc<dyn ConversationBackend>,
            Arc::new(MockToolRunner),
            rc_engine_events::EventStream::new(8),
        )
        .with_fallback_model("fallback-model");
        let mut engine = QueryEngine::new(
            config,
            vec![claude_core::Message::from(ConversationEntry::system("sys"))],
        );
        let context =
            ProcessUserInputContext::new(session_id, PermissionMode::Default, "primary-model");

        let result = engine
            .submit_message(
                vec![claude_core::Message::from(ConversationEntry::user("hello"))],
                context,
            )
            .await
            .expect("fallback query should succeed");

        assert_eq!(result.final_text.as_deref(), Some("fallback ok"));
        let contexts = backend.contexts();
        assert_eq!(contexts.len(), 2);
        assert_eq!(contexts[0].model_override, None);
        assert_eq!(
            contexts[1].model_override.as_deref(),
            Some("fallback-model")
        );
    }

    #[tokio::test]
    async fn query_engine_max_token_escalation_retries_with_provider_override() {
        let session_id = SessionId::new();
        let backend = Arc::new(MockBackend::new(vec![
            ProviderResponse {
                text: "partial".to_owned(),
                history_text: None,
                thinking: None,
                content_blocks: Vec::new(),
                tool_calls: Vec::new(),
                request_id: None,
                usage: UsageSummary {
                    input_tokens: 100,
                    output_tokens: 8_192,
                    cache_read_input_tokens: 0,
                    cache_creation_input_tokens: 0,
                    ..Default::default()
                },
                stop_reason: "max_tokens".to_owned(),
                research: None,
            },
            ProviderResponse {
                text: "full".to_owned(),
                history_text: None,
                thinking: None,
                content_blocks: Vec::new(),
                tool_calls: Vec::new(),
                request_id: None,
                usage: UsageSummary {
                    input_tokens: 100,
                    output_tokens: 10,
                    cache_read_input_tokens: 0,
                    cache_creation_input_tokens: 0,
                    ..Default::default()
                },
                stop_reason: "end_turn".to_owned(),
                research: None,
            },
        ]));
        let config = QueryEngineConfig::new(
            session_id.clone(),
            "primary-model",
            Arc::clone(&backend) as Arc<dyn ConversationBackend>,
            Arc::new(MockToolRunner),
            rc_engine_events::EventStream::new(8),
        );
        let mut engine = QueryEngine::new(
            config,
            vec![claude_core::Message::from(ConversationEntry::system("sys"))],
        );
        let context =
            ProcessUserInputContext::new(session_id, PermissionMode::Default, "primary-model");

        let result = engine
            .submit_message(
                vec![claude_core::Message::from(ConversationEntry::user("hello"))],
                context,
            )
            .await
            .expect("max-token retry should succeed");

        assert_eq!(result.final_text.as_deref(), Some("full"));
        let contexts = backend.contexts();
        assert_eq!(contexts.len(), 2);
        assert_eq!(contexts[0].max_output_tokens, None);
        assert_eq!(contexts[1].max_output_tokens, Some(64_000));
    }

    #[tokio::test]
    async fn query_engine_emits_compaction_events_to_observer_and_stream() {
        let session_id = SessionId::new();
        let observer = Arc::new(RecordingObserver::default());
        let backend = Arc::new(MockBackend::new(vec![ProviderResponse {
            text: "done".to_owned(),
            history_text: None,
            thinking: None,
            content_blocks: Vec::new(),
            tool_calls: Vec::new(),
            request_id: None,
            usage: UsageSummary {
                input_tokens: 1,
                output_tokens: 1,
                cache_read_input_tokens: 0,
                cache_creation_input_tokens: 0,
                ..Default::default()
            },
            stop_reason: "end_turn".to_owned(),
            research: None,
        }]));
        let mut config = QueryEngineConfig::new(
            session_id.clone(),
            "mock-model",
            backend,
            Arc::new(MockToolRunner),
            rc_engine_events::EventStream::new(16),
        )
        .with_observer(observer.clone());
        config.context_manager = ContextWindowManager::new(100, 20);
        let mut engine_events = config.event_stream.subscribe();

        let mut existing_messages =
            vec![claude_core::Message::from(ConversationEntry::system("sys"))];
        for index in 0..5 {
            existing_messages.push(claude_core::Message::from(ConversationEntry::user(
                format!("user-{index}-{}", "a".repeat(200)),
            )));
            existing_messages.push(claude_core::Message::from(ConversationEntry::assistant(
                format!("assistant-{index}-{}", "b".repeat(200)),
            )));
        }

        let mut engine = QueryEngine::new(config, existing_messages);
        let context =
            ProcessUserInputContext::new(session_id, PermissionMode::Default, "mock-model");
        let result = engine
            .submit_message(
                vec![claude_core::Message::from(ConversationEntry::user(
                    format!("latest-{}", "c".repeat(200)),
                ))],
                context,
            )
            .await
            .expect("query engine should succeed after compaction");

        assert_eq!(result.final_text.as_deref(), Some("done"));

        let observer_events = observer.snapshot();
        assert!(observer_events.iter().any(|event| matches!(
            event,
            QueryObserverEvent::ContextBudgetEvaluated { context, .. } if context.needs_compaction
        )));
        assert!(observer_events.iter().any(|event| matches!(
            event,
            QueryObserverEvent::ContextCompactionApplied {
                before_messages,
                after_messages,
                ..
            } if before_messages > after_messages
        )));

        let engine_events = drain_engine_events(&mut engine_events);
        assert!(engine_events.iter().any(|event| matches!(
            event,
            EngineEvent::CompactStarted { strategy } if strategy == "standard"
        )));
        assert!(engine_events.iter().any(|event| matches!(
            event,
            EngineEvent::CompactCompleted { result } if result.before_messages > result.after_messages
        )));
    }

    #[tokio::test]
    async fn query_engine_applies_post_compact_transform() {
        let session_id = SessionId::new();
        let backend = Arc::new(MockBackend::new(vec![ProviderResponse {
            text: "done".to_owned(),
            history_text: None,
            thinking: None,
            content_blocks: Vec::new(),
            tool_calls: Vec::new(),
            request_id: None,
            usage: UsageSummary {
                input_tokens: 1,
                output_tokens: 1,
                cache_read_input_tokens: 0,
                cache_creation_input_tokens: 0,
                ..Default::default()
            },
            stop_reason: "end_turn".to_owned(),
            research: None,
        }]));
        let mut config = QueryEngineConfig::new(
            session_id.clone(),
            "mock-model",
            backend,
            Arc::new(MockToolRunner),
            rc_engine_events::EventStream::new(8),
        );
        config.context_manager = ContextWindowManager::new(100, 20);
        config = config.with_post_compact_transform(Arc::new(|mut conversation| {
            Box::pin(async move {
                conversation.push(ConversationEntry::user("post-compact marker"));
                conversation
            })
        }));

        let mut existing_messages =
            vec![claude_core::Message::from(ConversationEntry::system("sys"))];
        for index in 0..5 {
            existing_messages.push(claude_core::Message::from(ConversationEntry::user(
                format!("user-{index}-{}", "a".repeat(200)),
            )));
            existing_messages.push(claude_core::Message::from(ConversationEntry::assistant(
                format!("assistant-{index}-{}", "b".repeat(200)),
            )));
        }

        let mut engine = QueryEngine::new(config, existing_messages);
        let context =
            ProcessUserInputContext::new(session_id, PermissionMode::Default, "mock-model");
        let result = engine
            .submit_message(
                vec![claude_core::Message::from(ConversationEntry::user(
                    format!("latest-{}", "c".repeat(200)),
                ))],
                context,
            )
            .await
            .expect("query engine should succeed after transformed compaction");

        assert_eq!(result.final_text.as_deref(), Some("done"));
        assert!(
            result
                .state
                .legacy_conversation()
                .iter()
                .any(|entry| entry.role == claude_core::ConversationRole::User
                    && entry.text == "post-compact marker")
        );
    }

    #[tokio::test]
    async fn query_engine_uses_custom_compact_handler_when_provided() {
        let session_id = SessionId::new();
        let backend = Arc::new(MockBackend::new(vec![ProviderResponse {
            text: "done".to_owned(),
            history_text: None,
            thinking: None,
            content_blocks: Vec::new(),
            tool_calls: Vec::new(),
            request_id: None,
            usage: UsageSummary {
                input_tokens: 1,
                output_tokens: 1,
                cache_read_input_tokens: 0,
                cache_creation_input_tokens: 0,
                ..Default::default()
            },
            stop_reason: "end_turn".to_owned(),
            research: None,
        }]));
        let mut config = QueryEngineConfig::new(
            session_id.clone(),
            "mock-model",
            backend,
            Arc::new(MockToolRunner),
            rc_engine_events::EventStream::new(8),
        );
        config.context_manager = ContextWindowManager::new(100, 20);
        config = config.with_compact_conversation_handler(Arc::new(|conversation, _manager| {
            Box::pin(async move {
                let retained = conversation
                    .into_iter()
                    .take(2)
                    .chain(std::iter::once(ConversationEntry::user("handler-marker")))
                    .collect::<Vec<_>>();
                Some((retained, "session_memory".to_owned()))
            })
        }));
        let mut engine_events = config.event_stream.subscribe();

        let mut existing_messages =
            vec![claude_core::Message::from(ConversationEntry::system("sys"))];
        for index in 0..5 {
            existing_messages.push(claude_core::Message::from(ConversationEntry::user(
                format!("user-{index}-{}", "a".repeat(200)),
            )));
            existing_messages.push(claude_core::Message::from(ConversationEntry::assistant(
                format!("assistant-{index}-{}", "b".repeat(200)),
            )));
        }

        let mut engine = QueryEngine::new(config, existing_messages);
        let context =
            ProcessUserInputContext::new(session_id, PermissionMode::Default, "mock-model");
        let result = engine
            .submit_message(
                vec![claude_core::Message::from(ConversationEntry::user(
                    format!("latest-{}", "c".repeat(200)),
                ))],
                context,
            )
            .await
            .expect("query engine should succeed with custom compact handler");

        assert_eq!(result.final_text.as_deref(), Some("done"));
        assert!(
            result
                .state
                .legacy_conversation()
                .iter()
                .any(|entry| entry.text == "handler-marker")
        );

        let engine_events = drain_engine_events(&mut engine_events);
        assert!(engine_events.iter().any(|event| matches!(
            event,
            EngineEvent::CompactCompleted { result } if result.strategy == "session_memory"
        )));
    }

    #[tokio::test]
    async fn query_engine_runs_post_sampling_before_stop_hook_on_terminal_turn() {
        let session_id = SessionId::new();
        let backend = Arc::new(MockBackend::new(vec![ProviderResponse {
            text: "done".to_owned(),
            history_text: None,
            thinking: None,
            content_blocks: Vec::new(),
            tool_calls: Vec::new(),
            request_id: None,
            usage: UsageSummary {
                input_tokens: 1,
                output_tokens: 1,
                cache_read_input_tokens: 0,
                cache_creation_input_tokens: 0,
                ..Default::default()
            },
            stop_reason: "end_turn".to_owned(),
            research: None,
        }]));
        let sequence = Arc::new(Mutex::new(Vec::<String>::new()));
        let post_sampling_sequence = Arc::clone(&sequence);
        let stop_hook_sequence = Arc::clone(&sequence);

        let config = QueryEngineConfig::new(
            session_id.clone(),
            "mock-model",
            backend,
            Arc::new(MockToolRunner),
            rc_engine_events::EventStream::new(8),
        )
        .with_post_sampling_hook(Arc::new(move |context: ReplHookContext| {
            let post_sampling_sequence = Arc::clone(&post_sampling_sequence);
            Box::pin(async move {
                post_sampling_sequence
                    .lock()
                    .push(format!("post_sampling:{}", context.messages.len()));
                Ok(())
            })
        }))
        .with_stop_hook(Arc::new(
            move |context: ReplHookContext, request: StopHookRequest| {
                let stop_hook_sequence = Arc::clone(&stop_hook_sequence);
                Box::pin(async move {
                    stop_hook_sequence.lock().push(format!(
                        "stop_hook:{}:{}",
                        request.stop_reason,
                        context.messages.len()
                    ));
                    Ok(StopHookOutcome::Allow)
                })
            },
        ));
        let mut engine = QueryEngine::new(
            config,
            vec![claude_core::Message::from(ConversationEntry::system("sys"))],
        );
        let mut context =
            ProcessUserInputContext::new(session_id, PermissionMode::Default, "mock-model");
        context.system_prompt = Some("system".to_owned());
        context
            .user_context
            .insert("currentDate".to_owned(), "Today".to_owned());

        let result = engine
            .submit_message(
                vec![claude_core::Message::from(ConversationEntry::user("hello"))],
                context,
            )
            .await
            .expect("terminal query should succeed");

        assert_eq!(result.final_text.as_deref(), Some("done"));
        let sequence = sequence.lock().clone();
        assert_eq!(
            sequence,
            vec![
                "post_sampling:3".to_owned(),
                "stop_hook:end_turn:3".to_owned()
            ]
        );
    }

    #[tokio::test]
    async fn query_engine_stop_hook_retry_appends_messages_and_continues() {
        let session_id = SessionId::new();
        let backend = Arc::new(MockBackend::new(vec![
            ProviderResponse {
                text: "first".to_owned(),
                history_text: None,
                thinking: None,
                content_blocks: Vec::new(),
                tool_calls: Vec::new(),
                request_id: None,
                usage: UsageSummary {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_read_input_tokens: 0,
                    cache_creation_input_tokens: 0,
                    ..Default::default()
                },
                stop_reason: "end_turn".to_owned(),
                research: None,
            },
            ProviderResponse {
                text: "second".to_owned(),
                history_text: None,
                thinking: None,
                content_blocks: Vec::new(),
                tool_calls: Vec::new(),
                request_id: None,
                usage: UsageSummary {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_read_input_tokens: 0,
                    cache_creation_input_tokens: 0,
                    ..Default::default()
                },
                stop_reason: "end_turn".to_owned(),
                research: None,
            },
        ]));
        let stop_count = Arc::new(AtomicUsize::new(0));
        let stop_count_for_hook = Arc::clone(&stop_count);

        let config = QueryEngineConfig::new(
            session_id.clone(),
            "mock-model",
            backend,
            Arc::new(MockToolRunner),
            rc_engine_events::EventStream::new(8),
        )
        .with_stop_hook(Arc::new(
            move |_context: ReplHookContext, _request: StopHookRequest| {
                let stop_count_for_hook = Arc::clone(&stop_count_for_hook);
                Box::pin(async move {
                    let attempt = stop_count_for_hook.fetch_add(1, Ordering::SeqCst);
                    if attempt == 0 {
                        Ok(StopHookOutcome::Retry {
                            injected_messages: vec![claude_core::Message::from(
                                ConversationEntry::user("retry please"),
                            )],
                        })
                    } else {
                        Ok(StopHookOutcome::Allow)
                    }
                })
            },
        ));
        let mut engine = QueryEngine::new(
            config,
            vec![claude_core::Message::from(ConversationEntry::system("sys"))],
        );
        let context =
            ProcessUserInputContext::new(session_id, PermissionMode::Default, "mock-model");

        let result = engine
            .submit_message(
                vec![claude_core::Message::from(ConversationEntry::user("hello"))],
                context,
            )
            .await
            .expect("retrying stop-hook query should succeed");

        assert_eq!(result.final_text.as_deref(), Some("second"));
        assert_eq!(stop_count.load(Ordering::SeqCst), 2);
        assert!(
            result
                .state
                .messages
                .iter()
                .filter_map(claude_core::Message::as_conversation_entry)
                .any(|entry| entry.role == claude_core::ConversationRole::User
                    && entry.text == "retry please")
        );
    }
}
