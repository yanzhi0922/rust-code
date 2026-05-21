//! Token budget section matching Claude Code's feature-gated wording.

use anyhow::Result;

use crate::PromptContext;
use crate::sections::SystemPromptSection;

pub struct TokenBudgetSection;

impl SystemPromptSection for TokenBudgetSection {
    fn name(&self) -> &str {
        "token_budget"
    }

    fn compute(&self, ctx: &PromptContext) -> Result<Option<String>> {
        Ok(ctx.features.include_token_budget_prompt.then_some("When the user specifies a token target (e.g., \"+500k\", \"spend 2M tokens\", \"use 1B tokens\"), your output token count will be shown each turn. Keep working until you approach the target — plan your work to fill it productively. The target is a hard minimum, not a suggestion. If you stop early, the system will automatically continue you.".to_string()))
    }
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
    fn token_budget_omitted_by_default() {
        let section = TokenBudgetSection;
        let result = section.compute(&test_ctx()).expect("compute ok");
        assert!(result.is_none(), "should be None by default");
    }

    #[test]
    fn token_budget_included_when_enabled() {
        let mut ctx = test_ctx();
        ctx.features.include_token_budget_prompt = true;
        let section = TokenBudgetSection;
        let result = section.compute(&ctx).expect("compute ok");
        let content = result.expect("should be Some when enabled");
        assert!(content.contains("token target"));
        assert!(content.contains("hard minimum"));
        assert!(content.contains("automatically continue"));
    }

    #[test]
    fn token_budget_section_name() {
        let section = TokenBudgetSection;
        assert_eq!(section.name(), "token_budget");
    }
}
