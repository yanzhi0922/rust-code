//! Microcompact configuration and decision logic.
//!
//! This module defines the configuration types that control when and how
//! micro-compaction is triggered. It supports two strategies:
//!
//! - **Cached** — trigger when the token count exceeds a static threshold.
//! - **Time-based** — trigger at regular intervals, with a per-hour cap to
//!   avoid excessive compaction.
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use claude_compact::mc_config::{McConfig, McStrategy, should_use_microcompact};
//!
//! let config = McConfig::default();
//! let should = should_use_microcompact(150_000, &config);
//! ```

use std::time::Instant;

// ---------------------------------------------------------------------------
// Strategy enum
// ---------------------------------------------------------------------------

/// Microcompact strategy selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McStrategy {
    /// Trigger when token count exceeds the threshold (stateless).
    Cached,
    /// Trigger based on elapsed time since the last compaction.
    TimeBased,
}

// ---------------------------------------------------------------------------
// Time-based configuration
// ---------------------------------------------------------------------------

/// Configuration for the time-based microcompact strategy.
#[derive(Debug, Clone, Copy)]
pub struct TimeBasedConfig {
    /// Minimum interval between microcompactions, in seconds.
    pub interval_secs: u64,
    /// Maximum number of microcompactions allowed per hour.
    pub max_compacts_per_hour: u32,
}

impl Default for TimeBasedConfig {
    fn default() -> Self {
        Self {
            interval_secs: 300, // 5 minutes
            max_compacts_per_hour: 12,
        }
    }
}

// ---------------------------------------------------------------------------
// McConfig
// ---------------------------------------------------------------------------

/// Top-level microcompact configuration.
#[derive(Debug, Clone)]
pub struct McConfig {
    /// Strategy to use for microcompact decisions.
    pub strategy: McStrategy,
    /// Token count threshold above which microcompaction is considered.
    pub threshold_tokens: u64,
    /// Optional time-based configuration (required when strategy is `TimeBased`).
    pub time_based: Option<TimeBasedConfig>,
}

impl Default for McConfig {
    fn default() -> Self {
        Self {
            strategy: McStrategy::Cached,
            threshold_tokens: 100_000,
            time_based: None,
        }
    }
}

impl McConfig {
    /// Create a cached-strategy config with a custom threshold.
    #[must_use]
    pub fn cached(threshold_tokens: u64) -> Self {
        Self {
            strategy: McStrategy::Cached,
            threshold_tokens,
            time_based: None,
        }
    }

