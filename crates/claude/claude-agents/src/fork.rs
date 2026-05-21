//! Fork subagent support matching Claude Code's `AgentTool/forkSubagent.ts`.
//!
//! Fork subagents inherit the parent's full conversation context and run
//! independently. This module provides fork configuration, message construction,
//! child directive formatting, and recursive fork protection.
//!
//! # Fork Recursion Protection
//!
//! Fork children keep the Agent tool in their pool for cache-identical tool
//! definitions. To prevent recursive forking, [`is_in_fork_child`] detects the
//! fork boilerplate tag in conversation history.
//!
//! # Cache Sharing
//!
//! [`build_fork_messages`] constructs messages that maximize prompt cache hits
//! by replacing all tool_result blocks with identical placeholder text.

use serde::{Deserialize, Serialize};

use crate::constants::{
    FORK_BOILERPLATE_TAG, FORK_DIRECTIVE_PREFIX, FORK_PLACEHOLDER_RESULT, FORK_SUBAGENT_TYPE,
};
use crate::definition::{AgentDefinition, AgentSource};

/// Model selection for a forked subagent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ForkModel {
    /// Inherit the parent's model for cache sharing.
    Inherit,
    /// Use a specific model.
    Specific(String),
}

/// Permission mode for a forked subagent.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ForkPermissionMode {
    /// Bubble permission prompts to the parent terminal.
    #[default]
    Bubble,
    /// Run in isolated permission mode.
    Isolated,
}

/// Configuration for a fork subagent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkConfig {
    /// Whether to inherit the parent's conversation context.
    pub inherit_context: bool,
    /// Model selection strategy.
    pub model: ForkModel,
    /// Permission handling mode.
    pub permission_mode: ForkPermissionMode,
    /// Maximum number of turns for the fork.
    pub max_turns: u32,
}

impl Default for ForkConfig {
    fn default() -> Self {
        Self {
            inherit_context: true,
            model: ForkModel::Inherit,
            permission_mode: ForkPermissionMode::Bubble,
            max_turns: 200,
        }
    }
}

/// A simplified conversation message for fork message construction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkMessage {
    /// Role of the message sender.
    pub role: String,
    /// Content blocks in the message.
    pub content: Vec<ForkContentBlock>,
}

/// A content block within a fork message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ForkContentBlock {
    /// Text content.
    #[serde(rename = "text")]
    Text { text: String },
    /// A tool use block from an assistant message.
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// A tool result block.
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

/// The synthetic agent definition for the fork path.
///
/// Not registered in built-in agents — used only when `subagent_type` is
/// omitted and fork mode is active. Inherits the parent's tool pool and
/// system prompt for cache-identical API prefixes.
pub fn fork_agent_definition() -> AgentDefinition {
    AgentDefinition {
        agent_type: FORK_SUBAGENT_TYPE.to_owned(),
        when_to_use: "Implicit fork — inherits full conversation context. Not selectable \
            via subagent_type; triggered by omitting subagent_type when the fork \
            experiment is active."
            .to_owned(),
        tools: vec!["*".to_owned()],
        disallowed_tools: Vec::new(),
        max_turns: 200,
        model: Some("inherit".to_owned()),
        effort: None,
        permission_mode: Some("bubble".to_owned()),
        source: AgentSource::BuiltIn,
        base_dir: "built-in".to_owned(),
        system_prompt: Some(String::new()),
        skills: Vec::new(),
        mcp_servers: Vec::new(),
        hooks: None,
        color: None,
        critical_system_reminder_experimental: None,
        required_mcp_servers: Vec::new(),
        memory: None,
        background: false,
        isolation: crate::definition::AgentIsolation::None,
        initial_prompt: None,
        omit_claude_md: false,
        filename: None,
    }
}

