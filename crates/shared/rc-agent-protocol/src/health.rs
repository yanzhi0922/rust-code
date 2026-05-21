//! Agent health check mechanism.
//!
//! Provides [`HealthChecker`] to track an Agent's health over time via periodic
//! probes. Each probe reports whether the Agent is alive; the checker aggregates
//! consecutive failures and transitions through [`HealthStatus`] variants.

use std::time::{Duration, Instant};

/// Agent health check configuration.
#[derive(Debug, Clone)]
pub struct HealthCheckConfig {
    /// Interval between health checks.
    pub interval: Duration,
    /// Timeout after which a lack of response is considered unhealthy.
    ///
    /// # Note
    ///
    /// This field is currently defined but **not used** by [`HealthChecker::check`].
    /// The health check currently relies on the caller providing a boolean
    /// `is_alive` result, which the caller should obtain with its own timeout
    /// logic. A future refactor may integrate this timeout directly into the
    /// checker.
    pub timeout: Duration,
    /// Number of consecutive failures before marking as [`HealthStatus::Unhealthy`].
    pub max_failures: u32,
}

impl Default for HealthCheckConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(30),
            timeout: Duration::from_secs(10),
            max_failures: 3,
        }
    }
}

/// Agent health status.
#[derive(Debug, Clone, PartialEq)]
pub enum HealthStatus {
    /// Agent is healthy and responsive.
    Healthy,
    /// Agent has experienced some failures but hasn't crossed the threshold yet.
    Degraded {
        /// Number of consecutive failed checks.
        consecutive_failures: u32,
    },
    /// Agent is unhealthy — consecutive failures exceeded the threshold.
    Unhealthy {
        /// When the Agent first became unhealthy.
        since: Instant,
    },
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Healthy => write!(f, "healthy"),
            Self::Degraded {
                consecutive_failures,
            } => {
                write!(f, "degraded ({consecutive_failures} failures)")
            }
            Self::Unhealthy { .. } => write!(f, "unhealthy"),
        }
    }
}

/// Health checker that tracks an Agent's health over time.
///
/// Call [`check`](HealthChecker::check) with the result of a liveness probe
/// (e.g. `adapter.is_alive()`). The checker aggregates consecutive failures
/// and transitions between [`HealthStatus`] variants.
pub struct HealthChecker {
    config: HealthCheckConfig,
    status: HealthStatus,
    consecutive_failures: u32,
    last_check: Option<Instant>,
}

impl HealthChecker {
    /// Create a new health checker with the given configuration.
    pub fn new(config: HealthCheckConfig) -> Self {
        Self {
            config,
            status: HealthStatus::Healthy,
            consecutive_failures: 0,
            last_check: None,
        }
    }

    /// Execute one health check.
    ///
    /// - If `is_alive` is `true`, resets consecutive failures and transitions
    ///   to [`HealthStatus::Healthy`].
    /// - If `is_alive` is `false`, increments the failure counter and
    ///   transitions to [`HealthStatus::Degraded`] or [`HealthStatus::Unhealthy`]
    ///   depending on the threshold.
    pub fn check(&mut self, is_alive: bool) -> &HealthStatus {
        self.last_check = Some(Instant::now());

        if is_alive {
            self.consecutive_failures = 0;
            self.status = HealthStatus::Healthy;
        } else {
            self.consecutive_failures += 1;
            if self.consecutive_failures >= self.config.max_failures {
                // Only set the `since` timestamp on the *first* transition to
                // Unhealthy so we know how long it has been unhealthy.
                self.status = HealthStatus::Unhealthy {
                    since: match &self.status {
                        HealthStatus::Unhealthy { since } => *since,
                        _ => Instant::now(),
                    },
                };
            } else {
                self.status = HealthStatus::Degraded {
                    consecutive_failures: self.consecutive_failures,
                };
            }
        }

        &self.status
    }

    /// Reset the checker to [`HealthStatus::Healthy`].
    pub fn reset(&mut self) {
        self.consecutive_failures = 0;
        self.status = HealthStatus::Healthy;
        self.last_check = None;
    }

    /// Returns a reference to the current [`HealthStatus`].
    pub fn status(&self) -> &HealthStatus {
        &self.status
    }

    /// Returns the number of consecutive failures.
    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    /// Returns the time of the last check, if any.
    pub fn last_check(&self) -> Option<Instant> {
        self.last_check
    }

    /// Returns a reference to the configuration.
    pub fn config(&self) -> &HealthCheckConfig {
        &self.config
    }
}

