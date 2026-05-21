//! In-process teammate runner.
//!
//! Runs a teammate within the same process using tokio tasks.
//! This is the simplest backend — no terminal splitting required.

use std::sync::Arc;

use tokio::sync::{Mutex, mpsc};

use crate::backends::TeammateExecutor;
use crate::backends::in_process::InProcessBackend;
use crate::backends::pane_executor::PaneExecutor;
use crate::backends::types::CreatePaneResult;
use crate::error::SwarmResult;
use crate::types::{SpawnConfig, TeammateState};

/// State for an in-process teammate runner.
#[derive(Debug)]
struct RunnerState {
    /// Current state of the teammate.
    state: TeammateState,
    /// Pane ID of the teammate.
    pane_id: Option<String>,
}

/// In-process teammate runner.
///
/// Manages the lifecycle of a teammate running in-process.
pub struct InProcessRunner {
    backend: Arc<InProcessBackend>,
    executor: PaneExecutor,
    state: Arc<Mutex<RunnerState>>,
    shutdown_tx: Mutex<Option<mpsc::Sender<()>>>,
}

impl std::fmt::Debug for InProcessRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InProcessRunner")
            .field("backend", &self.backend)
            .finish_non_exhaustive()
    }
}

impl InProcessRunner {
    /// Create a new in-process runner.
    #[must_use]
    pub fn new() -> Self {
        let backend = Arc::new(InProcessBackend::new());
        let executor =
            PaneExecutor::new(Arc::clone(&backend) as Arc<dyn crate::backends::PaneBackend>);
        Self {
            backend,
            executor,
            state: Arc::new(Mutex::new(RunnerState {
                state: TeammateState::Init,
                pane_id: None,
            })),
            shutdown_tx: Mutex::new(None),
        }
    }

    /// Get the current state of the teammate.
    pub async fn state(&self) -> TeammateState {
        let state = self.state.lock().await;
        state.state
    }

    /// Start the teammate.
    pub async fn start(&self, config: &SpawnConfig) -> SwarmResult<CreatePaneResult> {
        {
            let mut state = self.state.lock().await;
            state.state = TeammateState::Spawning;
        }

        let result = self.executor.start_teammate(config).await?;

        {
            let mut state = self.state.lock().await;
            state.state = TeammateState::Running;
            state.pane_id = Some(result.pane_id.clone());
        }

        // Create a shutdown channel.
        let (tx, _rx) = mpsc::channel::<()>(1);
        {
            let mut shutdown = self.shutdown_tx.lock().await;
            *shutdown = Some(tx);
        }

        Ok(result)
    }

    /// Stop the teammate.
    pub async fn stop(&self) -> SwarmResult<()> {
        let pane_id = {
            let state = self.state.lock().await;
            state.pane_id.clone()
        };

        if let Some(ref pane_id) = pane_id {
            self.executor.stop_teammate(pane_id).await?;
        }

        // Send shutdown signal.
        {
            let mut shutdown = self.shutdown_tx.lock().await;
            if let Some(tx) = shutdown.take() {
                let _ = tx.send(()).await;
            }
        }

        {
            let mut state = self.state.lock().await;
            state.state = TeammateState::Stopped;
            state.pane_id = None;
        }

        Ok(())
    }

    /// Check if the teammate is running.
    pub async fn is_running(&self) -> bool {
        let state = self.state.lock().await;
        state.state == TeammateState::Running
    }

    /// Get the pane ID.
    pub async fn pane_id(&self) -> Option<String> {
        let state = self.state.lock().await;
        state.pane_id.clone()
    }

    /// Get a reference to the underlying backend.
    #[must_use]
    pub fn backend(&self) -> &Arc<InProcessBackend> {
        &self.backend
    }
}

impl Default for InProcessRunner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::PaneBackend;
    use crate::types::BackendType;

    fn test_config() -> SpawnConfig {
        SpawnConfig {
            agent_id: "a1".to_owned(),
            agent_name: "worker-1".to_owned(),
            team_name: "test-team".to_owned(),
            model: None,
            cwd: "/tmp".to_owned(),
            backend_type: BackendType::InProcess,
            env_vars: vec![],
            permission_mode: None,
            worktree_path: None,
        }
    }

    #[tokio::test]
    async fn runner_start() {
        let runner = InProcessRunner::new();
        let config = test_config();
        let result = runner.start(&config).await.expect("should start");
        assert!(result.is_first_teammate);
        assert!(runner.is_running().await);
        assert!(runner.pane_id().await.is_some());
    }

    #[tokio::test]
    async fn runner_stop() {
        let runner = InProcessRunner::new();
        let config = test_config();
        runner.start(&config).await.expect("should start");
        runner.stop().await.expect("should stop");
        assert!(!runner.is_running().await);
        assert!(runner.pane_id().await.is_none());
    }

    #[tokio::test]
    async fn runner_state_transitions() {
        let runner = InProcessRunner::new();
        assert_eq!(runner.state().await, TeammateState::Init);

        let config = test_config();
        runner.start(&config).await.expect("should start");
        assert_eq!(runner.state().await, TeammateState::Running);

        runner.stop().await.expect("should stop");
        assert_eq!(runner.state().await, TeammateState::Stopped);
    }

    #[tokio::test]
    async fn runner_multiple_starts() {
        let runner = InProcessRunner::new();
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
            ..config1.clone()
        };

        let r1 = runner.start(&config1).await.expect("ok");
        assert!(r1.is_first_teammate);

        let r2 = runner.start(&config2).await.expect("ok");
        assert!(!r2.is_first_teammate);
    }

    #[tokio::test]
    async fn runner_default() {
        let runner = InProcessRunner::default();
        assert_eq!(runner.state().await, TeammateState::Init);
    }

    #[tokio::test]
    async fn runner_backend() {
        let runner = InProcessRunner::new();
        assert_eq!(runner.backend().backend_name(), "in_process");
    }

    #[tokio::test]
    async fn runner_stop_without_start() {
        let runner = InProcessRunner::new();
        runner.stop().await.expect("should handle gracefully");
        assert!(!runner.is_running().await);
    }
}
