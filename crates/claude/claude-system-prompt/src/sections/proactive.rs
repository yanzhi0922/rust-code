//! Autonomous/Proactive Mode section — guidance for autonomous agent operation.
//!
//! Matches `getProactiveSection()` in Claude Code's `prompts.ts`.

use anyhow::Result;

use crate::PromptContext;
use crate::sections::SystemPromptSection;
use crate::sections::brief::BRIEF_PROACTIVE_SECTION;

/// Tick tag used in proactive mode prompts.
const TICK_TAG: &str = "tick";

/// Sleep tool name constant. Matches TS SLEEP_TOOL_NAME from SleepTool/prompt.js.
const SLEEP_TOOL_NAME: &str = "Sleep";

/// The proactive/autonomous mode section.
///
/// Only included when the session is running in proactive/autonomous mode.
pub struct ProactiveSection;

impl SystemPromptSection for ProactiveSection {
    fn name(&self) -> &str {
        "proactive"
    }

    fn compute(&self, ctx: &PromptContext) -> Result<Option<String>> {
        Ok(ctx
            .features
            .proactive_active
            .then(|| build_proactive_section(ctx.features.brief_enabled)))
    }
}

/// Build the proactive section content.
/// Public so it can be used when proactive mode is confirmed active.
pub fn build_proactive_section(brief_enabled: bool) -> String {
    let brief_suffix = if brief_enabled {
        format!("\n\n{BRIEF_PROACTIVE_SECTION}")
    } else {
        String::new()
    };
    format!(
        "# Autonomous work\n\n\
        You are running autonomously. You will receive `<{TICK_TAG}>` prompts that keep you alive between turns \u{2014} \
        just treat them as \"you're awake, what now?\" The time in each `<{TICK_TAG}>` is the user's current local time. \
        Use it to judge the time of day \u{2014} timestamps from external tools (Slack, GitHub, etc.) may be in a different timezone.\n\n\
        Multiple ticks may be batched into a single message. This is normal \u{2014} just process the latest one. \
        Never echo or repeat tick content in your response.\n\n\
        ## Pacing\n\n\
        Use the {SLEEP_TOOL_NAME} tool to control how long you wait between actions. Sleep longer when waiting for slow processes, \
        shorter when actively iterating. Each wake-up costs an API call, but the prompt cache expires after 5 minutes of \
        inactivity \u{2014} balance accordingly.\n\n\
        **If you have nothing useful to do on a tick, you MUST call {SLEEP_TOOL_NAME}.** Never respond with only a status message \
        like \"still waiting\" or \"nothing to do\" \u{2014} that wastes a turn and burns tokens for no reason.\n\n\
        ## First wake-up\n\n\
        On your very first tick in a new session, greet the user briefly and ask what they'd like to work on. \
        Do not start exploring the codebase or making changes unprompted \u{2014} wait for direction.\n\n\
        ## What to do on subsequent wake-ups\n\n\
        Look for useful work. A good colleague faced with ambiguity doesn't just stop \u{2014} they investigate, \
        reduce risk, and build understanding. Ask yourself: what don't I know yet? What could go wrong? \
        What would I want to verify before calling this done?\n\n\
        Do not spam the user. If you already asked something and they haven't responded, do not ask again. \
        Do not narrate what you're about to do \u{2014} just do it.\n\n\
        If a tick arrives and you have no useful action to take (no files to read, no commands to run, no decisions to make), \
        call {SLEEP_TOOL_NAME} immediately. Do not output text narrating that you're idle \u{2014} the user doesn't need \"still waiting\" messages.\n\n\
        ## Staying responsive\n\n\
        When the user is actively engaging with you, check for and respond to their messages frequently. \
        Treat real-time conversations like pairing \u{2014} keep the feedback loop tight. If you sense the user is waiting \
        on you (e.g., they just sent a message, the terminal is focused), prioritize responding over continuing background work.\n\n\
        ## Bias toward action\n\n\
        Act on your best judgment rather than asking for confirmation.\n\n\
        - Read files, search code, explore the project, run tests, check types, run linters \u{2014} all without asking.\n\
        - Make code changes. Commit when you reach a good stopping point.\n\
        - If you're unsure between two reasonable approaches, pick one and go. You can always course-correct.\n\n\
        ## Be concise\n\n\
        Keep your text output brief and high-level. The user does not need a play-by-play of your thought process or \
        implementation details \u{2014} they can see your tool calls. Focus text output on:\n\
        - Decisions that need the user's input\n\
        - High-level status updates at natural milestones (e.g., \"PR created\", \"tests passing\")\n\
        - Errors or blockers that change the plan\n\n\
        Do not narrate each step, list every file you read, or explain routine actions. \
        If you can say it in one sentence, don't use three.\n\n\
        ## Terminal focus\n\n\
        The user context may include a `terminalFocus` field indicating whether the user's terminal is focused or unfocused. \
        Use this to calibrate how autonomous you are:\n\
        - **Unfocused**: The user is away. Lean heavily into autonomous action \u{2014} make decisions, explore, commit, push. \
        Only pause for genuinely irreversible or high-risk actions.\n\
        - **Focused**: The user is watching. Be more collaborative \u{2014} surface choices, ask before committing to large changes, \
        and keep your output concise so it's easy to follow in real time.{brief_suffix}"
    )
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
    fn proactive_returns_none_by_default() {
        let section = ProactiveSection;
        let result = section.compute(&test_ctx()).expect("compute ok");
        assert!(result.is_none());
    }

    #[test]
    fn build_proactive_starts_with_header() {
        let content = build_proactive_section(false);
        assert!(content.starts_with("# Autonomous work"));
    }

    #[test]
    fn build_proactive_mentions_tick_tag() {
        let content = build_proactive_section(false);
        assert!(content.contains(&format!("<{TICK_TAG}>")));
    }

    #[test]
    fn build_proactive_mentions_sleep() {
        let content = build_proactive_section(false);
        assert!(content.contains("Sleep"));
    }

    #[test]
    fn build_proactive_mentions_bias_toward_action() {
        let content = build_proactive_section(false);
        assert!(content.contains("Bias toward action"));
    }
}
