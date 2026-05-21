//! Agent definition types matching Claude Code's `AgentTool/loadAgentsDir.ts`.
//!
//! [`AgentDefinition`] describes an agent's type, capabilities, tools, and
//! configuration. It supports built-in, user, project, and marketplace agents.

use serde::{Deserialize, Serialize};

/// The origin of an agent definition.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AgentSource {
    /// Shipped with the application.
    #[default]
    BuiltIn,
    /// Defined in user-level settings.
    User,
    /// Defined in project-level settings.
    Project,
    /// Defined in local settings (not checked into VCS).
    Local,
    /// Defined in managed/policy settings.
    Policy,
    /// Provided by a plugin.
    Plugin,
    /// Provided via CLI flag.
    Flag,
    /// Obtained from the marketplace.
    Marketplace,
}

impl std::fmt::Display for AgentSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BuiltIn => write!(f, "built-in"),
            Self::User => write!(f, "userSettings"),
            Self::Project => write!(f, "projectSettings"),
            Self::Local => write!(f, "localSettings"),
            Self::Policy => write!(f, "policySettings"),
            Self::Plugin => write!(f, "plugin"),
            Self::Flag => write!(f, "flagSettings"),
            Self::Marketplace => write!(f, "marketplace"),
        }
    }
}

/// Persistent agent memory scope.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AgentMemoryScope {
    /// User-scoped memory (`~/.claude/agent-memory/`).
    User,
    /// Project-scoped memory (`.claude/agent-memory/`).
    #[default]
    Project,
    /// Local-scoped memory (`.claude/agent-memory-local/`).
    Local,
}

impl std::fmt::Display for AgentMemoryScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::User => write!(f, "user"),
            Self::Project => write!(f, "project"),
            Self::Local => write!(f, "local"),
        }
    }
}

/// Isolation mode for agent execution.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentIsolation {
    /// No isolation — runs in the current workspace.
    #[default]
    None,
    /// Runs in a temporary git worktree.
    Worktree,
}

/// Core definition of an agent, matching Claude Code's `AgentDefinition`.
///
/// This struct captures all the metadata needed to describe, display, and
/// run an agent: its type name, tool allow/deny lists, model preferences,
/// permission mode, memory scope, and more.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentDefinition {
    /// Unique agent type identifier (e.g. `"general-purpose"`, `"Explore"`).
    #[serde(alias = "agentType")]
    pub agent_type: String,

    /// Human-readable description of when to use this agent.
    #[serde(alias = "whenToUse")]
    pub when_to_use: String,

    /// Optional allowlist of tool names. `["*"]` means all tools.
    #[serde(default)]
    pub tools: Vec<String>,

    /// Optional denylist of tool names the agent cannot use.
    #[serde(default, alias = "disallowedTools")]
    pub disallowed_tools: Vec<String>,

    /// Maximum number of agentic turns before stopping.
    #[serde(default = "default_max_turns", alias = "maxTurns")]
    pub max_turns: u32,

    /// Optional model override (e.g. `"haiku"`, `"inherit"`).
    #[serde(default)]
    pub model: Option<String>,

    /// Optional reasoning effort override. Research accepts string levels or a
    /// numeric budget, so preserve the raw value until provider selection.
    #[serde(default)]
    pub effort: Option<serde_json::Value>,

    /// Optional permission mode override.
    #[serde(default, alias = "permissionMode")]
    pub permission_mode: Option<String>,

    /// Where this agent definition originated.
    #[serde(default)]
    pub source: AgentSource,

    /// Base directory for resolving relative paths.
    #[serde(default = "default_base_dir")]
    pub base_dir: String,

    /// Optional system prompt override.
    #[serde(default, alias = "systemPrompt")]
    pub system_prompt: Option<String>,

    /// Optional skill names to preload.
    #[serde(default)]
    pub skills: Vec<String>,

    /// Agent-specific MCP server references or inline server definitions.
    #[serde(default, alias = "mcpServers")]
    pub mcp_servers: Vec<serde_json::Value>,

    /// Session-scoped hook settings registered when the agent starts.
    #[serde(default)]
    pub hooks: Option<serde_json::Value>,

    /// Optional display color name from the agent definition.
    #[serde(default)]
    pub color: Option<String>,

    /// Short critical reminder reinjected for each user turn.
    #[serde(default, rename = "criticalSystemReminder_EXPERIMENTAL")]
    pub critical_system_reminder_experimental: Option<String>,

    /// MCP server name patterns required for this agent to be available.
    #[serde(default, rename = "requiredMcpServers")]
    pub required_mcp_servers: Vec<String>,

    /// Optional persistent memory scope.
    #[serde(default)]
    pub memory: Option<AgentMemoryScope>,

    /// Whether to always run as a background task.
    #[serde(default)]
    pub background: bool,

    /// Isolation mode for execution.
    #[serde(default)]
    pub isolation: AgentIsolation,

    /// Optional initial prompt prepended to the first user turn.
    #[serde(default, alias = "initialPrompt")]
    pub initial_prompt: Option<String>,

    /// Whether to omit CLAUDE.md hierarchy from the agent's user context.
    #[serde(default, alias = "omitClaudeMd")]
    pub omit_claude_md: bool,

    /// Original filename (for user/project/managed agents).
    #[serde(default)]
    pub filename: Option<String>,
}

