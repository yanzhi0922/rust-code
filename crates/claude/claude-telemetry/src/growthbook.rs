//! GrowthBook A/B Testing integration for feature flags and experiments.
//!
//! Provides feature flag evaluation, feature gates, remote evaluation,
//! user attribute tracking, and configuration management compatible with
//! the GrowthBook experimentation platform.
//!
//! # Architecture
//!
//! - [`FeatureFlag`] — individual feature flag with evaluation logic
//! - [`FeatureGate`] — named gates (e.g. `KAIROS`, `KAIROS_CHANNELS`)
//! - [`GrowthBookClient`] — central client managing flags, attributes, and caching
//! - [`UserAttributes`] — 15+ field user attribute set for targeting
//! - [`GrowthBookConfig`] — persistent configuration and overrides

use std::collections::HashMap;
use std::sync::RwLock;

use anyhow::Result;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Platform
// ---------------------------------------------------------------------------

/// Operating system platform identifier for GrowthBook targeting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    /// Windows.
    Win32,
    /// macOS.
    Darwin,
    /// Linux.
    Linux,
}

impl Platform {
    /// Returns the current platform based on compile-time target.
    #[must_use]
    pub fn current() -> Self {
        #[cfg(target_os = "windows")]
        {
            Self::Win32
        }
        #[cfg(target_os = "macos")]
        {
            Self::Darwin
        }
        #[cfg(target_os = "linux")]
        {
            Self::Linux
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        {
            Self::Linux
        }
    }

    /// Returns the string representation used in GrowthBook attributes.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Win32 => "win32",
            Self::Darwin => "darwin",
            Self::Linux => "linux",
        }
    }
}

// ---------------------------------------------------------------------------
// User Attributes
// ---------------------------------------------------------------------------

/// User attributes sent to GrowthBook for targeting decisions.
/// Contains 15+ fields for granular audience segmentation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserAttributes {
    /// Unique user identifier.
    pub id: String,
    /// Current session identifier.
    pub session_id: String,
    /// Device identifier.
    pub device_id: String,
    /// Operating system platform.
    pub platform: Platform,
    /// API base URL hostname (for enterprise proxy detection).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_base_url_host: Option<String>,
    /// Organization UUID for enterprise targeting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organization_uuid: Option<String>,
    /// Account UUID for billing-level targeting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_uuid: Option<String>,
    /// User type (e.g. "ant", "external").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_type: Option<String>,
    /// Subscription tier (e.g. "pro", "max").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription_type: Option<String>,
    /// Rate limit tier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit_tier: Option<String>,
    /// Timestamp of first token generation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_token_time: Option<u64>,
    /// User email address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Application version string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    /// GitHub Actions metadata for CI/CD targeting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github_actions_run_id: Option<String>,
    /// GitHub repository (owner/repo format).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github_repository: Option<String>,
    /// Whether this is a non-interactive session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_non_interactive: Option<bool>,
}

impl Default for UserAttributes {
    fn default() -> Self {
        Self {
            id: String::new(),
            session_id: String::new(),
            device_id: String::new(),
            platform: Platform::current(),
            api_base_url_host: None,
            organization_uuid: None,
            account_uuid: None,
            user_type: None,
            subscription_type: None,
            rate_limit_tier: None,
            first_token_time: None,
            email: None,
            app_version: None,
            github_actions_run_id: None,
            github_repository: None,
            is_non_interactive: None,
        }
    }
}

impl UserAttributes {
    /// Creates a new `UserAttributes` with the given identifiers.
    #[must_use]
    pub fn new(id: String, session_id: String, device_id: String) -> Self {
        Self {
            id,
            session_id,
            device_id,
            ..Self::default()
        }
    }

    /// Returns the number of non-empty attribute fields.
    #[must_use]
    pub fn populated_field_count(&self) -> usize {
        let mut count = 0usize;
        if !self.id.is_empty() {
            count += 1;
        }
        if !self.session_id.is_empty() {
            count += 1;
        }
        if !self.device_id.is_empty() {
            count += 1;
        }
        count += 1; // platform is always set
        if self.api_base_url_host.is_some() {
            count += 1;
        }
        if self.organization_uuid.is_some() {
            count += 1;
        }
        if self.account_uuid.is_some() {
            count += 1;
        }
        if self.user_type.is_some() {
            count += 1;
        }
        if self.subscription_type.is_some() {
            count += 1;
        }
        if self.rate_limit_tier.is_some() {
            count += 1;
        }
        if self.first_token_time.is_some() {
            count += 1;
        }
        if self.email.is_some() {
            count += 1;
        }
        if self.app_version.is_some() {
            count += 1;
        }
        if self.github_actions_run_id.is_some() {
            count += 1;
        }
        if self.github_repository.is_some() {
            count += 1;
        }
        if self.is_non_interactive.is_some() {
            count += 1;
        }
        count
    }
}

