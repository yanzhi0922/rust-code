//! Shared environment variable readers for the Claude Code runtime.
//!
//! Each reader is a small, testable function that reads a well-known env var
//! (or a set of equivalent env vars) and returns the parsed value.  The
//! readers are intentionally stateless and cheap — they call `std::env::var`
//! directly on every invocation so that runtime overrides applied after
//! process start-up are visible.
//!
//! # Convention
//!
//! Where the upstream TypeScript implementation reads an env var like
//! `CLAUDE_CODE_TEMPERATURE`, we also check the `REMOTE_CODE_`-prefixed
//! equivalent so that operators can use a single namespace if they prefer.

use std::env;

// ---------------------------------------------------------------------------
// Generic helpers
// ---------------------------------------------------------------------------

/// Check whether a string value looks truthy (`1`, `true`, `yes`, `on`).
fn is_truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Read the first non-empty value from a list of env var names.
fn read_first(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        env::var(key).ok().and_then(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_owned())
            }
        })
    })
}

// ---------------------------------------------------------------------------
// Gap 1: Temperature
// ---------------------------------------------------------------------------

/// Read the `CLAUDE_CODE_TEMPERATURE` env var (or `REMOTE_CODE_TEMPERATURE`).
///
/// Returns `None` when the variable is unset or cannot be parsed as `f64`.
pub fn temperature() -> Option<f64> {
    read_first(&["CLAUDE_CODE_TEMPERATURE", "REMOTE_CODE_TEMPERATURE"])
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|&t| (0.0..=2.0).contains(&t))
}

// ---------------------------------------------------------------------------
// Gap 2: top_p and top_k
// ---------------------------------------------------------------------------

/// Read the `CLAUDE_CODE_TOP_P` env var (or `REMOTE_CODE_TOP_P`).
///
/// Returns `None` when the variable is unset or cannot be parsed as `f64`.
pub fn top_p() -> Option<f64> {
    read_first(&["CLAUDE_CODE_TOP_P", "REMOTE_CODE_TOP_P"])
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|&p| (0.0..=1.0).contains(&p))
}

/// Read the `CLAUDE_CODE_TOP_K` env var (or `REMOTE_CODE_TOP_K`).
///
/// Returns `None` when the variable is unset or cannot be parsed as `u32`.
pub fn top_k() -> Option<u32> {
    read_first(&["CLAUDE_CODE_TOP_K", "REMOTE_CODE_TOP_K"])
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|&k| k > 0)
}

// ---------------------------------------------------------------------------
// Gap 3: CLAUDE_CODE_EFFORT_LEVEL
// ---------------------------------------------------------------------------

/// Read the `CLAUDE_CODE_EFFORT_LEVEL` env var (or `REMOTE_CODE_EFFORT`).
///
/// Returns `None` when the variable is unset.
pub fn effort_level() -> Option<String> {
    read_first(&["CLAUDE_CODE_EFFORT_LEVEL", "REMOTE_CODE_EFFORT"])
}

// ---------------------------------------------------------------------------
// Gap 4: CLAUDE_CODE_MAX_OUTPUT_TOKENS
// ---------------------------------------------------------------------------

/// Read the `CLAUDE_CODE_MAX_OUTPUT_TOKENS` env var.
///
/// Returns `None` when the variable is unset or cannot be parsed as `u32`.
pub fn max_output_tokens() -> Option<u32> {
    read_first(&[
        "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
        "REMOTE_CODE_MAX_OUTPUT_TOKENS",
    ])
    .and_then(|value| value.parse::<u32>().ok())
    .filter(|&t| t >= 256)
}

// ---------------------------------------------------------------------------
// Gap 5: CLAUDE_CODE_DISABLE_THINKING
// ---------------------------------------------------------------------------

/// Check whether extended thinking should be disabled.
///
/// When the `CLAUDE_CODE_DISABLE_THINKING` (or `REMOTE_CODE_DISABLE_THINKING`)
/// env var is set to a truthy value, extended thinking is turned off.
pub fn disable_thinking() -> bool {
    read_first(&[
        "CLAUDE_CODE_DISABLE_THINKING",
        "REMOTE_CODE_DISABLE_THINKING",
    ])
    .as_deref()
    .is_some_and(is_truthy)
}

