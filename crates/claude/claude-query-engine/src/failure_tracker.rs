//! Consecutive failure tracking with circuit breaker behavior.
//!
//! Tracks consecutive provider/tool failures and implements a circuit
//! breaker pattern to prevent cascading failures.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Circuit breaker state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CircuitState {
    /// Normal operation; failures are tracked.
    Closed,
    /// Too many failures; requests are blocked.
    Open,
    /// Testing whether the system has recovered.
    HalfOpen,
}

/// Tracks consecutive failures and implements circuit breaker logic.
#[derive(Debug, Clone)]
pub struct FailureTracker {
    /// Maximum consecutive failures before opening the circuit.
    max_failures: usize,
    /// Current count of consecutive failures.
    consecutive_failures: usize,
    /// Total failures since last reset.
    total_failures: usize,
    /// Total successes since last reset.
    total_successes: usize,
    /// Time of the last failure.
    last_failure_time: Option<Instant>,
    /// Duration to wait before transitioning from Open to HalfOpen.
    cooldown_duration: Duration,
    /// Current circuit state.
    state: CircuitState,
}

impl FailureTracker {
    /// Create a new failure tracker with the given threshold and cooldown.
    #[must_use]
    pub fn new(max_failures: usize, cooldown_duration: Duration) -> Self {
        Self {
            max_failures,
            consecutive_failures: 0,
            total_failures: 0,
            total_successes: 0,
            last_failure_time: None,
            cooldown_duration,
            state: CircuitState::Closed,
        }
    }

    /// Returns the current circuit state.
    #[must_use]
    pub fn state(&self) -> CircuitState {
        self.state
    }

    /// Returns the current consecutive failure count.
    #[must_use]
    pub fn consecutive_failures(&self) -> usize {
        self.consecutive_failures
    }

    /// Returns the total failure count.
    #[must_use]
    pub fn total_failures(&self) -> usize {
        self.total_failures
    }

    /// Returns the total success count.
    #[must_use]
    pub fn total_successes(&self) -> usize {
        self.total_successes
    }

    /// Returns the maximum failure threshold.
    #[must_use]
    pub fn max_failures(&self) -> usize {
        self.max_failures
    }

    /// Returns the cooldown duration.
    #[must_use]
    pub fn cooldown_duration(&self) -> Duration {
        self.cooldown_duration
    }

    /// Returns true if the circuit breaker is allowing requests.
    #[must_use]
    pub fn is_available(&self) -> bool {
        match self.state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                // Check if cooldown has elapsed
                self.last_failure_time
                    .is_none_or(|t| t.elapsed() >= self.cooldown_duration)
            }
            CircuitState::HalfOpen => true,
        }
    }

    /// Record a successful operation.
    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.total_successes += 1;
        self.state = CircuitState::Closed;
    }

    /// Record a failure. Returns the new circuit state.
    pub fn record_failure(&mut self) -> CircuitState {
        self.consecutive_failures += 1;
        self.total_failures += 1;
        self.last_failure_time = Some(Instant::now());

        if self.consecutive_failures >= self.max_failures {
            self.state = CircuitState::Open;
        }

        self.state
    }

    /// Attempt to transition from Open to HalfOpen if the cooldown has elapsed.
    /// Returns true if the transition occurred.
    pub fn try_half_open(&mut self) -> bool {
        if self.state == CircuitState::Open
            && let Some(last_failure) = self.last_failure_time
            && last_failure.elapsed() >= self.cooldown_duration
        {
            self.state = CircuitState::HalfOpen;
            return true;
        }
        false
    }

    /// Reset all counters and state.
    pub fn reset(&mut self) {
        self.consecutive_failures = 0;
        self.total_failures = 0;
        self.total_successes = 0;
        self.last_failure_time = None;
        self.state = CircuitState::Closed;
    }
}

impl Default for FailureTracker {
    fn default() -> Self {
        Self::new(3, Duration::from_secs(30))
    }
}

#[cfg(test)]
mod tests {
    use super::{CircuitState, FailureTracker};
    use std::time::Duration;

    #[test]
    fn failure_tracker_starts_closed() {
        let tracker = FailureTracker::default();
        assert_eq!(tracker.state(), CircuitState::Closed);
        assert!(tracker.is_available());
        assert_eq!(tracker.consecutive_failures(), 0);
    }

    #[test]
    fn failure_tracker_opens_after_max_failures() {
        let mut tracker = FailureTracker::new(3, Duration::from_secs(60));
        assert_eq!(tracker.record_failure(), CircuitState::Closed);
        assert_eq!(tracker.record_failure(), CircuitState::Closed);
        assert_eq!(tracker.record_failure(), CircuitState::Open);
        assert!(!tracker.is_available());
    }

    #[test]
    fn failure_tracker_success_resets_consecutive() {
        let mut tracker = FailureTracker::new(3, Duration::from_secs(60));
        tracker.record_failure();
        tracker.record_failure();
        assert_eq!(tracker.consecutive_failures(), 2);
        tracker.record_success();
        assert_eq!(tracker.consecutive_failures(), 0);
        assert_eq!(tracker.state(), CircuitState::Closed);
    }

    #[test]
    fn failure_tracker_reset_clears_all() {
        let mut tracker = FailureTracker::new(2, Duration::from_secs(60));
        tracker.record_failure();
        tracker.record_failure();
        tracker.reset();
        assert_eq!(tracker.consecutive_failures(), 0);
        assert_eq!(tracker.total_failures(), 0);
        assert_eq!(tracker.state(), CircuitState::Closed);
    }

    #[test]
    fn failure_tracker_counts_totals() {
        let mut tracker = FailureTracker::new(5, Duration::from_secs(60));
        tracker.record_failure();
        tracker.record_success();
        tracker.record_failure();
        assert_eq!(tracker.total_failures(), 2);
        assert_eq!(tracker.total_successes(), 1);
    }

    #[test]
    fn failure_tracker_try_half_open() {
        let mut tracker = FailureTracker::new(1, Duration::from_millis(1));
        tracker.record_failure(); // Opens circuit
        assert_eq!(tracker.state(), CircuitState::Open);
        // Wait for cooldown
        std::thread::sleep(Duration::from_millis(5));
        assert!(tracker.try_half_open());
        assert_eq!(tracker.state(), CircuitState::HalfOpen);
    }

    #[test]
    fn failure_tracker_half_open_to_closed_on_success() {
        let mut tracker = FailureTracker::new(1, Duration::from_millis(1));
        tracker.record_failure();
        std::thread::sleep(Duration::from_millis(5));
        tracker.try_half_open();
        tracker.record_success();
        assert_eq!(tracker.state(), CircuitState::Closed);
    }

    #[test]
    fn failure_tracker_default_values() {
        let tracker = FailureTracker::default();
        assert_eq!(tracker.max_failures(), 3);
        assert_eq!(tracker.cooldown_duration(), Duration::from_secs(30));
    }
}
