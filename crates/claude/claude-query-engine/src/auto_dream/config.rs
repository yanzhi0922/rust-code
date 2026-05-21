//! Auto-dream configuration and feature toggle.
//!
//! Mirrors TS `config.ts` with GrowthBook flag defaults
//! and settings.json override support.

use serde::{Deserialize, Serialize};

/// Default minimum hours between consolidations.
pub const DEFAULT_MIN_HOURS: f64 = 24.0;

/// Default minimum session count required to trigger consolidation.
pub const DEFAULT_MIN_SESSIONS: usize = 5;

/// Default scan throttle in minutes.
pub const DEFAULT_SCAN_THROTTLE_MINUTES: u64 = 10;

/// Auto-dream configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoDreamConfig {
    /// Whether auto-dream is enabled.
    /// Can be overridden by `settings.json` `"autoDreamEnabled"`.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Minimum hours since last consolidation before triggering.
    #[serde(default = "default_min_hours")]
    pub min_hours: f64,

    /// Minimum number of sessions (excluding current) needed to trigger.
    #[serde(default = "default_min_sessions")]
    pub min_sessions: usize,

    /// Minimum minutes between directory scans.
    #[serde(default = "default_scan_throttle")]
    pub scan_throttle_minutes: u64,
}

fn default_enabled() -> bool {
    true
}
fn default_min_hours() -> f64 {
    DEFAULT_MIN_HOURS
}
fn default_min_sessions() -> usize {
    DEFAULT_MIN_SESSIONS
}
fn default_scan_throttle() -> u64 {
    DEFAULT_SCAN_THROTTLE_MINUTES
}

impl Default for AutoDreamConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            min_hours: default_min_hours(),
            min_sessions: default_min_sessions(),
            scan_throttle_minutes: default_scan_throttle(),
        }
    }
}

impl AutoDreamConfig {
    /// Create a new config with defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply a user override from settings.json.
    pub fn with_user_override(mut self, user_enabled: Option<bool>) -> Self {
        if let Some(enabled) = user_enabled {
            self.enabled = enabled;
        }
        self
    }

    /// Gate 1: Feature toggle check.
    /// Auto-dream requires: enabled, not remote session, auto-memory enabled.
    pub fn is_enabled(&self, is_remote: bool, auto_memory_enabled: bool) -> bool {
        self.enabled && !is_remote && auto_memory_enabled
    }

    /// Gate 2: Time gate — enough hours since last consolidation?
    pub fn time_gate_passed(&self, hours_since_last: f64) -> bool {
        hours_since_last >= self.min_hours
    }

    /// Gate 4: Session gate — enough sessions accumulated?
    pub fn session_gate_passed(&self, session_count: usize) -> bool {
        session_count > self.min_sessions
    }

    /// Gate 3: Scan throttle — enough time since last scan?
    pub fn scan_throttle_passed(&self, minutes_since_last_scan: Option<f64>) -> bool {
        match minutes_since_last_scan {
            Some(mins) => mins >= self.scan_throttle_minutes as f64,
            None => true, // never scanned before
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let cfg = AutoDreamConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.min_hours, 24.0);
        assert_eq!(cfg.min_sessions, 5);
        assert_eq!(cfg.scan_throttle_minutes, 10);
    }

    #[test]
    fn gate_1_feature_toggle() {
        let cfg = AutoDreamConfig::default();
        assert!(cfg.is_enabled(false, true)); // enabled, local, auto-mem on
        assert!(!cfg.is_enabled(true, true)); // remote — blocked
        assert!(!cfg.is_enabled(false, false)); // auto-mem off
        assert!(!cfg.is_enabled(true, false)); // both blocked
    }

    #[test]
    fn gate_1_user_override_disables() {
        let cfg = AutoDreamConfig::default().with_user_override(Some(false));
        assert!(!cfg.is_enabled(false, true));
    }

    #[test]
    fn gate_2_time_gate() {
        let cfg = AutoDreamConfig::default();
        assert!(!cfg.time_gate_passed(12.0)); // not enough
        assert!(cfg.time_gate_passed(24.0)); // exactly enough
        assert!(cfg.time_gate_passed(48.0)); // more than enough
    }

    #[test]
    fn gate_3_scan_throttle() {
        let cfg = AutoDreamConfig::default();
        assert!(cfg.scan_throttle_passed(None)); // never scanned
        assert!(!cfg.scan_throttle_passed(Some(5.0))); // too soon
        assert!(cfg.scan_throttle_passed(Some(10.0))); // exactly enough
    }

    #[test]
    fn gate_4_session_gate() {
        let cfg = AutoDreamConfig::default();
        assert!(!cfg.session_gate_passed(5)); // equal, not greater
        assert!(cfg.session_gate_passed(6)); // more than min
    }

    #[test]
    fn serialization_roundtrip() {
        let cfg = AutoDreamConfig::default();
        let json = serde_json::to_string(&cfg).expect("auto dream config should serialize");
        let parsed: AutoDreamConfig =
            serde_json::from_str(&json).expect("auto dream config should deserialize");
        assert_eq!(parsed.min_hours, cfg.min_hours);
        assert_eq!(parsed.min_sessions, cfg.min_sessions);
    }
}