// ---------------------------------------------------------------------------
// Gap 6: DISABLE_INTERLEAVED_THINKING
// ---------------------------------------------------------------------------

/// Check whether interleaved thinking mode should be disabled.
///
/// When `DISABLE_INTERLEAVED_THINKING` is set to a truthy value, the
/// non-interleaved (sequential) thinking mode is used instead.
pub fn disable_interleaved_thinking() -> bool {
    read_first(&["DISABLE_INTERLEAVED_THINKING"])
        .as_deref()
        .is_some_and(is_truthy)
}

/// Check whether adaptive thinking should be disabled.
///
/// When `CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING` is set to a truthy value,
/// the budget-based thinking mode is used instead of adaptive thinking
/// even for models that support it.
pub fn disable_adaptive_thinking() -> bool {
    read_first(&["CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING"])
        .as_deref()
        .is_some_and(is_truthy)
}

// ---------------------------------------------------------------------------
// Gap 7: USER_TYPE
// ---------------------------------------------------------------------------

/// Read the `USER_TYPE` env var.
///
/// Returns `"ant"` for internal users, `"external"` (or any other value) for
/// external users.  Returns `None` when the variable is unset.
pub fn user_type() -> Option<String> {
    read_first(&["USER_TYPE"])
}

/// Check whether the current user is an internal ("ant") user.
pub fn is_ant_user() -> bool {
    user_type().as_deref() == Some("ant")
}

// ---------------------------------------------------------------------------
// Gap 8: DISABLE_COST_WARNINGS
// ---------------------------------------------------------------------------

/// Check whether cost/billing warnings should be suppressed.
///
/// When `DISABLE_COST_WARNINGS` (or `REMOTE_CODE_DISABLE_COST_WARNINGS`) is
/// set to a truthy value, cost warnings are silenced.
pub fn disable_cost_warnings() -> bool {
    read_first(&["DISABLE_COST_WARNINGS", "REMOTE_CODE_DISABLE_COST_WARNINGS"])
        .as_deref()
        .is_some_and(is_truthy)
}

// ---------------------------------------------------------------------------
// Gap 9: CLAUDE_CODE_DISABLE_BACKGROUND_TASKS
// ---------------------------------------------------------------------------

/// Check whether background tasks are disabled.
///
/// When `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS` (or
/// `REMOTE_CODE_DISABLE_BACKGROUND_TASKS`) is set to a truthy value,
/// background task execution is suppressed.
pub fn disable_background_tasks() -> bool {
    read_first(&[
        "CLAUDE_CODE_DISABLE_BACKGROUND_TASKS",
        "REMOTE_CODE_DISABLE_BACKGROUND_TASKS",
    ])
    .as_deref()
    .is_some_and(is_truthy)
}

// ---------------------------------------------------------------------------
// Gap 10: CLAUDE_CODE_REMOTE
// ---------------------------------------------------------------------------

