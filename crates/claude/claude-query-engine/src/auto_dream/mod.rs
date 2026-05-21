//! Auto-dream — background memory consolidation agent.
//!
//! Mirrors TS `autoDream.ts` with a 5-gate execution chain:
//! 1. Feature toggle (enabled + not remote + auto-memory on)
//! 2. Time gate (min hours since last consolidation)
//! 3. Scan throttle (min minutes since last directory scan)
//! 4. Session gate (enough sessions accumulated)
//! 5. Lock gate (no other process currently dreaming)
//!
//! If all gates pass, launches a forked sub-agent with the 4-phase
//! consolidation prompt (orient → gather → consolidate → prune).
//!
//! This runs as Phase 3 (BackgroundFireAndForget) of the stop-hook pipeline.

pub mod config;
pub mod consolidation_lock;
pub mod consolidation_prompt;

use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use config::AutoDreamConfig;
use consolidation_lock::ConsolidationLock;
use consolidation_prompt::AUTO_DREAM_SYSTEM_PROMPT;

/// Shared state for auto-dream gating across invocations.
#[derive(Debug, Default)]
pub struct AutoDreamState {
    /// Timestamp of last directory scan (ms since epoch).
    last_session_scan_at: Option<u128>,
    /// Number of sessions found in last scan.
    last_session_count: usize,
}

/// Auto-dream executor that manages the 5-gate chain.
#[derive(Debug)]
pub struct AutoDreamExecutor {
    /// Configuration for gating thresholds.
    config: AutoDreamConfig,
    /// Shared mutable state (scan timestamps, counts).
    state: Arc<Mutex<AutoDreamState>>,
    /// Path to the auto-memory directory.
    memory_dir: PathBuf,
    /// Path to the session transcripts directory.
    session_dir: Option<PathBuf>,
}

impl AutoDreamExecutor {
    /// Create a new auto-dream executor.
    pub fn new(config: AutoDreamConfig, memory_dir: PathBuf, session_dir: Option<PathBuf>) -> Self {
        Self {
            config,
            state: Arc::new(Mutex::new(AutoDreamState::default())),
            memory_dir,
            session_dir,
        }
    }

    /// Returns a reference to the config.
    pub fn config(&self) -> &AutoDreamConfig {
        &self.config
    }

    /// Returns the memory directory path.
    pub fn memory_dir(&self) -> &Path {
        &self.memory_dir
    }

    /// Execute the 5-gate chain. Returns true if auto-dream should proceed.
    ///
    /// # Arguments
    /// * `is_remote` — whether this is a remote session
    /// * `auto_memory_enabled` — whether auto-memory is enabled
    /// * `agent_id` — the agent ID (auto-dream only runs on main agent, not sub-agents)
    pub fn should_trigger(
        &self,
        is_remote: bool,
        auto_memory_enabled: bool,
        agent_id: Option<&str>,
    ) -> bool {
        // Auto-dream only fires on main agent (no agent_id)
        if agent_id.is_some() {
            return false;
        }

        // Gate 1: Feature toggle
        if !self.config.is_enabled(is_remote, auto_memory_enabled) {
            return false;
        }

        // Gate 2: Time gate
        let lock = ConsolidationLock::new(&self.memory_dir);
        let last_ms = lock.read_last_consolidated_at();
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let hours_since = (now_ms.saturating_sub(last_ms)) as f64 / 3_600_000.0;
        if !self.config.time_gate_passed(hours_since) {
            return false;
        }

        // Gate 3: Scan throttle
        let minutes_since_scan = self
            .state
            .lock()
            .last_session_scan_at
            .map(|scan_ms| (now_ms.saturating_sub(scan_ms)) as f64 / 60_000.0);
        if !self.config.scan_throttle_passed(minutes_since_scan) {
            return false;
        }

        // Gate 4: Session gate — scan sessions
        let session_count = self.count_sessions_since(last_ms);
        {
            let mut state = self.state.lock();
            state.last_session_scan_at = Some(now_ms);
            state.last_session_count = session_count;
        }
        if !self.config.session_gate_passed(session_count) {
            return false;
        }

        // Gate 5: Lock gate
        lock.try_acquire().is_ok()
    }