// ---------------------------------------------------------------------------
// Feature Flag
// ---------------------------------------------------------------------------

/// The result of evaluating a feature flag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FeatureValue {
    /// Feature is a simple boolean toggle.
    Bool(bool),
    /// Feature is a string value (e.g. variant name).
    String(String),
    /// Feature is a numeric value.
    Number(f64),
    /// Feature is a structured JSON value.
    Json(serde_json::Value),
}

impl FeatureValue {
    /// Returns `true` if the feature value represents an enabled/truthy state.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        match self {
            Self::Bool(b) => *b,
            Self::String(s) => !s.is_empty(),
            Self::Number(n) => *n != 0.0,
            Self::Json(v) => !v.is_null(),
        }
    }

    /// Attempts to extract a boolean value.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            Self::String(s) if s == "true" => Some(true),
            Self::String(s) if s == "false" => Some(false),
            Self::Number(n) => Some(*n != 0.0),
            Self::Json(serde_json::Value::Bool(b)) => Some(*b),
            _ => None,
        }
    }

    /// Attempts to extract a string value.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            Self::Json(serde_json::Value::String(s)) => Some(s),
            _ => None,
        }
    }
}

/// A single feature flag definition with its evaluation rules.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureFlag {
    /// Unique feature key (e.g. "tengu_session_memory").
    pub key: String,
    /// Default value when no rules match.
    pub default_value: FeatureValue,
    /// Optional experiment ID if this flag is part of an A/B test.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experiment_id: Option<String>,
    /// Variation ID for experiment tracking.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variation_id: Option<u32>,
}

impl FeatureFlag {
    /// Creates a new boolean feature flag.
    #[must_use]
    pub fn boolean(key: impl Into<String>, default: bool) -> Self {
        Self {
            key: key.into(),
            default_value: FeatureValue::Bool(default),
            experiment_id: None,
            variation_id: None,
        }
    }

    /// Creates a new string feature flag.
    #[must_use]
    pub fn string(key: impl Into<String>, default: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            default_value: FeatureValue::String(default.into()),
            experiment_id: None,
            variation_id: None,
        }
    }

    /// Evaluates the flag against the given overrides and returns the resolved value.
    #[must_use]
    pub fn evaluate(&self, overrides: &HashMap<String, FeatureValue>) -> FeatureValue {
        if let Some(value) = overrides.get(&self.key) {
            return value.clone();
        }
        self.default_value.clone()
    }
}

// ---------------------------------------------------------------------------
// Feature Gate
// ---------------------------------------------------------------------------

/// Named feature gates used for security and access control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FeatureGate {
    /// KAIROS — main gate for the KAIROS feature set.
    Kairos,
    /// KAIROS_CHANNELS — channel-level gating.
    KairosChannels,
    /// Session memory auto-extraction.
    SessionMemory,
    /// Prompt suggestion system.
    PromptSuggestion,
    /// Speculation mode for speculative execution.
    Speculation,
    /// Auto-compact context management.
    AutoCompact,
    /// First-party event logging.
    FirstPartyEventLogging,
    /// Tool use summary generation.
    ToolUseSummary,
    /// Away summary generation.
    AwaySummary,
    /// Skill search feature.
    SkillSearch,
}

