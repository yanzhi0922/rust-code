//! Exponential backoff reconnect scheduler with strategy pattern.
//!
//! Manages reconnect attempts for MCP servers that have disconnected,
//! using exponential backoff with configurable parameters. Includes a
//! strategy trait for custom reconnect behavior and a circuit breaker
//! implementation. Only remote transports (SSE/HTTP/WS) are eligible
//! for automatic reconnection; stdio and SDK transports are not
//! reconnected automatically.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Default maximum reconnect attempts.
const DEFAULT_MAX_ATTEMPTS: u32 = 5;
/// Default initial backoff in milliseconds.
const DEFAULT_INITIAL_BACKOFF_MS: u64 = 1000;
/// Default maximum backoff in milliseconds.
const DEFAULT_MAX_BACKOFF_MS: u64 = 30_000;

/// Action returned when scheduling a reconnect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconnectAction {
    /// The server should attempt to connect immediately.
    ConnectNow,
    /// Wait for the specified duration before the next attempt.
    WaitFor(Duration),
    /// The maximum number of attempts has been reached; give up.
    GiveUp,
}

/// State tracking a single server's reconnect progress.
#[derive(Debug, Clone)]
pub struct ReconnectState {
    /// Current attempt number (1-based).
    pub attempt: u32,
    /// The time at which the next reconnect attempt should be made.
    pub next_attempt_at: Instant,
    /// Whether this reconnect has been aborted.
    pub aborted: bool,
}

// ── Reconnect strategy trait ──────────────────────────────────────────────────

/// Trait for custom reconnect strategies.
///
/// Implementations define how reconnect timing and attempt limits
/// are managed for different server types or failure modes.
pub trait ReconnectStrategy: Send + Sync {
    /// Compute the action for the next reconnect attempt.
    ///
    /// Called with the current attempt number (1-based) and should
    /// return the appropriate action.
    fn next_action(&self, attempt: u32) -> ReconnectAction;

    /// Record a successful connection (resets internal state).
    fn record_success(&mut self, server_name: &str);

    /// Record a failed connection attempt.
    fn record_failure(&mut self, server_name: &str);

    /// Check if the strategy allows more attempts for a server.
    fn can_retry(&self, server_name: &str) -> bool;

    /// Reset the strategy state for a specific server.
    fn reset(&mut self, server_name: &str);

    /// Get the current attempt count for a server.
    fn attempt_count(&self, server_name: &str) -> u32;
}

// ── Exponential backoff strategy ──────────────────────────────────────────────

/// Exponential backoff reconnect strategy.
///
/// Doubles the backoff duration on each consecutive failure, capped
/// at a maximum backoff. Gives up after a configurable number of
/// attempts.
#[derive(Debug, Clone)]
pub struct ExponentialBackoffReconnect {
    max_attempts: u32,
    initial_backoff: Duration,
    max_backoff: Duration,
    attempts: HashMap<String, u32>,
}

impl ExponentialBackoffReconnect {
    /// Create a new exponential backoff strategy.
    #[must_use]
    pub fn new(max_attempts: u32, initial_backoff: Duration, max_backoff: Duration) -> Self {
        Self {
            max_attempts,
            initial_backoff,
            max_backoff,
            attempts: HashMap::new(),
        }
    }

    /// Create with default parameters.
    #[must_use]
    pub fn default_strategy() -> Self {
        Self::new(
            DEFAULT_MAX_ATTEMPTS,
            Duration::from_millis(DEFAULT_INITIAL_BACKOFF_MS),
            Duration::from_millis(DEFAULT_MAX_BACKOFF_MS),
        )
    }

    /// Compute the backoff for a given attempt number.
    fn compute_backoff(&self, attempt: u32) -> Duration {
        let exponent = attempt.saturating_sub(1);
        let multiplier = 2_u64.saturating_pow(exponent);
        let backoff_ms = self
            .initial_backoff
            .as_millis()
            .saturating_mul(multiplier as u128);
        let max_ms = self.max_backoff.as_millis();
        Duration::from_millis(backoff_ms.min(max_ms) as u64)
    }
}

