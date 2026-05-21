//! Output style loading from plugin manifests.
//!
//! Extracts output style configurations from plugin directories. Output styles
//! are defined as markdown files in the plugin's `output-styles/` directory.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::PluginBundle;
use crate::markdown_walker::walk_markdown_paths;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A plugin output style configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginOutputStyle {
    /// Fully-qualified style name (e.g., `"plugin-name:style-name"`).
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Source file path.
    pub file_path: PathBuf,
    /// Plugin name that provides this style.
    pub plugin_name: String,
    /// Style prompt / template.
    pub prompt: String,
    /// Whether this style should be forced for the plugin.
    #[serde(default)]
    pub force_for_plugin: Option<bool>,
    /// Whether default coding instructions should remain active.
    #[serde(default)]
    pub keep_coding_instructions: Option<bool>,
}

/// Result of loading output styles from a plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadOutputStylesResult {
    /// Styles found.
    pub styles: Vec<PluginOutputStyle>,
    /// Errors encountered.
    pub errors: Vec<String>,
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// Load plugin output styles from a directory.
///
/// Walks the `output-styles/` directory and extracts style definitions
/// from markdown files.
pub fn load_plugin_output_styles(plugin_name: &str, styles_dir: &Path) -> LoadOutputStylesResult {
    if !styles_dir.exists() {
        return LoadOutputStylesResult {
            styles: Vec::new(),
            errors: Vec::new(),
        };
    }
    if !styles_dir.is_dir() {
        return LoadOutputStylesResult {
            styles: Vec::new(),
            errors: vec![format!(
                "output-styles path {} is not a directory",
                styles_dir.display()
            )],
        };
    }
    load_plugin_output_styles_from_paths(plugin_name, &[styles_dir.to_path_buf()])
}

/// Load all output styles declared by a plugin bundle.
///
/// The default `output-styles/` directory is loaded first, followed by any
/// extra files or directories referenced by `manifest.outputStyles`.
#[must_use]
pub fn load_plugin_bundle_output_styles(plugin: &PluginBundle) -> LoadOutputStylesResult {
    let mut paths = Vec::new();
    if let Some(default_path) = plugin.default_output_styles_path() {
        paths.push(default_path);
    }
    paths.extend(plugin.output_styles_paths());
    load_plugin_output_styles_from_paths(&plugin.manifest.name, &paths)
}

fn load_plugin_output_styles_from_paths(
    plugin_name: &str,
    style_paths: &[PathBuf],
) -> LoadOutputStylesResult {
    let mut styles = Vec::new();
    let mut errors = Vec::new();
    let mut loaded_paths = HashSet::new();

    for style_path in style_paths {
        if !style_path.exists() {
            continue;
        }
        if style_path.is_file() {
            if style_path.extension().and_then(|ext| ext.to_str()) == Some("md") {
                load_style_file(
                    plugin_name,
                    style_path,
                    &mut loaded_paths,
                    &mut styles,
                    &mut errors,
                );
            }
            continue;
        }
        if !style_path.is_dir() {
            errors.push(format!(
                "output-styles path {} is not a directory or markdown file",
                style_path.display()
            ));
            continue;
        }

        for (file_path, _namespace) in walk_markdown_paths(style_path) {
            load_style_file(
                plugin_name,
                &file_path,
                &mut loaded_paths,
                &mut styles,
                &mut errors,
            );
        }
    }

    styles.sort_by(|a, b| a.name.cmp(&b.name));

    LoadOutputStylesResult { styles, errors }
}

fn load_style_file(
    plugin_name: &str,
    file_path: &Path,
    loaded_paths: &mut HashSet<PathBuf>,
    styles: &mut Vec<PluginOutputStyle>,
    errors: &mut Vec<String>,
) {
    let identity = std::fs::canonicalize(file_path).unwrap_or_else(|_| file_path.to_path_buf());
    if !loaded_paths.insert(identity) {
        return;
    }

    let file_stem = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");

    let content = match std::fs::read_to_string(file_path) {
        Ok(c) => c,
        Err(e) => {
            errors.push(format!(
                "failed to read output style {}: {e}",
                file_path.display()
            ));
            return;
        }
    };

    let parsed = parse_style_content(&content, file_stem, plugin_name);

    styles.push(PluginOutputStyle {
        name: parsed.name,
        description: parsed.description,
        file_path: file_path.to_path_buf(),
        plugin_name: plugin_name.to_owned(),
        prompt: parsed.prompt,
        force_for_plugin: parsed.force_for_plugin,
        keep_coding_instructions: parsed.keep_coding_instructions,
    });
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedStyleContent {
    name: String,
    description: String,
    prompt: String,
    force_for_plugin: Option<bool>,
    keep_coding_instructions: Option<bool>,
}

/// Parse style content into description and prompt.
fn parse_style_content(content: &str, file_stem: &str, plugin_name: &str) -> ParsedStyleContent {
    let (frontmatter, body) = split_frontmatter(content);
    let base_name = frontmatter
        .as_ref()
        .and_then(|fm| frontmatter_string(fm, "name"))
        .unwrap_or_else(|| file_stem.to_owned());
    let style_name = format!("{plugin_name}:{base_name}");

    // Extract description from first heading or first non-empty paragraph
    let lines = body.lines().collect::<Vec<_>>();
    let description = frontmatter
        .as_ref()
        .and_then(|fm| frontmatter_string(fm, "description"))
        .or_else(|| {
            lines
                .iter()
                .find(|line| line.starts_with('#'))
                .map(|line| line.trim_start_matches('#').trim().to_owned())
        })
        .or_else(|| {
            lines
                .iter()
                .find(|line| !line.trim().is_empty())
                .map(|line| line.trim().to_owned())
        })
        .unwrap_or_else(|| format!("Output style from {plugin_name} plugin"));

    ParsedStyleContent {
        name: style_name,
        description,
        prompt: body.trim().to_owned(),
        force_for_plugin: frontmatter
            .as_ref()
            .and_then(|fm| frontmatter_bool(fm, "force-for-plugin")),
        keep_coding_instructions: frontmatter
            .as_ref()
            .and_then(|fm| frontmatter_bool(fm, "keep-coding-instructions")),
    }
}

fn split_frontmatter(content: &str) -> (Option<String>, String) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (None, content.to_owned());
    }
    let rest = &trimmed[3..];
    let Some(end) = rest.find("\n---") else {
        return (None, content.to_owned());
    };
    let frontmatter = rest[..end].to_owned();
    let mut body = rest[end + 4..].to_owned();
    if let Some(stripped) = body.strip_prefix('\r') {
        body = stripped.to_owned();
    }
    if let Some(stripped) = body.strip_prefix('\n') {
        body = stripped.to_owned();
    }
    (Some(frontmatter), body)
}

