//! Max output tokens recovery — triggered when model output is truncated.
//!
//! Mirrors Claude Code's `query.ts` (lines 1188–1256): when the model
//! hits the `max_output_tokens` limit, this module provides an escalating
//! recovery strategy:
//!
//! 1. **Escalation** — retry the same request with a higher token limit
//!    (8k → 64k) in a single jump, gated by feature flag equivalent.
//! 2. **Continuation** — inject a meta-user-message asking the model to
//!    continue from where it left off.
//! 3. **Exhaustion** — after `MAX_OUTPUT_TOKENS_RECOVERY_LIMIT` (3) attempts,
//!    surface the truncated error.

use claude_core::Message;

use crate::message_utils::create_user_message;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of recovery attempts before surfacing the error.
/// Mirrors TS `MAX_OUTPUT_TOKENS_RECOVERY_LIMIT = 3`.
pub const MAX_OUTPUT_TOKENS_RECOVERY_LIMIT: usize = 3;

/// Escalation cap used after an 8k-default output truncates.
/// Mirrors TS `ESCALATED_MAX_TOKENS = 64_000` (from `context.ts`).
pub const ESCALATED_MAX_TOKENS: usize = 64_000;

/// Default output token cap before escalation kicks in.
pub const DEFAULT_MAX_OUTPUT_TOKENS: usize = 8_192;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Action to take when the model output is truncated.
#[derive(Debug, Clone)]
pub enum MaxTokensRecoveryAction {
    /// Retry the same request with `ESCALATED_MAX_TOKENS` (64k).
    /// Mirrors TS feature-gated `tengu_otk_slot_v1` escalation.
    Escalate {
        /// The new `max_tokens` to use (64k).
        new_max_tokens: usize,
    },
    /// Inject a continuation message and keep the current token limit.
    /// Mirrors TS max_output_tokens recovery (line 1223–1252).
    ContinueWithMessage {
        /// The `max_tokens` to use for the continuation request.
        max_tokens: usize,
        /// The continuation message to append.
        continuation_message: Message,
    },
    /// Recovery exhausted — the error should be surfaced to the user.
    Exhausted,
}

/// Manages recovery from max-output-tokens truncation.
///
/// Tracks recovery attempts and decides the appropriate action.
/// The escalation ladder is: undefined/8k → 64k (single jump, TS parity).
/// After escalation is exhausted, multi-turn continuation with a
/// "Resume directly" meta message is used.
#[derive(Debug, Clone, Default)]
pub struct MaxTokensRecovery {
    /// Number of recovery attempts already made in the current query.
    pub recovery_count: usize,
    /// Whether the 64k single-shot escalation has been attempted.
    pub escalation_attempted: bool,
}

impl MaxTokensRecovery {
    /// Create a new recovery handler with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` if recovery is still possible.
    #[must_use]
    pub fn can_recover(&self) -> bool {
        self.recovery_count < MAX_OUTPUT_TOKENS_RECOVERY_LIMIT
    }

    /// Reset the recovery counter (e.g. after a successful turn that didn't truncate).
    pub fn reset(&mut self) {
        self.recovery_count = 0;
        self.escalation_attempted = false;
    }

    /// Determine the recovery action for a truncation event.
    ///
    /// * `current_max_tokens` — the `max_tokens` value used in the truncated request
    ///   (use `estimate_current_max_tokens` from the calling module).
    /// * `user_has_override` — true if `CLAUDE_CODE_MAX_OUTPUT_TOKENS` is set.
    ///
    /// Returns the appropriate [`MaxTokensRecoveryAction`].
    pub fn handle_truncation(
        &mut self,
        current_max_tokens: usize,
        user_has_override: bool,
    ) -> Option<MaxTokensRecoveryAction> {
        if !self.can_recover() {
            return Some(MaxTokensRecoveryAction::Exhausted);
        }

        // Single-shot escalation: if we haven't tried 64k yet and
        // the current cap is below 64k and the user didn't set an
        // explicit override, escalate directly to 64k.
        if !self.escalation_attempted
            && current_max_tokens < ESCALATED_MAX_TOKENS
            && !user_has_override
        {
            self.escalation_attempted = true;
            self.recovery_count += 1;
            return Some(MaxTokensRecoveryAction::Escalate {
                new_max_tokens: ESCALATED_MAX_TOKENS,
            });
        }

        // Multi-turn continuation with a meta message
        self.recovery_count += 1;
        Some(MaxTokensRecoveryAction::ContinueWithMessage {
            max_tokens: current_max_tokens,
            continuation_message: create_continuation_message(),
        })
    }

    /// Returns the current recovery count.
    #[must_use]
    pub fn recovery_count(&self) -> usize {
        self.recovery_count
    }
}