impl ReconnectStrategy for ExponentialBackoffReconnect {
    fn next_action(&self, attempt: u32) -> ReconnectAction {
        if attempt > self.max_attempts {
            ReconnectAction::GiveUp
        } else if attempt == 1 {
            ReconnectAction::ConnectNow
        } else {
            ReconnectAction::WaitFor(self.compute_backoff(attempt - 1))
        }
    }

    fn record_success(&mut self, server_name: &str) {
        self.attempts.remove(server_name);
    }

    fn record_failure(&mut self, server_name: &str) {
        *self.attempts.entry(server_name.to_owned()).or_insert(0) += 1;
    }

    fn can_retry(&self, server_name: &str) -> bool {
        let attempt = self.attempts.get(server_name).copied().unwrap_or(0);
        attempt < self.max_attempts
    }

    fn reset(&mut self, server_name: &str) {
        self.attempts.remove(server_name);
    }

    fn attempt_count(&self, server_name: &str) -> u32 {
        self.attempts.get(server_name).copied().unwrap_or(0)
    }
}

// ── Circuit breaker strategy ──────────────────────────────────────────────────

/// Circuit breaker reconnect strategy.
///
/// After a configurable number of consecutive failures, the circuit
/// "opens" and prevents further attempts for a cooldown period.
/// After the cooldown, a single "half-open" attempt is allowed. If
/// it succeeds, the circuit closes. If it fails, the cooldown
/// restarts.
#[derive(Debug, Clone)]
pub struct CircuitBreakerReconnect {
    /// Number of failures before opening the circuit.
    failure_threshold: u32,
    /// Duration to wait in the open state before allowing a half-open attempt.
    cooldown: Duration,
    /// Backoff for individual retry attempts (before circuit opens).
    retry_backoff: Duration,
    /// Maximum total attempts across all phases.
    max_total_attempts: u32,
    /// Per-server state.
    states: HashMap<String, CircuitBreakerState>,
}

/// State for a single server in the circuit breaker.
#[derive(Debug, Clone)]
struct CircuitBreakerState {
    /// Consecutive failure count.
    consecutive_failures: u32,
    /// Total attempts made.
    total_attempts: u32,
    /// When the circuit opened (if open).
    opened_at: Option<Instant>,
    /// Whether in half-open state.
    half_open: bool,
    /// Whether permanently failed.
    given_up: bool,
}

impl CircuitBreakerState {
    fn new() -> Self {
        Self {
            consecutive_failures: 0,
            total_attempts: 0,
            opened_at: None,
            half_open: false,
            given_up: false,
        }
    }

    /// Whether the circuit is currently open (in cooldown).
    fn is_open(&self) -> bool {
        self.opened_at.is_some() && !self.half_open
    }
}

/// Circuit breaker state for external observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Circuit is closed — normal operation.
    Closed,
    /// Circuit is open — in cooldown.
    Open,
    /// Circuit is half-open — allowing a single test attempt.
    HalfOpen,
}

impl CircuitBreakerReconnect {
    /// Create a new circuit breaker strategy.
    #[must_use]
    pub fn new(
        failure_threshold: u32,
        cooldown: Duration,
        retry_backoff: Duration,
        max_total_attempts: u32,
    ) -> Self {
        Self {
            failure_threshold,
            cooldown,
            retry_backoff,
            max_total_attempts,
            states: HashMap::new(),
        }
    }

    /// Create with sensible defaults.
    #[must_use]
    pub fn default_strategy() -> Self {
        Self::new(3, Duration::from_secs(30), Duration::from_secs(2), 20)
    }

    /// Get the circuit state for a server.
    #[must_use]
    pub fn circuit_state(&self, server_name: &str) -> CircuitState {
        let state = self.states.get(server_name);
        match state {
            None => CircuitState::Closed,
            Some(s) if s.half_open => CircuitState::HalfOpen,
            Some(s) if s.is_open() => CircuitState::Open,
            _ => CircuitState::Closed,
        }
    }
}

