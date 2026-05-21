use std::collections::BTreeMap;
use std::sync::Arc;

use claude_config::RuntimeConfig;
use claude_core::{ConversationEntry, ConversationRole, SystemMemorySavedMessage};
use claude_provider::{ConversationBackend, DiscoveredToolScope};
use claude_query_engine::{ProcessUserInputContext, QueryEngineConfig, QuerySource};
use claude_session::SessionStore;

use crate::conversation::PromptEventSink;
use crate::extract_memories::{AppendSystemMessageFn, spawn_extract_memories_after_turn};
use crate::query_engine_compat::ForkCacheSafeParams;
use crate::session_memory_runtime::maybe_spawn_session_memory_update;

#[derive(Clone)]
pub(crate) struct ReplHookRuntimeResources {
    pub(crate) config: RuntimeConfig,
    pub(crate) store: Arc<SessionStore>,
    pub(crate) backend: Arc<dyn ConversationBackend>,
    pub(crate) discovered_tool_scope: DiscoveredToolScope,
    pub(crate) event_sink: Option<PromptEventSink>,
}

pub(crate) fn apply_runtime_hook_context(
    process_context: &mut ProcessUserInputContext,
    config: &RuntimeConfig,
    provider_conversation: &[ConversationEntry],
    fork_snapshot: Option<&ForkCacheSafeParams>,
) {
    if let Some(snapshot) = fork_snapshot {
        process_context.system_prompt = snapshot.system_prompt.clone().or_else(|| {
            provider_conversation
                .iter()
                .find(|entry| entry.role == ConversationRole::System)
                .map(|entry| entry.text.clone())
                .filter(|text| !text.trim().is_empty())
        });
        process_context.user_context = snapshot.user_context.clone();
        process_context.system_context = snapshot.system_context.clone();
        return;
    }
    process_context.system_prompt = provider_conversation
        .iter()
        .find(|entry| entry.role == ConversationRole::System)
        .map(|entry| entry.text.clone())
        .filter(|text| !text.trim().is_empty());
    process_context.user_context = extract_runtime_user_context(provider_conversation);
    process_context.system_context = build_runtime_system_context(config);
}

pub(crate) fn register_repl_runtime_hooks(
    mut query_config: QueryEngineConfig,
    resources: ReplHookRuntimeResources,
) -> QueryEngineConfig {
    let post_sampling = Arc::new({
        let resources = resources.clone();
        move |hook_context: claude_query_engine::stop_hooks::ReplHookContext| {
            let resources = resources.clone();
            Box::pin(async move {
                if hook_context.query_source != QuerySource::ReplMainThread
                    || hook_context.agent_id.is_some()
                {
                    return Ok(());
                }
                let conversation = hook_context
                    .messages
                    .iter()
                    .filter_map(claude_core::Message::as_conversation_entry)
                    .collect::<Vec<_>>();
                maybe_spawn_session_memory_update(
                    &resources.config,
                    resources.store.as_ref(),
                    resources.backend.clone(),
                    resources.discovered_tool_scope.clone(),
                    &conversation,
                    Some(ForkCacheSafeParams::from_repl_hook_context(&hook_context)),
                )
                .await;
                Ok(())
            })
                as std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send>>
        }
    });
    query_config = query_config.with_post_sampling_hook(post_sampling);

    let stop_hook = Arc::new({
        let resources = resources.clone();
        move |hook_context: claude_query_engine::stop_hooks::ReplHookContext,
              _request: claude_query_engine::stop_hooks::StopHookRequest| {
            let resources = resources.clone();
            Box::pin(async move {
                if matches!(
                    hook_context.query_source,
                    QuerySource::ReplMainThread | QuerySource::Sdk
                ) && hook_context.agent_id.is_none()
                {
                    let append_system_message: AppendSystemMessageFn = {
                        let store = resources.store.clone();
                        let event_sink = resources.event_sink.clone();
                        let session_id = resources.config.session_id;
                        Arc::new(move |entry: ConversationEntry| {
                            let memory_saved = entry
                                .name
                                .as_deref()
                                .is_some_and(|name| name == "memory_saved")
                                .then(|| {
                                    serde_json::from_str::<SystemMemorySavedMessage>(&entry.text)
                                        .ok()
                                })
                                .flatten();
                            let _ = store.append_conversation_entry(session_id, &entry);
                            if let (Some(event_sink), Some(payload)) =
                                (event_sink.as_ref(), memory_saved)
                            {
                                event_sink(crate::conversation::PromptStreamEvent::MemorySaved {
                                    written_paths: payload.written_paths,
                                    team_count: payload.team_count,
                                });
                            }
                        })
                    };
                    let conversation = hook_context
                        .messages
                        .iter()
                        .filter_map(claude_core::Message::as_conversation_entry)
                        .collect::<Vec<_>>();
                    spawn_extract_memories_after_turn(
                        &resources.config,
                        resources.store.as_ref(),
                        resources.backend.clone(),
                        resources.discovered_tool_scope.clone(),
                        &conversation,
                        Some(append_system_message),
                        Some(ForkCacheSafeParams::from_repl_hook_context(&hook_context)),
                    );
                }
                Ok(claude_query_engine::stop_hooks::StopHookOutcome::Allow)
            })
                as std::pin::Pin<
                    Box<
                        dyn std::future::Future<
                                Output = anyhow::Result<
                                    claude_query_engine::stop_hooks::StopHookOutcome,
                                >,
                            > + Send,
                    >,
                >
        }
    });
    query_config.with_stop_hook(stop_hook)
}

