//! Token budget tracker with continuation logic and diminishing-returns detection.
//!
//! Mirrors Claude Code's `query/tokenBudget.ts`:
//! - Continuations are allowed while `turn_tokens < budget * 0.9` (90% threshold)
//! - Diminishing returns: after 3+ continuations, if the last TWO checks both saw
//!   delta < 500 tokens, stop early even below the 90% threshold.
//! - Completion events carry analytics metadata.

use std::time::Instant;

/// When the cumulative output has reached this fraction of the budget,
/// stop issuing continuations.
const COMPLETION_THRESHOLD: f64 = 0.9;

/// Two consecutive deltas below this value after 3+ continuations
/// trigger an early stop for diminishing returns.
const DIMINISHING_THRESHOLD: u64 = 500;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// Mutable tracker that persists across query-loop iterations.
#[derive(Debug, Clone)]
pub struct BudgetTracker {
    /// Max turn count for the query.
    pub max_turns: u32,
    /// Optional hard token cap.
    pub max_total_tokens: Option<u64>,

    /// How many budget continuations have been issued so far.
    pub continuation_count: usize,
    /// Token delta since the previous check.
    pub last_delta_tokens: u64,
    /// Global turn-output token count seen at the previous check.
    pub last_global_turn_tokens: u64,
    /// When the tracker was created (to report duration on completion).
    pub started_at: Instant,
}

impl BudgetTracker {
    #[must_use]
    pub fn new(max_turns: u32, max_total_tokens: Option<u64>) -> Self {
        Self {
            max_turns,
            max_total_tokens,
            continuation_count: 0,
            last_delta_tokens: 0,
            last_global_turn_tokens: 0,
            started_at: Instant::now(),
        }
    }

    /// Evaluate hard limits (turns, total tokens) before the per-iteration
    /// token-budget continuation check. Returns [`TokenBudgetDecision::Stop`]
    /// when a hard limit is exceeded; otherwise returns [`TokenBudgetDecision::Continue`].
    #[must_use]
    pub fn evaluate_hard_limits(&self, turn: u32, total_tokens: u64) -> TokenBudgetDecision {
        if turn >= self.max_turns {
            return TokenBudgetDecision::Stop {
                reason: format!("turn budget exceeded ({})", self.max_turns),
                completion_event: None,
            };
        }
        if let Some(limit) = self.max_total_tokens
            && total_tokens >= limit
        {
            return TokenBudgetDecision::Stop {
                reason: format!("token budget exceeded ({limit})"),
                completion_event: None,
            };
        }
        TokenBudgetDecision::Continue
    }

    /// Check the per-iteration token budget using continuation / diminishing-returns logic.
    ///
    /// `budget` is the turn-level token budget (null/absent for sub-agents).
    /// `global_turn_tokens` is the cumulative output tokens for this turn so far.
    /// `agent_id` is used to skip continuation on sub-agents (always returns Stop).
    #[must_use]
    pub fn check_continuation(
        &mut self,
        agent_id: Option<&str>,
        budget: Option<u64>,
        global_turn_tokens: u64,
    ) -> TokenBudgetDecision {
        // Sub-agents never receive budget continuations.
        if agent_id.is_some() || budget.is_none() || budget == Some(0) {
            return TokenBudgetDecision::Stop {
                reason: "budget not applicable for sub-agent".to_owned(),
                completion_event: None,
            };
        }

        // Early return above guarantees budget is Some; safe to unwrap.
        let budget = budget.unwrap_or(1);
        let turn_tokens = global_turn_tokens;
        let pct = ((turn_tokens as f64 / budget as f64) * 100.0).round() as u32;
        let delta_since_last = global_turn_tokens.saturating_sub(self.last_global_turn_tokens);

        let is_diminishing = self.continuation_count >= 3
            && delta_since_last < DIMINISHING_THRESHOLD
            && self.last_delta_tokens < DIMINISHING_THRESHOLD;

        if !is_diminishing && (turn_tokens as f64) < (budget as f64) * COMPLETION_THRESHOLD {
            self.continuation_count += 1;
            self.last_delta_tokens = delta_since_last;
            self.last_global_turn_tokens = global_turn_tokens;

            let nudge_message = budget_continuation_message(pct, turn_tokens, budget);

            return TokenBudgetDecision::ContinueWithNudge {
                nudge_message,
                continuation_count: self.continuation_count,
                pct,
                turn_tokens,
                budget,
            };
        }

        // Stop. Emit a completion event if we ever issued at least one continuation
        // OR we are stopping for diminishing returns.
        if is_diminishing || self.continuation_count > 0 {
            return TokenBudgetDecision::Stop {
                reason: if is_diminishing {
                    "diminishing returns".to_owned()
                } else {
                    format!("budget threshold reached ({pct}%)")
                },
                completion_event: Some(BudgetCompletionEvent {
                    continuation_count: self.continuation_count,
                    pct,
                    turn_tokens,
                    budget,
                    diminishing_returns: is_diminishing,
                    duration_ms: self.started_at.elapsed().as_millis() as u64,
                }),
            };
        }

        TokenBudgetDecision::Stop {
            reason: "budget not yet active".to_owned(),
            completion_event: None,
        }
    }
}

impl Default for BudgetTracker {
    fn default() -> Self {
        Self::new(8, None)
    }
}

