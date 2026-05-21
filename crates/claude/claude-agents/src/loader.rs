//! Agent directory loader matching Claude Code's `AgentTool/loadAgentsDir.ts`.
//!
//! Loads agent definitions from `.claude/agents/` directories (user, project,
//! and local settings) as well as from JSON configuration files.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
#[cfg(test)]
use claude_context::RuntimeFeatureGates;
use claude_context::RuntimeIdentityContext;
use indexmap::IndexMap;
use serde::Deserialize;
use walkdir::WalkDir;

use crate::builtins::get_built_in_agents_with_context;
use crate::definition::{AgentDefinition, AgentSource};

/// JSON schema for agent definitions loaded from files.
#[derive(Debug, Deserialize)]
struct AgentFileEntry {
    description: String,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default, alias = "disallowedTools")]
    disallowed_tools: Vec<String>,
    prompt: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    effort: Option<serde_json::Value>,
    #[serde(default, alias = "permissionMode")]
    permission_mode: Option<String>,
    #[serde(default, alias = "mcpServers")]
    mcp_servers: Vec<serde_json::Value>,
    #[serde(default)]
    hooks: Option<serde_json::Value>,
    #[serde(default, alias = "maxTurns")]
    max_turns: Option<u32>,
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default, alias = "initialPrompt")]
    initial_prompt: Option<String>,
    #[serde(default, alias = "omitClaudeMd")]
    omit_claude_md: bool,
    #[serde(default)]
    color: Option<String>,
    #[serde(default, rename = "criticalSystemReminder_EXPERIMENTAL")]
    critical_system_reminder_experimental: Option<String>,
    #[serde(default, rename = "requiredMcpServers")]
    required_mcp_servers: Vec<String>,
    #[serde(default)]
    memory: Option<crate::definition::AgentMemoryScope>,
    #[serde(default)]
    background: bool,
}

/// Result of loading all agent definitions.
#[derive(Debug)]
pub struct AgentDefinitionsResult {
    /// Agents that are currently active (winning overrides).
    pub active_agents: Vec<AgentDefinition>,
    /// All agents including overridden ones.
    pub all_agents: Vec<AgentDefinition>,
    /// Files that failed to load with error messages.
    pub failed_files: Vec<(String, String)>,
}

/// Load all agent definitions from a directory.
///
/// Looks for `.md` files with YAML frontmatter and `.json` files containing
/// agent definitions. Each file becomes an agent definition with the filename
/// (without extension) as the `agent_type`.
pub fn load_agents_from_dir(dir: &Path, source: AgentSource) -> Result<Vec<AgentDefinition>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut agents = Vec::new();
    let mut paths = WalkDir::new(dir)
        .follow_links(true)
        .sort_by(|a, b| a.path().cmp(b.path()))
        .into_iter()
        .filter_map(|entry| match entry {
            Ok(entry) if entry.file_type().is_file() => Some(entry.into_path()),
            Ok(_) => None,
            Err(error) => {
                tracing::warn!("Skipping unreadable dir entry: {}", error);
                None
            }
        })
        .collect::<Vec<_>>();
    paths.sort();

    for path in paths {
        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        match extension {
            "md" => {
                if let Ok(agent) = load_agent_from_markdown(&path, source) {
                    agents.push(agent);
                }
            }
            "json" => {
                if let Ok(agent_list) = load_agents_from_json(&path, source) {
                    agents.extend(agent_list);
                }
            }
            _ => {}
        }
    }

    Ok(agents)
}