impl FeatureGate {
    /// Returns the GrowthBook feature key for this gate.
    #[must_use]
    pub fn feature_key(self) -> &'static str {
        match self {
            Self::Kairos => "tengu_kairos",
            Self::KairosChannels => "tengu_kairos_channels",
            Self::SessionMemory => "tengu_session_memory",
            Self::PromptSuggestion => "tengu_chomp_inflection",
            Self::Speculation => "tengu_speculation",
            Self::AutoCompact => "tengu_auto_compact",
            Self::FirstPartyEventLogging => "tengu_1p_event_logging",
            Self::ToolUseSummary => "tengu_tool_use_summary",
            Self::AwaySummary => "tengu_away_summary",
            Self::SkillSearch => "tengu_skill_search",
        }
    }

    /// Returns the default value for this gate when not configured.
    #[must_use]
    pub fn default_value(self) -> bool {
        match self {
            Self::Kairos => false,
            Self::KairosChannels => false,
            Self::SessionMemory => false,
            Self::PromptSuggestion => false,
            Self::Speculation => false,
            Self::AutoCompact => true,
            Self::FirstPartyEventLogging => false,
            Self::ToolUseSummary => true,
            Self::AwaySummary => true,
            Self::SkillSearch => false,
        }
    }

    /// Returns all known feature gates.
    #[must_use]
    pub fn all_gates() -> &'static [FeatureGate] {
        &[
            Self::Kairos,
            Self::KairosChannels,
            Self::SessionMemory,
            Self::PromptSuggestion,
            Self::Speculation,
            Self::AutoCompact,
            Self::FirstPartyEventLogging,
            Self::ToolUseSummary,
            Self::AwaySummary,
            Self::SkillSearch,
        ]
    }
}

// ---------------------------------------------------------------------------
// Remote Eval
// ---------------------------------------------------------------------------

/// A remote evaluation response from the GrowthBook server.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RemoteEvalResponse {
    /// Map of feature key → evaluated value.
    pub features: HashMap<String, FeatureValue>,
    /// Whether the response is from a forced evaluation.
    pub forced: bool,
    /// Timestamp of the evaluation (epoch millis).
    pub timestamp: u64,
}

/// Tracks experiment data for exposure logging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentData {
    /// Experiment identifier.
    pub experiment_id: String,
    /// Variation bucket assigned to this user.
    pub variation_id: u32,
    /// Whether the user is included in the experiment.
    pub in_experiment: bool,
    /// Hash attribute used for bucketing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash_attribute: Option<String>,
    /// Hash value used for bucketing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash_value: Option<String>,
}

// ---------------------------------------------------------------------------
// GrowthBook Config
// ---------------------------------------------------------------------------

/// Persistent GrowthBook configuration stored on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrowthBookConfig {
    /// Cached feature values from the last successful remote eval.
    #[serde(default)]
    pub cached_features: HashMap<String, FeatureValue>,
    /// Manual overrides set via config UI (ant-only).
    #[serde(default)]
    pub overrides: HashMap<String, FeatureValue>,
    /// Client key for the GrowthBook API.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_key: Option<String>,
    /// API host override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_host: Option<String>,
    /// Whether GrowthBook is enabled for this installation.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl Default for GrowthBookConfig {
    fn default() -> Self {
        Self {
            cached_features: HashMap::new(),
            overrides: HashMap::new(),
            client_key: None,
            api_host: None,
            enabled: true,
        }
    }
}

impl GrowthBookConfig {
    /// Creates a new config with the given client key.
    #[must_use]
    pub fn with_client_key(client_key: impl Into<String>) -> Self {
        Self {
            client_key: Some(client_key.into()),
            ..Self::default()
        }
    }

    /// Sets a config override for a feature key.
    pub fn set_override(&mut self, feature: impl Into<String>, value: FeatureValue) {
        self.overrides.insert(feature.into(), value);
    }

    /// Clears a config override for a feature key.
    pub fn clear_override(&mut self, feature: &str) {
        self.overrides.remove(feature);
    }

    /// Clears all config overrides.
    pub fn clear_all_overrides(&mut self) {
        self.overrides.clear();
    }

    /// Updates the cached features from a remote eval response.
    pub fn update_cached_features(&mut self, features: HashMap<String, FeatureValue>) {
        self.cached_features = features;
    }

    /// Resolves a feature value using the priority: overrides → cached → default.
    #[must_use]
    pub fn resolve_feature(&self, key: &str, default: &FeatureValue) -> FeatureValue {
        if let Some(v) = self.overrides.get(key) {
            return v.clone();
        }
        if let Some(v) = self.cached_features.get(key) {
            return v.clone();
        }
        default.clone()
    }
}

// ---------------------------------------------------------------------------
// GrowthBook Client
// ---------------------------------------------------------------------------

