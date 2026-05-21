//! Intro section — the opening block of the system prompt.
//!
//! Matches `getSimpleIntroSection()` in Claude Code's `prompts.ts`.

use anyhow::Result;

use crate::PromptContext;
use crate::sections::SystemPromptSection;

/// Cyber risk instruction — security boundary guidance.
/// IMPORTANT: Do not modify without security team review.
pub const CYBER_RISK_INSTRUCTION: &str = "IMPORTANT: Assist with authorized security testing, defensive security, CTF challenges, and educational contexts. Refuse requests for destructive techniques, DoS attacks, mass targeting, supply chain compromise, or detection evasion for malicious purposes. Dual-use security tools (C2 frameworks, credential testing, exploit development) require clear authorization context: pentesting engagements, CTF competitions, security research, or defensive use cases.";

/// The intro section of the system prompt.
///
/// Content:
/// - "You are an interactive agent..." opening
/// - Cyber risk instruction
/// - URL safety warning
pub struct IntroSection;

impl SystemPromptSection for IntroSection {
    fn name(&self) -> &str {
        "intro"
    }

    fn compute(&self, ctx: &PromptContext) -> Result<Option<String>> {
        let output_style_line = if ctx.output_style.is_some() {
            r#"according to your "Output Style" below, which describes how you should respond to user queries."#
        } else {
            "with software engineering tasks."
        };

        let content = format!(
            "\nYou are an interactive agent that helps users {output_style_line} Use the instructions below and the tools available to you to assist the user.\n\n\
{CYBER_RISK_INSTRUCTION}\n\
IMPORTANT: You must NEVER generate or guess URLs for the user unless you are confident that the URLs are for helping the user with programming. You may use URLs provided by the user in their messages or local files."
        );

        Ok(Some(content))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn test_ctx() -> PromptContext {
        PromptContext {
            model: "claude-sonnet-4-6".to_string(),
            cwd: PathBuf::from("/tmp/test"),
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
            features: crate::PromptFeatures::default(),
            is_undercover: false,
        }
    }

    #[test]
    fn intro_section_contains_cyber_risk() {
        let section = IntroSection;
        let result = section
            .compute(&test_ctx())
            .expect("compute should succeed");
        let content = result.expect("should return Some");
        assert!(content.contains(CYBER_RISK_INSTRUCTION));
    }

    #[test]
    fn intro_section_contains_url_warning() {
        let section = IntroSection;
        let result = section
            .compute(&test_ctx())
            .expect("compute should succeed");
        let content = result.expect("should return Some");
        assert!(content.contains("NEVER generate or guess URLs"));
    }

    #[test]
    fn intro_with_output_style() {
        let mut ctx = test_ctx();
        ctx.output_style = Some(crate::OutputStyleConfig {
            name: "Test".to_string(),
            prompt: "Be helpful".to_string(),
            keep_coding_instructions: true,
        });
        let section = IntroSection;
        let result = section.compute(&ctx).expect("compute should succeed");
        let content = result.expect("should return Some");
        assert!(content.contains("Output Style"));
    }

    #[test]
    fn intro_without_output_style() {
        let section = IntroSection;
        let result = section
            .compute(&test_ctx())
            .expect("compute should succeed");
        let content = result.expect("should return Some");
        assert!(content.contains("software engineering tasks"));
    }
}
