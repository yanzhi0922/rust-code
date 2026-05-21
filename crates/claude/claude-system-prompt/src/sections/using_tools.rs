//! Using Your Tools section — guidance on tool usage preferences.
//!
//! Matches `getUsingYourToolsSection()` in Claude Code's `prompts.ts`.

use anyhow::Result;

use crate::PromptContext;
use crate::sections::{BulletItem, SystemPromptSection, section_with_bullets};

/// Tool name constants.
const FILE_READ_TOOL_NAME: &str = "Read";
const FILE_EDIT_TOOL_NAME: &str = "Edit";
const FILE_WRITE_TOOL_NAME: &str = "Write";
const GLOB_TOOL_NAME: &str = "Glob";
const GREP_TOOL_NAME: &str = "Grep";
const BASH_TOOL_NAME: &str = "Bash";
const TASK_CREATE_TOOL_NAME: &str = "Task";
const TODO_WRITE_TOOL_NAME: &str = "TodoWrite";

/// The "Using your tools" section.
pub struct UsingToolsSection;

fn has_tool(ctx: &PromptContext, candidates: &[&str]) -> bool {
    candidates
        .iter()
        .any(|name| ctx.enabled_tools.contains(*name))
}

impl SystemPromptSection for UsingToolsSection {
    fn name(&self) -> &str {
        "using_tools"
    }

