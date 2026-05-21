//! Pane-based backend executor.
//!
//! A generic [`PaneExecutor`] that wraps any [`PaneBackend`] and
//! implements the [`TeammateExecutor`] trait.

use async_trait::async_trait;
use std::sync::Arc;

use crate::backends::PaneBackend;
use crate::backends::TeammateExecutor;
use crate::backends::types::PaneConfig;
use crate::backends::types::{CreatePaneResult, PaneId};
use crate::error::SwarmResult;
use crate::types::SpawnConfig;

/// Generic pane-based teammate executor.
///
/// Wraps a [`PaneBackend`] and uses it to start/stop teammates
/// by creating and destroying panes.
pub struct PaneExecutor {
    backend: Arc<dyn PaneBackend>,
}

impl PaneExecutor {
    /// Create a new pane executor wrapping the given backend.
    #[must_use]
    pub fn new(backend: Arc<dyn PaneBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl TeammateExecutor for PaneExecutor {
    async fn start_teammate(&self, config: &SpawnConfig) -> SwarmResult<CreatePaneResult> {
        let mut pane_config = PaneConfig::new(&config.agent_name, &config.cwd, config.backend_type);
        // Add environment variables.
        for (key, value) in &config.env_vars {
            pane_config = pane_config.with_env(key, value);
        }
        // Set command if available.
        pane_config = pane_config.with_command(format!(
            "remote-code --swarm --agent-name {} --team-name {}",
            config.agent_name, config.team_name
        ));
        self.backend.create_pane(&pane_config).await
    }

    async fn stop_teammate(&self, pane_id: &PaneId) -> SwarmResult<()> {
        self.backend.destroy_pane(pane_id).await
    }

    async fn is_teammate_running(&self, pane_id: &PaneId) -> SwarmResult<bool> {
        use crate::backends::types::PaneStatus;
        let status = self.backend.pane_status(pane_id).await?;
        Ok(status == PaneStatus::Running)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::in_process::InProcessBackend;
    use crate::types::BackendType;
    use claude_core::PermissionMode;

    #[tokio::test]
    async fn pane_executor_start_teammate() {
        let backend = Arc::new(InProcessBackend::new());
        let executor = PaneExecutor::new(backend);
        let config = SpawnConfig {
            agent_id: "a1".to_owned(),
            agent_name: "worker-1".to_owned(),
            team_name: "team-1".to_owned(),
            model: None,
            cwd: "/tmp".to_owned(),
            backend_type: BackendType::InProcess,
            env_vars: vec![("KEY".to_owned(), "VALUE".to_owned())],
            permission_mode: Some(PermissionMode::Default),
            worktree_path: None,
        };
        let result = executor
            .start_teammate(&config)
            .await
            .expect("should start");
        assert!(result.is_first_teammate);
    }

    #[tokio::test]
    async fn pane_executor_stop_teammate() {
        let backend: Arc<dyn crate::backends::PaneBackend> = Arc::new(InProcessBackend::new());
        let executor = PaneExecutor::new(Arc::clone(&backend));
        let pane_config = PaneConfig::new("w1", "/tmp", BackendType::InProcess);
        let result = backend.create_pane(&pane_config).await.expect("ok");
        executor
            .stop_teammate(&result.pane_id)
            .await
            .expect("should stop");
    }

    #[tokio::test]
    async fn pane_executor_is_running() {
        let backend: Arc<dyn crate::backends::PaneBackend> = Arc::new(InProcessBackend::new());
        let executor = PaneExecutor::new(Arc::clone(&backend));
        let pane_config = PaneConfig::new("w1", "/tmp", BackendType::InProcess);
        let result = backend.create_pane(&pane_config).await.expect("ok");
        assert!(
            executor
                .is_teammate_running(&result.pane_id)
                .await
                .expect("ok")
        );
    }

    #[tokio::test]
    async fn pane_executor_is_not_running_after_stop() {
        let backend: Arc<dyn crate::backends::PaneBackend> = Arc::new(InProcessBackend::new());
        let executor = PaneExecutor::new(Arc::clone(&backend));
        let pane_config = PaneConfig::new("w1", "/tmp", BackendType::InProcess);
        let result = backend.create_pane(&pane_config).await.expect("ok");
        executor.stop_teammate(&result.pane_id).await.expect("ok");
        assert!(
            !executor
                .is_teammate_running(&result.pane_id)
                .await
                .expect("ok")
        );
    }

    #[tokio::test]
    async fn pane_executor_multiple_teammates() {
        let backend: Arc<dyn crate::backends::PaneBackend> = Arc::new(InProcessBackend::new());
        let executor = PaneExecutor::new(Arc::clone(&backend));

        let config1 = SpawnConfig {
            agent_id: "a1".to_owned(),
            agent_name: "w1".to_owned(),
            team_name: "team".to_owned(),
            model: None,
            cwd: "/tmp".to_owned(),
            backend_type: BackendType::InProcess,
            env_vars: vec![],
            permission_mode: None,
            worktree_path: None,
        };
        let config2 = SpawnConfig {
            agent_id: "a2".to_owned(),
            agent_name: "w2".to_owned(),
            team_name: "team".to_owned(),
            model: None,
            cwd: "/tmp".to_owned(),
            backend_type: BackendType::InProcess,
            env_vars: vec![],
            permission_mode: None,
            worktree_path: None,
        };

        let r1 = executor.start_teammate(&config1).await.expect("ok");
        let r2 = executor.start_teammate(&config2).await.expect("ok");
        assert!(r1.is_first_teammate);
        assert!(!r2.is_first_teammate);
        assert!(executor.is_teammate_running(&r1.pane_id).await.expect("ok"));
        assert!(executor.is_teammate_running(&r2.pane_id).await.expect("ok"));
    }
}
