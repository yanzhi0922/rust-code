//! Dynamic system prompt builder matching Claude Code's `constants/prompts.ts`.
//!
//! This crate provides a modular, section-based system prompt builder that
//! produces the same prompt structure as Claude Code's TypeScript implementation.
//!
//! # Architecture
//!
//! - [`SystemPromptBuilder`] - orchestrates section computation and cache management
//! - [`PromptContext`] - runtime context passed to each section
//! - [`SystemPromptSection`] - trait implemented by each prompt section
//! - [`SectionCache`] - in-memory cache for computed sections
//!
//! # Section Ordering
//!
//! The sections are ordered to match Claude Code's `getSystemPrompt()`:
//!
//! **Static (cacheable):**
//! 1. Intro
//! 2. System
//! 3. Doing Tasks
//! 4. Actions with Care
//! 5. Using Your Tools
//! 6. Tone and Style
//! 7. Output Efficiency
//!
//! **Boundary marker**
//!
//! **Dynamic (per-session):**
//! 8. Session-specific Guidance
//! 9. Memory
//! 10. Ant model override
//! 11. Environment Info
//! 12. Language
//! 13. Output Style
//! 14. MCP Instructions
//! 15. Scratchpad
//! 16. Function Result Clearing
//! 17. Summarize Tool Results

pub mod cache;
pub mod sections;
pub mod subagent;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::Result;
use once_cell::sync::Lazy;
use serde_json::json;

use cache::{SYSTEM_PROMPT_DYNAMIC_BOUNDARY, SectionCache};
use sections::SystemPromptSection;
use sections::actions::ActionsSection;
use sections::ant_model_override::AntModelOverrideSection;
use sections::brief::BriefSection;
use sections::doing_tasks::DoingTasksSection;
use sections::env_info::EnvInfoSection;
use sections::intro::IntroSection;
use sections::language::LanguageSection;
use sections::mcp_instructions::McpInstructionsSection;
use sections::memory::MemorySection;
use sections::numeric_length_anchors::NumericLengthAnchorsSection;
use sections::output_efficiency::OutputEfficiencySection;
use sections::output_style::OutputStyleSection;
use sections::proactive::ProactiveSection;
use sections::scratchpad::ScratchpadSection;
use sections::session_guidance::SessionGuidanceSection;
use sections::system::SystemSection;
use sections::system_reminders::SystemRemindersSection;
use sections::token_budget::TokenBudgetSection;
use sections::tone_style::ToneStyleSection;
use sections::tool_result::{FunctionResultClearingSection, ToolResultSection};
use sections::using_tools::UsingToolsSection;

pub const CLAUDE_CODE_DOCS_MAP_URL: &str =
    "https://code.claude.com/docs/en/claude_code_docs_map.md";

/// Configuration for a custom output style.
#[derive(Debug, Clone)]
pub struct OutputStyleConfig {
    /// Name of the output style.
    pub name: String,
    /// The prompt text describing the output style.
    pub prompt: String,
    /// Whether to keep the default coding instructions alongside this style.
    pub keep_coding_instructions: bool,
}

/// Information about a connected MCP server.
#[derive(Debug, Clone)]
pub struct McpClientInfo {
    /// Name of the MCP server.
    pub name: String,
    /// Optional instructions provided by the server.
    pub instructions: Option<String>,
}

/// Feature flags and runtime prompt toggles that affect the final prompt text.
#[derive(Debug, Clone, Default)]
pub struct PromptFeatures {
    /// Whether the runtime should match Anthropic's internal ant-only branches.
    pub ant_user: bool,
    /// Whether autonomous/proactive mode is active.
    pub proactive_active: bool,
    /// Whether brief mode is active.
    pub brief_enabled: bool,
    /// Whether the runtime is in REPL mode, which uses a narrower tool-guidance section.
    pub repl_mode_active: bool,
    /// Whether search is expected to happen through embedded shell aliases instead of Glob/Grep.
    pub embedded_search_tools: bool,
    /// Whether there are user-invocable skills available for `/skill-name` expansion.
    pub user_invocable_skills_available: bool,
    /// Whether built-in Explore/Plan search agents should be mentioned in prompt guidance.
    pub explore_plan_agents_enabled: bool,
    /// Whether verification-agent contract guidance is enabled for this runtime.
    pub verification_agent_enabled: bool,
    /// Exact memory prompt content, if the runtime resolved one.
    pub memory_prompt: Option<String>,
    /// Whether scratchpad prompting is enabled for this runtime.
    pub scratchpad_enabled: bool,
    /// Scratchpad directory path, when scratchpad guidance should be shown.
    pub scratchpad_dir: Option<String>,
    /// Keep-recent count for function-result clearing guidance.
    pub function_result_keep_recent: Option<usize>,
    /// Whether the token-budget guidance section is enabled.
    pub include_token_budget_prompt: bool,
    /// Whether to produce a minimal prompt matching CLAUDE_CODE_SIMPLE mode.
    pub simple_mode: bool,
}

