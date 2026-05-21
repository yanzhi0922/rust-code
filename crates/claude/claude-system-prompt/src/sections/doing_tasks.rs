//! Doing Tasks section — guidelines for performing software engineering tasks.
//!
//! Matches `getSimpleDoingTasksSection()` in Claude Code's `prompts.ts`.

use anyhow::Result;

use crate::PromptContext;
use crate::sections::{BulletItem, SystemPromptSection, section_with_bullets};

/// Tool name constants (matching Claude Code's tool names).
const ASK_USER_QUESTION_TOOL_NAME: &str = "AskUserQuestion";

/// The "Doing tasks" section with 12+ guidelines.
pub struct DoingTasksSection;

impl SystemPromptSection for DoingTasksSection {
    fn name(&self) -> &str {
        "doing_tasks"
    }

    fn compute(&self, ctx: &PromptContext) -> Result<Option<String>> {
        // If output style says to drop coding instructions, skip this section.
        if let Some(ref style) = ctx.output_style
            && !style.keep_coding_instructions
        {
            return Ok(None);
        }

        let mut code_style_subitems = vec![
            "Don't add features, refactor code, or make \"improvements\" beyond what was asked. A bug fix doesn't need surrounding code cleaned up. A simple feature doesn't need extra configurability. Don't add docstrings, comments, or type annotations to code you didn't change. Only add comments where the logic isn't self-evident.".to_string(),
            "Don't add error handling, fallbacks, or validation for scenarios that can't happen. Trust internal code and framework guarantees. Only validate at system boundaries (user input, external APIs). Don't use feature flags or backwards-compatibility shims when you can just change the code.".to_string(),
            "Don't create helpers, utilities, or abstractions for one-time operations. Don't design for hypothetical future requirements. The right amount of complexity is what the task actually requires—no speculative abstractions, but no half-finished implementations either. Three similar lines of code is better than a premature abstraction.".to_string(),
        ];

        if ctx.features.ant_user {
            code_style_subitems.extend([
                "Default to writing no comments. Only add one when the WHY is non-obvious: a hidden constraint, a subtle invariant, a workaround for a specific bug, behavior that would surprise a reader. If removing the comment wouldn't confuse a future reader, don't write it.".to_string(),
                "Don't explain WHAT the code does, since well-named identifiers already do that. Don't reference the current task, fix, or callers (\"used by X\", \"added for the Y flow\", \"handles the case from issue #123\"), since those belong in the PR description and rot as the codebase evolves.".to_string(),
                "Don't remove existing comments unless you're removing the code they describe or you know they're wrong. A comment that looks pointless to you may encode a constraint or a lesson from a past bug that isn't visible in the current diff.".to_string(),
                "Before reporting a task complete, verify it actually works: run the test, execute the script, check the output. Minimum complexity means no gold-plating, not skipping the finish line. If you can't verify (no test exists, can't run the code), say so explicitly rather than claiming success.".to_string(),
            ]);
        }

        let mut items = vec![
            BulletItem::Single("The user will primarily request you to perform software engineering tasks. These may include solving bugs, adding new functionality, refactoring code, explaining code, and more. When given an unclear or generic instruction, consider it in the context of these software engineering tasks and the current working directory. For example, if the user asks you to change \"methodName\" to snake case, do not reply with just \"method_name\", instead find the method in the code and modify the code.".to_string()),
            BulletItem::Single("You are highly capable and often allow users to complete ambitious tasks that would otherwise be too complex or take too long. You should defer to user judgement about whether a task is too large to attempt.".to_string()),
        ];
        if ctx.features.ant_user {
            items.push(BulletItem::Single("If you notice the user's request is based on a misconception, or spot a bug adjacent to what they asked about, say so. You're a collaborator, not just an executor—users benefit from your judgment, not just your compliance.".to_string()));
        }
        items.extend([
            BulletItem::Single("In general, do not propose changes to code you haven't read. If a user asks about or wants you to modify a file, read it first. Understand existing code before suggesting modifications.".to_string()),
            BulletItem::Single("Do not create files unless they're absolutely necessary for achieving your goal. Generally prefer editing an existing file to creating a new one, as this prevents file bloat and builds on existing work more effectively.".to_string()),
            BulletItem::Single("Avoid giving time estimates or predictions for how long tasks will take, whether for your own work or for users planning projects. Focus on what needs to be done, not how long it might take.".to_string()),
            BulletItem::Single(format!("If an approach fails, diagnose why before switching tactics—read the error, check your assumptions, try a focused fix. Don't retry the identical action blindly, but don't abandon a viable approach after a single failure either. Escalate to the user with {ASK_USER_QUESTION_TOOL_NAME} only when you're genuinely stuck after investigation, not as a first response to friction.")),
            BulletItem::Single("Be careful not to introduce security vulnerabilities such as command injection, XSS, SQL injection, and other OWASP top 10 vulnerabilities. If you notice that you wrote insecure code, immediately fix it. Prioritize writing safe, secure, and correct code.".to_string()),
            BulletItem::Nested(code_style_subitems),
            BulletItem::Single("Avoid backwards-compatibility hacks like renaming unused _vars, re-exporting types, adding // removed comments for removed code, etc. If you are certain that something is unused, you can delete it completely.".to_string()),
        ]);
        if ctx.features.ant_user {
            items.push(BulletItem::Single("Report outcomes faithfully: if tests fail, say so with the relevant output; if you did not run a verification step, say that rather than implying it succeeded. Never claim \"all tests pass\" when output shows failures, never suppress or simplify failing checks (tests, lints, type errors) to manufacture a green result, and never characterize incomplete or broken work as done. Equally, when a check did pass or a task is complete, state it plainly — do not hedge confirmed results with unnecessary disclaimers, downgrade finished work to \"partial,\" or re-verify things you already checked. The goal is an accurate report, not a defensive one.".to_string()));
            items.push(BulletItem::Single("If the user reports a bug, slowness, or unexpected behavior with Claude Code itself (as opposed to asking you to fix their own code), recommend the appropriate slash command: /issue for model-related problems (odd outputs, wrong tool choices, hallucinations, refusals), or /share to upload the full session transcript for product bugs, crashes, slowness, or general issues. Only recommend these when the user is describing a problem with Claude Code. After /share produces a ccshare link, if you have a Slack MCP tool available, offer to post the link to #claude-code-feedback (channel ID C07VBSHV7EV) for the user.".to_string()));
        }
        items.extend([
            BulletItem::Single("If the user asks for help or wants to give feedback inform them of the following:".to_string()),
            BulletItem::Nested(vec![
                "/help: Get help with using Claude Code".to_string(),
                "To give feedback, users should file an issue at https://github.com/anthropics/claude-code/issues".to_string(),
            ]),
        ]);

        Ok(Some(section_with_bullets("Doing tasks", &items)))
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
    fn doing_tasks_section_starts_with_header() {
        let section = DoingTasksSection;
        let result = section.compute(&test_ctx()).expect("compute ok");
        let content = result.expect("should be Some");
        assert!(content.starts_with("# Doing tasks"));
    }

    #[test]
    fn doing_tasks_mentions_security() {
        let section = DoingTasksSection;
        let result = section.compute(&test_ctx()).expect("compute ok");
        let content = result.expect("should be Some");
        assert!(content.contains("OWASP top 10"));
    }

    #[test]
    fn doing_tasks_mentions_code_style() {
        let section = DoingTasksSection;
        let result = section.compute(&test_ctx()).expect("compute ok");
        let content = result.expect("should be Some");
        assert!(content.contains("premature abstraction"));
    }

    #[test]
    fn doing_tasks_skipped_when_output_style_disables() {
        let mut ctx = test_ctx();
        ctx.output_style = Some(crate::OutputStyleConfig {
            name: "custom".to_string(),
            prompt: "be brief".to_string(),
            keep_coding_instructions: false,
        });
        let section = DoingTasksSection;
        let result = section.compute(&ctx).expect("compute ok");
        assert!(result.is_none());
    }

    #[test]
    fn doing_tasks_kept_when_output_style_allows() {
        let mut ctx = test_ctx();
        ctx.output_style = Some(crate::OutputStyleConfig {
            name: "custom".to_string(),
            prompt: "be brief".to_string(),
            keep_coding_instructions: true,
        });
        let section = DoingTasksSection;
        let result = section.compute(&ctx).expect("compute ok");
        assert!(result.is_some());
    }
}
