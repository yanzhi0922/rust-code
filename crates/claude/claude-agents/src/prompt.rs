//! Agent tool prompt builder matching Claude Code's `AgentTool/prompt.ts`.
//!
//! Generates the tool description prompt for the Agent tool, including agent
//! listings, usage notes, fork instructions, and examples.

use crate::constants::AGENT_TOOL_NAME;
use crate::definition::AgentDefinition;

/// Build the complete agent tool prompt.
///
/// This function constructs the prompt text that describes how to use the
/// Agent tool, including available agent types, usage guidelines, and examples.
///
/// # Arguments
/// * `agents` - The list of available agent definitions
/// * `is_fork_enabled` - Whether fork subagent mode is enabled
/// * `is_coordinator` - Whether this is a coordinator-mode prompt (slim version)
/// * `allowed_agent_types` - Optional filter restricting which agents can be spawned
pub fn build_agent_prompt(
    agents: &[AgentDefinition],
    is_fork_enabled: bool,
    is_coordinator: bool,
    allowed_agent_types: Option<&[String]>,
) -> String {
    build_agent_prompt_with_options(
        agents,
        AgentPromptOptions {
            is_fork_enabled,
            is_coordinator,
            allowed_agent_types,
            available_mcp_servers: None,
            denied_agent_types: None,
            list_via_attachment: false,
        },
    )
}

/// Build the complete agent tool prompt with MCP requirement visibility filtering.
///
/// Claude Code keeps `requiredMcpServers` out of loader-time activation and
/// applies it only when presenting available agents to the model. Each required
/// pattern must match at least one MCP server that currently exposes tools.
pub fn build_agent_prompt_with_mcp_servers(
    agents: &[AgentDefinition],
    is_fork_enabled: bool,
    is_coordinator: bool,
    allowed_agent_types: Option<&[String]>,
    available_mcp_servers: Option<&[String]>,
) -> String {
    build_agent_prompt_with_options(
        agents,
        AgentPromptOptions {
            is_fork_enabled,
            is_coordinator,
            allowed_agent_types,
            available_mcp_servers,
            denied_agent_types: None,
            list_via_attachment: false,
        },
    )
}

/// Options controlling how the Agent tool prompt is rendered.
pub struct AgentPromptOptions<'a> {
    pub is_fork_enabled: bool,
    pub is_coordinator: bool,
    pub allowed_agent_types: Option<&'a [String]>,
    pub available_mcp_servers: Option<&'a [String]>,
    pub denied_agent_types: Option<&'a [String]>,
    pub list_via_attachment: bool,
}

/// Build the complete agent tool prompt with full runtime filtering options.
pub fn build_agent_prompt_with_options(
    agents: &[AgentDefinition],
    options: AgentPromptOptions<'_>,
) -> String {
    let effective_agents = visible_agents(
        agents,
        options.allowed_agent_types,
        options.available_mcp_servers,
        options.denied_agent_types,
    );

    let shared = build_shared_prompt(
        &effective_agents,
        options.is_fork_enabled,
        options.list_via_attachment,
    );

    if options.is_coordinator {
        return shared;
    }

    let when_not_to_use = if options.is_fork_enabled {
        String::new()
    } else {
        build_when_not_to_use_section()
    };

    let when_to_fork = if options.is_fork_enabled {
        build_when_to_fork_section()
    } else {
        String::new()
    };

    let writing_the_prompt = build_writing_the_prompt_section(options.is_fork_enabled);

    let examples = if options.is_fork_enabled {
        build_fork_examples()
    } else {
        build_current_examples()
    };

    let background_notes = if options.is_fork_enabled {
        String::new()
    } else {
        "\n\
        - You can optionally run agents in the background using the run_in_background parameter. \
        When an agent runs in the background, you will be automatically notified when it completes \
        — do NOT sleep, poll, or proactively check on its progress. Continue with other work or \
        respond to the user instead.\n\
        - **Foreground vs background**: Use foreground (default) when you need the agent's results \
        before you can proceed — e.g., research agents whose findings inform your next steps. Use \
        background when you have genuinely independent work to do in parallel."
            .to_owned()
    };

    let research_suffix = if options.is_fork_enabled {
        ""
    } else {
        ", since it is not aware of the user's intent"
    };

    format!(
        "{shared}\n{when_not_to_use}\n\n\
        Usage notes:\n\
        - Always include a short description (3-5 words) summarizing what the agent will do\n\
        - Launch multiple agents concurrently whenever possible, to maximize performance; \
        to do that, use a single message with multiple tool uses\n\
        - When the agent is done, it will return a single message back to you. The result \
        returned by the agent is not visible to the user. To show the user the result, you \
        should send a text message back to the user with a concise summary of the result.{background_notes}\n\
        - To continue a previously spawned agent, use SendMessage with the agent's ID or name as the `to` field. \
        The agent resumes with its full context preserved. {continuation_note}\n\
        - The agent's outputs should generally be trusted\n\
        - Clearly tell the agent whether you expect it to write code or just to do research \
        (search, file reads, web fetches, etc.){research_suffix}\n\
        - If the agent description mentions that it should be used proactively, then you should \
        try your best to use it without the user having to ask for it first. Use your judgement.\n\
        - If the user specifies that they want you to run agents \"in parallel\", you MUST send a \
        single message with multiple {AGENT_TOOL_NAME} tool use content blocks. For example, if \
        you need to launch both a build-validator agent and a test-runner agent in parallel, send \
        a single message with both tool calls.\n\
        {when_to_fork}{writing_the_prompt}\n\n\
        {examples}",
        background_notes = background_notes,
        research_suffix = research_suffix,
        continuation_note = if options.is_fork_enabled {
            "Each fresh Agent invocation with a subagent_type starts without context — provide a complete task description."
        } else {
            "Each Agent invocation starts fresh — provide a complete task description."
        }
    )
}

