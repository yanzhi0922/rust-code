//! # rc-context
//!
//! Context management system for AI model interactions.
//!
//! This crate provides three core subsystems:
//!
//! - **[`window`]** — Context window management: sizes, 1M support detection,
//!   usage percentage calculation, and max output token limits per model.
//! - **[`effort`]** — Effort Level system: parsing, resolution, and display
//!   of the effort/reasoning level that controls how much the model thinks.
//! - **[`fast_mode`]** — Fast Mode management: availability checks, cooldown
//!   state tracking, and API rejection handling for the fast mode feature.
//!
//! ## Design Principles
//!
//! All functions are pure or accept configuration parameters — no dependency
//! on global mutable state or environment variables. Configuration structs
//! ([`window::ContextConfig`], [`effort::EffortConfig`], [`fast_mode::FastModeConfig`])
//! are passed explicitly, making the code testable and composable.
//!
//! The one exception is [`fast_mode::FastModeRuntime`] which manages mutable
//! cooldown state via interior mutability (`Arc<Mutex<...>>`).

pub mod effort;
pub mod fast_mode;
pub mod runtime_identity;
pub mod window;

// Re-export the most commonly used types at the crate root.

// ── From window ─────────────────────────────────────────────────────────
pub use window::{
    CAPPED_DEFAULT_MAX_TOKENS, COMPACT_MAX_OUTPUT_TOKENS, ContextConfig, ContextPercentages,
    ContextWindow, ESCALATED_MAX_TOKENS, MODEL_CONTEXT_WINDOW_DEFAULT, MaxOutputTokens, TokenUsage,
    calculate_context_percentages, get_context_window_for_model, get_max_thinking_tokens_for_model,
    get_model_max_output_tokens, has_1m_context, model_supports_1m,
};

// ── From effort ─────────────────────────────────────────────────────────
pub use effort::{
    EffortConfig, EffortLevel, EffortValue, get_default_effort_for_model,
    get_displayed_effort_level, get_effort_level_description, get_effort_suffix,
    model_supports_effort, model_supports_max_effort, parse_effort_value, resolve_applied_effort,
    resolve_picker_effort_persistence, to_persistable_effort,
};

// ── From fast_mode ──────────────────────────────────────────────────────
pub use fast_mode::{
    CooldownReason, FAST_MODE_MODEL_DISPLAY, FastModeConfig, FastModeDisabledReason,
    FastModeRuntime, FastModeSimpleState, FastModeState, OrgFastModeStatus,
    get_disabled_reason_message, get_fast_mode_model, get_fast_mode_simple_state,
    get_initial_fast_mode_setting, get_overage_disabled_message, is_fast_mode_available,
    is_fast_mode_supported_by_model,
};

// ── From runtime_identity ───────────────────────────────────────────────
pub use runtime_identity::{
    RuntimeFeatureGates, RuntimeIdentityContext, RuntimeSubscriptionContext, RuntimeUserType,
};
