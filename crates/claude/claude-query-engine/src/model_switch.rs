//! Runtime model switching and fallback support.
//!
//! Allows the query engine to switch models at runtime, with optional
//! fallback to a secondary model when the primary fails.

use serde::{Deserialize, Serialize};

/// Manages runtime model switching with fallback support.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSwitcher {
    /// Currently active model identifier.
    current_model: String,
    /// Optional fallback model for error recovery.
    fallback_model: Option<String>,
    /// Number of times the model has been switched.
    switch_count: usize,
    /// Whether we are currently using the fallback model.
    using_fallback: bool,
}

/// Reason for a model switch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SwitchReason {
    /// User explicitly requested a different model.
    UserRequest,
    /// The primary model failed and we're falling back.
    Fallback,
    /// The primary model is unavailable (rate limit, etc.).
    Unavailable,
    /// Automatic optimization based on task complexity.
    AutoOptimization,
}

/// Result of a model switch attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitchResult {
    /// Successfully switched to the new model.
    Switched {
        from: String,
        to: String,
        reason: SwitchReason,
    },
    /// No switch was needed (already on the target model).
    NoChange,
    /// Switch failed (no fallback configured).
    Failed { reason: String },
}

impl ModelSwitcher {
    /// Create a new model switcher with the given primary model.
    #[must_use]
    pub fn new(current_model: impl Into<String>) -> Self {
        Self {
            current_model: current_model.into(),
            fallback_model: None,
            switch_count: 0,
            using_fallback: false,
        }
    }

    /// Set a fallback model.
    #[must_use]
    pub fn with_fallback(mut self, fallback: impl Into<String>) -> Self {
        self.fallback_model = Some(fallback.into());
        self
    }

    /// Returns the current model identifier.
    #[must_use]
    pub fn current_model(&self) -> &str {
        &self.current_model
    }

    /// Returns the fallback model identifier, if configured.
    #[must_use]
    pub fn fallback_model(&self) -> Option<&str> {
        self.fallback_model.as_deref()
    }

    /// Returns the number of model switches performed.
    #[must_use]
    pub fn switch_count(&self) -> usize {
        self.switch_count
    }

    /// Returns true if currently using the fallback model.
    #[must_use]
    pub fn is_using_fallback(&self) -> bool {
        self.using_fallback
    }

    /// Switch to a specific model.
    pub fn switch_to(&mut self, target: impl Into<String>, reason: SwitchReason) -> SwitchResult {
        let target = target.into();
        if target == self.current_model {
            return SwitchResult::NoChange;
        }
        let from = std::mem::replace(&mut self.current_model, target);
        self.switch_count += 1;
        self.using_fallback = self.fallback_model.as_deref() == Some(self.current_model.as_str());
        SwitchResult::Switched {
            from,
            to: self.current_model.clone(),
            reason,
        }
    }

    /// Attempt to switch to the fallback model.
    pub fn switch_to_fallback(&mut self) -> SwitchResult {
        match &self.fallback_model {
            Some(fallback) => {
                if *fallback == self.current_model {
                    SwitchResult::NoChange
                } else {
                    self.switch_to(fallback.clone(), SwitchReason::Fallback)
                }
            }
            None => SwitchResult::Failed {
                reason: "no fallback model configured".to_string(),
            },
        }
    }

    /// Switch back to the primary model from fallback.
    pub fn restore_primary(&mut self, primary: impl Into<String>) -> SwitchResult {
        let primary = primary.into();
        if primary == self.current_model {
            self.using_fallback = false;
            return SwitchResult::NoChange;
        }
        let result = self.switch_to(primary, SwitchReason::AutoOptimization);
        self.using_fallback = false;
        result
    }

    /// Reset the switcher to a clean state.
    pub fn reset(&mut self) {
        self.switch_count = 0;
        self.using_fallback = false;
    }
}

#[cfg(test)]
mod tests {
    use super::{ModelSwitcher, SwitchReason, SwitchResult};

    #[test]
    fn model_switcher_starts_with_primary() {
        let switcher = ModelSwitcher::new("claude-3.5");
        assert_eq!(switcher.current_model(), "claude-3.5");
        assert!(!switcher.is_using_fallback());
        assert_eq!(switcher.switch_count(), 0);
    }

    #[test]
    fn model_switcher_switches_model() {
        let mut switcher = ModelSwitcher::new("claude-3.5");
        let result = switcher.switch_to("gpt-4", SwitchReason::UserRequest);
        assert_eq!(switcher.current_model(), "gpt-4");
        assert_eq!(switcher.switch_count(), 1);
        if let SwitchResult::Switched { from, to, reason } = result {
            assert_eq!(from, "claude-3.5");
            assert_eq!(to, "gpt-4");
            assert_eq!(reason, SwitchReason::UserRequest);
        } else {
            panic!("expected Switched result");
        }
    }

    #[test]
    fn model_switcher_no_change_when_same() {
        let mut switcher = ModelSwitcher::new("claude-3.5");
        let result = switcher.switch_to("claude-3.5", SwitchReason::AutoOptimization);
        assert_eq!(result, SwitchResult::NoChange);
        assert_eq!(switcher.switch_count(), 0);
    }

    #[test]
    fn model_switcher_fallback() {
        let mut switcher = ModelSwitcher::new("claude-3.5").with_fallback("claude-3-haiku");
        let result = switcher.switch_to_fallback();
        assert!(matches!(result, SwitchResult::Switched { .. }));
        assert!(switcher.is_using_fallback());
        assert_eq!(switcher.current_model(), "claude-3-haiku");
    }

    #[test]
    fn model_switcher_fallback_without_config() {
        let mut switcher = ModelSwitcher::new("claude-3.5");
        let result = switcher.switch_to_fallback();
        assert!(matches!(result, SwitchResult::Failed { .. }));
    }

    #[test]
    fn model_switcher_restore_primary() {
        let mut switcher = ModelSwitcher::new("claude-3.5").with_fallback("haiku");
        switcher.switch_to_fallback();
        assert!(switcher.is_using_fallback());
        let _ = switcher.restore_primary("claude-3.5");
        assert!(!switcher.is_using_fallback());
        assert_eq!(switcher.current_model(), "claude-3.5");
    }

    #[test]
    fn model_switcher_reset() {
        let mut switcher = ModelSwitcher::new("claude-3.5");
        switcher.switch_to("gpt-4", SwitchReason::UserRequest);
        switcher.reset();
        assert_eq!(switcher.switch_count(), 0);
        assert!(!switcher.is_using_fallback());
    }
}