/// Filter agents by MCP requirements, deny rules, then allowed types.
pub fn visible_agents<'a>(
    agents: &'a [AgentDefinition],
    allowed: Option<&[String]>,
    available_mcp_servers: Option<&[String]>,
    denied_agent_types: Option<&[String]>,
) -> Vec<&'a AgentDefinition> {
    agents
        .iter()
        .filter(|agent| {
            available_mcp_servers
                .map(|servers| has_required_mcp_servers(agent, servers))
                .unwrap_or(true)
        })
        .filter(|agent| {
            denied_agent_types
                .map(|types| !types.contains(&agent.agent_type))
                .unwrap_or(true)
        })
        .filter(|agent| {
            allowed
                .map(|types| types.contains(&agent.agent_type))
                .unwrap_or(true)
        })
        .collect()
}

/// Return true when all of an agent's `requiredMcpServers` patterns are met.
///
/// Matching mirrors Claude Code's `loadAgentsDir.ts`: an empty requirement list
/// passes, and every pattern must case-insensitively match some available MCP
/// server name as a substring.
pub fn has_required_mcp_servers(agent: &AgentDefinition, available_mcp_servers: &[String]) -> bool {
    if agent.required_mcp_servers.is_empty() {
        return true;
    }

    agent.required_mcp_servers.iter().all(|pattern| {
        available_mcp_servers
            .iter()
            .any(|server| server_matches_required_pattern(server, pattern))
    })
}

fn server_matches_required_pattern(server_name: &str, pattern: &str) -> bool {
    server_name.to_lowercase().contains(&pattern.to_lowercase())
}