impl ReconnectStrategy for CircuitBreakerReconnect {
    /// Compute the next reconnect action based on the attempt number.
    ///
    /// **Note:** This method only uses the attempt number and does **not**
    /// consult per-server circuit breaker state (cooldown, half-open, etc.).
    /// For per-server-aware reconnect decisions, use
    /// [`CircuitBreakerReconnect::next_action_for_server`] instead, which
    /// checks the circuit breaker state for a specific server before falling
    /// back to this generic logic.
    ///
    /// Callers that track multiple servers should prefer
    /// `next_action_for_server` to avoid reconnecting to a server whose
    /// circuit is still in the open (cooldown) state.
    fn next_action(&self, attempt: u32) -> ReconnectAction {
        if attempt > self.max_total_attempts {
            ReconnectAction::GiveUp
        } else if attempt == 1 {
            ReconnectAction::ConnectNow
        } else {
            ReconnectAction::WaitFor(self.retry_backoff)
        }
    }

    fn record_success(&mut self, server_name: &str) {
        if let Some(state) = self.states.get_mut(server_name) {
            state.consecutive_failures = 0;
            state.half_open = false;
            state.opened_at = None;
            state.given_up = false;
        }
    }

    fn record_failure(&mut self, server_name: &str) {
        let state = self
            .states
            .entry(server_name.to_owned())
            .or_insert_with(CircuitBreakerState::new);

        state.consecutive_failures += 1;
        state.total_attempts += 1;

        if state.half_open {
            // Half-open attempt failed — reopen circuit
            state.half_open = false;
            state.opened_at = Some(Instant::now());
        }

        if state.consecutive_failures >= self.failure_threshold && state.opened_at.is_none() {
            // Threshold reached — open the circuit
            state.opened_at = Some(Instant::now());
        }

        if state.total_attempts >= self.max_total_attempts {
            state.given_up = true;
        }
    }

    fn can_retry(&self, server_name: &str) -> bool {
        let state = self.states.get(server_name);
        match state {
            None => true,
            Some(s) if s.given_up => false,
            Some(s) if s.is_open() => {
                // Check if cooldown has elapsed
                match s.opened_at {
                    Some(opened) => opened.elapsed() >= self.cooldown,
                    None => false,
                }
            }
            Some(s) if s.half_open => true,
            Some(s) => s.total_attempts < self.max_total_attempts,
        }
    }

    fn reset(&mut self, server_name: &str) {
        self.states.remove(server_name);
    }

    fn attempt_count(&self, server_name: &str) -> u32 {
        self.states
            .get(server_name)
            .map(|s| s.total_attempts)
            .unwrap_or(0)
    }
}

impl CircuitBreakerReconnect {
    /// Compute the next reconnect action for a specific server, taking its
    /// per-server circuit breaker state into account.
    ///
    /// If the circuit for `server_name` is open and the cooldown has not
    /// elapsed, returns [`ReconnectAction::WaitFor`] with the remaining
    /// cooldown. If the circuit is half-open, returns
    /// [`ReconnectAction::ConnectNow`] for a single probe attempt.
    /// Otherwise falls back to the attempt-based logic from
    /// [`ReconnectStrategy::next_action`].
    pub fn next_action_for_server(&self, server_name: &str, attempt: u32) -> ReconnectAction {
        if let Some(state) = self.states.get(server_name) {
            if state.given_up {
                return ReconnectAction::GiveUp;
            }
            if state.is_open() {
                // Circuit is open — check cooldown
                if let Some(opened_at) = state.opened_at {
                    let elapsed = opened_at.elapsed();
                    if elapsed < self.cooldown {
                        return ReconnectAction::WaitFor(self.cooldown - elapsed);
                    }
                    // Cooldown elapsed — allow a half-open probe
                    return ReconnectAction::ConnectNow;
                }
            }
            if state.half_open {
                return ReconnectAction::ConnectNow;
            }
        }
        // No per-server state or circuit is closed — use generic logic
        self.next_action(attempt)
    }
}

// ── Reconnect scheduler ──────────────────────────────────────────────────────

