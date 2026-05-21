//! In-process backend implementation.
//!
//! Runs teammates within the same process using tokio tasks.
//! No terminal splitting is required — all communication happens
//! through in-memory channels.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Mutex;

use crate::backends::types::{CreatePaneResult, PaneConfig, PaneId, PaneStatus};
use crate::backends::{PaneBackend, TeammateExecutor};
use crate::error::{SwarmError, SwarmResult};
use crate::types::SpawnConfig;

/// State tracked for each in-process pane.
#[derive(Debug, Clone)]
struct PaneState {
    /// Whether the pane is currently running.
    running: bool,
    /// The name of the pane.
    #[allow(dead_code)]
    name: String,
    /// The working directory.
    #[allow(dead_code)]
    cwd: String,
}

/// In-process backend that manages panes as in-memory state.
#[derive(Debug)]
pub struct InProcessBackend {
    panes: Arc<Mutex<HashMap<PaneId, PaneState>>>,
    counter: AtomicUsize,
}

impl InProcessBackend {
    /// Create a new in-process backend.
    #[must_use]
    pub fn new() -> Self {
        Self {
            panes: Arc::new(Mutex::new(HashMap::new())),
            counter: AtomicUsize::new(0),
        }
    }

    /// Generate a unique pane ID.
    fn next_pane_id(&self) -> PaneId {
        let n = self.counter.fetch_add(1, Ordering::Relaxed) + 1;
        format!("in-process-pane-{n}")
    }
}

impl Default for InProcessBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PaneBackend for InProcessBackend {
    fn backend_name(&self) -> &'static str {
        "in_process"
    }

    async fn create_pane(&self, config: &PaneConfig) -> SwarmResult<CreatePaneResult> {
        let pane_id = self.next_pane_id();
        let is_first = {
            let panes = self.panes.lock().await;
            panes.is_empty()
        };
        let state = PaneState {
            running: true,
            name: config.name.clone(),
            cwd: config.cwd.clone(),
        };
        {
            let mut panes = self.panes.lock().await;
            panes.insert(pane_id.clone(), state);
        }
        Ok(CreatePaneResult::new(pane_id, is_first))
    }

    async fn pane_status(&self, pane_id: &PaneId) -> SwarmResult<PaneStatus> {
        let panes = self.panes.lock().await;
        match panes.get(pane_id) {
            Some(state) => {
                if state.running {
                    Ok(PaneStatus::Running)
                } else {
                    Ok(PaneStatus::Exited)
                }
            }
            None => Ok(PaneStatus::NotFound),
        }
    }

    async fn list_panes(&self) -> SwarmResult<Vec<PaneId>> {
        let panes = self.panes.lock().await;
        Ok(panes.keys().cloned().collect())
    }

    async fn destroy_pane(&self, pane_id: &PaneId) -> SwarmResult<()> {
        let mut panes = self.panes.lock().await;
        match panes.remove(pane_id) {
            Some(_) => Ok(()),
            None => Err(SwarmError::InvalidPath(pane_id.clone().into())),
        }
    }

    async fn send_to_pane(&self, pane_id: &PaneId, text: &str) -> SwarmResult<()> {
        let panes = self.panes.lock().await;
        if panes.contains_key(pane_id) {
            // In-process panes do not have a stdin channel. Log the input
            // and return an error so callers know the operation is unsupported.
            tracing::warn!(
                pane_id = %pane_id,
                input_len = text.len(),
                "send_to_pane called on in-process pane; input delivery is not supported. \
                 Use channel-based communication for in-process teammates."
            );
            Err(SwarmError::InvalidPath(
                format!(
                    "in-process pane {pane_id} does not support send_to_pane; \
                     use channel-based communication instead"
                )
                .into(),
            ))
        } else {
            Err(SwarmError::InvalidPath(pane_id.clone().into()))
        }
    }

    async fn is_available(&self) -> bool {
        true
    }
}

/// In-process teammate executor.
#[derive(Debug)]
pub struct InProcessExecutor {
    backend: Arc<InProcessBackend>,
}

