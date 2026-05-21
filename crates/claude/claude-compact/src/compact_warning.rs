//! Compaction warning messages and state management.
//!
//! Provides user-facing warning formatting for context compaction,
//! including threshold-based warnings and acknowledgement flow.

// ---------------------------------------------------------------------------
// CompactWarningState
// ---------------------------------------------------------------------------

/// State of the compaction warning lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactWarningState {
    /// No warning has been issued.
    None,
    /// A warning has been shown to the user.
    Warned,
    /// The user has acknowledged and accepted the warning.
    Acknowledged,
}

impl std::fmt::Display for CompactWarningState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "none"),
            Self::Warned => write!(f, "warned"),
            Self::Acknowledged => write!(f, "acknowledged"),
        }
    }
}

// ---------------------------------------------------------------------------
// should_warn_before_compact
// ---------------------------------------------------------------------------

/// Parameters for deciding whether to warn before compaction.
#[derive(Debug, Clone)]
pub struct CompactWarningParams {
    /// Current token usage.
    pub current_tokens: u64,
    /// Maximum context window tokens.
    pub max_tokens: u64,
    /// Whether this is the first compaction in the session.
    pub is_first_compact: bool,
    /// Current warning state.
    pub warning_state: CompactWarningState,
    /// Threshold ratio (0.0–1.0) at which to warn.
    pub warn_threshold_ratio: f64,
}

impl Default for CompactWarningParams {
    fn default() -> Self {
        Self {
            current_tokens: 0,
            max_tokens: 200_000,
            is_first_compact: true,
            warning_state: CompactWarningState::None,
            warn_threshold_ratio: 0.8,
        }
    }
}

impl CompactWarningParams {
    /// Create params with token counts.
    #[must_use]
    pub fn new(current_tokens: u64, max_tokens: u64) -> Self {
        Self {
            current_tokens,
            max_tokens,
            ..Self::default()
        }
    }

    /// Current usage ratio (0.0–1.0).
    #[must_use]
    pub fn usage_ratio(&self) -> f64 {
        if self.max_tokens == 0 {
            0.0
        } else {
            self.current_tokens as f64 / self.max_tokens as f64
        }
    }
}

/// Determine whether a warning should be shown before compaction.
///
/// Returns `true` if:
/// - Token usage exceeds the warning threshold ratio, **or**
/// - This is the first compaction and no warning has been shown yet.
#[must_use]
pub fn should_warn_before_compact(params: &CompactWarningParams) -> bool {
    if matches!(params.warning_state, CompactWarningState::Acknowledged) {
        return false;
    }

    if params.is_first_compact && matches!(params.warning_state, CompactWarningState::None) {
        return true;
    }

    params.usage_ratio() >= params.warn_threshold_ratio
}

// ---------------------------------------------------------------------------
// format_compact_warning
// ---------------------------------------------------------------------------

/// Format a human-readable compaction warning message.
///
/// The warning explains that context compaction is about to occur and
/// provides relevant details about token usage.
#[must_use]
pub fn format_compact_warning(params: &CompactWarningParams) -> String {
    let usage_pct = (params.usage_ratio() * 100.0) as u64;
    let tokens_remaining = params.max_tokens.saturating_sub(params.current_tokens);

    format!(
        "⚠️  Context compaction is about to occur.\n\
         \n\
         Token usage: {current}/{max} ({pct}%)\n\
         Tokens remaining: {remaining}\n\
         \n\
         Compaction will summarize older conversation context to free up space.\n\
         Recent messages and key information will be preserved.",
        current = params.current_tokens,
        max = params.max_tokens,
        pct = usage_pct,
        remaining = tokens_remaining,
    )
}