/// Create a continuation message matching the TS reference verbatim.
/// TS: "Output token limit hit. Resume directly — no apology, no recap
/// of what you were doing. Pick up mid-thought if that is where the cut
/// happened. Break remaining work into smaller pieces."
fn create_continuation_message() -> Message {
    let text = concat!(
        "Output token limit hit. Resume directly — no apology, no recap ",
        "of what you were doing. Pick up mid-thought if that is where the ",
        "cut happened. Break remaining work into smaller pieces."
    );
    let mut msg = create_user_message(text);
    // Mark as meta (hidden from UI)
    if let Message::User(user_msg) = &mut msg {
        user_msg.base.is_meta = true;
    }
    msg
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Test 1: Escalation from 8k to 64k ----

    #[test]
    fn escalation_from_8k_to_64k() {
        let mut recovery = MaxTokensRecovery::new();
        let action = recovery
            .handle_truncation(8192, false)
            .expect("8192-token truncation should escalate");

        match action {
            MaxTokensRecoveryAction::Escalate { new_max_tokens } => {
                assert_eq!(new_max_tokens, 64_000);
            }
            other => panic!("Expected Escalate, got {other:?}"),
        }
        assert_eq!(recovery.recovery_count(), 1);
        assert!(recovery.escalation_attempted);
    }

    // ---- Test 2: User override blocks escalation, falls to continuation ----

    #[test]
    fn user_override_blocks_escalation() {
        let mut recovery = MaxTokensRecovery::new();
        let action = recovery
            .handle_truncation(8192, true)
            .expect("user override should block escalation, fall to continuation");

        // Since escalation is blocked and we still have recovery attempts,
        // we expect ContinueWithMessage
        match action {
            MaxTokensRecoveryAction::ContinueWithMessage { max_tokens, .. } => {
                assert_eq!(max_tokens, 8192);
            }
            other => panic!("Expected ContinueWithMessage, got {other:?}"),
        }
    }

    // ---- Test 3: Already escalated — falls to continuation ----

    #[test]
    fn already_escalated_falls_to_continuation() {
        let mut recovery = MaxTokensRecovery::new();
        let _ = recovery.handle_truncation(8192, false);
        // Second truncation at 64k → escalation already attempted, fall to continuation
        let action = recovery
            .handle_truncation(64_000, false)
            .expect("second truncation should use continuation");

        match action {
            MaxTokensRecoveryAction::ContinueWithMessage { max_tokens, .. } => {
                assert_eq!(max_tokens, 64_000);
            }
            other => panic!("Expected ContinueWithMessage, got {other:?}"),
        }
        assert_eq!(recovery.recovery_count(), 2);
    }

    // ---- Test 4: Exhaustion after 3 recoveries ----

    #[test]
    fn exhaustion_after_3_recoveries() {
        let mut recovery = MaxTokensRecovery::new();
        let _ = recovery.handle_truncation(8192, false); // escalation
        let _ = recovery.handle_truncation(64_000, false); // continuation 1
        let _ = recovery.handle_truncation(64_000, false); // continuation 2
        // 4th attempt: exhausted
        let action = recovery.handle_truncation(64_000, false);
        assert!(matches!(action, Some(MaxTokensRecoveryAction::Exhausted)));
    }

    // ---- Test 5: Can recover check ----

    #[test]
    fn can_recover_check() {
        let mut recovery = MaxTokensRecovery::new();
        assert!(recovery.can_recover());

        for _ in 0..3 {
            let _ = recovery.handle_truncation(8192, false);
        }
        assert!(!recovery.can_recover());
    }

    // ---- Test 6: Reset allows new recoveries ----

    #[test]
    fn reset_allows_new_recoveries() {
        let mut recovery = MaxTokensRecovery::new();
        let _ = recovery.handle_truncation(8192, false);
        // First truncation does escalate (8k → 64k single jump).
        assert!(recovery.escalation_attempted);
        assert_eq!(recovery.recovery_count(), 1);

        recovery.reset();
        assert!(recovery.can_recover());
        assert_eq!(recovery.recovery_count(), 0);
        assert!(!recovery.escalation_attempted);
    }

    // ---- Test 7: Zero max_tokens triggers escalation ----

    #[test]
    fn zero_max_tokens_triggers_escalation() {
        let mut recovery = MaxTokensRecovery::new();
        let action = recovery
            .handle_truncation(0, false)
            .expect("zero max_tokens should trigger escalation");

        match action {
            MaxTokensRecoveryAction::Escalate { new_max_tokens } => {
                assert_eq!(new_max_tokens, 64_000);
            }
            other => panic!("Expected Escalate, got {other:?}"),
        }
    }

    // ---- Test 8: Continuation message matches TS verbatim ----

    #[test]
    fn continuation_message_matches_ts_text() {
        let msg = create_continuation_message();
        match &msg {
            Message::User(user_msg) => {
                assert!(user_msg.base.is_meta);
                assert!(user_msg.text.contains("Output token limit hit"));
                assert!(user_msg.text.contains("Resume directly"));
                assert!(user_msg.text.contains("Pick up mid-thought"));
                assert!(user_msg.text.contains("Break remaining work"));
            }
            other => panic!("Expected User message, got {other:?}"),
        }
    }

    // ---- Test 9: Default values ----

    #[test]
    fn default_values() {
        let recovery = MaxTokensRecovery::new();
        assert_eq!(recovery.recovery_count(), 0);
        assert!(!recovery.escalation_attempted);
        assert!(recovery.can_recover());
    }
}
