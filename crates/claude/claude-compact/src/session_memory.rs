//! Session-memory compaction backed by the persisted `summary.md` file.
//!
//! This follows the research implementation more closely than the previous
//! placeholder path: compaction waits for session-memory extraction to settle,
//! loads the persisted summary, calculates the preserved tail window from the
//! last summarized message boundary, and directly emits a compact summary from
//! `summary.md` instead of asking the model to summarize again.

use parking_lot::Mutex;
use std::sync::Arc;

use anyhow::Result;
use claude_config::RuntimeConfig;
use claude_core::{
    Message, MessageBase, MessageOrigin, SystemMessage, SystemMessageSubtype, UserMessage,
};
use claude_session::session_memory::{
    SessionMemoryState, ensure_session_memory_file, is_session_memory_empty,
    load_session_memory_content, session_memory_path, truncate_session_memory_for_compact,
    wait_for_session_memory_extraction,
};

use crate::engine::create_compact_boundary_message;
use crate::estimate_message_tokens;
use crate::estimate_single_message_tokens;
use crate::prompt::build_compact_user_summary_message;
use crate::strategy::{
    CompactOptions, CompactProgressEvent, CompactStrategy, CompactStrategyType, CompactionResult,
    PreservedSegment, ProgressCallback, SummaryProvider,
};

pub const DEFAULT_SM_COMPACT_MIN_TOKENS: u64 = 10_000;
pub const DEFAULT_SM_COMPACT_MIN_TEXT_BLOCK_MESSAGES: usize = 5;
pub const DEFAULT_SM_COMPACT_MAX_TOKENS: u64 = 40_000;

#[derive(Debug, Clone)]
pub struct SessionMemoryCompactConfig {
    pub min_tokens: u64,
    pub min_text_block_messages: usize,
    pub max_tokens: u64,
}

impl Default for SessionMemoryCompactConfig {
    fn default() -> Self {
        Self {
            min_tokens: DEFAULT_SM_COMPACT_MIN_TOKENS,
            min_text_block_messages: DEFAULT_SM_COMPACT_MIN_TEXT_BLOCK_MESSAGES,
            max_tokens: DEFAULT_SM_COMPACT_MAX_TOKENS,
        }
    }
}

#[derive(Clone)]
pub struct SessionMemoryCompactFileContext {
    pub runtime_config: RuntimeConfig,
    pub state: Arc<Mutex<SessionMemoryState>>,
}

#[derive(Default)]
pub struct SessionMemoryCompactStrategy {
    pub config: SessionMemoryCompactConfig,
    pub file_context: Option<SessionMemoryCompactFileContext>,
}

impl SessionMemoryCompactStrategy {
    pub fn new(config: SessionMemoryCompactConfig) -> Self {
        Self {
            config,
            file_context: None,
        }
    }

    pub fn with_file_context(
        mut self,
        runtime_config: RuntimeConfig,
        state: Arc<Mutex<SessionMemoryState>>,
    ) -> Self {
        self.file_context = Some(SessionMemoryCompactFileContext {
            runtime_config,
            state,
        });
        self
    }
}

#[async_trait::async_trait]
impl CompactStrategy for SessionMemoryCompactStrategy {
    fn strategy_type(&self) -> CompactStrategyType {
        CompactStrategyType::SessionMemory
    }

    async fn compact(
        &self,
        messages: &[Message],
        options: &CompactOptions,
        _provider: &dyn SummaryProvider,
        progress: Option<&ProgressCallback>,
    ) -> Result<CompactionResult, anyhow::Error> {
        // Merge caller-provided options into the strategy config where applicable.
        // Session-memory compaction does not use a SummaryProvider — it relies on
        // the persisted session-memory file instead of LLM summarisation.
        let effective_config = SessionMemoryCompactConfig {
            max_tokens: if options.max_tokens > 0 {
                options.max_tokens
            } else {
                self.config.max_tokens
            },
            min_tokens: self.config.min_tokens,
            min_text_block_messages: if options.preserve_recent_messages > 0 {
                options.preserve_recent_messages
            } else {
                self.config.min_text_block_messages
            },
        };
        session_memory_compact(
            messages,
            &effective_config,
            self.file_context.as_ref(),
            progress,
        )
        .await
    }
}

