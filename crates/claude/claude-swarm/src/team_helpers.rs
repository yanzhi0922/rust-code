//! Team file CRUD operations and member management.
//!
//! Provides functions for creating, reading, updating, and deleting
//! team files on the file system, as well as member management
//! and name sanitization.

use std::cell::RefCell;
use std::path::PathBuf;

use tokio::fs;

use crate::constants::{MAX_TEAMMATES, TEAM_FILE_NAME};
use crate::error::{SwarmError, SwarmResult};
use crate::types::{TeamFile, TeamMember};

thread_local! {
    /// Override for the teams base directory (used in tests).
    static BASE_DIR_OVERRIDE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Set the base directory override for the current thread.
/// This is intended for use in tests only.
pub fn set_base_dir_override(dir: Option<PathBuf>) {
    BASE_DIR_OVERRIDE.with(|cell| {
        *cell.borrow_mut() = dir;
    });
}

/// Get the base directory override for the current thread.
pub fn base_dir_override() -> Option<PathBuf> {
    BASE_DIR_OVERRIDE.with(|cell| cell.borrow().clone())
}

/// Sanitize a team name for use as a directory name.
///
/// Replaces characters that are invalid in file names with underscores.
pub fn sanitize_team_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Validate a team name.
///
/// A valid team name is non-empty, starts with an alphanumeric character,
/// and contains only alphanumeric characters, hyphens, and underscores.
pub fn validate_team_name(name: &str) -> SwarmResult<()> {
    if name.is_empty() {
        return Err(SwarmError::InvalidTeamName(
            "name cannot be empty".to_owned(),
        ));
    }
    if name.len() > 64 {
        return Err(SwarmError::InvalidTeamName(
            "name cannot exceed 64 characters".to_owned(),
        ));
    }
    let first = name.chars().next().expect("name is non-empty");
    if !first.is_ascii_alphanumeric() {
        return Err(SwarmError::InvalidTeamName(
            "name must start with an alphanumeric character".to_owned(),
        ));
    }
    for c in name.chars() {
        if !c.is_ascii_alphanumeric() && c != '-' && c != '_' {
            return Err(SwarmError::InvalidTeamName(format!(
                "invalid character '{c}' in team name"
            )));
        }
    }
    Ok(())
}

/// Validate an agent name.
pub fn validate_agent_name(name: &str) -> SwarmResult<()> {
    if name.is_empty() {
        return Err(SwarmError::InvalidAgentName(
            "name cannot be empty".to_owned(),
        ));
    }
    if name.len() > 32 {
        return Err(SwarmError::InvalidAgentName(
            "name cannot exceed 32 characters".to_owned(),
        ));
    }
    let first = name.chars().next().expect("name is non-empty");
    if !first.is_ascii_alphanumeric() {
        return Err(SwarmError::InvalidAgentName(
            "name must start with an alphanumeric character".to_owned(),
        ));
    }
    Ok(())
}

/// Return the Claude-style config home directory.
///
/// Mirrors the research implementation:
/// - `CLAUDE_CONFIG_DIR` wins when set
/// - otherwise fall back to `$HOME/.claude` / `%USERPROFILE%/.claude`
pub fn claude_config_home_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        PathBuf::from(dir)
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".claude")
    } else if let Ok(userprofile) = std::env::var("USERPROFILE") {
        PathBuf::from(userprofile).join(".claude")
    } else {
        PathBuf::from(".claude")
    }
}

/// Get the base directory for team data.
///
/// Uses the thread-local override if set, then `$RC_SWARM_TEAM_DIR`,
/// then `CLAUDE_CONFIG_DIR/teams`, then a relative path.
pub fn teams_base_dir() -> PathBuf {
    // Check thread-local override first (for tests).
    if let Some(dir) = base_dir_override() {
        return dir;
    }

    if let Ok(dir) = std::env::var("RC_SWARM_TEAM_DIR") {
        PathBuf::from(dir)
    } else {
        claude_config_home_dir().join("teams")
    }
}

/// Get the directory for a specific team.
pub fn team_dir(team_name: &str) -> PathBuf {
    teams_base_dir().join(sanitize_team_name(team_name))
}

/// Get the path to a team's JSON file.
pub fn team_file_path(team_name: &str) -> PathBuf {
    team_dir(team_name).join(TEAM_FILE_NAME)
}

/// Create a new team.
///
/// Creates the team directory and writes the initial team config.
pub async fn create_team(team: &TeamFile) -> SwarmResult<()> {
    validate_team_name(&team.name)?;
    let dir = team_dir(&team.name);
    let file_path = team_file_path(&team.name);

    // Check if team already exists.
    if file_path.exists() {
        return Err(SwarmError::TeamAlreadyExists(team.name.clone()));
    }

    // Create directory.
    fs::create_dir_all(&dir).await?;

    // Write team file.
    let json = serde_json::to_string_pretty(team)?;
    fs::write(&file_path, json).await?;

    Ok(())
}

