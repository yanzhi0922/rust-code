//! Agent execution runner matching Claude Code's `AgentTool/runAgent.ts`.
//!
//! The [`AgentRunner`] orchestrates agent execution: resolving tools, building
//! the system prompt, and tracking turns and usage.
//!
//! # Enhanced Functions
//!
//! - [`enhance_system_prompt_with_env_details`] — Inject environment info into prompts
//! - [`resolve_effective_tools`] — Resolve tool set with wildcard/denylist support
//! - [`aggregate_run_results`] — Aggregate multiple run results

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use claude_core::PermissionMode;
use serde::{Deserialize, Serialize};

use crate::definition::AgentDefinition;
use crate::memory::append_memory_prompt_to_system_prompt;

/// Configuration for a single agent run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRunConfig {
    /// Maximum number of agentic turns before stopping.
    pub max_turns: u32,
    /// Model to use for this run.
    pub model: String,
    /// Tools available to the agent.
    pub tools: Vec<String>,
    /// Optional system prompt override.
    pub system_prompt: Option<String>,
    /// Working directory for the agent.
    pub working_dir: PathBuf,
    /// Additional working directories visible to the agent.
    #[serde(default)]
    pub additional_working_directories: Vec<PathBuf>,
}

/// Summary of token usage from an agent run.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageSummary {
    /// Total input tokens consumed.
    pub input_tokens: u64,
    /// Total output tokens generated.
    pub output_tokens: u64,
    /// Tokens written to cache.
    pub cache_creation_tokens: u64,
    /// Tokens read from cache.
    pub cache_read_tokens: u64,
}

/// Result of an agent run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRunResult {
    /// The agent's final output text.
    pub output: String,
    /// Whether the run completed successfully.
    pub success: bool,
    /// Number of turns completed.
    pub turns: u32,
    /// Token usage summary.
    pub usage: UsageSummary,
}

/// A simplified conversation entry for providing context to an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationEntry {
    /// Role of the message sender.
    pub role: String,
    /// Text content of the message.
    pub content: String,
}

/// Fully-resolved request for a concrete agent runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentExecutionRequest {
    /// Agent type identifier.
    pub agent_type: String,
    /// Optional teammate name.
    pub agent_name: Option<String>,
    /// Optional team name.
    pub team_name: Option<String>,
    /// Task prompt to execute.
    pub task: String,
    /// Conversation context inherited from the caller.
    pub context: Vec<ConversationEntry>,
    /// Resolved model name.
    pub model: String,
    /// Resolved maximum turn count.
    pub max_turns: u32,
    /// Resolved system prompt.
    pub system_prompt: String,
    /// Short critical reminder reinjected as a system-reminder user message.
    #[serde(default)]
    pub critical_system_reminder: Option<String>,
    /// Omit CLAUDE.md-derived user context for this child run.
    #[serde(default)]
    pub omit_claude_md: bool,
    /// Omit gitStatus from the child system context.
    #[serde(default)]
    pub omit_git_status: bool,
    /// Resolved tool set available to the agent.
    pub tools: Vec<String>,
    /// Permission mode to use for the run.
    pub permission_mode: Option<PermissionMode>,
    /// Working directory for the run.
    pub working_dir: PathBuf,
    /// Additional working directories available to the child runtime.
    #[serde(default)]
    pub additional_working_directories: Vec<PathBuf>,
    /// Run the child without writing transcript/session artifacts to the parent profile.
    #[serde(default)]
    pub skip_transcript: bool,
}

/// Concrete host runtime capable of executing an agent request.
#[async_trait]
pub trait AgentExecutor: Send + Sync {
    /// Execute the provided request and return the run result.
    async fn execute(&self, request: AgentExecutionRequest) -> Result<AgentRunResult>;
}

/// The agent execution runner.
///
/// Orchestrates the execution of an agent according to its definition and
/// configuration. Resolves the effective tool set, builds the system prompt,
/// and tracks execution state.
#[derive(Debug, Clone)]
pub struct AgentRunner {
    /// The agent definition being run.
    definition: AgentDefinition,
    /// Run-specific configuration.
    config: AgentRunConfig,
}

impl AgentRunner {
    /// Create a new runner for the given agent definition and configuration.
    pub fn new(definition: AgentDefinition, config: AgentRunConfig) -> Self {
        Self { definition, config }
    }