pub async fn session_memory_compact(
    messages: &[Message],
    config: &SessionMemoryCompactConfig,
    file_context: Option<&SessionMemoryCompactFileContext>,
    progress: Option<&ProgressCallback>,
) -> Result<CompactionResult, anyhow::Error> {
    if messages.is_empty() {
        return Err(anyhow::anyhow!("Not enough messages to compact."));
    }

    let Some(file_context) = file_context else {
        return Err(anyhow::anyhow!(
            "Session-memory compact requires a runtime file context."
        ));
    };

    if let Some(sink) = progress {
        sink(CompactProgressEvent::Started {
            strategy: CompactStrategyType::SessionMemory,
        });
    }

    let Some(session_memory) = load_session_memory_for_compact(file_context) else {
        return Err(anyhow::anyhow!(
            "Session-memory compact unavailable because the session summary is missing or still empty."
        ));
    };

    let last_summarized_uuid = file_context
        .state
        .lock()
        .last_summarized_message_id
        .as_deref()
        .and_then(|value| uuid::Uuid::parse_str(value).ok());

    let last_summarized_index = match last_summarized_uuid {
        Some(target) => messages
            .iter()
            .position(|message| message.uuid() == target)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Session-memory compact could not locate the last summarized message boundary."
                )
            })?,
        None => messages.len().saturating_sub(1),
    };

    let start_index = calculate_messages_to_keep_index(messages, last_summarized_index, config);
    let messages_to_keep = messages
        .iter()
        .skip(start_index)
        .filter(|message| !is_compact_boundary_message(message))
        .cloned()
        .collect::<Vec<_>>();

    let result =
        create_compaction_result_from_session_memory(messages, &session_memory, messages_to_keep);

    if let Some(sink) = progress {
        sink(CompactProgressEvent::Completed(result.clone()));
    }

    Ok(result)
}

fn load_session_memory_for_compact(
    file_context: &SessionMemoryCompactFileContext,
) -> Option<String> {
    wait_for_session_memory_extraction(&file_context.state);

    let runtime_config = &file_context.runtime_config;
    let _ = ensure_session_memory_file(runtime_config);
    let content = load_session_memory_content(runtime_config).ok().flatten()?;
    if is_session_memory_empty(runtime_config, &content) {
        return None;
    }

    let (truncated, was_truncated) = truncate_session_memory_for_compact(&content);
    if was_truncated {
        Some(format!(
            "{truncated}\n\nSome session memory sections were truncated for length. The full session memory can be viewed at: {}",
            session_memory_path(runtime_config).display()
        ))
    } else {
        Some(truncated)
    }
}

fn create_compaction_result_from_session_memory(
    messages: &[Message],
    session_memory: &str,
    messages_to_keep: Vec<Message>,
) -> CompactionResult {
    let pre_compact_token_count = estimate_message_tokens(messages);
    let boundary_marker = create_compact_boundary_message(
        "auto",
        pre_compact_token_count,
        messages.last().map(Message::uuid),
        None,
    );
    let summary_text = build_compact_user_summary_message(session_memory, true, None, true);
    let summary_message = Message::User(UserMessage {
        base: {
            let mut base = MessageBase::with_origin(MessageOrigin::Compact);
            base.is_compact_summary = true;
            base.is_visible_in_transcript_only = true;
            base
        },
        text: summary_text.clone(),
        attachments: Vec::new(),
        provider_content_blocks: Vec::new(),
        summarize_metadata: None,
    });

    let post_compact_token_count = estimate_single_message_tokens(&boundary_marker)
        + estimate_single_message_tokens(&summary_message)
        + estimate_message_tokens(&messages_to_keep);
    let tokens_saved = pre_compact_token_count.saturating_sub(post_compact_token_count);
    let messages_removed = messages.len().saturating_sub(messages_to_keep.len());

    let preserved_segments = if messages_to_keep.is_empty() {
        Vec::new()
    } else {
        vec![PreservedSegment {
            head_uuid: messages_to_keep
                .first()
                .map(Message::uuid)
                .unwrap_or_default(),
            anchor_uuid: summary_message.uuid(),
            tail_uuid: messages_to_keep
                .last()
                .map(Message::uuid)
                .unwrap_or_default(),
        }]
    };

    CompactionResult {
        summary: session_memory.to_owned(),
        messages_removed,
        tokens_saved,
        strategy_used: CompactStrategyType::SessionMemory,
        preserved_segments,
        pre_compact_token_count: Some(pre_compact_token_count),
        post_compact_token_count: Some(post_compact_token_count),
        messages_to_keep,
        attachments: vec![boundary_marker, summary_message],
        hook_results: Vec::new(),
        user_display_message: None,
    }
}