/// Exponential backoff reconnect scheduler.
#[derive(Debug)]
pub struct ReconnectScheduler {
    max_attempts: u32,
    initial_backoff_ms: u64,
    max_backoff_ms: u64,
    pending: HashMap<String, ReconnectState>,
}

impl ReconnectScheduler {
    /// Create a new scheduler with default parameters.
    ///
    /// Defaults: max 5 attempts, 1 s initial backoff, 30 s max backoff.
    #[must_use]
    pub fn new() -> Self {
        Self {
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            initial_backoff_ms: DEFAULT_INITIAL_BACKOFF_MS,
            max_backoff_ms: DEFAULT_MAX_BACKOFF_MS,
            pending: HashMap::new(),
        }
    }

    /// Create a new scheduler with custom parameters.
    #[must_use]
    pub fn with_params(max_attempts: u32, initial_backoff_ms: u64, max_backoff_ms: u64) -> Self {
        Self {
            max_attempts,
            initial_backoff_ms,
            max_backoff_ms,
            pending: HashMap::new(),
        }
    }

    /// Schedule a reconnect for a server.
    ///
    /// If this is the first attempt, returns [`ReconnectAction::ConnectNow`].
    /// If the server has previous failed attempts, returns
    /// [`ReconnectAction::WaitFor`] with the backoff duration.
    /// If the maximum attempts have been exceeded, returns
    /// [`ReconnectAction::GiveUp`].
    pub fn schedule_reconnect(&mut self, server_name: String) -> ReconnectAction {
        let state = self
            .pending
            .entry(server_name.clone())
            .or_insert_with(|| ReconnectState {
                attempt: 0,
                next_attempt_at: Instant::now(),
                aborted: false,
            });

        if state.aborted {
            return ReconnectAction::GiveUp;
        }

        state.attempt += 1;

        if state.attempt > self.max_attempts {
            return ReconnectAction::GiveUp;
        }

        if state.attempt == 1 {
            state.next_attempt_at = Instant::now();
            ReconnectAction::ConnectNow
        } else {
            // Compute backoff before borrowing to avoid borrow conflicts.
            let backoff = compute_backoff(
                state.attempt - 1,
                self.initial_backoff_ms,
                self.max_backoff_ms,
            );
            state.next_attempt_at = Instant::now() + backoff;
            ReconnectAction::WaitFor(backoff)
        }
    }

    /// Report that a reconnect succeeded. Removes the server from the
    /// pending set.
    pub fn report_success(&mut self, server_name: &str) {
        self.pending.remove(server_name);
    }

    /// Report that a reconnect failed. Returns the duration to wait before
    /// the next attempt, or `None` if the maximum attempts have been exceeded.
    pub fn report_failure(&self, server_name: &str) -> Option<Duration> {
        let state = self.pending.get(server_name)?;
        if state.attempt >= self.max_attempts || state.aborted {
            None
        } else {
            Some(compute_backoff(
                state.attempt,
                self.initial_backoff_ms,
                self.max_backoff_ms,
            ))
        }
    }

    /// Cancel reconnect for a specific server.
    pub fn cancel(&mut self, server_name: &str) {
        if let Some(state) = self.pending.get_mut(server_name) {
            state.aborted = true;
        }
    }

    /// Cancel all pending reconnects.
    pub fn cancel_all(&mut self) {
        for state in self.pending.values_mut() {
            state.aborted = true;
        }
    }

    /// Return `true` if the server has a pending (non-aborted) reconnect.
    #[must_use]
    pub fn is_reconnecting(&self, server_name: &str) -> bool {
        self.pending
            .get(server_name)
            .is_some_and(|s| !s.aborted && s.attempt <= self.max_attempts)
    }

    /// Return the reconnect state for a server, if any.
    #[must_use]
    pub fn reconnect_state(&self, server_name: &str) -> Option<&ReconnectState> {
        self.pending.get(server_name)
    }

    /// Return the number of servers with pending reconnects.
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.pending.values().filter(|s| !s.aborted).count()
    }
}