/// Central GrowthBook client managing feature flags, user attributes, and caching.
pub struct GrowthBookClient {
    /// Current configuration (overrides + cached features).
    config: RwLock<GrowthBookConfig>,
    /// Current user attributes for targeting.
    attributes: RwLock<UserAttributes>,
    /// Feature flags registered with this client.
    flags: RwLock<HashMap<String, FeatureFlag>>,
    /// Experiment data for exposure logging.
    experiments: RwLock<HashMap<String, ExperimentData>>,
    /// Features that have had exposure logged this session.
    logged_exposures: RwLock<std::collections::HashSet<String>>,
    /// Whether the client has been initialized.
    initialized: RwLock<bool>,
}

impl GrowthBookClient {
    /// Creates a new GrowthBook client with the given config.
    #[must_use]
    pub fn new(config: GrowthBookConfig) -> Self {
        Self {
            config: RwLock::new(config),
            attributes: RwLock::new(UserAttributes::default()),
            flags: RwLock::new(HashMap::new()),
            experiments: RwLock::new(HashMap::new()),
            logged_exposures: RwLock::new(std::collections::HashSet::new()),
            initialized: RwLock::new(false),
        }
    }

    /// Creates a client with default configuration.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(GrowthBookConfig::default())
    }

    /// Sets the user attributes for targeting.
    pub fn set_attributes(&self, attrs: UserAttributes) {
        if let Ok(mut guard) = self.attributes.write() {
            *guard = attrs;
        }
    }

    /// Registers a feature flag with the client.
    pub fn register_flag(&self, flag: FeatureFlag) {
        if let Ok(mut guard) = self.flags.write() {
            guard.insert(flag.key.clone(), flag);
        }
    }

    /// Marks the client as initialized.
    pub fn mark_initialized(&self) {
        if let Ok(mut guard) = self.initialized.write() {
            *guard = true;
        }
    }

    /// Returns whether the client has been initialized.
    #[must_use]
    pub fn is_initialized(&self) -> bool {
        self.initialized.read().is_ok_and(|g| *g)
    }

    /// Gets a feature value by key, using the cached (possibly stale) value.
    /// Returns the default if the feature is not found.
    #[must_use]
    pub fn get_feature_value_cached(&self, key: &str, default: bool) -> bool {
        let config = self.config.read().ok();
        let default_val = FeatureValue::Bool(default);

        let resolved = config.as_ref().map_or(default_val.clone(), |c| {
            c.resolve_feature(key, &default_val)
        });

        resolved.as_bool().unwrap_or(default)
    }

    /// Gets a feature value by key as a string.
    #[must_use]
    pub fn get_feature_string_cached(&self, key: &str, default: &str) -> String {
        let config = self.config.read().ok();
        let default_val = FeatureValue::String(default.to_string());

        config.as_ref().map_or(default.to_string(), |c| {
            c.resolve_feature(key, &default_val)
                .as_str()
                .map_or(default.to_string(), ToString::to_string)
        })
    }

    /// Evaluates a feature gate and returns whether it is enabled.
    #[must_use]
    pub fn is_gate_enabled(&self, gate: FeatureGate) -> bool {
        self.get_feature_value_cached(gate.feature_key(), gate.default_value())
    }

    /// Processes a remote eval response and updates cached features.
    pub fn process_remote_eval(&self, response: RemoteEvalResponse) -> Result<()> {
        if response.features.is_empty() {
            return Ok(());
        }

        if let Ok(mut config) = self.config.write() {
            config.update_cached_features(response.features);
        }

        self.mark_initialized();
        Ok(())
    }

    /// Sets a config override for a feature key.
    pub fn set_override(&self, feature: impl Into<String>, value: FeatureValue) {
        if let Ok(mut config) = self.config.write() {
            config.set_override(feature, value);
        }
    }

    /// Clears a config override.
    pub fn clear_override(&self, feature: &str) {
        if let Ok(mut config) = self.config.write() {
            config.clear_override(feature);
        }
    }

    /// Returns all current feature values (overrides + cached).
    #[must_use]
    pub fn get_all_features(&self) -> HashMap<String, FeatureValue> {
        self.config.read().map_or_else(
            |_| HashMap::new(),
            |c| {
                let mut result = c.cached_features.clone();
                for (k, v) in &c.overrides {
                    result.insert(k.clone(), v.clone());
                }
                result
            },
        )
    }

    /// Records experiment data for a feature.
    pub fn record_experiment(&self, feature_key: String, data: ExperimentData) {
        if let Ok(mut guard) = self.experiments.write() {
            guard.insert(feature_key, data);
        }
    }

    /// Logs exposure for a feature if it has experiment data and hasn't been logged yet.
    /// Returns `true` if exposure was logged, `false` if skipped (already logged or no data).
    pub fn log_exposure(&self, feature: &str) -> bool {
        let should_log = {
            let exposures = self.logged_exposures.read().ok();
            exposures.as_ref().is_some_and(|e| !e.contains(feature))
        };

        if !should_log {
            return false;
        }

        let has_experiment = self
            .experiments
            .read()
            .ok()
            .is_some_and(|e| e.contains_key(feature));

        if has_experiment {
            if let Ok(mut guard) = self.logged_exposures.write() {
                guard.insert(feature.to_string());
            }
            return true;
        }

        false
    }

    /// Resets the client state (for testing or auth change).
    pub fn reset(&self) {
        if let Ok(mut guard) = self.logged_exposures.write() {
            guard.clear();
        }
        if let Ok(mut guard) = self.experiments.write() {
            guard.clear();
        }
        if let Ok(mut guard) = self.initialized.write() {
            *guard = false;
        }
    }

    /// Returns the current user attributes.
    #[must_use]
    pub fn get_attributes(&self) -> UserAttributes {
        self.attributes
            .read()
            .map_or_else(|_| UserAttributes::default(), |a| a.clone())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_client() -> GrowthBookClient {
        GrowthBookClient::with_defaults()
    }

    fn make_test_attributes() -> UserAttributes {
        UserAttributes::new(
            "user-123".to_string(),
            "session-456".to_string(),
            "device-789".to_string(),
        )
    }

    // --- Platform tests ---

    #[test]
    fn test_platform_current_returns_valid() {
        let p = Platform::current();
        assert!(matches!(
            p,
            Platform::Win32 | Platform::Darwin | Platform::Linux
        ));
    }

    #[test]
    fn test_platform_as_str() {
        assert_eq!(Platform::Win32.as_str(), "win32");
        assert_eq!(Platform::Darwin.as_str(), "darwin");
        assert_eq!(Platform::Linux.as_str(), "linux");
    }

    // --- UserAttributes tests ---

    #[test]
    fn test_user_attributes_default() {
        let attrs = UserAttributes::default();
        assert!(attrs.id.is_empty());
        assert!(attrs.session_id.is_empty());
        assert!(attrs.api_base_url_host.is_none());
    }

    #[test]
    fn test_user_attributes_new() {
        let attrs = make_test_attributes();
        assert_eq!(attrs.id, "user-123");
        assert_eq!(attrs.session_id, "session-456");
        assert_eq!(attrs.device_id, "device-789");
    }

    #[test]
    fn test_user_attributes_populated_field_count() {
        let attrs = make_test_attributes();
        // id, session_id, device_id, platform = 4
        assert_eq!(attrs.populated_field_count(), 4);
    }

    #[test]
    fn test_user_attributes_populated_with_optional() {
        let mut attrs = make_test_attributes();
        attrs.email = Some("test@example.com".to_string());
        attrs.organization_uuid = Some("org-uuid".to_string());
        attrs.subscription_type = Some("pro".to_string());
        // 4 base + 3 optional = 7
        assert_eq!(attrs.populated_field_count(), 7);
    }

    #[test]
    fn test_user_attributes_serialization_roundtrip() {
        let mut attrs = make_test_attributes();
        attrs.email = Some("test@example.com".to_string());
        let json = serde_json::to_string(&attrs).expect("serialize");
        let deserialized: UserAttributes = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.id, "user-123");
        assert_eq!(deserialized.email, Some("test@example.com".to_string()));
    }

    // --- FeatureValue tests ---

    #[test]
    fn test_feature_value_bool_is_enabled() {
        assert!(FeatureValue::Bool(true).is_enabled());
        assert!(!FeatureValue::Bool(false).is_enabled());
    }

    #[test]
    fn test_feature_value_string_is_enabled() {
        assert!(FeatureValue::String("hello".to_string()).is_enabled());
        assert!(!FeatureValue::String(String::new()).is_enabled());
    }

    #[test]
    fn test_feature_value_as_bool() {
        assert_eq!(FeatureValue::Bool(true).as_bool(), Some(true));
        assert_eq!(FeatureValue::Bool(false).as_bool(), Some(false));
        assert_eq!(
            FeatureValue::String("true".to_string()).as_bool(),
            Some(true)
        );
        assert_eq!(FeatureValue::Number(1.0).as_bool(), Some(true));
        assert_eq!(FeatureValue::Number(0.0).as_bool(), Some(false));
    }

    #[test]
    fn test_feature_value_as_str() {
        assert_eq!(
            FeatureValue::String("variant_a".to_string()).as_str(),
            Some("variant_a")
        );
        assert_eq!(FeatureValue::Bool(true).as_str(), None);
    }

    // --- FeatureFlag tests ---

    #[test]
    fn test_feature_flag_boolean() {
        let flag = FeatureFlag::boolean("test_flag", false);
        assert_eq!(flag.key, "test_flag");
        assert_eq!(flag.default_value, FeatureValue::Bool(false));
    }

    #[test]
    fn test_feature_flag_evaluate_with_override() {
        let flag = FeatureFlag::boolean("test_flag", false);
        let mut overrides = HashMap::new();
        overrides.insert("test_flag".to_string(), FeatureValue::Bool(true));
        let result = flag.evaluate(&overrides);
        assert_eq!(result, FeatureValue::Bool(true));
    }

    #[test]
    fn test_feature_flag_evaluate_without_override() {
        let flag = FeatureFlag::boolean("test_flag", false);
        let overrides = HashMap::new();
        let result = flag.evaluate(&overrides);
        assert_eq!(result, FeatureValue::Bool(false));
    }

    // --- FeatureGate tests ---

    #[test]
    fn test_feature_gate_keys() {
        assert_eq!(FeatureGate::Kairos.feature_key(), "tengu_kairos");
        assert_eq!(
            FeatureGate::SessionMemory.feature_key(),
            "tengu_session_memory"
        );
        assert_eq!(
            FeatureGate::PromptSuggestion.feature_key(),
            "tengu_chomp_inflection"
        );
    }

    #[test]
    fn test_feature_gate_defaults() {
        assert!(!FeatureGate::Kairos.default_value());
        assert!(FeatureGate::AutoCompact.default_value());
        assert!(!FeatureGate::SessionMemory.default_value());
    }

    #[test]
    fn test_feature_gate_all_gates() {
        let gates = FeatureGate::all_gates();
        assert_eq!(gates.len(), 10);
    }

    // --- GrowthBookConfig tests ---

    #[test]
    fn test_config_default() {
        let config = GrowthBookConfig::default();
        assert!(config.cached_features.is_empty());
        assert!(config.overrides.is_empty());
        assert!(config.enabled);
    }

    #[test]
    fn test_config_set_and_clear_override() {
        let mut config = GrowthBookConfig::default();
        config.set_override("my_flag", FeatureValue::Bool(true));
        assert!(config.overrides.contains_key("my_flag"));
        config.clear_override("my_flag");
        assert!(!config.overrides.contains_key("my_flag"));
    }

    #[test]
    fn test_config_resolve_feature_priority() {
        let mut config = GrowthBookConfig::default();
        // No override, no cache → default
        let val = config.resolve_feature("flag1", &FeatureValue::Bool(false));
        assert_eq!(val, FeatureValue::Bool(false));

        // Add cached value
        let mut cached = HashMap::new();
        cached.insert("flag1".to_string(), FeatureValue::Bool(true));
        config.update_cached_features(cached);
        let val = config.resolve_feature("flag1", &FeatureValue::Bool(false));
        assert_eq!(val, FeatureValue::Bool(true));

        // Override takes priority
        config.set_override("flag1", FeatureValue::Bool(false));
        let val = config.resolve_feature("flag1", &FeatureValue::Bool(true));
        assert_eq!(val, FeatureValue::Bool(false));
    }

    #[test]
    fn test_config_clear_all_overrides() {
        let mut config = GrowthBookConfig::default();
        config.set_override("a", FeatureValue::Bool(true));
        config.set_override("b", FeatureValue::Bool(false));
        assert_eq!(config.overrides.len(), 2);
        config.clear_all_overrides();
        assert!(config.overrides.is_empty());
    }

    // --- GrowthBookClient tests ---

    #[test]
    fn test_client_new_and_initialized() {
        let client = make_test_client();
        assert!(!client.is_initialized());
        client.mark_initialized();
        assert!(client.is_initialized());
    }

    #[test]
    fn test_client_get_feature_value_default() {
        let client = make_test_client();
        let val = client.get_feature_value_cached("nonexistent", false);
        assert!(!val);
    }

    #[test]
    fn test_client_set_override_affects_value() {
        let client = make_test_client();
        client.set_override("my_flag", FeatureValue::Bool(true));
        let val = client.get_feature_value_cached("my_flag", false);
        assert!(val);
    }

    #[test]
    fn test_client_gate_enabled() {
        let client = make_test_client();
        assert!(!client.is_gate_enabled(FeatureGate::Kairos));
        assert!(client.is_gate_enabled(FeatureGate::AutoCompact));
    }

    #[test]
    fn test_client_process_remote_eval() {
        let client = make_test_client();
        let mut features = HashMap::new();
        features.insert("tengu_kairos".to_string(), FeatureValue::Bool(true));
        let response = RemoteEvalResponse {
            features,
            forced: false,
            timestamp: 12345,
        };
        client
            .process_remote_eval(response)
            .expect("should succeed");
        assert!(client.is_initialized());
        assert!(client.is_gate_enabled(FeatureGate::Kairos));
    }

    #[test]
    fn test_client_process_empty_eval() {
        let client = make_test_client();
        let response = RemoteEvalResponse::default();
        client
            .process_remote_eval(response)
            .expect("should succeed");
        assert!(!client.is_initialized());
    }

    #[test]
    fn test_client_set_attributes() {
        let client = make_test_client();
        let attrs = make_test_attributes();
        client.set_attributes(attrs);
        let retrieved = client.get_attributes();
        assert_eq!(retrieved.id, "user-123");
    }

    #[test]
    fn test_client_register_flag() {
        let client = make_test_client();
        let flag = FeatureFlag::boolean("custom_flag", true);
        client.register_flag(flag);
        // Registering a flag stores it but doesn't affect get_feature_value_cached
        // which only checks overrides and cached features.
        // The flag is stored in the internal flags map for reference.
        let val = client.get_feature_value_cached("custom_flag", false);
        // No override or cache, so returns the default parameter
        assert!(!val);
        // But after setting an override, it should return true
        client.set_override("custom_flag", FeatureValue::Bool(true));
        let val = client.get_feature_value_cached("custom_flag", false);
        assert!(val);
    }

    #[test]
    fn test_client_log_exposure_dedup() {
        let client = make_test_client();
        client.record_experiment(
            "test_flag".to_string(),
            ExperimentData {
                experiment_id: "exp-1".to_string(),
                variation_id: 0,
                in_experiment: true,
                hash_attribute: None,
                hash_value: None,
            },
        );
        assert!(client.log_exposure("test_flag"));
        assert!(!client.log_exposure("test_flag")); // dedup
    }

    #[test]
    fn test_client_log_exposure_no_experiment() {
        let client = make_test_client();
        assert!(!client.log_exposure("nonexistent"));
    }

    #[test]
    fn test_client_reset() {
        let client = make_test_client();
        client.mark_initialized();
        client.record_experiment(
            "flag".to_string(),
            ExperimentData {
                experiment_id: "e".to_string(),
                variation_id: 0,
                in_experiment: true,
                hash_attribute: None,
                hash_value: None,
            },
        );
        assert!(client.log_exposure("flag"));
        client.reset();
        assert!(!client.is_initialized());
        // After reset, experiment data is gone
        assert!(!client.log_exposure("flag"));
    }

    #[test]
    fn test_client_get_all_features() {
        let client = make_test_client();
        client.set_override("a", FeatureValue::Bool(true));
        let all = client.get_all_features();
        assert_eq!(all.len(), 1);
        assert_eq!(all["a"], FeatureValue::Bool(true));
    }

    #[test]
    fn test_remote_eval_response_serialization() {
        let mut features = HashMap::new();
        features.insert("flag1".to_string(), FeatureValue::Bool(true));
        let response = RemoteEvalResponse {
            features,
            forced: true,
            timestamp: 9999,
        };
        let json = serde_json::to_string(&response).expect("serialize");
        let deserialized: RemoteEvalResponse = serde_json::from_str(&json).expect("deserialize");
        assert!(deserialized.forced);
        assert_eq!(deserialized.timestamp, 9999);
    }

    #[test]
    fn test_config_with_client_key() {
        let config = GrowthBookConfig::with_client_key("key-abc");
        assert_eq!(config.client_key.as_deref(), Some("key-abc"));
    }
}