impl Default for HealthChecker {
    fn default() -> Self {
        Self::new(HealthCheckConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_checker_is_healthy() {
        let checker = HealthChecker::default();
        assert_eq!(checker.status(), &HealthStatus::Healthy);
        assert_eq!(checker.consecutive_failures(), 0);
        assert!(checker.last_check().is_none());
    }

    #[test]
    fn check_alive_stays_healthy() {
        let mut checker = HealthChecker::default();
        checker.check(true);
        assert_eq!(checker.status(), &HealthStatus::Healthy);
        assert!(checker.last_check().is_some());
    }

    #[test]
    fn check_degraded_then_healthy() {
        let mut checker = HealthChecker::new(HealthCheckConfig {
            max_failures: 3,
            ..Default::default()
        });

        // First failure → Degraded(1)
        checker.check(false);
        assert_eq!(
            checker.status(),
            &HealthStatus::Degraded {
                consecutive_failures: 1
            }
        );

        // Second failure → Degraded(2)
        checker.check(false);
        assert_eq!(
            checker.status(),
            &HealthStatus::Degraded {
                consecutive_failures: 2
            }
        );

        // Recovery → Healthy
        checker.check(true);
        assert_eq!(checker.status(), &HealthStatus::Healthy);
        assert_eq!(checker.consecutive_failures(), 0);
    }

    #[test]
    fn check_transitions_to_unhealthy() {
        let mut checker = HealthChecker::new(HealthCheckConfig {
            max_failures: 2,
            ..Default::default()
        });

        checker.check(false); // failure 1 → Degraded
        assert!(matches!(checker.status(), HealthStatus::Degraded { .. }));

        checker.check(false); // failure 2 → Unhealthy
        assert!(matches!(checker.status(), HealthStatus::Unhealthy { .. }));
    }

    #[test]
    fn unhealthy_stays_unhealthy_on_more_failures() {
        let mut checker = HealthChecker::new(HealthCheckConfig {
            max_failures: 1,
            ..Default::default()
        });

        checker.check(false); // → Unhealthy
        let since = match checker.status() {
            HealthStatus::Unhealthy { since } => *since,
            _ => panic!("expected Unhealthy"),
        };

        // Another failure — `since` should be preserved.
        checker.check(false);
        match checker.status() {
            HealthStatus::Unhealthy { since: s } => assert_eq!(*s, since),
            _ => panic!("expected Unhealthy"),
        }
    }

    #[test]
    fn unhealthy_recovers_to_healthy() {
        let mut checker = HealthChecker::new(HealthCheckConfig {
            max_failures: 1,
            ..Default::default()
        });

        checker.check(false);
        assert!(matches!(checker.status(), HealthStatus::Unhealthy { .. }));

        checker.check(true);
        assert_eq!(checker.status(), &HealthStatus::Healthy);
    }

    #[test]
    fn reset_clears_everything() {
        let mut checker = HealthChecker::new(HealthCheckConfig {
            max_failures: 1,
            ..Default::default()
        });

        checker.check(false);
        assert!(matches!(checker.status(), HealthStatus::Unhealthy { .. }));

        checker.reset();
        assert_eq!(checker.status(), &HealthStatus::Healthy);
        assert_eq!(checker.consecutive_failures(), 0);
        assert!(checker.last_check().is_none());
    }

    #[test]
    fn full_lifecycle_healthy_degraded_unhealthy_recovered() {
        let mut checker = HealthChecker::new(HealthCheckConfig {
            max_failures: 3,
            ..Default::default()
        });

        // Healthy
        assert_eq!(checker.status(), &HealthStatus::Healthy);

        // Degraded phase
        checker.check(false);
        assert_eq!(
            checker.status(),
            &HealthStatus::Degraded {
                consecutive_failures: 1
            }
        );
        checker.check(false);
        assert_eq!(
            checker.status(),
            &HealthStatus::Degraded {
                consecutive_failures: 2
            }
        );

        // Unhealthy
        checker.check(false);
        assert!(matches!(checker.status(), HealthStatus::Unhealthy { .. }));

        // Stay unhealthy
        checker.check(false);
        assert!(matches!(checker.status(), HealthStatus::Unhealthy { .. }));

        // Recovery
        checker.check(true);
        assert_eq!(checker.status(), &HealthStatus::Healthy);
        assert_eq!(checker.consecutive_failures(), 0);
    }

    #[test]
    fn health_status_display() {
        assert_eq!(HealthStatus::Healthy.to_string(), "healthy");
        assert_eq!(
            HealthStatus::Degraded {
                consecutive_failures: 2
            }
            .to_string(),
            "degraded (2 failures)"
        );
        assert_eq!(
            HealthStatus::Unhealthy {
                since: Instant::now()
            }
            .to_string(),
            "unhealthy"
        );
    }

    #[test]
    fn config_default_values() {
        let config = HealthCheckConfig::default();
        assert_eq!(config.interval, Duration::from_secs(30));
        assert_eq!(config.timeout, Duration::from_secs(10));
        assert_eq!(config.max_failures, 3);
    }
}