/// Format a short one-line compaction warning.
#[must_use]
pub fn format_compact_warning_short(params: &CompactWarningParams) -> String {
    let usage_pct = (params.usage_ratio() * 100.0) as u64;
    format!(
        "Context at {pct}% ({current}/{max} tokens) — compaction recommended.",
        pct = usage_pct,
        current = params.current_tokens,
        max = params.max_tokens,
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- CompactWarningState --------------------------------------------------

    #[test]
    fn warning_state_display() {
        assert_eq!(CompactWarningState::None.to_string(), "none");
        assert_eq!(CompactWarningState::Warned.to_string(), "warned");
        assert_eq!(
            CompactWarningState::Acknowledged.to_string(),
            "acknowledged"
        );
    }

    #[test]
    fn warning_state_equality() {
        assert_eq!(CompactWarningState::None, CompactWarningState::None);
        assert_ne!(CompactWarningState::None, CompactWarningState::Warned);
    }

    // -- CompactWarningParams -------------------------------------------------

    #[test]
    fn params_default() {
        let p = CompactWarningParams::default();
        assert_eq!(p.current_tokens, 0);
        assert_eq!(p.max_tokens, 200_000);
        assert!(p.is_first_compact);
        assert_eq!(p.warning_state, CompactWarningState::None);
        assert!((p.warn_threshold_ratio - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn params_new() {
        let p = CompactWarningParams::new(100_000, 200_000);
        assert_eq!(p.current_tokens, 100_000);
        assert_eq!(p.max_tokens, 200_000);
    }

    #[test]
    fn params_usage_ratio() {
        let p = CompactWarningParams::new(100_000, 200_000);
        assert!((p.usage_ratio() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn params_usage_ratio_zero_max() {
        let p = CompactWarningParams::new(100, 0);
        assert_eq!(p.usage_ratio(), 0.0);
    }

    #[test]
    fn params_usage_ratio_full() {
        let p = CompactWarningParams::new(200_000, 200_000);
        assert!((p.usage_ratio() - 1.0).abs() < f64::EPSILON);
    }

    // -- should_warn_before_compact -------------------------------------------

    #[test]
    fn warn_on_first_compact() {
        let params = CompactWarningParams {
            is_first_compact: true,
            warning_state: CompactWarningState::None,
            current_tokens: 10_000,
            max_tokens: 200_000,
            ..CompactWarningParams::default()
        };
        assert!(should_warn_before_compact(&params));
    }

    #[test]
    fn no_warn_when_acknowledged() {
        let params = CompactWarningParams {
            is_first_compact: false,
            warning_state: CompactWarningState::Acknowledged,
            current_tokens: 190_000,
            max_tokens: 200_000,
            ..CompactWarningParams::default()
        };
        assert!(!should_warn_before_compact(&params));
    }

    #[test]
    fn warn_when_above_threshold() {
        let params = CompactWarningParams {
            is_first_compact: false,
            warning_state: CompactWarningState::Warned,
            current_tokens: 170_000,
            max_tokens: 200_000,
            warn_threshold_ratio: 0.8,
        };
        assert!(should_warn_before_compact(&params));
    }

    #[test]
    fn no_warn_when_below_threshold_and_not_first() {
        let params = CompactWarningParams {
            is_first_compact: false,
            warning_state: CompactWarningState::Warned,
            current_tokens: 50_000,
            max_tokens: 200_000,
            warn_threshold_ratio: 0.8,
        };
        assert!(!should_warn_before_compact(&params));
    }

    #[test]
    fn warn_at_exact_threshold() {
        let params = CompactWarningParams {
            is_first_compact: false,
            warning_state: CompactWarningState::None,
            current_tokens: 160_000,
            max_tokens: 200_000,
            warn_threshold_ratio: 0.8,
        };
        assert!(should_warn_before_compact(&params));
    }

    // -- format_compact_warning -----------------------------------------------

    #[test]
    fn format_warning_contains_usage() {
        let params = CompactWarningParams::new(150_000, 200_000);
        let msg = format_compact_warning(&params);
        assert!(msg.contains("150000/200000"));
        assert!(msg.contains("75%"));
        assert!(msg.contains("50000"));
        assert!(msg.contains("compaction"));
    }

    #[test]
    fn format_warning_short() {
        let params = CompactWarningParams::new(150_000, 200_000);
        let msg = format_compact_warning_short(&params);
        assert!(msg.contains("75%"));
        assert!(msg.contains("150000/200000"));
    }

    #[test]
    fn format_warning_zero_tokens() {
        let params = CompactWarningParams::new(0, 200_000);
        let msg = format_compact_warning(&params);
        assert!(msg.contains("0/200000"));
        assert!(msg.contains("0%"));
    }

    #[test]
    fn format_warning_full_tokens() {
        let params = CompactWarningParams::new(200_000, 200_000);
        let msg = format_compact_warning(&params);
        assert!(msg.contains("100%"));
        assert!(msg.contains("0"));
    }
}