    /// Get a reference to the agent definition.
    pub fn definition(&self) -> &AgentDefinition {
        &self.definition
    }

    /// Get a reference to the run configuration.
    pub fn config(&self) -> &AgentRunConfig {
        &self.config
    }

    /// Resolve the effective tool set for this agent run.
    ///
    /// If the agent has an allowlist (`tools`), use that (filtered by denylist).
    /// If the agent has only a denylist, start with all tools and exclude.
    /// Otherwise, all tools are available.
    pub fn resolve_tools(&self, available_tools: &[String]) -> Vec<String> {
        if self.definition.has_tool_allowlist() {
            let deny_set: std::collections::HashSet<&str> = self
                .definition
                .disallowed_tools
                .iter()
                .map(|s| s.as_str())
                .collect();
            // Check for wildcard
            if self.definition.tools.contains(&"*".to_owned()) {
                return available_tools
                    .iter()
                    .filter(|tool| !deny_set.contains(tool.as_str()))
                    .cloned()
                    .collect();
            }
            // Filter allowlist by denylist
            self.definition
                .tools
                .iter()
                .filter(|t| !deny_set.contains(t.as_str()))
                .cloned()
                .collect()
        } else if self.definition.has_tool_denylist() {
            let deny_set: std::collections::HashSet<&str> = self
                .definition
                .disallowed_tools
                .iter()
                .map(|s| s.as_str())
                .collect();
            available_tools
                .iter()
                .filter(|t| !deny_set.contains(t.as_str()))
                .cloned()
                .collect()
        } else {
            available_tools.to_owned()
        }
    }

    /// Build the system prompt for this agent run.
    ///
    /// Uses the agent's system prompt if defined, otherwise generates a
    /// default prompt based on the agent type.
    pub fn build_system_prompt(&self) -> String {
        compose_agent_system_prompt(
            &self.definition,
            self.config.system_prompt.as_deref(),
            &self.config.working_dir,
        )
    }

    /// Resolve the model to use for this run.
    ///
    /// Priority: config override > agent definition > default.
    pub fn resolve_model(&self, default_model: &str) -> String {
        if !self.config.model.is_empty() && self.config.model != "inherit" {
            return self.config.model.clone();
        }
        match &self.definition.model {
            Some(m) if m != "inherit" && !m.is_empty() => m.clone(),
            _ => default_model.to_owned(),
        }
    }

    /// Resolve the maximum number of turns.
    ///
    /// Priority: config override > agent definition > default (200).
    pub fn resolve_max_turns(&self) -> u32 {
        if self.config.max_turns > 0 {
            return self.config.max_turns;
        }
        self.definition.max_turns
    }

    /// Build the fully-resolved execution request for this run.
    #[must_use]
    pub fn build_request(
        &self,
        task: &str,
        context: &[ConversationEntry],
    ) -> AgentExecutionRequest {
        AgentExecutionRequest {
            agent_type: self.definition.agent_type.clone(),
            agent_name: None,
            team_name: None,
            task: task.to_owned(),
            context: context.to_owned(),
            model: self.resolve_model("default"),
            max_turns: self.resolve_max_turns(),
            system_prompt: self.build_system_prompt(),
            critical_system_reminder: self
                .definition
                .critical_system_reminder_experimental
                .clone(),
            omit_claude_md: self.definition.omit_claude_md,
            omit_git_status: matches!(self.definition.agent_type.as_str(), "Explore" | "Plan"),
            tools: self.resolve_tools(&self.config.tools),
            permission_mode: None,
            working_dir: self.config.working_dir.clone(),
            additional_working_directories: self.config.additional_working_directories.clone(),
            skip_transcript: false,
        }
    }

    /// Run the agent with the given task and conversation context.
    ///
    /// This entry point requires the host runtime to supply a concrete
    /// executor via [`AgentRunner::run_with_executor`].
    pub async fn run(&self, _task: &str, _context: &[ConversationEntry]) -> Result<AgentRunResult> {
        Err(anyhow!(
            "AgentRunner requires a concrete executor; call run_with_executor() from the host runtime"
        ))
    }

    /// Run the agent using a concrete executor supplied by the host runtime.
    pub async fn run_with_executor(
        &self,
        task: &str,
        context: &[ConversationEntry],
        executor: &dyn AgentExecutor,
    ) -> Result<AgentRunResult> {
        let request = self.build_request(task, context);
        tracing::info!(
            agent_type = %request.agent_type,
            model = %request.model,
            max_turns = request.max_turns,
            prompt_len = request.system_prompt.len(),
            task_preview = %task.chars().take(80).collect::<String>(),
            "Executing agent run"
        );
        executor.execute(request).await
    }
}