impl InProcessExecutor {
    /// Create a new in-process executor wrapping the given backend.
    #[must_use]
    pub fn new(backend: Arc<InProcessBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl TeammateExecutor for InProcessExecutor {
    async fn start_teammate(&self, config: &SpawnConfig) -> SwarmResult<CreatePaneResult> {
        let pane_config = PaneConfig::new(&config.agent_name, &config.cwd, config.backend_type)
            .with_command("in-process-teammate");
        self.backend.create_pane(&pane_config).await
    }

    async fn stop_teammate(&self, pane_id: &PaneId) -> SwarmResult<()> {
        self.backend.destroy_pane(pane_id).await
    }

    async fn is_teammate_running(&self, pane_id: &PaneId) -> SwarmResult<bool> {
        let status = self.backend.pane_status(pane_id).await?;
        Ok(status == PaneStatus::Running)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::BackendType;

    #[tokio::test]
    async fn create_pane_first() {
        let backend = InProcessBackend::new();
        let config = PaneConfig::new("worker-1", "/tmp", BackendType::InProcess);
        let result = backend
            .create_pane(&config)
            .await
            .expect("should create pane");
        assert!(result.is_first_teammate);
        assert!(result.pane_id.starts_with("in-process-pane-"));
    }

    #[tokio::test]
    async fn create_pane_second_not_first() {
        let backend = InProcessBackend::new();
        let config1 = PaneConfig::new("w1", "/tmp", BackendType::InProcess);
        let config2 = PaneConfig::new("w2", "/tmp", BackendType::InProcess);
        backend.create_pane(&config1).await.expect("should create");
        let result2 = backend.create_pane(&config2).await.expect("should create");
        assert!(!result2.is_first_teammate);
    }

    #[tokio::test]
    async fn pane_status_running() {
        let backend = InProcessBackend::new();
        let config = PaneConfig::new("w1", "/tmp", BackendType::InProcess);
        let result = backend.create_pane(&config).await.expect("should create");
        let status = backend
            .pane_status(&result.pane_id)
            .await
            .expect("should get status");
        assert_eq!(status, PaneStatus::Running);
    }

    #[tokio::test]
    async fn pane_status_not_found() {
        let backend = InProcessBackend::new();
        let status = backend
            .pane_status(&"nonexistent".to_owned())
            .await
            .expect("should get status");
        assert_eq!(status, PaneStatus::NotFound);
    }

    #[tokio::test]
    async fn list_panes_empty() {
        let backend = InProcessBackend::new();
        let panes = backend.list_panes().await.expect("should list");
        assert!(panes.is_empty());
    }

    #[tokio::test]
    async fn list_panes_after_create() {
        let backend = InProcessBackend::new();
        backend
            .create_pane(&PaneConfig::new("w1", "/tmp", BackendType::InProcess))
            .await
            .expect("ok");
        backend
            .create_pane(&PaneConfig::new("w2", "/tmp", BackendType::InProcess))
            .await
            .expect("ok");
        let panes = backend.list_panes().await.expect("should list");
        assert_eq!(panes.len(), 2);
    }

    #[tokio::test]
    async fn destroy_pane() {
        let backend = InProcessBackend::new();
        let result = backend
            .create_pane(&PaneConfig::new("w1", "/tmp", BackendType::InProcess))
            .await
            .expect("ok");
        backend
            .destroy_pane(&result.pane_id)
            .await
            .expect("should destroy");
        let status = backend.pane_status(&result.pane_id).await.expect("ok");
        assert_eq!(status, PaneStatus::NotFound);
    }

    #[tokio::test]
    async fn destroy_nonexistent_pane() {
        let backend = InProcessBackend::new();
        let result = backend.destroy_pane(&"nonexistent".to_owned()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn send_to_pane_returns_unsupported_error() {
        let backend = InProcessBackend::new();
        let result = backend
            .create_pane(&PaneConfig::new("w1", "/tmp", BackendType::InProcess))
            .await
            .expect("ok");
        let err = backend
            .send_to_pane(&result.pane_id, "hello")
            .await
            .expect_err("send_to_pane should return error for in-process panes");
        assert!(
            err.to_string().contains("does not support send_to_pane"),
            "error should explain in-process panes don't support send_to_pane: {err}"
        );
    }

    #[tokio::test]
    async fn send_to_nonexistent_pane() {
        let backend = InProcessBackend::new();
        let result = backend
            .send_to_pane(&"nonexistent".to_owned(), "hello")
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn is_available() {
        let backend = InProcessBackend::new();
        assert!(backend.is_available().await);
    }

    #[tokio::test]
    async fn backend_name() {
        let backend = InProcessBackend::new();
        assert_eq!(backend.backend_name(), "in_process");
    }

    #[tokio::test]
    async fn executor_start_teammate() {
        let backend = Arc::new(InProcessBackend::new());
        let executor = InProcessExecutor::new(backend);
        let config = SpawnConfig {
            agent_id: "a1".to_owned(),
            agent_name: "worker-1".to_owned(),
            team_name: "team-1".to_owned(),
            model: None,
            cwd: "/tmp".to_owned(),
            backend_type: BackendType::InProcess,
            env_vars: vec![],
            permission_mode: None,
            worktree_path: None,
        };
        let result = executor
            .start_teammate(&config)
            .await
            .expect("should start");
        assert!(result.is_first_teammate);
    }

    #[tokio::test]
    async fn executor_stop_teammate() {
        let backend = Arc::new(InProcessBackend::new());
        let executor = InProcessExecutor::new(Arc::clone(&backend));
        let config = PaneConfig::new("w1", "/tmp", BackendType::InProcess);
        let result = backend.create_pane(&config).await.expect("ok");
        executor
            .stop_teammate(&result.pane_id)
            .await
            .expect("should stop");
        assert!(
            !executor
                .is_teammate_running(&result.pane_id)
                .await
                .expect("ok")
        );
    }

    #[tokio::test]
    async fn executor_is_teammate_running() {
        let backend = Arc::new(InProcessBackend::new());
        let executor = InProcessExecutor::new(Arc::clone(&backend));
        let config = PaneConfig::new("w1", "/tmp", BackendType::InProcess);
        let result = backend.create_pane(&config).await.expect("ok");
        assert!(
            executor
                .is_teammate_running(&result.pane_id)
                .await
                .expect("ok")
        );
    }

    #[tokio::test]
    async fn default_backend() {
        let backend = InProcessBackend::default();
        assert_eq!(backend.backend_name(), "in_process");
    }
}