fn calculate_messages_to_keep_index(
    messages: &[Message],
    last_summarized_index: usize,
    config: &SessionMemoryCompactConfig,
) -> usize {
    if messages.is_empty() {
        return 0;
    }

    let mut start_index = last_summarized_index.saturating_add(1).min(messages.len());
    let mut total_tokens = estimate_message_tokens(&messages[start_index..]);
    let mut text_block_message_count = messages[start_index..]
        .iter()
        .filter(|message| has_text_blocks(message))
        .count();

    if total_tokens >= config.max_tokens
        || (total_tokens >= config.min_tokens
            && text_block_message_count >= config.min_text_block_messages)
    {
        return adjust_index_to_preserve_api_invariants(messages, start_index);
    }

    let floor = messages
        .iter()
        .enumerate()
        .rev()
        .find(|(_, message)| is_compact_boundary_message(message))
        .map(|(index, _)| index + 1)
        .unwrap_or(0);

    while start_index > floor {
        let previous_index = start_index - 1;
        let message = &messages[previous_index];
        total_tokens += estimate_single_message_tokens(message);
        if has_text_blocks(message) {
            text_block_message_count += 1;
        }
        start_index = previous_index;

        if total_tokens >= config.max_tokens
            || (total_tokens >= config.min_tokens
                && text_block_message_count >= config.min_text_block_messages)
        {
            break;
        }
    }

    adjust_index_to_preserve_api_invariants(messages, start_index)
}

fn adjust_index_to_preserve_api_invariants(messages: &[Message], start_index: usize) -> usize {
    if start_index == 0 || start_index >= messages.len() {
        return start_index;
    }

    let mut adjusted_index = start_index;

    // Step 1: Ensure tool-use/tool-result pairs are complete.
    // Collect all tool_result IDs in the kept range and pull in any
    // missing tool_use messages from the summarized range.
    let all_tool_result_ids = messages[start_index..]
        .iter()
        .flat_map(tool_result_ids)
        .collect::<Vec<_>>();

    if !all_tool_result_ids.is_empty() {
        let tool_use_ids_in_kept_range = messages[adjusted_index..]
            .iter()
            .flat_map(tool_use_ids)
            .collect::<std::collections::BTreeSet<_>>();
        let mut missing_tool_use_ids = all_tool_result_ids
            .into_iter()
            .filter(|id| !tool_use_ids_in_kept_range.contains(id))
            .collect::<std::collections::BTreeSet<_>>();

        for index in (0..adjusted_index).rev() {
            if missing_tool_use_ids.is_empty() {
                break;
            }
            let present_ids = tool_use_ids(&messages[index])
                .into_iter()
                .filter(|id| missing_tool_use_ids.contains(id))
                .collect::<Vec<_>>();
            if !present_ids.is_empty() {
                adjusted_index = index;
                for id in present_ids {
                    missing_tool_use_ids.remove(&id);
                }
            }
        }
    }

    // Step 2: Thinking-block coalescing.
    // Streaming yields separate messages per content block (thinking,
    // tool_use, etc.) that share the same message UUID but have different
    // Rust UUIDs. normalizeMessagesForAPI merges them by message UUID.
    // We must include all fragments sharing a UUID present in the kept range.
    let kept_assistant_uuids: std::collections::BTreeSet<uuid::Uuid> = messages[adjusted_index..]
        .iter()
        .filter_map(|m| match m {
            Message::Assistant(a) => Some(a.base.uuid),
            _ => None,
        })
        .collect();

    if !kept_assistant_uuids.is_empty() {
        for index in (0..adjusted_index).rev() {
            if let Message::Assistant(a) = &messages[index] {
                // This is a streaming fragment with a matching message UUID
                // that needs to be coalesced with kept-range messages.
                // We use the provider_content_blocks to check for thinking blocks.
                if kept_assistant_uuids.contains(&a.base.uuid) {
                    adjusted_index = index;
                }
            }
        }
    }

    adjusted_index
}

