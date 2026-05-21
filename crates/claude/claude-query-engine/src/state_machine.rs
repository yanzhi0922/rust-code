//! Explicit state machine for the query engine lifecycle.
//!
//! Each phase represents a distinct step in the query processing pipeline.
//! The state machine validates transitions and records history for debugging.

use serde::{Deserialize, Serialize};

/// Distinct phases in the query engine lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnginePhase {
    /// No query in progress.
    Idle,
    /// Preparing context and prompt before calling the provider.
    Initializing,
    /// Assembling the prompt from system/user/tool messages.
    BuildingPrompt,
    /// Waiting for the LLM provider to respond.
    CallingProvider,
    /// Parsing the provider response into structured messages.
    ProcessingResponse,
    /// Executing tool calls returned by the provider.
    ExecutingTools,
    /// Compacting the conversation to fit within the context window.
    Compacting,
    /// Finalizing the query result and cleaning up.
    Finalizing,
    /// An unrecoverable error occurred.
    Failed,
    /// The query was cancelled by the user or system.
    Cancelled,
}

impl EnginePhase {
    /// Returns true if this phase represents a terminal state.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Idle | Self::Failed | Self::Cancelled)
    }

    /// Returns true if this phase represents an active (non-idle, non-terminal) state.
    #[must_use]
    pub fn is_active(self) -> bool {
        !matches!(self, Self::Idle | Self::Failed | Self::Cancelled)
    }

    /// Returns the set of phases that are valid transitions from this phase.
    #[must_use]
    pub fn valid_transitions(self) -> &'static [EnginePhase] {
        match self {
            Self::Idle => &[Self::Initializing],
            Self::Initializing => &[Self::BuildingPrompt, Self::Failed, Self::Cancelled],
            Self::BuildingPrompt => &[
                Self::CallingProvider,
                Self::Compacting,
                Self::Failed,
                Self::Cancelled,
            ],
            Self::CallingProvider => &[Self::ProcessingResponse, Self::Failed, Self::Cancelled],
            Self::ProcessingResponse => &[
                Self::ExecutingTools,
                Self::Finalizing,
                Self::Failed,
                Self::Cancelled,
            ],
            Self::ExecutingTools => &[
                Self::BuildingPrompt,
                Self::Finalizing,
                Self::Failed,
                Self::Cancelled,
            ],
            Self::Compacting => &[Self::BuildingPrompt, Self::Failed, Self::Cancelled],
            Self::Finalizing => &[Self::Idle],
            Self::Failed => &[Self::Idle],
            Self::Cancelled => &[Self::Idle],
        }
    }
}

impl std::fmt::Display for EnginePhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Idle => "idle",
            Self::Initializing => "initializing",
            Self::BuildingPrompt => "building_prompt",
            Self::CallingProvider => "calling_provider",
            Self::ProcessingResponse => "processing_response",
            Self::ExecutingTools => "executing_tools",
            Self::Compacting => "compacting",
            Self::Finalizing => "finalizing",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        };
        f.write_str(name)
    }
}

/// Reason for a phase transition, matching TS `State.transition.reason`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TransitionReason {
    /// Normal continuation to the next turn.
    NextTurn,
    /// Retrying after a collapse-drain recovery attempt.
    CollapseDrainRetry,
    /// Retrying after a reactive compaction.
    ReactiveCompactRetry,
    /// Escalating max_output_tokens to a higher value.
    MaxOutputTokensEscalate,
    /// Recovering from max_output_tokens by adjusting the limit.
    MaxOutputTokensRecovery,
    /// A stop hook is blocking continuation.
    StopHookBlocking,
    /// Continuing because the token budget has remaining capacity.
    TokenBudgetContinuation,
    /// No specific reason provided.
    #[default]
    Unspecified,
}

impl std::fmt::Display for TransitionReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::NextTurn => "next_turn",
            Self::CollapseDrainRetry => "collapse_drain_retry",
            Self::ReactiveCompactRetry => "reactive_compact_retry",
            Self::MaxOutputTokensEscalate => "max_output_tokens_escalate",
            Self::MaxOutputTokensRecovery => "max_output_tokens_recovery",
            Self::StopHookBlocking => "stop_hook_blocking",
            Self::TokenBudgetContinuation => "token_budget_continuation",
            Self::Unspecified => "unspecified",
        };
        f.write_str(name)
    }
}