/// Read a team file.
pub async fn read_team(team_name: &str) -> SwarmResult<TeamFile> {
    let file_path = team_file_path(team_name);
    if !file_path.exists() {
        return Err(SwarmError::TeamNotFound(team_name.to_owned()));
    }
    let content = fs::read_to_string(&file_path).await?;
    let team: TeamFile = serde_json::from_str(&content)?;
    Ok(team)
}

/// Update a team file.
pub async fn update_team(team: &TeamFile) -> SwarmResult<()> {
    let file_path = team_file_path(&team.name);
    if !file_path.exists() {
        return Err(SwarmError::TeamNotFound(team.name.clone()));
    }
    let json = serde_json::to_string_pretty(team)?;
    fs::write(&file_path, json).await?;
    Ok(())
}

/// Delete a team.
pub async fn delete_team(team_name: &str) -> SwarmResult<()> {
    let dir = team_dir(team_name);
    if !dir.exists() {
        return Err(SwarmError::TeamNotFound(team_name.to_owned()));
    }
    fs::remove_dir_all(&dir).await?;
    Ok(())
}

/// Add a member to a team.
pub async fn add_member(team_name: &str, member: TeamMember) -> SwarmResult<TeamFile> {
    validate_agent_name(&member.name)?;
    let mut team = read_team(team_name).await?;

    if team.has_member(&member.name) {
        return Err(SwarmError::TeammateNotFound {
            agent_name: format!("{} (already exists)", member.name),
            team_name: team_name.to_owned(),
        });
    }

    if team.members.len() >= MAX_TEAMMATES {
        return Err(SwarmError::MaxTeammatesExceeded {
            max: MAX_TEAMMATES,
            team_name: team_name.to_owned(),
        });
    }

    team.members.push(member);
    update_team(&team).await?;
    Ok(team)
}

/// Remove a member from a team.
pub async fn remove_member(team_name: &str, agent_name: &str) -> SwarmResult<TeamFile> {
    let mut team = read_team(team_name).await?;
    team.remove_member(agent_name)
        .ok_or_else(|| SwarmError::TeammateNotFound {
            agent_name: agent_name.to_owned(),
            team_name: team_name.to_owned(),
        })?;
    update_team(&team).await?;
    Ok(team)
}

/// Update a member's status.
pub async fn update_member_status(
    team_name: &str,
    agent_name: &str,
    is_active: bool,
) -> SwarmResult<TeamFile> {
    let mut team = read_team(team_name).await?;
    let member = team
        .find_member_mut(agent_name)
        .ok_or_else(|| SwarmError::TeammateNotFound {
            agent_name: agent_name.to_owned(),
            team_name: team_name.to_owned(),
        })?;
    member.is_active = Some(is_active);
    update_team(&team).await?;
    Ok(team)
}

/// List all team names in the base directory.
pub async fn list_teams() -> SwarmResult<Vec<String>> {
    let base = teams_base_dir();
    if !base.exists() {
        return Ok(Vec::new());
    }
    let mut entries = fs::read_dir(&base).await?;
    let mut teams = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.is_dir() {
            let team_file = path.join(TEAM_FILE_NAME);
            if team_file.exists()
                && let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string())
            {
                teams.push(name);
            }
        }
    }
    Ok(teams)
}