/// Check whether the session is running in remote/cloud mode.
///
/// When `CLAUDE_CODE_REMOTE` (or `REMOTE_CODE_REMOTE`) is set to a truthy
/// value, the session is considered a remote session.
pub fn is_remote() -> bool {
    read_first(&["CLAUDE_CODE_REMOTE", "REMOTE_CODE_REMOTE"])
        .as_deref()
        .is_some_and(is_truthy)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Helper to temporarily set an env var for a test.
    /// Uses a mutex to serialise env mutations across test threads.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        keys: Vec<&'static str>,
        originals: Vec<Option<String>>,
    }

    impl EnvGuard {
        fn new(keys: Vec<&'static str>) -> Self {
            let originals = keys.iter().map(|key| env::var(key).ok()).collect();
            Self { keys, originals }
        }

        fn set(&self, key: &str, value: &str) {
            unsafe {
                env::set_var(key, value);
            }
        }

        fn remove(&self, key: &str) {
            unsafe {
                env::remove_var(key);
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, original) in self.keys.iter().zip(&self.originals) {
                unsafe {
                    match original {
                        Some(value) => env::set_var(key, value),
                        None => env::remove_var(key),
                    }
                }
            }
        }
    }

    #[test]
    fn temperature_parses_valid_value() {
        let _lock = ENV_LOCK.lock().expect("env test lock poisoned");
        let guard = EnvGuard::new(vec!["CLAUDE_CODE_TEMPERATURE", "REMOTE_CODE_TEMPERATURE"]);
        guard.set("CLAUDE_CODE_TEMPERATURE", "0.5");
        assert_eq!(temperature(), Some(0.5));
        drop(guard);
    }

    #[test]
    fn temperature_rejects_out_of_range() {
        let _lock = ENV_LOCK.lock().expect("env test lock poisoned");
        let guard = EnvGuard::new(vec!["CLAUDE_CODE_TEMPERATURE", "REMOTE_CODE_TEMPERATURE"]);
        guard.set("CLAUDE_CODE_TEMPERATURE", "3.0");
        assert_eq!(temperature(), None);
        drop(guard);
    }

    #[test]
    fn temperature_returns_none_when_unset() {
        let _lock = ENV_LOCK.lock().expect("env test lock poisoned");
        let _guard = EnvGuard::new(vec!["CLAUDE_CODE_TEMPERATURE", "REMOTE_CODE_TEMPERATURE"]);
        assert_eq!(temperature(), None);
    }

    #[test]
    fn top_p_parses_valid_value() {
        let _lock = ENV_LOCK.lock().expect("env test lock poisoned");
        let guard = EnvGuard::new(vec!["CLAUDE_CODE_TOP_P", "REMOTE_CODE_TOP_P"]);
        guard.set("CLAUDE_CODE_TOP_P", "0.9");
        assert_eq!(top_p(), Some(0.9));
        drop(guard);
    }

    #[test]
    fn top_k_parses_valid_value() {
        let _lock = ENV_LOCK.lock().expect("env test lock poisoned");
        let guard = EnvGuard::new(vec!["CLAUDE_CODE_TOP_K", "REMOTE_CODE_TOP_K"]);
        guard.set("CLAUDE_CODE_TOP_K", "50");
        assert_eq!(top_k(), Some(50));
        drop(guard);
    }

    #[test]
    fn top_k_rejects_zero() {
        let _lock = ENV_LOCK.lock().expect("env test lock poisoned");
        let guard = EnvGuard::new(vec!["CLAUDE_CODE_TOP_K", "REMOTE_CODE_TOP_K"]);
        guard.set("CLAUDE_CODE_TOP_K", "0");
        assert_eq!(top_k(), None);
        drop(guard);
    }

    #[test]
    fn effort_level_reads_env() {
        let _lock = ENV_LOCK.lock().expect("env test lock poisoned");
        let guard = EnvGuard::new(vec!["CLAUDE_CODE_EFFORT_LEVEL", "REMOTE_CODE_EFFORT"]);
        guard.set("CLAUDE_CODE_EFFORT_LEVEL", "high");
        assert_eq!(effort_level(), Some("high".to_owned()));
        drop(guard);
    }

    #[test]
    fn max_output_tokens_parses_valid_value() {
        let _lock = ENV_LOCK.lock().expect("env test lock poisoned");
        let guard = EnvGuard::new(vec![
            "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
            "REMOTE_CODE_MAX_OUTPUT_TOKENS",
        ]);
        guard.set("CLAUDE_CODE_MAX_OUTPUT_TOKENS", "16384");
        assert_eq!(max_output_tokens(), Some(16384));
        drop(guard);
    }

    #[test]
    fn max_output_tokens_rejects_below_minimum() {
        let _lock = ENV_LOCK.lock().expect("env test lock poisoned");
        let guard = EnvGuard::new(vec![
            "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
            "REMOTE_CODE_MAX_OUTPUT_TOKENS",
        ]);
        guard.set("CLAUDE_CODE_MAX_OUTPUT_TOKENS", "100");
        assert_eq!(max_output_tokens(), None);
        drop(guard);
    }

    #[test]
    fn disable_thinking_reads_truthy() {
        let _lock = ENV_LOCK.lock().expect("env test lock poisoned");
        let guard = EnvGuard::new(vec![
            "CLAUDE_CODE_DISABLE_THINKING",
            "REMOTE_CODE_DISABLE_THINKING",
        ]);
        guard.set("CLAUDE_CODE_DISABLE_THINKING", "1");
        assert!(disable_thinking());
        drop(guard);
    }

    #[test]
    fn disable_thinking_returns_false_when_unset() {
        let _lock = ENV_LOCK.lock().expect("env test lock poisoned");
        let _guard = EnvGuard::new(vec![
            "CLAUDE_CODE_DISABLE_THINKING",
            "REMOTE_CODE_DISABLE_THINKING",
        ]);
        assert!(!disable_thinking());
    }

    #[test]
    fn disable_interleaved_thinking_reads_truthy() {
        let _lock = ENV_LOCK.lock().expect("env test lock poisoned");
        let guard = EnvGuard::new(vec!["DISABLE_INTERLEAVED_THINKING"]);
        guard.set("DISABLE_INTERLEAVED_THINKING", "true");
        assert!(disable_interleaved_thinking());
        drop(guard);
    }

    #[test]
    fn user_type_reads_ant() {
        let _lock = ENV_LOCK.lock().expect("env test lock poisoned");
        let guard = EnvGuard::new(vec!["USER_TYPE"]);
        guard.set("USER_TYPE", "ant");
        assert!(is_ant_user());
        assert_eq!(user_type(), Some("ant".to_owned()));
        drop(guard);
    }

    #[test]
    fn user_type_external_is_not_ant() {
        let _lock = ENV_LOCK.lock().expect("env test lock poisoned");
        let guard = EnvGuard::new(vec!["USER_TYPE"]);
        guard.set("USER_TYPE", "external");
        assert!(!is_ant_user());
        drop(guard);
    }

    #[test]
    fn disable_cost_warnings_reads_truthy() {
        let _lock = ENV_LOCK.lock().expect("env test lock poisoned");
        let guard = EnvGuard::new(vec![
            "DISABLE_COST_WARNINGS",
            "REMOTE_CODE_DISABLE_COST_WARNINGS",
        ]);
        guard.set("DISABLE_COST_WARNINGS", "yes");
        assert!(disable_cost_warnings());
        drop(guard);
    }

    #[test]
    fn disable_background_tasks_reads_truthy() {
        let _lock = ENV_LOCK.lock().expect("env test lock poisoned");
        let guard = EnvGuard::new(vec![
            "CLAUDE_CODE_DISABLE_BACKGROUND_TASKS",
            "REMOTE_CODE_DISABLE_BACKGROUND_TASKS",
        ]);
        guard.set("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS", "true");
        assert!(disable_background_tasks());
        drop(guard);
    }

    #[test]
    fn is_remote_reads_truthy() {
        let _lock = ENV_LOCK.lock().expect("env test lock poisoned");
        let guard = EnvGuard::new(vec!["CLAUDE_CODE_REMOTE", "REMOTE_CODE_REMOTE"]);
        guard.set("CLAUDE_CODE_REMOTE", "1");
        assert!(is_remote());
        drop(guard);
    }

    #[test]
    fn is_remote_returns_false_when_unset() {
        let _lock = ENV_LOCK.lock().expect("env test lock poisoned");
        let _guard = EnvGuard::new(vec!["CLAUDE_CODE_REMOTE", "REMOTE_CODE_REMOTE"]);
        assert!(!is_remote());
    }

    #[test]
    fn remote_code_prefix_fallback_works() {
        let _lock = ENV_LOCK.lock().expect("env test lock poisoned");
        let guard = EnvGuard::new(vec!["CLAUDE_CODE_TEMPERATURE", "REMOTE_CODE_TEMPERATURE"]);
        guard.remove("CLAUDE_CODE_TEMPERATURE");
        guard.set("REMOTE_CODE_TEMPERATURE", "0.7");
        assert_eq!(temperature(), Some(0.7));
        drop(guard);
    }
}
