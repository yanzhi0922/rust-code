//! Ant-only model override suffix section.
//!
//! Matches `getAntModelOverrideSection()` in Claude Code's `prompts.ts`.
//! For external builds, this returns `None` (no ant-specific suffix).
//! For ant builds, reads the suffix from `ANT_MODEL_OVERRIDE_CONFIG` env var
//! or the `getAntModelOverrideConfig()` configuration source.

use anyhow::Result;

use crate::PromptContext;
use crate::sections::SystemPromptSection;

pub struct AntModelOverrideSection;

impl SystemPromptSection for AntModelOverrideSection {
    fn name(&self) -> &str {
        "ant_model_override"
    }

    fn compute(&self, ctx: &PromptContext) -> Result<Option<String>> {
        // TS: if (process.env.USER_TYPE !== 'ant') return null
        let user_type = std::env::var("USER_TYPE").unwrap_or_default();
        if user_type != "ant" {
            return Ok(None);
        }

        // TS: if (isUndercover()) return null
        if ctx.is_undercover {
            return Ok(None);
        }

        // TS: return getAntModelOverrideConfig()?.defaultSystemPromptSuffix || null
        // For external builds, read from env var as the configuration source.
        let suffix = std::env::var("ANT_MODEL_OVERRIDE_CONFIG")
            .ok()
            .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
            .and_then(|v| {
                v.get("defaultSystemPromptSuffix")
                    .and_then(|s| s.as_str().map(|s| s.to_string()))
            });

        Ok(suffix.filter(|s| !s.is_empty()))
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
    fn returns_none_for_non_ant_user() {
        let section = AntModelOverrideSection;
        // USER_TYPE defaults to empty (not "ant")
        let result = section.compute(&test_ctx()).expect("compute ok");
        assert!(result.is_none());
    }

    #[test]
    fn returns_none_for_undercover() {
        unsafe {
            std::env::set_var("USER_TYPE", "ant");
        }
        let mut ctx = test_ctx();
        ctx.is_undercover = true;
        let section = AntModelOverrideSection;
        let result = section.compute(&ctx).expect("compute ok");
        assert!(result.is_none());
        unsafe {
            std::env::remove_var("USER_TYPE");
        }
    }

    #[test]
    fn returns_suffix_when_configured() {
        unsafe {
            std::env::set_var("USER_TYPE", "ant");
        }
        unsafe {
            std::env::set_var(
                "ANT_MODEL_OVERRIDE_CONFIG",
                r#"{"defaultSystemPromptSuffix": "You are running in ant mode."}"#,
            );
        }
        let section = AntModelOverrideSection;
        let result = section.compute(&test_ctx()).expect("compute ok");
        assert_eq!(result.as_deref(), Some("You are running in ant mode."));
        unsafe {
            std::env::remove_var("USER_TYPE");
        }
        unsafe {
            std::env::remove_var("ANT_MODEL_OVERRIDE_CONFIG");
        }
    }
}
