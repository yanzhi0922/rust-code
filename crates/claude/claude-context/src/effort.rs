//! Effort Level system.
//!
//! Provides types and functions for managing the effort level that controls
//! how much reasoning the model applies to a request.
//!
//! Ported from `claude-code-rev/src/utils/effort.ts`.

use serde::{Deserialize, Serialize};

// ── Types ───────────────────────────────────────────────────────────────

/// Effort level for model reasoning.
///
/// Controls how much thinking/reasoning the model applies to a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum EffortLevel {
    /// Quick, straightforward implementation with minimal overhead.
    Low,
    /// Balanced approach with standard implementation and testing.
    #[default]
    Medium,
    /// Comprehensive implementation with extensive testing and documentation.
    High,
    /// Maximum capability with deepest reasoning (Opus 4.6 only).
    Max,
}

impl EffortLevel {
    /// All valid effort levels in order.
    pub const ALL: [EffortLevel; 4] = [
        EffortLevel::Low,
        EffortLevel::Medium,
        EffortLevel::High,
        EffortLevel::Max,
    ];

    /// Convert to the string used in API requests.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Max => "max",
        }
    }
}

impl std::fmt::Display for EffortLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for EffortLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "max" => Ok(Self::Max),
            other => Err(format!("unknown effort level: {other}")),
        }
    }
}

/// Effort value — either a named level or a numeric budget.
///
/// Numeric values are ant-only and not persisted; they represent a
/// percentage-based thinking budget.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EffortValue {
    /// Named effort level.
    Level(EffortLevel),
    /// Numeric effort budget (ant-only, 0-100).
    Budget(u32),
}

impl EffortValue {
    /// Convert an effort value to a displayable effort level.
    ///
    /// Numeric values are mapped to levels using thresholds:
    /// - ≤50 → Low
    /// - ≤85 → Medium
    /// - ≤100 → High
    /// - >100 → Max
    pub fn to_level(&self) -> EffortLevel {
        match self {
            Self::Level(level) => *level,
            Self::Budget(n) => {
                if *n <= 50 {
                    EffortLevel::Low
                } else if *n <= 85 {
                    EffortLevel::Medium
                } else if *n <= 100 {
                    EffortLevel::High
                } else {
                    EffortLevel::Max
                }
            }
        }
    }
}

/// Configuration for effort level resolution.
///
/// Replaces the implicit dependency on environment variables and global
/// auth/config state in the original TypeScript implementation.
#[derive(Debug, Clone)]
pub struct EffortConfig {
    /// Whether the user is an internal "ant" user.
    pub is_ant_user: bool,
    /// Whether the user is a Pro subscriber.
    pub is_pro_subscriber: bool,
    /// Whether the user is a Max subscriber.
    pub is_max_subscriber: bool,
    /// Whether the user is a Team subscriber.
    pub is_team_subscriber: bool,
    /// Whether ultrathink feature is enabled.
    pub ultrathink_enabled: bool,
    /// Whether to always enable effort for all models (env override).
    pub always_enable_effort: bool,
    /// Whether the API provider is first-party.
    pub is_first_party: bool,
    /// Whether the opus default effort config (medium for Pro) is enabled.
    pub opus_default_effort_enabled: bool,
    /// Override for the default model's effort level (ant-only).
    pub ant_default_model_effort: Option<EffortValue>,
}

impl Default for EffortConfig {
    fn default() -> Self {
        Self {
            is_ant_user: false,
            is_pro_subscriber: false,
            is_max_subscriber: false,
            is_team_subscriber: false,
            ultrathink_enabled: false,
            always_enable_effort: false,
            is_first_party: true,
            opus_default_effort_enabled: true,
            ant_default_model_effort: None,
        }
    }
}

// ── Functions ───────────────────────────────────────────────────────────

