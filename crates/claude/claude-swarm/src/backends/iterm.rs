//! iTerm2 backend implementation (command construction only).
//!
//! Builds iTerm2 `it2` commands for pane management but does not execute them.
//! This allows testing on systems without iTerm2 installed.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Mutex;

use crate::backends::PaneBackend;
use crate::backends::types::{CreatePaneResult, PaneConfig, PaneId, PaneStatus};
use crate::error::{SwarmError, SwarmResult};

/// A constructed iTerm2 command (not executed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItermCommand {
    /// The command program (always "it2").
    pub program: String,
    /// Arguments to pass to it2.
    pub args: Vec<String>,
}

impl ItermCommand {
    /// Create a new iTerm2 command.
    #[must_use]
    pub fn new(args: Vec<String>) -> Self {
        Self {
            program: "it2".to_owned(),
            args,
        }
    }

    /// Render the command as a string for display/logging.
    #[must_use]
    pub fn to_command_string(&self) -> String {
        let mut s = self.program.clone();
        for arg in &self.args {
            s.push(' ');
            if arg.contains(' ') || arg.contains('"') {
                s.push_str(&format!("'{}'", arg));
            } else {
                s.push_str(arg);
            }
        }
        s
    }
}

/// Build the command to create a new iTerm2 split/pane.
#[must_use]
pub fn build_create_split_command(
    pane_name: &str,
    cwd: &str,
    command: Option<&str>,
) -> ItermCommand {
    let mut args = vec![
        "split-pane".to_owned(),
        "--title".to_owned(),
        pane_name.to_owned(),
        "--cwd".to_owned(),
        cwd.to_owned(),
    ];
    if let Some(cmd) = command {
        args.push(cmd.to_owned());
    }
    ItermCommand::new(args)
}

/// Build the command to send text to an iTerm2 pane.
#[must_use]
pub fn build_send_text_command(session_id: &str, text: &str) -> ItermCommand {
    ItermCommand::new(vec![
        "send-text".to_owned(),
        "--session-id".to_owned(),
        session_id.to_owned(),
        text.to_owned(),
    ])
}

/// Build the command to close an iTerm2 pane.
#[must_use]
pub fn build_close_pane_command(session_id: &str) -> ItermCommand {
    ItermCommand::new(vec![
        "close-pane".to_owned(),
        "--session-id".to_owned(),
        session_id.to_owned(),
    ])
}

/// Build the command to list iTerm2 sessions.
#[must_use]
pub fn build_list_sessions_command() -> ItermCommand {
    ItermCommand::new(vec!["list-sessions".to_owned()])
}

/// iTerm2 backend that constructs commands but tracks state in memory.
#[derive(Debug)]
pub struct ItermBackend {
    panes: Arc<Mutex<HashMap<PaneId, bool>>>,
    counter: AtomicUsize,
}

impl ItermBackend {
    /// Create a new iTerm2 backend.
    #[must_use]
    pub fn new() -> Self {
        Self {
            panes: Arc::new(Mutex::new(HashMap::new())),
            counter: AtomicUsize::new(0),
        }
    }
}

