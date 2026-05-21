//! Output Style section — custom output style configuration.
//!
//! Matches `getOutputStyleSection()` in Claude Code's `prompts.ts`.

use anyhow::Result;

use crate::PromptContext;
use crate::sections::SystemPromptSection;

/// The output style section.
pub struct OutputStyleSection;

impl SystemPromptSection for OutputStyleSection {
    fn name(&self) -> &str {
        "output_style"
    }

    fn compute(&self, ctx: &PromptContext) -> Result<Option<String>> {
        let style = match &ctx.output_style {
            Some(s) => s,
            None => return Ok(None),
        };

        Ok(Some(format!(
            "# Output Style: {}\n{}",
            style.name, style.prompt
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn test_ctx_with_style(style: Option<crate::OutputStyleConfig>) -> PromptContext {
        PromptContext {
            model: "test".to_string(),
            cwd: PathBuf::from("/tmp"),
            is_git: false,
            platform: "linux".to_string(),
            shell: "bash".to_string(),
            os_version: "Linux 6.6".to_string(),
            enabled_tools: HashSet::new(),
            language: None,
            output_style: style,
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
    fn output_style_with_config() {
        let style = crate::OutputStyleConfig {
            name: "Concise".to_string(),
            prompt: "Be very brief and to the point.".to_string(),
            keep_coding_instructions: true,
        };
        let section = OutputStyleSection;
        let result = section
            .compute(&test_ctx_with_style(Some(style)))
            .expect("compute ok");
        let content = result.expect("should be Some");
        assert!(content.starts_with("# Output Style: Concise"));
        assert!(content.contains("Be very brief and to the point."));
    }

    #[test]
    fn output_style_without_config() {
        let section = OutputStyleSection;
        let result = section
            .compute(&test_ctx_with_style(None))
            .expect("compute ok");
        assert!(result.is_none());
    }
}
