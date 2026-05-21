//! Auto mode state management.
//!
//! Corresponds to `src/utils/permissions/autoModeState.ts`.
//! Manages the state of the auto permission mode, including
//! classifier decisions and cooldown tracking.

use parking_lot::Mutex;
use std::time::Instant;

use crate::classifier::ClassifierResult;

/// State of the auto permission mode.
#[derive(Debug, Clone, Default)]
pub struct AutoModeState {
    /// Whether auto mode is currently active.
    pub active: bool,
    /// Number of auto-approved operations in this session.
    pub auto_approved_count: u64,
    /// Number of auto-denied operations in this session.
    pub auto_denied_count: u64,
    /// Number of operations that were escalated to the user.
    pub escalated_count: u64,
    /// Last classifier result.
    pub last_result: Option<ClassifierResult>,
    /// When auto mode was activated.
    pub activated_at: Option<Instant>,
    /// Cooldown until next auto-approval (ms).
    pub cooldown_ms: u64,
}

impl AutoModeState {
    /// Create a new auto mode state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Activate auto mode.
    pub fn activate(&mut self) {
        self.active = true;
        self.activated_at = Some(Instant::now());
        self.auto_approved_count = 0;
        self.auto_denied_count = 0;
        self.escalated_count = 0;
    }

    /// Deactivate auto mode.
    pub fn deactivate(&mut self) {
        self.active = false;
        self.activated_at = None;
    }

    /// Record a classifier result.
    pub fn record_result(&mut self, result: &ClassifierResult) {
        if result.should_allow {
            self.auto_approved_count += 1;
        } else {
            self.auto_denied_count += 1;
        }
        self.last_result = Some(result.clone());
    }

    /// Record an escalation to user.
    pub fn record_escalation(&mut self) {
        self.escalated_count += 1;
    }

    /// Get the duration auto mode has been active (in seconds).
    #[must_use]
    pub fn active_duration_secs(&self) -> Option<u64> {
        self.activated_at.map(|t| t.elapsed().as_secs())
    }

    /// Get total decisions made.
    #[must_use]
    pub fn total_decisions(&self) -> u64 {
        self.auto_approved_count + self.auto_denied_count + self.escalated_count
    }

    /// Get approval rate (0.0 - 1.0).
    #[must_use]
    pub fn approval_rate(&self) -> f64 {
        let total = self.total_decisions();
        if total == 0 {
            return 0.0;
        }
        self.auto_approved_count as f64 / total as f64
    }
}

/// Thread-safe auto mode state manager.
pub struct AutoModeManager {
    state: Mutex<AutoModeState>,
}

impl AutoModeManager {
    /// Create a new auto mode manager.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Mutex::new(AutoModeState::new()),
        }
    }

    /// Activate auto mode.
    pub fn activate(&self) {
        self.state.lock().activate();
    }

    /// Deactivate auto mode.
    pub fn deactivate(&self) {
        self.state.lock().deactivate();
    }

    /// Check if auto mode is active.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.state.lock().active
    }

    /// Record a classifier result.
    pub fn record_result(&self, result: &ClassifierResult) {
        self.state.lock().record_result(result);
    }

    /// Get a snapshot of the current state.
    #[must_use]
    pub fn snapshot(&self) -> AutoModeState {
        self.state.lock().clone()
    }
}

impl Default for AutoModeManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_mode_activate_deactivate() {
        let mut state = AutoModeState::new();
        assert!(!state.active);
        state.activate();
        assert!(state.active);
        state.deactivate();
        assert!(!state.active);
    }

    #[test]
    fn auto_mode_record_results() {
        let mut state = AutoModeState::new();
        state.activate();

        let allow = ClassifierResult::allow("safe", 90);
        let deny = ClassifierResult::deny("unsafe", 80);

        state.record_result(&allow);
        state.record_result(&allow);
        state.record_result(&deny);

        assert_eq!(state.auto_approved_count, 2);
        assert_eq!(state.auto_denied_count, 1);
        assert_eq!(state.total_decisions(), 3);
        assert!((state.approval_rate() - 0.6667).abs() < 0.01);
    }

    #[test]
    fn auto_mode_manager_thread_safe() {
        let manager = AutoModeManager::new();
        assert!(!manager.is_active());
        manager.activate();
        assert!(manager.is_active());

        let result = ClassifierResult::allow("test", 95);
        manager.record_result(&result);

        let snap = manager.snapshot();
        assert_eq!(snap.auto_approved_count, 1);
    }
}