/// Load a single agent definition from a markdown file.
///
/// Parses YAML frontmatter for metadata (tools, model, etc.) and uses the
/// body as the system prompt.
pub fn load_agent_from_markdown(path: &Path, source: AgentSource) -> Result<AgentDefinition> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read agent file: {}", path.display()))?;

    let filename = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_owned();

    let (frontmatter, body) = parse_frontmatter(&content);

    let name = frontmatter
        .name
        .as_deref()
        .context("Missing required \"name\" field in agent frontmatter")?;
    let description = frontmatter
        .description
        .as_deref()
        .context("Missing required \"description\" field in agent frontmatter")?;

    let mut agent = AgentDefinition::new(name, description.replace("\\n", "\n"));
    agent.source = source;
    agent.base_dir = path
        .parent()
        .and_then(|p| p.to_str())
        .unwrap_or("")
        .to_owned();
    agent.filename = Some(filename.clone());
    agent.system_prompt = Some(body.trim().to_owned());

    if !frontmatter.tools.is_empty() {
        agent.tools = frontmatter.tools;
    }
    if !frontmatter.disallowed_tools.is_empty() {
        agent.disallowed_tools = frontmatter.disallowed_tools;
    }
    if let Some(model) = frontmatter.model {
        agent.model = Some(normalize_model(model));
    }
    if let Some(effort) = frontmatter.effort {
        agent.effort = Some(effort);
    }
    if let Some(permission_mode) = frontmatter.permission_mode {
        agent.permission_mode = Some(permission_mode);
    }
    if let Some(max_turns) = frontmatter.max_turns {
        agent.max_turns = max_turns;
    }
    if !frontmatter.skills.is_empty() {
        agent.skills = frontmatter.skills;
    }
    if !frontmatter.mcp_servers.is_empty() {
        agent.mcp_servers = frontmatter.mcp_servers;
    }
    if let Some(hooks) = frontmatter.hooks {
        agent.hooks = Some(hooks);
    }
    if let Some(initial_prompt) = frontmatter.initial_prompt {
        agent.initial_prompt = Some(initial_prompt);
    }
    agent.omit_claude_md = frontmatter.omit_claude_md;
    if let Some(color) = frontmatter.color {
        agent.color = Some(color);
    }
    if let Some(reminder) = frontmatter.critical_system_reminder_experimental {
        agent.critical_system_reminder_experimental = Some(reminder);
    }
    if !frontmatter.required_mcp_servers.is_empty() {
        agent.required_mcp_servers = frontmatter.required_mcp_servers;
    }
    if let Some(memory) = frontmatter.memory {
        agent.memory = Some(memory);
    }
    if frontmatter.background {
        agent.background = true;
    }

    Ok(agent)
}

/// Load agent definitions from a JSON file.
///
/// The JSON format is a map from agent type names to agent definitions:
/// ```json
/// {
///   "my-agent": {
///     "description": "...",
///     "prompt": "...",
///     "tools": ["Bash", "Read"]
///   }
/// }
/// ```
pub fn load_agents_from_json(path: &Path, source: AgentSource) -> Result<Vec<AgentDefinition>> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read agent JSON file: {}", path.display()))?;

    let entries: IndexMap<String, AgentFileEntry> = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse agent JSON file: {}", path.display()))?;

    let base_dir = path
        .parent()
        .and_then(|p| p.to_str())
        .unwrap_or("")
        .to_owned();

    let agents = entries
        .into_iter()
        .map(|(name, entry)| {
            let mut agent = AgentDefinition::new(&name, &entry.description);
            agent.source = source;
            agent.base_dir = base_dir.clone();
            agent.filename = Some(name.clone());
            agent.system_prompt = Some(entry.prompt);
            agent.tools = entry.tools;
            agent.disallowed_tools = entry.disallowed_tools;
            agent.model = entry.model.map(normalize_model);
            agent.effort = entry.effort;
            agent.permission_mode = entry.permission_mode;
            agent.mcp_servers = entry.mcp_servers;
            agent.hooks = entry.hooks;
            agent.max_turns = entry.max_turns.unwrap_or(200);
            agent.skills = entry.skills;
            agent.initial_prompt = entry.initial_prompt;
            agent.omit_claude_md = entry.omit_claude_md;
            agent.color = entry.color;
            agent.critical_system_reminder_experimental =
                entry.critical_system_reminder_experimental;
            agent.required_mcp_servers = entry.required_mcp_servers;
            agent.memory = entry.memory;
            agent.background = entry.background;
            agent
        })
        .collect();

    Ok(agents)
}

/// Load a single agent definition from any supported file format.
pub fn load_agent_from_file(path: &Path, source: AgentSource) -> Result<AgentDefinition> {
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match extension {
        "md" => load_agent_from_markdown(path, source),
        "json" => load_agents_from_json(path, source)?
            .into_iter()
            .next()
            .context("JSON file contained no agent definitions"),
        _ => anyhow::bail!(
            "Unsupported agent file format: .{} (expected .md or .json)",
            extension
        ),
    }
}

/// Parsed frontmatter from a markdown agent file.
#[derive(Debug, Default)]
struct Frontmatter {
    name: Option<String>,
    description: Option<String>,
    tools: Vec<String>,
    disallowed_tools: Vec<String>,
    model: Option<String>,
    effort: Option<serde_json::Value>,
    permission_mode: Option<String>,
    mcp_servers: Vec<serde_json::Value>,
    hooks: Option<serde_json::Value>,
    max_turns: Option<u32>,
    skills: Vec<String>,
    initial_prompt: Option<String>,
    omit_claude_md: bool,
    color: Option<String>,
    critical_system_reminder_experimental: Option<String>,
    required_mcp_servers: Vec<String>,
    memory: Option<crate::definition::AgentMemoryScope>,
    background: bool,
}

