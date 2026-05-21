use anyhow::Result;

use crate::PromptContext;
use crate::sections::env_info::compute_env_info_xml;

pub const DEFAULT_AGENT_PROMPT: &str = "You are an agent for Claude Code, Anthropic's official CLI for Claude. Given the user's message, you should use the tools available to complete the task. Complete the task fully\u{2014}don't gold-plate, but don't leave it half-done. When you complete the task, respond with a concise report covering what was done and any key findings \u{2014} the caller will relay this to the user, so it only needs the essentials.";

const SUBAGENT_NOTES: &str = "Notes:
 - Agent threads always have their cwd reset between bash calls, as a result please only use absolute file paths.
 - In your final response, share file paths (always absolute, never relative) that are relevant to the task. Include code snippets only when the exact text is load-bearing (e.g., a bug you found, a function signature the caller asked for) \u{2014} do not recap code you merely read.
 - For clear communication with the user the assistant MUST avoid using emojis.
 - Do not use a colon before tool calls. Text like \"Let me read the file:\" followed by a read tool call should just be \"Let me read the file.\" with a period.";

/// DiscoverSkills guidance for subagents.
///
/// Matches `getDiscoverSkillsGuidance()` in `prompts.ts`. Subagents receive
/// skill_discovery attachments but don't go through the full system prompt
/// assembly, so they need explicit framing for skill discovery.
const DISCOVER_SKILLS_GUIDANCE: &str = "Relevant skills are automatically surfaced each turn as \"Skills relevant to your task:\" reminders. If you're about to do something those don't cover \u{2014} a mid-task pivot, an unusual workflow, a multi-step plan \u{2014} call the skill discovery tool with a specific description of what you're doing. Skills already visible or loaded are filtered automatically. Skip this if the surfaced skills already cover your next action.";

/// Whether DiscoverSkills guidance should be included.
/// Matches the TS feature gate: feature('EXPERIMENTAL_SKILL_SEARCH').
fn is_skill_search_enabled() -> bool {
    std::env::var("EXPERIMENTAL_SKILL_SEARCH")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

pub fn enhance_system_prompt_with_env_details(
    existing_system_prompt: Vec<String>,
    ctx: &PromptContext,
) -> Result<Vec<String>> {
    let mut result = existing_system_prompt;
    result.push(SUBAGENT_NOTES.to_string());

    // Subagents get skill_discovery attachments but don't go through
    // getSystemPrompt — surface the same DiscoverSkills framing.
    if is_skill_search_enabled() {
        result.push(DISCOVER_SKILLS_GUIDANCE.to_string());
    }

    let env_info = compute_env_info_xml(ctx);
    result.push(env_info);

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn test_ctx() -> PromptContext {
        PromptContext {
            model: "claude-sonnet-4-6".to_string(),
            cwd: PathBuf::from("/home/user/project"),
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
    fn default_agent_prompt_not_empty() {
        assert!(!DEFAULT_AGENT_PROMPT.is_empty());
        assert!(DEFAULT_AGENT_PROMPT.contains("agent for Claude Code"));
    }

    #[test]
    fn enhance_appends_notes_and_env() {
        let existing = vec!["system prompt".to_string()];
        let result =
            enhance_system_prompt_with_env_details(existing, &test_ctx()).expect("should succeed");
        assert_eq!(result.len(), 3);
        assert_eq!(result[0], "system prompt");
        assert!(result[1].contains("Agent threads"));
        assert!(result[2].contains("<env>"));
    }

    #[test]
    fn enhance_notes_mention_absolute_paths() {
        let result =
            enhance_system_prompt_with_env_details(vec![], &test_ctx()).expect("should succeed");
        assert!(result[0].contains("absolute file paths"));
    }
}
