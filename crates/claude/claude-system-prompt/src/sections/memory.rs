//! Memory section — runtime-resolved memory prompt content.

use anyhow::Result;

use crate::PromptContext;
use crate::sections::SystemPromptSection;

pub struct MemorySection;

impl SystemPromptSection for MemorySection {
    fn name(&self) -> &str {
        "memory"
    }

    fn compute(&self, ctx: &PromptContext) -> Result<Option<String>> {
        Ok(ctx.features.memory_prompt.clone())
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
    fn memory_section_returns_none_by_default() {
        let section = MemorySection;
        let result = section.compute(&test_ctx()).expect("compute ok");
        assert!(result.is_none());
    }

    #[test]
    fn memory_section_name() {
        let section = MemorySection;
        assert_eq!(section.name(), "memory");
    }
}
