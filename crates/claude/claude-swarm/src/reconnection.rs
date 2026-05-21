//! Reconnection logic for teammates.
//!
//! Handles reconnecting a teammate to an existing team session,
//! including state recovery and session validation.

use crate::error::{SwarmError, SwarmResult};
use crate::team_helpers;
use crate::types::{TeamFile, TeammateIdentity};

/// Result of a reconnection attempt.
#[derive(Debug, Clone)]
pub struct ReconnectionResult {
    /// The teammate's identity.
    pub identity: TeammateIdentity,
    /// The current team file.
    pub team_file: TeamFile,
    /// Whether the teammate was found active.
    pub was_active: bool,
}

/// Attempt to reconnect a teammate to a team.
///
/// Looks up the teammate in the team file and restores their identity.
pub async fn reconnect_teammate(
    team_name: &str,
    agent_name: &str,
) -> SwarmResult<ReconnectionResult> {
    let team = team_helpers::read_team(team_name).await?;

    let member = team
        .find_member(agent_name)
        .ok_or_else(|| SwarmError::ReconnectionFailed {
            agent_name: agent_name.to_owned(),
            reason: "agent not found in team".to_owned(),
        })?;

    let was_active = member.is_active.unwrap_or(false);

    let identity = TeammateIdentity {
        agent_id: member.agent_id.clone(),
        name: member.name.clone(),
        team_name: team_name.to_owned(),
        is_lead: member.agent_id == team.lead_agent_id,
        lead_agent_id: team.lead_agent_id.clone(),
        backend_type: member.backend_type.expect("should have backend type"),
    };

    Ok(ReconnectionResult {
        identity,
        team_file: team,
        was_active,
    })
}

/// Check if a teammate can be reconnected.
///
/// A teammate can be reconnected if:
/// - The team exists
/// - The teammate is in the team
/// - The teammate has a session ID
pub async fn can_reconnect(team_name: &str, agent_name: &str) -> bool {
    let team = match team_helpers::read_team(team_name).await {
        Ok(t) => t,
        Err(_) => return false,
    };

    let member = match team.find_member(agent_name) {
        Some(m) => m,
        None => return false,
    };

    member.session_id.is_some()
}

/// Mark a teammate as reconnected (active).
pub async fn mark_reconnected(team_name: &str, agent_name: &str) -> SwarmResult<TeamFile> {
    team_helpers::update_member_status(team_name, agent_name, true).await
}

/// Mark a teammate as disconnected (inactive).
pub async fn mark_disconnected(team_name: &str, agent_name: &str) -> SwarmResult<TeamFile> {
    team_helpers::update_member_status(team_name, agent_name, false).await
}

