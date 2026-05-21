//! Spawn utilities for teammates.
//!
//! Builds command lines and environment variables for spawning
//! teammate processes.

use crate::constants::{
    ENV_AGENT_ID, ENV_AGENT_NAME, ENV_BACKEND_TYPE, ENV_LEAD_AGENT_ID, ENV_PERMISSION_MODE,
    ENV_TEAM_DIR, ENV_TEAM_NAME,
};
use crate::team_helpers;
use crate::types::SpawnConfig;

/// A constructed spawn command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnCommand {
    /// The program to execute.
    pub program: String,
    /// Arguments to pass.
    pub args: Vec<String>,
    /// Environment variables to set.
    pub env: Vec<(String, String)>,
}

impl SpawnCommand {
    /// Create a new spawn command.
    #[must_use]
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
        }
    }

    /// Add an argument.
    #[must_use]
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Add an environment variable.
    #[must_use]
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Render the command as a string.
    #[must_use]
    pub fn to_command_string(&self) -> String {
        let mut s = self.program.clone();
        for arg in &self.args {
            s.push(' ');
            if arg.contains(' ') {
                s.push_str(&format!("'{}'", arg));
            } else {
                s.push_str(arg);
            }
        }
        s
    }
}

/// Build a spawn command for a teammate.
#[must_use]
pub fn build_spawn_command(config: &SpawnConfig) -> SpawnCommand {
    let mut cmd = SpawnCommand::new("remote-code")
        .arg("--swarm")
        .arg("--agent-name")
        .arg(&config.agent_name)
        .arg("--team-name")
        .arg(&config.team_name);

    if let Some(ref model) = config.model {
        cmd = cmd.arg("--model").arg(model);
    }

    // Add standard environment variables.
    cmd = cmd
        .env(ENV_AGENT_ID, &config.agent_id)
        .env(ENV_AGENT_NAME, &config.agent_name)
        .env(ENV_TEAM_NAME, &config.team_name)
        .env(ENV_LEAD_AGENT_ID, &config.agent_id) // Will be overridden by actual lead ID
        .env(ENV_BACKEND_TYPE, config.backend_type.as_str())
        .env(
            ENV_TEAM_DIR,
            team_helpers::team_dir(&config.team_name)
                .to_string_lossy()
                .to_string(),
        );

    if let Some(ref mode) = config.permission_mode {
        cmd = cmd.env(ENV_PERMISSION_MODE, mode.as_legacy_str());
    }

    // Add custom environment variables.
    for (key, value) in &config.env_vars {
        cmd = cmd.env(key, value);
    }

    cmd
}

/// Build environment variables for a teammate process.
#[must_use]
pub fn build_env_vars(config: &SpawnConfig) -> Vec<(String, String)> {
    let mut env = vec![
        (ENV_AGENT_ID.to_owned(), config.agent_id.clone()),
        (ENV_AGENT_NAME.to_owned(), config.agent_name.clone()),
        (ENV_TEAM_NAME.to_owned(), config.team_name.clone()),
        (
            ENV_BACKEND_TYPE.to_owned(),
            config.backend_type.as_str().to_owned(),
        ),
        (
            ENV_TEAM_DIR.to_owned(),
            team_helpers::team_dir(&config.team_name)
                .to_string_lossy()
                .to_string(),
        ),
    ];

    if let Some(ref mode) = config.permission_mode {
        env.push((
            ENV_PERMISSION_MODE.to_owned(),
            mode.as_legacy_str().to_owned(),
        ));
    }

    // Add custom vars.
    env.extend(config.env_vars.clone());

    env
}

