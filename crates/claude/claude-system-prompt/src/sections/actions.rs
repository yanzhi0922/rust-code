//! Actions with Care section — reversibility and blast radius guidance.
//!
//! Matches `getActionsSection()` in Claude Code's `prompts.ts`.

use anyhow::Result;

use crate::PromptContext;
use crate::sections::SystemPromptSection;

/// The "Executing actions with care" section.
pub struct ActionsSection;

impl SystemPromptSection for ActionsSection {
    fn name(&self) -> &str {
        "actions"
    }

    fn compute(&self, _ctx: &PromptContext) -> Result<Option<String>> {
        Ok(Some(
            "# Executing actions with care\n\n\
            Carefully consider the reversibility and blast radius of actions. \
            Generally you can freely take local, reversible actions like editing files or running tests. \
            But for actions that are hard to reverse, affect shared systems beyond your local environment, \
            or could otherwise be risky or destructive, check with the user before proceeding. \
            The cost of pausing to confirm is low, while the cost of an unwanted action \
            (lost work, unintended messages sent, deleted branches) can be very high. \
            For actions like these, consider the context, the action, and user instructions, \
            and by default transparently communicate the action and ask for confirmation before proceeding. \
            This default can be changed by user instructions - if explicitly asked to operate more autonomously, \
            then you may proceed without confirmation, but still attend to the risks and consequences when taking actions. \
            A user approving an action (like a git push) once does NOT mean that they approve it in all contexts, \
            so unless actions are authorized in advance in durable instructions like CLAUDE.md files, \
            always confirm first. Authorization stands for the scope specified, not beyond. \
            Match the scope of your actions to what was actually requested.\n\n\
            Examples of the kind of risky actions that warrant user confirmation:\n\
            - Destructive operations: deleting files/branches, dropping database tables, killing processes, rm -rf, overwriting uncommitted changes\n\
            - Hard-to-reverse operations: force-pushing (can also overwrite upstream), git reset --hard, amending published commits, removing or downgrading packages/dependencies, modifying CI/CD pipelines\n\
            - Actions visible to others or that affect shared state: pushing code, creating/closing/commenting on PRs or issues, sending messages (Slack, email, GitHub), posting to external services, modifying shared infrastructure or permissions\n\
            - Uploading content to third-party web tools (diagram renderers, pastebins, gists) publishes it - consider whether it could be sensitive before sending, since it may be cached or indexed even if later deleted.\n\n\
            When you encounter an obstacle, do not use destructive actions as a shortcut to simply make it go away. \
            For instance, try to identify root causes and fix underlying issues rather than bypassing safety checks (e.g. --no-verify). \
            If you discover unexpected state like unfamiliar files, branches, or configuration, investigate before deleting or overwriting, \
            as it may represent the user's in-progress work. For example, typically resolve merge conflicts rather than discarding changes; \
            similarly, if a lock file exists, investigate what process holds it rather than deleting it. \
            In short: only take risky actions carefully, and when in doubt, ask before acting. \
            Follow both the spirit and letter of these instructions - measure twice, cut once.".to_string()
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
    fn actions_section_starts_with_header() {
        let section = ActionsSection;
        let result = section.compute(&test_ctx()).expect("compute ok");
        let content = result.expect("should be Some");
        assert!(content.starts_with("# Executing actions with care"));
    }

    #[test]
    fn actions_section_mentions_destructive_operations() {
        let section = ActionsSection;
        let result = section.compute(&test_ctx()).expect("compute ok");
        let content = result.expect("should be Some");
        assert!(content.contains("rm -rf"));
        assert!(content.contains("force-pushing"));
    }

    #[test]
    fn actions_section_mentions_measure_twice() {
        let section = ActionsSection;
        let result = section.compute(&test_ctx()).expect("compute ok");
        let content = result.expect("should be Some");
        assert!(content.contains("measure twice, cut once"));
    }
}
