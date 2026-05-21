//! Agent Tool section — fork/subagent guidance.
//!
//! Matches `getAgentToolSection()` in Claude Code's `prompts.ts`.

use anyhow::Result;

use crate::PromptContext;
use crate::sections::SystemPromptSection;

const AGENT_TOOL_NAME: &str = "Agent";

/// The agent tool section.
///
/// Provides different guidance depending on whether fork subagent mode is enabled.
pub struct AgentSection;

impl SystemPromptSection for AgentSection {
    fn name(&self) -> &str {
        "agent"
    }

    fn compute(&self, ctx: &PromptContext) -> Result<Option<String>> {
        if !ctx.enabled_tools.contains(AGENT_TOOL_NAME) {
            return Ok(None);
        }

        if ctx.is_fork_subagent_enabled {
            Ok(Some(format!(
                "Calling {AGENT_TOOL_NAME} without a subagent_type creates a fork, which runs in the background and keeps its tool output out of your context \u{2014} so you can keep chatting with the user while it works. Reach for it when research or multi-step implementation work would otherwise fill your context with raw output you won't need again. **If you ARE the fork** \u{2014} execute directly; do not re-delegate."
            )))
        } else {
            Ok(Some(format!(
                "Use the {AGENT_TOOL_NAME} tool with specialized agents when the task at hand matches the agent's description. Subagents are valuable for parallelizing independent queries or for protecting the main context window from excessive results, but they should not be used excessively when not needed. Importantly, avoid duplicating work that subagents are already doing - if you delegate research to a subagent, do not also perform the same searches yourself."
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn test_ctx_with_agent(fork: bool) -> PromptContext {
        let mut tools = HashSet::new();
        tools.insert(AGENT_TOOL_NAME.to_string());
        PromptContext {
            model: "test".to_string(),
            cwd: PathBuf::from("/tmp"),
            is_git: false,
            platform: "linux".to_string(),
            shell: "bash".to_string(),
            os_version: "Linux 6.6".to_string(),
            enabled_tools: tools,
            language: None,
            output_style: None,
            mcp_clients: vec![],
            mcp_instructions_delta_enabled: false,
            is_worktree: false,
            additional_dirs: vec![],
            is_non_interactive: false,
            is_fork_subagent_enabled: fork,
            session_start_date: "2025-01-01".to_string(),
            features: crate::PromptFeatures::default(),
            is_undercover: false,
        }
    }

    #[test]
    fn agent_section_fork_mode() {
        let section = AgentSection;
        let result = section
            .compute(&test_ctx_with_agent(true))
            .expect("compute ok");
        let content = result.expect("should be Some");
        assert!(content.contains("fork"));
        assert!(content.contains("If you ARE the fork"));
    }

    #[test]
    fn agent_section_subagent_mode() {
        let section = AgentSection;
        let result = section
            .compute(&test_ctx_with_agent(false))
            .expect("compute ok");
        let content = result.expect("should be Some");
        assert!(content.contains("subagent"));
        assert!(content.contains("parallelizing"));
    }

    #[test]
    fn agent_section_no_agent_tool() {
        let ctx = PromptContext {
            model: "test".to_string(),
            cwd: PathBuf::from("/tmp"),
            is_git: false,
            platform: "linux".to_string(),
            shell: "bash".to_string(),
            os_version: "Linux 6.6".to_string(),
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
            features: crate::PromptFeatures::default(),
            is_undercover: false,
        };
        let section = AgentSection;
        let result = section.compute(&ctx).expect("compute ok");
        assert!(result.is_none());
    }
}
