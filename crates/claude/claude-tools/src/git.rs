//! Git-related tools: suggest_pr, enter/exit_worktree, list_worktrees.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use claude_config::{ActiveWorktreeSession, RuntimeConfig};
use claude_core::ToolResult;
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::ToolExecutionContext;

const MAX_WORKTREE_SLUG_LENGTH: usize = 64;

#[derive(Debug, Clone, Serialize)]
struct WorktreeInfo {
    path: String,
    branch: String,
    is_main: bool,
}

#[derive(Debug, Clone, Serialize)]
struct WorktreeChangeSummary {
    changed_files: usize,
    commits: usize,
}

pub fn suggest_pr_tool(context: &ToolExecutionContext) -> Result<String> {
    let diff_output = std::process::Command::new("git")
        .args(["diff", "--stat"])
        .current_dir(&context.cwd)
        .output();

    let diff_stat = match diff_output {
        Ok(out) => String::from_utf8_lossy(&out.stdout).to_string(),
        Err(_) => "Unable to run git diff.".to_owned(),
    };

    let log_output = std::process::Command::new("git")
        .args(["log", "--oneline", "-10"])
        .current_dir(&context.cwd)
        .output();

    let recent_commits = match log_output {
        Ok(out) => String::from_utf8_lossy(&out.stdout).to_string(),
        Err(_) => "Unable to run git log.".to_owned(),
    };

    let title_suggestion = recent_commits
        .lines()
        .next()
        .unwrap_or("Changes from current branch")
        .trim_start_matches(|c: char| c.is_ascii_hexdigit() || c == ' ');

    Ok(json!({
        "suggested_title": title_suggestion,
        "diff_stat": diff_stat.trim(),
        "recent_commits": recent_commits.trim(),
        "note": "Review the diff and commits above to craft a PR description."
    })
    .to_string())
}

pub fn enter_worktree_tool(input: &Value, context: &ToolExecutionContext) -> Result<String> {
    if context.active_worktree_session.is_some() {
        return Err(anyhow!("Already in a worktree session"));
    }

    let requested_name = input
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(name) = requested_name {
        validate_worktree_slug(name)?;
    }

    let canonical_root = find_canonical_git_root(&context.cwd).with_context(|| {
        format!(
            "enter_worktree requires a git repository rooted above {}",
            context.cwd.display()
        )
    })?;
    let slug = requested_name
        .map(ToOwned::to_owned)
        .unwrap_or_else(generate_default_worktree_slug);

    let worktrees_dir = canonical_root.join(".claude").join("worktrees");
    fs::create_dir_all(&worktrees_dir)
        .with_context(|| format!("failed to create {}", worktrees_dir.display()))?;

    let flattened_slug = flatten_slug(&slug);
    let worktree_path = worktrees_dir.join(&flattened_slug);
    let worktree_branch = format!("worktree-{flattened_slug}");
    let current_branch = git_stdout(
        &canonical_root,
        ["branch", "--show-current"],
        "resolve current branch",
    )
    .ok()
    .filter(|value| !value.is_empty());
    let _head_commit = git_stdout(&canonical_root, ["rev-parse", "HEAD"], "resolve HEAD")
        .ok()
        .filter(|value| !value.is_empty());

    if !worktree_path.exists() {
        let base_ref = current_branch.as_deref().unwrap_or("HEAD");
        let status = std::process::Command::new("git")
            .args([
                "worktree",
                "add",
                "-B",
                &worktree_branch,
                &worktree_path.to_string_lossy(),
                base_ref,
            ])
            .current_dir(&canonical_root)
            .status()
            .with_context(|| {
                format!(
                    "failed to spawn git worktree add in {}",
                    canonical_root.display()
                )
            })?;
        if !status.success() {
            return Err(anyhow!(
                "git worktree add failed for {}",
                worktree_path.display()
            ));
        }
    }

    let message = format!(
        "Created worktree at {} on branch {}. The session is now working in the worktree. Use exit_worktree to leave mid-session.",
        worktree_path.display(),
        worktree_branch
    );

    Ok(json!({
        "worktreePath": worktree_path.display().to_string(),
        "worktreeBranch": worktree_branch,
        "message": message,
    })
    .to_string())
}

