//! Agent restart strategy with exponential backoff.
//!
//! [`RestartTracker`] enforces a maximum restart count and computes the
//! next backoff duration using exponential backoff with a configurable
//! multiplier and cap.

use std::time::Duration;

/// Restart policy configuration.
#[derive(Debug, Clone)]
pub struct RestartPolicy {
    /// Maximum number of restart attempts allowed.
    pub max_restarts: u32,
    /// Initial backoff duration before the first retry.
    pub initial_backoff: Duration,
    /// Maximum backoff duration (cap).
    pub max_backoff: Duration,
    /// Multiplier applied to the backoff after each attempt.
    pub backoff_multiplier: f64,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            max_restarts: 3,
            initial_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(60),
            backoff_multiplier: 2.0,
        }
    }
}

/// Tracks restart attempts and computes backoff durations.
pub struct RestartTracker {
    policy: RestartPolicy,
    restart_count: u32,
    next_backoff: Duration,
}

impl RestartTracker {
    /// Create a new tracker with the given policy.
    ///
    /// # Panics
    ///
    /// Will **not** panic, but will clamp `backoff_multiplier` to `>= 1.0`
    /// if the caller provides an invalid value (zero, negative, or NaN).
    pub fn new(mut policy: RestartPolicy) -> Self {
        // #21: Validate backoff_multiplier — must be >= 1.0 for exponential
        // backoff to make sense. Use default of 2.0 if invalid.
        if policy.backoff_multiplier.is_nan() || policy.backoff_multiplier < 1.0 {
            // Catches NaN, negative, zero, and sub-one values.
            policy.backoff_multiplier = 2.0;
        }
        let next_backoff = policy.initial_backoff;
        Self {
            policy,
            restart_count: 0,
            next_backoff,
        }
    }

    /// Request a restart.
    ///
    /// Returns `Some(Duration)` — the time to wait before restarting — if the
    /// restart count has not exceeded the maximum. Returns `None` if no more
    /// restarts are allowed.
    pub fn request_restart(&mut self) -> Option<Duration> {
        if self.restart_count >= self.policy.max_restarts {
            return None;
        }

        let backoff = self.next_backoff;
        self.restart_count += 1;

        // Compute the next backoff, capped at `max_backoff`.
        let next_secs = self.next_backoff.as_secs_f64() * self.policy.backoff_multiplier;
        self.next_backoff =
            Duration::from_secs_f64(next_secs.min(self.policy.max_backoff.as_secs_f64()));

        Some(backoff)
    }

    /// Reset the restart counter (call after a successful run).
    pub fn reset(&mut self) {
        self.restart_count = 0;
        self.next_backoff = self.policy.initial_backoff;
    }

    /// Returns the number of restarts attempted so far.
    pub fn restart_count(&self) -> u32 {
        self.restart_count
    }

    /// Returns `true` if more restarts are allowed.
    pub fn can_restart(&self) -> bool {
        self.restart_count < self.policy.max_restarts
    }

    /// Returns a reference to the underlying policy.
    pub fn policy(&self) -> &RestartPolicy {
        &self.policy
    }
}

impl Default for RestartTracker {
    fn default() -> Self {
        Self::new(RestartPolicy::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_tracker_allows_restarts() {
        let tracker = RestartTracker::default();
        assert_eq!(tracker.restart_count(), 0);
        assert!(tracker.can_restart());
    }

    #[test]
    fn request_restart_increments_count() {
        let mut tracker = RestartTracker::default();
        let backoff = tracker.request_restart().expect("should allow restart");
        assert_eq!(backoff, Duration::from_secs(1));
        assert_eq!(tracker.restart_count(), 1);
        assert!(tracker.can_restart());
    }

    #[test]
    fn backoff_increases_exponentially() {
        let mut tracker = RestartTracker::new(RestartPolicy {
            max_restarts: 5,
            initial_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(60),
            backoff_multiplier: 2.0,
        });

        let b1 = tracker.request_restart().expect("restart 1");
        assert_eq!(b1, Duration::from_secs(1));

        let b2 = tracker.request_restart().expect("restart 2");
        assert_eq!(b2, Duration::from_secs(2));

        let b3 = tracker.request_restart().expect("restart 3");
        assert_eq!(b3, Duration::from_secs(4));
    }

    #[test]
    fn backoff_capped_at_max() {
        let mut tracker = RestartTracker::new(RestartPolicy {
            max_restarts: 10,
            initial_backoff: Duration::from_secs(30),
            max_backoff: Duration::from_secs(60),
            backoff_multiplier: 3.0,
        });

        let b1 = tracker.request_restart().expect("restart 1");
        assert_eq!(b1, Duration::from_secs(30));

        // 30 * 3 = 90, capped at 60
        let b2 = tracker.request_restart().expect("restart 2");
        assert_eq!(b2, Duration::from_secs(60));

        // Still capped
        let b3 = tracker.request_restart().expect("restart 3");
        assert_eq!(b3, Duration::from_secs(60));
    }

    #[test]
    fn max_restarts_exhausted() {
        let mut tracker = RestartTracker::new(RestartPolicy {
            max_restarts: 2,
            initial_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(10),
            backoff_multiplier: 2.0,
        });

        assert!(tracker.request_restart().is_some());
        assert_eq!(tracker.restart_count(), 1);

        assert!(tracker.request_restart().is_some());
        assert_eq!(tracker.restart_count(), 2);

        // Exhausted
        assert!(tracker.request_restart().is_none());
        assert!(!tracker.can_restart());
    }

    #[test]
    fn reset_clears_counter() {
        let mut tracker = RestartTracker::new(RestartPolicy {
            max_restarts: 1,
            initial_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(10),
            backoff_multiplier: 2.0,
        });

        tracker.request_restart().expect("restart 1");
        assert!(!tracker.can_restart());

        tracker.reset();
        assert_eq!(tracker.restart_count(), 0);
        assert!(tracker.can_restart());

        // After reset, backoff should be back to initial
        let backoff = tracker.request_restart().expect("restart after reset");
        assert_eq!(backoff, Duration::from_secs(1));
    }

    #[test]
    fn full_restart_cycle() {
        let mut tracker = RestartTracker::new(RestartPolicy {
            max_restarts: 3,
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(1),
            backoff_multiplier: 2.0,
        });

        // Exhaust all restarts
        let b1 = tracker.request_restart().expect("r1");
        assert_eq!(b1, Duration::from_millis(100));

        let b2 = tracker.request_restart().expect("r2");
        assert_eq!(b2, Duration::from_millis(200));

        let b3 = tracker.request_restart().expect("r3");
        assert_eq!(b3, Duration::from_millis(400));

        assert!(tracker.request_restart().is_none());

        // Reset and start over
        tracker.reset();
        assert_eq!(tracker.restart_count(), 0);
        assert!(tracker.can_restart());

        let b_after = tracker.request_restart().expect("after reset");
        assert_eq!(b_after, Duration::from_millis(100));
    }

    #[test]
    fn policy_default_values() {
        let policy = RestartPolicy::default();
        assert_eq!(policy.max_restarts, 3);
        assert_eq!(policy.initial_backoff, Duration::from_secs(1));
        assert_eq!(policy.max_backoff, Duration::from_secs(60));
        assert!((policy.backoff_multiplier - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn zero_max_restarts_never_allows() {
        let mut tracker = RestartTracker::new(RestartPolicy {
            max_restarts: 0,
            initial_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(10),
            backoff_multiplier: 2.0,
        });
        assert!(!tracker.can_restart());
        assert!(tracker.request_restart().is_none());
    }
}
