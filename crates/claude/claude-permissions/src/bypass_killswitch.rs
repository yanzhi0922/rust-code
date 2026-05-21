//! Bypass permissions killswitch.
//!
//! Corresponds to `src/utils/permissions/bypassPermissionsKillswitch.ts`.
//! Provides a safety mechanism to disable bypass permissions mode remotely.

use parking_lot::Mutex;
use std::time::{Duration, Instant};

/// State of the bypass permissions killswitch.
#[derive(Debug, Clone)]
pub struct BypassKillswitchState {
    /// Whether the killswitch is active (bypass is disabled).
    pub active: bool,
    /// Reason for activation.
    pub reason: Option<String>,
    /// When the killswitch was activated.
    pub activated_at: Option<Instant>,
    /// TTL for the killswitch state (auto-expire).
    pub ttl: Option<Duration>,
}

impl Default for BypassKillswitchState {
    fn default() -> Self {
        Self {
            active: false,
            reason: None,
            activated_at: None,
            ttl: Some(Duration::from_secs(300)), // 5 minute default TTL
        }
    }
}

impl BypassKillswitchState {
    /// Create a new killswitch state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Activate the killswitch.
    pub fn activate(&mut self, reason: String) {
        self.active = true;
        self.reason = Some(reason);
        self.activated_at = Some(Instant::now());
    }

    /// Deactivate the killswitch.
    pub fn deactivate(&mut self) {
        self.active = false;
        self.reason = None;
        self.activated_at = None;
    }

    /// Check if the killswitch is currently active (considering TTL).
    #[must_use]
    pub fn is_active(&self) -> bool {
        if !self.active {
            return false;
        }

        if let (Some(activated), Some(ttl)) = (self.activated_at, self.ttl) {
            activated.elapsed() < ttl
        } else {
            true
        }
    }
}

/// Thread-safe killswitch manager.
pub struct BypassKillswitchManager {
    state: Mutex<BypassKillswitchState>,
}

impl BypassKillswitchManager {
    /// Create a new killswitch manager.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Mutex::new(BypassKillswitchState::new()),
        }
    }

    /// Activate the killswitch.
    pub fn activate(&self, reason: String) {
        self.state.lock().activate(reason);
    }

    /// Deactivate the killswitch.
    pub fn deactivate(&self) {
        self.state.lock().deactivate();
    }

    /// Check if bypass permissions is currently disabled.
    #[must_use]
    pub fn is_bypass_disabled(&self) -> bool {
        self.state.lock().is_active()
    }

    /// Get the reason for the killswitch being active.
    #[must_use]
    pub fn reason(&self) -> Option<String> {
        self.state.lock().reason.clone()
    }
}

impl Default for BypassKillswitchManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn killswitch_starts_inactive() {
        let state = BypassKillswitchState::new();
        assert!(!state.is_active());
    }

    #[test]
    fn killswitch_activate_deactivate() {
        let mut state = BypassKillswitchState::new();
        state.activate("security alert".to_string());
        assert!(state.is_active());
        assert_eq!(state.reason, Some("security alert".to_string()));

        state.deactivate();
        assert!(!state.is_active());
    }

    #[test]
    fn killswitch_manager_thread_safe() {
        let manager = BypassKillswitchManager::new();
        assert!(!manager.is_bypass_disabled());

        manager.activate("test".to_string());
        assert!(manager.is_bypass_disabled());
        assert_eq!(manager.reason(), Some("test".to_string()));

        manager.deactivate();
        assert!(!manager.is_bypass_disabled());
    }

    #[test]
    fn killswitch_ttl_expires() {
        let mut state = BypassKillswitchState::new();
        state.ttl = Some(Duration::from_millis(1));
        state.activate("test".to_string());

        // Wait for TTL to expire
        std::thread::sleep(Duration::from_millis(10));
        assert!(!state.is_active());
    }
}