/// Check if a model supports the effort parameter.
///
/// Equivalent to `modelSupportsEffort()` in effort.ts.
pub fn model_supports_effort(model: &str, config: &EffortConfig) -> bool {
    if config.always_enable_effort {
        return true;
    }

    let m = model.to_ascii_lowercase();

    // Supported by a subset of Claude 4 models
    if m.contains("opus-4-6") || m.contains("sonnet-4-6") {
        return true;
    }

    // Exclude any other known legacy models (haiku, older opus/sonnet variants)
    if m.contains("haiku") || m.contains("sonnet") || m.contains("opus") {
        return false;
    }

    // Default to true for unknown model strings on 1P
    config.is_first_party
}

/// Check if a model supports the 'max' effort level.
///
/// Equivalent to `modelSupportsMaxEffort()` in effort.ts.
/// Per API docs, 'max' is Opus 4.6 only for public models.
pub fn model_supports_max_effort(model: &str, config: &EffortConfig) -> bool {
    if model.to_ascii_lowercase().contains("opus-4-6") {
        return true;
    }
    // Ant users with internal models may support max
    if config.is_ant_user {
        // Internal ant models can support max effort
        return model.to_ascii_lowercase().contains("opus");
    }
    false
}

/// Parse an effort value from a string or number.
///
/// Equivalent to `parseEffortValue()` in effort.ts.
///
/// Returns `None` if the value cannot be parsed.
pub fn parse_effort_value(value: &str) -> Option<EffortValue> {
    if value.is_empty() {
        return None;
    }

    let lower = value.to_ascii_lowercase();

    // Try as a named level first
    if let Ok(level) = lower.parse::<EffortLevel>() {
        return Some(EffortValue::Level(level));
    }

    // Try as a numeric value
    if let Ok(n) = lower.parse::<u32>() {
        return Some(EffortValue::Budget(n));
    }

    None
}

/// Resolve the effort value that will actually be sent to the API.
///
/// Equivalent to `resolveAppliedEffort()` in effort.ts.
///
/// Resolution order:
/// 1. `env_override` (if Some — use it; if explicit `None` — skip effort)
/// 2. `app_state_effort` (user's current setting)
/// 3. Model default
///
/// If the resolved value is 'max' but the model doesn't support it,
/// downgrades to 'high'.
pub fn resolve_applied_effort(
    model: &str,
    app_state_effort: Option<EffortValue>,
    env_override: Option<Option<EffortValue>>,
    config: &EffortConfig,
) -> Option<EffortValue> {
    // env_override = Some(None) means explicitly unset → no effort
    // env_override = Some(Some(value)) means env override → use that value
    // env_override = None means no env override → fall through
    if let Some(inner) = env_override {
        inner?;
    }

    let resolved = env_override
        .flatten()
        .or(app_state_effort)
        .or(get_default_effort_for_model(model, config));

    // API rejects 'max' on non-Opus-4.6 models — downgrade to 'high'
    match resolved {
        Some(EffortValue::Level(EffortLevel::Max)) if !model_supports_max_effort(model, config) => {
            Some(EffortValue::Level(EffortLevel::High))
        }
        other => other,
    }
}

/// Get the " with {level} effort" suffix for display.
///
/// Equivalent to `getEffortSuffix()` in effort.ts.
/// Returns empty string if no effort value is set.
pub fn get_effort_suffix(
    model: &str,
    effort_value: Option<EffortValue>,
    config: &EffortConfig,
) -> String {
    if effort_value.is_none() {
        return String::new();
    }
    let resolved = resolve_applied_effort(model, effort_value, None, config);
    match resolved {
        Some(value) => format!(" with {} effort", value.to_level()),
        None => String::new(),
    }
}