#[must_use]
pub fn compose_agent_system_prompt(
    definition: &AgentDefinition,
    system_prompt_override: Option<&str>,
    working_dir: &Path,
) -> String {
    if let Some(prompt) = system_prompt_override.filter(|prompt| !prompt.trim().is_empty()) {
        return prompt.to_owned();
    }

    let base_prompt = match &definition.system_prompt {
        Some(prompt) if !prompt.is_empty() => prompt.clone(),
        _ => format!(
            "You are an agent of type '{}'. Complete the task as instructed.",
            definition.agent_type
        ),
    };

    if let Some(scope) = definition.memory {
        return append_memory_prompt_to_system_prompt(
            &base_prompt,
            &definition.agent_type,
            scope,
            working_dir,
            None,
        );
    }

    base_prompt
}

// ── Enhanced functions ────────────────────────────────────────────────────

/// Result of resolving effective tools for an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedTools {
    /// Whether the agent has a wildcard tool specification.
    pub has_wildcard: bool,
    /// Valid tool names from the agent's specification.
    pub valid_tools: Vec<String>,
    /// Invalid tool names that couldn't be resolved.
    pub invalid_tools: Vec<String>,
    /// The resolved set of tool names.
    pub resolved: Vec<String>,
}

/// Enhance a system prompt with environment details.
///
/// Appends information about the working directory, absolute paths,
/// and formatting guidelines (no emoji, no colons before tool calls).
pub fn enhance_system_prompt_with_env_details(
    base_prompt: &str,
    working_dir: &Path,
    absolute_paths: &[&Path],
) -> String {
    let mut prompt = base_prompt.to_owned();

    prompt.push_str("\n\n## Environment\n");
    prompt.push_str(&format!("Working directory: {}\n", working_dir.display()));

    if !absolute_paths.is_empty() {
        prompt.push_str("Additional paths:\n");
        for path in absolute_paths {
            prompt.push_str(&format!("- {}\n", path.display()));
        }
    }

    prompt.push_str("\n## Formatting Guidelines\n");
    prompt.push_str("- Use absolute paths when referring to files\n");
    prompt.push_str("- Do not use emoji in output\n");
    prompt.push_str("- Do not use colons before tool calls\n");

    prompt
}

/// Resolve the effective tool set for an agent.
///
/// Handles wildcard expansion (`*`), denylist filtering, and validation
/// against the available tool set. Returns a [`ResolvedTools`] with
/// detailed information about the resolution.
pub fn resolve_effective_tools(
    agent_tools: &[String],
    disallowed_tools: &[String],
    available_tools: &[String],
) -> ResolvedTools {
    let deny_set: BTreeSet<&str> = disallowed_tools.iter().map(|s| s.as_str()).collect();
    let available_set: BTreeSet<&str> = available_tools.iter().map(|s| s.as_str()).collect();

    // Check for wildcard (explicit "*" or empty means all tools)
    let has_wildcard = agent_tools.is_empty() || (agent_tools.len() == 1 && agent_tools[0] == "*");

    if has_wildcard || agent_tools.is_empty() {
        let resolved: Vec<String> = available_tools
            .iter()
            .filter(|t| !deny_set.contains(t.as_str()))
            .cloned()
            .collect();
        return ResolvedTools {
            has_wildcard,
            valid_tools: Vec::new(),
            invalid_tools: Vec::new(),
            resolved,
        };
    }

    let mut valid = Vec::new();
    let mut invalid = Vec::new();
    let mut resolved = Vec::new();
    let mut seen = BTreeSet::new();

    for tool in agent_tools {
        if deny_set.contains(tool.as_str()) {
            // Tool is in denylist — skip
            continue;
        }
        if available_set.contains(tool.as_str()) {
            valid.push(tool.clone());
            if seen.insert(tool.as_str()) {
                resolved.push(tool.clone());
            }
        } else {
            invalid.push(tool.clone());
        }
    }

    ResolvedTools {
        has_wildcard: false,
        valid_tools: valid,
        invalid_tools: invalid,
        resolved,
    }
}

