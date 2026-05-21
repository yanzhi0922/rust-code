//! Code indexing tool detection.
//!
//! Detects installed code indexing tools and provides their CLI commands
//! for integration with the remote-code-rust ecosystem.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// CodeIndexingTool enum
// ---------------------------------------------------------------------------

/// Known code indexing tools.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum CodeIndexingTool {
    /// Sourcegraph (src CLI).
    Sourcegraph,
    /// Cody (Sourcegraph AI assistant).
    Cody,
    /// Aider (AI pair programming).
    Aider,
    /// Cursor (AI code editor).
    Cursor,
    /// GitHub Copilot.
    Copilot,
    /// ctags (universal-ctags).
    Ctags,
    /// ripgrep (rg).
    Ripgrep,
    /// LLVM/Clangd index.
    Clangd,
}

impl CodeIndexingTool {
    /// Return the display name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Sourcegraph => "Sourcegraph",
            Self::Cody => "Cody",
            Self::Aider => "Aider",
            Self::Cursor => "Cursor",
            Self::Copilot => "Copilot",
            Self::Ctags => "ctags",
            Self::Ripgrep => "ripgrep",
            Self::Clangd => "clangd",
        }
    }

    /// All known code indexing tools.
    #[must_use]
    pub fn all_values() -> &'static [CodeIndexingTool] {
        &[
            CodeIndexingTool::Sourcegraph,
            CodeIndexingTool::Cody,
            CodeIndexingTool::Aider,
            CodeIndexingTool::Cursor,
            CodeIndexingTool::Copilot,
            CodeIndexingTool::Ctags,
            CodeIndexingTool::Ripgrep,
            CodeIndexingTool::Clangd,
        ]
    }
}

impl std::fmt::Display for CodeIndexingTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

// ---------------------------------------------------------------------------
// CLI command
// ---------------------------------------------------------------------------

/// Get the CLI command for a code indexing tool.
///
/// # Arguments
///
/// * `tool` — The code indexing tool.
///
/// # Returns
///
/// The CLI command name (e.g. `"src"`, `"aider"`), or `None` if the tool
/// doesn't have a CLI interface.
#[must_use]
pub fn cli_command_for_tool(tool: CodeIndexingTool) -> Option<&'static str> {
    match tool {
        CodeIndexingTool::Sourcegraph => Some("src"),
        CodeIndexingTool::Cody => Some("cody"),
        CodeIndexingTool::Aider => Some("aider"),
        CodeIndexingTool::Cursor => None, // GUI only.
        CodeIndexingTool::Copilot => Some("github-copilot-cli"),
        CodeIndexingTool::Ctags => Some("ctags"),
        CodeIndexingTool::Ripgrep => Some("rg"),
        CodeIndexingTool::Clangd => Some("clangd"),
    }
}

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

/// Result of detecting a code indexing tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedTool {
    /// The tool that was detected.
    pub tool: CodeIndexingTool,
    /// The path to the tool's binary, if found.
    pub path: Option<String>,
    /// The version string, if detected.
    pub version: Option<String>,
}

/// Detect installed code indexing tools.
///
/// Checks for the presence of known tools by looking for their CLI commands
/// in the system PATH.
///
/// # Returns
///
/// A vector of detected tools.
pub fn detect_indexing_tools() -> Vec<DetectedTool> {
    let mut detected = Vec::new();

    for tool in CodeIndexingTool::all_values() {
        if let Some(cmd) = cli_command_for_tool(*tool)
            && let Some(path) = find_in_path(cmd)
        {
            detected.push(DetectedTool {
                tool: *tool,
                path: Some(path),
                version: None,
            });
        }
    }

    detected
}

/// Find a command in the system PATH.
///
/// # Arguments
///
/// * `command` — The command name to find.
///
/// # Returns
///
/// The full path to the command, or `None` if not found.
fn find_in_path(command: &str) -> Option<String> {
    // Use `which` on Unix, `where` on Windows.
    #[cfg(windows)]
    let output = std::process::Command::new("where")
        .arg(command)
        .output()
        .ok()?;

    #[cfg(not(windows))]
    let output = std::process::Command::new("which")
        .arg(command)
        .output()
        .ok()?;

    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout);
        let first_line = path.lines().next()?.trim().to_string();
        if !first_line.is_empty() {
            return Some(first_line);
        }
    }

    None
}

/// Check if a specific tool is installed.
///
/// # Arguments
///
/// * `tool` — The tool to check.
///
/// # Returns
///
/// `true` if the tool's CLI command is found in PATH.
pub fn is_tool_installed(tool: CodeIndexingTool) -> bool {
    if let Some(cmd) = cli_command_for_tool(tool) {
        find_in_path(cmd).is_some()
    } else {
        false
    }
}

