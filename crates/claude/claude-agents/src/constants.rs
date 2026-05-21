//! Agent tool constants matching Claude Code's `AgentTool/constants.ts`.

/// The tool name used for the Agent tool in the tool schema.
pub const AGENT_TOOL_NAME: &str = "Agent";

/// Legacy wire name for backward compatibility (permission rules, hooks, resumed sessions).
pub const LEGACY_AGENT_TOOL_NAME: &str = "Task";

/// Synthetic agent type name used for analytics when the fork path fires.
pub const FORK_SUBAGENT_TYPE: &str = "fork";

/// Verification agent type identifier.
pub const VERIFICATION_AGENT_TYPE: &str = "verification";

/// Default maximum number of agentic turns before stopping.
pub const MAX_AGENT_TURNS: u32 = 200;

/// Placeholder text used for all tool_result blocks in the fork prefix.
/// Must be identical across all fork children for prompt cache sharing.
pub const FORK_PLACEHOLDER_RESULT: &str = "Fork started — processing in background";

/// XML tag used to mark fork boilerplate in messages.
pub const FORK_BOILERPLATE_TAG: &str = "fork-boilerplate";

/// Prefix for fork directive messages.
pub const FORK_DIRECTIVE_PREFIX: &str = "Your directive: ";

/// Built-in agent types that run once and return a report.
/// The parent never sends messages back to continue them.
pub const ONE_SHOT_BUILTIN_AGENT_TYPES: &[&str] = &["Explore", "Plan"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_tool_name_is_agent() {
        assert_eq!(AGENT_TOOL_NAME, "Agent");
    }

    #[test]
    fn legacy_tool_name_is_task() {
        assert_eq!(LEGACY_AGENT_TOOL_NAME, "Task");
    }

    #[test]
    fn fork_subagent_type_constant() {
        assert_eq!(FORK_SUBAGENT_TYPE, "fork");
    }

    #[test]
    fn fork_boilerplate_tag_matches_research() {
        assert_eq!(FORK_BOILERPLATE_TAG, "fork-boilerplate");
        assert_eq!(FORK_DIRECTIVE_PREFIX, "Your directive: ");
    }

    #[test]
    fn max_agent_turns_default() {
        assert_eq!(MAX_AGENT_TURNS, 200);
    }

    #[test]
    fn one_shot_types_contains_explore_and_plan() {
        assert!(ONE_SHOT_BUILTIN_AGENT_TYPES.contains(&"Explore"));
        assert!(ONE_SHOT_BUILTIN_AGENT_TYPES.contains(&"Plan"));
        assert!(!ONE_SHOT_BUILTIN_AGENT_TYPES.contains(&"general-purpose"));
    }
}
