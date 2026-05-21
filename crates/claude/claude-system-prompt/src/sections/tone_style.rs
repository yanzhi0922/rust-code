//! Tone and Style section — formatting conventions.
//!
//! Matches `getSimpleToneAndStyleSection()` in Claude Code's `prompts.ts`.

use anyhow::Result;

use crate::PromptContext;
use crate::sections::{BulletItem, SystemPromptSection, section_with_bullets};

/// The tone and style section.
pub struct ToneStyleSection;

impl SystemPromptSection for ToneStyleSection {
    fn name(&self) -> &str {
        "tone_style"
    }

    fn compute(&self, ctx: &PromptContext) -> Result<Option<String>> {
        let mut items = vec![BulletItem::Single(
            "Only use emojis if the user explicitly requests it. Avoid using emojis in all communication unless asked.".to_string(),
        )];
        if !ctx.features.ant_user {
            items.push(BulletItem::Single(
                "Your responses should be short and concise.".to_string(),
            ));
        }
        items.extend([
            BulletItem::Single(
                "When referencing specific functions or pieces of code include the pattern file_path:line_number to allow the user to easily navigate to the source code location.".to_string(),
            ),
            BulletItem::Single(
                "When referencing GitHub issues or pull requests, use the owner/repo#123 format (e.g. anthropics/claude-code#100) so they render as clickable links.".to_string(),
            ),
            BulletItem::Single(
                "Do not use a colon before tool calls. Your tool calls may not be shown directly in the output, so text like \"Let me read the file:\" followed by a read tool call should just be \"Let me read the file.\" with a period.".to_string(),
            ),
        ]);

        Ok(Some(section_with_bullets("Tone and style", &items)))
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
    fn tone_style_starts_with_header() {
        let section = ToneStyleSection;
        let result = section.compute(&test_ctx()).expect("compute ok");
        let content = result.expect("should be Some");
        assert!(content.starts_with("# Tone and style"));
    }

    #[test]
    fn tone_style_mentions_emoji() {
        let section = ToneStyleSection;
        let result = section.compute(&test_ctx()).expect("compute ok");
        let content = result.expect("should be Some");
        assert!(content.contains("emojis"));
    }

    #[test]
    fn tone_style_mentions_file_line_format() {
        let section = ToneStyleSection;
        let result = section.compute(&test_ctx()).expect("compute ok");
        let content = result.expect("should be Some");
        assert!(content.contains("file_path:line_number"));
    }

    #[test]
    fn tone_style_mentions_github_format() {
        let section = ToneStyleSection;
        let result = section.compute(&test_ctx()).expect("compute ok");
        let content = result.expect("should be Some");
        assert!(content.contains("owner/repo#123"));
    }
}
