//! Teammate initialization logic.
//!
//! Handles the initialization of a new teammate, including
//! identity creation, team file registration, and initial setup.

use uuid::Uuid;

use crate::constants::{
    ENV_AGENT_ID, ENV_AGENT_NAME, ENV_LEAD_AGENT_ID, ENV_TEAM_NAME, TEAM_LEAD_NAME,
};
use crate::error::{SwarmError, SwarmResult};
use crate::team_helpers;
use crate::types::{BackendType, TeamFile, TeammateIdentity};

/// Result of initializing a teammate.
#[derive(Debug, Clone)]
pub struct InitResult {
    /// The teammate's identity.
    pub identity: TeammateIdentity,
    /// The updated team file.
    pub team_file: TeamFile,
}

/// Initialize a new teammate in a team.
///
/// Creates the teammate identity, adds it to the team file,
/// and returns the updated team state.
pub async fn init_teammate(
    team_name: &str,
    agent_name: &str,
    lead_agent_id: &str,
    backend_type: BackendType,
    _cwd: &str,
) -> SwarmResult<InitResult> {
    team_helpers::validate_team_name(team_name)?;
    team_helpers::validate_agent_name(agent_name)?;

    if agent_name == TEAM_LEAD_NAME {
        return Err(SwarmError::InvalidAgentName(format!(
            "'{agent_name}' is reserved for the team lead"
        )));
    }

    let mut team = team_helpers::read_team(team_name).await?;

    if team.has_member(agent_name) {
        return Err(SwarmError::TeammateNotFound {
            agent_name: format!("{agent_name} (already exists)"),
            team_name: team_name.to_owned(),
        });
    }

    let agent_id = Uuid::new_v4().to_string();
    let identity = TeammateIdentity {
        agent_id: agent_id.clone(),
        name: agent_name.to_owned(),
        team_name: team_name.to_owned(),
        is_lead: false,
        lead_agent_id: lead_agent_id.to_owned(),
        backend_type,
    };

    let mut member =
        crate::types::TeamMember::new(&agent_id, agent_name, format!("pane-{agent_id}"), _cwd);
    member.backend_type = Some(backend_type);

    team.members.push(member);
    team_helpers::update_team(&team).await?;

    Ok(InitResult {
        identity,
        team_file: team,
    })
}

/// Initialize the team lead.
///
/// Creates the team and the lead agent identity.
pub async fn init_lead(
    team_name: &str,
    lead_agent_id: &str,
    backend_type: BackendType,
    _cwd: &str,
) -> SwarmResult<(TeammateIdentity, TeamFile)> {
    team_helpers::validate_team_name(team_name)?;

    let team = TeamFile::new(team_name, lead_agent_id);
    team_helpers::create_team(&team).await?;

    let identity = TeammateIdentity {
        agent_id: lead_agent_id.to_owned(),
        name: TEAM_LEAD_NAME.to_owned(),
        team_name: team_name.to_owned(),
        is_lead: true,
        lead_agent_id: lead_agent_id.to_owned(),
        backend_type,
    };

    Ok((identity, team))
}

/// Parse teammate identity from environment variables.
///
/// Reads `RC_SWARM_AGENT_ID`, `RC_SWARM_AGENT_NAME`, `RC_SWARM_TEAM_NAME`,
/// and `RC_SWARM_LEAD_AGENT_ID` to reconstruct the identity.
pub fn identity_from_env() -> Option<TeammateIdentity> {
    let agent_id = std::env::var(ENV_AGENT_ID).ok()?;
    let agent_name = std::env::var(ENV_AGENT_NAME).ok()?;
    let team_name = std::env::var(ENV_TEAM_NAME).ok()?;
    let lead_agent_id = std::env::var(ENV_LEAD_AGENT_ID).ok()?;
    let backend_type_str = std::env::var("RC_SWARM_BACKEND_TYPE")
        .ok()
        .and_then(|s| BackendType::from_str_opt(&s))?;

    let is_lead = agent_name == TEAM_LEAD_NAME;

    Some(TeammateIdentity {
        agent_id,
        name: agent_name,
        team_name,
        is_lead,
        lead_agent_id,
        backend_type: backend_type_str,
    })
}