/// Compute the backoff duration for a given attempt number (1-based).
fn compute_backoff(attempt: u32, initial_backoff_ms: u64, max_backoff_ms: u64) -> Duration {
    let exponent = attempt.saturating_sub(1);
    let multiplier = 2_u64.saturating_pow(exponent);
    let backoff_ms = initial_backoff_ms
        .saturating_mul(multiplier)
        .min(max_backoff_ms);
    Duration::from_millis(backoff_ms)
}

impl Default for ReconnectScheduler {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── ReconnectScheduler tests (original) ───────────────────────────────

    #[test]
    fn first_attempt_returns_connect_now() {
        let mut scheduler = ReconnectScheduler::new();
        let action = scheduler.schedule_reconnect("test-server".to_owned());
        assert_eq!(action, ReconnectAction::ConnectNow);
    }

    #[test]
    fn second_attempt_returns_wait() {
        let mut scheduler = ReconnectScheduler::new();
        scheduler.schedule_reconnect("test-server".to_owned());
        let action = scheduler.schedule_reconnect("test-server".to_owned());
        match action {
            ReconnectAction::WaitFor(d) => assert_eq!(d, Duration::from_secs(1)),
            other => panic!("expected WaitFor, got {other:?}"),
        }
    }

    #[test]
    fn max_attempts_returns_give_up() {
        let mut scheduler = ReconnectScheduler::with_params(2, 100, 1000);
        scheduler.schedule_reconnect("srv".to_owned()); // attempt 1
        scheduler.schedule_reconnect("srv".to_owned()); // attempt 2
        let action = scheduler.schedule_reconnect("srv".to_owned()); // attempt 3 > max
        assert_eq!(action, ReconnectAction::GiveUp);
    }

    #[test]
    fn report_success_removes_state() {
        let mut scheduler = ReconnectScheduler::new();
        scheduler.schedule_reconnect("srv".to_owned());
        assert!(scheduler.is_reconnecting("srv"));
        scheduler.report_success("srv");
        assert!(!scheduler.is_reconnecting("srv"));
        assert!(scheduler.reconnect_state("srv").is_none());
    }

    #[test]
    fn report_failure_returns_next_backoff() {
        let mut scheduler = ReconnectScheduler::new();
        scheduler.schedule_reconnect("srv".to_owned());
        let next = scheduler.report_failure("srv");
        assert!(next.is_some());
        assert_eq!(next, Some(Duration::from_secs(1)));
    }

    #[test]
    fn cancel_marks_aborted() {
        let mut scheduler = ReconnectScheduler::new();
        scheduler.schedule_reconnect("srv".to_owned());
        assert!(scheduler.is_reconnecting("srv"));
        scheduler.cancel("srv");
        assert!(!scheduler.is_reconnecting("srv"));
        let state = scheduler.reconnect_state("srv").expect("state exists");
        assert!(state.aborted);
    }

    #[test]
    fn cancel_all_aborts_everything() {
        let mut scheduler = ReconnectScheduler::new();
        scheduler.schedule_reconnect("a".to_owned());
        scheduler.schedule_reconnect("b".to_owned());
        scheduler.cancel_all();
        assert!(!scheduler.is_reconnecting("a"));
        assert!(!scheduler.is_reconnecting("b"));
        assert_eq!(scheduler.pending_count(), 0);
    }

    #[test]
    fn backoff_increases_exponentially() {
        assert_eq!(
            compute_backoff(1, 1000, 30_000),
            Duration::from_millis(1000)
        );
        assert_eq!(
            compute_backoff(2, 1000, 30_000),
            Duration::from_millis(2000)
        );
        assert_eq!(
            compute_backoff(3, 1000, 30_000),
            Duration::from_millis(4000)
        );
        assert_eq!(
            compute_backoff(4, 1000, 30_000),
            Duration::from_millis(8000)
        );
        assert_eq!(
            compute_backoff(5, 1000, 30_000),
            Duration::from_millis(16_000)
        );
        assert_eq!(
            compute_backoff(6, 1000, 30_000),
            Duration::from_millis(30_000)
        ); // capped
    }