/// A recorded phase transition for audit/debugging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseTransition {
    pub from: EnginePhase,
    pub to: EnginePhase,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Why this transition happened. Matches TS `State.transition.reason`.
    pub reason: TransitionReason,
}

/// Error returned when an invalid transition is attempted.
#[derive(Debug, Clone, thiserror::Error)]
#[error("invalid state transition: cannot go from {from} to {to}")]
pub struct InvalidTransition {
    pub from: EnginePhase,
    pub to: EnginePhase,
}

/// Explicit state machine that tracks and validates engine phase transitions.
#[derive(Debug, Clone)]
pub struct StateMachine {
    phase: EnginePhase,
    transitions: Vec<PhaseTransition>,
}

impl StateMachine {
    /// Create a new state machine starting in the `Idle` phase.
    #[must_use]
    pub fn new() -> Self {
        Self {
            phase: EnginePhase::Idle,
            transitions: Vec::new(),
        }
    }

    /// Returns the current phase.
    #[must_use]
    pub fn phase(&self) -> EnginePhase {
        self.phase
    }

    /// Returns the full transition history.
    #[must_use]
    pub fn transitions(&self) -> &[PhaseTransition] {
        &self.transitions
    }

    /// Attempt to transition to a new phase. Returns an error if the
    /// transition is not valid from the current phase.
    pub fn transition(&mut self, target: EnginePhase) -> Result<(), InvalidTransition> {
        self.transition_with_reason(target, TransitionReason::Unspecified)
    }

    /// Attempt to transition to a new phase with an explicit reason.
    pub fn transition_with_reason(
        &mut self,
        target: EnginePhase,
        reason: TransitionReason,
    ) -> Result<(), InvalidTransition> {
        let valid = self.phase.valid_transitions();
        if !valid.contains(&target) {
            return Err(InvalidTransition {
                from: self.phase,
                to: target,
            });
        }
        let record = PhaseTransition {
            from: self.phase,
            to: target,
            timestamp: chrono::Utc::now(),
            reason,
        };
        self.phase = target;
        self.transitions.push(record);
        Ok(())
    }

    /// Force-set the phase without validation. Used for error recovery.
    pub fn force_set(&mut self, phase: EnginePhase) {
        self.force_set_with_reason(phase, TransitionReason::Unspecified);
    }

    /// Force-set the phase with an explicit reason.
    pub fn force_set_with_reason(&mut self, phase: EnginePhase, reason: TransitionReason) {
        if self.phase != phase {
            let record = PhaseTransition {
                from: self.phase,
                to: phase,
                timestamp: chrono::Utc::now(),
                reason,
            };
            self.phase = phase;
            self.transitions.push(record);
        }
    }

    /// Reset the state machine back to `Idle`, clearing transition history.
    pub fn reset(&mut self) {
        self.phase = EnginePhase::Idle;
        self.transitions.clear();
    }

    /// Returns the number of recorded transitions.
    #[must_use]
    pub fn transition_count(&self) -> usize {
        self.transitions.len()
    }

    /// Returns true if the state machine is in a terminal phase.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        self.phase.is_terminal()
    }

    /// Returns true if the state machine is in an active phase.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.phase.is_active()
    }
}

