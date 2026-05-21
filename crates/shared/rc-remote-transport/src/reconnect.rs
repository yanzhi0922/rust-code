//! Reconnection policies for all strategies.

use std::time::Duration;

/// Policy controlling how a transport reconnects after a failure.
#[derive(Debug, Clone)]
pub struct ReconnectPolicy {
    /// Initial delay before first reconnect attempt.
    pub initial_delay: Duration,
    /// Maximum delay between reconnect attempts.
    pub max_delay: Duration,
    /// Multiplier for exponential backoff.
    pub multiplier: f64,
    /// Random jitter fraction (0.0–1.0) to add to each delay.
    pub jitter_fraction: f64,
    /// Maximum number of reconnect attempts before giving up (None = unlimited).
    pub max_attempts: Option<u32>,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(15),
            multiplier: 2.0,
            jitter_fraction: 0.25,
            max_attempts: None,
        }
    }
}

impl ReconnectPolicy {
    /// Calculate the delay for the given attempt number (0-based).
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let base_secs = self.initial_delay.as_secs_f64()
            * self
                .multiplier
                .powi(i32::try_from(attempt).unwrap_or(i32::MAX));
        let capped = base_secs.min(self.max_delay.as_secs_f64());
        let jitter = if self.jitter_fraction > 0.0 {
            let rand_factor = 1.0 - self.jitter_fraction
                + (2.0
                    * self.jitter_fraction
                    * ((attempt.wrapping_mul(2654435761) % 1000) as f64 / 1000.0));
            capped * rand_factor
        } else {
            capped
        };
        Duration::from_secs_f64(jitter)
    }

    /// Whether we should retry after the given number of attempts.
    pub fn should_retry(&self, attempt: u32) -> bool {
        self.max_attempts.is_none_or(|max| attempt < max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_increases_exponentially() {
        let policy = ReconnectPolicy::default();
        let d0 = policy.delay_for_attempt(0);
        let d1 = policy.delay_for_attempt(1);
        let d2 = policy.delay_for_attempt(2);
        assert!(d0 < d1);
        assert!(d1 < d2);
    }

    #[test]
    fn backoff_caps_at_max() {
        let policy = ReconnectPolicy::default();
        let d100 = policy.delay_for_attempt(100);
        assert!(d100 <= policy.max_delay * 2); // jitter can add up to 2x
    }

    #[test]
    fn should_retry_respects_max_attempts() {
        let policy = ReconnectPolicy {
            max_attempts: Some(3),
            ..Default::default()
        };
        assert!(policy.should_retry(0));
        assert!(policy.should_retry(2));
        assert!(!policy.should_retry(3));
    }
}