/// Check whether the current conversation is a fork child by looking for
/// the fork boilerplate tag in message history.
///
/// Fork children keep the Agent tool in their tool pool for cache-identical
/// tool definitions, so we reject fork attempts at call time by detecting
/// the boilerplate tag.
pub fn is_fork_child(messages: &[ForkMessage]) -> bool {
    messages.iter().any(|msg| {
        if msg.role != "user" {
            return false;
        }
        msg.content.iter().any(|block| match block {
            ForkContentBlock::Text { text } => text.contains(&format!("<{FORK_BOILERPLATE_TAG}>")),
            _ => false,
        })
    })
}

/// Build the forked conversation messages for the child agent.
///
/// For prompt cache sharing, all fork children must produce byte-identical
/// API request prefixes. This function:
/// 1. Keeps the full parent assistant message (all tool_use blocks)
/// 2. Builds a single user message with tool_results for every tool_use block
///    using an identical placeholder, then appends the per-child directive
///
/// Result: `[...history, assistant(all_tool_uses), user(placeholder_results..., directive)]`
pub fn build_fork_messages(parent_messages: &[ForkMessage], directive: &str) -> Vec<ForkMessage> {
    // Find the last assistant message with tool_use blocks
    let last_assistant = parent_messages
        .iter()
        .rev()
        .find(|msg| msg.role == "assistant");

    let Some(assistant_msg) = last_assistant else {
        // No assistant message with tool_use blocks — just send the directive
        return vec![ForkMessage {
            role: "user".to_owned(),
            content: vec![ForkContentBlock::Text {
                text: build_child_message(directive),
            }],
        }];
    };

    // Collect all tool_use blocks
    let tool_use_blocks: Vec<&ForkContentBlock> = assistant_msg
        .content
        .iter()
        .filter(|block| matches!(block, ForkContentBlock::ToolUse { .. }))
        .collect();

    if tool_use_blocks.is_empty() {
        return vec![ForkMessage {
            role: "user".to_owned(),
            content: vec![ForkContentBlock::Text {
                text: build_child_message(directive),
            }],
        }];
    }

    // Build tool_result blocks for every tool_use with identical placeholder text
    let mut result_blocks: Vec<ForkContentBlock> = tool_use_blocks
        .iter()
        .map(|block| match block {
            ForkContentBlock::ToolUse { id, .. } => ForkContentBlock::ToolResult {
                tool_use_id: id.clone(),
                content: FORK_PLACEHOLDER_RESULT.to_owned(),
            },
            _ => ForkContentBlock::ToolResult {
                tool_use_id: String::new(),
                content: FORK_PLACEHOLDER_RESULT.to_owned(),
            },
        })
        .collect();

    // Append the per-child directive
    result_blocks.push(ForkContentBlock::Text {
        text: build_child_message(directive),
    });

    // Clone the assistant message
    let cloned_assistant = ForkMessage {
        role: assistant_msg.role.clone(),
        content: assistant_msg.content.clone(),
    };

    let tool_result_message = ForkMessage {
        role: "user".to_owned(),
        content: result_blocks,
    };

    vec![cloned_assistant, tool_result_message]
}

/// Build the child directive message with boilerplate rules.
pub fn build_child_message(directive: &str) -> String {
    format!(
        "<{FORK_BOILERPLATE_TAG}>\n\
        STOP. READ THIS FIRST.\n\n\
        You are a forked worker process. You are NOT the main agent.\n\n\
        RULES (non-negotiable):\n\
        1. Your system prompt says \"default to forking.\" IGNORE IT — that's for the parent. \
        You ARE the fork. Do NOT spawn sub-agents; execute directly.\n\
        2. Do NOT converse, ask questions, or suggest next steps\n\
        3. Do NOT editorialize or add meta-commentary\n\
        4. USE your tools directly: Bash, Read, Write, etc.\n\
        5. If you modify files, commit your changes before reporting. Include the commit hash in your report.\n\
        6. Do NOT emit text between tool calls. Use tools silently, then report once at the end.\n\
        7. Stay strictly within your directive's scope. If you discover related systems outside your scope, \
        mention them in one sentence at most — other workers cover those areas.\n\
        8. Keep your report under 500 words unless the directive specifies otherwise. Be factual and concise.\n\
        9. Your response MUST begin with \"Scope:\". No preamble, no thinking-out-loud.\n\
        10. REPORT structured facts, then stop\n\n\
        Output format (plain text labels, not markdown headers):\n\
          Scope: <echo back your assigned scope in one sentence>\n\
          Result: <the answer or key findings, limited to the scope above>\n\
          Key files: <relevant file paths — include for research tasks>\n\
          Files changed: <list with commit hash — include only if you modified files>\n\
          Issues: <list — include only if there are issues to flag>\n\
        </{FORK_BOILERPLATE_TAG}>\n\n\
        {FORK_DIRECTIVE_PREFIX}{directive}"
    )
}