/// List all reconnectable teammates in a team.
///
/// Returns teammates that have a session ID but are marked as inactive.
pub async fn list_reconnectable(team_name: &str) -> SwarmResult<Vec<TeammateIdentity>> {
    let team = team_helpers::read_team(team_name).await?;

    let reconnectable: Vec<TeammateIdentity> = team
        .members
        .iter()
        .filter(|m| m.session_id.is_some() && !m.is_active.unwrap_or(false))
        .map(|m| TeammateIdentity {
            agent_id: m.agent_id.clone(),
            name: m.name.clone(),
            team_name: team_name.to_owned(),
            is_lead: m.agent_id == team.lead_agent_id,
            lead_agent_id: team.lead_agent_id.clone(),
            backend_type: m.backend_type.expect("should have backend type"),
        })
        .collect();

    Ok(reconnectable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::team_helpers::set_base_dir_override;
    use crate::teammate_init;
    use crate::types::BackendType;

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

    #[tokio::test]
    async fn reconnect_existing_teammate() {
        let _td = TestDir::new();
        teammate_init::init_lead("test-team", "lead-123", BackendType::InProcess, "/tmp")
            .await
            .expect("should init lead");

        teammate_init::init_teammate(
            "test-team",
            "worker-1",
            "lead-123",
            BackendType::InProcess,
            "/tmp",
        )
        .await
        .expect("should init teammate");

        let result = reconnect_teammate("test-team", "worker-1")
            .await
            .expect("should reconnect");
        assert_eq!(result.identity.name, "worker-1");
        assert!(!result.identity.is_lead);
    }

    #[tokio::test]
    async fn reconnect_nonexistent_teammate() {
        let _td = TestDir::new();
        teammate_init::init_lead("test-team", "lead-123", BackendType::InProcess, "/tmp")
            .await
            .expect("should init lead");

        let result = reconnect_teammate("test-team", "nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn can_reconnect_check() {
        let _td = TestDir::new();
        teammate_init::init_lead("test-team", "lead-123", BackendType::InProcess, "/tmp")
            .await
            .expect("should init lead");

        teammate_init::init_teammate(
            "test-team",
            "worker-1",
            "lead-123",
            BackendType::InProcess,
            "/tmp",
        )
        .await
        .expect("should init teammate");

        // Without session_id, can't reconnect.
        assert!(!can_reconnect("test-team", "worker-1").await);
    }

    #[tokio::test]
    async fn can_reconnect_with_session() {
        let _td = TestDir::new();
        teammate_init::init_lead("test-team", "lead-123", BackendType::InProcess, "/tmp")
            .await
            .expect("should init lead");

        teammate_init::init_teammate(
            "test-team",
            "worker-1",
            "lead-123",
            BackendType::InProcess,
            "/tmp",
        )
        .await
        .expect("should init teammate");

        // Add session_id manually.
        let mut team = team_helpers::read_team("test-team").await.expect("ok");
        team.find_member_mut("worker-1").expect("found").session_id = Some("sess-1".to_owned());
        team_helpers::update_team(&team).await.expect("ok");

        assert!(can_reconnect("test-team", "worker-1").await);
    }

    #[tokio::test]
    async fn mark_reconnected_and_disconnected() {
        let _td = TestDir::new();
        teammate_init::init_lead("test-team", "lead-123", BackendType::InProcess, "/tmp")
            .await
            .expect("should init lead");

        teammate_init::init_teammate(
            "test-team",
            "worker-1",
            "lead-123",
            BackendType::InProcess,
            "/tmp",
        )
        .await
        .expect("should init teammate");

        mark_disconnected("test-team", "worker-1")
            .await
            .expect("should disconnect");

        let team = team_helpers::read_team("test-team").await.expect("ok");
        assert_eq!(
            team.find_member("worker-1").expect("found").is_active,
            Some(false)
        );

        mark_reconnected("test-team", "worker-1")
            .await
            .expect("should reconnect");

        let team = team_helpers::read_team("test-team").await.expect("ok");
        assert_eq!(
            team.find_member("worker-1").expect("found").is_active,
            Some(true)
        );
    }

    #[tokio::test]
    async fn list_reconnectable_teammates() {
        let _td = TestDir::new();
        teammate_init::init_lead("test-team", "lead-123", BackendType::InProcess, "/tmp")
            .await
            .expect("should init lead");

        teammate_init::init_teammate(
            "test-team",
            "worker-1",
            "lead-123",
            BackendType::InProcess,
            "/tmp",
        )
        .await
        .expect("should init teammate");

        // Set up: worker-1 has session but is inactive.
        let mut team = team_helpers::read_team("test-team").await.expect("ok");
        let m = team.find_member_mut("worker-1").expect("found");
        m.session_id = Some("sess-1".to_owned());
        m.is_active = Some(false);
        team_helpers::update_team(&team).await.expect("ok");

        let reconnectable = list_reconnectable("test-team").await.expect("ok");
        assert_eq!(reconnectable.len(), 1);
        assert_eq!(reconnectable[0].name, "worker-1");
    }

    #[tokio::test]
    async fn list_reconnectable_empty() {
        let _td = TestDir::new();
        teammate_init::init_lead("test-team", "lead-123", BackendType::InProcess, "/tmp")
            .await
            .expect("should init lead");

        let reconnectable = list_reconnectable("test-team").await.expect("ok");
        assert!(reconnectable.is_empty());
    }
}