impl Default for StateMachine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{EnginePhase, StateMachine};

    #[test]
    fn state_machine_starts_idle() {
        let sm = StateMachine::new();
        assert_eq!(sm.phase(), EnginePhase::Idle);
        assert!(sm.transitions().is_empty());
        assert!(sm.is_terminal());
        assert!(!sm.is_active());
    }

    #[test]
    fn state_machine_happy_path_transitions() {
        let mut sm = StateMachine::new();
        sm.transition(EnginePhase::Initializing)
            .expect("idle -> initializing");
        sm.transition(EnginePhase::BuildingPrompt)
            .expect("initializing -> building_prompt");
        sm.transition(EnginePhase::CallingProvider)
            .expect("building_prompt -> calling_provider");
        sm.transition(EnginePhase::ProcessingResponse)
            .expect("calling_provider -> processing_response");
        sm.transition(EnginePhase::ExecutingTools)
            .expect("processing_response -> executing_tools");
        sm.transition(EnginePhase::Finalizing)
            .expect("executing_tools -> finalizing");
        sm.transition(EnginePhase::Idle)
            .expect("finalizing -> idle");
        assert_eq!(sm.transition_count(), 7);
    }

    #[test]
    fn state_machine_multi_turn_loop_back() {
        let mut sm = StateMachine::new();
        sm.transition(EnginePhase::Initializing)
            .expect("transition");
        sm.transition(EnginePhase::BuildingPrompt)
            .expect("transition");
        sm.transition(EnginePhase::CallingProvider)
            .expect("transition");
        sm.transition(EnginePhase::ProcessingResponse)
            .expect("transition");
        sm.transition(EnginePhase::ExecutingTools)
            .expect("transition");
        // Loop back for next turn
        sm.transition(EnginePhase::BuildingPrompt)
            .expect("executing_tools -> building_prompt loop-back");
        sm.transition(EnginePhase::CallingProvider)
            .expect("transition");
        assert_eq!(sm.phase(), EnginePhase::CallingProvider);
    }

    #[test]
    fn state_machine_rejects_invalid_transition() {
        let mut sm = StateMachine::new();
        let result = sm.transition(EnginePhase::CallingProvider);
        assert!(result.is_err());
        let err = result.expect_err("should be invalid");
        assert_eq!(err.from, EnginePhase::Idle);
        assert_eq!(err.to, EnginePhase::CallingProvider);
        assert_eq!(sm.phase(), EnginePhase::Idle);
    }

    #[test]
    fn state_machine_force_set_bypasses_validation() {
        let mut sm = StateMachine::new();
        sm.force_set(EnginePhase::ExecutingTools);
        assert_eq!(sm.phase(), EnginePhase::ExecutingTools);
        assert_eq!(sm.transition_count(), 1);
    }

    #[test]
    fn state_machine_reset_clears_history() {
        let mut sm = StateMachine::new();
        sm.transition(EnginePhase::Initializing)
            .expect("transition");
        sm.transition(EnginePhase::Failed).expect("transition");
        assert_eq!(sm.transition_count(), 2);
        sm.reset();
        assert_eq!(sm.phase(), EnginePhase::Idle);
        assert!(sm.transitions().is_empty());
    }

    #[test]
    fn state_machine_compacting_loop() {
        let mut sm = StateMachine::new();
        sm.transition(EnginePhase::Initializing)
            .expect("transition");
        sm.transition(EnginePhase::BuildingPrompt)
            .expect("transition");
        sm.transition(EnginePhase::Compacting).expect("transition");
        sm.transition(EnginePhase::BuildingPrompt)
            .expect("compacting -> building_prompt loop");
        sm.transition(EnginePhase::CallingProvider)
            .expect("transition");
        assert_eq!(sm.phase(), EnginePhase::CallingProvider);
    }

    #[test]
    fn engine_phase_display() {
        assert_eq!(EnginePhase::Idle.to_string(), "idle");
        assert_eq!(EnginePhase::CallingProvider.to_string(), "calling_provider");
        assert_eq!(EnginePhase::ExecutingTools.to_string(), "executing_tools");
    }

    #[test]
    fn engine_phase_terminal_and_active_flags() {
        assert!(EnginePhase::Idle.is_terminal());
        assert!(EnginePhase::Failed.is_terminal());
        assert!(EnginePhase::Cancelled.is_terminal());
        assert!(!EnginePhase::Idle.is_active());
        assert!(EnginePhase::Initializing.is_active());
        assert!(EnginePhase::ExecutingTools.is_active());
    }

    #[test]
    fn state_machine_failed_to_idle_recovery() {
        let mut sm = StateMachine::new();
        sm.transition(EnginePhase::Initializing)
            .expect("transition");
        sm.transition(EnginePhase::Failed).expect("transition");
        sm.transition(EnginePhase::Idle)
            .expect("failed -> idle recovery");
        assert_eq!(sm.phase(), EnginePhase::Idle);
    }

    #[test]
    fn state_machine_cancelled_to_idle() {
        let mut sm = StateMachine::new();
        sm.transition(EnginePhase::Initializing)
            .expect("transition");
        sm.transition(EnginePhase::Cancelled).expect("transition");
        sm.transition(EnginePhase::Idle).expect("cancelled -> idle");
        assert_eq!(sm.phase(), EnginePhase::Idle);
    }
}