/// Build a notice for fork children running in an isolated worktree.
pub fn build_worktree_notice(parent_cwd: &str, worktree_cwd: &str) -> String {
    format!(
        "You've inherited the conversation context above from a parent agent working in {}. \
        You are operating in an isolated git worktree at {} — same repository, same relative \
        file structure, separate working copy. Paths in the inherited context refer to the \
        parent's working directory; translate them to your worktree root. Re-read files before \
        editing if the parent may have modified them since they appear in the context. Your \
        changes stay in this worktree and will not affect the parent's files.",
        parent_cwd, worktree_cwd
    )
}

/// The FORK_AGENT constant definition, matching Claude Code's `FORK_AGENT`.
///
/// This is the synthetic agent definition for the fork path. It is not
/// registered in built-in agents — used only when `subagent_type` is
/// omitted and fork mode is active.
pub const FORK_AGENT: ForkAgentStatic = ForkAgentStatic {
    agent_type: FORK_SUBAGENT_TYPE,
    max_turns: 200,
};

/// Static definition of the fork agent for constant-time access.
pub struct ForkAgentStatic {
    /// The agent type identifier.
    pub agent_type: &'static str,
    /// Maximum number of turns.
    pub max_turns: u32,
}

/// Check if we are inside a fork child by detecting the fork boilerplate tag.
///
/// This is an alias for [`is_fork_child`] with additional context about
/// non-interactive sessions. Fork children keep the Agent tool in their
/// tool pool for cache-identical tool definitions, so we reject fork
/// attempts at call time by detecting the boilerplate tag.
///
/// Returns `true` if the conversation history contains the fork boilerplate
/// tag in any user message, indicating we are already in a fork child.
pub fn is_in_fork_child(messages: &[ForkMessage]) -> bool {
    is_fork_child(messages)
}

/// Check if fork subagent is enabled.
///
/// Fork is disabled in coordinator mode and non-interactive sessions.
pub fn is_fork_subagent_enabled(is_coordinator: bool, is_non_interactive: bool) -> bool {
    !is_coordinator && !is_non_interactive
}

/// Replace tool_result blocks in messages with placeholder text for cache sharing.
///
/// This function processes a list of messages and replaces all `ToolResult`
/// content blocks with the standard placeholder text. This ensures that
/// fork children produce byte-identical API request prefixes for maximum
/// prompt cache hits.
///
/// Returns a new vector of messages with tool results replaced.
pub fn replace_tool_results_with_placeholder(messages: &[ForkMessage]) -> Vec<ForkMessage> {
    messages
        .iter()
        .map(|msg| ForkMessage {
            role: msg.role.clone(),
            content: msg
                .content
                .iter()
                .map(|block| match block {
                    ForkContentBlock::ToolResult { tool_use_id, .. } => {
                        ForkContentBlock::ToolResult {
                            tool_use_id: tool_use_id.clone(),
                            content: FORK_PLACEHOLDER_RESULT.to_owned(),
                        }
                    }
                    other => other.clone(),
                })
                .collect(),
        })
        .collect()
}