/// Runtime context for system prompt section computation.
///
/// This struct carries all the information that sections need to decide
/// what content to include. It is constructed by the application layer
/// and passed to [`SystemPromptBuilder::build`].
#[derive(Debug, Clone)]
pub struct PromptContext {
    /// Model identifier (e.g. "claude-sonnet-4-6").
    pub model: String,
    /// Current working directory.
    pub cwd: PathBuf,
    /// Whether the cwd is inside a git repository.
    pub is_git: bool,
    /// Platform string (e.g. "linux", "darwin", "win32").
    pub platform: String,
    /// User's shell (e.g. "bash", "zsh").
    pub shell: String,
    /// OS version string (e.g. "Linux 6.6.4", "Darwin 25.3.0").
    pub os_version: String,
    /// Set of enabled tool names.
    pub enabled_tools: HashSet<String>,
    /// User's preferred response language.
    pub language: Option<String>,
    /// Custom output style configuration.
    pub output_style: Option<OutputStyleConfig>,
    /// Connected MCP server clients.
    pub mcp_clients: Vec<McpClientInfo>,
    /// When true, MCP instructions are carried in runtime delta messages rather
    /// than recomputed inside the system prompt each turn.
    pub mcp_instructions_delta_enabled: bool,
    /// Whether this is a git worktree session.
    pub is_worktree: bool,
    /// Additional working directories beyond cwd.
    pub additional_dirs: Vec<PathBuf>,
    /// Whether this is a non-interactive session.
    pub is_non_interactive: bool,
    /// Whether fork subagent mode is enabled.
    pub is_fork_subagent_enabled: bool,
    /// ISO 8601 date string for when the session started.
    pub session_start_date: String,
    /// Feature flags and mode toggles affecting prompt assembly.
    pub features: PromptFeatures,
    /// Whether undercover mode is active (suppresses model names/branding).
    pub is_undercover: bool,
}

/// Cache scope for a provider-facing system prompt block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheScope {
    /// Cacheable across requests when global prompt caching is allowed.
    Global,
    /// Cacheable without a global scope marker.
    Org,
}

/// Provider-facing system prompt block with resolved cache semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemPromptBlock {
    /// Text content of the block.
    pub text: String,
    /// Optional cache scope for this block.
    pub cache_scope: Option<CacheScope>,
}

/// Fully rendered provider-facing system prompt payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedSystemPrompt {
    /// Raw prompt blocks before provider serialization.
    pub raw_blocks: Vec<String>,
    /// Joined text form with empty blocks and boundary markers removed.
    pub text: String,
    /// Provider-facing content blocks with cache-control metadata applied.
    pub content_blocks: Vec<serde_json::Value>,
}

/// Inputs for applying Claude Code's effective-system-prompt precedence.
#[derive(Debug, Clone, Default)]
pub struct EffectiveSystemPromptOptions {
    /// Main-thread agent prompt. In normal mode this replaces the default
    /// system prompt; in proactive mode it is appended as custom agent
    /// instructions.
    pub agent_system_prompt: Option<String>,
    /// Coordinator-mode prompt. This takes precedence over agent/custom/default
    /// when coordinator mode is active and there is no main-thread agent.
    pub coordinator_system_prompt: Option<String>,
    /// User-specified custom system prompt (`--system-prompt`).
    pub custom_system_prompt: Option<String>,
    /// User-specified append system prompt (`--append-system-prompt`).
    pub append_system_prompt: Option<String>,
    /// Override prompt used by loop/override modes. Replaces everything,
    /// including append-system-prompt.
    pub override_system_prompt: Option<String>,
    /// Whether the proactive/Kairos prompt path is active.
    pub proactive_active: bool,
}

/// Apply the same precedence as Claude Code's `buildEffectiveSystemPrompt()`.
#[must_use]
pub fn build_effective_system_prompt(
    default_system_prompt: Vec<String>,
    options: &EffectiveSystemPromptOptions,
) -> Vec<String> {
    if let Some(override_prompt) = non_empty(options.override_system_prompt.as_deref()) {
        return vec![override_prompt.to_owned()];
    }

    if let Some(coordinator_prompt) = non_empty(options.coordinator_system_prompt.as_deref())
        && options
            .agent_system_prompt
            .as_deref()
            .is_none_or(str::is_empty)
    {
        return append_if_present(vec![coordinator_prompt.to_owned()], options);
    }

    if let Some(agent_prompt) = non_empty(options.agent_system_prompt.as_deref()) {
        if options.proactive_active {
            let mut prompt = default_system_prompt;
            prompt.push(format!("\n# Custom Agent Instructions\n{agent_prompt}"));
            return append_if_present(prompt, options);
        }
        return append_if_present(vec![agent_prompt.to_owned()], options);
    }

    if let Some(custom_prompt) = non_empty(options.custom_system_prompt.as_deref()) {
        return append_if_present(vec![custom_prompt.to_owned()], options);
    }

    append_if_present(default_system_prompt, options)
}