/// Build environment variable pairs for a teammate identity.
///
/// Returns the env vars as a Vec of (key, value) pairs so the caller
/// can set them in the appropriate context (e.g., spawn command env).
/// This avoids using `std::env::set_var` which is unsafe in Rust 2024.
#[must_use]
pub fn teammate_env_vars(identity: &TeammateIdentity) -> Vec<(String, String)> {
    vec![
        (ENV_AGENT_ID.to_owned(), identity.agent_id.clone()),
        (ENV_AGENT_NAME.to_owned(), identity.name.clone()),
        (ENV_TEAM_NAME.to_owned(), identity.team_name.clone()),
        (ENV_LEAD_AGENT_ID.to_owned(), identity.lead_agent_id.clone()),
        (
            "RC_SWARM_BACKEND_TYPE".to_owned(),
            identity.backend_type.as_str().to_owned(),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::team_helpers::set_base_dir_override;

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
    async fn init_lead_creates_team() {
        let _td = TestDir::new();
        let (identity, team) = init_lead("test-team", "lead-123", BackendType::InProcess, "/tmp")
            .await
            .expect("should init lead");
        assert!(identity.is_lead);
        assert_eq!(identity.name, "lead");
        assert_eq!(team.lead_agent_id, "lead-123");
    }

    #[tokio::test]
    async fn init_teammate_adds_member() {
        let _td = TestDir::new();
        init_lead("test-team", "lead-123", BackendType::InProcess, "/tmp")
            .await
            .expect("should init lead");

        let result = init_teammate(
            "test-team",
            "worker-1",
            "lead-123",
            BackendType::InProcess,
            "/tmp",
        )
        .await
        .expect("should init teammate");
        assert!(!result.identity.is_lead);
        assert_eq!(result.identity.name, "worker-1");
        assert_eq!(result.team_file.members.len(), 1);
    }

    #[tokio::test]
    async fn init_teammate_reserved_name() {
        let _td = TestDir::new();
        init_lead("test-team", "lead-123", BackendType::InProcess, "/tmp")
            .await
            .expect("should init lead");

        let result = init_teammate(
            "test-team",
            "lead",
            "lead-123",
            BackendType::InProcess,
            "/tmp",
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn init_teammate_duplicate_name() {
        let _td = TestDir::new();
        init_lead("test-team", "lead-123", BackendType::InProcess, "/tmp")
            .await
            .expect("should init lead");

        init_teammate(
            "test-team",
            "worker-1",
            "lead-123",
            BackendType::InProcess,
            "/tmp",
        )
        .await
        .expect("should init first");

        let result = init_teammate(
            "test-team",
            "worker-1",
            "lead-123",
            BackendType::InProcess,
            "/tmp",
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn init_teammate_invalid_team() {
        let _td = TestDir::new();
        let result = init_teammate(
            "nonexistent",
            "worker-1",
            "lead-123",
            BackendType::InProcess,
            "/tmp",
        )
        .await;
        assert!(result.is_err());
    }

    #[test]
    fn teammate_env_vars_contains_all_keys() {
        let identity = TeammateIdentity {
            agent_id: "a1".to_owned(),
            name: "worker-1".to_owned(),
            team_name: "team-1".to_owned(),
            is_lead: false,
            lead_agent_id: "lead-1".to_owned(),
            backend_type: BackendType::InProcess,
        };
        let vars = teammate_env_vars(&identity);
        assert_eq!(vars.len(), 5);
        let keys: Vec<&str> = vars.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&ENV_AGENT_ID));
        assert!(keys.contains(&ENV_AGENT_NAME));
        assert!(keys.contains(&ENV_TEAM_NAME));
        assert!(keys.contains(&ENV_LEAD_AGENT_ID));
    }

    #[test]
    fn identity_from_env_missing() {
        // This test just verifies the function handles missing env vars gracefully.
        let result = identity_from_env();
        // Result depends on whether env vars are set; just ensure no panic.
        let _ = result;
    }
}