/// Get a description of what a tool does.
///
/// # Arguments
///
/// * `tool` — The tool.
///
/// # Returns
///
/// A human-readable description.
#[must_use]
pub fn tool_description(tool: CodeIndexingTool) -> &'static str {
    match tool {
        CodeIndexingTool::Sourcegraph => "Code search and navigation across repositories",
        CodeIndexingTool::Cody => "AI coding assistant by Sourcegraph",
        CodeIndexingTool::Aider => "AI pair programming in the terminal",
        CodeIndexingTool::Cursor => "AI-powered code editor",
        CodeIndexingTool::Copilot => "GitHub's AI coding assistant",
        CodeIndexingTool::Ctags => "Universal code tag generator for symbol navigation",
        CodeIndexingTool::Ripgrep => "Fast recursive regex search tool",
        CodeIndexingTool::Clangd => "C/C++ language server with code indexing",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- CodeIndexingTool ---

    #[test]
    fn tool_names() {
        assert_eq!(CodeIndexingTool::Sourcegraph.name(), "Sourcegraph");
        assert_eq!(CodeIndexingTool::Cody.name(), "Cody");
        assert_eq!(CodeIndexingTool::Aider.name(), "Aider");
        assert_eq!(CodeIndexingTool::Cursor.name(), "Cursor");
        assert_eq!(CodeIndexingTool::Copilot.name(), "Copilot");
        assert_eq!(CodeIndexingTool::Ctags.name(), "ctags");
        assert_eq!(CodeIndexingTool::Ripgrep.name(), "ripgrep");
        assert_eq!(CodeIndexingTool::Clangd.name(), "clangd");
    }

    #[test]
    fn tool_display() {
        assert_eq!(CodeIndexingTool::Sourcegraph.to_string(), "Sourcegraph");
        assert_eq!(CodeIndexingTool::Ripgrep.to_string(), "ripgrep");
    }

    #[test]
    fn tool_all_values() {
        assert_eq!(CodeIndexingTool::all_values().len(), 8);
    }

    #[test]
    fn tool_serialization_roundtrip() {
        let tool = CodeIndexingTool::Aider;
        let json = serde_json::to_string(&tool).expect("serialize");
        let deserialized: CodeIndexingTool = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(tool, deserialized);
    }

    // --- cli_command_for_tool ---

    #[test]
    fn cli_command_sourcegraph() {
        assert_eq!(
            cli_command_for_tool(CodeIndexingTool::Sourcegraph),
            Some("src")
        );
    }

    #[test]
    fn cli_command_cody() {
        assert_eq!(cli_command_for_tool(CodeIndexingTool::Cody), Some("cody"));
    }

    #[test]
    fn cli_command_aider() {
        assert_eq!(cli_command_for_tool(CodeIndexingTool::Aider), Some("aider"));
    }

    #[test]
    fn cli_command_cursor() {
        assert_eq!(cli_command_for_tool(CodeIndexingTool::Cursor), None);
    }

    #[test]
    fn cli_command_copilot() {
        assert_eq!(
            cli_command_for_tool(CodeIndexingTool::Copilot),
            Some("github-copilot-cli")
        );
    }

    #[test]
    fn cli_command_ctags() {
        assert_eq!(cli_command_for_tool(CodeIndexingTool::Ctags), Some("ctags"));
    }

    #[test]
    fn cli_command_ripgrep() {
        assert_eq!(cli_command_for_tool(CodeIndexingTool::Ripgrep), Some("rg"));
    }

    #[test]
    fn cli_command_clangd() {
        assert_eq!(
            cli_command_for_tool(CodeIndexingTool::Clangd),
            Some("clangd")
        );
    }

    // --- tool_description ---

    #[test]
    fn tool_descriptions() {
        for tool in CodeIndexingTool::all_values() {
            let desc = tool_description(*tool);
            assert!(!desc.is_empty(), "Tool {:?} has empty description", tool);
        }
    }

    // --- DetectedTool ---

    #[test]
    fn detected_tool_fields() {
        let detected = DetectedTool {
            tool: CodeIndexingTool::Ripgrep,
            path: Some("/usr/bin/rg".to_string()),
            version: Some("14.0.0".to_string()),
        };
        assert_eq!(detected.tool, CodeIndexingTool::Ripgrep);
        assert_eq!(detected.path.as_ref().expect("path"), "/usr/bin/rg");
        assert_eq!(detected.version.as_ref().expect("version"), "14.0.0");
    }

    // --- detect_indexing_tools ---

    #[test]
    fn detect_tools_runs() {
        // This test just verifies the function doesn't panic.
        let tools = detect_indexing_tools();
        // On CI, there may be no tools installed.
        assert!(tools.len() <= CodeIndexingTool::all_values().len());
    }

    // --- is_tool_installed ---

    #[test]
    fn is_tool_installed_cursor() {
        // Cursor is GUI-only, so it should never be "installed" via CLI.
        assert!(!is_tool_installed(CodeIndexingTool::Cursor));
    }
}
