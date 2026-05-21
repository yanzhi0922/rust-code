//! Backend type definitions.
//!
//! Provides [`BackendType`], [`PaneId`], [`CreatePaneResult`], and related
//! types used by the backend abstraction layer.

use serde::{Deserialize, Serialize};

use crate::types::BackendType;

/// Identifier for a terminal pane.
pub type PaneId = String;

/// Result of creating a new pane.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreatePaneResult {
    /// The ID of the newly created pane.
    pub pane_id: PaneId,
    /// Whether this is the first teammate in the team.
    pub is_first_teammate: bool,
}

impl CreatePaneResult {
    /// Create a new pane result.
    #[must_use]
    pub fn new(pane_id: impl Into<String>, is_first_teammate: bool) -> Self {
        Self {
            pane_id: pane_id.into(),
            is_first_teammate,
        }
    }
}

/// Configuration for creating a pane.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PaneConfig {
    /// Name for the pane.
    pub name: String,
    /// Working directory for the pane.
    pub cwd: String,
    /// Environment variables to set.
    #[serde(default)]
    pub env_vars: Vec<(String, String)>,
    /// Command to run in the pane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Backend type.
    pub backend_type: BackendType,
}

impl PaneConfig {
    /// Create a new pane configuration.
    #[must_use]
    pub fn new(name: impl Into<String>, cwd: impl Into<String>, backend_type: BackendType) -> Self {
        Self {
            name: name.into(),
            cwd: cwd.into(),
            env_vars: Vec::new(),
            command: None,
            backend_type,
        }
    }

    /// Add an environment variable.
    #[must_use]
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env_vars.push((key.into(), value.into()));
        self
    }

    /// Set the command to run.
    #[must_use]
    pub fn with_command(mut self, command: impl Into<String>) -> Self {
        self.command = Some(command.into());
        self
    }
}

/// Status of a backend pane.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PaneStatus {
    /// Pane is running.
    Running,
    /// Pane has exited.
    Exited,
    /// Pane does not exist.
    NotFound,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_pane_result_new() {
        let result = CreatePaneResult::new("pane-1", true);
        assert_eq!(result.pane_id, "pane-1");
        assert!(result.is_first_teammate);
    }

    #[test]
    fn create_pane_result_not_first() {
        let result = CreatePaneResult::new("pane-2", false);
        assert!(!result.is_first_teammate);
    }

    #[test]
    fn pane_config_new() {
        let config = PaneConfig::new("worker-1", "/tmp", BackendType::InProcess);
        assert_eq!(config.name, "worker-1");
        assert_eq!(config.cwd, "/tmp");
        assert!(config.env_vars.is_empty());
        assert!(config.command.is_none());
    }

    #[test]
    fn pane_config_with_env() {
        let config = PaneConfig::new("worker-1", "/tmp", BackendType::InProcess)
            .with_env("KEY1", "val1")
            .with_env("KEY2", "val2");
        assert_eq!(config.env_vars.len(), 2);
        assert_eq!(config.env_vars[0], ("KEY1".to_owned(), "val1".to_owned()));
        assert_eq!(config.env_vars[1], ("KEY2".to_owned(), "val2".to_owned()));
    }

    #[test]
    fn pane_config_with_command() {
        let config = PaneConfig::new("worker-1", "/tmp", BackendType::Tmux)
            .with_command("remote-code --swarm");
        assert_eq!(config.command.as_deref(), Some("remote-code --swarm"));
    }

    #[test]
    fn pane_config_builder_chain() {
        let config = PaneConfig::new("w", "/tmp", BackendType::InProcess)
            .with_env("A", "B")
            .with_command("run");
        assert_eq!(config.env_vars.len(), 1);
        assert!(config.command.is_some());
    }

    #[test]
    fn pane_status_values() {
        assert_ne!(PaneStatus::Running, PaneStatus::Exited);
        assert_ne!(PaneStatus::Exited, PaneStatus::NotFound);
    }

    #[test]
    fn create_pane_result_serialization() {
        let result = CreatePaneResult::new("pane-1", true);
        let json = serde_json::to_string(&result).expect("should serialize");
        let result2: CreatePaneResult = serde_json::from_str(&json).expect("should deserialize");
        assert_eq!(result, result2);
    }

    #[test]
    fn pane_config_serialization() {
        let config = PaneConfig::new("w", "/tmp", BackendType::Tmux)
            .with_env("K", "V")
            .with_command("cmd");
        let json = serde_json::to_string(&config).expect("should serialize");
        let config2: PaneConfig = serde_json::from_str(&json).expect("should deserialize");
        assert_eq!(config, config2);
    }
}