pub fn exit_worktree_tool(input: &Value, context: &ToolExecutionContext) -> Result<String> {
    let Some(session) = context.active_worktree_session.clone() else {
        return Ok(json!({
            "action": input.get("action").and_then(Value::as_str).unwrap_or("keep"),
            "originalCwd": context.original_cwd.display().to_string(),
            "worktreePath": context.cwd.display().to_string(),
            "message": "No-op: there is no active EnterWorktree session to exit. This tool only operates on worktrees created by enter_worktree in the current session. No filesystem changes were made.",
        })
        .to_string());
    };

    let action = input
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("action is required"))?;
    let discard_changes = input
        .get("discard_changes")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !matches!(action, "keep" | "remove") {
        return Err(anyhow!("action must be one of: keep, remove"));
    }

    let summary = count_worktree_changes(&session)?;
    if action == "remove" && !discard_changes && (summary.changed_files > 0 || summary.commits > 0)
    {
        let mut parts = Vec::new();
        if summary.changed_files > 0 {
            parts.push(format!(
                "{} uncommitted {}",
                summary.changed_files,
                if summary.changed_files == 1 {
                    "file"
                } else {
                    "files"
                }
            ));
        }
        if summary.commits > 0 {
            parts.push(format!(
                "{} {} on {}",
                summary.commits,
                if summary.commits == 1 {
                    "commit"
                } else {
                    "commits"
                },
                session
                    .worktree_branch
                    .as_deref()
                    .unwrap_or("the worktree branch")
            ));
        }
        return Err(anyhow!(
            "Worktree has {}. Removing will discard this work permanently. Re-invoke with discard_changes=true to proceed, or use action=\"keep\".",
            parts.join(" and ")
        ));
    }

    match action {
        "keep" => Ok(json!({
            "action": "keep",
            "originalCwd": session.original_cwd.display().to_string(),
            "worktreePath": session.worktree_path.display().to_string(),
            "worktreeBranch": session.worktree_branch,
            "tmuxSessionName": session.tmux_session_name,
            "message": format!(
                "Exited worktree. Your work is preserved at {}{}. Session is now back in {}.",
                session.worktree_path.display(),
                session.worktree_branch.as_deref().map(|branch| format!(" on branch {branch}")).unwrap_or_default(),
                session.original_cwd.display()
            ),
        })
        .to_string()),
        "remove" => {
            remove_git_worktree(&session)?;
            let discard_note = if summary.changed_files > 0 || summary.commits > 0 {
                format!(
                    " Discarded {}{}{}.",
                    if summary.commits > 0 {
                        format!(
                            "{} {}",
                            summary.commits,
                            if summary.commits == 1 { "commit" } else { "commits" }
                        )
                    } else {
                        String::new()
                    },
                    if summary.commits > 0 && summary.changed_files > 0 {
                        " and "
                    } else {
                        ""
                    },
                    if summary.changed_files > 0 {
                        format!(
                            "{} uncommitted {}",
                            summary.changed_files,
                            if summary.changed_files == 1 { "file" } else { "files" }
                        )
                    } else {
                        String::new()
                    }
                )
            } else {
                String::new()
            };
            Ok(json!({
                "action": "remove",
                "originalCwd": session.original_cwd.display().to_string(),
                "worktreePath": session.worktree_path.display().to_string(),
                "worktreeBranch": session.worktree_branch,
                "discardedFiles": summary.changed_files,
                "discardedCommits": summary.commits,
                "message": format!(
                    "Exited and removed worktree at {}.{} Session is now back in {}.",
                    session.worktree_path.display(),
                    discard_note,
                    session.original_cwd.display()
                ),
            })
            .to_string())
        }
        _ => unreachable!(),
    }
}