/// Build the shared core prompt used by both coordinator and non-coordinator modes.
fn build_shared_prompt(
    agents: &[&AgentDefinition],
    is_fork_enabled: bool,
    list_via_attachment: bool,
) -> String {
    let agent_list = if list_via_attachment {
        "Available agent types are listed in <system-reminder> messages in the conversation."
            .to_owned()
    } else if agents.is_empty() {
        "No agents available.".to_owned()
    } else {
        agents
            .iter()
            .map(|a| format_agent_line(a))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let fork_or_type = if is_fork_enabled {
        format!(
            "When using the {AGENT_TOOL_NAME} tool, specify a subagent_type to use a specialized \
            agent, or omit it to fork yourself — a fork inherits your full conversation context."
        )
    } else {
        format!(
            "When using the {AGENT_TOOL_NAME} tool, specify a subagent_type parameter to select \
            which agent type to use. If omitted, the general-purpose agent is used."
        )
    };

    format!(
        "Launch a new agent to handle complex, multi-step tasks autonomously.\n\n\
        The {AGENT_TOOL_NAME} tool launches specialized agents (subprocesses) that autonomously \
        handle complex tasks. Each agent type has specific capabilities and tools available to it.\n\n\
        {agent_list_label}\n\
        {agent_list}\n\n\
        {fork_or_type}",
        agent_list_label = if list_via_attachment {
            String::new()
        } else {
            "Available agent types and the tools they have access to:\n".to_owned()
        }
    )
}

/// Build the "When NOT to use" section.
fn build_when_not_to_use_section() -> String {
    format!(
        "\nWhen NOT to use the {AGENT_TOOL_NAME} tool:\n\
        - If you want to read a specific file path, use the Read tool or Glob tool instead, \
        to find the match more quickly\n\
        - If you are searching for a specific class definition like \"class Foo\", use the Grep \
        tool instead, to find the match more quickly\n\
        - If you are searching for code within a specific file or set of 2-3 files, use the Read \
        tool instead of the {AGENT_TOOL_NAME} tool, to find the match more quickly\n\
        - Other tasks that are not related to the agent descriptions above"
    )
}

/// Build the "When to fork" section (fork mode only).
fn build_when_to_fork_section() -> String {
    "\n## When to fork\n\n\
        Fork yourself (omit `subagent_type`) when the intermediate tool output isn't worth \
        keeping in your context. The criterion is qualitative — \"will I need this output again\" \
        — not task size.\n\
        - **Research**: fork open-ended questions. If research can be broken into independent \
        questions, launch parallel forks in one message. A fork beats a fresh subagent for this — \
        it inherits context and shares your cache.\n\
        - **Implementation**: prefer to fork implementation work that requires more than a couple \
        of edits. Do research before jumping to implementation.\n\n\
        Forks are cheap because they share your prompt cache. Don't set `model` on a fork — a \
        different model can't reuse the parent's cache. Pass a short `name` (one or two words, \
        lowercase) so the user can see the fork in the teams panel and steer it mid-run.\n\n\
        **Don't peek.** The tool result includes an `output_file` path — do not Read or tail it \
        unless the user explicitly asks for a progress check. You get a completion notification; \
        trust it.\n\n\
        **Don't race.** After launching, you know nothing about what the fork found. Never fabricate \
        or predict fork results in any format. The notification arrives as a user-role message in \
        a later turn; it is never something you write yourself.\n\n\
        **Writing a fork prompt.** Since the fork inherits your context, the prompt is a *directive* \
        — what to do, not what the situation is. Be specific about scope: what's in, what's out, \
        what another agent is handling. Don't re-explain background.".to_string()
}

/// Build the "Writing the prompt" section.
fn build_writing_the_prompt_section(is_fork_enabled: bool) -> String {
    let prefix = if is_fork_enabled {
        "When spawning a fresh agent (with a `subagent_type`), it starts with zero context. "
    } else {
        ""
    };

    format!(
        "\n## Writing the prompt\n\n\
        {prefix}Brief the agent like a smart colleague who just walked into the room — it hasn't \
        seen this conversation, doesn't know what you've tried, doesn't understand why this task matters.\n\
        - Explain what you're trying to accomplish and why.\n\
        - Describe what you've already learned or ruled out.\n\
        - Give enough context about the surrounding problem that the agent can make judgment calls \
        rather than just following a narrow instruction.\n\
        - If you need a short response, say so (\"report in under 200 words\").\n\
        - Lookups: hand over the exact command. Investigations: hand over the question — prescribed \
        steps become dead weight when the premise is wrong.\n\n\
        {terse_note}\n\n\
        **Never delegate understanding.** Don't write \"based on your findings, fix the bug\" or \
        \"based on the research, implement it.\" Those phrases push synthesis onto the agent instead \
        of doing it yourself. Write prompts that prove you understood: include file paths, line \
        numbers, what specifically to change.",
        prefix = prefix,
        terse_note = if is_fork_enabled {
            "For fresh agents, terse command-style prompts produce shallow, generic work."
        } else {
            "Terse command-style prompts produce shallow, generic work."
        }
    )
}

/// Build fork-mode examples.
fn build_fork_examples() -> String {
    format!(
        "Example usage:\n\n\
        <example>\n\
        user: \"What's left on this branch before we can ship?\"\n\
        assistant: Forking this — it's a survey question. I want the punch list, not the git \
        output in my context.\n\
        {AGENT_TOOL_NAME}({{\n\
          name: \"ship-audit\",\n\
          description: \"Branch ship-readiness audit\",\n\
          prompt: \"Audit what's left before this branch can ship. Check: uncommitted changes, \
        commits ahead of main, whether tests exist, whether the GrowthBook gate is wired up, \
        whether CI-relevant files changed. Report a punch list — done vs. missing. Under 200 words.\"\n\
        }})\n\
        assistant: Ship-readiness audit running.\n\
        <commentary>\n\
        Turn ends here. The coordinator knows nothing about the findings yet. What follows is a \
        SEPARATE turn — the notification arrives from outside, as a user-role message. It is not \
        something the coordinator writes.\n\
        </commentary>\n\
        [later turn — notification arrives as user message]\n\
        assistant: Audit's back. Three blockers: no tests for the new prompt path, GrowthBook \
        gate wired but not in build_flags.yaml, and one uncommitted file.\n\
        </example>\n\n\
        <example>\n\
        user: \"so is the gate wired up or not\"\n\
        <commentary>\n\
        User asks mid-wait. The audit fork was launched to answer exactly this, and it hasn't \
        returned. The coordinator does not have this answer. Give status, not a fabricated result.\n\
        </commentary>\n\
        assistant: Still waiting on the audit — that's one of the things it's checking. Should \
        land shortly.\n\
        </example>\n\n\
        <example>\n\
        user: \"Can you get a second opinion on whether this migration is safe?\"\n\
        assistant: I'll ask the code-reviewer agent — it won't see my analysis, so it can give \
        an independent read.\n\
        <commentary>\n\
        A subagent_type is specified, so the agent starts fresh. It needs full context in the \
        prompt. The briefing explains what to assess and why.\n\
        </commentary>\n\
        {AGENT_TOOL_NAME}({{\n\
          name: \"migration-review\",\n\
          description: \"Independent migration review\",\n\
          subagent_type: \"code-reviewer\",\n\
          prompt: \"Review migration 0042_user_schema.sql for safety. Context: we're adding a \
        NOT NULL column to a 50M-row table. Existing rows get a backfill default. I want a \
        second opinion on whether the backfill approach is safe under concurrent writes — I've \
        checked locking behavior but want independent verification. Report: is this safe, and \
        if not, what specifically breaks?\"\n\
        }})\n\
        </example>"
    )
}

/// Build standard (non-fork) examples.
fn build_current_examples() -> String {
    format!(
        "Example usage:\n\n\
        <example_agent_descriptions>\n\
        \"test-runner\": use this agent after you are done writing code to run tests\n\
        \"greeting-responder\": use this agent to respond to user greetings with a friendly joke\n\
        </example_agent_descriptions>\n\n\
        <example>\n\
        user: \"Please write a function that checks if a number is prime\"\n\
        assistant: I'm going to use the Write tool to write the following code:\n\
        <code>\n\
        function isPrime(n) {{\n\
          if (n <= 1) return false\n\
          for (let i = 2; i * i <= n; i++) {{\n\
            if (n % i === 0) return false\n\
          }}\n\
          return true\n\
        }}\n\
        </code>\n\
        <commentary>\n\
        Since a significant piece of code was written and the task was completed, now use the \
        test-runner agent to run the tests\n\
        </commentary>\n\
        assistant: Uses the {AGENT_TOOL_NAME} tool to launch the test-runner agent\n\
        </example>\n\n\
        <example>\n\
        user: \"Hello\"\n\
        <commentary>\n\
        Since the user is greeting, use the greeting-responder agent to respond with a friendly joke\n\
        </commentary>\n\
        assistant: \"I'm going to use the {AGENT_TOOL_NAME} tool to launch the greeting-responder agent\"\n\
        </example>"
    )
}

/// Format one agent line for the agent listing: `- type: whenToUse (Tools: ...)`.
pub fn format_agent_line(agent: &AgentDefinition) -> String {
    let tools_description = get_tools_description(agent);
    format!(
        "- {}: {} (Tools: {})",
        agent.agent_type, agent.when_to_use, tools_description
    )
}

/// Get a human-readable description of an agent's tool access.
pub fn get_tools_description(agent: &AgentDefinition) -> String {
    let has_allowlist = agent.has_tool_allowlist();
    let has_denylist = agent.has_tool_denylist();

    if has_allowlist && has_denylist {
        // Both defined: filter allowlist by denylist
        let deny_set: std::collections::HashSet<&str> =
            agent.disallowed_tools.iter().map(|s| s.as_str()).collect();
        let effective: Vec<&str> = agent
            .tools
            .iter()
            .map(|s| s.as_str())
            .filter(|t| !deny_set.contains(t))
            .collect();
        if effective.is_empty() {
            return "None".to_owned();
        }
        effective.join(", ")
    } else if has_allowlist {
        agent.tools.join(", ")
    } else if has_denylist {
        format!("All tools except {}", agent.disallowed_tools.join(", "))
    } else {
        "All tools".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::AgentDefinition;
    use claude_context::{RuntimeFeatureGates, RuntimeIdentityContext};

    #[test]
    fn format_agent_line_basic() {
        let agent = AgentDefinition::new("test", "A test agent");
        let line = format_agent_line(&agent);
        assert!(line.starts_with("- test: A test agent (Tools: All tools)"));
    }

    #[test]
    fn format_agent_line_with_tools() {
        let mut agent = AgentDefinition::new("test", "desc");
        agent.tools = vec!["Bash".to_owned(), "Read".to_owned()];
        let line = format_agent_line(&agent);
        assert!(line.contains("Tools: Bash, Read"));
    }

    #[test]
    fn format_agent_line_with_denylist() {
        let mut agent = AgentDefinition::new("test", "desc");
        agent.disallowed_tools = vec!["Agent".to_owned(), "Write".to_owned()];
        let line = format_agent_line(&agent);
        assert!(line.contains("All tools except Agent, Write"));
    }

    #[test]
    fn tools_description_both_lists() {
        let mut agent = AgentDefinition::new("test", "desc");
        agent.tools = vec!["Bash".to_owned(), "Read".to_owned(), "Write".to_owned()];
        agent.disallowed_tools = vec!["Write".to_owned()];
        let desc = get_tools_description(&agent);
        assert_eq!(desc, "Bash, Read");
    }

    #[test]
    fn tools_description_empty_after_filter() {
        let mut agent = AgentDefinition::new("test", "desc");
        agent.tools = vec!["Write".to_owned()];
        agent.disallowed_tools = vec!["Write".to_owned()];
        let desc = get_tools_description(&agent);
        assert_eq!(desc, "None");
    }

    #[test]
    fn tools_description_wildcard() {
        let mut agent = AgentDefinition::new("test", "desc");
        agent.tools = vec!["*".to_owned()];
        let desc = get_tools_description(&agent);
        assert_eq!(desc, "*");
    }

    #[test]
    fn build_prompt_coordinator_mode_is_slim() {
        let agents = vec![AgentDefinition::new("test", "desc")];
        let prompt = build_agent_prompt(&agents, false, true, None);
        assert!(prompt.contains("Launch a new agent"));
        assert!(!prompt.contains("Usage notes:"));
    }

    #[test]
    fn build_prompt_non_coordinator_has_usage_notes() {
        let agents = vec![AgentDefinition::new("test", "desc")];
        let prompt = build_agent_prompt(&agents, false, false, None);
        assert!(prompt.contains("Usage notes:"));
        assert!(prompt.contains("When NOT to use"));
    }

    #[test]
    fn build_prompt_fork_mode_adds_fork_section() {
        let agents = vec![AgentDefinition::new("test", "desc")];
        let prompt = build_agent_prompt(&agents, true, false, None);
        assert!(prompt.contains("When to fork"));
        assert!(prompt.contains("Writing a fork prompt"));
        assert!(!prompt.contains("When NOT to use"));
    }

    #[test]
    fn build_prompt_filters_by_allowed_types() {
        let agents = vec![
            AgentDefinition::new("a", "agent a"),
            AgentDefinition::new("b", "agent b"),
        ];
        let allowed = vec!["a".to_owned()];
        let prompt = build_agent_prompt(&agents, false, false, Some(&allowed));
        assert!(prompt.contains("- a:"));
        assert!(!prompt.contains("- b:"));
    }

    #[test]
    fn build_prompt_hides_agents_when_required_mcp_servers_are_missing() {
        let always = AgentDefinition::new("always", "always visible");
        let mut mcp = AgentDefinition::new("mcp", "needs context7");
        mcp.required_mcp_servers = vec!["context7".to_owned()];

        let prompt =
            build_agent_prompt_with_mcp_servers(&[always, mcp], false, false, None, Some(&[]));
        assert!(prompt.contains("- always:"));
        assert!(!prompt.contains("- mcp:"));
    }

    #[test]
    fn build_prompt_matches_required_mcp_servers_case_insensitive_substrings() {
        let mut agent = AgentDefinition::new("mcp", "needs servers");
        agent.required_mcp_servers = vec!["context".to_owned(), "MINI".to_owned()];

        let prompt = build_agent_prompt_with_mcp_servers(
            &[agent],
            false,
            false,
            None,
            Some(&["context7".to_owned(), "MiniMax".to_owned()]),
        );
        assert!(prompt.contains("- mcp:"));
    }

    #[test]
    fn build_prompt_requires_all_required_mcp_server_patterns() {
        let mut agent = AgentDefinition::new("mcp", "needs servers");
        agent.required_mcp_servers = vec!["context".to_owned(), "MiniMax".to_owned()];

        let prompt = build_agent_prompt_with_mcp_servers(
            &[agent],
            false,
            false,
            None,
            Some(&["context7".to_owned()]),
        );
        assert!(!prompt.contains("- mcp:"));
    }

    #[test]
    fn build_prompt_combines_mcp_and_allowed_type_filters() {
        let mut visible = AgentDefinition::new("visible", "visible");
        visible.required_mcp_servers = vec!["context".to_owned()];
        let hidden = AgentDefinition::new("hidden", "hidden");
        let allowed = vec!["visible".to_owned()];

        let prompt = build_agent_prompt_with_mcp_servers(
            &[visible, hidden],
            false,
            false,
            Some(&allowed),
            Some(&["context7".to_owned()]),
        );
        assert!(prompt.contains("- visible:"));
        assert!(!prompt.contains("- hidden:"));
    }

    #[test]
    fn build_prompt_filters_denied_agent_types() {
        let visible = AgentDefinition::new("visible", "visible");
        let hidden = AgentDefinition::new("hidden", "hidden");
        let denied = vec!["hidden".to_owned()];

        let prompt = build_agent_prompt_with_options(
            &[visible, hidden],
            AgentPromptOptions {
                is_fork_enabled: false,
                is_coordinator: false,
                allowed_agent_types: None,
                available_mcp_servers: None,
                denied_agent_types: Some(&denied),
                list_via_attachment: false,
            },
        );
        assert!(prompt.contains("- visible:"));
        assert!(!prompt.contains("- hidden:"));
    }

    #[test]
    fn build_prompt_uses_attachment_placeholder_when_requested() {
        let agent = AgentDefinition::new("visible", "visible");
        let prompt = build_agent_prompt_with_options(
            &[agent],
            AgentPromptOptions {
                is_fork_enabled: false,
                is_coordinator: false,
                allowed_agent_types: None,
                available_mcp_servers: None,
                denied_agent_types: None,
                list_via_attachment: true,
            },
        );
        assert!(prompt.contains(
            "Available agent types are listed in <system-reminder> messages in the conversation."
        ));
        assert!(!prompt.contains("- visible:"));
    }

    #[test]
    fn build_prompt_no_agents() {
        let agents: Vec<AgentDefinition> = vec![];
        let prompt = build_agent_prompt(&agents, false, false, None);
        assert!(prompt.contains("No agents available"));
    }

    #[test]
    fn full_prompt_with_builtins() {
        let ctx = RuntimeIdentityContext {
            features: RuntimeFeatureGates {
                explore_plan_agents_enabled: true,
                code_guide_enabled: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let agents = crate::builtins::get_built_in_agents_with_context(&ctx);
        let prompt = build_agent_prompt(&agents, false, false, None);
        assert!(prompt.contains("general-purpose"));
        assert!(prompt.contains("Explore"));
        assert!(prompt.contains("Plan"));
        assert!(prompt.contains("claude-code-guide"));
        assert!(!prompt.contains("verification"));
    }
}