fn tool_result_ids(message: &Message) -> Vec<String> {
    match message {
        Message::User(message) => message
            .provider_content_blocks
            .iter()
            .filter_map(|block| {
                if block.get("type").and_then(serde_json::Value::as_str) == Some("tool_result") {
                    block
                        .get("tool_use_id")
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned)
                } else {
                    None
                }
            })
            .collect(),
        Message::ToolUseSummary(message) => {
            if message.content_blocks.is_empty() {
                Vec::new()
            } else {
                vec![message.tool_call_id.clone()]
            }
        }
        _ => Vec::new(),
    }
}

fn tool_use_ids(message: &Message) -> Vec<String> {
    match message {
        Message::Assistant(message) => message
            .provider_content_blocks()
            .into_iter()
            .filter_map(|block| {
                if block.get("type").and_then(serde_json::Value::as_str) == Some("tool_use") {
                    block
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned)
                } else {
                    None
                }
            })
            .collect(),
        _ => Vec::new(),
    }
}

pub fn has_text_blocks(message: &Message) -> bool {
    match message {
        Message::User(message) => {
            !message.text.is_empty()
                || message
                    .provider_content_blocks
                    .iter()
                    .any(|block| block.get("type").and_then(|v| v.as_str()) == Some("text"))
        }
        Message::Assistant(message) => {
            !message.text.is_empty()
                || message.blocks.iter().any(|block| {
                    matches!(block, claude_core::AssistantContentBlock::Text { text } if !text.is_empty())
                })
        }
        _ => false,
    }
}

