use claude_plugins::CommandManifestEntry;
use claude_runtime::RuntimeConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
pub enum CommandAvailability {
    Always,
    InteractiveOnly,
    NonInteractiveOnly,
    FeatureGated { flag: String },
}

impl Default for CommandAvailability {
    fn default() -> Self {
        Self::Always
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandResultDisplay {
    pub text: String,
    pub is_error: bool,
}

#[derive(Debug, Clone)]
pub enum CommandHandler {
    Builtin(fn(&str, &RuntimeConfig) -> CommandResultDisplay),
    Prompt {
        description: String,
        prompt_template: String,
        allowed_tools: Vec<String>,
    },
    LocalJsx,
    Local(fn(&str, &RuntimeConfig) -> CommandResultDisplay),
}

impl CommandHandler {
    pub fn is_immediate(&self) -> bool {
        matches!(self, CommandHandler::Builtin(_) | CommandHandler::Local(_))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlashCommandSpec {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub args_hint: Option<String>,
    #[serde(default)]
    pub is_hidden: bool,
    #[serde(default)]
    pub availability: CommandAvailability,
    #[serde(skip)]
    pub handler: Option<CommandHandler>,
}

impl SlashCommandSpec {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            aliases: Vec::new(),
            args_hint: None,
            is_hidden: false,
            availability: CommandAvailability::default(),
            handler: None,
        }
    }

    pub fn aliases(mut self, aliases: &[&str]) -> Self {
        self.aliases = aliases.iter().map(|s| s.to_string()).collect();
        self
    }

    pub fn args_hint(mut self, hint: impl Into<String>) -> Self {
        self.args_hint = Some(hint.into());
        self
    }

    pub fn hidden(mut self) -> Self {
        self.is_hidden = true;
        self
    }

    pub fn feature_gated(mut self, flag: impl Into<String>) -> Self {
        self.availability = CommandAvailability::FeatureGated { flag: flag.into() };
        self
    }

    pub fn non_interactive_only(mut self) -> Self {
        self.availability = CommandAvailability::NonInteractiveOnly;
        self
    }
}

pub struct CommandRegistry {
    commands: HashMap<String, SlashCommandSpec>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            commands: HashMap::new(),
        };
        registry.register_defaults();
        registry.register_prompt_commands();
        registry.register_feature_gated_commands();
        registry
    }