/// Count tool_use blocks in a slice of messages.
pub fn count_tool_uses(messages: &[ForkMessage]) -> usize {
    messages
        .iter()
        .flat_map(|msg| msg.content.iter())
        .filter(|block| matches!(block, ForkContentBlock::ToolUse { .. }))
        .count()
}

/// Extract all tool_use IDs from a slice of messages.
pub fn extract_tool_use_ids(messages: &[ForkMessage]) -> Vec<String> {
    messages
        .iter()
        .flat_map(|msg| msg.content.iter())
        .filter_map(|block| match block {
            ForkContentBlock::ToolUse { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect()
}

/// Validate that a fork directive is safe to execute.
///
/// Checks for:
/// - Non-empty directive
/// - No embedded boilerplate tags (prevents injection)
/// - Reasonable length (under 10,000 chars)
pub fn validate_fork_directive(directive: &str) -> std::result::Result<(), String> {
    if directive.trim().is_empty() {
        return Err("Fork directive must not be empty".to_owned());
    }

    if directive.contains(&format!("<{FORK_BOILERPLATE_TAG}>")) {
        return Err("Fork directive contains embedded boilerplate tag".to_owned());
    }

    if directive.contains(&format!("</{FORK_BOILERPLATE_TAG}>")) {
        return Err("Fork directive contains embedded closing boilerplate tag".to_owned());
    }

    if directive.len() > 10_000 {
        return Err(format!(
            "Fork directive too long ({} chars, max 10000)",
            directive.len()
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fork_config_default() {
        let config = ForkConfig::default();
        assert!(config.inherit_context);
        assert_eq!(config.model, ForkModel::Inherit);
        assert_eq!(config.permission_mode, ForkPermissionMode::Bubble);
        assert_eq!(config.max_turns, 200);
    }

    #[test]
    fn fork_agent_definition_type() {
        let def = fork_agent_definition();
        assert_eq!(def.agent_type, "fork");
        assert_eq!(def.tools, vec!["*"]);
        assert_eq!(def.model.as_deref(), Some("inherit"));
    }

    #[test]
    fn is_fork_child_detects_boilerplate() {
        let messages = vec![ForkMessage {
            role: "user".to_owned(),
            content: vec![ForkContentBlock::Text {
                text: format!("<{FORK_BOILERPLATE_TAG}> some content"),
            }],
        }];
        assert!(is_fork_child(&messages));
    }

    #[test]
    fn is_fork_child_no_boilerplate() {
        let messages = vec![ForkMessage {
            role: "user".to_owned(),
            content: vec![ForkContentBlock::Text {
                text: "normal message".to_owned(),
            }],
        }];
        assert!(!is_fork_child(&messages));
    }

    #[test]
    fn is_fork_child_ignores_assistant_messages() {
        let messages = vec![ForkMessage {
            role: "assistant".to_owned(),
            content: vec![ForkContentBlock::Text {
                text: format!("<{FORK_BOILERPLATE_TAG}> content"),
            }],
        }];
        assert!(!is_fork_child(&messages));
    }

    #[test]
    fn build_fork_messages_with_tool_uses() {
        let messages = vec![ForkMessage {
            role: "assistant".to_owned(),
            content: vec![
                ForkContentBlock::ToolUse {
                    id: "tool-1".to_owned(),
                    name: "Bash".to_owned(),
                    input: serde_json::json!({"command": "ls"}),
                },
                ForkContentBlock::ToolUse {
                    id: "tool-2".to_owned(),
                    name: "Read".to_owned(),
                    input: serde_json::json!({"path": "/test"}),
                },
            ],
        }];

        let result = build_fork_messages(&messages, "check tests");
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].role, "assistant");
        assert_eq!(result[1].role, "user");

        // Should have 2 tool_results + 1 directive text
        assert_eq!(result[1].content.len(), 3);
    }

    #[test]
    fn build_fork_messages_no_assistant() {
        let messages: Vec<ForkMessage> = vec![];
        let result = build_fork_messages(&messages, "do something");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "user");
    }

    #[test]
    fn build_child_message_contains_rules() {
        let msg = build_child_message("test directive");
        assert!(msg.contains("STOP. READ THIS FIRST."));
        assert!(msg.contains("Scope:"));
        assert!(msg.contains(&format!("{FORK_DIRECTIVE_PREFIX}test directive")));
    }

    #[test]
    fn worktree_notice_contains_paths() {
        let notice = super::build_worktree_notice("/home/user/project", "/tmp/worktree");
        assert!(notice.contains("/home/user/project"));
        assert!(notice.contains("/tmp/worktree"));
    }

    #[test]
    fn fork_model_serde() {
        let model = ForkModel::Specific("claude-3".to_owned());
        let json = serde_json::to_string(&model).expect("serialize");
        let parsed: ForkModel = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, model);
    }

    // ── Enhanced tests ──────────────────────────────────────────────────────

    #[test]
    fn fork_agent_constant_matches_definition() {
        assert_eq!(FORK_AGENT.agent_type, FORK_SUBAGENT_TYPE);
        assert_eq!(FORK_AGENT.max_turns, 200);
    }

    #[test]
    fn is_in_fork_child_alias() {
        let messages = vec![ForkMessage {
            role: "user".to_owned(),
            content: vec![ForkContentBlock::Text {
                text: format!("<{FORK_BOILERPLATE_TAG}> content"),
            }],
        }];
        assert!(is_in_fork_child(&messages));
        assert_eq!(is_in_fork_child(&messages), is_fork_child(&messages));
    }

    #[test]
    fn is_in_fork_child_no_boilerplate() {
        let messages = vec![ForkMessage {
            role: "user".to_owned(),
            content: vec![ForkContentBlock::Text {
                text: "normal".to_owned(),
            }],
        }];
        assert!(!is_in_fork_child(&messages));
    }

    #[test]
    fn is_fork_subagent_enabled_normal() {
        assert!(is_fork_subagent_enabled(false, false));
    }

    #[test]
    fn is_fork_subagent_disabled_in_coordinator() {
        assert!(!is_fork_subagent_enabled(true, false));
    }

    #[test]
    fn is_fork_subagent_disabled_in_non_interactive() {
        assert!(!is_fork_subagent_enabled(false, true));
    }

    #[test]
    fn replace_tool_results_with_placeholder_replaces_content() {
        let messages = vec![ForkMessage {
            role: "user".to_owned(),
            content: vec![
                ForkContentBlock::ToolResult {
                    tool_use_id: "t-1".to_owned(),
                    content: "actual result".to_owned(),
                },
                ForkContentBlock::Text {
                    text: "some text".to_owned(),
                },
            ],
        }];
        let replaced = replace_tool_results_with_placeholder(&messages);
        assert_eq!(replaced.len(), 1);
        match &replaced[0].content[0] {
            ForkContentBlock::ToolResult { content, .. } => {
                assert_eq!(content, FORK_PLACEHOLDER_RESULT);
            }
            _ => panic!("Expected ToolResult"),
        }
        // Text block should be unchanged
        match &replaced[0].content[1] {
            ForkContentBlock::Text { text } => {
                assert_eq!(text, "some text");
            }
            _ => panic!("Expected Text"),
        }
    }

    #[test]
    fn replace_tool_results_preserves_tool_use_ids() {
        let messages = vec![ForkMessage {
            role: "user".to_owned(),
            content: vec![ForkContentBlock::ToolResult {
                tool_use_id: "unique-id-123".to_owned(),
                content: "original".to_owned(),
            }],
        }];
        let replaced = replace_tool_results_with_placeholder(&messages);
        match &replaced[0].content[0] {
            ForkContentBlock::ToolResult {
                tool_use_id,
                content,
            } => {
                assert_eq!(tool_use_id, "unique-id-123");
                assert_eq!(content, FORK_PLACEHOLDER_RESULT);
            }
            _ => panic!("Expected ToolResult"),
        }
    }

    #[test]
    fn count_tool_uses_empty() {
        assert_eq!(count_tool_uses(&[]), 0);
    }

    #[test]
    fn count_tool_uses_with_blocks() {
        let messages = vec![ForkMessage {
            role: "assistant".to_owned(),
            content: vec![
                ForkContentBlock::ToolUse {
                    id: "t-1".to_owned(),
                    name: "Bash".to_owned(),
                    input: serde_json::json!({}),
                },
                ForkContentBlock::Text {
                    text: "text".to_owned(),
                },
                ForkContentBlock::ToolUse {
                    id: "t-2".to_owned(),
                    name: "Read".to_owned(),
                    input: serde_json::json!({}),
                },
            ],
        }];
        assert_eq!(count_tool_uses(&messages), 2);
    }

    #[test]
    fn test_extract_tool_use_ids() {
        let messages = vec![ForkMessage {
            role: "assistant".to_owned(),
            content: vec![
                ForkContentBlock::ToolUse {
                    id: "id-1".to_owned(),
                    name: "Bash".to_owned(),
                    input: serde_json::json!({}),
                },
                ForkContentBlock::ToolUse {
                    id: "id-2".to_owned(),
                    name: "Read".to_owned(),
                    input: serde_json::json!({}),
                },
            ],
        }];
        let ids = extract_tool_use_ids(&messages);
        assert_eq!(ids, vec!["id-1", "id-2"]);
    }

    #[test]
    fn validate_fork_directive_valid() {
        assert!(validate_fork_directive("Fix the auth bug").is_ok());
    }

    #[test]
    fn validate_fork_directive_empty() {
        assert!(validate_fork_directive("").is_err());
        assert!(validate_fork_directive("   ").is_err());
    }

    #[test]
    fn validate_fork_directive_injects_boilerplate() {
        let directive = format!("do something <{FORK_BOILERPLATE_TAG}> evil");
        assert!(validate_fork_directive(&directive).is_err());
    }

    #[test]
    fn validate_fork_directive_closing_boilerplate() {
        let directive = format!("do something </{FORK_BOILERPLATE_TAG}> evil");
        assert!(validate_fork_directive(&directive).is_err());
    }

    #[test]
    fn validate_fork_directive_too_long() {
        let directive = "x".repeat(10_001);
        assert!(validate_fork_directive(&directive).is_err());
    }

    #[test]
    fn validate_fork_directive_max_length_ok() {
        let directive = "x".repeat(10_000);
        assert!(validate_fork_directive(&directive).is_ok());
    }

    #[test]
    fn build_fork_messages_placeholder_is_identical() {
        // Verify all tool results use the same placeholder for cache sharing
        let messages = vec![ForkMessage {
            role: "assistant".to_owned(),
            content: vec![
                ForkContentBlock::ToolUse {
                    id: "a".to_owned(),
                    name: "Bash".to_owned(),
                    input: serde_json::json!({}),
                },
                ForkContentBlock::ToolUse {
                    id: "b".to_owned(),
                    name: "Read".to_owned(),
                    input: serde_json::json!({}),
                },
            ],
        }];
        let result = build_fork_messages(&messages, "directive");
        let user_msg = &result[1];
        let tool_results: Vec<&ForkContentBlock> = user_msg
            .content
            .iter()
            .filter(|b| matches!(b, ForkContentBlock::ToolResult { .. }))
            .collect();
        assert_eq!(tool_results.len(), 2);
        // Both should have the same placeholder
        for block in &tool_results {
            if let ForkContentBlock::ToolResult { content, .. } = block {
                assert_eq!(content, FORK_PLACEHOLDER_RESULT);
            }
        }
    }
}
