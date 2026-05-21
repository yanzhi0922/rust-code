//! Circuit breaker for provider fault tolerance.
//!
//! Implements the classic three-state circuit breaker pattern to prevent
//! wasting time on requests to a provider that is known to be down.
//!
//! State machine:
//! ```text
//! CLOSED ──(N consecutive failures)──▶ OPEN
//!    ▲                                  │
//!    │                                  │ (recovery timeout elapses)
//!    │                                  ▼
//!    └────(success)──────────── HALF_OPEN
//!                                      │
//!                                (failure)
//!                                      │
//!                                      ▼
//!                                    OPEN
//! ```

use parking_lot::Mutex;
use std::time::{Duration, Instant};

/// Configuration for a circuit breaker instance.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures before opening the circuit.
    pub failure_threshold: u32,
    /// How long to wait in the OPEN state before transitioning to HALF_OPEN.
    pub recovery_timeout: Duration,
    /// Maximum number of probe requests allowed in HALF_OPEN state.
    pub half_open_max_probes: u32,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            recovery_timeout: Duration::from_secs(30),
            half_open_max_probes: 1,
        }
    }
}

/// Circuit breaker state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation — requests pass through.
    Closed,
    /// Provider is considered down — requests are rejected immediately.
    Open,
    /// Probing the provider to check if it has recovered.
    HalfOpen,
}

/// Internal state machine data.
#[derive(Debug)]
enum InternalState {
    Closed { failure_count: u32 },
    Open { opened_at: Instant },
    HalfOpen { probe_count: u32 },
}

/// A circuit breaker that protects against cascading failures.
///
/// Thread-safe via internal `Mutex`. Uses `unwrap_or_else` on the mutex to
/// recover from poisoning rather than panicking.
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    state: Mutex<InternalState>,
}

impl CircuitBreaker {
    /// Create a new circuit breaker with the given configuration.
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            state: Mutex::new(InternalState::Closed { failure_count: 0 }),
        }
    }

    /// Create a new circuit breaker with default configuration.
    pub fn new_default() -> Self {
        Self::new(CircuitBreakerConfig::default())
    }

    /// Check if a request is allowed to proceed.
    ///
    /// Returns `Ok(())` if the circuit is CLOSED or HALF_OPEN (probing).
    /// Returns `Err` with the current state if the circuit is OPEN.
    ///
    /// # Errors
    /// Returns an error if the circuit is OPEN and the recovery timeout has
    /// not yet elapsed.
    pub fn allow_request(&self) -> Result<(), CircuitState> {
        let mut state = self.state.lock();
        match &mut *state {
            InternalState::Closed { .. } => Ok(()),
            InternalState::Open { opened_at } => {
                if opened_at.elapsed() >= self.config.recovery_timeout {
                    // Transition to HALF_OPEN — allow a probe request.
                    *state = InternalState::HalfOpen { probe_count: 0 };
                    Ok(())
                } else {
                    Err(CircuitState::Open)
                }
            }
            InternalState::HalfOpen { probe_count } => {
                if *probe_count < self.config.half_open_max_probes {
                    *probe_count += 1;
                    Ok(())
                } else {
                    Err(CircuitState::HalfOpen)
                }
            }
        }
    }

    /// Record a successful request.
    ///
    /// Transitions HALF_OPEN → CLOSED on success.
    pub fn record_success(&self) {
        let mut state = self.state.lock();
        *state = InternalState::Closed { failure_count: 0 };
    }

    /// Record a failed request.
    ///
    /// Increments the failure counter in CLOSED state, and transitions
    /// to OPEN when the threshold is reached. In HALF_OPEN, immediately
    /// transitions back to OPEN.
    pub fn record_failure(&self) {
        let mut state = self.state.lock();
        match &mut *state {
            InternalState::Closed { failure_count } => {
                *failure_count += 1;
                if *failure_count >= self.config.failure_threshold {
                    *state = InternalState::Open {
                        opened_at: Instant::now(),
                    };
                }
            }
            InternalState::HalfOpen { .. } => {
                // Probe failed — go back to OPEN.
                *state = InternalState::Open {
                    opened_at: Instant::now(),
                };
            }
            InternalState::Open { .. } => {
                // Already open — reset the timer.
                *state = InternalState::Open {
                    opened_at: Instant::now(),
                };
            }
        }
    }

    /// Get the current circuit state.
    pub fn state(&self) -> CircuitState {
        let state = self.state.lock();
        match &*state {
            InternalState::Closed { .. } => CircuitState::Closed,
            InternalState::Open { opened_at } => {
                if opened_at.elapsed() >= self.config.recovery_timeout {
                    CircuitState::HalfOpen
                } else {
                    CircuitState::Open
                }
            }
            InternalState::HalfOpen { .. } => CircuitState::HalfOpen,
        }
    }

    /// Reset the circuit breaker to CLOSED state.
    pub fn reset(&self) {
        let mut state = self.state.lock();
        *state = InternalState::Closed { failure_count: 0 };
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn circuit_starts_closed() {
        let cb = CircuitBreaker::new_default();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.allow_request().is_ok());
    }

    #[test]
    fn circuit_opens_after_threshold() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 3,
            recovery_timeout: Duration::from_secs(60),
            half_open_max_probes: 1,
        });
        assert!(cb.allow_request().is_ok());
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(cb.allow_request().is_err());
    }

    #[test]
    fn circuit_half_open_after_recovery_timeout() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            recovery_timeout: Duration::from_millis(10),
            half_open_max_probes: 1,
        });
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        // Wait for recovery timeout.
        std::thread::sleep(Duration::from_millis(20));
        // Now it should transition to HALF_OPEN and allow a probe.
        assert!(cb.allow_request().is_ok());
        assert_eq!(cb.state(), CircuitState::HalfOpen);
    }

    #[test]
    fn circuit_closes_on_success_after_half_open() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            recovery_timeout: Duration::from_millis(10),
            half_open_max_probes: 1,
        });
        cb.record_failure();
        std::thread::sleep(Duration::from_millis(20));
        assert!(cb.allow_request().is_ok());
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn circuit_reopens_on_failure_in_half_open() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            recovery_timeout: Duration::from_millis(10),
            half_open_max_probes: 1,
        });
        cb.record_failure();
        std::thread::sleep(Duration::from_millis(20));
        assert!(cb.allow_request().is_ok());
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn reset_returns_to_closed() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            recovery_timeout: Duration::from_secs(60),
            half_open_max_probes: 1,
        });
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        cb.reset();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn success_resets_failure_count() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 3,
            recovery_timeout: Duration::from_secs(60),
            half_open_max_probes: 1,
        });
        cb.record_failure();
        cb.record_failure();
        cb.record_success(); // resets count
        cb.record_failure();
        cb.record_failure();
        // Still closed — only 2 consecutive failures after reset.
        assert_eq!(cb.state(), CircuitState::Closed);
    }
}