/// Aggregate multiple agent run results into a single summary.
///
/// Combines output, usage, and success status from multiple runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregatedRunResults {
    /// Combined output from all runs.
    pub combined_output: String,
    /// Whether all runs succeeded.
    pub all_succeeded: bool,
    /// Total turns across all runs.
    pub total_turns: u32,
    /// Aggregated usage.
    pub total_usage: UsageSummary,
    /// Number of runs.
    pub run_count: usize,
}

/// Aggregate multiple run results.
pub fn aggregate_run_results(results: &[AgentRunResult]) -> AggregatedRunResults {
    let mut combined_output = String::new();
    let mut all_succeeded = true;
    let mut total_turns = 0u32;
    let mut total_usage = UsageSummary::default();

    for (i, result) in results.iter().enumerate() {
        if i > 0 {
            combined_output.push_str("\n---\n");
        }
        combined_output.push_str(&result.output);

        if !result.success {
            all_succeeded = false;
        }
        total_turns += result.turns;
        total_usage.input_tokens += result.usage.input_tokens;
        total_usage.output_tokens += result.usage.output_tokens;
        total_usage.cache_creation_tokens += result.usage.cache_creation_tokens;
        total_usage.cache_read_tokens += result.usage.cache_read_tokens;
    }

    AggregatedRunResults {
        combined_output,
        all_succeeded,
        total_turns,
        total_usage,
        run_count: results.len(),
    }
}

