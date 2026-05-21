//! Tool result clearing and summarize sections.
//!
//! Matches `getFunctionResultClearingSection()` and `SUMMARIZE_TOOL_RESULTS_SECTION`
//! in Claude Code's `prompts.ts`.

use anyhow::Result;

use crate::PromptContext;
use crate::sections::SystemPromptSection;

/// The summarize tool results instruction.
pub const SUMMARIZE_TOOL_RESULTS_SECTION: &str = "When working with tool results, write down any important information you might need later in your response, as the original tool result may be cleared later.";

/// Function result clearing section.
///
/// This is gated in Claude Code by the cached microcompact feature and runtime
/// model support. External builds normally omit it, but the section name and
/// position must still exist in the registry.
pub struct FunctionResultClearingSection;

impl SystemPromptSection for FunctionResultClearingSection {
    fn name(&self) -> &str {
        "frc"
    }

    fn compute(&self, ctx: &PromptContext) -> Result<Option<String>> {
        Ok(ctx
            .features
            .function_result_keep_recent
            .map(build_function_result_clearing_section))
    }
}

/// Summarize tool results section.
pub struct ToolResultSection;

impl SystemPromptSection for ToolResultSection {
    fn name(&self) -> &str {
        "summarize_tool_results"
    }

    fn compute(&self, _ctx: &PromptContext) -> Result<Option<String>> {
        Ok(Some(SUMMARIZE_TOOL_RESULTS_SECTION.to_string()))
    }
}

/// Build the function result clearing section for a specific keep-recent count.
/// Public so it can be used when the clearing config is known.
pub fn build_function_result_clearing_section(keep_recent: usize) -> String {
    format!(
        "# Function Result Clearing\n\n\
        Old tool results will be automatically cleared from context to free up space. \
        The {keep_recent} most recent results are always kept."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn test_ctx() -> PromptContext {
        PromptContext {
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
        }
    }

    #[test]
    fn tool_result_section_returns_summarize_guidance() {
        let section = ToolResultSection;
        let result = section.compute(&test_ctx()).expect("compute ok");
        let content = result.expect("should be Some");
        assert!(content.contains("write down any important information"));
        assert!(content.contains("tool result may be cleared"));
    }

    #[test]
    fn build_clearing_section_format() {
        let content = build_function_result_clearing_section(5);
        assert!(content.starts_with("# Function Result Clearing"));
        assert!(content.contains("5 most recent"));
    }

    #[test]
    fn summarize_constant_is_sensible() {
        assert!(SUMMARIZE_TOOL_RESULTS_SECTION.len() > 50);
        assert!(SUMMARIZE_TOOL_RESULTS_SECTION.contains("tool result"));
    }

    #[test]
    fn function_result_clearing_section_is_gated_off_by_default() {
        let section = FunctionResultClearingSection;
        let result = section.compute(&test_ctx()).expect("compute ok");
        assert!(result.is_none());
        assert_eq!(section.name(), "frc");
    }
}
