//! Team management tools: team_delete, team_list.
//!
//! `team_delete` follows the research contract:
//! - the input schema is a strict empty object
//! - the active team comes from the current session context
//! - active non-lead teammates block cleanup
//! - cleanup removes worktrees plus team/task directories

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::ToolExecutionContext;
use crate::tasks;
use claude_swarm::constants::{TEAM_FILE_NAME, TEAM_LEAD_NAME};

/// Set a base-directory override (primarily for testing).
pub fn set_base_dir_override(dir: Option<PathBuf>) {
    claude_swarm::team_helpers::set_base_dir_override(dir);
}

/// Delete the current session team and clean up associated resources.
///
/// Mirrors the research implementation by resolving the active team from the
/// current session context instead of accepting an explicit `team_name`.
///
/// # Errors
/// Returns an error only when cleanup itself fails unexpectedly.
pub fn team_delete(_input: &Value, _context: &ToolExecutionContext) -> Result<String> {
    let Some(team_name) = current_session_team_name()? else {
        return Ok(json!({
            "success": true,
            "message": "No team name found, nothing to clean up"
        })
        .to_string());
    };

    let team_dir = resolve_team_dir(&team_name);
    let team = if team_dir.join(TEAM_FILE_NAME).exists() {
        Some(load_team_from_path(&team_name, &team_dir)?)
    } else {
        None
    };

    if let Some(team) = team.as_ref() {
        let active_members = team
            .members
            .iter()
            .filter(|member| member.name != TEAM_LEAD_NAME && member.is_active != Some(false))
            .map(|member| member.name.as_str())
            .collect::<Vec<_>>();
        if !active_members.is_empty() {
            return Ok(json!({
                "success": false,
                "message": format!(
                    "Cannot cleanup team with {} active member(s): {}. Use requestShutdown to gracefully terminate teammates first.",
                    active_members.len(),
                    active_members.join(", ")
                ),
                "team_name": team_name,
            })
            .to_string());
        }
    }

    cleanup_team_resources(team.as_ref(), &team_name)
        .with_context(|| format!("failed to clean up team '{team_name}'"))?;
    tasks::clear_leader_team_name()?;

    Ok(json!({
        "success": true,
        "message": format!("Cleaned up directories and worktrees for team \"{team_name}\""),
        "team_name": team_name,
    })
    .to_string())
}

/// List all multi-agent teams.
///
/// Returns a list of team names and their basic metadata.
///
/// # Errors
/// Returns an error if the teams directory cannot be read.
pub fn team_list(_input: &Value, _context: &ToolExecutionContext) -> Result<String> {
    let teams_dir = resolve_teams_base_dir();

    if !teams_dir.exists() {
        return Ok(json!({
            "teams": [],
            "total": 0,
            "message": "No teams directory found."
        })
        .to_string());
    }

    let mut teams = Vec::new();
    let entries = std::fs::read_dir(&teams_dir)?;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let team_file = path.join(TEAM_FILE_NAME);
            if team_file.exists()
                && let Ok(content) = std::fs::read_to_string(&team_file)
                && let Ok(team_data) = serde_json::from_str::<Value>(&content)
            {
                teams.push(json!({
                    "name": team_data["name"].as_str().unwrap_or("unknown"),
                    "lead": team_data["lead_agent_id"].as_str().unwrap_or("unknown"),
                    "created_at": team_data["created_at"].as_i64().unwrap_or(0),
                    "member_count": team_data["members"]
                        .as_array()
                        .map(|a| a.len())
                        .unwrap_or(0),
                }));
            }
        }
    }

    let total = teams.len();
    Ok(json!({
        "teams": teams,
        "total": total,
        "message": format!("Found {total} team(s).")
    })
    .to_string())
}

fn current_session_team_name() -> Result<Option<String>> {
    if let Some(team_name) = tasks::leader_team_name()? {
        return Ok(Some(team_name));
    }

    if let Ok(value) = std::env::var(claude_swarm::constants::ENV_TEAM_NAME) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Ok(Some(trimmed.to_owned()));
        }
    }

    Ok(None)
}