/// Check if a team exists.
pub fn team_exists(team_name: &str) -> bool {
    team_file_path(team_name).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test helper: set up a temp directory as the teams base dir.
    struct TestDir {
        _temp: tempfile::TempDir,
    }

    impl TestDir {
        fn new() -> Self {
            let temp = tempfile::tempdir().expect("tempdir");
            let path = temp.path().to_path_buf();
            set_base_dir_override(Some(path));
            Self { _temp: temp }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            set_base_dir_override(None);
        }
    }

    #[test]
    fn sanitize_team_name_basic() {
        assert_eq!(sanitize_team_name("my-team"), "my-team");
        assert_eq!(sanitize_team_name("my team"), "my_team");
        assert_eq!(sanitize_team_name("my/team"), "my_team");
        assert_eq!(sanitize_team_name("a.b"), "a_b");
    }

    #[test]
    fn sanitize_team_name_special_chars() {
        assert_eq!(sanitize_team_name("hello!@#$%"), "hello_____");
    }

    #[test]
    fn validate_team_name_valid() {
        assert!(validate_team_name("my-team").is_ok());
        assert!(validate_team_name("my_team").is_ok());
        assert!(validate_team_name("MyTeam123").is_ok());
    }

    #[test]
    fn validate_team_name_empty() {
        assert!(validate_team_name("").is_err());
    }

    #[test]
    fn validate_team_name_too_long() {
        let long_name = "a".repeat(65);
        assert!(validate_team_name(&long_name).is_err());
    }

    #[test]
    fn validate_team_name_starts_with_special() {
        assert!(validate_team_name("-team").is_err());
        assert!(validate_team_name("_team").is_err());
    }

    #[test]
    fn validate_team_name_invalid_chars() {
        assert!(validate_team_name("my team").is_err());
        assert!(validate_team_name("my/team").is_err());
        assert!(validate_team_name("my.team").is_err());
    }

    #[test]
    fn validate_agent_name_valid() {
        assert!(validate_agent_name("worker-1").is_ok());
        assert!(validate_agent_name("lead").is_ok());
    }

    #[test]
    fn validate_agent_name_empty() {
        assert!(validate_agent_name("").is_err());
    }

    #[test]
    fn validate_agent_name_too_long() {
        let long_name = "a".repeat(33);
        assert!(validate_agent_name(&long_name).is_err());
    }

    #[test]
    fn validate_agent_name_starts_with_special() {
        assert!(validate_agent_name("-agent").is_err());
    }

    #[tokio::test]
    async fn test_create_and_read_team() {
        let _td = TestDir::new();
        let team = TeamFile::new("test-team", "lead-123");
        create_team(&team).await.expect("should create");
        let read = read_team("test-team").await.expect("should read");
        assert_eq!(read.name, "test-team");
        assert_eq!(read.lead_agent_id, "lead-123");
    }

    #[tokio::test]
    async fn test_create_team_duplicate() {
        let _td = TestDir::new();
        let team = TeamFile::new("dup-team", "lead-1");
        create_team(&team).await.expect("should create");
        let result = create_team(&team).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_read_nonexistent_team() {
        let _td = TestDir::new();
        let result = read_team("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_update_team() {
        let _td = TestDir::new();
        let mut team = TeamFile::new("update-team", "lead-1");
        create_team(&team).await.expect("should create");
        team.description = Some("updated description".to_owned());
        update_team(&team).await.expect("should update");
        let read = read_team("update-team").await.expect("should read");
        assert_eq!(read.description.as_deref(), Some("updated description"));
    }

    #[tokio::test]
    async fn test_delete_team() {
        let _td = TestDir::new();
        let team = TeamFile::new("delete-team", "lead-1");
        create_team(&team).await.expect("should create");
        delete_team("delete-team").await.expect("should delete");
        let result = read_team("delete-team").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_add_and_remove_member() {
        let _td = TestDir::new();
        let team = TeamFile::new("member-team", "lead-1");
        create_team(&team).await.expect("should create");

        let member = TeamMember::new("agent-1", "worker-1", "pane-1", "/tmp");
        let updated = add_member("member-team", member).await.expect("should add");
        assert_eq!(updated.members.len(), 1);

        let updated = remove_member("member-team", "worker-1")
            .await
            .expect("should remove");
        assert!(updated.members.is_empty());
    }

    #[tokio::test]
    async fn test_add_member_exceeds_max() {
        let _td = TestDir::new();
        let team = TeamFile::new("max-team", "lead-1");
        create_team(&team).await.expect("should create");

        for i in 0..MAX_TEAMMATES {
            let member = TeamMember::new(
                format!("agent-{i}"),
                format!("worker-{i}"),
                format!("pane-{i}"),
                "/tmp",
            );
            add_member("max-team", member).await.expect("should add");
        }

        let extra = TeamMember::new("agent-extra", "worker-extra", "pane-extra", "/tmp");
        let result = add_member("max-team", extra).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_update_member_status() {
        let _td = TestDir::new();
        let team = TeamFile::new("status-team", "lead-1");
        create_team(&team).await.expect("should create");

        let member = TeamMember::new("agent-1", "worker-1", "pane-1", "/tmp");
        add_member("status-team", member).await.expect("should add");

        let updated = update_member_status("status-team", "worker-1", false)
            .await
            .expect("should update");
        let m = updated.find_member("worker-1").expect("should find");
        assert_eq!(m.is_active, Some(false));
    }

    #[tokio::test]
    async fn test_list_teams_empty() {
        let _td = TestDir::new();
        let teams = list_teams().await.expect("should list");
        assert!(teams.is_empty());
    }

    #[tokio::test]
    async fn test_list_teams_multiple() {
        let _td = TestDir::new();
        create_team(&TeamFile::new("team-a", "lead-1"))
            .await
            .expect("ok");
        create_team(&TeamFile::new("team-b", "lead-2"))
            .await
            .expect("ok");

        let teams = list_teams().await.expect("should list");
        assert_eq!(teams.len(), 2);
    }

    #[test]
    fn team_dir_path() {
        let _td = TestDir::new();
        let dir = team_dir("my-team");
        assert!(dir.to_string_lossy().contains("my-team"));
    }

    #[test]
    fn team_file_path_check() {
        let _td = TestDir::new();
        let path = team_file_path("my-team");
        assert!(path.to_string_lossy().ends_with("config.json"));
    }

    #[test]
    fn base_dir_override_works() {
        let path = PathBuf::from("/tmp/test-override");
        set_base_dir_override(Some(path.clone()));
        assert_eq!(teams_base_dir(), path);
        set_base_dir_override(None);
    }
}