/// Build the working directory for a teammate.
///
/// Uses the worktree path if specified, otherwise the CWD.
#[must_use]
pub fn build_working_dir(config: &SpawnConfig) -> String {
    config
        .worktree_path
        .clone()
        .expect("should have worktree path or cwd")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::BackendType;
    use claude_core::PermissionMode;

    fn test_config() -> SpawnConfig {
        SpawnConfig {
            agent_id: "agent-123".to_owned(),
            agent_name: "worker-1".to_owned(),
            team_name: "test-team".to_owned(),
            model: Some("gpt-4".to_owned()),
            cwd: "/tmp/project".to_owned(),
            backend_type: BackendType::InProcess,
            env_vars: vec![("CUSTOM_VAR".to_owned(), "custom_value".to_owned())],
            permission_mode: Some(PermissionMode::Default),
            worktree_path: Some("/tmp/project-worktree".to_owned()),
        }
    }

    #[test]
    fn build_spawn_command_basic() {
        let config = test_config();
        let cmd = build_spawn_command(&config);
        assert_eq!(cmd.program, "remote-code");
        assert!(cmd.args.contains(&"--swarm".to_owned()));
        assert!(cmd.args.contains(&"worker-1".to_owned()));
        assert!(cmd.args.contains(&"test-team".to_owned()));
        assert!(cmd.args.contains(&"--model".to_owned()));
        assert!(cmd.args.contains(&"gpt-4".to_owned()));
    }

    #[test]
    fn build_spawn_command_env_vars() {
        let config = test_config();
        let cmd = build_spawn_command(&config);
        let env_keys: Vec<&str> = cmd.env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(env_keys.contains(&ENV_AGENT_ID));
        assert!(env_keys.contains(&ENV_AGENT_NAME));
        assert!(env_keys.contains(&ENV_TEAM_NAME));
        assert!(env_keys.contains(&ENV_BACKEND_TYPE));
        assert!(env_keys.contains(&ENV_PERMISSION_MODE));
    }

    #[test]
    fn build_spawn_command_custom_env() {
        let config = test_config();
        let cmd = build_spawn_command(&config);
        let custom = cmd.env.iter().find(|(k, _)| k == "CUSTOM_VAR");
        assert!(custom.is_some());
        assert_eq!(custom.expect("found").1, "custom_value");
    }

    #[test]
    fn build_env_vars_basic() {
        let config = test_config();
        let env = build_env_vars(&config);
        assert!(
            env.iter()
                .any(|(k, v)| k == ENV_AGENT_ID && v == "agent-123")
        );
        assert!(
            env.iter()
                .any(|(k, v)| k == ENV_AGENT_NAME && v == "worker-1")
        );
        assert!(
            env.iter()
                .any(|(k, v)| k == ENV_TEAM_NAME && v == "test-team")
        );
    }

    #[test]
    fn build_env_vars_custom() {
        let config = test_config();
        let env = build_env_vars(&config);
        assert!(
            env.iter()
                .any(|(k, v)| k == "CUSTOM_VAR" && v == "custom_value")
        );
    }

    #[test]
    fn build_working_dir_with_worktree() {
        let config = test_config();
        let dir = build_working_dir(&config);
        assert_eq!(dir, "/tmp/project-worktree");
    }

    #[test]
    fn spawn_command_to_string() {
        let cmd = SpawnCommand::new("remote-code")
            .arg("--swarm")
            .arg("--team-name")
            .arg("my team");
        let s = cmd.to_command_string();
        assert!(s.starts_with("remote-code"));
        assert!(s.contains("--swarm"));
        assert!(s.contains("'my team'"));
    }

    #[test]
    fn spawn_command_builder() {
        let cmd = SpawnCommand::new("prog").arg("arg1").env("KEY", "VALUE");
        assert_eq!(cmd.args, vec!["arg1"]);
        assert_eq!(cmd.env, vec![("KEY".to_owned(), "VALUE".to_owned())]);
    }

    #[test]
    fn build_spawn_command_no_model() {
        let config = SpawnConfig {
            agent_id: "a1".to_owned(),
            agent_name: "w1".to_owned(),
            team_name: "team".to_owned(),
            model: None,
            cwd: "/tmp".to_owned(),
            backend_type: BackendType::Tmux,
            env_vars: vec![],
            permission_mode: None,
            worktree_path: None,
        };
        let cmd = build_spawn_command(&config);
        assert!(!cmd.args.contains(&"--model".to_owned()));
    }
}
