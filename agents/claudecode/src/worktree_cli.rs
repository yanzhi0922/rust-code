use anyhow::Result;
use claude_config::RuntimeConfig;
use claude_tools::{ToolExecutionContext, git};
use serde_json::{Value, json};

use crate::cli::{WorktreeAddArgs, WorktreeCommand, WorktreeListArgs, WorktreeRemoveArgs};

pub(crate) fn run_worktree(config: &RuntimeConfig, command: WorktreeCommand) -> Result<()> {
    match command {
        WorktreeCommand::List(args) => run_worktree_list(config, args),
        WorktreeCommand::Add(args) => run_worktree_add(config, args),
        WorktreeCommand::Remove(args) => run_worktree_remove(config, args),
    }
}

fn run_worktree_list(config: &RuntimeConfig, args: WorktreeListArgs) -> Result<()> {
    let output = build_worktree_list_output(config)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    let worktrees = output["worktrees"].as_array().cloned().unwrap_or_default();
    if worktrees.is_empty() {
        println!(
            "No worktrees found. {}",
            output["note"].as_str().unwrap_or_default()
        );
        return Ok(());
    }

    println!("Worktrees:");
    for worktree in worktrees {
        println!(
            "  {}  {}",
            worktree["branch"].as_str().unwrap_or("(detached)"),
            worktree["path"].as_str().unwrap_or("(missing path)")
        );
    }
    Ok(())
}

fn run_worktree_add(config: &RuntimeConfig, args: WorktreeAddArgs) -> Result<()> {
    let output = build_worktree_action_output(
        config,
        &json!({
            "name": args.name,
        }),
        git::enter_worktree_tool,
    )?;
    print_worktree_action_output(&output, args.json, "create worktree")
}

fn run_worktree_remove(config: &RuntimeConfig, args: WorktreeRemoveArgs) -> Result<()> {
    let output = build_worktree_action_output(
        config,
        &json!({
            "action": args.action,
            "discard_changes": args.discard_changes,
        }),
        git::exit_worktree_tool,
    )?;
    print_worktree_action_output(&output, args.json, "remove worktree")
}

fn print_worktree_action_output(output: &Value, json_mode: bool, label: &str) -> Result<()> {
    if json_mode {
        println!("{}", serde_json::to_string_pretty(output)?);
        return Ok(());
    }

    println!("Worktree {label}:");
    for key in [
        "action",
        "worktreeBranch",
        "worktreePath",
        "originalCwd",
        "discardedFiles",
        "discardedCommits",
        "message",
        "error",
    ] {
        if let Some(rendered) = render_scalar(output, key) {
            println!("  {key}: {rendered}");
        }
    }
    Ok(())
}

fn build_worktree_list_output(config: &RuntimeConfig) -> Result<Value> {
    let context = tool_context(config);
    let payload = git::list_worktrees_tool(&context)?;
    Ok(serde_json::from_str(&payload)?)
}

fn build_worktree_action_output(
    config: &RuntimeConfig,
    input: &Value,
    tool: fn(&Value, &ToolExecutionContext) -> Result<String>,
) -> Result<Value> {
    let context = tool_context(config);
    let payload = tool(input, &context)?;
    Ok(serde_json::from_str(&payload)?)
}

fn tool_context(config: &RuntimeConfig) -> ToolExecutionContext {
    ToolExecutionContext::from_runtime_config(config)
}

fn render_scalar(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(|entry| match entry {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        other => Some(other.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use claude_config::{ProviderOverrides, RuntimeOverrides, load_runtime_config};
    use tempfile::tempdir;

    use super::build_worktree_list_output;

    #[test]
    fn worktree_list_output_has_expected_shape() {
        let tempdir = tempdir().expect("tempdir");
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        fs::create_dir_all(&cwd).expect("cwd");
        fs::create_dir_all(&profile).expect("profile");
        let config = load_runtime_config(
            Some(cwd),
            Some(profile),
            None,
            claude_core::PermissionMode::Default,
            claude_core::InputFormat::Text,
            claude_core::OutputFormat::Text,
            false,
            false,
            false,
            false,
            4,
            ProviderOverrides::default(),
            RuntimeOverrides::default(),
        )
        .expect("config");

        let output = build_worktree_list_output(&config).expect("worktree list");
        assert!(output.get("worktrees").is_some());
    }
}