    fn register_defaults(&mut self) {
        let defaults: Vec<SlashCommandSpec> = vec![
            SlashCommandSpec::new("help", "Show available commands").aliases(&["h", "?"]),
            SlashCommandSpec::new("clear", "Clear conversation history")
                .aliases(&["c", "reset", "new"]),
            SlashCommandSpec::new("compact", "Compact conversation context"),
            SlashCommandSpec::new("model", "Change the current model").args_hint("<model>"),
            SlashCommandSpec::new("status", "Show current status")
                .aliases(&["st"])
                .non_interactive_only(),
            SlashCommandSpec::new("cost", "Show token usage and cost").aliases(&["usage"]),
            SlashCommandSpec::new("config", "Show or edit configuration")
                .aliases(&["settings", "cfg"])
                .args_hint("[key] [value]"),
            SlashCommandSpec::new("permissions", "Manage permission rules")
                .aliases(&["allowed-tools"]),
            SlashCommandSpec::new("diff", "View git diff"),
            SlashCommandSpec::new("exit", "Exit the CLI").aliases(&["quit", "q"]),
            SlashCommandSpec::new("doctor", "Check system health"),
            SlashCommandSpec::new("mcp", "Manage MCP servers")
                .args_hint("[enable|disable [server]]"),
            SlashCommandSpec::new("memory", "Edit Claude memory files"),
            SlashCommandSpec::new("skills", "List available skills"),
            SlashCommandSpec::new("context", "Show context window usage").non_interactive_only(),
            SlashCommandSpec::new("version", "Print version")
                .aliases(&["v"])
                .non_interactive_only(),
            SlashCommandSpec::new("init", "Initialize CLAUDE.md and project config"),
            SlashCommandSpec::new("session", "Resume a previous conversation")
                .aliases(&["continue", "resume"])
                .args_hint("[id or search]"),
            SlashCommandSpec::new("export", "Export conversation").args_hint("[filename]"),
            SlashCommandSpec::new("model", "Set the AI model"),
            SlashCommandSpec::new("fast", "Toggle fast mode (Haiku only)")
                .args_hint("[on|off]")
                .feature_gated("FAST_MODE"),
            SlashCommandSpec::new("effort", "Set effort level")
                .args_hint("[low|medium|high|max|auto]"),
            SlashCommandSpec::new("sandbox", "Toggle sandbox mode"),
            SlashCommandSpec::new("compact", "Compact conversation"),
            SlashCommandSpec::new("rename", "Rename current conversation").args_hint("[name]"),
            SlashCommandSpec::new("theme", "Change theme"),
            SlashCommandSpec::new("color", "Set prompt bar color").args_hint("<color|default>"),
            SlashCommandSpec::new("vim", "Toggle Vim editing mode"),
            SlashCommandSpec::new("tasks", "List background tasks").aliases(&["bashes"]),
            SlashCommandSpec::new("stats", "Show usage statistics"),
            SlashCommandSpec::new("hooks", "View hook configurations"),
            SlashCommandSpec::new("permissions", "Manage permission rules"),
            SlashCommandSpec::new("plan", "Enable plan mode").args_hint("[open|<description>]"),
            SlashCommandSpec::new("context", "Visualize context usage"),
            SlashCommandSpec::new("diff", "View git diffs"),
            SlashCommandSpec::new("login", "Sign in to Anthropic account"),
            SlashCommandSpec::new("logout", "Sign out"),
            SlashCommandSpec::new("branch", "Create conversation branch").aliases(&["fork"]),
            SlashCommandSpec::new("btw", "Ask a quick side question").args_hint("<question>"),
            SlashCommandSpec::new("copy", "Copy last response to clipboard").args_hint("[N]"),
            SlashCommandSpec::new("resume", "Resume a previous conversation"),
            SlashCommandSpec::new("release-notes", "View release notes"),
            SlashCommandSpec::new("reload-plugins", "Reload plugins"),
            SlashCommandSpec::new("plugin", "Manage plugins").aliases(&["plugins", "marketplace"]),
            SlashCommandSpec::new("keybindings", "Open keybindings config"),
            SlashCommandSpec::new("mobile", "Show mobile app QR code").aliases(&["ios", "android"]),
            SlashCommandSpec::new("heapdump", "Dump JS heap (hidden)").hidden(),
            SlashCommandSpec::new("terminal-setup", "Install Shift+Enter binding").hidden(),
            SlashCommandSpec::new("env", "Show environment variables"),
            SlashCommandSpec::new("pr-comments", "Get GitHub PR comments")
                .feature_gated("PR_COMMENTS"),
            SlashCommandSpec::new("review", "Review current changes").feature_gated("REVIEW"),
            SlashCommandSpec::new("security-review", "Security audit")
                .feature_gated("SECURITY_REVIEW"),
            SlashCommandSpec::new("voice", "Toggle voice mode").feature_gated("VOICE"),
            SlashCommandSpec::new("desktop", "Open in Claude Desktop")
                .aliases(&["app"])
                .feature_gated("DESKTOP"),
            SlashCommandSpec::new("advisor", "Configure advisor model"),
            SlashCommandSpec::new("agents", "Manage agent configurations"),
            SlashCommandSpec::new("add-dir", "Add a working directory").args_hint("<path>"),
            SlashCommandSpec::new("chrome", "Chrome integration settings"),
            SlashCommandSpec::new("feedback", "Submit feedback").aliases(&["bug"]),
            SlashCommandSpec::new("files", "List files in context"),
            SlashCommandSpec::new("commit-push-pr", "Commit, push, and open PR"),
            SlashCommandSpec::new("statusline", "Set up status line"),
            SlashCommandSpec::new("insights", "Analyze session history"),
            SlashCommandSpec::new("summary", "Generate session summary"),
            SlashCommandSpec::new("tasks", "List background tasks"),
            SlashCommandSpec::new("worktrees", "Worktree management"),
            SlashCommandSpec::new("rewind", "Restore to previous point").aliases(&["checkpoint"]),
            SlashCommandSpec::new("passes", "Share a free week"),
            SlashCommandSpec::new("extra-usage", "Configure extra usage"),
            SlashCommandSpec::new("rate-limit", "Show rate limit options"),
            SlashCommandSpec::new("upgrade", "Upgrade to Max"),
            SlashCommandSpec::new("thread", "Toggle conversation threading"),
            SlashCommandSpec::new("auto", "Toggle auto-continue mode"),
            SlashCommandSpec::new("scope", "Set output scope").args_hint("[full|concise]"),
            SlashCommandSpec::new("debug", "Toggle debug logging"),
            SlashCommandSpec::new("memory", "Manage persistent memory"),
            SlashCommandSpec::new("desktop", "Continue in Claude Desktop"),
            SlashCommandSpec::new("shortcuts", "Show keyboard shortcuts"),
            SlashCommandSpec::new("config", "Open config settings"),
            SlashCommandSpec::new("team", "Team management").feature_gated("SWARM"),
        ];
        for spec in defaults {
            self.commands.insert(spec.name.clone(), spec);
        }
    }

    fn register_prompt_commands(&mut self) {
        let prompt_commands: Vec<SlashCommandSpec> = vec![
            SlashCommandSpec::new("commit", "Create a git commit"),
            SlashCommandSpec::new("commit-push-pr", "Commit, push, and open a PR"),
            SlashCommandSpec::new("review", "Review a pull request"),
            SlashCommandSpec::new("security-review", "Security audit of changes"),
            SlashCommandSpec::new("pr-comments", "Get GitHub PR comments"),
            SlashCommandSpec::new("init", "Initialize project config"),
            SlashCommandSpec::new("insights", "Analyze session history"),
            SlashCommandSpec::new("statusline", "Set up status line"),
            SlashCommandSpec::new("verify", "Verify code changes work correctly"),
        ];
        for spec in prompt_commands {
            self.commands.insert(spec.name.clone(), spec);
        }
    }