fn load_team_from_path(team_name: &str, team_dir: &Path) -> Result<claude_swarm::TeamFile> {
    let content = std::fs::read_to_string(team_dir.join(TEAM_FILE_NAME))
        .with_context(|| format!("failed to read team config for '{team_name}'"))?;
    serde_json::from_str::<claude_swarm::TeamFile>(&content)
        .with_context(|| format!("failed to parse team config for '{team_name}'"))
}

fn cleanup_team_resources(team: Option<&claude_swarm::TeamFile>, team_name: &str) -> Result<()> {
    if let Some(team) = team {
        for worktree_path in team
            .members
            .iter()
            .filter_map(|member| member.worktree_path.as_deref())
        {
            destroy_worktree(Path::new(worktree_path))?;
        }
    }

    remove_dir_if_exists(&resolve_team_dir(team_name))
        .with_context(|| format!("failed to remove team directory for '{team_name}'"))?;
    remove_dir_if_exists(&tasks::task_list_dir(team_name))
        .with_context(|| format!("failed to remove task directory for '{team_name}'"))?;
    Ok(())
}

fn destroy_worktree(worktree_path: &Path) -> Result<()> {
    if !worktree_path.exists() {
        return Ok(());
    }

    if let Some(main_repo_path) = discover_main_repo_path(worktree_path) {
        let output = Command::new("git")
            .arg("-C")
            .arg(&main_repo_path)
            .args(["worktree", "remove", "--force"])
            .arg(worktree_path)
            .output();
        if let Ok(output) = output
            && output.status.success()
        {
            return Ok(());
        }
    }

    remove_dir_if_exists(worktree_path)
}

fn discover_main_repo_path(worktree_path: &Path) -> Option<PathBuf> {
    let git_file = worktree_path.join(".git");
    let content = std::fs::read_to_string(git_file).ok()?;
    let gitdir = content.strip_prefix("gitdir:")?.trim();
    let gitdir_path = PathBuf::from(gitdir);
    let resolved = if gitdir_path.is_absolute() {
        gitdir_path
    } else {
        worktree_path.join(gitdir_path)
    };
    resolved.parent()?.parent()?.parent().map(Path::to_path_buf)
}

fn remove_dir_if_exists(path: &Path) -> Result<()> {
    if path.exists() {
        std::fs::remove_dir_all(path)
            .with_context(|| format!("failed to remove {}", path.display()))?;
    }
    Ok(())
}

/// Resolve the base directory for teams data.
fn resolve_teams_base_dir() -> PathBuf {
    claude_swarm::team_helpers::teams_base_dir()
}

/// Resolve the directory for a specific team.
fn resolve_team_dir(team_name: &str) -> PathBuf {
    let sanitized = sanitize_team_name(team_name);
    resolve_teams_base_dir().join(sanitized)
}

