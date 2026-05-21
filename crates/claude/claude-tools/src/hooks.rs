//! Hook execution integration — bridges the new hook system with the tool runtime.
//!
//! Provides high-level functions for executing hooks at various lifecycle points,
//! integrating the `claude_core` hook types, matcher, executor, and registry.

use std::collections::HashMap;

use anyhow::{Context, Result};
use claude_core::HookShell;
use claude_core::hook_executor::{HookBatchResult, HookExecutor};
use claude_core::hook_matcher::match_hooks;
use claude_core::hook_registry::HookRegistry;
use claude_core::hook_types::{HookInput, HookMatcherEntry};
use claude_core::hooks::HookEventKind;

use std::process::Stdio;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

// ── Legacy command hook execution ────────────────────────────────────────

use super::{CommandHookExecutionRequest, CommandHookExecutionResult};

/// Execute a legacy command hook (backward compatible).
pub async fn execute_command_hook(
    request: &CommandHookExecutionRequest,
) -> Result<CommandHookExecutionResult> {
    let shell = request.shell.unwrap_or_else(default_hook_shell);
    let timeout_secs = request.timeout_secs.unwrap_or(15).max(1);
    let mut process = build_shell_command(shell, &request.command);
    process.current_dir(&request.cwd);
    process.stdin(Stdio::piped());
    process.stdout(Stdio::piped());
    process.stderr(Stdio::piped());

    let mut child = process.spawn().context("failed to spawn command hook")?;
    if let Some(mut stdin) = child.stdin.take() {
        let input = serde_json::to_vec(&request.input)?;
        tokio::spawn(async move {
            let _ = tokio::io::AsyncWriteExt::write_all(&mut stdin, &input).await;
        });
    }

    let future = async {
        let mut stdout = String::new();
        let mut stderr = String::new();
        if let Some(mut stream) = child.stdout.take() {
            let _ = stream.read_to_string(&mut stdout).await;
        }
        if let Some(mut stream) = child.stderr.take() {
            let _ = stream.read_to_string(&mut stderr).await;
        }
        let status = child.wait().await?;
        Ok::<_, anyhow::Error>((status.code(), stdout, stderr))
    };

    let (exit_code, stdout, stderr) =
        tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), future)
            .await
            .map_err(|_| anyhow::anyhow!("command hook timed out after {timeout_secs}s"))??;

    Ok(CommandHookExecutionResult {
        event: request.event,
        command: request.command.clone(),
        shell,
        exit_code,
        stdout,
        stderr,
    })
}

pub(crate) fn build_shell_command(shell: HookShell, command: &str) -> Command {
    match shell {
        HookShell::PowerShell => {
            let mut cmd = Command::new("powershell");
            cmd.args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                command,
            ]);
            cmd
        }
        HookShell::Bash => {
            #[cfg(windows)]
            {
                let mut cmd = Command::new("bash");
                cmd.args(["-lc", command]);
                cmd
            }
            #[cfg(not(windows))]
            {
                let mut cmd = Command::new("sh");
                cmd.args(["-lc", command]);
                cmd
            }
        }
    }
}

pub(crate) fn default_hook_shell() -> HookShell {
    if cfg!(windows) {
        HookShell::PowerShell
    } else {
        HookShell::Bash
    }
}

// ── New hook execution integration ──────────────────────────────────────

/// Context for hook execution at various lifecycle points.
pub struct HookExecutionContext {
    /// The hook registry.
    pub registry: HookRegistry,
    /// The hook executor.
    pub executor: HookExecutor,
}

impl HookExecutionContext {
    /// Create a new context with the given working directory.
    #[must_use]
    pub fn new(cwd: String) -> Self {
        Self {
            registry: HookRegistry::new(),
            executor: HookExecutor::new(cwd),
        }
    }

    /// Create a new context with custom timeout.
    #[must_use]
    pub fn with_timeout(cwd: String, timeout_secs: u64) -> Self {
        Self {
            registry: HookRegistry::new(),
            executor: HookExecutor::new(cwd).with_timeout(timeout_secs),
        }
    }

    /// Execute hooks for a given event.
    ///
    /// Looks up registered hooks, matches them against the tool name,
    /// and executes them using the executor.
    pub async fn execute_event_hooks(
        &self,
        event: HookEventKind,
        tool_name: Option<&str>,
        tool_input: Option<&serde_json::Value>,
        session_id: Option<&str>,
        cwd: Option<&str>,
    ) -> HookBatchResult {
        let matchers = self.registry.get_hooks_for_event(event);
        let matched = match_hooks(matchers.as_slice(), tool_name, tool_name, tool_input);

        let input = HookInput {
            event,
            tool_name: tool_name.map(String::from),
            tool_input: tool_input.cloned(),
            session_id: session_id.map(String::from),
            cwd: cwd.map(String::from),
            user_prompt: None,
            tool_use_id: None,
            tool_result: None,
        };

        self.executor.execute_hooks(&matched.hooks, &input).await
    }

