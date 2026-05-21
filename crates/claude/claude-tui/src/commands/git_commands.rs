//! Git and code commands: `/commit`, `/diff`, `/pr_comments`, `/branch`, `/autofix-pr`.

use claude_config::RuntimeConfig;
use claude_tools::{ToolExecutionContext, git};
use serde_json::Value;

/// Dispatch `/commit` — one-click commit (git add + commit with generated message).
pub fn render_commit(config: &RuntimeConfig) {
    let context = ToolExecutionContext::from_runtime_config(config);

    match git::suggest_pr_tool(&context)
        .and_then(|payload| serde_json::from_str::<Value>(&payload).map_err(Into::into))
    {
        Ok(value) => {
            let diff_stat = value["diff_stat"].as_str().unwrap_or_default();
            if diff_stat.is_empty() {
                println!("Working tree is clean — nothing to commit.");
                return;
            }
            println!("Commit preview:");
            println!("  diff stat:");
            for line in diff_stat.lines().take(20) {
                println!("    {line}");
            }
            let suggested = value["suggested_title"].as_str().unwrap_or("(auto)");
            println!("  suggested message: {suggested}");
            println!("  (use the Bash tool to run: git add -A && git commit -m \"...\")");
        }
        Err(error) => eprintln!("Failed to generate commit preview: {error}"),
    }
}

/// Dispatch `/diff` — view code changes.
pub fn render_diff(config: &RuntimeConfig) {
    let context = ToolExecutionContext::from_runtime_config(config);

    match git::suggest_pr_tool(&context)
        .and_then(|payload| serde_json::from_str::<Value>(&payload).map_err(Into::into))
    {
        Ok(value) => {
            let diff_stat = value["diff_stat"].as_str().unwrap_or_default();
            if diff_stat.is_empty() {
                println!("No changes detected.");
            } else {
                println!("Diff stat:");
                for line in diff_stat.lines() {
                    println!("  {line}");
                }
            }
            let recent = value["recent_commits"].as_str().unwrap_or_default();
            if !recent.is_empty() {
                println!("Recent commits:");
                for line in recent.lines().take(10) {
                    println!("  {line}");
                }
            }
        }
        Err(error) => eprintln!("Failed to load diff: {error}"),
    }
}

/// Dispatch `/pr_comments` — view PR comments.
pub fn render_pr_comments(config: &RuntimeConfig) {
    let context = ToolExecutionContext::from_runtime_config(config);

    match git::suggest_pr_tool(&context)
        .and_then(|payload| serde_json::from_str::<Value>(&payload).map_err(Into::into))
    {
        Ok(value) => {
            println!("PR comments surface:");
            let suggested = value["suggested_title"].as_str().unwrap_or("(none)");
            println!("  suggested title: {suggested}");
            let note = value["note"].as_str().unwrap_or_default();
            if !note.is_empty() {
                println!("  note: {note}");
            }
            println!("  (use the Bash tool to run: gh pr view --comments)");
        }
        Err(error) => eprintln!("Failed to load PR comments: {error}"),
    }
}

/// Dispatch `/branch` — branch management.
pub fn dispatch_branch(input: &str, config: &RuntimeConfig) {
    let remainder = input
        .trim()
        .strip_prefix("/branch")
        .unwrap_or_default()
        .trim();

    if remainder.is_empty() || remainder == "list" {
        render_branch_list(config);
        return;
    }

    let mut parts = remainder.split_whitespace();
    match parts.next().unwrap_or_default() {
        "create" => {
            let name = parts.next().unwrap_or_default();
            if name.is_empty() {
                println!("Usage: /branch create <branch-name>");
            } else {
                println!("Branch '{name}' — use the Bash tool to run: git checkout -b {name}");
            }
        }
        "switch" => {
            let name = parts.next().unwrap_or_default();
            if name.is_empty() {
                println!("Usage: /branch switch <branch-name>");
            } else {
                println!("Switch to '{name}' — use the Bash tool to run: git checkout {name}");
            }
        }
        other => {
            println!("Unknown /branch subcommand '{other}'.");
            println!("Usage: /branch [list|create <name>|switch <name>]");
        }
    }
}