/// Get the default effort level for a model.
///
/// Equivalent to `getDefaultEffortForModel()` in effort.ts.
pub fn get_default_effort_for_model(model: &str, config: &EffortConfig) -> Option<EffortValue> {
    // Ant users may have a default model effort override
    if config.is_ant_user {
        if let Some(ref effort) = config.ant_default_model_effort {
            return Some(*effort);
        }
        // Ant users default to undefined (high)
        return None;
    }

    let m = model.to_ascii_lowercase();

    // Default effort on Opus 4.6 to medium for Pro/Max/Team subscribers
    if m.contains("opus-4-6") {
        if config.is_pro_subscriber {
            return Some(EffortValue::Level(EffortLevel::Medium));
        }
        if config.opus_default_effort_enabled
            && (config.is_max_subscriber || config.is_team_subscriber)
        {
            return Some(EffortValue::Level(EffortLevel::Medium));
        }
    }

    // When ultrathink is on, default effort to medium
    if config.ultrathink_enabled && model_supports_effort(model, config) {
        return Some(EffortValue::Level(EffortLevel::Medium));
    }

    // Fallback: no effort level set → API defaults to high
    None
}

/// Get a human-readable description for an effort level.
///
/// Equivalent to `getEffortLevelDescription()` in effort.ts.
pub fn get_effort_level_description(level: EffortLevel) -> &'static str {
    match level {
        EffortLevel::Low => "Quick, straightforward implementation with minimal overhead",
        EffortLevel::Medium => "Balanced approach with standard implementation and testing",
        EffortLevel::High => {
            "Comprehensive implementation with extensive testing and documentation"
        }
        EffortLevel::Max => "Maximum capability with deepest reasoning (Opus 4.6 only)",
    }
}

/// Get the displayed effort level for the user.
///
/// Equivalent to `getDisplayedEffortLevel()` in effort.ts.
/// Wraps `resolve_applied_effort` with a 'high' fallback when no effort
/// param is sent.
pub fn get_displayed_effort_level(
    model: &str,
    app_state_effort: Option<EffortValue>,
    config: &EffortConfig,
) -> EffortLevel {
    let resolved = resolve_applied_effort(model, app_state_effort, None, config);
    resolved.map(|v| v.to_level()).unwrap_or(EffortLevel::High)
}

/// Determine what effort value should be persisted when the user picks a model.
///
/// Equivalent to `resolvePickerEffortPersistence()` in effort.ts.
/// Keeps an explicit prior effort choice sticky even when it matches the
/// model default, while letting purely-default effort fall through.
pub fn resolve_picker_effort_persistence(
    picked: Option<EffortLevel>,
    model_default: EffortLevel,
    prior_persisted: Option<EffortLevel>,
    toggled_in_picker: bool,
) -> Option<EffortLevel> {
    let had_explicit = prior_persisted.is_some() || toggled_in_picker;
    if had_explicit {
        picked
    } else {
        // Only persist if it differs from the model default
        picked.and_then(|p| if p != model_default { Some(p) } else { None })
    }
}

