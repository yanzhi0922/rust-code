use std::path::{Path, PathBuf};

pub static SYSTEM_PROMPT_SPEC: &str = "You are Claude, an AI assistant built by Anthropic. You are an interactive CLI tool that helps users with software engineering tasks.";
pub static MAX_SYSTEM_PROMPT_CHARS: usize = 100_000;

pub struct SystemPromptBuilder {
    pub cwd: PathBuf,
    pub claude_md_content: Option<String>,
}

impl SystemPromptBuilder {
    pub fn new(cwd: PathBuf) -> Self {
        let claude_md_content = Self::load_claude_md(&cwd);
        Self {
            cwd,
            claude_md_content,
        }
    }

    fn load_claude_md(cwd: &Path) -> Option<String> {
        let path = cwd.join("CLAUDE.md");
        if path.exists() {
            std::fs::read_to_string(&path).ok()
        } else {
            None
        }
    }

    pub fn build(&self) -> String {
        let mut parts = Vec::new();

        parts.push("You are Claude, an AI assistant built by Anthropic. You are an interactive CLI tool that helps users with software engineering tasks.".to_string());

        let date = chrono::Utc::now().format("%Y-%m-%d %H:%M UTC").to_string();
        parts.push(format!("Current time: {date}"));
        parts.push(format!("Working directory: {}", self.cwd.display()));

        if let Ok(output) = std::process::Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(&self.cwd)
            .output()
        {
            if output.status.success() {
                let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
                parts.push(format!("Git branch: {branch}"));
            }
        }

        if let Some(ref claude_md) = self.claude_md_content {
            parts.push("\n## Project Instructions\n".to_string());
            parts.push(claude_md.clone());
        }

        parts.push("\n## Available Tools\n".to_string());
        parts.push("You have access to tools for reading/writing files, executing commands, searching, and more. Use them to accomplish the user's task.".to_string());

        parts.join("\n\n")
    }
}
