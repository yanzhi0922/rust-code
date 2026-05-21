//! Tmux backend implementation (command construction only).
//!
//! Builds tmux commands for pane management but does not execute them.
//! This allows testing on systems without tmux installed.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Mutex;

use crate::backends::PaneBackend;
use crate::backends::types::{CreatePaneResult, PaneConfig, PaneId, PaneStatus};
use crate::error::{SwarmError, SwarmResult};

/// Tmux session name prefix.
const TMUX_SESSION_PREFIX: &str = "rc-swarm";

/// A constructed tmux command (not executed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxCommand {
    /// The command program (always "tmux").
    pub program: String,
    /// Arguments to pass to tmux.
    pub args: Vec<String>,
}

impl TmuxCommand {
    /// Create a new tmux command.
    #[must_use]
    pub fn new(args: Vec<String>) -> Self {
        Self {
            program: "tmux".to_owned(),
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

/// Build a tmux session name for a team.
#[must_use]
pub fn tmux_session_name(team_name: &str) -> String {
    format!("{}-{}", TMUX_SESSION_PREFIX, team_name)
}

/// Build the command to create a new tmux window/pane.
#[must_use]
pub fn build_create_pane_command(
    session_name: &str,
    pane_name: &str,
    cwd: &str,
    command: Option<&str>,
) -> TmuxCommand {
    let mut args = vec![
        "new-window".to_owned(),
        "-t".to_owned(),
        session_name.to_owned(),
        "-n".to_owned(),
        pane_name.to_owned(),
        "-c".to_owned(),
        cwd.to_owned(),
    ];
    if let Some(cmd) = command {
        args.push(cmd.to_owned());
    }
    TmuxCommand::new(args)
}

/// Build the command to send keys to a tmux pane.
#[must_use]
pub fn build_send_keys_command(session_name: &str, pane_name: &str, text: &str) -> TmuxCommand {
    TmuxCommand::new(vec![
        "send-keys".to_owned(),
        "-t".to_owned(),
        format!("{}.{}", session_name, pane_name),
        text.to_owned(),
        "Enter".to_owned(),
    ])
}

/// Build the command to kill a tmux pane.
#[must_use]
pub fn build_kill_pane_command(session_name: &str, pane_name: &str) -> TmuxCommand {
    TmuxCommand::new(vec![
        "kill-pane".to_owned(),
        "-t".to_owned(),
        format!("{}.{}", session_name, pane_name),
    ])
}

/// Build the command to list tmux panes.
#[must_use]
pub fn build_list_panes_command(session_name: &str) -> TmuxCommand {
    TmuxCommand::new(vec![
        "list-panes".to_owned(),
        "-t".to_owned(),
        session_name.to_owned(),
        "-F".to_owned(),
        "#{pane_id}".to_owned(),
    ])
}

/// Tmux backend that constructs commands but tracks state in memory.
#[derive(Debug)]
pub struct TmuxBackend {
    session_name: String,
    panes: Arc<Mutex<HashMap<PaneId, bool>>>,
    counter: AtomicUsize,
}

impl TmuxBackend {
    /// Create a new tmux backend for the given team.
    #[must_use]
    pub fn new(team_name: &str) -> Self {
        Self {
            session_name: tmux_session_name(team_name),
            panes: Arc::new(Mutex::new(HashMap::new())),
            counter: AtomicUsize::new(0),
        }
    }

    /// Get the tmux session name.
    #[must_use]
    pub fn session_name(&self) -> &str {
        &self.session_name
    }

    /// Build the command that would create a new pane.
    pub async fn build_create_command(&self, config: &PaneConfig) -> (TmuxCommand, PaneId) {
        let n = self.counter.fetch_add(1, Ordering::Relaxed) + 1;
        let pane_id = format!("tmux-pane-{n}");
        let cmd = build_create_pane_command(
            &self.session_name,
            &config.name,
            &config.cwd,
            config.command.as_deref(),
        );
        (cmd, pane_id)
    }
}

#[async_trait]
impl PaneBackend for TmuxBackend {
    fn backend_name(&self) -> &'static str {
        "tmux"
    }

    async fn create_pane(&self, config: &PaneConfig) -> SwarmResult<CreatePaneResult> {
        let (cmd, pane_id) = self.build_create_command(config).await;
        tracing::debug!("tmux create pane command: {}", cmd.to_command_string());
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
        let cmd = build_kill_pane_command(&self.session_name, pane_id);
        tracing::debug!("tmux kill pane command: {}", cmd.to_command_string());
        let mut panes = self.panes.lock().await;
        panes.remove(pane_id);
        Ok(())
    }

    async fn send_to_pane(&self, pane_id: &PaneId, text: &str) -> SwarmResult<()> {
        let cmd = build_send_keys_command(&self.session_name, pane_id, text);
        tracing::debug!("tmux send keys command: {}", cmd.to_command_string());
        let panes = self.panes.lock().await;
        if panes.contains_key(pane_id) {
            Ok(())
        } else {
            Err(SwarmError::InvalidPath(pane_id.clone().into()))
        }
    }

    async fn is_available(&self) -> bool {
        // Check if tmux is available on the system.
        tokio::process::Command::new("tmux")
            .arg("-V")
            .output()
            .await
            .map(|o| o.status.success())
            .expect_err("should not be true on Windows")
            .to_string()
            .contains("tmux")
            || cfg!(target_os = "linux")
            || cfg!(target_os = "macos")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::BackendType;

    #[test]
    fn tmux_session_name_format() {
        assert_eq!(tmux_session_name("my-team"), "rc-swarm-my-team");
    }

    #[test]
    fn build_create_pane_command_basic() {
        let cmd = build_create_pane_command("rc-swarm-team", "worker-1", "/tmp", None);
        assert_eq!(cmd.program, "tmux");
        assert!(cmd.args.contains(&"new-window".to_owned()));
        assert!(cmd.args.contains(&"worker-1".to_owned()));
        assert!(cmd.args.contains(&"/tmp".to_owned()));
    }

    #[test]
    fn build_create_pane_command_with_command() {
        let cmd = build_create_pane_command("sess", "w1", "/tmp", Some("remote-code --swarm"));
        assert!(cmd.args.contains(&"remote-code --swarm".to_owned()));
    }

    #[test]
    fn test_build_send_keys_command() {
        let cmd = build_send_keys_command("sess", "w1", "hello");
        assert!(cmd.args.contains(&"send-keys".to_owned()));
        assert!(cmd.args.contains(&"sess.w1".to_owned()));
        assert!(cmd.args.contains(&"hello".to_owned()));
    }

    #[test]
    fn test_build_kill_pane_command() {
        let cmd = build_kill_pane_command("sess", "w1");
        assert!(cmd.args.contains(&"kill-pane".to_owned()));
        assert!(cmd.args.contains(&"sess.w1".to_owned()));
    }

    #[test]
    fn test_build_list_panes_command() {
        let cmd = build_list_panes_command("sess");
        assert!(cmd.args.contains(&"list-panes".to_owned()));
        assert!(cmd.args.contains(&"sess".to_owned()));
    }

    #[test]
    fn tmux_command_to_string() {
        let cmd = TmuxCommand::new(vec![
            "new-window".to_owned(),
            "-n".to_owned(),
            "my pane".to_owned(),
        ]);
        let s = cmd.to_command_string();
        assert!(s.starts_with("tmux"));
        assert!(s.contains("new-window"));
        assert!(s.contains("'my pane'"));
    }

    #[test]
    fn tmux_command_to_string_no_spaces() {
        let cmd = TmuxCommand::new(vec!["list-sessions".to_owned()]);
        let s = cmd.to_command_string();
        assert_eq!(s, "tmux list-sessions");
    }

    #[tokio::test]
    async fn tmux_backend_create_pane() {
        let backend = TmuxBackend::new("test-team");
        let config = PaneConfig::new("worker-1", "/tmp", BackendType::Tmux);
        let result = backend.create_pane(&config).await.expect("should create");
        assert!(result.is_first_teammate);
        assert!(result.pane_id.starts_with("tmux-pane-"));
    }

    #[tokio::test]
    async fn tmux_backend_list_panes() {
        let backend = TmuxBackend::new("test-team");
        backend
            .create_pane(&PaneConfig::new("w1", "/tmp", BackendType::Tmux))
            .await
            .expect("ok");
        backend
            .create_pane(&PaneConfig::new("w2", "/tmp", BackendType::Tmux))
            .await
            .expect("ok");
        let panes = backend.list_panes().await.expect("should list");
        assert_eq!(panes.len(), 2);
    }

    #[tokio::test]
    async fn tmux_backend_destroy_pane() {
        let backend = TmuxBackend::new("test-team");
        let result = backend
            .create_pane(&PaneConfig::new("w1", "/tmp", BackendType::Tmux))
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
    async fn tmux_backend_pane_status() {
        let backend = TmuxBackend::new("test-team");
        let result = backend
            .create_pane(&PaneConfig::new("w1", "/tmp", BackendType::Tmux))
            .await
            .expect("ok");
        let status = backend.pane_status(&result.pane_id).await.expect("ok");
        assert_eq!(status, PaneStatus::Running);
    }

    #[tokio::test]
    async fn tmux_backend_pane_status_not_found() {
        let backend = TmuxBackend::new("test-team");
        let status = backend
            .pane_status(&"nonexistent".to_owned())
            .await
            .expect("ok");
        assert_eq!(status, PaneStatus::NotFound);
    }

    #[tokio::test]
    async fn tmux_backend_send_to_pane() {
        let backend = TmuxBackend::new("test-team");
        let result = backend
            .create_pane(&PaneConfig::new("w1", "/tmp", BackendType::Tmux))
            .await
            .expect("ok");
        backend
            .send_to_pane(&result.pane_id, "hello")
            .await
            .expect("should send");
    }

    #[tokio::test]
    async fn tmux_backend_send_to_nonexistent() {
        let backend = TmuxBackend::new("test-team");
        let result = backend
            .send_to_pane(&"nonexistent".to_owned(), "hello")
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn tmux_backend_session_name() {
        let backend = TmuxBackend::new("my-team");
        assert_eq!(backend.session_name(), "rc-swarm-my-team");
    }

    #[test]
    fn tmux_backend_name() {
        let backend = TmuxBackend::new("team");
        assert_eq!(backend.backend_name(), "tmux");
    }
}