fn is_compact_boundary_message(message: &Message) -> bool {
    matches!(
        message,
        Message::System(SystemMessage {
            subtype: SystemMessageSubtype::CompactBoundary,
            ..
        })
    )
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::{Arc, OnceLock};

    use claude_config::settings_layers::RuntimeOverrides;
    use claude_config::{ProviderOverrides, load_runtime_config};
    use claude_core::{
        AssistantContentBlock, InputFormat, Message, MessageBase, OutputFormat, PermissionMode,
        UserMessage,
    };
    use tempfile::{TempDir, tempdir};

    use super::{
        DEFAULT_SM_COMPACT_MAX_TOKENS, DEFAULT_SM_COMPACT_MIN_TEXT_BLOCK_MESSAGES,
        DEFAULT_SM_COMPACT_MIN_TOKENS, SessionMemoryCompactConfig, SessionMemoryCompactStrategy,
        adjust_index_to_preserve_api_invariants, calculate_messages_to_keep_index, has_text_blocks,
        load_session_memory_for_compact,
    };
    use claude_session::session_memory::{
        DEFAULT_SESSION_MEMORY_TEMPLATE, SessionMemoryState, ensure_session_memory_file,
        project_dir, session_memory_path,
    };

    struct TestRuntime {
        _env_guard: parking_lot::MutexGuard<'static, ()>,
        _tempdir: TempDir,
        config: claude_config::RuntimeConfig,
        cleanup_project_dir: PathBuf,
        previous_claude_config_dir: Option<OsString>,
    }

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn test_runtime() -> TestRuntime {
        let env_guard = env_lock().lock();
        let tempdir = tempdir().expect("tempdir");
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        std::fs::create_dir_all(&cwd).expect("cwd");
        std::fs::create_dir_all(&profile).expect("profile");
        let previous_claude_config_dir = std::env::var_os("CLAUDE_CONFIG_DIR");
        // These tests exercise claude-session helpers that intentionally read
        // Claude's global config directory. Keep that global lookup scoped to
        // the temp profile so parallel workspace tests cannot touch user state.
        unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", &profile) };

        let config = load_runtime_config(
            Some(cwd),
            Some(profile),
            None,
            PermissionMode::BypassPermissions,
            InputFormat::Text,
            OutputFormat::Text,
            false,
            false,
            false,
            false,
            8,
            ProviderOverrides::default(),
            RuntimeOverrides::default(),
        )
        .expect("runtime config");
        let cleanup_project_dir = project_dir(&config.cwd);

        TestRuntime {
            _env_guard: env_guard,
            _tempdir: tempdir,
            config,
            cleanup_project_dir,
            previous_claude_config_dir,
        }
    }

    impl Drop for TestRuntime {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.cleanup_project_dir);
            match &self.previous_claude_config_dir {
                Some(value) => unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", value) },
                None => unsafe { std::env::remove_var("CLAUDE_CONFIG_DIR") },
            }
        }
    }

    #[test]
    fn session_memory_config_default() {
        let config = SessionMemoryCompactConfig::default();
        assert_eq!(config.min_tokens, DEFAULT_SM_COMPACT_MIN_TOKENS);
        assert_eq!(
            config.min_text_block_messages,
            DEFAULT_SM_COMPACT_MIN_TEXT_BLOCK_MESSAGES
        );
        assert_eq!(config.max_tokens, DEFAULT_SM_COMPACT_MAX_TOKENS);
    }

    #[test]
    fn has_text_blocks_user() {
        let message = Message::User(UserMessage {
            base: MessageBase::default(),
            text: "hello".into(),
            attachments: Vec::new(),
            provider_content_blocks: Vec::new(),
            summarize_metadata: None,
        });
        assert!(has_text_blocks(&message));
    }

    #[test]
    fn has_text_blocks_assistant_text_block() {
        let message = Message::Assistant(claude_core::AssistantMessage {
            base: MessageBase::default(),
            text: String::new(),
            blocks: vec![AssistantContentBlock::Text {
                text: "hi".to_owned(),
            }],
            tool_calls: Vec::new(),
            provider_content_blocks: Vec::new(),
        });
        assert!(has_text_blocks(&message));
    }

    #[test]
    fn strategy_defaults_to_no_file_context() {
        let strategy = SessionMemoryCompactStrategy::default();
        assert!(strategy.file_context.is_none());
    }

    #[test]
    fn load_session_memory_for_compact_returns_none_for_template_only() {
        let runtime = test_runtime();
        let memory_path = ensure_session_memory_file(&runtime.config).expect("memory path");
        std::fs::write(&memory_path, DEFAULT_SESSION_MEMORY_TEMPLATE).expect("template write");
        let context = super::SessionMemoryCompactFileContext {
            runtime_config: runtime.config.clone(),
            state: Arc::new(Mutex::new(SessionMemoryState::default())),
        };
        let result = load_session_memory_for_compact(&context);
        assert!(result.is_none());
    }

    #[test]
    fn load_session_memory_for_compact_appends_path_when_truncated() {
        let runtime = test_runtime();
        let memory_path = ensure_session_memory_file(&runtime.config).expect("memory path");
        std::fs::write(
            &memory_path,
            format!("# Current State\n{}\n", "a".repeat(12_000)),
        )
        .expect("memory write");
        let context = super::SessionMemoryCompactFileContext {
            runtime_config: runtime.config.clone(),
            state: Arc::new(Mutex::new(SessionMemoryState::default())),
        };
        let result = load_session_memory_for_compact(&context).expect("session memory");
        assert!(result.contains("Some session memory sections were truncated"));
        assert!(result.contains(&session_memory_path(&runtime.config).display().to_string()));
    }

    #[test]
    fn calculate_messages_to_keep_expands_to_meet_minimum_context_thresholds() {
        let messages = vec![
            Message::User(UserMessage {
                base: MessageBase::default(),
                text: "a".repeat(100),
                attachments: Vec::new(),
                provider_content_blocks: Vec::new(),
                summarize_metadata: None,
            }),
            Message::User(UserMessage {
                base: MessageBase::default(),
                text: "b".repeat(100),
                attachments: Vec::new(),
                provider_content_blocks: Vec::new(),
                summarize_metadata: None,
            }),
        ];
        let index =
            calculate_messages_to_keep_index(&messages, 0, &SessionMemoryCompactConfig::default());
        assert_eq!(index, 0);
    }

    #[test]
    fn preserve_api_invariants_pulls_in_missing_tool_use_messages() {
        let assistant = Message::Assistant(claude_core::AssistantMessage {
            base: MessageBase::default(),
            text: String::new(),
            blocks: Vec::new(),
            tool_calls: Vec::new(),
            provider_content_blocks: vec![serde_json::json!({
                "type": "tool_use",
                "id": "tool-1",
                "name": "Read"
            })],
        });
        let user = Message::User(UserMessage {
            base: MessageBase::default(),
            text: String::new(),
            attachments: Vec::new(),
            provider_content_blocks: vec![serde_json::json!({
                "type": "tool_result",
                "tool_use_id": "tool-1",
                "content": "ok"
            })],
            summarize_metadata: None,
        });
        let messages = vec![assistant, user];
        assert_eq!(adjust_index_to_preserve_api_invariants(&messages, 1), 0);
    }
}
