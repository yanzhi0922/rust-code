//! Command loading from plugin manifests.
//!
//! Extracts slash command definitions from plugin directories. Commands are
//! defined as markdown files (`.md`) in the plugin's `commands/` directory.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::markdown_walker::walk_markdown_paths;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A plugin command definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginCommand {
    /// Fully-qualified command name (e.g., `"plugin-name:command-name"`).
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Source file path.
    pub file_path: PathBuf,
    /// Plugin name that provides this command.
    pub plugin_name: String,
    /// Whether this is a skill command (from SKILL.md).
    #[serde(default)]
    pub is_skill: bool,
}

/// Result of loading commands from a plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadCommandsResult {
    /// Commands found.
    pub commands: Vec<PluginCommand>,
    /// Errors encountered.
    pub errors: Vec<String>,
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// Load plugin commands from a directory.
///
/// Walks the `commands/` directory (or the directory specified by the manifest)
/// and extracts command definitions from markdown files.
pub fn load_plugin_commands(plugin_name: &str, commands_dir: &Path) -> LoadCommandsResult {
    let mut commands = Vec::new();
    let mut errors = Vec::new();

    if !commands_dir.exists() {
        return LoadCommandsResult { commands, errors };
    }

    if !commands_dir.is_dir() {
        errors.push(format!(
            "commands path {} is not a directory",
            commands_dir.display()
        ));
        return LoadCommandsResult { commands, errors };
    }

    let markdown_entries = walk_markdown_paths(commands_dir);

    for (file_path, _namespace) in markdown_entries {
        let relative = file_path.strip_prefix(commands_dir).unwrap_or(&file_path);

        let command_name = build_command_name(plugin_name, relative);

        // Read first line as description (or use filename)
        let description = std::fs::read_to_string(&file_path)
            .ok()
            .and_then(|content| {
                content
                    .lines()
                    .find(|line| !line.trim().is_empty())
                    .map(|line| line.trim().to_owned())
            })
            .unwrap_or_else(|| command_name.clone());

        let is_skill = file_path
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("skill.md"));

        commands.push(PluginCommand {
            name: command_name,
            description,
            file_path: file_path.clone(),
            plugin_name: plugin_name.to_owned(),
            is_skill,
        });
    }

    commands.sort_by(|a, b| a.name.cmp(&b.name));

    LoadCommandsResult { commands, errors }
}

/// Build a fully-qualified command name from a plugin name and file path.
///
/// Examples:
/// - `("my-plugin", "build.md")` → `"my-plugin:build"`
/// - `("my-plugin", "deploy/prod.md")` → `"my-plugin:deploy:prod"`
/// - `("my-plugin", "skills/demo/SKILL.md")` → `"my-plugin:demo"`
pub fn build_command_name(plugin_name: &str, relative_path: &Path) -> String {
    let file_name = relative_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");

    // For SKILL.md, use the parent directory name
    let is_skill = file_name.eq_ignore_ascii_case("skill");
    let base_name = if is_skill {
        relative_path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or(file_name)
    } else {
        file_name
    };

    // Build namespace from parent directories
    let parent = if is_skill {
        relative_path.parent().and_then(|p| p.parent())
    } else {
        relative_path.parent()
    };

    let namespace_parts: Vec<&str> = parent
        .and_then(|p| p.to_str())
        .map(|s| {
            s.split(std::path::MAIN_SEPARATOR)
                .flat_map(|part| part.split('/'))
                .filter(|part: &&str| !part.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let mut parts = vec![plugin_name];
    parts.extend(namespace_parts);
    parts.push(base_name);

    parts.join(":")
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
    fn build_command_name_simple() {
        assert_eq!(
            build_command_name("my-plugin", Path::new("build.md")),
            "my-plugin:build"
        );
    }

    #[test]
    fn build_command_name_nested() {
        assert_eq!(
            build_command_name("my-plugin", Path::new("deploy/prod.md")),
            "my-plugin:deploy:prod"
        );
    }

    #[test]
    fn build_command_name_skill_file() {
        assert_eq!(
            build_command_name("my-plugin", Path::new("demo/SKILL.md")),
            "my-plugin:demo"
        );
    }

    #[test]
    fn load_plugin_commands_from_directory() {
        let temp = ok(tempdir());
        let cmd_dir = temp.path().join("commands");
        fs::create_dir_all(&cmd_dir).expect("create dir");
        fs::write(cmd_dir.join("build.md"), "# Build\nBuild the project.").expect("write");
        fs::write(cmd_dir.join("deploy.md"), "# Deploy\nDeploy to prod.").expect("write");

        let result = load_plugin_commands("my-plugin", &cmd_dir);
        assert_eq!(result.commands.len(), 2);
        assert!(result.errors.is_empty());

        let names: Vec<&str> = result.commands.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"my-plugin:build"));
        assert!(names.contains(&"my-plugin:deploy"));
    }

    #[test]
    fn load_plugin_commands_nonexistent_directory() {
        let result = load_plugin_commands("my-plugin", Path::new("/nonexistent/commands"));
        assert!(result.commands.is_empty());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn load_plugin_commands_not_a_directory() {
        let temp = ok(tempdir());
        let file = temp.path().join("notadir.md");
        fs::write(&file, "content").expect("write");

        let result = load_plugin_commands("my-plugin", &file);
        assert!(result.commands.is_empty());
        assert_eq!(result.errors.len(), 1);
    }

    #[test]
    fn load_plugin_commands_nested_directories() {
        let temp = ok(tempdir());
        let cmd_dir = temp.path().join("commands");
        let nested = cmd_dir.join("deploy");
        fs::create_dir_all(&nested).expect("create dir");
        fs::write(nested.join("prod.md"), "# Deploy Prod\nDeploy to prod.").expect("write");

        let result = load_plugin_commands("my-plugin", &cmd_dir);
        assert_eq!(result.commands.len(), 1);
        assert_eq!(result.commands[0].name, "my-plugin:deploy:prod");
    }
}