pub fn list_worktrees_tool(context: &ToolExecutionContext) -> Result<String> {
    let repo_root = find_git_root(&context.cwd).unwrap_or_else(|| context.cwd.clone());
    let output = std::process::Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(&repo_root)
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let worktrees = parse_worktree_list(&stdout);
            Ok(json!({
                "worktrees": worktrees,
                "count": worktrees.len(),
            })
            .to_string())
        }
        Ok(_) => Ok(json!({
            "worktrees": [],
            "note": "Not in a git repository or git worktree not supported."
        })
        .to_string()),
        Err(_) => Ok(json!({
            "worktrees": [],
            "note": "git is not available."
        })
        .to_string()),
    }
}

pub fn sync_tool_context_from_runtime(
    config: &RuntimeConfig,
    tool_context: &mut ToolExecutionContext,
) {
    tool_context.cwd = config.cwd.clone();
    tool_context.original_cwd = config.original_cwd.clone();
    tool_context.active_worktree_session = config.active_worktree_session.clone();
}

pub fn apply_worktree_tool_result_to_runtime(
    tool_name: &str,
    tool_input: &Value,
    tool_result: &ToolResult,
    config: &mut RuntimeConfig,
    tool_context: &mut ToolExecutionContext,
) -> Result<bool> {
    if tool_result.is_error {
        return Ok(false);
    }

    match tool_name {
        "enter_worktree" => {
            let parsed: Value = serde_json::from_str(&tool_result.content)
                .with_context(|| format!("failed to parse {tool_name} tool result"))?;
            let worktree_path = parsed
                .get("worktreePath")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("enter_worktree result missing worktreePath"))?;
            let worktree_branch = parsed
                .get("worktreeBranch")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            let original_cwd = config.cwd.clone();
            let original_branch = git_stdout(
                &original_cwd,
                ["branch", "--show-current"],
                "resolve current branch",
            )
            .ok()
            .filter(|value| !value.is_empty());
            let original_head_commit =
                git_stdout(&original_cwd, ["rev-parse", "HEAD"], "resolve HEAD")
                    .ok()
                    .filter(|value| !value.is_empty());
            let worktree_path = PathBuf::from(worktree_path);
            let worktree_name = tool_input
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .or_else(|| {
                    worktree_path
                        .file_name()
                        .and_then(|value| value.to_str())
                        .map(ToOwned::to_owned)
                })
                .unwrap_or_else(generate_default_worktree_slug);

            config.active_worktree_session = Some(ActiveWorktreeSession {
                original_cwd: original_cwd.clone(),
                worktree_path: worktree_path.clone(),
                worktree_name,
                worktree_branch,
                original_branch,
                original_head_commit,
                session_id: config.session_id,
                tmux_session_name: None,
                hook_based: false,
            });
            config.cwd = worktree_path;
            config.original_cwd = original_cwd;
            sync_tool_context_from_runtime(config, tool_context);
            Ok(true)
        }
        "exit_worktree" => {
            let parsed: Value = serde_json::from_str(&tool_result.content)
                .with_context(|| format!("failed to parse {tool_name} tool result"))?;
            let original_cwd = parsed
                .get("originalCwd")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("exit_worktree result missing originalCwd"))?;
            config.cwd = PathBuf::from(original_cwd);
            config.original_cwd = PathBuf::from(original_cwd);
            config.active_worktree_session = None;
            sync_tool_context_from_runtime(config, tool_context);
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn validate_worktree_slug(slug: &str) -> Result<()> {
    if slug.len() > MAX_WORKTREE_SLUG_LENGTH {
        return Err(anyhow!(
            "Invalid worktree name: must be {MAX_WORKTREE_SLUG_LENGTH} characters or fewer (got {})",
            slug.len()
        ));
    }
    for segment in slug.split('/') {
        if segment == "." || segment == ".." {
            return Err(anyhow!(
                "Invalid worktree name \"{slug}\": must not contain \".\" or \"..\" path segments"
            ));
        }
        if segment.is_empty()
            || !segment
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
        {
            return Err(anyhow!(
                "Invalid worktree name \"{slug}\": each \"/\"-separated segment must be non-empty and contain only letters, digits, dots, underscores, and dashes"
            ));
        }
    }
    Ok(())
}

fn flatten_slug(slug: &str) -> String {
    slug.replace('/', "+")
}

fn generate_default_worktree_slug() -> String {
    format!("plan-{}", &Uuid::new_v4().simple().to_string()[..12])
}

fn git_stdout<const N: usize>(cwd: &Path, args: [&str; N], purpose: &str) -> Result<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("failed to spawn git to {purpose}"))?;
    if !output.status.success() {
        return Err(anyhow!(
            "git command failed while trying to {purpose}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut current = start;
    loop {
        if current.join(".git").exists() {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}

fn find_canonical_git_root(start: &Path) -> Option<PathBuf> {
    let resolved = git_stdout(start, ["rev-parse", "--show-toplevel"], "resolve git root").ok()?;
    Some(PathBuf::from(resolved))
}

fn count_worktree_changes(session: &ActiveWorktreeSession) -> Result<WorktreeChangeSummary> {
    let status = git_stdout(
        &session.worktree_path,
        ["status", "--porcelain"],
        "inspect worktree status",
    )?;
    let changed_files = status
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    let commits = if let Some(original_head_commit) = session.original_head_commit.as_deref() {
        git_stdout(
            &session.worktree_path,
            [
                "rev-list",
                "--count",
                &format!("{original_head_commit}..HEAD"),
            ],
            "count worktree commits",
        )?
        .parse::<usize>()
        .unwrap_or(0)
    } else {
        0
    };
    Ok(WorktreeChangeSummary {
        changed_files,
        commits,
    })
}

fn remove_git_worktree(session: &ActiveWorktreeSession) -> Result<()> {
    let canonical_root = find_canonical_git_root(&session.original_cwd)
        .or_else(|| find_canonical_git_root(&session.worktree_path))
        .ok_or_else(|| anyhow!("failed to resolve canonical git root for worktree removal"))?;

    let remove_status = std::process::Command::new("git")
        .args([
            "worktree",
            "remove",
            "--force",
            &session.worktree_path.to_string_lossy(),
        ])
        .current_dir(&canonical_root)
        .status()
        .with_context(|| "failed to spawn git worktree remove")?;
    if !remove_status.success() {
        return Err(anyhow!(
            "git worktree remove failed for {}",
            session.worktree_path.display()
        ));
    }

    if let Some(branch) = session.worktree_branch.as_deref() {
        let _ = std::process::Command::new("git")
            .args(["branch", "-D", branch])
            .current_dir(&canonical_root)
            .status();
    }
    Ok(())
}

fn parse_worktree_list(text: &str) -> Vec<WorktreeInfo> {
    let mut worktrees = Vec::new();
    let mut current_path = String::new();
    let mut current_branch = String::new();
    // The first worktree in `git worktree list --porcelain` output is the
    // main/principal worktree. We track this with an index counter.
    let mut worktree_index: usize = 0;

    for line in text.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            if !current_path.is_empty() {
                worktrees.push(WorktreeInfo {
                    path: current_path.clone(),
                    branch: current_branch.clone(),
                    is_main: worktree_index == 0,
                });
            }
            current_path = path.to_owned();
            current_branch.clear();
            worktree_index += 1;
        } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
            current_branch = branch.to_owned();
        }
        // NOTE: "bare" is intentionally ignored here. In `git worktree list
        // --porcelain`, "bare" indicates a bare repository (no working tree),
        // NOT the main worktree. Previously this was incorrectly treated as
        // is_main = true, which would mark bare repos as the main worktree.
        // The main worktree is identified by being the first entry (index 0).
    }

    if !current_path.is_empty() {
        worktrees.push(WorktreeInfo {
            path: current_path,
            branch: current_branch,
            is_main: worktree_index == 0,
        });
    }
    worktrees
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use claude_config::settings_layers::RuntimeOverrides;
    use claude_config::{ProviderOverrides, load_runtime_config};
    use claude_core::task_stack::TaskStack;
    use claude_core::{InputFormat, OutputFormat, PermissionMode, ProviderProtocol, ToolResult};
    use serde_json::json;
    use tempfile::{TempDir, tempdir};

    use super::*;

    fn test_context() -> ToolExecutionContext {
        ToolExecutionContext {
            cwd: PathBuf::from("C:\\repo"),
            original_cwd: PathBuf::from("C:\\repo"),
            active_worktree_session: None,
            timeout_ms: 30_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Arc::new(parking_lot::Mutex::new(TaskStack::default())),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    fn test_runtime_config() -> (TempDir, RuntimeConfig) {
        let tempdir = tempdir().expect("tempdir");
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        std::fs::create_dir_all(&cwd).expect("cwd");
        std::fs::create_dir_all(&profile).expect("profile");
        let config = load_runtime_config(
            Some(cwd),
            Some(profile),
            None,
            PermissionMode::Default,
            InputFormat::Text,
            OutputFormat::Text,
            false,
            false,
            false,
            false,
            4,
            ProviderOverrides {
                provider: Some("mock".to_owned()),
                base_url: Some("mock://provider".to_owned()),
                api_key: Some("mock".to_owned()),
                model: Some("mock-model".to_owned()),
                protocol: Some(ProviderProtocol::Anthropic),
            },
            RuntimeOverrides::default(),
        )
        .expect("config");
        (tempdir, config)
    }

    #[test]
    fn validate_slug_rejects_parent_segments() {
        let error = validate_worktree_slug("../evil").expect_err("must fail");
        assert!(error.to_string().contains("path segments"));
    }

    #[test]
    fn validate_slug_accepts_nested_valid_segments() {
        validate_worktree_slug("user/feature-1").expect("valid slug");
    }

    #[test]
    fn exit_worktree_without_session_is_noop_payload() {
        let payload =
            exit_worktree_tool(&json!({"action":"keep"}), &test_context()).expect("payload");
        let parsed: Value = serde_json::from_str(&payload).expect("json");
        assert!(
            parsed["message"]
                .as_str()
                .expect("message")
                .contains("No-op")
        );
    }

    #[test]
    fn parse_worktree_list_handles_multiple_blocks() {
        let input = "worktree /repo\nbranch refs/heads/main\n\nworktree /repo/.claude/worktrees/w1\nbranch refs/heads/worktree-w1\n";
        let parsed = parse_worktree_list(input);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[1].branch, "worktree-w1");
    }

    #[test]
    fn apply_worktree_tool_result_ignores_non_worktree_text_output() {
        let (_tempdir, mut config) = test_runtime_config();
        let original_cwd = config.cwd.clone();
        let mut tool_context = ToolExecutionContext::from_runtime_config(&config);

        let applied = apply_worktree_tool_result_to_runtime(
            "bash_command",
            &json!({"command": "cat Cargo.toml"}),
            &ToolResult {
                content: "remote-code-rust".to_owned(),
                is_error: false,
                content_blocks: Vec::new(),
                follow_up_user_blocks: Vec::new(),
            },
            &mut config,
            &mut tool_context,
        )
        .expect("non-worktree tool output should be ignored");

        assert!(!applied);
        assert_eq!(config.cwd, original_cwd);
        assert_eq!(tool_context.cwd, original_cwd);
    }
}
