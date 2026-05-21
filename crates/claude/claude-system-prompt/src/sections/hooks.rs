//! Hooks section — informs the model about user-configurable hooks.
//!
//! Matches `getHooksSection()` in Claude Code's `prompts.ts`.
//! Returns a plain paragraph (no header/bullets) that is inlined as one
//! bullet inside the System section.

use anyhow::Result;

use crate::PromptContext;
use crate::sections::SystemPromptSection;

/// The hooks section.
///
/// Returns the raw hooks paragraph. This is used by the System section as
/// one of its bullet items. It is NOT a standalone section with a header.
pub struct HooksSection;

impl SystemPromptSection for HooksSection {
    fn name(&self) -> &str {
        "hooks"
    }

    fn compute(&self, _ctx: &PromptContext) -> Result<Option<String>> {
        Ok(Some(
            "Users may configure 'hooks', shell commands that execute in response to events like tool calls, in settings. Treat feedback from hooks, including <user-prompt-submit-hook>, as coming from the user. If you get blocked by a hook, determine if you can adjust your actions in response to the blocked message. If not, ask the user to check their hooks configuration.".to_string(),
        ))
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
    fn hooks_section_always_included() {
        let section = HooksSection;
        let result = section.compute(&test_ctx()).expect("compute ok");
        assert!(result.is_some(), "hooks section should always be Some");
    }

    #[test]
    fn hooks_section_matches_ts_reference() {
        let section = HooksSection;
        let result = section.compute(&test_ctx()).expect("compute ok");
        let content = result.expect("should be Some");
        assert!(content.contains("Users may configure 'hooks'"));
        assert!(content.contains("<user-prompt-submit-hook>"));
        assert!(content.contains("check their hooks configuration"));
        // Should NOT have a section header
        assert!(!content.starts_with("# "));
        // Should NOT have bullet markers
        assert!(!content.contains("\n - "));
    }

    #[test]
    fn hooks_section_name() {
        let section = HooksSection;
        assert_eq!(section.name(), "hooks");
    }
}
