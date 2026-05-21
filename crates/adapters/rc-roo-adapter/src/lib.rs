//! # rc-roo-adapter — In-process Roo Code Agent Adapter (100% Native)
//!
//! Implements the [`AgentAdapter`] trait for the Roo Code agent using
//! Roo's native [`AgentLoop`] for full feature coverage including:
//! MCP tools, context compression, budget management, subtasks,
//! custom modes, code indexing, and more.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────┐
//! │  GUI (lib.rs)       │
//! │  send_prompt()      │
//! └────────┬────────────┘
//!          │ AgentAdapter::send_message()
//! ┌────────▼────────────┐
//! │  RooInProcessAdapter│
//! │  ┌───────────────┐  │
//! │  │ dyn Provider  │  │
//! │  │  (Anthropic/  │  │
//! │  │   OpenAI/...) │  │
//! │  └───────┬───────┘  │
//! │  ┌───────▼────────┐  │
//! │  │   AgentLoop    │  │
//! │  │  (native Roo   │  │
//! │  │   loop with    │  │
//! │  │   MCP, condense│  │
//! │  │   budget, etc) │  │
//! │  └───────────────┘  │
//! └──────────┼───────────┘
//!            │ mpsc::Receiver<UnifiedAgentEvent>
//! ┌──────────▼───────────┐
//! │  GUI event loop      │
//! └──────────────────────┘
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, info, warn};

use rc_agent_protocol::adapter::AgentAdapter;
use rc_agent_protocol::events::UnifiedAgentEvent;
use rc_agent_protocol::permission::PermissionDecision;
use rc_agent_protocol::types::{AgentConfig, AgentInfo, AgentStatus, AgentType};

use roo_auto_approval::types::AutoApprovalState;
use roo_context_tracking::{FileContextTracker, InMemoryMetadataStore};
use roo_editor::diff_view::DiffViewProvider;
use roo_ignore::RooIgnoreController;
use roo_prompt::build_system_prompt;
use roo_protect::RooProtectedController;
use roo_provider::handler::Provider;
use roo_task::TaskEvent as RooTaskEvent;
use roo_task::agent_loop::{AgentLoop, AgentLoopConfig, MistakeLimitAction};
use roo_task::engine::TaskEngine;
use roo_task::message_builder::MessageBuilder;
use roo_task::tool_dispatcher::ToolDispatcher;
use roo_task::types::TaskConfig;
use roo_terminal::TerminalRegistry;
use roo_types::api::ApiMessage;
use roo_types::mcp::McpServerConnection;

type BoolDecisionHandle = Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<bool>>>>;
type StringDecisionHandle = Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<String>>>>;
type MistakeLimitDecisionHandle =
    Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<MistakeLimitAction>>>>;

/// Type aliases for handles passed from the worker thread back to the adapter.
type BoolDecisionSender = Arc<std::sync::Mutex<Option<BoolDecisionHandle>>>;
type StringDecisionSender = Arc<std::sync::Mutex<Option<StringDecisionHandle>>>;
type MistakeLimitDecisionSender = Arc<std::sync::Mutex<Option<MistakeLimitDecisionHandle>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RooPermissionKind {
    ToolApproval,
    ApiRetry,
    AutoApprovalLimit,
    MistakeLimit,
    Followup,
    Completion,
}

impl RooPermissionKind {
    fn from_request(request_id: &str, request_kind: Option<&str>) -> Option<Self> {
        if request_id.starts_with("api_retry:") {
            return Some(Self::ApiRetry);
        }
        if request_id.starts_with("auto_approval_limit:") {
            return Some(Self::AutoApprovalLimit);
        }
        if request_id.starts_with("mistake_limit:") {
            return Some(Self::MistakeLimit);
        }

        let normalized = request_kind
            .map(|kind| kind.trim().to_ascii_lowercase())
            .unwrap_or_default();
        match normalized.as_str() {
            "tool" | "tool_approval" => Some(Self::ToolApproval),
            "api_req_failed" | "api_request_failed" | "api_retry" => Some(Self::ApiRetry),
            "auto_approval_max_req_reached" | "auto_approval_limit" => {
                Some(Self::AutoApprovalLimit)
            }
            "mistake_limit_reached" | "mistake_limit" => Some(Self::MistakeLimit),
            "followup" | "ask_followup_question" => Some(Self::Followup),
            "completion_result" | "attempt_completion" => Some(Self::Completion),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Provider builder — mirrors roo-cli's build_handler()
// ---------------------------------------------------------------------------

fn strip_trailing_path_segments<'a>(raw: &'a str, suffix: &[&str]) -> Option<&'a str> {
    let trimmed = raw.trim().trim_end_matches('/');
    let mut end = trimmed.len();
    for segment in suffix.iter().rev() {
        let candidate = &trimmed[..end];
        let slash = candidate.rfind('/')?;
        if !candidate[slash + 1..].eq_ignore_ascii_case(segment) {
            return None;
        }
        end = slash;
    }
    Some(&trimmed[..end])
}

fn normalize_anthropic_base_url(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    strip_trailing_path_segments(trimmed, &["v1", "messages"])
        .or_else(|| strip_trailing_path_segments(trimmed, &["messages"]))
        .unwrap_or(trimmed)
        .to_string()
}

fn normalize_openai_base_url(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    strip_trailing_path_segments(trimmed, &["chat", "completions"])
        .unwrap_or(trimmed)
        .to_string()
}

fn anthropic_base_url(base_url: Option<&str>, default: &str) -> String {
    normalize_anthropic_base_url(base_url.unwrap_or(default))
}

fn openai_base_url(base_url: Option<&str>, default: &str) -> String {
    normalize_openai_base_url(base_url.unwrap_or(default))
}

fn current_shell() -> String {
    std::env::var("SHELL")
        .or_else(|_| std::env::var("COMSPEC"))
        .or_else(|_| std::env::var("ComSpec"))
        .unwrap_or_else(|_| {
            if cfg!(windows) {
                "powershell.exe".to_string()
            } else {
                "/bin/bash".to_string()
            }
        })
}

fn looks_anthropic_compatible_url(base_url: Option<&str>) -> bool {
    base_url
        .map(|url| {
            let lowered = url.trim().to_ascii_lowercase();
            lowered.contains("/anthropic") || lowered.contains("claude-code-proxy")
        })
        .unwrap_or(false)
}

fn build_handler(
    provider_name: &str,
    api_key: Option<&str>,
    base_url: Option<&str>,
    model_id: Option<&str>,
) -> anyhow::Result<Box<dyn Provider>> {
    match provider_name {
        "anthropic" | "kuaikat" | "kuai-kat" | "kat" | "kat-coder" | "kat-coder-pro"
        | "streamlake" => {
            let api_key = api_key
                .ok_or_else(|| anyhow::anyhow!("api_key is required for {provider_name}"))?;
            let cfg = roo_provider_anthropic::AnthropicConfig {
                api_key: api_key.to_string(),
                base_url: anthropic_base_url(
                    base_url,
                    roo_provider_anthropic::AnthropicConfig::DEFAULT_BASE_URL,
                ),
                model_id: model_id.map(|s| s.to_string()),
                temperature: None,
                use_extended_thinking: None,
                max_thinking_tokens: None,
                request_timeout: None,
                enable_1m_context: false,
            };
            Ok(Box::new(roo_provider_anthropic::AnthropicHandler::new(
                cfg,
            )?))
        }

        "openai" => {
            let api_key =
                api_key.ok_or_else(|| anyhow::anyhow!("api_key is required for openai"))?;
            let cfg = roo_provider_openai::OpenAiConfig {
                api_key: api_key.to_string(),
                base_url: openai_base_url(
                    base_url,
                    roo_provider_openai::OpenAiConfig::DEFAULT_BASE_URL,
                ),
                org_id: None,
                model_id: model_id.map(|s| s.to_string()),
                temperature: None,
                reasoning_effort: None,
                request_timeout: None,
                use_azure: false,
                azure_api_version: None,
                streaming_enabled: true,
                headers: std::collections::HashMap::new(),
                r1_format_enabled: false,
                custom_model_info: None,
            };
            Ok(Box::new(roo_provider_openai::OpenAiHandler::new(cfg)?))
        }

        "openai-native" => {
            let api_key =
                api_key.ok_or_else(|| anyhow::anyhow!("api_key is required for openai-native"))?;
            let cfg = roo_provider_openai_native::OpenAiNativeConfig {
                api_key: api_key.to_string(),
                base_url: base_url.map(|s| s.to_string()),
                model_id: model_id.map(|s| s.to_string()),
                temperature: None,
                reasoning_effort: None,
                request_timeout: None,
                service_tier: None,
                enable_reasoning_summary: true,
            };
            Ok(Box::new(
                roo_provider_openai_native::OpenAiNativeHandler::new(cfg)?,
            ))
        }

        "openrouter" => {
            let api_key =
                api_key.ok_or_else(|| anyhow::anyhow!("api_key is required for openrouter"))?;
            let cfg = roo_provider_openrouter::OpenRouterConfig {
                api_key: api_key.to_string(),
                base_url: openai_base_url(
                    base_url,
                    roo_provider_openrouter::OpenRouterConfig::DEFAULT_BASE_URL,
                ),
                model_id: model_id.map(|s| s.to_string()),
                temperature: None,
                request_timeout: None,
            };
            Ok(Box::new(roo_provider_openrouter::OpenRouterHandler::new(
                cfg,
            )?))
        }

        "deepseek" if looks_anthropic_compatible_url(base_url) => {
            let api_key =
                api_key.ok_or_else(|| anyhow::anyhow!("api_key is required for deepseek"))?;
            let cfg = roo_provider_anthropic::AnthropicConfig {
                api_key: api_key.to_string(),
                base_url: anthropic_base_url(
                    base_url,
                    roo_provider_anthropic::AnthropicConfig::DEFAULT_BASE_URL,
                ),
                model_id: model_id.map(|s| s.to_string()),
                temperature: None,
                use_extended_thinking: None,
                max_thinking_tokens: None,
                request_timeout: None,
                enable_1m_context: false,
            };
            Ok(Box::new(roo_provider_anthropic::AnthropicHandler::new(
                cfg,
            )?))
        }

        "deepseek" => {
            let api_key =
                api_key.ok_or_else(|| anyhow::anyhow!("api_key is required for deepseek"))?;
            let cfg = roo_provider_deepseek::DeepSeekConfig {
                api_key: api_key.to_string(),
                base_url: openai_base_url(
                    base_url,
                    roo_provider_deepseek::DeepSeekConfig::DEFAULT_BASE_URL,
                ),
                model_id: model_id.map(|s| s.to_string()),
                temperature: None,
                request_timeout: None,
            };
            Ok(Box::new(roo_provider_deepseek::DeepSeekHandler::new(cfg)?))
        }

        "gemini" | "google" => {
            let api_key =
                api_key.ok_or_else(|| anyhow::anyhow!("api_key is required for gemini"))?;
            let cfg = roo_provider_google::GoogleConfig {
                api_key: api_key.to_string(),
                base_url: base_url
                    .unwrap_or(roo_provider_google::GoogleConfig::DEFAULT_BASE_URL)
                    .to_string(),
                model_id: model_id.map(|s| s.to_string()),
                temperature: None,
                request_timeout: None,
            };
            Ok(Box::new(roo_provider_google::GoogleHandler::new(cfg)?))
        }

        "ollama" => {
            let cfg = roo_provider_ollama::OllamaConfig {
                base_url: base_url
                    .unwrap_or(roo_provider_ollama::OllamaConfig::DEFAULT_BASE_URL)
                    .to_string(),
                model_id: model_id.map(|s| s.to_string()),
                temperature: None,
                request_timeout: None,
                api_options: None,
                num_ctx: None,
            };
            Ok(Box::new(roo_provider_ollama::OllamaHandler::new(cfg)?))
        }

        "lmstudio" => {
            let cfg = roo_provider_lmstudio::LmStudioConfig {
                base_url: base_url
                    .unwrap_or(roo_provider_lmstudio::LmStudioConfig::DEFAULT_BASE_URL)
                    .to_string(),
                model_id: model_id.map(|s| s.to_string()),
                temperature: None,
                request_timeout: None,
                speculative_decoding_enabled: false,
                draft_model_id: None,
            };
            Ok(Box::new(roo_provider_lmstudio::LmStudioHandler::new(cfg)?))
        }

        "xai" => {
            let api_key = api_key.ok_or_else(|| anyhow::anyhow!("api_key is required for xai"))?;
            let cfg = roo_provider_xai::XaiConfig {
                api_key: api_key.to_string(),
                base_url: openai_base_url(base_url, roo_provider_xai::XaiConfig::DEFAULT_BASE_URL),
                model_id: model_id.map(|s| s.to_string()),
                temperature: None,
                request_timeout: None,
            };
            Ok(Box::new(roo_provider_xai::XaiHandler::new(cfg)?))
        }

        "mistral" => {
            let api_key =
                api_key.ok_or_else(|| anyhow::anyhow!("api_key is required for mistral"))?;
            let cfg = roo_provider_mistral::MistralConfig {
                api_key: api_key.to_string(),
                base_url: openai_base_url(
                    base_url,
                    roo_provider_mistral::MistralConfig::DEFAULT_BASE_URL,
                ),
                model_id: model_id.map(|s| s.to_string()),
                temperature: None,
                request_timeout: None,
            };
            Ok(Box::new(roo_provider_mistral::MistralHandler::new(cfg)?))
        }

        "fireworks" => {
            let api_key =
                api_key.ok_or_else(|| anyhow::anyhow!("api_key is required for fireworks"))?;
            let cfg = roo_provider_fireworks::FireworksConfig {
                api_key: api_key.to_string(),
                base_url: openai_base_url(
                    base_url,
                    roo_provider_fireworks::FireworksConfig::DEFAULT_BASE_URL,
                ),
                model_id: model_id.map(|s| s.to_string()),
                temperature: None,
                request_timeout: None,
            };
            Ok(Box::new(roo_provider_fireworks::FireworksHandler::new(
                cfg,
            )?))
        }

        "litellm" => {
            let api_key = api_key.unwrap_or("dummy-key").to_string();
            let cfg = roo_provider_litellm::LiteLlmConfig {
                api_key,
                base_url: openai_base_url(
                    base_url,
                    roo_provider_litellm::LiteLlmConfig::DEFAULT_BASE_URL,
                ),
                model_id: model_id.map(|s| s.to_string()),
                temperature: None,
                use_prompt_cache: false,
                request_timeout: None,
            };
            Ok(Box::new(roo_provider_litellm::LiteLlmHandler::new(cfg)?))
        }

        "qwen" | "qwen-code" => {
            let api_key = api_key.ok_or_else(|| anyhow::anyhow!("api_key is required for qwen"))?;
            let cfg = roo_provider_qwen::QwenConfig {
                api_key: api_key.to_string(),
                base_url: openai_base_url(
                    base_url,
                    roo_provider_qwen::QwenConfig::DEFAULT_BASE_URL,
                ),
                model_id: model_id.map(|s| s.to_string()),
                temperature: None,
                request_timeout: None,
            };
            Ok(Box::new(roo_provider_qwen::QwenHandler::new(cfg)?))
        }

        "minimax" => {
            let api_key =
                api_key.ok_or_else(|| anyhow::anyhow!("api_key is required for minimax"))?;
            let cfg = roo_provider_minimax::MiniMaxConfig {
                api_key: api_key.to_string(),
                base_url: anthropic_base_url(
                    base_url,
                    roo_provider_minimax::MiniMaxConfig::DEFAULT_BASE_URL,
                ),
                group_id: None,
                model_id: model_id.map(|s| s.to_string()),
                temperature: None,
                request_timeout: None,
            };
            Ok(Box::new(roo_provider_minimax::MiniMaxHandler::new(cfg)?))
        }

        "fake-ai" | "fake" => {
            let cfg = roo_provider_fake_ai::FakeAiConfig {
                model_id: model_id
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "fake-ai-model".to_string()),
                ..roo_provider_fake_ai::FakeAiConfig::default()
            };
            Ok(Box::new(roo_provider_fake_ai::FakeAiHandler::new(cfg)))
        }

        "moonshot" => {
            let api_key =
                api_key.ok_or_else(|| anyhow::anyhow!("api_key is required for moonshot"))?;
            let cfg = roo_provider_moonshot::MoonshotConfig {
                api_key: api_key.to_string(),
                base_url: openai_base_url(
                    base_url,
                    roo_provider_moonshot::MoonshotConfig::DEFAULT_BASE_URL,
                ),
                model_id: model_id.map(|s| s.to_string()),
                temperature: None,
                request_timeout: None,
            };
            Ok(Box::new(roo_provider_moonshot::MoonshotHandler::new(cfg)?))
        }

        "zai" => {
            let api_key = api_key.ok_or_else(|| anyhow::anyhow!("api_key is required for zai"))?;
            let cfg = roo_provider_zai::ZaiConfig {
                api_key: api_key.to_string(),
                base_url: openai_base_url(base_url, roo_provider_zai::ZaiConfig::DEFAULT_BASE_URL),
                model_id: model_id.map(|s| s.to_string()),
                temperature: None,
                request_timeout: None,
            };
            Ok(Box::new(roo_provider_zai::ZaiHandler::new(cfg)?))
        }

        "sambanova" => {
            let api_key =
                api_key.ok_or_else(|| anyhow::anyhow!("api_key is required for sambanova"))?;
            let cfg = roo_provider_sambanova::SambaNovaConfig {
                api_key: api_key.to_string(),
                base_url: openai_base_url(
                    base_url,
                    roo_provider_sambanova::SambaNovaConfig::DEFAULT_BASE_URL,
                ),
                model_id: model_id.map(|s| s.to_string()),
                temperature: None,
                request_timeout: None,
            };
            Ok(Box::new(roo_provider_sambanova::SambaNovaHandler::new(
                cfg,
            )?))
        }

        "baseten" => {
            let api_key =
                api_key.ok_or_else(|| anyhow::anyhow!("api_key is required for baseten"))?;
            let cfg = roo_provider_baseten::BasetenConfig {
                api_key: api_key.to_string(),
                base_url: openai_base_url(
                    base_url,
                    roo_provider_baseten::BasetenConfig::DEFAULT_BASE_URL,
                ),
                model_id: model_id.map(|s| s.to_string()),
                temperature: None,
                request_timeout: None,
            };
            Ok(Box::new(roo_provider_baseten::BasetenHandler::new(cfg)?))
        }

        "poe" => {
            let api_key = api_key.ok_or_else(|| anyhow::anyhow!("api_key is required for poe"))?;
            let cfg = roo_provider_poe::PoeConfig {
                api_key: api_key.to_string(),
                base_url: base_url.map(|s| s.to_string()),
                model_id: model_id.map(|s| s.to_string()),
                temperature: None,
                max_thinking_tokens: None,
                reasoning_effort: None,
                request_timeout: None,
            };
            Ok(Box::new(roo_provider_poe::PoeHandler::new(cfg)?))
        }

        "requesty" => {
            let api_key =
                api_key.ok_or_else(|| anyhow::anyhow!("api_key is required for requesty"))?;
            let cfg = roo_provider_requesty::RequestyConfig {
                api_key: api_key.to_string(),
                base_url: openai_base_url(
                    base_url,
                    roo_provider_requesty::RequestyConfig::DEFAULT_BASE_URL,
                ),
                model_id: model_id.map(|s| s.to_string()),
                temperature: None,
                request_timeout: None,
            };
            Ok(Box::new(roo_provider_requesty::RequestyHandler::new(cfg)?))
        }

        "unbound" => {
            let api_key =
                api_key.ok_or_else(|| anyhow::anyhow!("api_key is required for unbound"))?;
            let cfg = roo_provider_unbound::UnboundConfig {
                api_key: api_key.to_string(),
                base_url: None,
                model_id: model_id.map(|s| s.to_string()),
                temperature: None,
                request_timeout: None,
            };
            Ok(Box::new(roo_provider_unbound::UnboundHandler::new(cfg)?))
        }

        "vercel" | "vercel-ai-gateway" => {
            let api_key =
                api_key.ok_or_else(|| anyhow::anyhow!("api_key is required for vercel"))?;
            let cfg = roo_provider_vercel::VercelConfig {
                api_key: api_key.to_string(),
                base_url: base_url.map(|s| s.to_string()),
                model_id: model_id.map(|s| s.to_string()),
                temperature: None,
                request_timeout: None,
            };
            Ok(Box::new(roo_provider_vercel::VercelHandler::new(cfg)?))
        }

        "roo" => {
            let cfg = roo_provider_roo::RooConfig {
                api_key: api_key.map(|s| s.to_string()),
                base_url: base_url
                    .map(normalize_openai_base_url)
                    .or_else(|| Some(roo_provider_roo::RooConfig::DEFAULT_BASE_URL.to_string())),
                model_id: model_id.map(|s| s.to_string()),
                temperature: None,
                request_timeout: None,
            };
            Ok(Box::new(roo_provider_roo::RooHandler::new(cfg)?))
        }

        "aws" | "bedrock" => {
            let api_key = api_key.ok_or_else(|| {
                anyhow::anyhow!("api_key is required for bedrock (format: access_key:secret_key)")
            })?;
            let parts: Vec<&str> = api_key.splitn(2, ':').collect();
            let (access_key, secret_key) = if parts.len() == 2 {
                (parts[0].to_string(), parts[1].to_string())
            } else {
                anyhow::bail!("api_key for bedrock must be in format access_key:secret_key");
            };
            let cfg = roo_provider_aws::AwsBedrockConfig {
                access_key,
                secret_key,
                session_token: None,
                region: base_url
                    .unwrap_or(roo_provider_aws::AwsBedrockConfig::DEFAULT_REGION)
                    .to_string(),
                model_id: model_id.map(|s| s.to_string()),
                use_cross_region_inference: false,
                endpoint_url: None,
                request_timeout: None,
                temperature: None,
                service_tier: None,
                enable_1m_context: false,
                use_global_inference: false,
                use_profile: false,
                profile_name: None,
                use_api_key: false,
                api_key: None,
                vpc_endpoint: None,
                vpc_endpoint_enabled: false,
                use_prompt_cache: true,
            };
            Ok(Box::new(roo_provider_aws::AwsBedrockHandler::new(cfg)?))
        }

        _ => {
            anyhow::bail!(
                "Unsupported Roo provider: '{}'. Supported: anthropic, kuaikat, openai, openai-native, \
                 openrouter, deepseek, gemini, ollama, lmstudio, xai, mistral, fireworks, \
                 litellm, qwen, minimax, fake-ai, moonshot, zai, sambanova, baseten, poe, \
                 requesty, unbound, vercel, roo, bedrock",
                provider_name
            )
        }
    }
}