/// Parse YAML frontmatter from a markdown file.
///
/// Expects `---` delimiters at the start and end of the frontmatter block.
fn parse_frontmatter(content: &str) -> (Frontmatter, String) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (Frontmatter::default(), content.to_owned());
    }

    let rest = &trimmed[3..];
    let end = match rest.find("---") {
        Some(i) => i,
        None => return (Frontmatter::default(), content.to_owned()),
    };

    let yaml_str = &rest[..end];
    let body = rest[end + 3..].to_owned();

    let fm = parse_yaml_frontmatter(yaml_str);
    (fm, body)
}

/// Minimal YAML frontmatter parser for agent files.
fn parse_yaml_frontmatter(yaml: &str) -> Frontmatter {
    let mut fm = Frontmatter::default();

    for line in yaml.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            let value = value.trim();

            match key {
                "name" => {
                    fm.name = Some(unquote(value).to_owned());
                }
                "description" => {
                    fm.description = Some(unquote(value).to_owned());
                }
                "tools" => {
                    fm.tools = parse_yaml_list(value);
                }
                "disallowedTools" | "disallowed_tools" => {
                    fm.disallowed_tools = parse_yaml_list(value);
                }
                "model" => {
                    fm.model = Some(unquote(value).to_owned());
                }
                "effort" => {
                    fm.effort = Some(parse_yaml_value(value));
                }
                "permissionMode" | "permission_mode" => {
                    fm.permission_mode = Some(unquote(value).to_owned());
                }
                "mcpServers" | "mcp_servers" => {
                    fm.mcp_servers = parse_yaml_value_list(value);
                }
                "hooks" => {
                    fm.hooks = Some(parse_yaml_value(value));
                }
                "maxTurns" | "max_turns" => {
                    fm.max_turns = value.parse().ok();
                }
                "skills" => {
                    fm.skills = parse_yaml_list(value);
                }
                "initialPrompt" | "initial_prompt" => {
                    fm.initial_prompt = Some(unquote(value).to_owned());
                }
                "omitClaudeMd" | "omit_claude_md" => {
                    fm.omit_claude_md = parse_yaml_bool(value);
                }
                "color" => {
                    fm.color = Some(unquote(value).to_owned());
                }
                "criticalSystemReminder_EXPERIMENTAL" => {
                    fm.critical_system_reminder_experimental = Some(unquote(value).to_owned());
                }
                "requiredMcpServers" | "required_mcp_servers" => {
                    fm.required_mcp_servers = parse_yaml_list(value);
                }
                "background" => {
                    fm.background = parse_yaml_bool(value);
                }
                "memory" => {
                    fm.memory = match value {
                        "user" => Some(crate::definition::AgentMemoryScope::User),
                        "project" => Some(crate::definition::AgentMemoryScope::Project),
                        "local" => Some(crate::definition::AgentMemoryScope::Local),
                        _ => None,
                    };
                }
                _ => {}
            }
        }
    }

    fm
}