    fn register_feature_gated_commands(&mut self) {
        let gated: Vec<SlashCommandSpec> = vec![
            SlashCommandSpec::new("proactive", "Toggle proactive assistant mode")
                .feature_gated("PROACTIVE"),
            SlashCommandSpec::new("brief", "Toggle brief mode").feature_gated("BRIEF"),
            SlashCommandSpec::new("remote-control", "Remote control sessions")
                .feature_gated("BRIDGE"),
            SlashCommandSpec::new("force-snip", "Force context snipping").feature_gated("SNIP"),
            SlashCommandSpec::new("workflows", "Manage workflow scripts")
                .feature_gated("WORKFLOWS"),
            SlashCommandSpec::new("subscribe-pr", "Subscribe to PR events")
                .feature_gated("PR_WEBHOOKS"),
            SlashCommandSpec::new("torch", "Torch feature").feature_gated("TORCH"),
            SlashCommandSpec::new("peers", "Peer management").feature_gated("UDS_INBOX"),
            SlashCommandSpec::new("fork", "Fork conversation").feature_gated("FORK"),
            SlashCommandSpec::new("buddy", "Buddy mode").feature_gated("BUDDY"),
        ];
        for spec in gated {
            self.commands.insert(spec.name.clone(), spec);
        }
    }

    pub fn register(&mut self, spec: SlashCommandSpec) {
        let name = spec.name.clone();
        for alias in &spec.aliases {
            self.commands.insert(alias.clone(), spec.clone());
        }
        self.commands.insert(name, spec);
    }

    pub fn register_from_plugin(&mut self, entry: &CommandManifestEntry) {
        let spec = SlashCommandSpec::new(entry.name.clone(), entry.description.clone());
        self.register(spec);
    }

    pub fn get(&self, name: &str) -> Option<&SlashCommandSpec> {
        self.commands.get(name)
    }

    pub fn list_visible(&self) -> Vec<&SlashCommandSpec> {
        let mut seen = std::collections::HashSet::new();
        self.commands
            .values()
            .filter(|s| !s.is_hidden)
            .filter(|s| {
                matches!(
                    s.availability,
                    CommandAvailability::Always | CommandAvailability::InteractiveOnly
                )
            })
            .filter(|s| seen.insert(s.name.clone()))
            .collect()
    }

    pub fn list_all(&self) -> Vec<&SlashCommandSpec> {
        let mut seen = std::collections::HashSet::new();
        self.commands
            .values()
            .filter(|s| !s.is_hidden)
            .filter(|s| seen.insert(s.name.clone()))
            .collect()
    }

    pub fn parse_command(input: &str) -> Option<(&str, &str)> {
        let input = input.trim();
        if input.starts_with('/') {
            let rest = &input[1..];
            if let Some(space_idx) = rest.find(' ') {
                Some((&rest[..space_idx], rest[space_idx + 1..].trim()))
            } else {
                Some((rest, ""))
            }
        } else {
            None
        }
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_command_simple() {
        let result = CommandRegistry::parse_command("/help");
        assert_eq!(result, Some(("help", "")));
    }

    #[test]
    fn test_parse_command_with_args() {
        let result = CommandRegistry::parse_command("/model claude-3");
        assert_eq!(result, Some(("model", "claude-3")));
    }

    #[test]
    fn test_parse_command_no_slash() {
        let result = CommandRegistry::parse_command("hello");
        assert_eq!(result, None);
    }

    #[test]
    fn test_registry_get() {
        let registry = CommandRegistry::new();
        let help = registry.get("help");
        assert!(help.is_some());
        assert_eq!(help.unwrap().name, "help");

        let unknown = registry.get("nonexistent-command");
        assert!(unknown.is_none());
    }

    #[test]
    fn test_registry_list_visible() {
        let registry = CommandRegistry::new();
        let visible = registry.list_visible();

        assert!(!visible.is_empty());
        let hidden_names: Vec<&str> = visible.iter().map(|s| s.name.as_str()).collect();
        assert!(hidden_names.contains(&"help"));
        assert!(hidden_names.contains(&"clear"));

        for spec in &visible {
            assert!(!spec.is_hidden);
        }
    }

    #[test]
    fn test_command_spec_builder() {
        let spec = SlashCommandSpec::new("test", "A test command")
            .aliases(&["t", "testing"])
            .args_hint("<arg>")
            .hidden();

        assert_eq!(spec.name, "test");
        assert_eq!(spec.description, "A test command");
        assert_eq!(spec.aliases, vec!["t", "testing"]);
        assert_eq!(spec.args_hint, Some("<arg>".to_string()));
        assert!(spec.is_hidden);
        assert!(spec.handler.is_none());

        let gated =
            SlashCommandSpec::new("feature", "A feature command").feature_gated("MY_FEATURE");
        assert!(matches!(
            gated.availability,
            CommandAvailability::FeatureGated { ref flag } if flag == "MY_FEATURE"
        ));

        let nio = SlashCommandSpec::new("nonint", "Non-interactive only").non_interactive_only();
        assert_eq!(nio.availability, CommandAvailability::NonInteractiveOnly);
    }
}