    /// Execute PreToolUse hooks.
    pub async fn execute_pre_tool_hooks(
        &self,
        tool_name: &str,
        tool_input: &serde_json::Value,
        session_id: Option<&str>,
        cwd: &str,
    ) -> HookBatchResult {
        self.execute_event_hooks(
            HookEventKind::PreToolUse,
            Some(tool_name),
            Some(tool_input),
            session_id,
            Some(cwd),
        )
        .await
    }

    /// Execute PostToolUse hooks.
    pub async fn execute_post_tool_hooks(
        &self,
        tool_name: &str,
        tool_input: &serde_json::Value,
        session_id: Option<&str>,
        cwd: &str,
    ) -> HookBatchResult {
        self.execute_event_hooks(
            HookEventKind::PostToolUse,
            Some(tool_name),
            Some(tool_input),
            session_id,
            Some(cwd),
        )
        .await
    }

    /// Execute Stop hooks.
    pub async fn execute_stop_hooks(&self, session_id: Option<&str>, cwd: &str) -> HookBatchResult {
        self.execute_event_hooks(HookEventKind::Stop, None, None, session_id, Some(cwd))
            .await
    }

    /// Execute SessionStart hooks.
    pub async fn execute_session_start_hooks(
        &self,
        session_id: Option<&str>,
        cwd: &str,
    ) -> HookBatchResult {
        self.execute_event_hooks(
            HookEventKind::SessionStart,
            None,
            None,
            session_id,
            Some(cwd),
        )
        .await
    }

    /// Execute SessionEnd hooks.
    pub async fn execute_session_end_hooks(
        &self,
        session_id: Option<&str>,
        cwd: &str,
    ) -> HookBatchResult {
        self.execute_event_hooks(HookEventKind::SessionEnd, None, None, session_id, Some(cwd))
            .await
    }

    /// Execute Notification hooks.
    pub async fn execute_notification_hooks(
        &self,
        session_id: Option<&str>,
        cwd: &str,
    ) -> HookBatchResult {
        self.execute_event_hooks(
            HookEventKind::Notification,
            None,
            None,
            session_id,
            Some(cwd),
        )
        .await
    }

    /// Execute PreCompact hooks.
    pub async fn execute_pre_compact_hooks(
        &self,
        session_id: Option<&str>,
        cwd: &str,
    ) -> HookBatchResult {
        self.execute_event_hooks(HookEventKind::PreCompact, None, None, session_id, Some(cwd))
            .await
    }

    /// Execute PostCompact hooks.
    pub async fn execute_post_compact_hooks(
        &self,
        session_id: Option<&str>,
        cwd: &str,
    ) -> HookBatchResult {
        self.execute_event_hooks(
            HookEventKind::PostCompact,
            None,
            None,
            session_id,
            Some(cwd),
        )
        .await
    }

