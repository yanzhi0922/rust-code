//! Session-specific Guidance section - dynamic guidance based on enabled tools.
//!
//! Matches `getSessionSpecificGuidanceSection()` in Claude Code's `prompts.ts`.

use anyhow::Result;

use crate::PromptContext;
use crate::sections::{BulletItem, SystemPromptSection, section_with_bullets};

/// Tool name constants.
const ASK_USER_QUESTION_TOOL_NAME: &str = "AskUserQuestion";
const AGENT_TOOL_NAME: &str = "Agent";
const SKILL_TOOL_NAME: &str = "Skill";
const DISCOVER_SKILLS_TOOL_NAME: &str = "discover_skills";
const BASH_TOOL_NAME: &str = "Bash";
const GLOB_TOOL_NAME: &str = "Glob";
const GREP_TOOL_NAME: &str = "Grep";
const EXPLORE_AGENT_TYPE: &str = "Explore";
const EXPLORE_AGENT_MIN_QUERIES: usize = 3;
const VERIFICATION_AGENT_TYPE: &str = "verification";

/// The session-specific guidance section.
pub struct SessionGuidanceSection;

impl SystemPromptSection for SessionGuidanceSection {
    fn name(&self) -> &str {
        "session_guidance"
    }

    fn compute(&self, ctx: &PromptContext) -> Result<Option<String>> {
        let has_ask = ctx.enabled_tools.contains(ASK_USER_QUESTION_TOOL_NAME);
        let has_agent = ctx.enabled_tools.contains(AGENT_TOOL_NAME);
        let has_skills = ctx.features.user_invocable_skills_available
            && ctx.enabled_tools.contains(SKILL_TOOL_NAME);
        let has_discover_skills =
            has_skills && ctx.enabled_tools.contains(DISCOVER_SKILLS_TOOL_NAME);
        let search_tools = if ctx.features.embedded_search_tools {
            format!("`find` or `grep` via the {BASH_TOOL_NAME} tool")
        } else {
            format!("the {GLOB_TOOL_NAME} or {GREP_TOOL_NAME}")
        };

        let mut items: Vec<BulletItem> = Vec::new();

        if has_ask {
            items.push(BulletItem::Single(format!(
                "If you do not understand why the user has denied a tool call, use the {ASK_USER_QUESTION_TOOL_NAME} to ask them."
            )));
        }

        if !ctx.is_non_interactive {
            items.push(BulletItem::Single(
                "If you need the user to run a shell command themselves (e.g., an interactive login like `gcloud auth login`), suggest they type `! <command>` in the prompt \u{2014} the `!` prefix runs the command in this session so its output lands directly in the conversation.".to_string(),
            ));
        }

        if has_agent {
            if ctx.is_fork_subagent_enabled {
                items.push(BulletItem::Single(format!(
                    "Calling {AGENT_TOOL_NAME} without a subagent_type creates a fork, which runs in the background and keeps its tool output out of your context \u{2014} so you can keep chatting with the user while it works. Reach for it when research or multi-step implementation work would otherwise fill your context with raw output you won't need again. **If you ARE the fork** \u{2014} execute directly; do not re-delegate."
                )));
            } else {
                items.push(BulletItem::Single(format!(
                    "Use the {AGENT_TOOL_NAME} tool with specialized agents when the task at hand matches the agent's description. Subagents are valuable for parallelizing independent queries or for protecting the main context window from excessive results, but they should not be used excessively when not needed. Importantly, avoid duplicating work that subagents are already doing - if you delegate research to a subagent, do not also perform the same searches yourself."
                )));
            }
        }

        if has_agent && ctx.features.explore_plan_agents_enabled && !ctx.is_fork_subagent_enabled {
            items.push(BulletItem::Single(format!(
                "For simple, directed codebase searches (e.g. for a specific file/class/function) use {search_tools} directly."
            )));
            items.push(BulletItem::Single(format!(
                "For broader codebase exploration and deep research, use the {AGENT_TOOL_NAME} tool with subagent_type={EXPLORE_AGENT_TYPE}. This is slower than using {search_tools} directly, so use this only when a simple, directed search proves to be insufficient or when your task will clearly require more than {EXPLORE_AGENT_MIN_QUERIES} queries."
            )));
        }

        if has_skills {
            items.push(BulletItem::Single(format!(
                "/<skill-name> (e.g., /commit) is shorthand for users to invoke a user-invocable skill. When executed, the skill gets expanded to a full prompt. Use the {SKILL_TOOL_NAME} tool to execute them. IMPORTANT: Only use {SKILL_TOOL_NAME} for skills listed in its user-invocable skills section - do not guess or use built-in CLI commands."
            )));
        }

        if has_discover_skills {
            items.push(BulletItem::Single(format!(
                "Relevant skills are automatically surfaced each turn as \"Skills relevant to your task:\" reminders. If you're about to do something those don't cover \u{2014} a mid-task pivot, an unusual workflow, a multi-step plan \u{2014} call {DISCOVER_SKILLS_TOOL_NAME} with a specific description of what you're doing. Skills already visible or loaded are filtered automatically. Skip this if the surfaced skills already cover your next action."
            )));
        }

        if has_agent && ctx.features.verification_agent_enabled {
            items.push(BulletItem::Single(format!(
                "The contract: when non-trivial implementation happens on your turn, independent adversarial verification must happen before you report completion \u{2014} regardless of who did the implementing (you directly, a fork you spawned, or a subagent). You are the one reporting to the user; you own the gate. Non-trivial means: 3+ file edits, backend/API changes, or infrastructure changes. Spawn the {AGENT_TOOL_NAME} tool with subagent_type=\"{VERIFICATION_AGENT_TYPE}\". Your own checks, caveats, and a fork's self-checks do NOT substitute \u{2014} only the verifier assigns a verdict; you cannot self-assign PARTIAL. Pass the original user request, all files changed (by anyone), the approach, and the plan file path if applicable. Flag concerns if you have them but do NOT share test results or claim things work. On FAIL: fix, resume the verifier with its findings plus your fix, repeat until PASS. On PASS: spot-check it \u{2014} re-run 2-3 commands from its report, confirm every PASS has a Command run block with output that matches your re-run. If any PASS lacks a command block or diverges, resume the verifier with the specifics. On PARTIAL (from the verifier): report what passed and what could not be verified."
            )));
        }

        if items.is_empty() {
            return Ok(None);
        }

        Ok(Some(section_with_bullets(
            "Session-specific guidance",
            &items,
        )))
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
    fn session_guidance_with_ask_tool() {
        let section = SessionGuidanceSection;
        let result = section
            .compute(&test_ctx_with_tools(&[ASK_USER_QUESTION_TOOL_NAME]))
            .expect("compute ok");
        let content = result.expect("should be Some");
        assert!(content.contains(ASK_USER_QUESTION_TOOL_NAME));
    }

    #[test]
    fn session_guidance_with_agent_tool() {
        let section = SessionGuidanceSection;
        let result = section
            .compute(&test_ctx_with_tools(&[AGENT_TOOL_NAME]))
            .expect("compute ok");
        let content = result.expect("should be Some");
        assert!(content.contains(AGENT_TOOL_NAME));
    }

    #[test]
    fn session_guidance_empty_when_no_tools() {
        let mut ctx = test_ctx_with_tools(&[]);
        ctx.is_non_interactive = true;
        let section = SessionGuidanceSection;
        let result = section.compute(&ctx).expect("compute ok");
        assert!(result.is_none());
    }

    #[test]
    fn session_guidance_skill_bullet_requires_available_skills() {
        let mut ctx = test_ctx_with_tools(&[SKILL_TOOL_NAME]);
        ctx.is_non_interactive = true;
        let section = SessionGuidanceSection;
        let result = section.compute(&ctx).expect("compute ok");
        assert!(result.is_none());
    }

    #[test]
    fn session_guidance_fork_mode() {
        let mut ctx = test_ctx_with_tools(&[AGENT_TOOL_NAME]);
        ctx.is_fork_subagent_enabled = true;
        let section = SessionGuidanceSection;
        let result = section.compute(&ctx).expect("compute ok");
        let content = result.expect("should be Some");
        assert!(content.contains("fork"));
    }

    #[test]
    fn session_guidance_matches_research_cacheability() {
        let section = SessionGuidanceSection;
        assert!(section.is_cacheable());
    }

    #[test]
    fn session_guidance_discover_skills_guidance_requires_skill_inventory() {
        let mut ctx = test_ctx_with_tools(&[SKILL_TOOL_NAME, DISCOVER_SKILLS_TOOL_NAME]);
        ctx.features.user_invocable_skills_available = true;
        ctx.is_non_interactive = true;
        let section = SessionGuidanceSection;
        let result = section.compute(&ctx).expect("compute ok");
        let content = result.expect("should be Some");
        assert!(content.contains("Relevant skills are automatically surfaced"));
    }
}