    #[test]
    fn default_scheduler_has_expected_params() {
        let scheduler = ReconnectScheduler::default();
        assert_eq!(scheduler.max_attempts, 5);
        assert_eq!(scheduler.initial_backoff_ms, 1000);
        assert_eq!(scheduler.max_backoff_ms, 30_000);
        assert_eq!(scheduler.pending_count(), 0);
    }

    #[test]
    fn cancelled_server_schedule_returns_give_up() {
        let mut scheduler = ReconnectScheduler::new();
        scheduler.schedule_reconnect("srv".to_owned());
        scheduler.cancel("srv");
        let action = scheduler.schedule_reconnect("srv".to_owned());
        assert_eq!(action, ReconnectAction::GiveUp);
    }

    #[test]
    fn report_failure_after_max_attempts_returns_none() {
        let mut scheduler = ReconnectScheduler::with_params(1, 100, 1000);
        scheduler.schedule_reconnect("srv".to_owned()); // attempt 1
        let next = scheduler.report_failure("srv");
        assert!(next.is_none());
    }

    // ── ExponentialBackoffReconnect strategy tests ────────────────────────

    #[test]
    fn exp_backoff_first_attempt_connect_now() {
        let strategy = ExponentialBackoffReconnect::default_strategy();
        assert_eq!(strategy.next_action(1), ReconnectAction::ConnectNow);
    }

    #[test]
    fn exp_backoff_second_attempt_waits() {
        let strategy =
            ExponentialBackoffReconnect::new(5, Duration::from_secs(1), Duration::from_secs(30));
        match strategy.next_action(2) {
            ReconnectAction::WaitFor(d) => assert_eq!(d, Duration::from_secs(1)),
            other => panic!("expected WaitFor, got {other:?}"),
        }
    }

    #[test]
    fn exp_backoff_exceeds_max_gives_up() {
        let strategy =
            ExponentialBackoffReconnect::new(3, Duration::from_secs(1), Duration::from_secs(30));
        assert_eq!(strategy.next_action(4), ReconnectAction::GiveUp);
    }

    #[test]
    fn exp_backoff_record_success_resets() {
        let mut strategy = ExponentialBackoffReconnect::default_strategy();
        strategy.record_failure("srv");
        strategy.record_failure("srv");
        assert_eq!(strategy.attempt_count("srv"), 2);
        strategy.record_success("srv");
        assert_eq!(strategy.attempt_count("srv"), 0);
    }

    #[test]
    fn exp_backoff_can_retry_under_limit() {
        let mut strategy =
            ExponentialBackoffReconnect::new(3, Duration::from_secs(1), Duration::from_secs(30));
        assert!(strategy.can_retry("srv"));
        strategy.record_failure("srv");
        assert!(strategy.can_retry("srv"));
        strategy.record_failure("srv");
        assert!(strategy.can_retry("srv"));
        strategy.record_failure("srv");
        assert!(!strategy.can_retry("srv"));
    }

    #[test]
    fn exp_backoff_reset_clears_state() {
        let mut strategy = ExponentialBackoffReconnect::default_strategy();
        strategy.record_failure("srv");
        strategy.record_failure("srv");
        strategy.reset("srv");
        assert_eq!(strategy.attempt_count("srv"), 0);
        assert!(strategy.can_retry("srv"));
    }

    #[test]
    fn exp_backoff_backoff_caps_at_max() {
        let strategy =
            ExponentialBackoffReconnect::new(10, Duration::from_secs(1), Duration::from_secs(5));
        match strategy.next_action(10) {
            ReconnectAction::WaitFor(d) => assert!(d <= Duration::from_secs(5)),
            other => panic!("expected WaitFor, got {other:?}"),
        }
    }

    // ── CircuitBreakerReconnect strategy tests ────────────────────────────

    #[test]
    fn circuit_breaker_starts_closed() {
        let cb = CircuitBreakerReconnect::default_strategy();
        assert_eq!(cb.circuit_state("srv"), CircuitState::Closed);
    }

