use claude_config::RuntimeConfig;
use claude_tools::{ToolExecutionContext, git};
use serde_json::{Value, json};

pub fn dispatch(input: &str, config: &RuntimeConfig) {
    let context = ToolExecutionContext {
        cwd: config.cwd.clone(),
        original_cwd: config.original_cwd.clone(),
        active_worktree_session: config.active_worktree_session.clone(),
        timeout_ms: config.provider.timeout_ms,
        ..ToolExecutionContext::default()
    };
    let remainder = input
        .trim()
        .strip_prefix("/worktree")
        .unwrap_or_default()
        .trim();
    if remainder.is_empty() || remainder == "list" {
        render_list(&context);
        return;
    }

    let mut parts = remainder.split_whitespace();
    let action = parts.next().unwrap_or_default();

    match action {
        "add" => {
            let name = parts.next();
            render_action(
                git::enter_worktree_tool(
                    &json!({
                        "name": name,
                    }),
                    &context,
                ),
                "create worktree",
            );
        }
        "remove" => {
            let remove_action = parts.next().unwrap_or_default();
            if remove_action.is_empty() {
                print_usage();
                return;
            }
            let discard_changes = parts.any(|part| part == "--discard-changes");
            render_action(
                git::exit_worktree_tool(
                    &json!({
                        "action": remove_action,
                        "discard_changes": discard_changes,
                    }),
                    &context,
                ),
                "remove worktree",
            );
        }
        _ => print_usage(),
    }
}

fn render_list(context: &ToolExecutionContext) {
    match git::list_worktrees_tool(context)
        .and_then(|payload| serde_json::from_str::<Value>(&payload).map_err(Into::into))
    {
        Ok(value) => {
            let worktrees = value["worktrees"].as_array().cloned().unwrap_or_default();
            if worktrees.is_empty() {
                println!(
                    "No worktrees found. {}",
                    value["note"].as_str().unwrap_or_default()
                );
                return;
            }

            println!("Worktrees:");
            for worktree in worktrees {
                println!(
                    "  {}  {}",
                    worktree["branch"].as_str().unwrap_or("(detached)"),
                    worktree["path"].as_str().unwrap_or("(missing path)")
                );
            }
        }
        Err(error) => eprintln!("Failed to list worktrees: {error}"),
    }
}

fn render_action(result: anyhow::Result<String>, label: &str) {
    match result.and_then(|payload| serde_json::from_str::<Value>(&payload).map_err(Into::into)) {
        Ok(value) => {
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
                if let Some(rendered) = render_scalar(&value, key) {
                    println!("  {key}: {rendered}");
                }
            }
        }
        Err(error) => eprintln!("Failed to {label}: {error}"),
    }
}

fn render_scalar(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(|entry| match entry {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        other => Some(other.to_string()),
    })
}

fn print_usage() {
    println!("Usage:");
    println!("  /worktree");
    println!("  /worktree list");
    println!("  /worktree add [name]");
    println!("  /worktree remove <keep|remove> [--discard-changes]");
}