/// Decision produced by the budget tracker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenBudgetDecision {
    /// Continue without a nudge.
    Continue,
    /// Continue, but inject a nudge message for the model.
    ContinueWithNudge {
        nudge_message: String,
        continuation_count: usize,
        pct: u32,
        turn_tokens: u64,
        budget: u64,
    },
    /// Stop the query loop.
    Stop {
        reason: String,
        completion_event: Option<BudgetCompletionEvent>,
    },
}

/// Analytics payload emitted when a budget run completes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetCompletionEvent {
    pub continuation_count: usize,
    pub pct: u32,
    pub turn_tokens: u64,
    pub budget: u64,
    pub diminishing_returns: bool,
    pub duration_ms: u64,
}

// ---------------------------------------------------------------------------
// Nudge message – mirrors `getBudgetContinuationMessage` in TS
// ---------------------------------------------------------------------------

fn budget_continuation_message(pct: u32, turn_tokens: u64, budget: u64) -> String {
    format!(
        "Stopped at {pct}% of token target ({turn_tokens} / {budget}). Keep working -- do not summarize."
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_limits_stop_on_turn_limit() {
        let tracker = BudgetTracker::new(2, None);
        assert_eq!(
            tracker.evaluate_hard_limits(0, 0),
            TokenBudgetDecision::Continue
        );
        assert_eq!(
            tracker.evaluate_hard_limits(2, 0),
            TokenBudgetDecision::Stop {
                reason: "turn budget exceeded (2)".to_owned(),
                completion_event: None,
            }
        );
    }

    #[test]
    fn hard_limits_stop_on_token_limit() {
        let tracker = BudgetTracker::new(5, Some(100));
        assert_eq!(
            tracker.evaluate_hard_limits(1, 100),
            TokenBudgetDecision::Stop {
                reason: "token budget exceeded (100)".to_owned(),
                completion_event: None,
            }
        );
    }

    #[test]
    fn continuation_below_90_percent() {
        let mut tracker = BudgetTracker::new(10, None);
        let decision = tracker.check_continuation(
            None,          // no agent_id → main thread
            Some(100_000), // budget
            10_000,        // 10% used
        );
        assert!(matches!(
            decision,
            TokenBudgetDecision::ContinueWithNudge { .. }
        ));
        assert_eq!(tracker.continuation_count, 1);
    }

    #[test]
    fn stops_above_90_percent() {
        let mut tracker = BudgetTracker::new(10, None);
        let decision = tracker.check_continuation(
            None,
            Some(100_000),
            95_000, // 95% used
        );
        assert!(matches!(decision, TokenBudgetDecision::Stop { .. }));
    }

    #[test]
    fn sub_agent_always_stops() {
        let mut tracker = BudgetTracker::new(10, None);
        let decision = tracker.check_continuation(
            Some("agent-1"), // sub-agent
            Some(100_000),
            10_000,
        );
        assert!(matches!(decision, TokenBudgetDecision::Stop { .. }));
    }

    #[test]
    fn null_budget_always_stops() {
        let mut tracker = BudgetTracker::new(10, None);
        let decision = tracker.check_continuation(None, None, 10_000);
        assert!(matches!(decision, TokenBudgetDecision::Stop { .. }));
    }

    #[test]
    fn zero_budget_always_stops() {
        let mut tracker = BudgetTracker::new(10, None);
        let decision = tracker.check_continuation(None, Some(0), 10_000);
        assert!(matches!(decision, TokenBudgetDecision::Stop { .. }));
    }

    #[test]
    fn diminishing_returns_triggers_after_3_continuations() {
        let mut tracker = BudgetTracker::new(10, None);
        let budget = Some(100_000);

        // 3 continuations with small deltas
        for _ in 0..3 {
            let decision = tracker.check_continuation(None, budget, 10_000);
            assert!(matches!(
                decision,
                TokenBudgetDecision::ContinueWithNudge { .. }
            ));
        }

        // 4th check: last_delta_tokens was set to a small value (0),
        // and this delta is also small (0) → diminishing returns triggers
        let decision = tracker.check_continuation(None, budget, 10_000);
        assert!(matches!(decision, TokenBudgetDecision::Stop { .. }));
        if let TokenBudgetDecision::Stop {
            completion_event, ..
        } = decision
        {
            let event = completion_event.expect("should have completion event");
            assert!(event.diminishing_returns);
        }
    }

    #[test]
    fn completion_event_emitted_when_continuations_were_issued() {
        let mut tracker = BudgetTracker::new(10, None);
        let _ = tracker.check_continuation(None, Some(100_000), 10_000);
        // Now go above 90% on second check
        let decision = tracker.check_continuation(None, Some(100_000), 95_000);
        assert!(matches!(decision, TokenBudgetDecision::Stop { .. }));
        if let TokenBudgetDecision::Stop {
            completion_event, ..
        } = decision
        {
            assert!(completion_event.is_some());
            let event = completion_event.expect("completion event should be present");
            assert_eq!(event.continuation_count, 1);
            assert!(!event.diminishing_returns);
        }
    }

    #[test]
    fn no_completion_event_without_prior_continuations() {
        let tracker = BudgetTracker::new(10, None);
        let decision = tracker.evaluate_hard_limits(10, 0);
        match decision {
            TokenBudgetDecision::Stop {
                completion_event, ..
            } => {
                assert!(completion_event.is_none());
            }
            _ => panic!("expected Stop"),
        }
    }
}