    #[test]
    fn circuit_breaker_opens_after_threshold() {
        let mut cb =
            CircuitBreakerReconnect::new(3, Duration::from_secs(30), Duration::from_secs(2), 20);
        cb.record_failure("srv");
        cb.record_failure("srv");
        assert_eq!(cb.circuit_state("srv"), CircuitState::Closed);
        cb.record_failure("srv"); // hits threshold
        assert_eq!(cb.circuit_state("srv"), CircuitState::Open);
    }

    #[test]
    fn circuit_breaker_success_closes() {
        let mut cb =
            CircuitBreakerReconnect::new(2, Duration::from_secs(30), Duration::from_secs(2), 20);
        cb.record_failure("srv");
        cb.record_failure("srv");
        assert_eq!(cb.circuit_state("srv"), CircuitState::Open);
        cb.record_success("srv");
        assert_eq!(cb.circuit_state("srv"), CircuitState::Closed);
    }

    #[test]
    fn circuit_breaker_can_retry_when_closed() {
        let cb = CircuitBreakerReconnect::default_strategy();
        assert!(cb.can_retry("srv"));
    }

    #[test]
    fn circuit_breaker_gives_up_after_max_total() {
        let mut cb = CircuitBreakerReconnect::new(
            100, // high threshold so circuit never opens
            Duration::from_secs(30),
            Duration::from_secs(2),
            3, // low max total
        );
        cb.record_failure("srv");
        cb.record_failure("srv");
        cb.record_failure("srv");
        assert!(!cb.can_retry("srv"));
    }

    #[test]
    fn circuit_breaker_attempt_count() {
        let mut cb = CircuitBreakerReconnect::default_strategy();
        assert_eq!(cb.attempt_count("srv"), 0);
        cb.record_failure("srv");
        assert_eq!(cb.attempt_count("srv"), 1);
        cb.record_failure("srv");
        assert_eq!(cb.attempt_count("srv"), 2);
    }

    #[test]
    fn circuit_breaker_reset_clears() {
        let mut cb = CircuitBreakerReconnect::default_strategy();
        cb.record_failure("srv");
        cb.record_failure("srv");
        cb.reset("srv");
        assert_eq!(cb.circuit_state("srv"), CircuitState::Closed);
        assert_eq!(cb.attempt_count("srv"), 0);
    }

    #[test]
    fn circuit_breaker_next_action() {
        let cb = CircuitBreakerReconnect::default_strategy();
        assert_eq!(cb.next_action(1), ReconnectAction::ConnectNow);
        assert!(matches!(cb.next_action(2), ReconnectAction::WaitFor(_)));
        assert_eq!(cb.next_action(100), ReconnectAction::GiveUp);
    }

    #[test]
    fn circuit_breaker_half_open_after_cooldown() {
        let mut cb = CircuitBreakerReconnect::new(
            1,
            Duration::from_nanos(1), // instant cooldown
            Duration::from_secs(2),
            20,
        );
        cb.record_failure("srv"); // opens circuit
        assert_eq!(cb.circuit_state("srv"), CircuitState::Open);
        // After cooldown, can_retry should return true
        assert!(cb.can_retry("srv"));
    }

    #[test]
    fn circuit_breaker_reopen_on_half_open_failure() {
        let mut cb = CircuitBreakerReconnect::new(
            1,
            Duration::from_nanos(1), // instant cooldown
            Duration::from_secs(2),
            20,
        );
        cb.record_failure("srv"); // opens circuit
        assert_eq!(cb.circuit_state("srv"), CircuitState::Open);

        // Simulate half-open: manually set state
        if let Some(state) = cb.states.get_mut("srv") {
            state.half_open = true;
        }
        assert_eq!(cb.circuit_state("srv"), CircuitState::HalfOpen);

        // Fail the half-open attempt
        cb.record_failure("srv");
        assert_eq!(cb.circuit_state("srv"), CircuitState::Open);
    }

    #[test]
    fn circuit_breaker_default_strategy() {
        let cb = CircuitBreakerReconnect::default_strategy();
        assert_eq!(cb.failure_threshold, 3);
        assert_eq!(cb.cooldown, Duration::from_secs(30));
        assert_eq!(cb.max_total_attempts, 20);
    }
}