impl Default for ItermBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PaneBackend for ItermBackend {
    fn backend_name(&self) -> &'static str {
        "iterm2"
    }

    async fn create_pane(&self, config: &PaneConfig) -> SwarmResult<CreatePaneResult> {
        let cmd = build_create_split_command(&config.name, &config.cwd, config.command.as_deref());
        tracing::debug!("iterm2 create pane command: {}", cmd.to_command_string());

        let n = self.counter.fetch_add(1, Ordering::Relaxed) + 1;
        let pane_id = format!("iterm-pane-{n}");
        let is_first = {
            let panes = self.panes.lock().await;
            panes.is_empty()
        };
        {
            let mut panes = self.panes.lock().await;
            panes.insert(pane_id.clone(), true);
        }
        Ok(CreatePaneResult::new(pane_id, is_first))
    }

    async fn pane_status(&self, pane_id: &PaneId) -> SwarmResult<PaneStatus> {
        let panes = self.panes.lock().await;
        match panes.get(pane_id) {
            Some(true) => Ok(PaneStatus::Running),
            Some(false) => Ok(PaneStatus::Exited),
            None => Ok(PaneStatus::NotFound),
        }
    }

    async fn list_panes(&self) -> SwarmResult<Vec<PaneId>> {
        let panes = self.panes.lock().await;
        Ok(panes.keys().cloned().collect())
    }

    async fn destroy_pane(&self, pane_id: &PaneId) -> SwarmResult<()> {
        let cmd = build_close_pane_command(pane_id);
        tracing::debug!("iterm2 close pane command: {}", cmd.to_command_string());
        let mut panes = self.panes.lock().await;
        panes.remove(pane_id);
        Ok(())
    }

    async fn send_to_pane(&self, pane_id: &PaneId, text: &str) -> SwarmResult<()> {
        let cmd = build_send_text_command(pane_id, text);
        tracing::debug!("iterm2 send text command: {}", cmd.to_command_string());
        let panes = self.panes.lock().await;
        if panes.contains_key(pane_id) {
            Ok(())
        } else {
            Err(SwarmError::InvalidPath(pane_id.clone().into()))
        }
    }

    async fn is_available(&self) -> bool {
        // iTerm2 is only available on macOS.
        cfg!(target_os = "macos")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::BackendType;

    #[test]
    fn build_create_split_command_basic() {
        let cmd = build_create_split_command("worker-1", "/tmp", None);
        assert_eq!(cmd.program, "it2");
        assert!(cmd.args.contains(&"split-pane".to_owned()));
        assert!(cmd.args.contains(&"worker-1".to_owned()));
    }

    #[test]
    fn build_create_split_command_with_command() {
        let cmd = build_create_split_command("w1", "/tmp", Some("remote-code --swarm"));
        assert!(cmd.args.contains(&"remote-code --swarm".to_owned()));
    }

    #[test]
    fn test_build_send_text_command() {
        let cmd = build_send_text_command("sess-1", "hello world");
        assert!(cmd.args.contains(&"send-text".to_owned()));
        assert!(cmd.args.contains(&"sess-1".to_owned()));
        assert!(cmd.args.contains(&"hello world".to_owned()));
    }

    #[test]
    fn test_build_close_pane_command() {
        let cmd = build_close_pane_command("sess-1");
        assert!(cmd.args.contains(&"close-pane".to_owned()));
        assert!(cmd.args.contains(&"sess-1".to_owned()));
    }

    #[test]
    fn test_build_list_sessions_command() {
        let cmd = build_list_sessions_command();
        assert!(cmd.args.contains(&"list-sessions".to_owned()));
    }

    #[test]
    fn iterm_command_to_string() {
        let cmd = ItermCommand::new(vec![
            "split-pane".to_owned(),
            "--title".to_owned(),
            "my pane".to_owned(),
        ]);
        let s = cmd.to_command_string();
        assert!(s.starts_with("it2"));
        assert!(s.contains("split-pane"));
        assert!(s.contains("'my pane'"));
    }

    #[tokio::test]
    async fn iterm_backend_create_pane() {
        let backend = ItermBackend::new();
        let config = PaneConfig::new("worker-1", "/tmp", BackendType::ITerm2);
        let result = backend.create_pane(&config).await.expect("should create");
        assert!(result.is_first_teammate);
        assert!(result.pane_id.starts_with("iterm-pane-"));
    }

    #[tokio::test]
    async fn iterm_backend_second_pane() {
        let backend = ItermBackend::new();
        backend
            .create_pane(&PaneConfig::new("w1", "/tmp", BackendType::ITerm2))
            .await
            .expect("ok");
        let result = backend
            .create_pane(&PaneConfig::new("w2", "/tmp", BackendType::ITerm2))
            .await
            .expect("ok");
        assert!(!result.is_first_teammate);
    }

    #[tokio::test]
    async fn iterm_backend_list_panes() {
        let backend = ItermBackend::new();
        backend
            .create_pane(&PaneConfig::new("w1", "/tmp", BackendType::ITerm2))
            .await
            .expect("ok");
        backend
            .create_pane(&PaneConfig::new("w2", "/tmp", BackendType::ITerm2))
            .await
            .expect("ok");
        let panes = backend.list_panes().await.expect("should list");
        assert_eq!(panes.len(), 2);
    }

    #[tokio::test]
    async fn iterm_backend_destroy_pane() {
        let backend = ItermBackend::new();
        let result = backend
            .create_pane(&PaneConfig::new("w1", "/tmp", BackendType::ITerm2))
            .await
            .expect("ok");
        backend
            .destroy_pane(&result.pane_id)
            .await
            .expect("should destroy");
        let panes = backend.list_panes().await.expect("ok");
        assert!(panes.is_empty());
    }

    #[tokio::test]
    async fn iterm_backend_pane_status() {
        let backend = ItermBackend::new();
        let result = backend
            .create_pane(&PaneConfig::new("w1", "/tmp", BackendType::ITerm2))
            .await
            .expect("ok");
        let status = backend.pane_status(&result.pane_id).await.expect("ok");
        assert_eq!(status, PaneStatus::Running);
    }

    #[tokio::test]
    async fn iterm_backend_pane_status_not_found() {
        let backend = ItermBackend::new();
        let status = backend
            .pane_status(&"nonexistent".to_owned())
            .await
            .expect("ok");
        assert_eq!(status, PaneStatus::NotFound);
    }

    #[tokio::test]
    async fn iterm_backend_send_to_pane() {
        let backend = ItermBackend::new();
        let result = backend
            .create_pane(&PaneConfig::new("w1", "/tmp", BackendType::ITerm2))
            .await
            .expect("ok");
        backend
            .send_to_pane(&result.pane_id, "hello")
            .await
            .expect("should send");
    }

    #[tokio::test]
    async fn iterm_backend_send_to_nonexistent() {
        let backend = ItermBackend::new();
        let result = backend
            .send_to_pane(&"nonexistent".to_owned(), "hello")
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn iterm_backend_name() {
        let backend = ItermBackend::new();
        assert_eq!(backend.backend_name(), "iterm2");
    }

    #[test]
    fn iterm_backend_default() {
        let backend = ItermBackend::default();
        assert_eq!(backend.backend_name(), "iterm2");
    }
}