    fn compute(&self, ctx: &PromptContext) -> Result<Option<String>> {
        let task_tool_name = if has_tool(ctx, &[TASK_CREATE_TOOL_NAME, "task_create", "TaskCreate"])
        {
            Some(TASK_CREATE_TOOL_NAME)
        } else if has_tool(ctx, &[TODO_WRITE_TOOL_NAME, "todo_write"]) {
            Some(TODO_WRITE_TOOL_NAME)
        } else {
            None
        };

        let mut items: Vec<BulletItem> = Vec::new();

        if ctx.features.repl_mode_active {
            if let Some(task_name) = task_tool_name {
                items.push(BulletItem::Single(format!(
                    "Break down and manage your work with the {task_name} tool. These tools are helpful for planning your work and helping the user track your progress. Mark each task as completed as soon as you are done with the task. Do not batch up multiple tasks before marking them as completed."
                )));
            }

            return Ok(
                (!items.is_empty()).then(|| section_with_bullets("Using your tools", &items))
            );
        }

        let mut provided_tool_subitems = vec![
            format!("To read files use {FILE_READ_TOOL_NAME} instead of cat, head, tail, or sed"),
            format!("To edit files use {FILE_EDIT_TOOL_NAME} instead of sed or awk"),
            format!(
                "To create files use {FILE_WRITE_TOOL_NAME} instead of cat with heredoc or echo redirection"
            ),
        ];

        if !ctx.features.embedded_search_tools {
            provided_tool_subitems.push(format!(
                "To search for files use {GLOB_TOOL_NAME} instead of find or ls"
            ));
            provided_tool_subitems.push(format!(
                "To search the content of files, use {GREP_TOOL_NAME} instead of grep or rg"
            ));
        }

        provided_tool_subitems.push(format!(
            "Reserve using the {BASH_TOOL_NAME} exclusively for system commands and terminal operations that require shell execution. If you are unsure and there is a relevant dedicated tool, default to using the dedicated tool and only fallback on using the {BASH_TOOL_NAME} tool for these if it is absolutely necessary."
        ));

        items.push(BulletItem::Single(format!(
            "Do NOT use the {BASH_TOOL_NAME} to run commands when a relevant dedicated tool is provided. Using dedicated tools allows the user to better understand and review your work. This is CRITICAL to assisting the user:"
        )));
        items.push(BulletItem::Nested(provided_tool_subitems));

        if let Some(task_name) = task_tool_name {
            items.push(BulletItem::Single(format!(
                "Break down and manage your work with the {task_name} tool. These tools are helpful for planning your work and helping the user track your progress. Mark each task as completed as soon as you are done with the task. Do not batch up multiple tasks before marking them as completed."
            )));
        }

        items.push(BulletItem::Single(
            "You can call multiple tools in a single response. If you intend to call multiple tools and there are no dependencies between them, make all independent tool calls in parallel. Maximize use of parallel tool calls where possible to increase efficiency. However, if some tool calls depend on previous calls to inform dependent values, do NOT call these tools in parallel and instead call them sequentially. For instance, if one operation must complete before another starts, run these operations sequentially instead.".to_string(),
        ));

        if items.is_empty() {
            return Ok(None);
        }

        Ok(Some(section_with_bullets("Using your tools", &items)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_ctx_with_tools(tools: &[&str]) -> PromptContext {
        PromptContext {
            model: "test".to_string(),
            cwd: PathBuf::from("/tmp"),
            is_git: false,
            platform: "linux".to_string(),
            shell: "bash".to_string(),
            os_version: "Linux 6.6".to_string(),
            enabled_tools: tools.iter().map(|s| s.to_string()).collect(),
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
    fn using_tools_starts_with_header() {
        let section = UsingToolsSection;
        let result = section
            .compute(&test_ctx_with_tools(&[FILE_READ_TOOL_NAME, BASH_TOOL_NAME]))
            .expect("compute ok");
        let content = result.expect("should be Some");
        assert!(content.starts_with("# Using your tools"));
    }

    #[test]
    fn using_tools_mentions_dedicated_tools() {
        let section = UsingToolsSection;
        let result = section
            .compute(&test_ctx_with_tools(&[
                FILE_READ_TOOL_NAME,
                FILE_EDIT_TOOL_NAME,
                FILE_WRITE_TOOL_NAME,
                BASH_TOOL_NAME,
            ]))
            .expect("compute ok");
        let content = result.expect("should be Some");
        assert!(content.contains(FILE_READ_TOOL_NAME));
        assert!(content.contains(FILE_EDIT_TOOL_NAME));
        assert!(content.contains(FILE_WRITE_TOOL_NAME));
    }

    #[test]
    fn using_tools_with_task_tool() {
        let section = UsingToolsSection;
        let result = section
            .compute(&test_ctx_with_tools(&[TASK_CREATE_TOOL_NAME]))
            .expect("compute ok");
        let content = result.expect("should be Some");
        assert!(content.contains(TASK_CREATE_TOOL_NAME));
        assert!(content.contains("Break down and manage"));
    }

    #[test]
    fn using_tools_mentions_parallel_execution() {
        let section = UsingToolsSection;
        let result = section
            .compute(&test_ctx_with_tools(&[BASH_TOOL_NAME]))
            .expect("compute ok");
        let content = result.expect("should be Some");
        assert!(content.contains("parallel"));
    }

    #[test]
    fn using_tools_lists_dedicated_guidance_by_default() {
        let section = UsingToolsSection;
        let result = section
            .compute(&test_ctx_with_tools(&[BASH_TOOL_NAME]))
            .expect("compute ok");
        let content = result.expect("should be Some");
        assert!(content.contains(FILE_READ_TOOL_NAME));
        assert!(content.contains(FILE_EDIT_TOOL_NAME));
        assert!(content.contains(FILE_WRITE_TOOL_NAME));
        assert!(content.contains(BASH_TOOL_NAME));
    }

    #[test]
    fn using_tools_repl_mode_only_keeps_task_guidance() {
        let section = UsingToolsSection;
        let mut ctx = test_ctx_with_tools(&[TASK_CREATE_TOOL_NAME]);
        ctx.features.repl_mode_active = true;
        let result = section.compute(&ctx).expect("compute ok");
        let content = result.expect("should be Some");
        assert!(content.contains(TASK_CREATE_TOOL_NAME));
        assert!(!content.contains(FILE_READ_TOOL_NAME));
        assert!(!content.contains(FILE_EDIT_TOOL_NAME));
        assert!(!content.contains(FILE_WRITE_TOOL_NAME));
    }
}
