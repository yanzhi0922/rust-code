//! System reminders used by the proactive prompt path.

use anyhow::Result;

use crate::PromptContext;
use crate::sections::SystemPromptSection;

pub struct SystemRemindersSection;

impl SystemPromptSection for SystemRemindersSection {
    fn name(&self) -> &str {
        "system_reminders"
    }

    fn compute(&self, _ctx: &PromptContext) -> Result<Option<String>> {
        Ok(Some(
            "- Tool results and user messages may include <system-reminder> tags. <system-reminder> tags contain useful information and reminders. They are automatically added by the system, and bear no direct relation to the specific tool results or user messages in which they appear.\n- The conversation has unlimited context through automatic summarization.".to_string(),
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
    fn system_reminders_always_included() {
        let section = SystemRemindersSection;
        let result = section.compute(&test_ctx()).expect("compute ok");
        assert!(
            result.is_some(),
            "system reminders section should always be Some"
        );
    }

    #[test]
    fn system_reminders_mentions_system_reminder_tags() {
        let section = SystemRemindersSection;
        let result = section.compute(&test_ctx()).expect("compute ok");
        let content = result.expect("should be Some");
        assert!(content.contains("<system-reminder>"));
        assert!(content.contains("automatically added"));
        assert!(content.contains("automatic summarization"));
    }

    #[test]
    fn system_reminders_section_name() {
        let section = SystemRemindersSection;
        assert_eq!(section.name(), "system_reminders");
    }
}