fn extract_runtime_user_context(
    provider_conversation: &[ConversationEntry],
) -> BTreeMap<String, String> {
    let Some(reminder) = provider_conversation.iter().find(|entry| {
        entry.role == ConversationRole::User
            && entry
                .text
                .contains("As you answer the user's questions, you can use the following context:")
    }) else {
        return BTreeMap::new();
    };

    parse_context_sections(&reminder.text)
}

fn parse_context_sections(text: &str) -> BTreeMap<String, String> {
    let mut sections = BTreeMap::new();
    let mut current_key: Option<String> = None;
    let mut current_value = Vec::new();

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("# ") {
            if let Some(key) = current_key.take() {
                let value = current_value.join("\n").trim().to_owned();
                if !value.is_empty() {
                    sections.insert(key, value);
                }
                current_value.clear();
            }
            current_key = Some(rest.trim().to_owned());
            continue;
        }

        if line.contains("IMPORTANT: this context may or may not be relevant") {
            break;
        }

        if current_key.is_some() {
            current_value.push(line.to_owned());
        }
    }

    if let Some(key) = current_key {
        let value = current_value.join("\n").trim().to_owned();
        if !value.is_empty() {
            sections.insert(key, value);
        }
    }

    sections
}

fn build_runtime_system_context(config: &RuntimeConfig) -> BTreeMap<String, String> {
    let mut context = BTreeMap::new();
    context.insert("cwd".to_owned(), config.cwd.display().to_string());
    context.insert(
        "provider".to_owned(),
        format!(
            "{}:{}",
            config.provider.name,
            config.provider.model.as_deref().unwrap_or("unknown")
        ),
    );
    if let Some(language) = config.language.as_deref() {
        context.insert("language".to_owned(), language.to_owned());
    }
    if let Some(output_style) = config.output_style.as_deref() {
        context.insert("outputStyle".to_owned(), output_style.to_owned());
    }
    context.insert("printMode".to_owned(), config.print_mode.to_string());
    context
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{
        ReplHookRuntimeResources, build_runtime_system_context, parse_context_sections,
        register_repl_runtime_hooks,
    };
    use claude_core::{AgentId, Message};
    use claude_provider::ConversationBackend;
    use claude_query_engine::QueryEngineConfig;

    #[test]
    fn parse_context_sections_extracts_named_blocks() {
        let parsed = parse_context_sections(
            "<system-reminder>\nAs you answer the user's questions, you can use the following context:\n# currentDate\nToday\n# workerToolsContext\nWorkers available\n\n      IMPORTANT: this context may or may not be relevant to your tasks.\n</system-reminder>\n",
        );
        assert_eq!(parsed.get("currentDate").map(String::as_str), Some("Today"));
        assert_eq!(
            parsed.get("workerToolsContext").map(String::as_str),
            Some("Workers available")
        );
    }

    #[test]
    fn build_runtime_system_context_includes_basic_runtime_keys() {
        let mut config = claude_config::load_runtime_config(
            None,
            None,
            None,
            claude_core::PermissionMode::Default,
            claude_core::InputFormat::Text,
            claude_core::OutputFormat::Text,
            false,
            false,
            false,
            false,
            8,
            claude_config::ProviderOverrides::default(),
            claude_config::settings_layers::RuntimeOverrides::default(),
        )
        .expect("config");
        config.language = Some("Chinese".to_owned());
        config.output_style = Some("Explanatory".to_owned());

        let context = build_runtime_system_context(&config);
        assert!(context.contains_key("cwd"));
        assert!(context.contains_key("provider"));
        assert_eq!(context.get("language").map(String::as_str), Some("Chinese"));
        assert_eq!(
            context.get("outputStyle").map(String::as_str),
            Some("Explanatory")
        );
    }

    #[tokio::test]
    async fn runtime_hooks_skip_agent_scoped_post_sampling_and_stop_hooks() {
        struct NoopBackend;

        #[async_trait::async_trait]
        impl ConversationBackend for NoopBackend {
            async fn complete(
                &self,
                _conversation: &[claude_core::ConversationEntry],
            ) -> anyhow::Result<claude_core::ProviderResponse> {
                Ok(claude_core::ProviderResponse::default())
            }

            async fn complete_streaming(
                &self,
                _conversation: &[claude_core::ConversationEntry],
                _callbacks: Option<claude_provider::StreamingCallbacks>,
            ) -> anyhow::Result<claude_core::ProviderResponse> {
                Ok(claude_core::ProviderResponse::default())
            }

            fn sub_agent_completion(&self) -> Arc<dyn claude_core::SubAgentCompletion> {
                struct DummyCompletion;

                #[async_trait::async_trait]
                impl claude_core::SubAgentCompletion for DummyCompletion {
                    async fn complete(
                        &self,
                        _conversation: &[claude_core::ConversationEntry],
                    ) -> anyhow::Result<claude_core::ProviderResponse> {
                        Ok(claude_core::ProviderResponse::default())
                    }
                }

                Arc::new(DummyCompletion)
            }
        }

        struct NoopToolRunner;

        #[async_trait::async_trait]
        impl claude_query_engine::ToolRunner for NoopToolRunner {
            async fn run_tool(
                &self,
                _tool_call: &claude_core::ToolCall,
                _context: &claude_query_engine::ProcessUserInputContext,
            ) -> anyhow::Result<claude_query_engine::ToolRunResult> {
                Ok(claude_query_engine::ToolRunResult::from(
                    claude_core::ToolResult::default(),
                ))
            }
        }

        let config = claude_config::load_runtime_config(
            None,
            None,
            None,
            claude_core::PermissionMode::Default,
            claude_core::InputFormat::Text,
            claude_core::OutputFormat::Text,
            false,
            false,
            false,
            false,
            8,
            claude_config::ProviderOverrides::default(),
            claude_config::settings_layers::RuntimeOverrides::default(),
        )
        .expect("config");
        let store =
            Arc::new(claude_session::SessionStore::open(config.paths.clone()).expect("store"));
        let query_config = QueryEngineConfig::new(
            config.session_id.into(),
            "mock",
            Arc::new(NoopBackend),
            Arc::new(NoopToolRunner),
            rc_engine_events::EventStream::new(8),
        );
        let query_config = register_repl_runtime_hooks(
            query_config,
            ReplHookRuntimeResources {
                config: config.clone(),
                store,
                backend: Arc::new(NoopBackend),
                discovered_tool_scope: claude_provider::DiscoveredToolScope::default(),
                event_sink: None,
            },
        );

        let hook_context = claude_query_engine::stop_hooks::ReplHookContext {
            session_id: config.session_id.into(),
            turn: 1,
            messages: vec![Message::from(claude_core::ConversationEntry::user("hello"))],
            query_source: claude_query_engine::QuerySource::ReplMainThread,
            agent_id: Some(AgentId::from("agent-test")),
            system_prompt: None,
            user_context: Default::default(),
            system_context: Default::default(),
        };

        for hook in &query_config.post_sampling_hooks {
            hook(hook_context.clone()).await.expect("hook");
        }
        let stop_hook = query_config.stop_hook.expect("stop hook");
        let outcome = stop_hook(
            hook_context,
            claude_query_engine::stop_hooks::StopHookRequest {
                stop_reason: "end_turn".to_owned(),
                final_text: Some("done".to_owned()),
            },
        )
        .await
        .expect("stop hook");

        assert!(matches!(
            outcome,
            claude_query_engine::stop_hooks::StopHookOutcome::Allow
        ));
    }
}
