//! Denial tracking for permission decisions.
//!
//! Corresponds to `src/utils/permissions/denialTracking.ts`.
//! Tracks permission denials to detect patterns and prevent repeated prompts.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::time::Instant;

/// A single denial record.
#[derive(Debug, Clone)]
pub struct DenialRecord {
    /// Tool name that was denied.
    pub tool_name: String,
    /// Reason for denial.
    pub reason: String,
    /// When the denial occurred.
    pub timestamp: Instant,
    /// Number of consecutive denials for this tool.
    pub consecutive_count: u32,
}

/// Session-level denial limits (matching TS `DENIAL_LIMITS`).
pub const MAX_CONSECUTIVE_DENIALS: u32 = 3;
pub const MAX_TOTAL_DENIALS: u64 = 20;

/// Denial tracking state.
#[derive(Debug, Default)]
pub struct DenialTracker {
    /// Denial records by tool name.
    denials: HashMap<String, DenialRecord>,
    /// Total denial count across all tools.
    total_denials: u64,
    /// Consecutive denials across any tool (resets on any approval).
    session_consecutive: u32,
}

impl DenialTracker {
    /// Create a new denial tracker.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a denial.
    pub fn record_denial(&mut self, tool_name: &str, reason: &str) {
        self.total_denials += 1;
        self.session_consecutive += 1;

        let entry = self.denials.get_mut(tool_name);
        match entry {
            Some(record) => {
                record.consecutive_count += 1;
                record.reason = reason.to_string();
                record.timestamp = Instant::now();
            }
            None => {
                self.denials.insert(
                    tool_name.to_string(),
                    DenialRecord {
                        tool_name: tool_name.to_string(),
                        reason: reason.to_string(),
                        timestamp: Instant::now(),
                        consecutive_count: 1,
                    },
                );
            }
        }
    }

    /// Record an approval (resets consecutive denial count).
    pub fn record_approval(&mut self, tool_name: &str) {
        self.session_consecutive = 0;
        if let Some(record) = self.denials.get_mut(tool_name) {
            record.consecutive_count = 0;
        }
    }

    /// Get the consecutive denial count for a tool.
    #[must_use]
    pub fn consecutive_denials(&self, tool_name: &str) -> u32 {
        self.denials
            .get(tool_name)
            .map(|r| r.consecutive_count)
            .unwrap_or(0)
    }

    /// Check if a tool has been denied too many times (suggest auto-skip).
    #[must_use]
    pub fn should_auto_skip(&self, tool_name: &str, threshold: u32) -> bool {
        self.consecutive_denials(tool_name) >= threshold
    }

    /// Get total denial count.
    #[must_use]
    pub fn total_denials(&self) -> u64 {
        self.total_denials
    }

    /// Check if the system should fall back to prompting (not auto-mode)
    /// because too many denials have occurred in this session.
    /// Matches TS `shouldFallbackToPrompting()`.
    #[must_use]
    pub fn should_fallback_to_prompting(&self) -> bool {
        self.session_consecutive >= MAX_CONSECUTIVE_DENIALS
            || self.total_denials >= MAX_TOTAL_DENIALS
    }

    /// Clear all tracking data.
    pub fn clear(&mut self) {
        self.denials.clear();
        self.total_denials = 0;
        self.session_consecutive = 0;
    }
}

/// Thread-safe denial tracker.
pub struct SharedDenialTracker {
    inner: Mutex<DenialTracker>,
}

impl SharedDenialTracker {
    /// Create a new shared denial tracker.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(DenialTracker::new()),
        }
    }

    /// Record a denial.
    pub fn record_denial(&self, tool_name: &str, reason: &str) {
        self.inner.lock().record_denial(tool_name, reason);
    }

    /// Record an approval.
    pub fn record_approval(&self, tool_name: &str) {
        self.inner.lock().record_approval(tool_name);
    }

    /// Get consecutive denial count.
    #[must_use]
    pub fn consecutive_denials(&self, tool_name: &str) -> u32 {
        self.inner.lock().consecutive_denials(tool_name)
    }

    /// Check if should auto-skip.
    #[must_use]
    pub fn should_auto_skip(&self, tool_name: &str, threshold: u32) -> bool {
        self.inner.lock().should_auto_skip(tool_name, threshold)
    }

    /// Check if should fall back to prompting.
    #[must_use]
    pub fn should_fallback_to_prompting(&self) -> bool {
        self.inner.lock().should_fallback_to_prompting()
    }
}

impl Default for SharedDenialTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_denials() {
        let mut tracker = DenialTracker::new();
        tracker.record_denial("Bash", "dangerous");
        tracker.record_denial("Bash", "dangerous");
        tracker.record_denial("Bash", "dangerous");

        assert_eq!(tracker.consecutive_denials("Bash"), 3);
        assert_eq!(tracker.total_denials(), 3);
    }

    #[test]
    fn approval_resets_consecutive() {
        let mut tracker = DenialTracker::new();
        tracker.record_denial("Bash", "dangerous");
        tracker.record_denial("Bash", "dangerous");
        tracker.record_approval("Bash");

        assert_eq!(tracker.consecutive_denials("Bash"), 0);
    }

    #[test]
    fn auto_skip_threshold() {
        let mut tracker = DenialTracker::new();
        tracker.record_denial("Bash", "dangerous");
        tracker.record_denial("Bash", "dangerous");
        tracker.record_denial("Bash", "dangerous");

        assert!(tracker.should_auto_skip("Bash", 3));
        assert!(!tracker.should_auto_skip("Bash", 4));
    }

    #[test]
    fn shared_tracker_thread_safe() {
        let tracker = SharedDenialTracker::new();
        tracker.record_denial("Bash", "test");
        assert_eq!(tracker.consecutive_denials("Bash"), 1);
    }

    #[test]
    fn clear_resets_everything() {
        let mut tracker = DenialTracker::new();
        tracker.record_denial("Bash", "test");
        tracker.clear();
        assert_eq!(tracker.total_denials(), 0);
        assert_eq!(tracker.consecutive_denials("Bash"), 0);
    }

    #[test]
    fn fallback_to_prompting_after_consecutive_limit() {
        let mut tracker = DenialTracker::new();
        for _ in 0..MAX_CONSECUTIVE_DENIALS {
            tracker.record_denial("Bash", "dangerous");
        }
        assert!(tracker.should_fallback_to_prompting());
    }

    #[test]
    fn approval_resets_session_consecutive() {
        let mut tracker = DenialTracker::new();
        tracker.record_denial("Bash", "dangerous");
        tracker.record_denial("Bash", "dangerous");
        tracker.record_approval("Bash");
        assert!(!tracker.should_fallback_to_prompting());
    }

    #[test]
    fn fallback_after_total_limit() {
        let mut tracker = DenialTracker::new();
        for i in 0..MAX_TOTAL_DENIALS {
            let tool = format!("Tool{i}");
            tracker.record_denial(&tool, "test");
            tracker.record_approval(&tool); // Reset consecutive each time
        }
        assert!(tracker.should_fallback_to_prompting());
    }
}