fn adapter_auto_approval_state(auto_approval_enabled: bool) -> AutoApprovalState {
    AutoApprovalState {
        auto_approval_enabled,
        ..AutoApprovalState::default()
    }
}

fn parse_mode_from_config(config: &AgentConfig) -> String {
    let mut args = config.args.iter();
    while let Some(arg) = args.next() {
        if arg == "--mode" {
            if let Some(mode) = args.next()
                && !mode.trim().is_empty()
            {
                return mode.to_string();
            }
        } else if let Some(mode) = arg.strip_prefix("--mode=")
            && !mode.trim().is_empty()
        {
            return mode.to_string();
        }
    }

    config
        .env
        .iter()
        .find_map(|(key, value)| {
            if key.eq_ignore_ascii_case("ROO_MODE") && !value.trim().is_empty() {
                Some(value.to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| roo_modes::default_mode_slug().to_string())
}

fn default_global_custom_modes_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    [
        home.join(".roo").join("settings").join("custom_modes.yaml"),
        home.join(".roo").join("custom_modes.yaml"),
    ]
    .into_iter()
    .find(|path| path.exists())
}

fn load_custom_modes_for_cwd(cwd: &std::path::Path) -> Vec<roo_types::mode::ModeConfig> {
    let project_roomodes = cwd.join(roo_modes::ROOMODES_FILENAME);
    let project_roomodes = project_roomodes.exists().then_some(project_roomodes);
    let global_settings = default_global_custom_modes_path().unwrap_or_default();
    let mut manager = roo_modes::CustomModesManager::new(global_settings, project_roomodes);
    manager.get_custom_modes()
}

// ---------------------------------------------------------------------------
// Event bridge: TaskEvent → UnifiedAgentEvent
// ---------------------------------------------------------------------------

/// Maps Roo's native [`RooTaskEvent`] to [`UnifiedAgentEvent`].
///
/// This bridge is registered as a listener on the [`TaskEventEmitter`]
/// inside the [`TaskEngine`]. Every event the native `AgentLoop` emits
/// is translated and forwarded to the GUI via the unified event channel.
fn map_task_event(event: &RooTaskEvent, session_id: &str) -> Option<UnifiedAgentEvent> {
    match event {
        // --- Streaming text ---
        RooTaskEvent::StreamingTextDelta { text, .. } => Some(UnifiedAgentEvent::MessageDelta {
            session_id: session_id.to_string(),
            delta: text.clone(),
        }),

        // --- Streaming reasoning ---
        RooTaskEvent::StreamingReasoningDelta { text, .. } => {
            Some(UnifiedAgentEvent::MessageDelta {
                session_id: session_id.to_string(),
                delta: format!("[thinking] {}", text),
            })
        }

        // --- Tool use lifecycle ---
        RooTaskEvent::StreamingToolUseStarted {
            tool_name, tool_id, ..
        } => Some(UnifiedAgentEvent::ToolCallStarted {
            session_id: session_id.to_string(),
            tool_name: tool_name.clone(),
            tool_input: serde_json::json!({ "tool_id": tool_id }),
        }),

        RooTaskEvent::StreamingToolUseDelta { tool_id, delta, .. } => {
            Some(UnifiedAgentEvent::ToolCallProgress {
                session_id: session_id.to_string(),
                tool_name: String::new(),
                progress: format!("[{tool_id}] {delta}"),
            })
        }

        RooTaskEvent::StreamingToolUseCompleted {
            tool_name, success, ..
        } => Some(UnifiedAgentEvent::ToolCallCompleted {
            session_id: session_id.to_string(),
            tool_name: tool_name.clone(),
            result: serde_json::json!({
                "success": success,
            }),
        }),

        // --- Task lifecycle ---
        RooTaskEvent::TaskStarted { .. } => {
            let caps = rc_agent_protocol::util::standard_capabilities(&[
                rc_agent_protocol::types::AgentCapability::McpSupport,
                rc_agent_protocol::types::AgentCapability::Subtasks,
            ]);
            Some(UnifiedAgentEvent::Started(AgentInfo {
                name: "Roo In-Process".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                capabilities: caps,
                status: AgentStatus::Busy,
            }))
        }

        RooTaskEvent::TaskCompleted { .. } => {
            // The Completed event is emitted from run_loop() result handling below.
            // We map it here for completeness, but the final result is built
            // after run_loop() returns.
            Some(UnifiedAgentEvent::Ready)
        }

        RooTaskEvent::TaskAborted { reason, .. } => Some(UnifiedAgentEvent::Error {
            session_id: session_id.to_string(),
            message: reason.clone().unwrap_or_else(|| "Task aborted".to_string()),
            recoverable: false,
        }),

        // --- Context management ---
        RooTaskEvent::ContextCondensationCompleted {
            messages_removed, ..
        } => Some(UnifiedAgentEvent::ContextCompacted {
            session_id: session_id.to_string(),
            entries_removed: *messages_removed,
            usage_ratio: 0.0,
        }),

        RooTaskEvent::ContextTruncationPerformed {
            messages_removed, ..
        } => Some(UnifiedAgentEvent::ContextCompacted {
            session_id: session_id.to_string(),
            entries_removed: *messages_removed,
            usage_ratio: 0.0,
        }),

        // --- Token usage ---
        RooTaskEvent::TokenUsageUpdated { usage } => Some(UnifiedAgentEvent::ContextUsage {
            session_id: session_id.to_string(),
            used: (usage.total_tokens_in + usage.total_tokens_out) as usize,
            total: usage.context_tokens as usize,
        }),

        RooTaskEvent::TaskTokenUsageUpdated { token_usage, .. } => {
            Some(UnifiedAgentEvent::ContextUsage {
                session_id: session_id.to_string(),
                used: (token_usage.total_tokens_in + token_usage.total_tokens_out) as usize,
                total: token_usage.context_tokens as usize,
            })
        }

        // --- Tool approval ---
        RooTaskEvent::ToolApprovalRequired {
            tool_name,
            tool_id,
            reason,
            task_id,
        } => Some(UnifiedAgentEvent::PermissionRequest {
            session_id: session_id.to_string(),
            request_id: tool_id.clone(),
            tool_name: tool_name.clone(),
            input: serde_json::json!({
                "ask": "tool",
                "task_id": task_id,
                "tool_use_id": tool_id,
                "tool": tool_name,
                "reason": reason,
                "feedback_supported": true,
                "allow_all_supported": true,
            }),
        }),

        // --- Subtask lifecycle ---
        RooTaskEvent::TaskSpawned { child_task_id, .. } => {
            Some(UnifiedAgentEvent::SubtaskStarted {
                session_id: session_id.to_string(),
                task_id: child_task_id.clone(),
                description: String::new(),
            })
        }

        RooTaskEvent::TaskDelegationCompleted { summary, .. } => {
            Some(UnifiedAgentEvent::SubtaskCompleted {
                session_id: session_id.to_string(),
                task_id: String::new(),
                result: serde_json::json!({ "summary": summary }),
            })
        }

        // --- Errors ---
        RooTaskEvent::Error { error, .. } => Some(UnifiedAgentEvent::Error {
            session_id: session_id.to_string(),
            message: error.clone(),
            recoverable: true,
        }),

        RooTaskEvent::ToolError {
            tool_name, error, ..
        } => Some(UnifiedAgentEvent::Error {
            session_id: session_id.to_string(),
            message: format!("Tool '{}' error: {}", tool_name, error),
            recoverable: true,
        }),

        RooTaskEvent::TaskToolFailed {
            tool_name, error, ..
        } => Some(UnifiedAgentEvent::Error {
            session_id: session_id.to_string(),
            message: format!("Tool '{}' failed: {}", tool_name, error),
            recoverable: true,
        }),

        RooTaskEvent::ApiRequestRetryDelayed {
            delay_seconds,
            retry_attempt,
            ..
        } => Some(UnifiedAgentEvent::ToolCallProgress {
            session_id: session_id.to_string(),
            tool_name: "api_retry".to_string(),
            progress: format!(
                "retry delayed: {}s (attempt {})",
                delay_seconds, retry_attempt
            ),
        }),

        RooTaskEvent::ApiRequestFailed { task_id, error } => {
            Some(UnifiedAgentEvent::PermissionRequest {
                session_id: session_id.to_string(),
                request_id: format!("api_retry:{task_id}"),
                tool_name: "api_request_failed".to_string(),
                input: serde_json::json!({
                    "error": error,
                    "decision": "allow retries the request; deny cancels the task",
                }),
            })
        }

        RooTaskEvent::MistakeLimitReached {
            task_id,
            count,
            limit,
        } => Some(UnifiedAgentEvent::PermissionRequest {
            session_id: session_id.to_string(),
            request_id: format!("mistake_limit:{task_id}"),
            tool_name: "mistake_limit_reached".to_string(),
            input: serde_json::json!({
                "count": count,
                "limit": limit,
                "decision": "allow continues without extra feedback; deny cancels the task",
            }),
        }),

        RooTaskEvent::AutoApprovalLimitReached {
            task_id,
            approval_type,
            approval_count,
        } => Some(UnifiedAgentEvent::PermissionRequest {
            session_id: session_id.to_string(),
            request_id: format!("auto_approval_limit:{task_id}"),
            tool_name: "auto_approval_max_req_reached".to_string(),
            input: serde_json::json!({
                "ask": "auto_approval_max_req_reached",
                "task_id": task_id,
                "type": approval_type,
                "count": approval_count,
                "decision": "allow continues after the auto-approval limit; deny cancels the task",
            }),
        }),

        RooTaskEvent::ApiRateLimitWait { seconds, .. } => {
            Some(UnifiedAgentEvent::ToolCallProgress {
                session_id: session_id.to_string(),
                tool_name: "api_rate_limit_wait".to_string(),
                progress: format!("{seconds}s remaining before next API request"),
            })
        }

        RooTaskEvent::FollowupQuestion {
            task_id,
            tool_call_id,
            question_json,
        } => Some(UnifiedAgentEvent::PermissionRequest {
            session_id: session_id.to_string(),
            request_id: tool_call_id.clone(),
            tool_name: "ask_followup_question".to_string(),
            input: serde_json::json!({
                "ask": "followup",
                "task_id": task_id,
                "tool_use_id": tool_call_id,
                "question": serde_json::from_str::<serde_json::Value>(question_json)
                    .unwrap_or_else(|_| serde_json::json!({ "question": question_json })),
                "requires_text_response": true,
                "response_via": "send_message",
            }),
        }),

        RooTaskEvent::CompletionResult {
            task_id,
            tool_call_id,
            completion_text,
        } => Some(UnifiedAgentEvent::PermissionRequest {
            session_id: session_id.to_string(),
            request_id: tool_call_id.clone(),
            tool_name: "attempt_completion".to_string(),
            input: serde_json::json!({
                "ask": "completion_result",
                "task_id": task_id,
                "tool_use_id": tool_call_id,
                "result": completion_text,
                "accepts_feedback": true,
                "feedback_via": "send_message",
            }),
        }),

        // --- Streaming completed ---
        RooTaskEvent::StreamingCompleted { .. } => {
            debug!("Streaming completed");
            None
        }

        // --- Tool execution feedback ---
        RooTaskEvent::ToolExecuted { tool_name, success } => {
            Some(UnifiedAgentEvent::ToolCallProgress {
                session_id: session_id.to_string(),
                tool_name: tool_name.clone(),
                progress: if *success {
                    "completed".to_string()
                } else {
                    "failed".to_string()
                },
            })
        }

        // --- State changes ---
        RooTaskEvent::StateChanged { to, .. } => {
            debug!(state = %to, "Task state changed");
            None
        }

        // --- API events ---
        RooTaskEvent::ApiRequestStarted { .. } => {
            debug!("API request started");
            None
        }
        RooTaskEvent::ApiRequestFinished { .. } => {
            debug!("API request finished");
            None
        }

        // --- Ignore events that don't map to unified events ---
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// RooInProcessAdapter
// ---------------------------------------------------------------------------

/// In-process adapter for Roo Code using the native [`AgentLoop`].
///
/// This adapter provides 100% feature coverage by delegating to Roo's
/// built-in agent loop, which handles:
/// - API calls with streaming
/// - Tool execution (including MCP tools)
/// - Context compression / condensation
/// - Budget management and rate limiting
/// - Subtask spawning and delegation
/// - Mistake detection and recovery
/// - Checkpoint management
pub struct RooInProcessAdapter {
    info: AgentInfo,
    status: AgentStatus,
    cwd: PathBuf,
    model: Option<String>,
    api_key: Option<String>,
    provider_name: Option<String>,
    base_url: Option<String>,
    storage_path: Option<PathBuf>,
    api_config_name: Option<String>,
    mode: String,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
    worker_handle: Option<std::thread::JoinHandle<()>>,
    conversation_history: Arc<Mutex<Vec<ApiMessage>>>,
    auto_approval_enabled: bool,
    external_mcp_servers: std::collections::HashMap<String, serde_json::Value>,
    /// Shared handle to the AgentLoop's pending approval channel.
    /// When the GUI approves/denies a tool, we send the response through this.
    approval_handle: Option<BoolDecisionHandle>,
    /// Shared handle to the AgentLoop's pending API retry prompt.
    api_retry_handle: Option<BoolDecisionHandle>,
    /// Shared handle to the AgentLoop's pending auto-approval limit prompt.
    auto_approval_limit_handle: Option<BoolDecisionHandle>,
    /// Shared handle to the AgentLoop's pending mistake-limit prompt.
    mistake_limit_handle: Option<MistakeLimitDecisionHandle>,
    /// Shared handle to the AgentLoop's pending followup answer prompt.
    followup_handle: Option<StringDecisionHandle>,
    /// Shared handle to the AgentLoop's pending completion feedback prompt.
    completion_handle: Option<StringDecisionHandle>,
}

impl RooInProcessAdapter {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let caps = rc_agent_protocol::util::standard_capabilities(&[
            rc_agent_protocol::types::AgentCapability::McpSupport,
            rc_agent_protocol::types::AgentCapability::Subtasks,
        ]);

        Self {
            info: AgentInfo {
                name: "Roo In-Process".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                capabilities: caps,
                status: AgentStatus::Starting,
            },
            status: AgentStatus::Starting,
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            model: None,
            api_key: None,
            provider_name: None,
            base_url: None,
            storage_path: None,
            api_config_name: None,
            mode: roo_modes::default_mode_slug().to_string(),
            cancel_token: None,
            worker_handle: None,
            approval_handle: None,
            api_retry_handle: None,
            auto_approval_limit_handle: None,
            mistake_limit_handle: None,
            followup_handle: None,
            completion_handle: None,
            conversation_history: Arc::new(Mutex::new(Vec::new())),
            auto_approval_enabled: false,
            external_mcp_servers: std::collections::HashMap::new(),
        }
    }

    fn config_env_value(config: &AgentConfig, key: &str) -> Option<String> {
        config
            .env
            .iter()
            .find(|(name, value)| name == key && !value.trim().is_empty())
            .map(|(_, value)| value.clone())
    }

    fn build_task_config(
        task_id: &str,
        cwd: &str,
        mode: &str,
        message: &str,
        auto_approval: bool,
        storage_path: Option<&str>,
        api_config_name: Option<&str>,
    ) -> TaskConfig {
        let mut config = TaskConfig::new(task_id, cwd)
            .with_mode(mode)
            .with_task_text(message)
            .with_auto_approval(auto_approval)
            .with_start_task(true);

        if let Some(storage_path) = storage_path.filter(|path| !path.trim().is_empty()) {
            config = config.with_storage_path(storage_path);
        }
        if let Some(api_config_name) = api_config_name.filter(|name| !name.trim().is_empty()) {
            config = config.with_api_config(api_config_name);
        }

        config
    }

    /// Set MCP servers discovered from the GUI's centralized configuration.
    /// These are merged with `.roo/mcp.json` servers in `load_mcp_servers()`.
    pub fn set_external_mcp_servers(
        &mut self,
        servers: std::collections::HashMap<String, serde_json::Value>,
    ) {
        self.external_mcp_servers = servers;
    }

    fn build_system_prompt(
        &self,
        has_mcp: bool,
        custom_modes: Option<&[roo_types::mode::ModeConfig]>,
    ) -> String {
        let cwd_str = self.cwd.to_string_lossy();
        let shell = current_shell();
        let home = dirs::home_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "/tmp".to_string());
        let os_info = format!("{} {}", std::env::consts::OS, std::env::consts::ARCH);
        let mut ignore_controller = RooIgnoreController::new(&cwd_str);
        let roo_ignore_instructions = std::fs::read_to_string(self.cwd.join(".rooignore"))
            .ok()
            .and_then(|content| {
                ignore_controller.load_patterns(&content);
                ignore_controller.get_instructions()
            });
        let settings = roo_prompt::types::SystemPromptSettings::default();

        build_system_prompt(
            &cwd_str,
            &self.mode,
            custom_modes,
            None,
            has_mcp,
            None,
            None,
            roo_ignore_instructions.as_deref(),
            Some(&settings),
            &[],
            &os_info,
            &shell,
            &home,
        )
    }

    fn build_dispatcher(
        cwd: &std::path::Path,
        mode: &str,
        mcp_hub: Arc<roo_mcp::McpHub>,
    ) -> ToolDispatcher {
        let registry = Arc::new(TerminalRegistry::new());
        let output_dir = cwd.join(".roo");
        roo_task::tool_dispatcher::default_dispatcher_full(registry, output_dir, mode, mcp_hub)
    }

    pub fn set_auto_approval_enabled(&mut self, enabled: bool) {
        self.auto_approval_enabled = enabled;
    }

    pub async fn clear_history(&self) {
        self.conversation_history.lock().await.clear();
    }

    /// Resolve a pending Roo tool-approval request.
    ///
    /// If the agent loop is waiting on a tool approval, forward the GUI
    /// decision to its pending oneshot channel.
    pub async fn resolve_roo_approval(
        &mut self,
        request_id: &str,
        allowed: bool,
    ) -> anyhow::Result<()> {
        let decision = if allowed {
            PermissionDecision::Allow
        } else {
            PermissionDecision::Deny
        };
        self.resolve_roo_permission(request_id, decision, None, None)
            .await
    }

    /// Resolve a pending Roo interaction with its request kind preserved.
    ///
    /// Roo exposes several user-interaction paths through the same unified GUI
    /// permission event: tool approval, API retry, mistake-limit feedback,
    /// follow-up questions, and completion acceptance/feedback. The GUI passes
    /// the `ask`/tool kind back here so the adapter can answer the matching
    /// native oneshot instead of treating every response as a boolean approval.
    pub async fn resolve_roo_permission(
        &mut self,
        request_id: &str,
        decision: PermissionDecision,
        response: Option<String>,
        request_kind: Option<&str>,
    ) -> anyhow::Result<()> {
        let allowed = matches!(
            decision,
            PermissionDecision::Allow | PermissionDecision::AllowAll
        );
        match RooPermissionKind::from_request(request_id, request_kind) {
            Some(RooPermissionKind::ApiRetry) => return self.resolve_api_retry(allowed).await,
            Some(RooPermissionKind::AutoApprovalLimit) => {
                return self.resolve_auto_approval_limit(allowed).await;
            }
            Some(RooPermissionKind::MistakeLimit) => {
                return self.resolve_mistake_limit(allowed, response).await;
            }
            Some(RooPermissionKind::Followup) => {
                let text = if allowed {
                    response.unwrap_or_default()
                } else {
                    response
                        .filter(|text| !text.trim().is_empty())
                        .unwrap_or_else(|| {
                            "The user declined to answer the follow-up question.".to_string()
                        })
                };
                if self.resolve_followup_response(text) {
                    return Ok(());
                }
                debug!(allowed, "No pending Roo follow-up response in AgentLoop");
                return Ok(());
            }
            Some(RooPermissionKind::Completion) => {
                let feedback = if allowed {
                    response.unwrap_or_default()
                } else {
                    response
                        .filter(|text| !text.trim().is_empty())
                        .unwrap_or_else(|| "The user rejected the completion result.".to_string())
                };
                if self.resolve_completion_feedback(feedback) {
                    return Ok(());
                }
                debug!(allowed, "No pending Roo completion response in AgentLoop");
                return Ok(());
            }
            Some(RooPermissionKind::ToolApproval) | None => {}
        }

        // Compatibility fallback for callers that only know the request id and
        // boolean decision. Prefer active text interactions first because the
        // native loop has no separate boolean channel for those asks.
        if request_kind.is_none() {
            if self.resolve_completion_feedback(if allowed {
                String::new()
            } else {
                "The user rejected the completion result.".to_string()
            }) {
                return Ok(());
            }
            if self.resolve_followup_response(if allowed {
                response.unwrap_or_default()
            } else {
                response
                    .filter(|text| !text.trim().is_empty())
                    .unwrap_or_else(|| {
                        "The user declined to answer the follow-up question.".to_string()
                    })
            }) {
                return Ok(());
            }
        }

        if let Some(ref handle) = self.approval_handle {
            let mut guard = match handle.lock() {
                Ok(g) => g,
                Err(e) => {
                    warn!("Approval mutex poisoned, recovering: {e}");
                    e.into_inner()
                }
            };
            if let Some(tx) = guard.take() {
                let _ = tx.send(allowed);
                debug!(allowed, "Sent approval response to AgentLoop");
                return Ok(());
            }
        }
        debug!(
            allowed,
            "No pending approval in AgentLoop (auto-approval mode or no handle)"
        );
        Ok(())
    }

    fn resolve_text_handle(handle: &Option<StringDecisionHandle>, text: String) -> bool {
        let Some(handle) = handle else {
            return false;
        };
        let mut guard = match handle.lock() {
            Ok(g) => g,
            Err(e) => {
                warn!("Text-response mutex poisoned, recovering: {e}");
                e.into_inner()
            }
        };
        if let Some(tx) = guard.take() {
            let _ = tx.send(text);
            return true;
        }
        false
    }

    fn resolve_followup_response(&mut self, response: String) -> bool {
        Self::resolve_text_handle(&self.followup_handle, response)
    }

    fn resolve_completion_feedback(&mut self, feedback: String) -> bool {
        Self::resolve_text_handle(&self.completion_handle, feedback)
    }

    async fn resolve_api_retry(&mut self, allowed: bool) -> anyhow::Result<()> {
        if let Some(ref handle) = self.api_retry_handle {
            let mut guard = match handle.lock() {
                Ok(g) => g,
                Err(e) => {
                    warn!("API retry mutex poisoned, recovering: {e}");
                    e.into_inner()
                }
            };
            if let Some(tx) = guard.take() {
                let _ = tx.send(allowed);
                debug!(retry = allowed, "Sent API retry response to AgentLoop");
                return Ok(());
            }
        }
        debug!(retry = allowed, "No pending API retry in AgentLoop");
        Ok(())
    }

    async fn resolve_auto_approval_limit(&mut self, allowed: bool) -> anyhow::Result<()> {
        if let Some(ref handle) = self.auto_approval_limit_handle {
            let mut guard = match handle.lock() {
                Ok(g) => g,
                Err(e) => {
                    warn!("Auto-approval-limit mutex poisoned, recovering: {e}");
                    e.into_inner()
                }
            };
            if let Some(tx) = guard.take() {
                let _ = tx.send(allowed);
                debug!(
                    approved = allowed,
                    "Sent auto-approval-limit response to AgentLoop"
                );
                return Ok(());
            }
        }
        debug!(
            approved = allowed,
            "No pending auto-approval-limit prompt in AgentLoop"
        );
        Ok(())
    }

    async fn resolve_mistake_limit(
        &mut self,
        allowed: bool,
        feedback: Option<String>,
    ) -> anyhow::Result<()> {
        if let Some(ref handle) = self.mistake_limit_handle {
            let mut guard = match handle.lock() {
                Ok(g) => g,
                Err(e) => {
                    warn!("Mistake-limit mutex poisoned, recovering: {e}");
                    e.into_inner()
                }
            };
            if let Some(tx) = guard.take() {
                let action = if allowed {
                    MistakeLimitAction::Continue {
                        feedback: feedback.filter(|text| !text.trim().is_empty()),
                    }
                } else {
                    MistakeLimitAction::Cancel
                };
                let _ = tx.send(action);
                debug!(
                    continue_task = allowed,
                    "Sent mistake-limit response to AgentLoop"
                );
                return Ok(());
            }
        }
        debug!(
            continue_task = allowed,
            "No pending mistake-limit prompt in AgentLoop"
        );
        Ok(())
    }

    /// Load MCP servers from GUI configuration and `.roo/mcp.json`.
    ///
    /// The returned hub must live for the whole task because MCP tool handlers
    /// call through it when `use_mcp_tool` or `access_mcp_resource` executes.
    async fn load_mcp_hub(&self) -> (Arc<roo_mcp::McpHub>, Vec<McpServerConnection>) {
        let cwd_str = self.cwd.to_string_lossy().to_string();
        let hub = Arc::new(roo_mcp::McpHub::new_with_paths(Some(cwd_str.clone()), None));
        hub.initialize_weak_self();

        // 1. Load GUI-managed MCP servers (from centralized config: mcp.toml, .mcp.json).
        //    These are "global" scope — project-level config can override them.
        if !self.external_mcp_servers.is_empty() {
            let count = self.external_mcp_servers.len();
            if let Err(e) = hub
                .update_server_connections(
                    &self.external_mcp_servers,
                    roo_mcp::McpSource::Global,
                    true,
                )
                .await
            {
                warn!("Failed to load GUI MCP servers: {}", e);
            } else {
                info!("Loaded {} GUI-managed MCP servers", count);
            }
        }

        // 2. Load project-level MCP config (.roo/mcp.json).
        //    Project scope overrides global, so same-name servers will be replaced.
        let mcp_path = std::path::Path::new(&cwd_str).join(".roo").join("mcp.json");

        if mcp_path.exists()
            && let Ok(content) = std::fs::read_to_string(&mcp_path)
            && let Ok(config) = serde_json::from_str::<serde_json::Value>(&content)
            && let Some(servers) = config.get("mcpServers").and_then(|v| v.as_object())
        {
            let server_map: std::collections::HashMap<String, serde_json::Value> = servers
                .into_iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let count = server_map.len();
            if let Err(e) = hub
                .update_server_connections(&server_map, roo_mcp::McpSource::Project, true)
                .await
            {
                warn!("Failed to load project MCP servers: {}", e);
            } else {
                info!(
                    "Loaded {} project MCP servers from {}",
                    count,
                    mcp_path.display()
                );
            }
        }

        // Give servers a moment to connect (max 5 seconds per server, total 10s cap)
        let server_count = hub.get_servers().len();
        if server_count > 0 {
            info!(
                count = server_count,
                "Waiting for MCP servers to connect..."
            );
            tokio::time::sleep(std::time::Duration::from_millis(
                (server_count as u64 * 2000).min(10000),
            ))
            .await;
        }

        let servers = hub.get_servers();
        (hub, servers)
    }

    /// Inner body of the Roo agent loop, extracted so it can be wrapped in
    /// `catch_unwind`.  Runs on a dedicated OS thread (not a tokio task)
    /// because `AgentLoop` contains !Send types (`git2::Repository`).
    #[allow(clippy::too_many_arguments)]
    fn run_agent_loop_inner(
        task_id: String,
        cwd_str: String,
        mode: String,
        message_owned: String,
        auto_approval: bool,
        session_id: String,
        tx: tokio::sync::mpsc::Sender<UnifiedAgentEvent>,
        history_snapshot: Vec<ApiMessage>,
        provider: Box<dyn Provider>,
        message_builder: MessageBuilder,
        dispatcher: ToolDispatcher,
        history: Arc<tokio::sync::Mutex<Vec<ApiMessage>>>,
        mcp_servers: Vec<McpServerConnection>,
        custom_modes: Vec<roo_types::mode::ModeConfig>,
        cancel_token: tokio_util::sync::CancellationToken,
        approval_out: BoolDecisionSender,
        api_retry_out: BoolDecisionSender,
        auto_approval_limit_out: BoolDecisionSender,
        mistake_limit_out: MistakeLimitDecisionSender,
        followup_out: StringDecisionSender,
        completion_out: StringDecisionSender,
        storage_path: Option<String>,
        api_config_name: Option<String>,
    ) {
        let config = Self::build_task_config(
            &task_id,
            &cwd_str,
            &mode,
            &message_owned,
            auto_approval,
            storage_path.as_deref(),
            api_config_name.as_deref(),
        );

        let mut engine = match TaskEngine::new(config) {
            Ok(e) => e,
            Err(e) => {
                let _ = tx.blocking_send(UnifiedAgentEvent::Error {
                    session_id: session_id.clone(),
                    message: format!("TaskEngine creation failed: {}", e),
                    recoverable: false,
                });
                return;
            }
        };

        if !history_snapshot.is_empty() {
            engine.set_api_conversation_history(history_snapshot);
        }

        let tx_ev = tx.clone();
        let sid_ev = session_id.clone();
        engine.emitter().on(move |event: &RooTaskEvent| {
            if let Some(unified) = map_task_event(event, &sid_ev) {
                let _ = tx_ev.blocking_send(unified);
            }
        });

        let has_mcp = !mcp_servers.is_empty();
        let loop_config = AgentLoopConfig {
            auto_approval_enabled: auto_approval,
            auto_approval: adapter_auto_approval_state(auto_approval),
            enable_condense: true,
            has_mcp,
            mcp_servers: mcp_servers.clone(),
            custom_modes: (!custom_modes.is_empty()).then_some(custom_modes),
            ..AgentLoopConfig::default()
        };

        // --- Wire service controllers into AgentLoop ---

        // RooIgnoreController: load .rooignore patterns if the file exists
        let mut ignore_controller = RooIgnoreController::new(&cwd_str);
        let rooignore_path = std::path::Path::new(&cwd_str).join(".rooignore");
        if let Ok(content) = std::fs::read_to_string(&rooignore_path) {
            ignore_controller.load_patterns(&content);
            info!(
                path = %rooignore_path.display(),
                "Loaded .rooignore patterns"
            );
        }

        // RooProtectedController: enforce write-protection on Roo config files
        let protected_controller = RooProtectedController::new(&cwd_str);

        // FileContextTracker: track file reads/edits for stale context detection
        let context_tracker = FileContextTracker::new(&task_id, InMemoryMetadataStore::new());

        // DiffViewProvider: manage file editing sessions with diff tracking
        let diff_view_provider = DiffViewProvider::new_default();

        // MCP servers loaded from .roo/mcp.json via McpHub.
        // The hub is created in the async context (send_message) and
        // server connections are collected before spawning the thread.
        // If MCP loading fails, we gracefully fall back to empty servers.

        let mut agent_loop = AgentLoop::new(engine, provider, message_builder, dispatcher)
            .with_config(loop_config)
            .with_roo_ignore_controller(ignore_controller)
            .with_roo_protected_controller(protected_controller)
            .with_file_context_tracker(context_tracker)
            .with_diff_view_provider(diff_view_provider)
            .with_mcp_servers(mcp_servers);

        // Wire the adapter's cancellation token to the AgentLoop's internal token
        // so that cancel() from the GUI propagates into the running agent loop.
        let agent_loop_token = agent_loop.cancellation_token().clone();

        // Pass interactive-response handles back to the adapter so the GUI can
        // respond to Roo asks emitted by the native loop.
        {
            let approval_handle = agent_loop.approval_handle();
            match approval_out.lock() {
                Ok(mut guard) => *guard = Some(approval_handle),
                Err(e) => {
                    warn!("Approval-out mutex poisoned, recovering: {e}");
                    *e.into_inner() = Some(agent_loop.approval_handle());
                }
            }

            let api_retry_handle = agent_loop.api_retry_handle();
            match api_retry_out.lock() {
                Ok(mut guard) => *guard = Some(api_retry_handle),
                Err(e) => {
                    warn!("API-retry-out mutex poisoned, recovering: {e}");
                    *e.into_inner() = Some(agent_loop.api_retry_handle());
                }
            }

            let auto_approval_limit_handle = agent_loop.auto_approval_limit_handle();
            match auto_approval_limit_out.lock() {
                Ok(mut guard) => *guard = Some(auto_approval_limit_handle),
                Err(e) => {
                    warn!("Auto-approval-limit-out mutex poisoned, recovering: {e}");
                    *e.into_inner() = Some(agent_loop.auto_approval_limit_handle());
                }
            }

            let mistake_limit_handle = agent_loop.mistake_limit_handle();
            match mistake_limit_out.lock() {
                Ok(mut guard) => *guard = Some(mistake_limit_handle),
                Err(e) => {
                    warn!("Mistake-limit-out mutex poisoned, recovering: {e}");
                    *e.into_inner() = Some(agent_loop.mistake_limit_handle());
                }
            }

            let followup_handle = agent_loop.followup_handle();
            match followup_out.lock() {
                Ok(mut guard) => *guard = Some(followup_handle),
                Err(e) => {
                    warn!("Followup-out mutex poisoned, recovering: {e}");
                    *e.into_inner() = Some(agent_loop.followup_handle());
                }
            }

            let completion_handle = agent_loop.completion_handle();
            match completion_out.lock() {
                Ok(mut guard) => *guard = Some(completion_handle),
                Err(e) => {
                    warn!("Completion-out mutex poisoned, recovering: {e}");
                    *e.into_inner() = Some(agent_loop.completion_handle());
                }
            }
        }

        {
            let adapter_token = cancel_token.clone();
            // Use a simple polling approach since CancellationToken doesn't have
            // a synchronous blocking wait. The poll interval is short enough to
            // provide responsive cancellation without busy-wait overhead.
            std::thread::spawn(move || {
                while !adapter_token.is_cancelled() {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                agent_loop_token.cancel();
            });
        }

        let _ = tx.blocking_send(UnifiedAgentEvent::Ready);

        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to create tokio runtime for agent loop");
                return;
            }
        };

        match rt.block_on(agent_loop.run_loop()) {
            Ok(result) => {
                tracing::info!(
                    task_id = %result.task_id,
                    iterations = result.iterations,
                    "AgentLoop completed successfully"
                );

                {
                    let engine_history = agent_loop.engine().api_conversation_history();
                    let mut h = history.blocking_lock();
                    *h = engine_history.to_vec();
                }

                let final_message = result.final_message.clone().unwrap_or_default();
                let usage_info = rc_agent_protocol::events::UsageInfo {
                    input_tokens: result.token_usage.total_tokens_in,
                    output_tokens: result.token_usage.total_tokens_out,
                    cache_read: result.token_usage.total_cache_reads.unwrap_or(0),
                    cache_write: result.token_usage.total_cache_writes.unwrap_or(0),
                };

                let tool_calls: Vec<rc_agent_protocol::events::ToolCallInfo> = result
                    .tool_usage
                    .iter()
                    .map(|(name, count)| rc_agent_protocol::events::ToolCallInfo {
                        id: String::new(),
                        name: name.clone(),
                        input: serde_json::json!({}),
                        output: serde_json::json!({ "count": count }),
                    })
                    .collect();

                let _ = tx.blocking_send(UnifiedAgentEvent::Completed {
                    session_id,
                    result: rc_agent_protocol::events::AgentResult {
                        response_text: final_message,
                        tool_calls,
                        usage: usage_info,
                        cost: Some(result.token_usage.total_cost),
                    },
                });
            }
            Err(e) => {
                tracing::warn!(error = %e, "AgentLoop ended with error");
                let _ = tx.blocking_send(UnifiedAgentEvent::Error {
                    session_id,
                    message: format!("AgentLoop error: {}", e),
                    recoverable: false,
                });
            }
        }

        // Cancel the token so the watchdog thread exits cleanly.
        cancel_token.cancel();

        tracing::debug!("Roo native agent loop background task finished");
    }
}

// ---------------------------------------------------------------------------
// AgentAdapter implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl AgentAdapter for RooInProcessAdapter {
    async fn start(&mut self, config: &AgentConfig) -> anyhow::Result<()> {
        info!(working_dir = ?config.working_dir, "Starting Roo In-Process adapter (native AgentLoop)");

        if let Some(ref wd) = config.working_dir {
            self.cwd = wd.clone();
        }
        self.model = config.model.clone();
        self.api_key = config.api_key.clone();
        self.base_url = config.base_url.clone();
        self.storage_path =
            Self::config_env_value(config, "ROO_TASK_STORAGE_PATH").map(PathBuf::from);
        self.api_config_name = Self::config_env_value(config, "ROO_API_CONFIG_NAME");
        self.mode = parse_mode_from_config(config);

        self.provider_name = config.provider.clone().or_else(|| match config.agent_type {
            AgentType::RemoteRoo => Some("anthropic".to_string()),
            _ => None,
        });

        let provider_name = self
            .provider_name
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("provider name is required for Roo adapter"))?;

        // Validate provider config by building a handler (but don't keep it)
        let _ = build_handler(
            provider_name,
            self.api_key.as_deref(),
            self.base_url.as_deref(),
            self.model.as_deref(),
        )?;

        self.status = AgentStatus::Ready;
        self.info.status = AgentStatus::Ready;

        info!(provider = %provider_name, "Roo In-Process adapter started successfully (native AgentLoop)");
        Ok(())
    }

    async fn send_message(
        &mut self,
        session_id: &str,
        message: &str,
    ) -> anyhow::Result<mpsc::Receiver<UnifiedAgentEvent>> {
        let (tx, rx) = mpsc::channel(256);

        if self.resolve_followup_response(message.to_string()) {
            let _ = tx.send(UnifiedAgentEvent::Ready).await;
            return Ok(rx);
        }
        if self.resolve_completion_feedback(message.to_string()) {
            let _ = tx.send(UnifiedAgentEvent::Ready).await;
            return Ok(rx);
        }

        let panic_tx = tx.clone();

        // Build provider
        let provider = build_handler(
            self.provider_name.as_deref().unwrap_or("anthropic"),
            self.api_key.as_deref(),
            self.base_url.as_deref(),
            self.model.as_deref(),
        )?;

        // Load MCP servers before building prompt/tools so MCP capability
        // signals and tool handlers match the connected server state.
        let (mcp_hub, mcp_servers) = self.load_mcp_hub().await;
        info!(
            count = mcp_servers.len(),
            "Loaded MCP servers for agent loop"
        );

        let custom_modes = load_custom_modes_for_cwd(&self.cwd);
        let has_mcp = !mcp_servers.is_empty();

        // Build system prompt and message builder.
        let system_prompt = self.build_system_prompt(has_mcp, Some(&custom_modes));
        let message_builder = MessageBuilder::new(&system_prompt);

        // Build tool dispatcher with a live MCP hub so MCP tools can execute.
        let dispatcher = Self::build_dispatcher(&self.cwd, &self.mode, mcp_hub);

        // Clone conversation history while in async context (before spawning).
        let history_snapshot = {
            let history = self.conversation_history.lock().await;
            history.clone()
        };

        // Cancel any previous run's token so its watchdog thread exits.
        if let Some(ref old_token) = self.cancel_token {
            old_token.cancel();
        }

        // Create a cancellation token for this run.
        let cancel_token = tokio_util::sync::CancellationToken::new();
        self.cancel_token = Some(cancel_token.clone());

        // Keep a reference to conversation history for updating after the run.
        let history = Arc::clone(&self.conversation_history);
        let session_id_for_completion = session_id.to_string();
        let panic_session_id = session_id_for_completion.clone();

        // Prepare owned parameters for the worker thread.
        let task_id = uuid::Uuid::now_v7().to_string();
        let cwd_str = self.cwd.to_string_lossy().to_string();
        let mode = self.mode.clone();
        let message_owned = message.to_string();
        let auto_approval = self.auto_approval_enabled;
        let mcp_servers_owned = mcp_servers;
        let custom_modes_owned = custom_modes;
        let storage_path = self
            .storage_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string());
        let api_config_name = self.api_config_name.clone();

        // Shared slot for the AgentLoop's approval handle to be passed back
        // from the worker thread to this adapter.
        let approval_out: BoolDecisionSender = Arc::new(std::sync::Mutex::new(None));
        let approval_out_clone = Arc::clone(&approval_out);
        let api_retry_out: BoolDecisionSender = Arc::new(std::sync::Mutex::new(None));
        let api_retry_out_clone = Arc::clone(&api_retry_out);
        let auto_approval_limit_out: BoolDecisionSender = Arc::new(std::sync::Mutex::new(None));
        let auto_approval_limit_out_clone = Arc::clone(&auto_approval_limit_out);
        let mistake_limit_out: MistakeLimitDecisionSender = Arc::new(std::sync::Mutex::new(None));
        let mistake_limit_out_clone = Arc::clone(&mistake_limit_out);
        let followup_out: StringDecisionSender = Arc::new(std::sync::Mutex::new(None));
        let followup_out_clone = Arc::clone(&followup_out);
        let completion_out: StringDecisionSender = Arc::new(std::sync::Mutex::new(None));
        let completion_out_clone = Arc::clone(&completion_out);

        // Spawn in a dedicated OS thread.
        //
        // AgentLoop contains `Option<ShadowCheckpointService>` which wraps
        // `git2::Repository` — a raw-pointer type that is NOT Send/Sync.
        // By constructing AgentLoop INSIDE the thread, it never crosses a
        // thread boundary, so the closure only needs to capture Send-safe
        // components (provider, message_builder, dispatcher, etc.).
        let handle = std::thread::spawn(move || {
            // Wrap entire agent loop body in catch_unwind so a panic inside
            // the Roo AgentLoop (which contains !Send types like git2::Repository)
            // is converted to an error event instead of crashing the whole
            // Tauri process.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                Self::run_agent_loop_inner(
                    task_id,
                    cwd_str,
                    mode,
                    message_owned,
                    auto_approval,
                    session_id_for_completion,
                    tx,
                    history_snapshot,
                    provider,
                    message_builder,
                    dispatcher,
                    history,
                    mcp_servers_owned,
                    custom_modes_owned,
                    cancel_token,
                    approval_out_clone,
                    api_retry_out_clone,
                    auto_approval_limit_out_clone,
                    mistake_limit_out_clone,
                    followup_out_clone,
                    completion_out_clone,
                    storage_path,
                    api_config_name,
                );
            }));

            if let Err(panic_payload) = result {
                let event = rc_agent_protocol::util::panic_to_error_event(
                    &panic_session_id,
                    "Roo agent panicked",
                    panic_payload,
                );
                tracing::error!(
                    error = match &event {
                        rc_agent_protocol::events::UnifiedAgentEvent::Error { message, .. } =>
                            message.clone(),
                        _ => unreachable!(),
                    },
                    "Roo agent loop panicked — isolated to thread"
                );
                let _ = panic_tx.blocking_send(event);
            }
        });

        let _ = cancel_token; // already stored in self.cancel_token
        self.worker_handle = Some(handle);
        self.status = AgentStatus::Busy;
        self.info.status = AgentStatus::Busy;

        // Pick up the approval handle that was set by the worker thread.
        // Busy-wait briefly because the worker may not have constructed
        // the AgentLoop yet.  In practice this resolves within 1-2 iterations.
        for _ in 0..50 {
            let taken = match approval_out.lock() {
                Ok(mut guard) => guard.take(),
                Err(e) => {
                    warn!("Approval-out mutex poisoned during pickup, recovering: {e}");
                    e.into_inner().take()
                }
            };
            if let Some(ah) = taken {
                self.approval_handle = Some(ah);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        for _ in 0..50 {
            let taken = match api_retry_out.lock() {
                Ok(mut guard) => guard.take(),
                Err(e) => {
                    warn!("API-retry-out mutex poisoned during pickup, recovering: {e}");
                    e.into_inner().take()
                }
            };
            if let Some(handle) = taken {
                self.api_retry_handle = Some(handle);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        for _ in 0..50 {
            let taken = match auto_approval_limit_out.lock() {
                Ok(mut guard) => guard.take(),
                Err(e) => {
                    warn!("Auto-approval-limit-out mutex poisoned during pickup, recovering: {e}");
                    e.into_inner().take()
                }
            };
            if let Some(handle) = taken {
                self.auto_approval_limit_handle = Some(handle);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        for _ in 0..50 {
            let taken = match mistake_limit_out.lock() {
                Ok(mut guard) => guard.take(),
                Err(e) => {
                    warn!("Mistake-limit-out mutex poisoned during pickup, recovering: {e}");
                    e.into_inner().take()
                }
            };
            if let Some(handle) = taken {
                self.mistake_limit_handle = Some(handle);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        for _ in 0..50 {
            let taken = match followup_out.lock() {
                Ok(mut guard) => guard.take(),
                Err(e) => {
                    warn!("Followup-out mutex poisoned during pickup, recovering: {e}");
                    e.into_inner().take()
                }
            };
            if let Some(handle) = taken {
                self.followup_handle = Some(handle);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        for _ in 0..50 {
            let taken = match completion_out.lock() {
                Ok(mut guard) => guard.take(),
                Err(e) => {
                    warn!("Completion-out mutex poisoned during pickup, recovering: {e}");
                    e.into_inner().take()
                }
            };
            if let Some(handle) = taken {
                self.completion_handle = Some(handle);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        Ok(rx)
    }

    async fn cancel(&mut self, _session_id: &str) -> anyhow::Result<()> {
        info!("Cancelling Roo task");

        if let Some(ref token) = self.cancel_token {
            token.cancel();
        }

        // Keep the worker_handle alive so is_alive() stays true until the
        // thread actually exits.  The next send_message() will replace it.
        // Reset status to Ready so the adapter can accept new prompts.
        self.status = AgentStatus::Ready;
        self.info.status = AgentStatus::Ready;

        Ok(())
    }

    async fn resolve_permission(
        &mut self,
        _session_id: &str,
        request_id: &str,
        decision: PermissionDecision,
    ) -> anyhow::Result<()> {
        let approved = matches!(
            decision,
            PermissionDecision::Allow | PermissionDecision::AllowAll
        );
        if request_id.starts_with("api_retry:") {
            return self.resolve_api_retry(approved).await;
        }
        if request_id.starts_with("auto_approval_limit:") {
            return self.resolve_auto_approval_limit(approved).await;
        }
        if request_id.starts_with("mistake_limit:") {
            return self.resolve_mistake_limit(approved, None).await;
        }
        self.resolve_roo_permission(request_id, decision, None, None)
            .await
    }

    async fn stop(&mut self) -> anyhow::Result<()> {
        info!("Stopping Roo In-Process adapter");

        if let Some(ref token) = self.cancel_token {
            token.cancel();
        }
        // std::thread::JoinHandle has no abort().  The thread will
        // exit when it observes the cancelled CancellationToken.
        self.worker_handle = None;
        self.approval_handle = None;
        self.api_retry_handle = None;
        self.auto_approval_limit_handle = None;
        self.mistake_limit_handle = None;
        self.followup_handle = None;
        self.completion_handle = None;

        self.conversation_history.lock().await.clear();

        self.status = AgentStatus::Stopped;
        self.info.status = AgentStatus::Stopped;

        Ok(())
    }

    fn is_alive(&self) -> bool {
        !matches!(self.status, AgentStatus::Stopped)
    }

    fn info(&self) -> &AgentInfo {
        &self.info
    }

    fn agent_type(&self) -> AgentType {
        AgentType::RemoteRoo
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roo_task::TaskEvent as RooTaskEvent;
    use roo_types::message::TokenUsage;

    const SID: &str = "test-session-roo";

    fn make_token_usage(in_t: u64, out_t: u64, ctx: u64) -> TokenUsage {
        TokenUsage {
            total_tokens_in: in_t,
            total_tokens_out: out_t,
            total_cache_writes: None,
            total_cache_reads: None,
            total_cost: 0.0,
            context_tokens: ctx,
        }
    }

    fn test_config(args: Vec<String>, env: Vec<(String, String)>) -> AgentConfig {
        AgentConfig {
            agent_type: AgentType::RemoteRoo,
            binary_path: None,
            args,
            env,
            working_dir: None,
            model: None,
            provider: Some("fake-ai".to_string()),
            api_key: None,
            base_url: None,
        }
    }

    #[test]
    fn adapter_defaults_do_not_auto_approve_tools() {
        let adapter = RooInProcessAdapter::new();

        assert!(!adapter.auto_approval_enabled);
    }

    #[test]
    fn adapter_defaults_to_alive_before_worker_starts() {
        let adapter = RooInProcessAdapter::new();

        assert!(<RooInProcessAdapter as AgentAdapter>::is_alive(&adapter));
    }

    #[test]
    fn parse_mode_prefers_cli_arg_then_env_then_roo_default() {
        let arg_config = test_config(vec!["--mode=debug".to_string()], Vec::new());
        assert_eq!(parse_mode_from_config(&arg_config), "debug");

        let split_arg_config = test_config(
            vec!["--mode".to_string(), "ask".to_string()],
            vec![("ROO_MODE".to_string(), "code".to_string())],
        );
        assert_eq!(parse_mode_from_config(&split_arg_config), "ask");

        let env_config = test_config(
            Vec::new(),
            vec![("ROO_MODE".to_string(), "code".to_string())],
        );
        assert_eq!(parse_mode_from_config(&env_config), "code");

        let default_config = test_config(Vec::new(), Vec::new());
        assert_eq!(parse_mode_from_config(&default_config), "architect");
    }

    #[test]
    fn task_config_preserves_gui_storage_and_provider_profile() {
        let config = RooInProcessAdapter::build_task_config(
            "task-1",
            "D:/work",
            "code",
            "hello",
            false,
            Some("D:/profile/roo"),
            Some("minimax"),
        );

        assert_eq!(config.storage_path.as_deref(), Some("D:/profile/roo"));
        assert_eq!(config.api_config_name.as_deref(), Some("minimax"));
        assert_eq!(config.mode, "code");
        assert_eq!(config.task_text.as_deref(), Some("hello"));
    }

    #[test]
    fn adapter_system_prompt_includes_rooignore_instructions() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(".rooignore"), "secrets/**\n").expect("write .rooignore");
        let mut adapter = RooInProcessAdapter::new();
        adapter.cwd = dir.path().to_path_buf();
        adapter.mode = "code".to_string();

        let prompt = adapter.build_system_prompt(false, None);

        assert!(prompt.contains("# .rooignore"));
        assert!(prompt.contains("secrets/**"));
    }

    #[test]
    fn normalizes_gui_endpoint_base_urls_before_provider_builders_append_paths() {
        assert_eq!(
            normalize_anthropic_base_url("https://api.minimaxi.com/anthropic/v1/messages"),
            "https://api.minimaxi.com/anthropic"
        );
        assert_eq!(
            normalize_anthropic_base_url("https://api.anthropic.com/v1/messages"),
            "https://api.anthropic.com"
        );
        assert_eq!(
            normalize_openai_base_url("https://api.openai.com/v1/chat/completions"),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            normalize_openai_base_url("https://openrouter.ai/api/v1/"),
            "https://openrouter.ai/api/v1"
        );
    }

    #[test]
    fn build_dispatcher_registers_mcp_handlers() {
        let hub = Arc::new(roo_mcp::McpHub::new());
        let dispatcher =
            RooInProcessAdapter::build_dispatcher(std::path::Path::new("."), "code", hub);

        assert!(dispatcher.has_handler("use_mcp_tool"));
        assert!(dispatcher.has_handler("access_mcp_resource"));
    }

    #[test]
    fn adapter_auto_approval_state_does_not_grant_broad_permissions() {
        let state = adapter_auto_approval_state(true);

        assert!(state.auto_approval_enabled);
        assert!(!state.always_allow_read_only);
        assert!(!state.always_allow_read_only_outside_workspace);
        assert!(!state.always_allow_write);
        assert!(!state.always_allow_write_outside_workspace);
        assert!(!state.always_allow_mcp);
        assert!(!state.always_allow_execute);
    }

    #[test]
    fn build_handler_supports_fake_ai_without_api_key() {
        let handler = build_handler("fake-ai", None, None, None).expect("fake-ai provider");
        let (model_id, _) = handler.get_model();

        assert_eq!(model_id, "fake-ai-model");
    }

    #[test]
    fn build_handler_supports_fake_alias_and_model_override() {
        let handler =
            build_handler("fake", None, None, Some("fake-override")).expect("fake provider");
        let (model_id, _) = handler.get_model();

        assert_eq!(model_id, "fake-override");
    }

    #[test]
    fn build_handler_supports_kuaikat_anthropic_compatible_alias() {
        let handler = build_handler(
            "kuaikat",
            Some("test-key"),
            Some("https://wanqing.streamlakeapi.com/api/gateway/coding/kat-coder-pro-v2/claude-code-proxy"),
            Some("kat-coder-pro-v2"),
        )
        .expect("kuaikat provider");
        let (model_id, _) = handler.get_model();

        assert_eq!(model_id, "kat-coder-pro-v2");
    }

    #[test]
    fn build_handler_routes_deepseek_anthropic_endpoint_to_anthropic_protocol() {
        let handler = build_handler(
            "deepseek",
            Some("test-key"),
            Some("https://api.deepseek.com/anthropic"),
            Some("deepseek-v4-flash"),
        )
        .expect("deepseek anthropic-compatible provider");
        let (model_id, _) = handler.get_model();

        assert_eq!(model_id, "deepseek-v4-flash");
    }

    // ── Streaming events ──────────────────────────────────────────

    #[test]
    fn streaming_text_delta_maps_to_message_delta() {
        let event = RooTaskEvent::StreamingTextDelta {
            task_id: "t1".into(),
            text: "Hello".into(),
        };
        let result = map_task_event(&event, SID).expect("should map");
        match result {
            UnifiedAgentEvent::MessageDelta { session_id, delta } => {
                assert_eq!(session_id, SID);
                assert_eq!(delta, "Hello");
            }
            other => panic!("expected MessageDelta, got {other:?}"),
        }
    }

    #[test]
    fn streaming_reasoning_delta_has_thinking_prefix() {
        let event = RooTaskEvent::StreamingReasoningDelta {
            task_id: "t1".into(),
            text: "reasoning".into(),
        };
        let result = map_task_event(&event, SID).expect("should map");
        if let UnifiedAgentEvent::MessageDelta { delta, .. } = &result {
            assert!(delta.starts_with("[thinking] "), "got: {delta}");
        } else {
            panic!("expected MessageDelta");
        }
    }

    #[test]
    fn streaming_tool_use_started_maps_to_tool_call_started() {
        let event = RooTaskEvent::StreamingToolUseStarted {
            task_id: "t1".into(),
            tool_name: "read_file".into(),
            tool_id: "tc-1".into(),
        };
        let result = map_task_event(&event, SID).expect("should map");
        match result {
            UnifiedAgentEvent::ToolCallStarted { tool_name, .. } => {
                assert_eq!(tool_name, "read_file");
            }
            other => panic!("expected ToolCallStarted, got {other:?}"),
        }
    }

    #[test]
    fn streaming_tool_use_delta_maps_to_progress() {
        let event = RooTaskEvent::StreamingToolUseDelta {
            task_id: "t1".into(),
            tool_id: "tc-1".into(),
            delta: "partial".into(),
        };
        let result = map_task_event(&event, SID).expect("should map");
        if let UnifiedAgentEvent::ToolCallProgress {
            tool_name,
            progress,
            ..
        } = &result
        {
            assert_eq!(tool_name, "");
            assert!(
                progress.contains("tc-1"),
                "progress should contain tool_id: {progress}"
            );
            assert!(
                progress.contains("partial"),
                "progress should contain delta: {progress}"
            );
        } else {
            panic!("expected ToolCallProgress");
        }
    }

    #[test]
    fn streaming_tool_use_completed_maps_to_tool_call_completed() {
        let event = RooTaskEvent::StreamingToolUseCompleted {
            task_id: "t1".into(),
            tool_name: "bash".into(),
            tool_id: "tc-1".into(),
            success: true,
        };
        let result = map_task_event(&event, SID).expect("should map");
        if let UnifiedAgentEvent::ToolCallCompleted {
            tool_name, result, ..
        } = &result
        {
            assert_eq!(tool_name, "bash");
            assert_eq!(result["success"], true);
        } else {
            panic!("expected ToolCallCompleted");
        }
    }

    #[test]
    fn streaming_completed_returns_none() {
        let event = RooTaskEvent::StreamingCompleted {
            task_id: "t1".into(),
        };
        assert!(map_task_event(&event, SID).is_none());
    }

    // ── Task lifecycle ─────────────────────────────────────────────

    #[test]
    fn task_started_maps_to_started_with_capabilities() {
        let event = RooTaskEvent::TaskStarted {
            task_id: "t1".into(),
        };
        let result = map_task_event(&event, SID).expect("should map");
        if let UnifiedAgentEvent::Started(info) = &result {
            assert!(info.capabilities.len() >= 5);
        } else {
            panic!("expected Started");
        }
    }

    #[test]
    fn task_completed_maps_to_ready() {
        let event = RooTaskEvent::TaskCompleted {
            task_id: "t1".into(),
            token_usage: make_token_usage(100, 50, 200),
            tool_usage: roo_types::tool::ToolUsage::new(),
            is_subtask: false,
        };
        let result = map_task_event(&event, SID).expect("should map");
        assert!(matches!(result, UnifiedAgentEvent::Ready));
    }

    #[test]
    fn task_aborted_maps_to_error_not_recoverable() {
        let event = RooTaskEvent::TaskAborted {
            task_id: "t1".into(),
            reason: Some("user cancelled".into()),
        };
        let result = map_task_event(&event, SID).expect("should map");
        if let UnifiedAgentEvent::Error {
            message,
            recoverable,
            ..
        } = &result
        {
            assert_eq!(message, "user cancelled");
            assert!(!recoverable);
        } else {
            panic!("expected Error");
        }
    }

    #[test]
    fn task_aborted_no_reason_uses_default() {
        let event = RooTaskEvent::TaskAborted {
            task_id: "t1".into(),
            reason: None,
        };
        let result = map_task_event(&event, SID).expect("should map");
        if let UnifiedAgentEvent::Error { message, .. } = &result {
            assert_eq!(message, "Task aborted");
        } else {
            panic!("expected Error");
        }
    }

    // ── Context management ─────────────────────────────────────────

    #[test]
    fn context_condensation_maps_to_compacted() {
        let event = RooTaskEvent::ContextCondensationCompleted {
            task_id: "t1".into(),
            messages_removed: 15,
        };
        let result = map_task_event(&event, SID).expect("should map");
        if let UnifiedAgentEvent::ContextCompacted {
            entries_removed, ..
        } = &result
        {
            assert_eq!(*entries_removed, 15);
        } else {
            panic!("expected ContextCompacted");
        }
    }

    #[test]
    fn context_truncation_maps_to_compacted() {
        let event = RooTaskEvent::ContextTruncationPerformed {
            task_id: "t1".into(),
            messages_removed: 30,
        };
        let result = map_task_event(&event, SID).expect("should map");
        if let UnifiedAgentEvent::ContextCompacted {
            entries_removed, ..
        } = &result
        {
            assert_eq!(*entries_removed, 30);
        } else {
            panic!("expected ContextCompacted");
        }
    }

    // ── Token usage ────────────────────────────────────────────────

    #[test]
    fn token_usage_updated_maps_to_context_usage() {
        let event = RooTaskEvent::TokenUsageUpdated {
            usage: make_token_usage(500, 200, 4000),
        };
        let result = map_task_event(&event, SID).expect("should map");
        if let UnifiedAgentEvent::ContextUsage { used, total, .. } = &result {
            assert_eq!(*used, 700);
            assert_eq!(*total, 4000);
        } else {
            panic!("expected ContextUsage");
        }
    }

    #[test]
    fn task_token_usage_updated_maps_to_context_usage() {
        let event = RooTaskEvent::TaskTokenUsageUpdated {
            task_id: "t1".into(),
            token_usage: make_token_usage(300, 100, 2000),
            tool_usage: roo_types::tool::ToolUsage::new(),
        };
        let result = map_task_event(&event, SID).expect("should map");
        if let UnifiedAgentEvent::ContextUsage { used, total, .. } = &result {
            assert_eq!(*used, 400);
            assert_eq!(*total, 2000);
        } else {
            panic!("expected ContextUsage");
        }
    }

    // ── Tool approval ──────────────────────────────────────────────

    #[test]
    fn tool_approval_required_maps_to_permission_request() {
        let event = RooTaskEvent::ToolApprovalRequired {
            task_id: "t1".into(),
            tool_name: "bash".into(),
            tool_id: "tc-1".into(),
            reason: "dangerous command".into(),
        };
        let result = map_task_event(&event, SID).expect("should map");
        match result {
            UnifiedAgentEvent::PermissionRequest {
                request_id,
                tool_name,
                input,
                ..
            } => {
                assert_eq!(request_id, "tc-1");
                assert_eq!(tool_name, "bash");
                assert_eq!(input["reason"], "dangerous command");
            }
            other => panic!("expected PermissionRequest, got {other:?}"),
        }
    }

    // ── Subtask lifecycle ──────────────────────────────────────────

    #[test]
    fn task_spawned_maps_to_subtask_started() {
        let event = RooTaskEvent::TaskSpawned {
            parent_task_id: "p1".into(),
            child_task_id: "c1".into(),
        };
        let result = map_task_event(&event, SID).expect("should map");
        if let UnifiedAgentEvent::SubtaskStarted { task_id, .. } = &result {
            assert_eq!(task_id, "c1");
        } else {
            panic!("expected SubtaskStarted");
        }
    }

    #[test]
    fn task_delegation_completed_maps_to_subtask_completed() {
        let event = RooTaskEvent::TaskDelegationCompleted {
            parent_task_id: "p1".into(),
            child_task_id: "c1".into(),
            summary: "done".into(),
        };
        let result = map_task_event(&event, SID).expect("should map");
        if let UnifiedAgentEvent::SubtaskCompleted { result, .. } = &result {
            assert_eq!(result["summary"], "done");
        } else {
            panic!("expected SubtaskCompleted");
        }
    }

    // ── Error events ───────────────────────────────────────────────

    #[test]
    fn error_maps_to_recoverable_error() {
        let event = RooTaskEvent::Error {
            task_id: "t1".into(),
            error: "something broke".into(),
        };
        let result = map_task_event(&event, SID).expect("should map");
        if let UnifiedAgentEvent::Error {
            message,
            recoverable,
            ..
        } = &result
        {
            assert_eq!(message, "something broke");
            assert!(recoverable);
        } else {
            panic!("expected Error");
        }
    }

    #[test]
    fn tool_error_maps_to_recoverable_error() {
        let event = RooTaskEvent::ToolError {
            task_id: "t1".into(),
            tool_name: "bash".into(),
            error: "exit code 1".into(),
        };
        let result = map_task_event(&event, SID).expect("should map");
        if let UnifiedAgentEvent::Error {
            message,
            recoverable,
            ..
        } = &result
        {
            assert!(message.contains("bash"));
            assert!(message.contains("exit code 1"));
            assert!(recoverable);
        } else {
            panic!("expected Error");
        }
    }

    #[test]
    fn task_tool_failed_maps_to_recoverable_error() {
        let event = RooTaskEvent::TaskToolFailed {
            task_id: "t1".into(),
            tool_name: "edit_file".into(),
            error: "file not found".into(),
        };
        let result = map_task_event(&event, SID).expect("should map");
        if let UnifiedAgentEvent::Error {
            message,
            recoverable,
            ..
        } = &result
        {
            assert!(message.contains("edit_file"));
            assert!(message.contains("file not found"));
            assert!(recoverable);
        } else {
            panic!("expected Error");
        }
    }

    // ── API retry ──────────────────────────────────────────────────

    #[test]
    fn api_retry_delayed_maps_to_progress() {
        let event = RooTaskEvent::ApiRequestRetryDelayed {
            task_id: "t1".into(),
            delay_seconds: 5,
            retry_attempt: 2,
        };
        let result = map_task_event(&event, SID).expect("should map");
        if let UnifiedAgentEvent::ToolCallProgress {
            tool_name,
            progress,
            ..
        } = &result
        {
            assert_eq!(tool_name, "api_retry");
            assert!(progress.contains("5s"));
            assert!(progress.contains("attempt 2"));
        } else {
            panic!("expected ToolCallProgress");
        }
    }

    // ── Suppressed events ──────────────────────────────────────────

    #[test]
    fn state_changed_returns_none() {
        let event = RooTaskEvent::StateChanged {
            from: roo_task::types::TaskState::Idle,
            to: roo_task::types::TaskState::Running,
        };
        assert!(map_task_event(&event, SID).is_none());
    }

    #[test]
    fn api_request_started_returns_none() {
        let event = RooTaskEvent::ApiRequestStarted {
            task_id: "t1".into(),
        };
        assert!(map_task_event(&event, SID).is_none());
    }

    #[test]
    fn api_request_finished_returns_none() {
        let event = RooTaskEvent::ApiRequestFinished {
            task_id: "t1".into(),
            cost: Some(0.05),
            tokens_in: Some(100),
            tokens_out: Some(50),
        };
        assert!(map_task_event(&event, SID).is_none());
    }

    #[test]
    fn tool_executed_maps_to_progress() {
        let event = RooTaskEvent::ToolExecuted {
            tool_name: "read_file".into(),
            success: true,
        };
        let result = map_task_event(&event, SID).expect("should map");
        if let UnifiedAgentEvent::ToolCallProgress { progress, .. } = &result {
            assert_eq!(progress, "completed");
        } else {
            panic!("expected ToolCallProgress");
        }
    }

    #[test]
    fn tool_executed_failed_maps_to_failed_progress() {
        let event = RooTaskEvent::ToolExecuted {
            tool_name: "bash".into(),
            success: false,
        };
        let result = map_task_event(&event, SID).expect("should map");
        if let UnifiedAgentEvent::ToolCallProgress { progress, .. } = &result {
            assert_eq!(progress, "failed");
        } else {
            panic!("expected ToolCallProgress");
        }
    }

    // ── Catch-all events (fall into _ => None) ────────────────────

    #[test]
    fn checkpoint_saved_returns_none() {
        let event = RooTaskEvent::CheckpointSaved {
            task_id: "t1".into(),
            commit: Some("abc123".into()),
        };
        assert!(map_task_event(&event, SID).is_none());
    }

    #[test]
    fn task_created_returns_none() {
        let event = RooTaskEvent::TaskCreated {
            task_id: "t1".into(),
        };
        assert!(map_task_event(&event, SID).is_none());
    }

    #[test]
    fn mode_changed_returns_none() {
        let event = RooTaskEvent::ModeChanged {
            mode: "architect".into(),
        };
        assert!(map_task_event(&event, SID).is_none());
    }

    #[test]
    fn api_request_failed_maps_to_retry_permission_request() {
        let event = RooTaskEvent::ApiRequestFailed {
            task_id: "t1".into(),
            error: "timeout".into(),
        };
        let result = map_task_event(&event, SID).expect("should map");
        match result {
            UnifiedAgentEvent::PermissionRequest {
                request_id,
                tool_name,
                input,
                ..
            } => {
                assert_eq!(request_id, "api_retry:t1");
                assert_eq!(tool_name, "api_request_failed");
                assert_eq!(input["error"], "timeout");
            }
            other => panic!("expected PermissionRequest, got {other:?}"),
        }
    }

    #[test]
    fn mistake_limit_reached_maps_to_permission_request() {
        let event = RooTaskEvent::MistakeLimitReached {
            task_id: "t1".into(),
            count: 3,
            limit: 3,
        };
        let result = map_task_event(&event, SID).expect("should map");
        match result {
            UnifiedAgentEvent::PermissionRequest {
                request_id,
                tool_name,
                input,
                ..
            } => {
                assert_eq!(request_id, "mistake_limit:t1");
                assert_eq!(tool_name, "mistake_limit_reached");
                assert_eq!(input["count"], 3);
                assert_eq!(input["limit"], 3);
            }
            other => panic!("expected PermissionRequest, got {other:?}"),
        }
    }

    #[test]
    fn auto_approval_limit_reached_maps_to_permission_request() {
        let event = RooTaskEvent::AutoApprovalLimitReached {
            task_id: "t1".into(),
            approval_type: "Requests".into(),
            approval_count: Some("5".into()),
        };
        let result = map_task_event(&event, SID).expect("should map");
        if let UnifiedAgentEvent::PermissionRequest {
            request_id,
            tool_name,
            input,
            ..
        } = &result
        {
            assert_eq!(request_id, "auto_approval_limit:t1");
            assert_eq!(tool_name, "auto_approval_max_req_reached");
            assert_eq!(input["ask"], "auto_approval_max_req_reached");
            assert_eq!(input["type"], "Requests");
            assert_eq!(input["count"], "5");
        } else {
            panic!("expected PermissionRequest");
        }
    }

    #[test]
    fn followup_question_maps_to_permission_request_with_suggestions() {
        let event = RooTaskEvent::FollowupQuestion {
            task_id: "t1".into(),
            tool_call_id: "tool-1".into(),
            question_json: serde_json::json!({
                "question": "Continue?",
                "suggest": [{"answer": "Yes", "mode": "code"}],
            })
            .to_string(),
        };
        let result = map_task_event(&event, SID).expect("should map");
        match result {
            UnifiedAgentEvent::PermissionRequest {
                request_id,
                tool_name,
                input,
                ..
            } => {
                assert_eq!(request_id, "tool-1");
                assert_eq!(tool_name, "ask_followup_question");
                assert_eq!(input["ask"], "followup");
                assert_eq!(input["question"]["question"], "Continue?");
                assert_eq!(input["question"]["suggest"][0]["mode"], "code");
                assert_eq!(input["requires_text_response"], true);
            }
            other => panic!("expected PermissionRequest, got {other:?}"),
        }
    }

    #[test]
    fn completion_result_maps_to_permission_request() {
        let event = RooTaskEvent::CompletionResult {
            task_id: "t1".into(),
            tool_call_id: "tool-2".into(),
            completion_text: "Done".into(),
        };
        let result = map_task_event(&event, SID).expect("should map");
        match result {
            UnifiedAgentEvent::PermissionRequest {
                request_id,
                tool_name,
                input,
                ..
            } => {
                assert_eq!(request_id, "tool-2");
                assert_eq!(tool_name, "attempt_completion");
                assert_eq!(input["ask"], "completion_result");
                assert_eq!(input["result"], "Done");
                assert_eq!(input["accepts_feedback"], true);
            }
            other => panic!("expected PermissionRequest, got {other:?}"),
        }
    }

    #[test]
    fn api_rate_limit_wait_maps_to_progress() {
        let event = RooTaskEvent::ApiRateLimitWait {
            task_id: "t1".into(),
            seconds: 30,
        };
        let result = map_task_event(&event, SID).expect("should map");
        if let UnifiedAgentEvent::ToolCallProgress {
            tool_name,
            progress,
            ..
        } = &result
        {
            assert_eq!(tool_name, "api_rate_limit_wait");
            assert!(progress.contains("30s"));
        } else {
            panic!("expected ToolCallProgress");
        }
    }

    // ── TC-07: resolve_permission mutex recovery ───────────────────

    /// Test that a poisoned mutex in the approval handle is recovered gracefully
    /// via `unwrap_or_else(|e| e.into_inner())`, matching the pattern used in
    /// `resolve_roo_approval`.
    #[test]
    fn poisoned_approval_mutex_is_recovered() {
        let (tx, _rx) = tokio::sync::oneshot::channel::<bool>();
        let handle: Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<bool>>>> =
            Arc::new(std::sync::Mutex::new(Some(tx)));

        // Poison the mutex by panicking while holding the lock.
        // We use a separate thread so the panic is caught and does not
        // abort the test process.
        let handle_clone = Arc::clone(&handle);
        let join = std::thread::spawn(move || {
            let _guard = handle_clone
                .lock()
                .expect("test mutex should be lockable before intentional poisoning");
            panic!("intentional panic to poison mutex");
        });
        // The thread panicked, so the mutex is now poisoned.
        assert!(join.join().is_err(), "thread should have panicked");

        // Now verify the recovery pattern used in resolve_roo_approval:
        //   handle.lock().unwrap_or_else(|e| e.into_inner())
        // This must succeed (not panic) and give access to the inner data.
        let guard = handle.lock().unwrap_or_else(|e| e.into_inner());
        // The sender is still present; we just verify the guard is usable.
        assert!(
            guard.is_some(),
            "sender should still be present after poisoning"
        );
    }

    // ── TC-06: MCP server loading / merging ─────────────────────────

    /// Test that `set_external_mcp_servers` correctly stores the provided
    /// servers and that project-level servers can be identified by the
    /// stored external set.
    #[test]
    fn set_external_mcp_servers_stores_servers() {
        let mut adapter = RooInProcessAdapter::new();
        assert!(adapter.external_mcp_servers.is_empty());

        let mut servers = std::collections::HashMap::new();
        servers.insert(
            "filesystem".to_string(),
            serde_json::json!({ "command": "npx", "args": ["-y", "@anthropic/mcp-filesystem"] }),
        );
        servers.insert(
            "github".to_string(),
            serde_json::json!({ "command": "npx", "args": ["-y", "@anthropic/mcp-github"] }),
        );

        adapter.set_external_mcp_servers(servers.clone());
        assert_eq!(adapter.external_mcp_servers.len(), 2);
        assert!(adapter.external_mcp_servers.contains_key("filesystem"));
        assert!(adapter.external_mcp_servers.contains_key("github"));
    }

    /// Test that calling `set_external_mcp_servers` replaces the previous set
    /// entirely (i.e., it is a full replacement, not an incremental merge).
    #[test]
    fn set_external_mcp_servers_replaces_previous_set() {
        let mut adapter = RooInProcessAdapter::new();

        let mut first = std::collections::HashMap::new();
        first.insert(
            "old-server".to_string(),
            serde_json::json!({ "command": "old" }),
        );
        adapter.set_external_mcp_servers(first);
        assert_eq!(adapter.external_mcp_servers.len(), 1);

        let mut second = std::collections::HashMap::new();
        second.insert(
            "new-server".to_string(),
            serde_json::json!({ "command": "new" }),
        );
        adapter.set_external_mcp_servers(second);
        assert_eq!(adapter.external_mcp_servers.len(), 1);
        assert!(adapter.external_mcp_servers.contains_key("new-server"));
        assert!(!adapter.external_mcp_servers.contains_key("old-server"));
    }

    // ── Permission decision mapping ──────────────────────────────────

    /// Test that `PermissionDecision::Allow` is mapped to `approved = true`
    /// by the `resolve_permission` trait method.
    #[tokio::test]
    async fn resolve_permission_allow_resolves_true() {
        let mut adapter = RooInProcessAdapter::new();

        // Set up an approval handle with a real oneshot channel so we can
        // observe the value that resolve_roo_approval would send.
        let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
        let handle: Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<bool>>>> =
            Arc::new(std::sync::Mutex::new(Some(tx)));
        adapter.approval_handle = Some(handle);

        let result = adapter
            .resolve_permission("sid", "req-1", PermissionDecision::Allow)
            .await;
        assert!(
            result.is_ok(),
            "resolve_permission should succeed for Allow"
        );

        // The oneshot sender should have been consumed and sent `true`.
        let approved = rx.await.expect("receiver should get a value");
        assert!(approved, "Allow decision should resolve as true");
    }

    /// Test that `PermissionDecision::AllowAll` is also mapped to `approved = true`.
    #[tokio::test]
    async fn resolve_permission_allow_all_resolves_true() {
        let mut adapter = RooInProcessAdapter::new();

        let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
        let handle: Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<bool>>>> =
            Arc::new(std::sync::Mutex::new(Some(tx)));
        adapter.approval_handle = Some(handle);

        let result = adapter
            .resolve_permission("sid", "req-1", PermissionDecision::AllowAll)
            .await;
        assert!(
            result.is_ok(),
            "resolve_permission should succeed for AllowAll"
        );

        let approved = rx.await.expect("receiver should get a value");
        assert!(approved, "AllowAll decision should resolve as true");
    }

    /// Test that `PermissionDecision::Deny` is mapped to `approved = false`.
    #[tokio::test]
    async fn resolve_permission_deny_resolves_false() {
        let mut adapter = RooInProcessAdapter::new();

        let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
        let handle: Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<bool>>>> =
            Arc::new(std::sync::Mutex::new(Some(tx)));
        adapter.approval_handle = Some(handle);

        let result = adapter
            .resolve_permission("sid", "req-1", PermissionDecision::Deny)
            .await;
        assert!(result.is_ok(), "resolve_permission should succeed for Deny");

        let approved = rx.await.expect("receiver should get a value");
        assert!(!approved, "Deny decision should resolve as false");
    }

    #[tokio::test]
    async fn resolve_permission_api_retry_request_resolves_retry_bool() {
        let mut adapter = RooInProcessAdapter::new();

        let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
        adapter.api_retry_handle = Some(Arc::new(std::sync::Mutex::new(Some(tx))));

        let result = adapter
            .resolve_permission("sid", "api_retry:t1", PermissionDecision::Allow)
            .await;
        assert!(result.is_ok());

        let retry = rx.await.expect("receiver should get retry decision");
        assert!(retry);
    }

    #[tokio::test]
    async fn resolve_permission_auto_approval_limit_resolves_bool() {
        let mut adapter = RooInProcessAdapter::new();

        let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
        adapter.auto_approval_limit_handle = Some(Arc::new(std::sync::Mutex::new(Some(tx))));

        let result = adapter
            .resolve_permission(
                "sid",
                "auto_approval_limit:t1",
                PermissionDecision::AllowAll,
            )
            .await;
        assert!(result.is_ok());

        let approved = rx
            .await
            .expect("receiver should get auto-approval-limit decision");
        assert!(approved);
    }

    #[tokio::test]
    async fn send_message_resolves_pending_followup_text() {
        let mut adapter = RooInProcessAdapter::new();

        let (tx, rx) = tokio::sync::oneshot::channel::<String>();
        adapter.followup_handle = Some(Arc::new(std::sync::Mutex::new(Some(tx))));

        let mut events = adapter
            .send_message("sid", "Use TypeScript")
            .await
            .expect("send_message should resolve followup");
        assert!(matches!(
            events.recv().await,
            Some(UnifiedAgentEvent::Ready)
        ));

        let response = rx.await.expect("receiver should get followup text");
        assert_eq!(response, "Use TypeScript");
    }

    #[tokio::test]
    async fn resolve_permission_accepts_pending_completion_with_empty_feedback() {
        let mut adapter = RooInProcessAdapter::new();

        let (tx, rx) = tokio::sync::oneshot::channel::<String>();
        adapter.completion_handle = Some(Arc::new(std::sync::Mutex::new(Some(tx))));

        let result = adapter
            .resolve_permission("sid", "tool-2", PermissionDecision::Allow)
            .await;
        assert!(result.is_ok());

        let feedback = rx.await.expect("receiver should get completion feedback");
        assert_eq!(feedback, "");
    }

    #[tokio::test]
    async fn resolve_roo_permission_routes_followup_text_by_kind() {
        let mut adapter = RooInProcessAdapter::new();

        let (tx, rx) = tokio::sync::oneshot::channel::<String>();
        adapter.followup_handle = Some(Arc::new(std::sync::Mutex::new(Some(tx))));

        let result = adapter
            .resolve_roo_permission(
                "tool-followup",
                PermissionDecision::Allow,
                Some("Use Rust.".to_string()),
                Some("followup"),
            )
            .await;
        assert!(result.is_ok());

        let response = rx.await.expect("receiver should get followup text");
        assert_eq!(response, "Use Rust.");
    }

    #[tokio::test]
    async fn resolve_roo_permission_routes_completion_feedback_by_kind() {
        let mut adapter = RooInProcessAdapter::new();

        let (tx, rx) = tokio::sync::oneshot::channel::<String>();
        adapter.completion_handle = Some(Arc::new(std::sync::Mutex::new(Some(tx))));

        let result = adapter
            .resolve_roo_permission(
                "tool-completion",
                PermissionDecision::Deny,
                Some("Please add tests first.".to_string()),
                Some("completion_result"),
            )
            .await;
        assert!(result.is_ok());

        let feedback = rx.await.expect("receiver should get completion feedback");
        assert_eq!(feedback, "Please add tests first.");
    }

    #[tokio::test]
    async fn resolve_roo_permission_mistake_limit_preserves_feedback() {
        let mut adapter = RooInProcessAdapter::new();

        let (tx, rx) = tokio::sync::oneshot::channel::<MistakeLimitAction>();
        adapter.mistake_limit_handle = Some(Arc::new(std::sync::Mutex::new(Some(tx))));

        let result = adapter
            .resolve_roo_permission(
                "mistake_limit:t1",
                PermissionDecision::Allow,
                Some("Try the MCP tool again with JSON args.".to_string()),
                Some("mistake_limit_reached"),
            )
            .await;
        assert!(result.is_ok());

        let action = rx.await.expect("receiver should get mistake-limit action");
        match action {
            MistakeLimitAction::Continue { feedback } => {
                assert_eq!(
                    feedback.as_deref(),
                    Some("Try the MCP tool again with JSON args.")
                );
            }
            MistakeLimitAction::Cancel => panic!("expected continue action"),
        }
    }

    #[tokio::test]
    async fn resolve_permission_mistake_limit_deny_cancels_task() {
        let mut adapter = RooInProcessAdapter::new();

        let (tx, rx) = tokio::sync::oneshot::channel::<MistakeLimitAction>();
        adapter.mistake_limit_handle = Some(Arc::new(std::sync::Mutex::new(Some(tx))));

        let result = adapter
            .resolve_permission("sid", "mistake_limit:t1", PermissionDecision::Deny)
            .await;
        assert!(result.is_ok());

        let action = rx.await.expect("receiver should get mistake-limit action");
        assert!(matches!(action, MistakeLimitAction::Cancel));
    }

    /// Test that when no approval handle is set (None), resolve_permission
    /// still returns Ok (graceful no-op) rather than erroring.
    #[tokio::test]
    async fn resolve_permission_no_handle_returns_ok() {
        let mut adapter = RooInProcessAdapter::new();
        // approval_handle is None by default — no oneshot sender registered.

        let result = adapter
            .resolve_permission("sid", "req-1", PermissionDecision::Allow)
            .await;
        assert!(
            result.is_ok(),
            "resolve_permission should return Ok when no approval handle is set"
        );
    }

    /// Test that when the approval handle exists but the inner sender has
    /// already been consumed (taken), resolve_permission returns Ok gracefully
    /// instead of panicking or erroring.
    #[tokio::test]
    async fn resolve_permission_consumed_sender_returns_ok() {
        let mut adapter = RooInProcessAdapter::new();

        // Create a sender and immediately consume it by dropping the receiver.
        let (tx, _rx) = tokio::sync::oneshot::channel::<bool>();
        let handle: Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<bool>>>> =
            Arc::new(std::sync::Mutex::new(Some(tx)));

        // Take the sender out, simulating it was already consumed by a prior
        // approval resolution.
        {
            let mut guard = handle
                .lock()
                .expect("test mutex should be lockable before consuming sender");
            guard.take();
        }

        adapter.approval_handle = Some(handle);

        let result = adapter
            .resolve_permission("sid", "req-1", PermissionDecision::Deny)
            .await;
        assert!(
            result.is_ok(),
            "resolve_permission should return Ok even when sender was already consumed"
        );
    }
}