    /// Create a time-based config with default threshold.
    #[must_use]
    pub fn time_based(interval_secs: u64, max_compacts_per_hour: u32) -> Self {
        Self {
            strategy: McStrategy::TimeBased,
            threshold_tokens: 100_000,
            time_based: Some(TimeBasedConfig {
                interval_secs,
                max_compacts_per_hour,
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Decision function
// ---------------------------------------------------------------------------

/// Decide whether microcompaction should be performed.
///
/// - **Cached strategy**: returns `true` when `current_tokens >= threshold_tokens`.
/// - **Time-based strategy**: returns `true` when the threshold is exceeded
///   **and** the interval since the last compaction has elapsed, subject to
///   the per-hour cap.
///
/// The `cached_mc_config` parameter carries stateful tracking data for the
/// time-based strategy. Pass a fresh [`CachedMcConfig`] for the first call.
pub fn should_use_microcompact(
    current_tokens: u64,
    config: &McConfig,
    cached_mc_config: &mut CachedMcConfig,
) -> bool {
    if current_tokens < config.threshold_tokens {
        return false;
    }

    match config.strategy {
        McStrategy::Cached => true,
        McStrategy::TimeBased => {
            let tb = match config.time_based {
                Some(ref tb) => tb,
                None => return true, // fallback to cached behaviour
            };

            // Check per-hour cap.
            if cached_mc_config.compacts_this_hour >= tb.max_compacts_per_hour {
                return false;
            }

            // Check interval.
            if let Some(last) = cached_mc_config.last_compact_time {
                let elapsed = last.elapsed().as_secs();
                if elapsed < tb.interval_secs {
                    return false;
                }
            }

            true
        }
    }
}

// ---------------------------------------------------------------------------
// CachedMcConfig — stateful tracking
// ---------------------------------------------------------------------------

/// Stateful tracking data for microcompact decisions.
///
/// Maintains the last compaction timestamp and a rolling count of
/// compactions within the current hour.
#[derive(Debug, Clone)]
pub struct CachedMcConfig {
    /// Instant of the last microcompaction.
    pub last_compact_time: Option<Instant>,
    /// Number of compactions performed in the current tracking window.
    pub compacts_this_hour: u32,
    /// Start of the current tracking window.
    pub window_start: Instant,
}

impl Default for CachedMcConfig {
    fn default() -> Self {
        Self {
            last_compact_time: None,
            compacts_this_hour: 0,
            window_start: Instant::now(),
        }
    }
}

impl CachedMcConfig {
    /// Create a new, empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that a microcompaction was just performed.
    pub fn record_compact(&mut self) {
        let now = Instant::now();

        // Reset the hourly counter if the window has expired.
        if now.duration_since(self.window_start).as_secs() >= 3600 {
            self.compacts_this_hour = 0;
            self.window_start = now;
        }

        self.last_compact_time = Some(now);
        self.compacts_this_hour += 1;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_strategy_below_threshold() {
        let config = McConfig::cached(100_000);
        let mut cache = CachedMcConfig::new();
        assert!(!should_use_microcompact(50_000, &config, &mut cache));
    }

    #[test]
    fn cached_strategy_at_threshold() {
        let config = McConfig::cached(100_000);
        let mut cache = CachedMcConfig::new();
        assert!(should_use_microcompact(100_000, &config, &mut cache));
    }

    #[test]
    fn cached_strategy_above_threshold() {
        let config = McConfig::cached(100_000);
        let mut cache = CachedMcConfig::new();
        assert!(should_use_microcompact(150_000, &config, &mut cache));
    }

    #[test]
    fn time_based_strategy_respects_interval() {
        let config = McConfig::time_based(300, 12);
        let mut cache = CachedMcConfig::new();
        // Record a compact just now.
        cache.record_compact();

        // Should not trigger again immediately.
        assert!(!should_use_microcompact(150_000, &config, &mut cache));
    }

    #[test]
    fn time_based_strategy_respects_hourly_cap() {
        let config = McConfig::time_based(0, 2);
        let mut cache = CachedMcConfig::new();

        // First two should succeed.
        assert!(should_use_microcompact(150_000, &config, &mut cache));
        cache.record_compact();

        assert!(should_use_microcompact(150_000, &config, &mut cache));
        cache.record_compact();

        // Third should be capped.
        assert!(!should_use_microcompact(150_000, &config, &mut cache));
    }

    #[test]
    fn time_based_strategy_below_threshold_never_triggers() {
        let config = McConfig::time_based(0, 100);
        let mut cache = CachedMcConfig::new();
        assert!(!should_use_microcompact(10_000, &config, &mut cache));
    }

    #[test]
    fn time_based_without_config_falls_back_to_cached() {
        let config = McConfig {
            strategy: McStrategy::TimeBased,
            threshold_tokens: 50_000,
            time_based: None,
        };
        let mut cache = CachedMcConfig::new();
        assert!(should_use_microcompact(100_000, &config, &mut cache));
    }

    #[test]
    fn record_compact_increments_counter() {
        let mut cache = CachedMcConfig::new();
        assert_eq!(cache.compacts_this_hour, 0);

        cache.record_compact();
        assert_eq!(cache.compacts_this_hour, 1);

        cache.record_compact();
        assert_eq!(cache.compacts_this_hour, 2);

        assert!(cache.last_compact_time.is_some());
    }

    #[test]
    fn default_config_has_cached_strategy() {
        let config = McConfig::default();
        assert_eq!(config.strategy, McStrategy::Cached);
        assert_eq!(config.threshold_tokens, 100_000);
        assert!(config.time_based.is_none());
    }

    #[test]
    fn mc_strategy_equality() {
        assert_eq!(McStrategy::Cached, McStrategy::Cached);
        assert_ne!(McStrategy::Cached, McStrategy::TimeBased);
    }
}
