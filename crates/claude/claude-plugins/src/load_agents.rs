//! Agent loading from plugin manifests.
//!
//! Extracts agent configurations from plugin directories. Agents are defined
//! as markdown files in the plugin's `agents/` directory.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::markdown_walker::walk_markdown_paths;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A plugin agent configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginAgentConfig {
    /// Fully-qualified agent name (e.g., `"plugin-name:agent-name"`).
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Source file path.
    pub file_path: PathBuf,
    /// Plugin name that provides this agent.
    pub plugin_name: String,
    /// Agent system prompt / instructions.
    pub prompt: String,
    /// Optional model override for the agent.
    #[serde(default)]
    pub model: Option<String>,
    /// Optional tools allowlist for the agent.
    #[serde(default)]
    pub tools: Vec<String>,
}

/// Result of loading agents from a plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadAgentsResult {
    /// Agents found.
    pub agents: Vec<PluginAgentConfig>,
    /// Errors encountered.
    pub errors: Vec<String>,
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// Load plugin agents from a directory.
///
/// Walks the `agents/` directory and extracts agent definitions from
/// markdown files.
pub fn load_plugin_agents(plugin_name: &str, agents_dir: &Path) -> LoadAgentsResult {
    let mut agents = Vec::new();
    let mut errors = Vec::new();

    if !agents_dir.exists() {
        return LoadAgentsResult { agents, errors };
    }

    if !agents_dir.is_dir() {
        errors.push(format!(
            "agents path {} is not a directory",
            agents_dir.display()
        ));
        return LoadAgentsResult { agents, errors };
    }

    let markdown_entries = walk_markdown_paths(agents_dir);

    for (file_path, _namespace) in markdown_entries {
        let relative = file_path.strip_prefix(agents_dir).unwrap_or(&file_path);

        let agent_name = build_agent_name(plugin_name, relative);

        let content = match std::fs::read_to_string(&file_path) {
            Ok(c) => c,
            Err(e) => {
                errors.push(format!("failed to read agent {}: {e}", file_path.display()));
                continue;
            }
        };

        let (description, prompt) = parse_agent_content(&content, &agent_name);

        agents.push(PluginAgentConfig {
            name: agent_name,
            description,
            file_path: file_path.clone(),
            plugin_name: plugin_name.to_owned(),
            prompt,
            model: None,
            tools: Vec::new(),
        });
    }

    agents.sort_by(|a, b| a.name.cmp(&b.name));

    LoadAgentsResult { agents, errors }
}

/// Build a fully-qualified agent name from a plugin name and file path.
pub fn build_agent_name(plugin_name: &str, relative_path: &Path) -> String {
    let file_stem = relative_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");

    let parent = relative_path.parent();
    let namespace_parts: Vec<&str> = parent
        .and_then(|p| p.to_str())
        .map(|s| {
            s.split([std::path::MAIN_SEPARATOR, '/'])
                .filter(|part| !part.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let mut parts = vec![plugin_name];
    parts.extend(namespace_parts);
    parts.push(file_stem);

    parts.join(":")
}

/// Parse agent content into description and prompt.
fn parse_agent_content(content: &str, fallback_name: &str) -> (String, String) {
    let lines: Vec<&str> = content.lines().collect();

    // Try to extract description from the first heading
    let description = lines
        .iter()
        .find(|line| line.starts_with('#'))
        .map(|line| line.trim_start_matches('#').trim().to_owned())
        .unwrap_or_else(|| fallback_name.to_owned());

    // The prompt is the full content (minus frontmatter if any)
    let prompt = content.trim().to_owned();

    (description, prompt)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn ok<T, E: std::fmt::Display>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("unexpected error: {error}"),
        }
    }

    #[test]
    fn build_agent_name_simple() {
        assert_eq!(
            build_agent_name("my-plugin", Path::new("test-runner.md")),
            "my-plugin:test-runner"
        );
    }

    #[test]
    fn build_agent_name_nested() {
        assert_eq!(
            build_agent_name("my-plugin", Path::new("ci/test-runner.md")),
            "my-plugin:ci:test-runner"
        );
    }

    #[test]
    fn load_plugin_agents_from_directory() {
        let temp = ok(tempdir());
        let agents_dir = temp.path().join("agents");
        fs::create_dir_all(&agents_dir).expect("create dir");
        fs::write(
            agents_dir.join("test-runner.md"),
            "# Test Runner\nRun the test suite.",
        )
        .expect("write");
        fs::write(agents_dir.join("linter.md"), "# Linter\nLint the codebase.").expect("write");

        let result = load_plugin_agents("my-plugin", &agents_dir);
        assert_eq!(result.agents.len(), 2);
        assert!(result.errors.is_empty());

        let names: Vec<&str> = result.agents.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"my-plugin:linter"));
        assert!(names.contains(&"my-plugin:test-runner"));
    }

    #[test]
    fn load_plugin_agents_nonexistent_directory() {
        let result = load_plugin_agents("my-plugin", Path::new("/nonexistent/agents"));
        assert!(result.agents.is_empty());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn load_plugin_agents_extracts_description() {
        let temp = ok(tempdir());
        let agents_dir = temp.path().join("agents");
        fs::create_dir_all(&agents_dir).expect("create dir");
        fs::write(
            agents_dir.join("test-runner.md"),
            "# My Test Runner\nRun the test suite.",
        )
        .expect("write");

        let result = load_plugin_agents("my-plugin", &agents_dir);
        assert_eq!(result.agents.len(), 1);
        assert_eq!(result.agents[0].description, "My Test Runner");
    }

    #[test]
    fn load_plugin_agents_not_a_directory() {
        let temp = ok(tempdir());
        let file = temp.path().join("notadir.md");
        fs::write(&file, "content").expect("write");

        let result = load_plugin_agents("my-plugin", &file);
        assert!(result.agents.is_empty());
        assert_eq!(result.errors.len(), 1);
    }
}