fn default_max_turns() -> u32 {
    200
}

fn default_base_dir() -> String {
    "built-in".to_owned()
}

impl AgentDefinition {
    /// Create a minimal agent definition with the given type and description.
    #[must_use]
    pub fn new(agent_type: impl Into<String>, when_to_use: impl Into<String>) -> Self {
        Self {
            agent_type: agent_type.into(),
            when_to_use: when_to_use.into(),
            tools: Vec::new(),
            disallowed_tools: Vec::new(),
            max_turns: default_max_turns(),
            model: None,
            effort: None,
            permission_mode: None,
            source: AgentSource::BuiltIn,
            base_dir: default_base_dir(),
            system_prompt: None,
            skills: Vec::new(),
            mcp_servers: Vec::new(),
            hooks: None,
            color: None,
            critical_system_reminder_experimental: None,
            required_mcp_servers: Vec::new(),
            memory: None,
            background: false,
            isolation: AgentIsolation::None,
            initial_prompt: None,
            omit_claude_md: false,
            filename: None,
        }
    }

    /// Returns `true` if this agent has an explicit tool allowlist.
    pub fn has_tool_allowlist(&self) -> bool {
        !self.tools.is_empty()
    }

    /// Returns `true` if this agent has an explicit tool denylist.
    pub fn has_tool_denylist(&self) -> bool {
        !self.disallowed_tools.is_empty()
    }

    /// Returns `true` if this is a one-shot agent (runs once, returns a report).
    pub fn is_one_shot(&self) -> bool {
        const ONE_SHOT_TYPES: &[&str] = &["Explore", "Plan"];
        ONE_SHOT_TYPES.contains(&self.agent_type.as_str())
    }

    /// Returns `true` if this agent definition originated as a built-in.
    pub fn is_built_in(&self) -> bool {
        self.source == AgentSource::BuiltIn
    }

    /// Returns `true` if this agent definition came from user/project/policy settings.
    pub fn is_custom(&self) -> bool {
        !matches!(self.source, AgentSource::BuiltIn | AgentSource::Plugin)
    }

    /// Returns `true` if this agent definition came from a plugin.
    pub fn is_plugin(&self) -> bool {
        self.source == AgentSource::Plugin
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_agent_definition_defaults() {
        let def = AgentDefinition::new("test-agent", "A test agent");
        assert_eq!(def.agent_type, "test-agent");
        assert_eq!(def.when_to_use, "A test agent");
        assert!(def.tools.is_empty());
        assert!(def.disallowed_tools.is_empty());
        assert_eq!(def.max_turns, 200);
        assert!(def.model.is_none());
        assert_eq!(def.source, AgentSource::BuiltIn);
        assert_eq!(def.base_dir, "built-in");
        assert!(def.system_prompt.is_none());
        assert!(!def.background);
        assert!(def.is_built_in());
        assert!(!def.is_custom());
        assert!(!def.is_plugin());
    }

    #[test]
    fn tool_list_checks() {
        let mut def = AgentDefinition::new("agent", "desc");
        assert!(!def.has_tool_allowlist());
        assert!(!def.has_tool_denylist());

        def.tools.push("Bash".to_owned());
        assert!(def.has_tool_allowlist());

        def.disallowed_tools.push("Agent".to_owned());
        assert!(def.has_tool_denylist());
    }

    #[test]
    fn one_shot_detection() {
        let explore = AgentDefinition::new("Explore", "Explore agent");
        assert!(explore.is_one_shot());

        let plan = AgentDefinition::new("Plan", "Plan agent");
        assert!(plan.is_one_shot());

        let general = AgentDefinition::new("general-purpose", "General agent");
        assert!(!general.is_one_shot());
    }

    #[test]
    fn source_display() {
        assert_eq!(AgentSource::BuiltIn.to_string(), "built-in");
        assert_eq!(AgentSource::User.to_string(), "userSettings");
        assert_eq!(AgentSource::Project.to_string(), "projectSettings");
        assert_eq!(AgentSource::Plugin.to_string(), "plugin");
        assert_eq!(AgentSource::Marketplace.to_string(), "marketplace");
    }

    #[test]
    fn memory_scope_display() {
        assert_eq!(AgentMemoryScope::User.to_string(), "user");
        assert_eq!(AgentMemoryScope::Project.to_string(), "project");
        assert_eq!(AgentMemoryScope::Local.to_string(), "local");
    }

    #[test]
    fn serde_roundtrip() {
        let def = AgentDefinition::new("test", "test agent");
        let json = serde_json::to_string(&def).expect("serialize");
        let parsed: AgentDefinition = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.agent_type, "test");
        assert_eq!(parsed.when_to_use, "test agent");
    }
}