/// Filter an effort value to only the persistable form.
///
/// Equivalent to `toPersistableEffort()` in effort.ts.
/// Numeric values are never persisted. 'max' is only persisted for ant users.
pub fn to_persistable_effort(value: Option<EffortValue>, is_ant_user: bool) -> Option<EffortLevel> {
    match value {
        Some(EffortValue::Level(level @ EffortLevel::Low))
        | Some(EffortValue::Level(level @ EffortLevel::Medium))
        | Some(EffortValue::Level(level @ EffortLevel::High)) => Some(level),
        Some(EffortValue::Level(EffortLevel::Max)) if is_ant_user => Some(EffortLevel::Max),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_effort_level_from_str() {
        assert_eq!("low".parse::<EffortLevel>(), Ok(EffortLevel::Low));
        assert_eq!("medium".parse::<EffortLevel>(), Ok(EffortLevel::Medium));
        assert_eq!("high".parse::<EffortLevel>(), Ok(EffortLevel::High));
        assert_eq!("max".parse::<EffortLevel>(), Ok(EffortLevel::Max));
        assert!("unknown".parse::<EffortLevel>().is_err());
    }

    #[test]
    fn test_effort_level_display() {
        assert_eq!(EffortLevel::Low.to_string(), "low");
        assert_eq!(EffortLevel::Max.to_string(), "max");
    }

    #[test]
    fn test_effort_value_to_level() {
        assert_eq!(EffortValue::Budget(30).to_level(), EffortLevel::Low);
        assert_eq!(EffortValue::Budget(70).to_level(), EffortLevel::Medium);
        assert_eq!(EffortValue::Budget(95).to_level(), EffortLevel::High);
        assert_eq!(EffortValue::Budget(150).to_level(), EffortLevel::Max);
    }

    #[test]
    fn test_parse_effort_value() {
        assert_eq!(
            parse_effort_value("high"),
            Some(EffortValue::Level(EffortLevel::High))
        );
        assert_eq!(parse_effort_value("75"), Some(EffortValue::Budget(75)));
        assert_eq!(parse_effort_value(""), None);
        assert_eq!(parse_effort_value("invalid"), None);
    }

    #[test]
    fn test_model_supports_effort() {
        let config = EffortConfig::default();
        assert!(model_supports_effort("claude-opus-4-6", &config));
        assert!(model_supports_effort("claude-sonnet-4-6", &config));
        assert!(!model_supports_effort("claude-haiku-4-5", &config));
        assert!(!model_supports_effort("claude-sonnet-4-5", &config));
    }

    #[test]
    fn test_model_supports_max_effort() {
        let config = EffortConfig::default();
        assert!(model_supports_max_effort("claude-opus-4-6", &config));
        assert!(!model_supports_max_effort("claude-sonnet-4-6", &config));
    }

    #[test]
    fn test_resolve_applied_effort_max_downgrade() {
        let config = EffortConfig::default();
        let result = resolve_applied_effort(
            "claude-sonnet-4-6",
            Some(EffortValue::Level(EffortLevel::Max)),
            None,
            &config,
        );
        assert_eq!(result, Some(EffortValue::Level(EffortLevel::High)));
    }

    #[test]
    fn test_resolve_applied_effort_env_unset() {
        let config = EffortConfig::default();
        let result = resolve_applied_effort(
            "claude-sonnet-4-6",
            Some(EffortValue::Level(EffortLevel::High)),
            Some(None), // env explicitly unset
            &config,
        );
        assert_eq!(result, None);
    }

    #[test]
    fn test_get_default_effort_for_model_opus_pro() {
        let config = EffortConfig {
            is_pro_subscriber: true,
            ..Default::default()
        };
        let result = get_default_effort_for_model("claude-opus-4-6", &config);
        assert_eq!(result, Some(EffortValue::Level(EffortLevel::Medium)));
    }

    #[test]
    fn test_get_effort_suffix() {
        let config = EffortConfig::default();
        let suffix = get_effort_suffix(
            "claude-opus-4-6",
            Some(EffortValue::Level(EffortLevel::High)),
            &config,
        );
        assert_eq!(suffix, " with high effort");
    }

    #[test]
    fn test_get_effort_suffix_none() {
        let config = EffortConfig::default();
        let suffix = get_effort_suffix("claude-opus-4-6", None, &config);
        assert_eq!(suffix, "");
    }

    #[test]
    fn test_to_persistable_effort() {
        assert_eq!(
            to_persistable_effort(Some(EffortValue::Level(EffortLevel::High)), false),
            Some(EffortLevel::High)
        );
        assert_eq!(
            to_persistable_effort(Some(EffortValue::Level(EffortLevel::Max)), false),
            None
        );
        assert_eq!(
            to_persistable_effort(Some(EffortValue::Level(EffortLevel::Max)), true),
            Some(EffortLevel::Max)
        );
        assert_eq!(
            to_persistable_effort(Some(EffortValue::Budget(80)), false),
            None
        );
    }

    #[test]
    fn test_get_effort_level_description() {
        assert_eq!(
            get_effort_level_description(EffortLevel::Low),
            "Quick, straightforward implementation with minimal overhead"
        );
        assert_eq!(
            get_effort_level_description(EffortLevel::Max),
            "Maximum capability with deepest reasoning (Opus 4.6 only)"
        );
    }
}