fn normalize_model(model: String) -> String {
    let trimmed = model.trim();
    if trimmed.eq_ignore_ascii_case("inherit") {
        "inherit".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn parse_yaml_bool(value: &str) -> bool {
    matches!(unquote(value), "true" | "True" | "TRUE")
}

/// Parse a YAML list value like `[a, b, c]`.
fn parse_yaml_list(value: &str) -> Vec<String> {
    let value = value.trim();
    if value.starts_with('[') && value.ends_with(']') {
        value[1..value.len() - 1]
            .split(',')
            .map(|s| unquote(s.trim()).to_owned())
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        Vec::new()
    }
}

fn parse_yaml_value_list(value: &str) -> Vec<serde_json::Value> {
    parse_yaml_list(value)
        .into_iter()
        .map(serde_json::Value::String)
        .collect()
}

fn parse_yaml_value(value: &str) -> serde_json::Value {
    let unquoted = unquote(value);
    if let Ok(parsed) = unquoted.parse::<i64>() {
        return serde_json::json!(parsed);
    }
    if let Ok(parsed) = unquoted.parse::<f64>() {
        return serde_json::json!(parsed);
    }
    match unquoted {
        "true" => serde_json::Value::Bool(true),
        "false" => serde_json::Value::Bool(false),
        _ => serde_json::Value::String(unquoted.to_owned()),
    }
}

/// Remove surrounding quotes from a YAML value.
fn unquote(s: &str) -> &str {
    s.trim()
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| s.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(s)
}

/// Load all agents: built-in + from directories.
///
/// Combines built-in agents with user-loaded agents from the given
/// directories, resolving overrides by agent type.
pub fn load_all_agents(
    user_dir: Option<&Path>,
    project_dir: Option<&Path>,
) -> AgentDefinitionsResult {
    load_all_agents_with_context(
        user_dir,
        project_dir,
        &RuntimeIdentityContext::from_legacy_env(),
    )
}

pub fn load_all_agents_with_context(
    user_dir: Option<&Path>,
    project_dir: Option<&Path>,
    ctx: &RuntimeIdentityContext,
) -> AgentDefinitionsResult {
    load_all_agents_with_options(user_dir, project_dir, ctx, simple_mode_enabled())
}

fn load_all_agents_with_options(
    user_dir: Option<&Path>,
    project_dir: Option<&Path>,
    ctx: &RuntimeIdentityContext,
    simple_mode: bool,
) -> AgentDefinitionsResult {
    let mut all_agents = get_built_in_agents_with_context(ctx);
    let mut failed_files = Vec::new();

    if !simple_mode {
        if let Some(dir) = user_dir {
            match load_agents_from_dir(dir, AgentSource::User) {
                Ok(agents) => all_agents.extend(agents),
                Err(e) => failed_files.push((dir.display().to_string(), e.to_string())),
            }
        }

        if let Some(dir) = project_dir {
            match load_agents_from_dir(dir, AgentSource::Project) {
                Ok(agents) => all_agents.extend(agents),
                Err(e) => failed_files.push((dir.display().to_string(), e.to_string())),
            }
        }
    }

    let active_agents = resolve_active_agents(&all_agents);

    AgentDefinitionsResult {
        active_agents,
        all_agents,
        failed_files,
    }
}

fn simple_mode_enabled() -> bool {
    std::env::var("CLAUDE_CODE_SIMPLE").is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// Resolve which agents are active using research precedence while preserving
/// first-seen ordering.
fn resolve_active_agents(all_agents: &[AgentDefinition]) -> Vec<AgentDefinition> {
    let mut active = IndexMap::<String, AgentDefinition>::new();
    for source in [
        AgentSource::BuiltIn,
        AgentSource::Plugin,
        AgentSource::Marketplace,
        AgentSource::User,
        AgentSource::Project,
        AgentSource::Local,
        AgentSource::Flag,
        AgentSource::Policy,
    ] {
        for agent in all_agents.iter().filter(|agent| agent.source == source) {
            active.insert(agent.agent_type.clone(), agent.clone());
        }
    }

    active.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_from_nonexistent_dir_returns_empty() {
        let dir = Path::new("/nonexistent/path");
        let result = load_agents_from_dir(dir, AgentSource::User);
        assert!(result.is_ok());
        assert!(result.expect("ok").is_empty());
    }

    #[test]
    fn load_agent_from_json_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let json = r#"{
            "my-agent": {
                "description": "My custom agent",
                "prompt": "You are a custom agent",
                "tools": ["Bash", "Read"]
            }
        }"#;
        let path = dir.path().join("agents.json");
        fs::write(&path, json).expect("write");

        let agents = load_agents_from_json(&path, AgentSource::User).expect("load");
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].agent_type, "my-agent");
        assert_eq!(agents[0].tools, vec!["Bash", "Read"]);
        assert_eq!(agents[0].source, AgentSource::User);
    }

    #[test]
    fn load_agents_from_json_preserves_insertion_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        let json = r#"{
            "z-agent": {"description": "Z", "prompt": "z"},
            "a-agent": {"description": "A", "prompt": "a"}
        }"#;
        let path = dir.path().join("agents.json");
        fs::write(&path, json).expect("write");

        let agents = load_agents_from_json(&path, AgentSource::User).expect("load");
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].agent_type, "z-agent");
        assert_eq!(agents[1].agent_type, "a-agent");
    }

    #[test]
    fn load_agent_from_markdown_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let md = "---\nname: test-agent\ndescription: Test agent\ntools: [Bash]\n---\nYou are a test agent.\n";
        let path = dir.path().join("test-agent.md");
        fs::write(&path, md).expect("write");

        let agent = load_agent_from_markdown(&path, AgentSource::Project).expect("load");
        assert_eq!(agent.agent_type, "test-agent");
        assert_eq!(agent.when_to_use, "Test agent");
        assert_eq!(agent.tools, vec!["Bash"]);
        assert_eq!(agent.source, AgentSource::Project);
        assert_eq!(
            agent.system_prompt.as_deref(),
            Some("You are a test agent.")
        );
    }

    #[test]
    fn load_agent_from_file_dispatches_by_extension() {
        let dir = tempfile::tempdir().expect("tempdir");

        // JSON
        let json = r#"{"a": {"description": "d", "prompt": "p"}}"#;
        let json_path = dir.path().join("agents.json");
        fs::write(&json_path, json).expect("write");
        let agent = load_agent_from_file(&json_path, AgentSource::User).expect("load json");
        assert_eq!(agent.agent_type, "a");

        // Markdown
        let md = "---\nname: my-agent\ndescription: Test\n---\nBody text\n";
        let md_path = dir.path().join("my-agent.md");
        fs::write(&md_path, md).expect("write");
        let agent = load_agent_from_file(&md_path, AgentSource::User).expect("load md");
        assert_eq!(agent.agent_type, "my-agent");
    }

    #[test]
    fn load_agents_from_dir_skips_markdown_without_agent_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let md = "---\ndescription: Reference doc, not an agent\n---\nBody text\n";
        fs::write(dir.path().join("reference.md"), md).expect("write");

        let agents = load_agents_from_dir(dir.path(), AgentSource::Project).expect("load dir");
        assert!(agents.is_empty());
    }

    #[test]
    fn load_agents_from_dir_recurses_into_nested_directories() {
        let dir = tempfile::tempdir().expect("tempdir");
        let nested = dir.path().join("nested").join("deeper");
        fs::create_dir_all(&nested).expect("nested agents dir");
        fs::write(
            nested.join("reviewer.md"),
            "---\nname: reviewer\ndescription: Nested reviewer\n---\nUse nested prompt.\n",
        )
        .expect("write nested agent");

        let agents = load_agents_from_dir(dir.path(), AgentSource::Project).expect("load dir");
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].agent_type, "reviewer");
    }

    #[test]
    fn load_agent_metadata_fields_from_json() {
        let dir = tempfile::tempdir().expect("tempdir");
        let json = r#"{
            "verify": {
                "description": "Verify",
                "prompt": "You verify",
                "disallowedTools": ["Edit"],
                "permissionMode": "dontAsk",
                "mcpServers": ["context7"],
                "hooks": {"PreToolUse": []},
                "maxTurns": 42,
                "initialPrompt": "Start here",
                "omitClaudeMd": true,
                "color": "red",
                "criticalSystemReminder_EXPERIMENTAL": "remember",
                "requiredMcpServers": ["MiniMax"],
                "effort": 80,
                "background": true
            }
        }"#;
        let path = dir.path().join("agents.json");
        fs::write(&path, json).expect("write");

        let agents = load_agents_from_json(&path, AgentSource::User).expect("load");
        let agent = &agents[0];
        assert_eq!(agent.disallowed_tools, vec!["Edit"]);
        assert_eq!(agent.permission_mode.as_deref(), Some("dontAsk"));
        assert_eq!(agent.mcp_servers, vec![serde_json::json!("context7")]);
        assert!(agent.hooks.is_some());
        assert_eq!(agent.max_turns, 42);
        assert_eq!(agent.initial_prompt.as_deref(), Some("Start here"));
        assert!(agent.omit_claude_md);
        assert_eq!(agent.color.as_deref(), Some("red"));
        assert_eq!(
            agent.critical_system_reminder_experimental.as_deref(),
            Some("remember")
        );
        assert_eq!(agent.required_mcp_servers, vec!["MiniMax"]);
        assert_eq!(agent.effort, Some(serde_json::json!(80)));
        assert!(agent.background);
    }

    #[test]
    fn load_agent_metadata_fields_from_markdown() {
        let dir = tempfile::tempdir().expect("tempdir");
        let md = "---\nname: verify\ndescription: Verify\ndisallowedTools: [Edit]\npermissionMode: dontAsk\nmcpServers: [context7]\nmaxTurns: 42\ninitialPrompt: Start here\nomitClaudeMd: true\ncolor: red\ncriticalSystemReminder_EXPERIMENTAL: remember\nrequiredMcpServers: [MiniMax]\neffort: 80\nbackground: true\n---\nYou verify.\n";
        let path = dir.path().join("verify.md");
        fs::write(&path, md).expect("write");

        let agent = load_agent_from_markdown(&path, AgentSource::Project).expect("load");
        assert_eq!(agent.agent_type, "verify");
        assert_eq!(agent.disallowed_tools, vec!["Edit"]);
        assert_eq!(agent.permission_mode.as_deref(), Some("dontAsk"));
        assert_eq!(agent.mcp_servers, vec![serde_json::json!("context7")]);
        assert_eq!(agent.max_turns, 42);
        assert_eq!(agent.initial_prompt.as_deref(), Some("Start here"));
        assert!(agent.omit_claude_md);
        assert_eq!(agent.color.as_deref(), Some("red"));
        assert_eq!(
            agent.critical_system_reminder_experimental.as_deref(),
            Some("remember")
        );
        assert_eq!(agent.required_mcp_servers, vec!["MiniMax"]);
        assert_eq!(agent.effort, Some(serde_json::json!(80)));
        assert!(agent.background);
    }

    #[test]
    fn unsupported_extension_returns_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("agent.yaml");
        fs::write(&path, "key: value").expect("write");
        let result = load_agent_from_file(&path, AgentSource::User);
        assert!(result.is_err());
    }

    #[test]
    fn load_all_combines_builtins_and_custom() {
        let dir = tempfile::tempdir().expect("tempdir");
        let json = r#"{"custom": {"description": "Custom", "prompt": "Do stuff"}}"#;
        fs::write(dir.path().join("agents.json"), json).expect("write");

        let ctx = RuntimeIdentityContext {
            features: RuntimeFeatureGates {
                explore_plan_agents_enabled: true,
                code_guide_enabled: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let result = load_all_agents_with_context(Some(dir.path()), None, &ctx);
        // Should have built-in agents + custom
        assert!(result.active_agents.len() > 5);
        assert!(
            result
                .active_agents
                .iter()
                .any(|a| a.agent_type == "custom")
        );
    }

    #[test]
    fn resolve_active_agents_deduplicates() {
        let agents = vec![AgentDefinition::new("test", "built-in"), {
            let mut d = AgentDefinition::new("test", "user override");
            d.source = AgentSource::User;
            d
        }];

        let active = resolve_active_agents(&agents);
        // Should have exactly one "test" agent
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].source, AgentSource::User);
    }

    #[test]
    fn resolve_active_agents_preserves_first_seen_order_while_overriding_values() {
        let built_in_alpha = AgentDefinition::new("alpha", "built-in alpha");
        let built_in_beta = AgentDefinition::new("beta", "built-in beta");
        let user_beta = {
            let mut definition = AgentDefinition::new("beta", "user beta");
            definition.source = AgentSource::User;
            definition
        };

        let active = resolve_active_agents(&[built_in_alpha, built_in_beta, user_beta]);
        assert_eq!(active.len(), 2);
        assert_eq!(active[0].agent_type, "alpha");
        assert_eq!(active[1].agent_type, "beta");
        assert_eq!(active[1].source, AgentSource::User);
    }

    #[test]
    fn simple_mode_skips_custom_agent_loading() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(
            dir.path().join("custom.md"),
            "---\nname: custom\ndescription: Custom agent\n---\nCustom prompt.\n",
        )
        .expect("write custom agent");

        let result = load_all_agents_with_options(
            Some(dir.path()),
            Some(dir.path()),
            &RuntimeIdentityContext::from_legacy_env(),
            true,
        );

        assert!(
            !result
                .active_agents
                .iter()
                .any(|agent| agent.agent_type == "custom")
        );
        assert!(
            !result
                .all_agents
                .iter()
                .any(|agent| agent.agent_type == "custom")
        );
    }

    #[test]
    fn parse_yaml_list_brackets() {
        let result = parse_yaml_list("[Bash, Read, Write]");
        assert_eq!(result, vec!["Bash", "Read", "Write"]);
    }

    #[test]
    fn parse_yaml_list_empty() {
        let result = parse_yaml_list("[]");
        assert!(result.is_empty());
    }

    #[test]
    fn parse_yaml_list_non_list() {
        let result = parse_yaml_list("not a list");
        assert!(result.is_empty());
    }

    #[test]
    fn unquote_removes_double_quotes() {
        assert_eq!(unquote("\"hello\""), "hello");
    }

    #[test]
    fn unquote_removes_single_quotes() {
        assert_eq!(unquote("'hello'"), "hello");
    }

    #[test]
    fn unquote_no_quotes() {
        assert_eq!(unquote("hello"), "hello");
    }
}
