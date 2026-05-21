//! MCP Server Instructions section — connected MCP server guidance.
//!
//! Matches `getMcpInstructionsSection()` in Claude Code's `prompts.ts`.

use anyhow::Result;

use crate::sections::SystemPromptSection;
use crate::{McpClientInfo, PromptContext};

/// The MCP instructions section.
pub struct McpInstructionsSection;

impl SystemPromptSection for McpInstructionsSection {
    fn name(&self) -> &str {
        "mcp_instructions"
    }

    /// MCP instructions may change between turns as servers connect/disconnect.
    fn is_cacheable(&self) -> bool {
        false
    }

    fn compute(&self, ctx: &PromptContext) -> Result<Option<String>> {
        if ctx.mcp_instructions_delta_enabled {
            return Ok(None);
        }

        if ctx.mcp_clients.is_empty() {
            return Ok(None);
        }

        let clients_with_instructions: Vec<&McpClientInfo> = ctx
            .mcp_clients
            .iter()
            .filter(|client| {
                client
                    .instructions
                    .as_deref()
                    .is_some_and(|instructions| !instructions.is_empty())
            })
            .collect();

        if clients_with_instructions.is_empty() {
            return Ok(None);
        }

        let instruction_blocks: Vec<String> = clients_with_instructions
            .into_iter()
            .map(|client| {
                let instructions = client.instructions.as_deref().unwrap_or("");
                format!("## {}\n{}", client.name, instructions)
            })
            .collect();

        Ok(Some(format!(
            "# MCP Server Instructions\n\n\
            The following MCP servers have provided instructions for how to use their tools and resources:\n\n\
            {}",
            instruction_blocks.join("\n\n")
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn test_ctx_with_mcp(clients: Vec<McpClientInfo>) -> PromptContext {
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
            mcp_clients: clients,
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
    fn mcp_instructions_empty_clients() {
        let section = McpInstructionsSection;
        let result = section
            .compute(&test_ctx_with_mcp(vec![]))
            .expect("compute ok");
        assert!(result.is_none());
    }

    #[test]
    fn mcp_instructions_with_client() {
        let clients = vec![McpClientInfo {
            name: "test-server".to_string(),
            instructions: Some("Use tools carefully.".to_string()),
        }];
        let section = McpInstructionsSection;
        let result = section
            .compute(&test_ctx_with_mcp(clients))
            .expect("compute ok");
        let content = result.expect("should be Some");
        assert!(content.starts_with("# MCP Server Instructions"));
        assert!(content.contains("test-server"));
        assert!(content.contains("Use tools carefully."));
    }

    #[test]
    fn mcp_instructions_client_without_instructions() {
        let clients = vec![McpClientInfo {
            name: "no-instructions".to_string(),
            instructions: None,
        }];
        let section = McpInstructionsSection;
        let result = section
            .compute(&test_ctx_with_mcp(clients))
            .expect("compute ok");
        assert!(result.is_none());
    }

    #[test]
    fn mcp_instructions_ignores_empty_instruction_block() {
        let clients = vec![McpClientInfo {
            name: "empty".to_string(),
            instructions: Some(String::new()),
        }];
        let section = McpInstructionsSection;
        let result = section
            .compute(&test_ctx_with_mcp(clients))
            .expect("compute ok");
        assert!(result.is_none());
    }

    #[test]
    fn mcp_instructions_not_cacheable() {
        let section = McpInstructionsSection;
        assert!(!section.is_cacheable());
    }

    #[test]
    fn mcp_instructions_suppressed_when_delta_mode_enabled() {
        let mut ctx = test_ctx_with_mcp(vec![McpClientInfo {
            name: "delta".to_string(),
            instructions: Some("Use delta mode.".to_string()),
        }]);
        ctx.mcp_instructions_delta_enabled = true;

        let section = McpInstructionsSection;
        let result = section.compute(&ctx).expect("compute ok");
        assert!(result.is_none());
    }
}