    /// Load hooks from a settings map into the registry.
    pub fn load_from_settings(&mut self, settings: &HashMap<String, Vec<HookMatcherEntry>>) {
        self.registry.register_from_settings(settings);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use claude_core::hook_types::{HookCommand, HookDefinition, HookMatcherEntry};

    fn make_command_hook(cmd: &str) -> HookDefinition {
        HookDefinition::Command(HookCommand {
            command: cmd.to_string(),
            shell: None,
            timeout: Some(5),
            if_condition: None,
            status_message: None,
            once: false,
            r#async: false,
            async_rewake: false,
        })
    }

    fn make_matcher(pattern: Option<&str>, cmds: &[&str]) -> HookMatcherEntry {
        HookMatcherEntry {
            matcher: pattern.map(String::from),
            hooks: cmds.iter().map(|c| make_command_hook(c)).collect(),
        }
    }

    // ── Legacy execution tests ───────────────────────────────────────────

    #[test]
    fn default_hook_shell_platform() {
        let shell = default_hook_shell();
        if cfg!(windows) {
            assert_eq!(shell, HookShell::PowerShell);
        } else {
            assert_eq!(shell, HookShell::Bash);
        }
    }

    #[tokio::test]
    async fn execute_command_hook_success() {
        let request = CommandHookExecutionRequest {
            event: claude_core::HookEvent::SessionStart,
            command: "echo hello".to_string(),
            cwd: std::env::temp_dir(),
            input: serde_json::json!({}),
            shell: None,
            timeout_secs: Some(5),
        };
        let result = execute_command_hook(&request).await;
        assert!(result.is_ok());
        let result = result.expect("result");
        assert!(result.exit_code == Some(0));
        assert!(result.stdout.contains("hello"));
    }

    #[tokio::test]
    async fn execute_command_hook_timeout() {
        let request = CommandHookExecutionRequest {
            event: claude_core::HookEvent::SessionStart,
            command: "sleep 60".to_string(),
            cwd: std::env::temp_dir(),
            input: serde_json::json!({}),
            shell: None,
            timeout_secs: Some(1),
        };
        let result = execute_command_hook(&request).await;
        assert!(result.is_err());
    }

    // ── HookExecutionContext tests ────────────────────────────────────────

    #[test]
    fn context_new() {
        let ctx = HookExecutionContext::new("/tmp".to_string());
        assert_eq!(ctx.executor.cwd, "/tmp");
        assert!(!ctx.registry.has_any_hooks());
    }

    #[test]
    fn context_with_timeout() {
        let ctx = HookExecutionContext::with_timeout("/tmp".to_string(), 60);
        assert_eq!(ctx.executor.default_timeout_secs, 60);
    }

    #[tokio::test]
    async fn execute_event_hooks_empty_registry() {
        let ctx = HookExecutionContext::new("/tmp".to_string());
        let result = ctx
            .execute_event_hooks(HookEventKind::PreToolUse, None, None, None, None)
            .await;
        assert!(!result.is_blocked());
        assert!(result.outcomes.is_empty());
    }

    #[tokio::test]
    async fn execute_pre_tool_hooks_matching() {
        let cwd = std::env::temp_dir().to_string_lossy().to_string();
        let mut ctx = HookExecutionContext::new(cwd.clone());
        ctx.registry.register_hooks(
            HookEventKind::PreToolUse,
            vec![make_matcher(Some("Bash"), &["echo pre-tool"])],
        );
        let result = ctx
            .execute_pre_tool_hooks("Bash", &serde_json::json!({}), None, &cwd)
            .await;
        assert_eq!(result.outcomes.len(), 1);
        assert!(result.outcomes[0].success);
    }

    #[tokio::test]
    async fn execute_pre_tool_hooks_non_matching() {
        let cwd = std::env::temp_dir().to_string_lossy().to_string();
        let mut ctx = HookExecutionContext::new(cwd.clone());
        ctx.registry.register_hooks(
            HookEventKind::PreToolUse,
            vec![make_matcher(Some("Write"), &["echo pre-tool"])],
        );
        let result = ctx
            .execute_pre_tool_hooks("Bash", &serde_json::json!({}), None, &cwd)
            .await;
        assert!(result.outcomes.is_empty());
    }

    #[tokio::test]
    async fn execute_session_start_hooks() {
        let cwd = std::env::temp_dir().to_string_lossy().to_string();
        let mut ctx = HookExecutionContext::new(cwd.clone());
        ctx.registry.register_hooks(
            HookEventKind::SessionStart,
            vec![make_matcher(None, &["echo session-start"])],
        );
        let result = ctx.execute_session_start_hooks(None, &cwd).await;
        assert_eq!(result.outcomes.len(), 1);
    }

    #[tokio::test]
    async fn execute_stop_hooks() {
        let cwd = std::env::temp_dir().to_string_lossy().to_string();
        let mut ctx = HookExecutionContext::new(cwd.clone());
        ctx.registry.register_hooks(
            HookEventKind::Stop,
            vec![make_matcher(None, &["echo stop"])],
        );
        let result = ctx.execute_stop_hooks(None, &cwd).await;
        assert_eq!(result.outcomes.len(), 1);
    }

    #[tokio::test]
    async fn execute_notification_hooks() {
        let cwd = std::env::temp_dir().to_string_lossy().to_string();
        let mut ctx = HookExecutionContext::new(cwd.clone());
        ctx.registry.register_hooks(
            HookEventKind::Notification,
            vec![make_matcher(None, &["echo notify"])],
        );
        let result = ctx.execute_notification_hooks(None, &cwd).await;
        assert_eq!(result.outcomes.len(), 1);
    }

    #[tokio::test]
    async fn load_from_settings() {
        let mut ctx = HookExecutionContext::new("/tmp".to_string());
        let mut settings = HashMap::new();
        settings.insert(
            "PreToolUse".to_string(),
            vec![make_matcher(Some("Bash"), &["echo loaded"])],
        );
        ctx.load_from_settings(&settings);
        assert!(ctx.registry.has_hooks_for_event(HookEventKind::PreToolUse));
    }
}