fn render_branch_list(config: &RuntimeConfig) {
    let context = ToolExecutionContext::from_runtime_config(config);

    match git::list_worktrees_tool(&context)
        .and_then(|payload| serde_json::from_str::<Value>(&payload).map_err(Into::into))
    {
        Ok(value) => {
            let worktrees = value["worktrees"].as_array().cloned().unwrap_or_default();
            println!("Branches ({} worktrees):", worktrees.len());
            for wt in &worktrees {
                println!(
                    "  {}  {}",
                    wt["branch"].as_str().unwrap_or("(detached)"),
                    wt["path"].as_str().unwrap_or("(unknown)")
                );
            }
        }
        Err(error) => eprintln!("Failed to list branches: {error}"),
    }
}

/// Dispatch `/autofix-pr` — auto-fix PR issues.
pub fn render_autofix_pr(config: &RuntimeConfig) {
    let context = ToolExecutionContext::from_runtime_config(config);

    match git::suggest_pr_tool(&context)
        .and_then(|payload| serde_json::from_str::<Value>(&payload).map_err(Into::into))
    {
        Ok(value) => {
            println!("AutoFix PR surface:");
            let suggested = value["suggested_title"].as_str().unwrap_or("(none)");
            println!("  PR title: {suggested}");
            let diff_stat = value["diff_stat"].as_str().unwrap_or_default();
            if !diff_stat.is_empty() {
                println!("  Changes detected:");
                for line in diff_stat.lines().take(10) {
                    println!("    {line}");
                }
            }
            println!("  (ask the agent to fix specific PR review comments)");
        }
        Err(error) => eprintln!("Failed to load autofix surface: {error}"),
    }
}

/// Helper used by other modules to build a tool execution context.
#[allow(dead_code)]
pub fn tool_context(config: &RuntimeConfig) -> ToolExecutionContext {
    ToolExecutionContext::from_runtime_config(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use claude_config::{ProviderOverrides, RuntimeOverrides, load_runtime_config};
    use claude_core::{InputFormat, OutputFormat, PermissionMode};
    use tempfile::tempdir;

    fn build_test_config() -> RuntimeConfig {
        let temp = tempdir().expect("tempdir should work");
        let root = temp.keep();
        load_runtime_config(
            Some(root.clone()),
            Some(root.join(".remote-code-rust")),
            None,
            PermissionMode::Default,
            InputFormat::Text,
            OutputFormat::Text,
            false,
            false,
            false,
            false,
            8,
            ProviderOverrides {
                provider: Some("glm-coding".to_owned()),
                base_url: Some("https://open.bigmodel.cn/api/anthropic".to_owned()),
                api_key: Some("secret".to_owned()),
                model: Some("glm-5.1".to_owned()),
                protocol: Some(claude_core::ProviderProtocol::Anthropic),
            },
            RuntimeOverrides::default(),
        )
        .expect("config should load")
    }

    #[test]
    fn commit_shows_preview() {
        let config = build_test_config();
        render_commit(&config);
    }

    #[test]
    fn diff_shows_changes() {
        let config = build_test_config();
        render_diff(&config);
    }

    #[test]
    fn pr_comments_shows_surface() {
        let config = build_test_config();
        render_pr_comments(&config);
    }

    #[test]
    fn branch_list_shows_worktrees() {
        let config = build_test_config();
        dispatch_branch("/branch list", &config);
    }

    #[test]
    fn branch_default_lists() {
        let config = build_test_config();
        dispatch_branch("/branch", &config);
    }

    #[test]
    fn branch_create_requires_name() {
        let config = build_test_config();
        dispatch_branch("/branch create", &config);
    }

    #[test]
    fn branch_create_with_name() {
        let config = build_test_config();
        dispatch_branch("/branch create feature-x", &config);
    }

    #[test]
    fn branch_switch_requires_name() {
        let config = build_test_config();
        dispatch_branch("/branch switch", &config);
    }

    #[test]
    fn branch_switch_with_name() {
        let config = build_test_config();
        dispatch_branch("/branch switch main", &config);
    }

    #[test]
    fn branch_unknown_subcommand() {
        let config = build_test_config();
        dispatch_branch("/branch foo", &config);
    }

    #[test]
    fn autofix_pr_shows_surface() {
        let config = build_test_config();
        render_autofix_pr(&config);
    }

    #[test]
    fn tool_context_returns_valid_context() {
        let config = build_test_config();
        let ctx = tool_context(&config);
        assert_eq!(ctx.cwd, config.cwd);
    }
}