/// Sanitize a team name for use as a directory name.
fn sanitize_team_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::TempDir;

    struct ResetBaseDirOverride;

    impl Drop for ResetBaseDirOverride {
        fn drop(&mut self) {
            set_base_dir_override(None);
            tasks::configure_task_list_context(None, None).expect("reset task list context");
            tasks::set_leader_team_name(None).expect("reset leader team name");
        }
    }

    fn with_base_dir_override<T>(dir: PathBuf, f: impl FnOnce() -> T) -> T {
        let _test_guard = tasks::test_guard_for_tests();
        set_base_dir_override(Some(dir));
        let _reset = ResetBaseDirOverride;
        f()
    }

    fn test_context() -> ToolExecutionContext {
        ToolExecutionContext {
            cwd: PathBuf::from("/tmp"),
            original_cwd: PathBuf::from("/tmp"),
            active_worktree_session: None,
            timeout_ms: 30_000,
            sub_agent: None,
            progress_cb: None,
            task_stack: Arc::new(parking_lot::Mutex::new(
                claude_core::task_stack::TaskStack::default(),
            )),
            read_file_state: crate::FileStateCache::new(),
            sub_agent_output_tokens: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    #[test]
    #[serial_test::serial]
    fn sanitize_team_name_handles_special_chars() {
        assert_eq!(sanitize_team_name("my-team"), "my-team");
        assert_eq!(sanitize_team_name("my team"), "my_team");
        assert_eq!(sanitize_team_name("my/team"), "my_team");
        assert_eq!(sanitize_team_name("my.team"), "my_team");
    }

    #[test]
    #[serial_test::serial]
    fn sanitize_team_name_preserves_alphanumeric() {
        assert_eq!(sanitize_team_name("team123"), "team123");
        assert_eq!(sanitize_team_name("My-Team_2"), "My-Team_2");
    }

    #[test]
    #[serial_test::serial]
    fn team_delete_returns_noop_when_session_has_no_team() {
        let result = with_base_dir_override(PathBuf::from("/tmp"), || {
            let input = json!({});
            let context = test_context();
            team_delete(&input, &context)
        })
        .expect("team_delete should succeed");
        let parsed: Value = serde_json::from_str(&result).expect("valid json");
        assert_eq!(parsed["success"], true);
        assert!(
            parsed["message"]
                .as_str()
                .expect("message string")
                .contains("No team name found")
        );
    }

    #[test]
    #[serial_test::serial]
    fn team_delete_cleans_up_team_dir() {
        let temp = TempDir::new().expect("temp dir");
        let team_dir = temp.path().join("test-team");
        let tasks_root = temp.path().join("tasks");
        let task_dir = tasks_root.join("test-team");
        let worktree_dir = temp.path().join("worktree-a");
        std::fs::create_dir_all(&team_dir).expect("create team dir");
        std::fs::create_dir_all(&task_dir).expect("create task dir");
        std::fs::create_dir_all(&worktree_dir).expect("create worktree dir");
        std::fs::write(
            worktree_dir.join(".git"),
            "gitdir: ../repo/.git/worktrees/worktree-a",
        )
        .expect("write worktree git file");
        std::fs::create_dir_all(temp.path().join("repo").join(".git").join("worktrees"))
            .expect("create fake repo admin dir");
        std::fs::write(
            team_dir.join("config.json"),
            serde_json::json!({
                "name": "test-team",
                "lead_agent_id": "lead",
                "created_at": 0,
                "members": [{
                    "agent_id": "a1",
                    "name": "worker1",
                    "joined_at": 0,
                    "pane_id": "p1",
                    "cwd": ".",
                    "is_active": false,
                    "worktree_path": worktree_dir.to_string_lossy(),
                }],
                "hidden_pane_ids": [],
                "team_allowed_paths": [],
            })
            .to_string(),
        )
        .expect("write config.json");

        let result = with_base_dir_override(temp.path().to_path_buf(), || {
            tasks::configure_task_list_context(None, Some(tasks_root))
                .expect("configure tasks dir");
            tasks::set_leader_team_name(Some("test-team".to_owned()))
                .expect("set leader team name");
            let input = json!({});
            let context = test_context();
            team_delete(&input, &context)
        });

        assert!(result.is_ok());
        let output = result.expect("team_delete should succeed");
        let parsed: Value = serde_json::from_str(&output).expect("valid json");
        assert_eq!(parsed["success"], true);
        assert_eq!(parsed["team_name"], "test-team");

        // On Windows, remove_dir_all may return before the directory entry is
        // fully gone from the filesystem. Retry with a generous back-off.
        let wait_for_removal = |path: &Path| {
            for i in 0..50 {
                if !path.exists() {
                    return true;
                }
                // Exponential back-off: 10ms, 20ms, 40ms … up to ~5s total
                std::thread::sleep(std::time::Duration::from_millis(10 << i.min(5)));
            }
            !path.exists()
        };
        assert!(wait_for_removal(&team_dir), "team dir should be removed");
        assert!(wait_for_removal(&task_dir), "task dir should be removed");
        assert!(
            wait_for_removal(&worktree_dir),
            "worktree dir should be removed"
        );
    }

    #[test]
    #[serial_test::serial]
    fn team_delete_fails_with_active_members() {
        let temp = TempDir::new().expect("temp dir");
        let team_dir = temp.path().join("active-team");
        std::fs::create_dir_all(&team_dir).expect("create team dir");
        std::fs::write(
            team_dir.join("config.json"),
            r#"{"name":"active-team","lead_agent_id":"lead","created_at":0,"members":[{"agent_id":"a1","name":"worker1","joined_at":0,"pane_id":"p1","cwd":".","is_active":true}],"hidden_pane_ids":[],"team_allowed_paths":[]}"#,
        )
        .expect("write config.json");

        let result = with_base_dir_override(temp.path().to_path_buf(), || {
            tasks::set_leader_team_name(Some("active-team".to_owned()))
                .expect("set leader team name");
            let input = json!({});
            let context = test_context();
            team_delete(&input, &context)
        });

        let output = result.expect("team_delete should return json");
        let parsed: Value = serde_json::from_str(&output).expect("valid json");
        assert_eq!(parsed["success"], false);
        assert!(
            parsed["message"]
                .as_str()
                .expect("message string")
                .contains("active member")
        );
        assert!(
            team_dir.exists(),
            "team should remain when members are active"
        );
    }

    #[test]
    #[serial_test::serial]
    fn team_list_returns_empty_when_no_teams() {
        let temp = TempDir::new().expect("temp dir");
        let result = with_base_dir_override(temp.path().to_path_buf(), || {
            let input = json!({});
            let context = test_context();
            team_list(&input, &context)
        });

        assert!(result.is_ok());
        let output = result.expect("team_list should succeed");
        let parsed: Value = serde_json::from_str(&output).expect("valid json");
        assert_eq!(parsed["total"], 0);
    }

    #[test]
    #[serial_test::serial]
    fn team_list_returns_existing_teams() {
        let temp = TempDir::new().expect("temp dir");
        let team_dir = temp.path().join("my-team");
        std::fs::create_dir_all(&team_dir).expect("create team dir");
        std::fs::write(
            team_dir.join("config.json"),
            r#"{"name":"my-team","lead_agent_id":"lead-123","created_at":1700000000,"members":[{"name":"worker1"}]}"#,
        )
        .expect("write config.json");

        let result = with_base_dir_override(temp.path().to_path_buf(), || {
            let input = json!({});
            let context = test_context();
            team_list(&input, &context)
        });

        assert!(result.is_ok());
        let output = result.expect("team_list should succeed");
        let parsed: Value = serde_json::from_str(&output).expect("valid json");
        assert_eq!(parsed["total"], 1);
        let teams = parsed["teams"].as_array().expect("teams array");
        assert_eq!(teams[0]["name"], "my-team");
        assert_eq!(teams[0]["member_count"], 1);
    }

    #[test]
    fn resolve_teams_base_dir_uses_override() {
        let dir = with_base_dir_override(PathBuf::from("/custom/path"), resolve_teams_base_dir);
        assert_eq!(dir, PathBuf::from("/custom/path"));
    }

    #[test]
    #[serial_test::serial]
    fn resolve_team_dir_sanitizes_name() {
        let dir =
            with_base_dir_override(PathBuf::from("/tmp"), || resolve_team_dir("my cool team"));
        assert!(dir.to_string_lossy().contains("my_cool_team"));
    }

    #[test]
    fn team_list_handles_nonexistent_base_dir() {
        let result = with_base_dir_override(PathBuf::from("/nonexistent/path/xyz/abc"), || {
            let input = json!({});
            let context = test_context();
            team_list(&input, &context)
        });
        assert!(result.is_ok());
        let output = result.expect("team_list should succeed");
        let parsed: Value = serde_json::from_str(&output).expect("valid json");
        assert_eq!(parsed["total"], 0);
    }

    #[test]
    #[serial_test::serial]
    fn team_delete_json_output_format() {
        let temp = TempDir::new().expect("temp dir");
        let result = with_base_dir_override(temp.path().to_path_buf(), || {
            tasks::set_leader_team_name(Some("nonexistent-xyz".to_owned()))
                .expect("set leader team name");
            let context = test_context();
            team_delete(&json!({}), &context)
        })
        .expect("team_delete should succeed");
        let parsed: Value = serde_json::from_str(&result).expect("valid json");
        assert!(parsed.get("success").is_some());
        assert!(parsed.get("message").is_some());
    }
}