/// Format an agent result for return to the caller.
///
/// Produces a structured output string with the agent's final text,
/// usage information, and status.
pub fn format_agent_run_result(agent_id: &str, result: &AgentRunResult) -> String {
    let status = if result.success {
        "completed"
    } else {
        "failed"
    };
    format!(
        "Agent {agent_id} {status}\n\
         Turns: {turns}\n\
         Tokens: {input_in}+{output_out} (cache: +{cache_create}, -{cache_read})\n\
         Output:\n{output}",
        turns = result.turns,
        input_in = result.usage.input_tokens,
        output_out = result.usage.output_tokens,
        cache_create = result.usage.cache_creation_tokens,
        cache_read = result.usage.cache_read_tokens,
        output = result.output,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::AgentSource;
    use std::sync::{Arc, Mutex};

    fn test_definition() -> AgentDefinition {
        AgentDefinition {
            agent_type: "test-agent".to_owned(),
            when_to_use: "Test agent".to_owned(),
            tools: vec!["Bash".to_owned(), "Read".to_owned()],
            disallowed_tools: vec!["Agent".to_owned()],
            max_turns: 100,
            model: Some("haiku".to_owned()),
            effort: None,
            permission_mode: None,
            source: AgentSource::BuiltIn,
            base_dir: "built-in".to_owned(),
            system_prompt: Some("You are a test agent.".to_owned()),
            skills: Vec::new(),
            mcp_servers: Vec::new(),
            hooks: None,
            color: None,
            critical_system_reminder_experimental: None,
            required_mcp_servers: Vec::new(),
            memory: None,
            background: false,
            isolation: crate::definition::AgentIsolation::None,
            initial_prompt: None,
            omit_claude_md: false,
            filename: None,
        }
    }

    fn test_config() -> AgentRunConfig {
        AgentRunConfig {
            max_turns: 0,
            model: String::new(),
            tools: vec!["Bash".to_owned(), "Read".to_owned(), "Write".to_owned()],
            system_prompt: None,
            working_dir: PathBuf::from("/tmp"),
            additional_working_directories: Vec::new(),
        }
    }

    #[test]
    fn resolve_tools_with_allowlist_and_denylist() {
        let runner = AgentRunner::new(test_definition(), test_config());
        let available = vec![
            "Bash".to_owned(),
            "Read".to_owned(),
            "Write".to_owned(),
            "Agent".to_owned(),
        ];
        let tools = runner.resolve_tools(&available);
        assert_eq!(tools, vec!["Bash", "Read"]); // Agent filtered by denylist
    }

    #[test]
    fn resolve_tools_wildcard() {
        let mut def = test_definition();
        def.tools = vec!["*".to_owned()];
        let runner = AgentRunner::new(def, test_config());
        let available = vec!["Bash".to_owned(), "Read".to_owned()];
        let tools = runner.resolve_tools(&available);
        assert_eq!(tools, vec!["Bash", "Read"]);
    }

    #[test]
    fn resolve_tools_wildcard_respects_denylist() {
        let mut def = test_definition();
        def.tools = vec!["*".to_owned()];
        def.disallowed_tools = vec!["Read".to_owned()];
        let runner = AgentRunner::new(def, test_config());
        let available = vec!["Bash".to_owned(), "Read".to_owned(), "Write".to_owned()];
        let tools = runner.resolve_tools(&available);
        assert_eq!(tools, vec!["Bash", "Write"]);
    }

    #[test]
    fn resolve_tools_denylist_only() {
        let mut def = test_definition();
        def.tools = Vec::new();
        let runner = AgentRunner::new(def, test_config());
        let available = vec!["Bash".to_owned(), "Read".to_owned(), "Agent".to_owned()];
        let tools = runner.resolve_tools(&available);
        assert_eq!(tools, vec!["Bash", "Read"]);
    }

    #[test]
    fn resolve_tools_no_restrictions() {
        let mut def = test_definition();
        def.tools = Vec::new();
        def.disallowed_tools = Vec::new();
        let runner = AgentRunner::new(def, test_config());
        let available = vec!["Bash".to_owned(), "Read".to_owned()];
        let tools = runner.resolve_tools(&available);
        assert_eq!(tools, vec!["Bash", "Read"]);
    }

    #[test]
    fn build_system_prompt_uses_definition() {
        let runner = AgentRunner::new(test_definition(), test_config());
        let prompt = runner.build_system_prompt();
        assert_eq!(prompt, "You are a test agent.");
    }

    #[test]
    fn build_system_prompt_prefers_config_override() {
        let mut config = test_config();
        config.system_prompt = Some("Config override".to_owned());
        let runner = AgentRunner::new(test_definition(), config);
        let prompt = runner.build_system_prompt();
        assert_eq!(prompt, "Config override");
    }

    #[test]
    fn build_system_prompt_default_when_empty() {
        let mut def = test_definition();
        def.system_prompt = None;
        let runner = AgentRunner::new(def, test_config());
        let prompt = runner.build_system_prompt();
        assert!(prompt.contains("test-agent"));
    }

    #[test]
    fn build_system_prompt_appends_agent_memory_when_enabled() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut def = test_definition();
        def.memory = Some(crate::definition::AgentMemoryScope::Project);
        let mut config = test_config();
        config.working_dir = temp.path().to_path_buf();
        let memory_dir = crate::memory::get_agent_memory_dir(
            &def.agent_type,
            crate::definition::AgentMemoryScope::Project,
            &config.working_dir,
            &PathBuf::from("/unused"),
        );
        std::fs::create_dir_all(&memory_dir).expect("memory dir");
        std::fs::write(
            memory_dir.join("MEMORY.md"),
            "- [User preference](pref.md) — keep responses terse\n",
        )
        .expect("memory file");

        let runner = AgentRunner::new(def, config);
        let prompt = runner.build_system_prompt();

        assert!(prompt.starts_with("You are a test agent.\n\n# Persistent Agent Memory"));
        assert!(prompt.contains("Since this memory is project-scope"));
        assert!(prompt.contains("keep responses terse"));
    }

    #[test]
    fn build_request_carries_additional_working_directories() {
        let mut config = test_config();
        config.additional_working_directories = vec![PathBuf::from("/workspace/extra")];
        let runner = AgentRunner::new(test_definition(), config);

        let request = runner.build_request("task", &[]);

        assert_eq!(
            request.additional_working_directories,
            vec![PathBuf::from("/workspace/extra")]
        );
    }

    #[test]
    fn resolve_model_from_definition() {
        let runner = AgentRunner::new(test_definition(), test_config());
        assert_eq!(runner.resolve_model("sonnet"), "haiku");
    }

    #[test]
    fn resolve_model_config_overrides() {
        let mut config = test_config();
        config.model = "opus".to_owned();
        let runner = AgentRunner::new(test_definition(), config);
        assert_eq!(runner.resolve_model("sonnet"), "opus");
    }

    #[test]
    fn resolve_model_inherit_falls_through() {
        let mut def = test_definition();
        def.model = Some("inherit".to_owned());
        let runner = AgentRunner::new(def, test_config());
        assert_eq!(runner.resolve_model("sonnet"), "sonnet");
    }

    #[test]
    fn resolve_max_turns_from_definition() {
        let runner = AgentRunner::new(test_definition(), test_config());
        assert_eq!(runner.resolve_max_turns(), 100);
    }

    #[test]
    fn resolve_max_turns_config_overrides() {
        let mut config = test_config();
        config.max_turns = 50;
        let runner = AgentRunner::new(test_definition(), config);
        assert_eq!(runner.resolve_max_turns(), 50);
    }

    #[tokio::test]
    async fn run_requires_executor() {
        let runner = AgentRunner::new(test_definition(), test_config());
        let error = runner
            .run("test task", &[])
            .await
            .expect_err("missing executor");
        assert!(error.to_string().contains("run_with_executor"));
    }

    struct RecordingExecutor {
        seen_requests: Arc<Mutex<Vec<AgentExecutionRequest>>>,
        result: AgentRunResult,
    }

    #[async_trait]
    impl AgentExecutor for RecordingExecutor {
        async fn execute(&self, request: AgentExecutionRequest) -> Result<AgentRunResult> {
            self.seen_requests
                .lock()
                .expect("requests lock")
                .push(request);
            Ok(self.result.clone())
        }
    }

    #[tokio::test]
    async fn run_with_executor_passes_resolved_request() {
        let seen_requests = Arc::new(Mutex::new(Vec::new()));
        let executor = RecordingExecutor {
            seen_requests: Arc::clone(&seen_requests),
            result: AgentRunResult {
                output: "done".to_owned(),
                success: true,
                turns: 2,
                usage: UsageSummary {
                    input_tokens: 10,
                    output_tokens: 5,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                },
            },
        };
        let runner = AgentRunner::new(test_definition(), test_config());
        let context = vec![ConversationEntry {
            role: "user".to_owned(),
            content: "prior context".to_owned(),
        }];

        let result = runner
            .run_with_executor("test task", &context, &executor)
            .await
            .expect("run with executor");

        assert_eq!(result.output, "done");
        let requests = seen_requests.lock().expect("requests");
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.agent_type, "test-agent");
        assert_eq!(request.task, "test task");
        assert_eq!(request.context.len(), 1);
        assert_eq!(request.model, "haiku");
        assert_eq!(request.max_turns, 100);
        assert_eq!(request.system_prompt, "You are a test agent.");
        assert_eq!(request.tools, vec!["Bash", "Read"]);
        assert_eq!(request.working_dir, PathBuf::from("/tmp"));
    }

    // ── Enhanced tests ──────────────────────────────────────────────────

    #[test]
    fn enhance_system_prompt_adds_env_details() {
        let base = "You are a test agent.";
        let working_dir = PathBuf::from("/home/user/project");
        let tmp = PathBuf::from("/tmp");
        let extra = vec![tmp.as_path()];
        let enhanced = enhance_system_prompt_with_env_details(base, &working_dir, &extra);
        assert!(enhanced.starts_with("You are a test agent."));
        assert!(enhanced.contains("## Environment"));
        assert!(enhanced.contains("/home/user/project"));
        assert!(enhanced.contains("/tmp"));
    }

    #[test]
    fn enhance_system_prompt_no_extra_paths() {
        let base = "Base prompt";
        let working_dir = PathBuf::from("/tmp");
        let enhanced = enhance_system_prompt_with_env_details(base, &working_dir, &[]);
        assert!(enhanced.contains("## Environment"));
        assert!(!enhanced.contains("Additional paths"));
    }

    #[test]
    fn enhance_system_prompt_formatting_guidelines() {
        let base = "Base";
        let working_dir = PathBuf::from("/tmp");
        let enhanced = enhance_system_prompt_with_env_details(base, &working_dir, &[]);
        assert!(enhanced.contains("## Formatting Guidelines"));
        assert!(enhanced.contains("absolute paths"));
        assert!(enhanced.contains("emoji"));
        assert!(enhanced.contains("colons before tool calls"));
    }

    #[test]
    fn resolve_effective_tools_wildcard() {
        let agent_tools = vec!["*".to_owned()];
        let disallowed = vec!["Agent".to_owned()];
        let available = vec!["Bash".to_owned(), "Read".to_owned(), "Agent".to_owned()];
        let result = resolve_effective_tools(&agent_tools, &disallowed, &available);
        assert!(result.has_wildcard);
        assert_eq!(result.resolved, vec!["Bash", "Read"]);
    }

    #[test]
    fn resolve_effective_tools_specific_list() {
        let agent_tools = vec![
            "Bash".to_owned(),
            "Read".to_owned(),
            "NonExistent".to_owned(),
        ];
        let disallowed: Vec<String> = Vec::new();
        let available = vec!["Bash".to_owned(), "Read".to_owned(), "Write".to_owned()];
        let result = resolve_effective_tools(&agent_tools, &disallowed, &available);
        assert!(!result.has_wildcard);
        assert_eq!(result.valid_tools, vec!["Bash", "Read"]);
        assert_eq!(result.invalid_tools, vec!["NonExistent"]);
        assert_eq!(result.resolved, vec!["Bash", "Read"]);
    }

    #[test]
    fn resolve_effective_tools_denylist_filters() {
        let agent_tools = vec!["Bash".to_owned(), "Agent".to_owned()];
        let disallowed = vec!["Agent".to_owned()];
        let available = vec!["Bash".to_owned(), "Agent".to_owned()];
        let result = resolve_effective_tools(&agent_tools, &disallowed, &available);
        assert_eq!(result.resolved, vec!["Bash"]);
    }

    #[test]
    fn resolve_effective_tools_empty_agent_tools() {
        let agent_tools: Vec<String> = Vec::new();
        let disallowed: Vec<String> = Vec::new();
        let available = vec!["Bash".to_owned()];
        let result = resolve_effective_tools(&agent_tools, &disallowed, &available);
        assert!(result.has_wildcard); // Empty means all tools
        assert_eq!(result.resolved, vec!["Bash"]);
    }

    #[test]
    fn resolve_effective_tools_deduplicates() {
        let agent_tools = vec!["Bash".to_owned(), "Bash".to_owned()];
        let disallowed: Vec<String> = Vec::new();
        let available = vec!["Bash".to_owned()];
        let result = resolve_effective_tools(&agent_tools, &disallowed, &available);
        assert_eq!(result.resolved, vec!["Bash"]);
    }

    #[test]
    fn aggregate_run_results_single() {
        let results = vec![AgentRunResult {
            output: "Done".to_owned(),
            success: true,
            turns: 3,
            usage: UsageSummary {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        }];
        let agg = aggregate_run_results(&results);
        assert!(agg.all_succeeded);
        assert_eq!(agg.run_count, 1);
        assert_eq!(agg.total_turns, 3);
        assert_eq!(agg.total_usage.input_tokens, 100);
        assert!(agg.combined_output.contains("Done"));
    }

    #[test]
    fn aggregate_run_results_multiple() {
        let results = vec![
            AgentRunResult {
                output: "First".to_owned(),
                success: true,
                turns: 2,
                usage: UsageSummary {
                    input_tokens: 50,
                    output_tokens: 25,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                },
            },
            AgentRunResult {
                output: "Second".to_owned(),
                success: false,
                turns: 1,
                usage: UsageSummary {
                    input_tokens: 30,
                    output_tokens: 15,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                },
            },
        ];
        let agg = aggregate_run_results(&results);
        assert!(!agg.all_succeeded);
        assert_eq!(agg.run_count, 2);
        assert_eq!(agg.total_turns, 3);
        assert_eq!(agg.total_usage.input_tokens, 80);
        assert!(agg.combined_output.contains("First"));
        assert!(agg.combined_output.contains("Second"));
        assert!(agg.combined_output.contains("---"));
    }

    #[test]
    fn aggregate_run_results_empty() {
        let agg = aggregate_run_results(&[]);
        assert!(agg.all_succeeded);
        assert_eq!(agg.run_count, 0);
        assert!(agg.combined_output.is_empty());
    }

    #[test]
    fn format_agent_run_result_success() {
        let result = AgentRunResult {
            output: "Fixed the bug".to_owned(),
            success: true,
            turns: 5,
            usage: UsageSummary {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_tokens: 10,
                cache_read_tokens: 20,
            },
        };
        let formatted = format_agent_run_result("agent-123", &result);
        assert!(formatted.contains("agent-123 completed"));
        assert!(formatted.contains("Turns: 5"));
        assert!(formatted.contains("100+50"));
        assert!(formatted.contains("Fixed the bug"));
    }

    #[test]
    fn format_agent_run_result_failure() {
        let result = AgentRunResult {
            output: "Error occurred".to_owned(),
            success: false,
            turns: 0,
            usage: UsageSummary::default(),
        };
        let formatted = format_agent_run_result("agent-456", &result);
        assert!(formatted.contains("agent-456 failed"));
    }
}