fn frontmatter_string(frontmatter: &str, key: &str) -> Option<String> {
    let value = frontmatter_value(frontmatter, key)?;
    let value = unquote(value.trim());
    (!value.is_empty()).then(|| value.to_owned())
}

fn frontmatter_bool(frontmatter: &str, key: &str) -> Option<bool> {
    let value = unquote(frontmatter_value(frontmatter, key)?.trim());
    match value.to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn frontmatter_value<'a>(frontmatter: &'a str, key: &str) -> Option<&'a str> {
    frontmatter.lines().find_map(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        let (candidate, value) = line.split_once(':')?;
        (candidate.trim() == key).then_some(value.trim())
    })
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|inner| inner.strip_suffix('\''))
        })
        .unwrap_or(value)
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
    fn load_plugin_output_styles_basic() {
        let temp = ok(tempdir());
        let styles_dir = temp.path().join("output-styles");
        fs::create_dir_all(&styles_dir).expect("create dir");
        fs::write(
            styles_dir.join("concise.md"),
            "# Concise\nOutput in a concise format.",
        )
        .expect("write");
        fs::write(
            styles_dir.join("detailed.md"),
            "# Detailed\nOutput with full details.",
        )
        .expect("write");

        let result = load_plugin_output_styles("my-plugin", &styles_dir);
        assert_eq!(result.styles.len(), 2);
        assert!(result.errors.is_empty());

        let names: Vec<&str> = result.styles.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"my-plugin:concise"));
        assert!(names.contains(&"my-plugin:detailed"));
    }

    #[test]
    fn load_plugin_output_styles_nonexistent() {
        let result = load_plugin_output_styles("my-plugin", Path::new("/nonexistent/styles"));
        assert!(result.styles.is_empty());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn load_plugin_output_styles_extracts_description() {
        let temp = ok(tempdir());
        let styles_dir = temp.path().join("output-styles");
        fs::create_dir_all(&styles_dir).expect("create dir");
        fs::write(
            styles_dir.join("concise.md"),
            "# My Concise Style\nBe brief.",
        )
        .expect("write");

        let result = load_plugin_output_styles("my-plugin", &styles_dir);
        assert_eq!(result.styles.len(), 1);
        assert_eq!(result.styles[0].description, "My Concise Style");
    }

    #[test]
    fn load_plugin_output_styles_parses_frontmatter_flags() {
        let temp = ok(tempdir());
        let styles_dir = temp.path().join("output-styles");
        fs::create_dir_all(&styles_dir).expect("create dir");
        fs::write(
            styles_dir.join("coach.md"),
            "---\nname: Pair Coach\ndescription: Pairing guidance\nforce-for-plugin: true\nkeep-coding-instructions: false\n---\n# Ignored heading\nUse pair-programming style.",
        )
        .expect("write");

        let result = load_plugin_output_styles("my-plugin", &styles_dir);

        assert!(result.errors.is_empty());
        assert_eq!(result.styles.len(), 1);
        assert_eq!(result.styles[0].name, "my-plugin:Pair Coach");
        assert_eq!(result.styles[0].description, "Pairing guidance");
        assert_eq!(result.styles[0].force_for_plugin, Some(true));
        assert_eq!(result.styles[0].keep_coding_instructions, Some(false));
        assert!(!result.styles[0].prompt.contains("force-for-plugin"));
        assert!(
            result.styles[0]
                .prompt
                .contains("Use pair-programming style.")
        );
    }

    #[test]
    fn load_plugin_output_styles_not_a_directory() {
        let temp = ok(tempdir());
        let file = temp.path().join("notadir.md");
        fs::write(&file, "content").expect("write");

        let result = load_plugin_output_styles("my-plugin", &file);
        assert!(result.styles.is_empty());
        assert_eq!(result.errors.len(), 1);
    }
}
