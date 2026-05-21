//! Language Preference section — user's preferred response language.
//!
//! Matches `getLanguageSection()` in Claude Code's `prompts.ts`.

use anyhow::Result;

use crate::PromptContext;
use crate::sections::SystemPromptSection;

/// The language preference section.
pub struct LanguageSection;

impl SystemPromptSection for LanguageSection {
    fn name(&self) -> &str {
        "language"
    }

    fn compute(&self, ctx: &PromptContext) -> Result<Option<String>> {
        let lang = match &ctx.language {
            Some(l) if !l.is_empty() => l,
            _ => return Ok(None),
        };

        Ok(Some(format!(
            "# Language\n\
            Always respond in {lang}. Use {lang} for all explanations, comments, and communications with the user. \
            Technical terms and code identifiers should remain in their original form."
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn test_ctx_with_lang(lang: Option<&str>) -> PromptContext {
        PromptContext {
            model: "test".to_string(),
            cwd: PathBuf::from("/tmp"),
            is_git: false,
            platform: "linux".to_string(),
            shell: "bash".to_string(),
            os_version: "Linux 6.6".to_string(),
            enabled_tools: HashSet::new(),
            language: lang.map(|s| s.to_string()),
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
    fn language_section_with_preference() {
        let section = LanguageSection;
        let result = section
            .compute(&test_ctx_with_lang(Some("Japanese")))
            .expect("compute ok");
        let content = result.expect("should be Some");
        assert!(content.starts_with("# Language"));
        assert!(content.contains("Japanese"));
    }

    #[test]
    fn language_section_without_preference() {
        let section = LanguageSection;
        let result = section
            .compute(&test_ctx_with_lang(None))
            .expect("compute ok");
        assert!(result.is_none());
    }

    #[test]
    fn language_section_empty_string() {
        let section = LanguageSection;
        let result = section
            .compute(&test_ctx_with_lang(Some("")))
            .expect("compute ok");
        assert!(result.is_none());
    }
}