    /// Count sessions modified since the given timestamp.
    fn count_sessions_since(&self, since_ms: u128) -> usize {
        let Some(ref session_dir) = self.session_dir else {
            return 0;
        };

        let since_time = UNIX_EPOCH + std::time::Duration::from_millis(since_ms as u64);

        std::fs::read_dir(session_dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| {
                        e.metadata()
                            .ok()
                            .and_then(|m| m.modified().ok())
                            .is_some_and(|mtime| mtime > since_time)
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    /// Build the dream prompt for the forked agent.
    pub fn build_prompt(&self) -> String {
        consolidation_prompt::build_dream_prompt(
            self.memory_dir.to_str().unwrap_or("."),
            self.session_dir.as_deref().and_then(|p| p.to_str()),
        )
    }

    /// Get the system prompt for the dream agent.
    pub fn system_prompt(&self) -> &'static str {
        AUTO_DREAM_SYSTEM_PROMPT
    }

    /// Roll back the consolidation lock after failure.
    pub fn rollback_lock(&self, prior_mtime_ms: u128) {
        let lock = ConsolidationLock::new(&self.memory_dir);
        lock.rollback(prior_mtime_ms);
    }

    /// Record successful consolidation.
    pub fn record_consolidation(&self) -> anyhow::Result<()> {
        let lock = ConsolidationLock::new(&self.memory_dir);
        lock.record_consolidation()
    }

    /// Launch the auto-dream forked sub-agent.
    ///
    /// This spawns a child `claude` process with the dream system prompt
    /// and user prompt. The child runs asynchronously (fire-and-forget)
    /// as part of Phase 3 (BackgroundFireAndForget) of the stop-hook pipeline.
    ///
    /// # Arguments
    /// * `claude_bin` — path to the claude binary (or "claude" for PATH lookup)
    /// * `model` — model to use for the dream agent (e.g. "claude-haiku-4-5")
    /// * `timeout_secs` — max seconds the dream agent may run (default: 300)
    ///
    /// # Returns
    /// The child process PID on success, or an error if spawning fails.
    /// Lock rollback happens internally if the child exits with failure.
    pub fn run(&self, claude_bin: &str, model: &str, timeout_secs: u64) -> anyhow::Result<u32> {
        let prior_ms = {
            let lock = ConsolidationLock::new(&self.memory_dir);
            lock.read_last_consolidated_at()
        };

        let user_prompt = self.build_prompt();
        let system_prompt = self.system_prompt().to_owned();
        let memory_dir = self.memory_dir.clone();

        // Spawn the claude child process with dream prompts
        let mut child = std::process::Command::new(claude_bin)
            .arg("--print")
            .arg("--model")
            .arg(model)
            .arg("--system-prompt")
            .arg(&system_prompt)
            .arg("--max-turns")
            .arg("30")
            .arg("--output-format")
            .arg("text")
            .arg("--dangerously-skip-permissions")
            .arg("--verbose")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .current_dir(&memory_dir)
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
            .spawn()
            .map_err(|e| anyhow::anyhow!("failed to spawn claude dream agent: {e}"))?;

        let pid = child.id();

        // Write the dream prompt to stdin
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            let _ = stdin.write_all(user_prompt.as_bytes());
            let _ = stdin.flush();
            // stdin is dropped here, closing it
        }

        // Spawn a background thread to wait for the child with timeout
        let memory_dir_bg = memory_dir.clone();
        std::thread::spawn(move || {
            let result = child.wait_timeout(std::time::Duration::from_secs(timeout_secs));

            match result {
                Ok(Some(status)) if status.success() => {
                    // Success — record consolidation
                    let lock = ConsolidationLock::new(&memory_dir_bg);
                    if let Err(e) = lock.record_consolidation() {
                        eprintln!("[auto-dream] failed to record consolidation: {e}");
                    }
                }
                Ok(Some(status)) => {
                    // Child exited with error — rollback
                    eprintln!(
                        "[auto-dream] child exited with status {}, rolling back lock",
                        status.code().unwrap_or(-1)
                    );
                    let lock = ConsolidationLock::new(&memory_dir_bg);
                    lock.rollback(prior_ms);
                }
                Ok(None) => {
                    // Timeout — kill and rollback
                    eprintln!("[auto-dream] child timed out after {timeout_secs}s, killing");
                    let _ = child.kill();
                    let _ = child.wait();
                    let lock = ConsolidationLock::new(&memory_dir_bg);
                    lock.rollback(prior_ms);
                }
                Err(e) => {
                    eprintln!("[auto-dream] failed to wait for child: {e}");
                    let lock = ConsolidationLock::new(&memory_dir_bg);
                    lock.rollback(prior_ms);
                }
            }
        });

        Ok(pid)
    }
}

/// Extension trait for `std::process::Child` to support timeout on wait.
trait ChildWaitTimeout {
    fn wait_timeout(
        &mut self,
        timeout: std::time::Duration,
    ) -> std::io::Result<Option<std::process::ExitStatus>>;
}

impl ChildWaitTimeout for std::process::Child {
    fn wait_timeout(
        &mut self,
        timeout: std::time::Duration,
    ) -> std::io::Result<Option<std::process::ExitStatus>> {
        // Poll-based timeout: check status every 100ms up to timeout
        let start = std::time::Instant::now();
        loop {
            match self.try_wait()? {
                Some(status) => return Ok(Some(status)),
                None => {
                    if start.elapsed() >= timeout {
                        return Ok(None);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn executor_skips_sub_agents() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let executor =
            AutoDreamExecutor::new(AutoDreamConfig::default(), dir.path().to_path_buf(), None);
        assert!(!executor.should_trigger(false, true, Some("agent-123")));
    }

    #[test]
    fn executor_skips_remote_sessions() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let executor =
            AutoDreamExecutor::new(AutoDreamConfig::default(), dir.path().to_path_buf(), None);
        assert!(!executor.should_trigger(true, true, None));
    }

    #[test]
    fn executor_skips_when_auto_memory_disabled() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let executor =
            AutoDreamExecutor::new(AutoDreamConfig::default(), dir.path().to_path_buf(), None);
        assert!(!executor.should_trigger(false, false, None));
    }

    #[test]
    fn build_prompt_returns_nonempty() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let executor = AutoDreamExecutor::new(
            AutoDreamConfig::default(),
            dir.path().to_path_buf(),
            Some(dir.path().to_path_buf()),
        );
        let prompt = executor.build_prompt();
        assert!(!prompt.is_empty());
        assert!(prompt.contains("Phase 1"));
    }

    #[test]
    fn system_prompt_is_dream_prompt() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let executor =
            AutoDreamExecutor::new(AutoDreamConfig::default(), dir.path().to_path_buf(), None);
        assert!(executor.system_prompt().contains("Phase 4"));
    }

    #[test]
    fn session_count_with_no_dir() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let executor =
            AutoDreamExecutor::new(AutoDreamConfig::default(), dir.path().to_path_buf(), None);
        assert_eq!(executor.count_sessions_since(0), 0);
    }

    #[test]
    fn session_count_counts_recent_files() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let session_dir = dir.path().join("sessions");
        fs::create_dir_all(&session_dir).expect("session dir should be created");
        fs::write(session_dir.join("sess1.jsonl"), "test")
            .expect("first session fixture should be written");
        fs::write(session_dir.join("sess2.jsonl"), "test")
            .expect("second session fixture should be written");

        let executor = AutoDreamExecutor::new(
            AutoDreamConfig::default(),
            dir.path().to_path_buf(),
            Some(session_dir),
        );
        // Since since_ms = 0, all files should be counted
        assert!(executor.count_sessions_since(0) >= 2);
    }
}