fn append_if_present(
    mut prompt: Vec<String>,
    options: &EffectiveSystemPromptOptions,
) -> Vec<String> {
    if let Some(append_prompt) = non_empty(options.append_system_prompt.as_deref()) {
        prompt.push(append_prompt.to_owned());
    }
    prompt
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
}

/// Options controlling how the raw system prompt is split for API transport.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SystemPromptSplitOptions {
    /// When true, collapse the prompt into org-scoped blocks instead of using
    /// the static/dynamic boundary for global cache scope.
    pub skip_global_cache_for_system_prompt: bool,
}

/// Split raw system prompt blocks into provider-facing blocks with cache scope.
#[must_use]
pub fn split_system_prompt_for_api(
    raw_blocks: &[String],
    options: &SystemPromptSplitOptions,
) -> Vec<SystemPromptBlock> {
    let join_blocks = |blocks: &[String]| {
        blocks
            .iter()
            .map(String::as_str)
            .filter(|block| {
                let trimmed = block.trim();
                !trimmed.is_empty() && trimmed != SYSTEM_PROMPT_DYNAMIC_BOUNDARY
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    };

    if options.skip_global_cache_for_system_prompt {
        let joined = join_blocks(raw_blocks);
        return (!joined.is_empty())
            .then_some(SystemPromptBlock {
                text: joined,
                cache_scope: Some(CacheScope::Org),
            })
            .into_iter()
            .collect();
    }

    if let Some(boundary_index) = raw_blocks
        .iter()
        .position(|block| block == SYSTEM_PROMPT_DYNAMIC_BOUNDARY)
    {
        let static_joined = join_blocks(&raw_blocks[..boundary_index]);
        let dynamic_joined = join_blocks(&raw_blocks[boundary_index + 1..]);
        let mut result = Vec::new();
        if !static_joined.is_empty() {
            result.push(SystemPromptBlock {
                text: static_joined,
                cache_scope: Some(CacheScope::Global),
            });
        }
        if !dynamic_joined.is_empty() {
            result.push(SystemPromptBlock {
                text: dynamic_joined,
                cache_scope: None,
            });
        }
        return result;
    }

    let joined = join_blocks(raw_blocks);
    (!joined.is_empty())
        .then_some(SystemPromptBlock {
            text: joined,
            cache_scope: Some(CacheScope::Org),
        })
        .into_iter()
        .collect()
}

/// Render raw prompt blocks into provider-facing content blocks and text.
#[must_use]
pub fn render_system_prompt_for_api(
    raw_blocks: &[String],
    options: &SystemPromptSplitOptions,
) -> RenderedSystemPrompt {
    let content_blocks = split_system_prompt_for_api(raw_blocks, options)
        .into_iter()
        .map(|block| {
            let mut content_block = json!({
                "type": "text",
                "text": block.text,
            });
            match block.cache_scope {
                Some(CacheScope::Global) => {
                    content_block["cache_control"] =
                        json!({"type": "ephemeral", "scope": "global"});
                }
                Some(CacheScope::Org) => {
                    content_block["cache_control"] = json!({"type": "ephemeral"});
                }
                None => {}
            }
            content_block
        })
        .collect::<Vec<_>>();

    let text = raw_blocks
        .iter()
        .filter_map(|block| {
            let trimmed = block.trim();
            (!trimmed.is_empty() && trimmed != SYSTEM_PROMPT_DYNAMIC_BOUNDARY)
                .then_some(block.clone())
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    RenderedSystemPrompt {
        raw_blocks: raw_blocks.to_vec(),
        text,
        content_blocks,
    }
}

/// Main system prompt builder.
///
/// Orchestrates the computation of static and dynamic sections,
/// manages caching, and produces the final prompt string array.
pub struct SystemPromptBuilder {
    static_sections: Vec<Box<dyn SystemPromptSection>>,
    dynamic_sections: Vec<Box<dyn SystemPromptSection>>,
    cache: SectionCache,
    use_global_cache_scope: bool,
}

static SESSION_SECTION_CACHES: Lazy<Mutex<HashMap<uuid::Uuid, SectionCache>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

impl SystemPromptBuilder {
    /// Create a new builder with no sections.
    #[must_use]
    pub fn new() -> Self {
        Self {
            static_sections: Vec::new(),
            dynamic_sections: Vec::new(),
            cache: SectionCache::new(),
            use_global_cache_scope: true,
        }
    }

    /// Create a builder pre-loaded with all default sections in the correct order.
    ///
    /// The section ordering matches Claude Code's `getSystemPrompt()`.
    #[must_use]
    pub fn with_default_sections() -> Self {
        let mut builder = Self::new();

        // Static sections (before the boundary marker)
        builder.static_sections.push(Box::new(IntroSection));
        builder.static_sections.push(Box::new(SystemSection));
        builder.static_sections.push(Box::new(DoingTasksSection));
        builder.static_sections.push(Box::new(ActionsSection));
        builder.static_sections.push(Box::new(UsingToolsSection));
        builder.static_sections.push(Box::new(ToneStyleSection));
        builder
            .static_sections
            .push(Box::new(OutputEfficiencySection));

        // Dynamic sections (after the boundary marker)
        builder
            .dynamic_sections
            .push(Box::new(SessionGuidanceSection));
        builder.dynamic_sections.push(Box::new(MemorySection));
        builder
            .dynamic_sections
            .push(Box::new(AntModelOverrideSection));
        builder.dynamic_sections.push(Box::new(EnvInfoSection));
        builder.dynamic_sections.push(Box::new(LanguageSection));
        builder.dynamic_sections.push(Box::new(OutputStyleSection));
        builder
            .dynamic_sections
            .push(Box::new(McpInstructionsSection));
        builder.dynamic_sections.push(Box::new(ScratchpadSection));
        builder
            .dynamic_sections
            .push(Box::new(FunctionResultClearingSection));
        builder.dynamic_sections.push(Box::new(ToolResultSection));
        builder
            .dynamic_sections
            .push(Box::new(NumericLengthAnchorsSection));
        builder.dynamic_sections.push(Box::new(TokenBudgetSection));
        builder.dynamic_sections.push(Box::new(BriefSection));

        builder
    }

    /// Add a custom static section.
    pub fn add_static_section(&mut self, section: Box<dyn SystemPromptSection>) {
        self.static_sections.push(section);
    }

    /// Add a custom dynamic section.
    pub fn add_dynamic_section(&mut self, section: Box<dyn SystemPromptSection>) {
        self.dynamic_sections.push(section);
    }

    /// Set whether to use global cache scope (include the boundary marker).
    pub fn set_global_cache_scope(&mut self, enabled: bool) {
        self.use_global_cache_scope = enabled;
    }

    /// Build the complete system prompt.
    ///
    /// Returns a vector of strings representing the system prompt blocks.
    /// The boundary marker [`SYSTEM_PROMPT_DYNAMIC_BOUNDARY`] separates static
    /// from dynamic content (if global cache scope is enabled).
    pub fn build(&mut self, ctx: &PromptContext) -> Result<Vec<String>> {
        if ctx.features.simple_mode {
            return Ok(vec![format!(
                "You are Claude Code, Anthropic's official CLI for Claude.\n\nCWD: {}\nDate: {}",
                ctx.cwd.display(),
                ctx.session_start_date
            )]);
        }

        if ctx.features.proactive_active {
            return self.build_proactive(ctx);
        }

        let mut result = Vec::new();

        // Compute static sections
        for section in &self.static_sections {
            let name = section.name().to_string();
            let content = if section.is_cacheable() {
                if let Some(cached) = self.cache.get(&name) {
                    cached.clone()
                } else {
                    let computed = section.compute(ctx)?;
                    self.cache.set(&name, computed.clone());
                    computed
                }
            } else {
                section.compute(ctx)?
            };

            if let Some(text) = content {
                result.push(text);
            }
        }

        // Insert boundary marker
        if self.use_global_cache_scope {
            result.push(SYSTEM_PROMPT_DYNAMIC_BOUNDARY.to_string());
        }

        // Compute dynamic sections
        for section in &self.dynamic_sections {
            let name = section.name().to_string();
            let content = if section.is_cacheable() {
                if let Some(cached) = self.cache.get(&name) {
                    cached.clone()
                } else {
                    let computed = section.compute(ctx)?;
                    self.cache.set(&name, computed.clone());
                    computed
                }
            } else {
                section.compute(ctx)?
            };

            if let Some(text) = content {
                result.push(text);
            }
        }

        Ok(result)
    }

    fn build_proactive(&mut self, ctx: &PromptContext) -> Result<Vec<String>> {
        let mut result = vec![format!(
            "\nYou are an autonomous agent. Use the available tools to do useful work.\n\n{}",
            sections::intro::CYBER_RISK_INSTRUCTION
        )];

        for section in [
            &SystemRemindersSection as &dyn SystemPromptSection,
            &MemorySection,
            &EnvInfoSection,
            &LanguageSection,
            &McpInstructionsSection,
            &ScratchpadSection,
            &FunctionResultClearingSection,
            &ToolResultSection,
            &ProactiveSection,
        ] {
            if let Some(text) = section.compute(ctx)? {
                result.push(text);
            }
        }

        Ok(result)
    }

    /// Clear all cached section values.
    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }

    /// Get the number of static sections.
    #[must_use]
    pub fn static_section_count(&self) -> usize {
        self.static_sections.len()
    }

    /// Get the number of dynamic sections.
    #[must_use]
    pub fn dynamic_section_count(&self) -> usize {
        self.dynamic_sections.len()
    }
}

impl Default for SystemPromptBuilder {
    fn default() -> Self {
        Self::new()
    }
}

fn resolve_sections(
    cache: &mut SectionCache,
    ctx: &PromptContext,
    sections: &[&dyn SystemPromptSection],
) -> Result<Vec<String>> {
    let mut resolved = Vec::new();
    for section in sections {
        let name = section.name();
        let content = if section.is_cacheable() {
            if let Some(cached) = cache.get(name) {
                cached.clone()
            } else {
                let computed = section.compute(ctx)?;
                cache.set(name, computed.clone());
                computed
            }
        } else {
            section.compute(ctx)?
        };

        if let Some(text) = content {
            resolved.push(text);
        }
    }
    Ok(resolved)
}

fn resolve_default_prompt_blocks(
    cache: &mut SectionCache,
    ctx: &PromptContext,
    use_global_cache_scope: bool,
) -> Result<Vec<String>> {
    if ctx.features.proactive_active {
        let proactive_sections: [&dyn SystemPromptSection; 9] = [
            &SystemRemindersSection,
            &MemorySection,
            &EnvInfoSection,
            &LanguageSection,
            &McpInstructionsSection,
            &ScratchpadSection,
            &FunctionResultClearingSection,
            &ToolResultSection,
            &ProactiveSection,
        ];
        let mut result = vec![format!(
            "\nYou are an autonomous agent. Use the available tools to do useful work.\n\n{}",
            sections::intro::CYBER_RISK_INSTRUCTION
        )];
        result.extend(resolve_sections(cache, ctx, &proactive_sections)?);
        return Ok(result);
    }

    let static_sections: [&dyn SystemPromptSection; 7] = [
        &IntroSection,
        &SystemSection,
        &DoingTasksSection,
        &ActionsSection,
        &UsingToolsSection,
        &ToneStyleSection,
        &OutputEfficiencySection,
    ];
    let dynamic_sections: [&dyn SystemPromptSection; 13] = [
        &SessionGuidanceSection,
        &MemorySection,
        &AntModelOverrideSection,
        &EnvInfoSection,
        &LanguageSection,
        &OutputStyleSection,
        &McpInstructionsSection,
        &ScratchpadSection,
        &FunctionResultClearingSection,
        &ToolResultSection,
        &NumericLengthAnchorsSection,
        &TokenBudgetSection,
        &BriefSection,
    ];

    let mut result = resolve_sections(cache, ctx, &static_sections)?;
    if use_global_cache_scope {
        result.push(SYSTEM_PROMPT_DYNAMIC_BOUNDARY.to_owned());
    }
    result.extend(resolve_sections(cache, ctx, &dynamic_sections)?);
    Ok(result)
}

pub use subagent::{DEFAULT_AGENT_PROMPT, enhance_system_prompt_with_env_details};

pub fn build_default_system_prompt_for_session(
    session_id: uuid::Uuid,
    ctx: &PromptContext,
    use_global_cache_scope: bool,
) -> Result<Vec<String>> {
    let mut caches = SESSION_SECTION_CACHES
        .lock()
        .expect("system prompt section cache poisoned");
    let cache = caches.entry(session_id).or_default();
    resolve_default_prompt_blocks(cache, ctx, use_global_cache_scope)
}

pub fn clear_system_prompt_sections_for_session(session_id: uuid::Uuid) {
    if let Ok(mut caches) = SESSION_SECTION_CACHES.lock() {
        caches.remove(&session_id);
    }
}

/// Create a minimal prompt context for testing purposes.
#[cfg(test)]
pub fn test_prompt_context() -> PromptContext {
    PromptContext {
        model: "claude-sonnet-4-6".to_string(),
        cwd: PathBuf::from("/home/user/project"),
        is_git: true,
        platform: "linux".to_string(),
        shell: "bash".to_string(),
        os_version: "Linux 6.6.4".to_string(),
        enabled_tools: HashSet::new(),
        language: None,
        output_style: None,
        mcp_clients: vec![],
        mcp_instructions_delta_enabled: false,
        is_worktree: false,
        additional_dirs: vec![],
        is_non_interactive: false,
        is_fork_subagent_enabled: false,
        session_start_date: "2025-01-01".to_string(),
        features: PromptFeatures::default(),
        is_undercover: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_default_has_no_sections() {
        let builder = SystemPromptBuilder::new();
        assert_eq!(builder.static_section_count(), 0);
        assert_eq!(builder.dynamic_section_count(), 0);
    }

    #[test]
    fn builder_with_defaults_has_sections() {
        let builder = SystemPromptBuilder::with_default_sections();
        assert_eq!(builder.static_section_count(), 7);
        assert_eq!(builder.dynamic_section_count(), 13);
    }

    #[test]
    fn build_produces_non_empty_prompt() {
        let mut builder = SystemPromptBuilder::with_default_sections();
        let ctx = test_prompt_context();
        let result = builder.build(&ctx).expect("build should succeed");
        assert!(!result.is_empty());
    }

    #[test]
    fn build_contains_boundary_marker() {
        let mut builder = SystemPromptBuilder::with_default_sections();
        let ctx = test_prompt_context();
        let result = builder.build(&ctx).expect("build should succeed");
        assert!(result.contains(&SYSTEM_PROMPT_DYNAMIC_BOUNDARY.to_string()));
    }

    #[test]
    fn build_boundary_comes_after_static_sections() {
        let mut builder = SystemPromptBuilder::with_default_sections();
        let ctx = test_prompt_context();
        let result = builder.build(&ctx).expect("build should succeed");

        let boundary_idx = result
            .iter()
            .position(|s| s == SYSTEM_PROMPT_DYNAMIC_BOUNDARY)
            .expect("boundary should exist");

        // Static content before boundary
        for (i, section) in result.iter().enumerate().take(boundary_idx) {
            assert!(
                !section.is_empty(),
                "Static section {i} should not be empty"
            );
        }

        // The first static section should be the intro
        assert!(
            result[0].contains("You are an interactive agent"),
            "First section should be intro"
        );
    }

    #[test]
    fn build_without_global_cache_has_no_boundary() {
        let mut builder = SystemPromptBuilder::with_default_sections();
        builder.set_global_cache_scope(false);
        let ctx = test_prompt_context();
        let result = builder.build(&ctx).expect("build should succeed");
        assert!(!result.contains(&SYSTEM_PROMPT_DYNAMIC_BOUNDARY.to_string()));
    }

    #[test]
    fn clear_cache_works() {
        let mut builder = SystemPromptBuilder::with_default_sections();
        let ctx = test_prompt_context();
        let _ = builder.build(&ctx);
        builder.clear_cache();
        // After clearing, a new build should still work
        let result = builder
            .build(&ctx)
            .expect("build after clear should succeed");
        assert!(!result.is_empty());
    }

    #[test]
    fn static_section_ordering_matches_claude_code() {
        let builder = SystemPromptBuilder::with_default_sections();
        let names: Vec<&str> = builder.static_sections.iter().map(|s| s.name()).collect();
        assert_eq!(
            names,
            vec![
                "intro",
                "system",
                "doing_tasks",
                "actions",
                "using_tools",
                "tone_style",
                "output_efficiency"
            ]
        );
    }

    #[test]
    fn dynamic_section_ordering_matches_claude_code() {
        let builder = SystemPromptBuilder::with_default_sections();
        let names: Vec<&str> = builder.dynamic_sections.iter().map(|s| s.name()).collect();
        assert_eq!(
            names,
            vec![
                "session_guidance",
                "memory",
                "ant_model_override",
                "env_info_simple",
                "language",
                "output_style",
                "mcp_instructions",
                "scratchpad",
                "frc",
                "summarize_tool_results",
                "numeric_length_anchors",
                "token_budget",
                "brief"
            ]
        );
    }

    #[test]
    fn full_prompt_contains_expected_sections() {
        let mut builder = SystemPromptBuilder::with_default_sections();
        let ctx = test_prompt_context();
        let result = builder.build(&ctx).expect("build should succeed");
        let combined = result.join("\n---\n");

        // Static sections
        assert!(combined.contains("You are an interactive agent"), "intro");
        assert!(combined.contains("# System"), "system");
        assert!(combined.contains("# Doing tasks"), "doing_tasks");
        assert!(
            combined.contains("# Executing actions with care"),
            "actions"
        );
        assert!(combined.contains("# Using your tools"), "using_tools");
        assert!(combined.contains("# Tone and style"), "tone_style");
        assert!(
            combined.contains("# Output efficiency"),
            "output_efficiency"
        );

        // Dynamic sections (env_info always present)
        assert!(combined.contains("# Environment"), "env_info");
    }

    #[test]
    fn proactive_build_uses_autonomous_prompt_shape() {
        let mut builder = SystemPromptBuilder::with_default_sections();
        let mut ctx = test_prompt_context();
        ctx.features.proactive_active = true;
        let result = builder.build(&ctx).expect("build should succeed");
        let combined = result.join("\n---\n");

        assert!(combined.contains("You are an autonomous agent."));
        assert!(
            combined
                .contains("- Tool results and user messages may include <system-reminder> tags.")
        );
        assert!(combined.contains("# Autonomous work"));
        assert!(!combined.contains(SYSTEM_PROMPT_DYNAMIC_BOUNDARY));
    }

    #[test]
    fn proactive_with_brief_contains_brief_guidance_no_boundary() {
        let mut builder = SystemPromptBuilder::with_default_sections();
        let mut ctx = test_prompt_context();
        ctx.features.proactive_active = true;
        ctx.features.brief_enabled = true;
        let result = builder.build(&ctx).expect("build should succeed");
        let combined = result.join("\n---\n");

        assert!(combined.contains("## Talking to the user"));
        assert!(combined.contains("SendUserMessage is where your replies go"));
        assert!(!combined.contains(SYSTEM_PROMPT_DYNAMIC_BOUNDARY));
    }

    #[test]
    fn effective_system_prompt_override_replaces_everything() {
        let prompt = build_effective_system_prompt(
            vec!["default".to_owned()],
            &EffectiveSystemPromptOptions {
                override_system_prompt: Some("override".to_owned()),
                append_system_prompt: Some("append".to_owned()),
                ..Default::default()
            },
        );
        assert_eq!(prompt, vec!["override".to_owned()]);
    }

    #[test]
    fn effective_system_prompt_agent_replaces_default() {
        let prompt = build_effective_system_prompt(
            vec!["default".to_owned()],
            &EffectiveSystemPromptOptions {
                agent_system_prompt: Some("agent".to_owned()),
                append_system_prompt: Some("append".to_owned()),
                ..Default::default()
            },
        );
        assert_eq!(prompt, vec!["agent".to_owned(), "append".to_owned()]);
    }

    #[test]
    fn effective_system_prompt_proactive_agent_appends_to_default() {
        let prompt = build_effective_system_prompt(
            vec!["default".to_owned()],
            &EffectiveSystemPromptOptions {
                agent_system_prompt: Some("agent".to_owned()),
                append_system_prompt: Some("append".to_owned()),
                proactive_active: true,
                ..Default::default()
            },
        );
        assert_eq!(
            prompt,
            vec![
                "default".to_owned(),
                "\n# Custom Agent Instructions\nagent".to_owned(),
                "append".to_owned()
            ]
        );
    }

    #[test]
    fn effective_system_prompt_custom_replaces_default_before_append() {
        let prompt = build_effective_system_prompt(
            vec!["default".to_owned()],
            &EffectiveSystemPromptOptions {
                custom_system_prompt: Some("custom".to_owned()),
                append_system_prompt: Some("append".to_owned()),
                ..Default::default()
            },
        );
        assert_eq!(prompt, vec!["custom".to_owned(), "append".to_owned()]);
    }

    #[test]
    fn conditional_language_section() {
        let mut builder = SystemPromptBuilder::with_default_sections();
        let mut ctx = test_prompt_context();
        ctx.language = Some("Japanese".to_string());
        let result = builder.build(&ctx).expect("build should succeed");
        let combined = result.join("\n---\n");
        assert!(combined.contains("# Language"));
        assert!(combined.contains("Japanese"));
    }

    #[test]
    fn conditional_language_section_absent() {
        let mut builder = SystemPromptBuilder::with_default_sections();
        let ctx = test_prompt_context();
        let result = builder.build(&ctx).expect("build should succeed");
        let combined = result.join("\n---\n");
        assert!(!combined.contains("# Language"));
    }

    #[test]
    fn conditional_output_style_section() {
        let mut builder = SystemPromptBuilder::with_default_sections();
        let mut ctx = test_prompt_context();
        ctx.output_style = Some(OutputStyleConfig {
            name: "Concise".to_string(),
            prompt: "Be brief.".to_string(),
            keep_coding_instructions: true,
        });
        let result = builder.build(&ctx).expect("build should succeed");
        let combined = result.join("\n---\n");
        assert!(combined.contains("# Output Style: Concise"));
    }

    #[test]
    fn conditional_mcp_section() {
        let mut builder = SystemPromptBuilder::with_default_sections();
        let mut ctx = test_prompt_context();
        ctx.mcp_clients = vec![McpClientInfo {
            name: "test-mcp".to_string(),
            instructions: Some("Use tools carefully.".to_string()),
        }];
        let result = builder.build(&ctx).expect("build should succeed");
        let combined = result.join("\n---\n");
        assert!(combined.contains("# MCP Server Instructions"));
        assert!(combined.contains("test-mcp"));
    }

    #[test]
    fn add_custom_section() {
        let mut builder = SystemPromptBuilder::new();
        builder.add_static_section(Box::new(IntroSection));
        assert_eq!(builder.static_section_count(), 1);
        let ctx = test_prompt_context();
        let result = builder.build(&ctx).expect("build should succeed");
        assert!(result.len() >= 2); // at least intro + boundary
    }

    #[test]
    fn prompt_context_default_values() {
        let ctx = test_prompt_context();
        assert_eq!(ctx.model, "claude-sonnet-4-6");
        assert!(ctx.is_git);
        assert!(ctx.language.is_none());
        assert!(ctx.output_style.is_none());
        assert!(ctx.mcp_clients.is_empty());
        assert!(!ctx.is_non_interactive);
    }

    #[test]
    fn split_system_prompt_for_api_uses_global_boundary_when_present() {
        let raw = vec![
            "static one".to_owned(),
            "static two".to_owned(),
            SYSTEM_PROMPT_DYNAMIC_BOUNDARY.to_owned(),
            "dynamic one".to_owned(),
            "dynamic two".to_owned(),
        ];

        let split = split_system_prompt_for_api(&raw, &SystemPromptSplitOptions::default());
        assert_eq!(split.len(), 2);
        assert_eq!(split[0].cache_scope, Some(CacheScope::Global));
        assert_eq!(split[0].text, "static one\n\nstatic two");
        assert_eq!(split[1].cache_scope, None);
        assert_eq!(split[1].text, "dynamic one\n\ndynamic two");
    }

    #[test]
    fn split_system_prompt_for_api_can_skip_global_cache_scope() {
        let raw = vec![
            "static one".to_owned(),
            SYSTEM_PROMPT_DYNAMIC_BOUNDARY.to_owned(),
            "dynamic one".to_owned(),
        ];

        let split = split_system_prompt_for_api(
            &raw,
            &SystemPromptSplitOptions {
                skip_global_cache_for_system_prompt: true,
            },
        );
        assert_eq!(split.len(), 1);
        assert_eq!(split[0].cache_scope, Some(CacheScope::Org));
        assert_eq!(split[0].text, "static one\n\ndynamic one");
    }

    #[test]
    fn split_system_prompt_for_api_falls_back_to_org_without_boundary() {
        let raw = vec!["one".to_owned(), "two".to_owned()];

        let split = split_system_prompt_for_api(&raw, &SystemPromptSplitOptions::default());
        assert_eq!(split.len(), 1);
        assert_eq!(split[0].cache_scope, Some(CacheScope::Org));
        assert_eq!(split[0].text, "one\n\ntwo");
    }

    #[test]
    fn render_system_prompt_for_api_applies_cache_control_metadata() {
        let raw = vec![
            "static one".to_owned(),
            SYSTEM_PROMPT_DYNAMIC_BOUNDARY.to_owned(),
            "dynamic one".to_owned(),
        ];

        let rendered = render_system_prompt_for_api(&raw, &SystemPromptSplitOptions::default());
        assert_eq!(rendered.text, "static one\n\ndynamic one");
        assert_eq!(rendered.content_blocks.len(), 2);
        assert_eq!(
            rendered.content_blocks[0]["cache_control"],
            serde_json::json!({"type": "ephemeral", "scope": "global"})
        );
        assert!(rendered.content_blocks[1].get("cache_control").is_none());
    }

    #[test]
    fn render_system_prompt_for_api_uses_org_cache_when_global_skipped() {
        let raw = vec![
            "static one".to_owned(),
            SYSTEM_PROMPT_DYNAMIC_BOUNDARY.to_owned(),
            "dynamic one".to_owned(),
        ];

        let rendered = render_system_prompt_for_api(
            &raw,
            &SystemPromptSplitOptions {
                skip_global_cache_for_system_prompt: true,
            },
        );
        assert_eq!(rendered.content_blocks.len(), 1);
        assert_eq!(
            rendered.content_blocks[0]["cache_control"],
            serde_json::json!({"type": "ephemeral"})
        );
    }

    #[test]
    fn render_system_prompt_for_api_omits_empty_blocks_from_joined_text() {
        let raw = vec![
            String::new(),
            "   ".to_owned(),
            "one".to_owned(),
            SYSTEM_PROMPT_DYNAMIC_BOUNDARY.to_owned(),
            "two".to_owned(),
        ];

        let rendered = render_system_prompt_for_api(&raw, &SystemPromptSplitOptions::default());
        assert_eq!(rendered.text, "one\n\ntwo");
    }

    #[test]
    fn simple_mode_returns_minimal_prompt() {
        let mut builder = SystemPromptBuilder::with_default_sections();
        let mut ctx = test_prompt_context();
        ctx.features.simple_mode = true;
        let result = builder.build(&ctx).expect("build should succeed");
        assert_eq!(result.len(), 1);
        assert!(result[0].contains("You are Claude Code"));
        assert!(result[0].contains("CWD:"));
        assert!(result[0].contains("Date:"));
        assert!(!result[0].contains("# System"));
    }

    #[test]
    fn docs_map_url_constant() {
        assert!(CLAUDE_CODE_DOCS_MAP_URL.starts_with("https://"));
        assert!(CLAUDE_CODE_DOCS_MAP_URL.contains("docs_map"));
    }

    #[test]
    fn default_agent_prompt_is_exported() {
        assert!(!DEFAULT_AGENT_PROMPT.is_empty());
        assert!(DEFAULT_AGENT_PROMPT.contains("agent for Claude Code"));
    }

    #[test]
    fn enhance_system_prompt_is_callable() {
        let result = enhance_system_prompt_with_env_details(
            vec!["test".to_string()],
            &test_prompt_context(),
        )
        .expect("should succeed");
        assert!(result.len() > 1);
        assert!(result.last().unwrap().contains("<env>"));
    }
}
