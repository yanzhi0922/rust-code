//! Hook registry — in-memory index of hooks by event type.
//!
//! Provides registration, lookup, session hook merging, and configuration
//! snapshot capabilities. Mirrors the upstream `getHooksConfig()` pattern.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::hook_matcher::parse_hook_event;
use crate::hook_types::HookMatcherEntry;
use crate::hooks::HookEventKind;

/// In-memory hook registry, indexed by event type.
///
/// Hooks are registered from settings files, plugins, and session overrides.
/// The registry supports merging session-specific hooks and taking immutable
/// snapshots for safe concurrent access.
#[derive(Debug, Clone, Default)]
pub struct HookRegistry {
    /// Hooks indexed by event type.
    entries: HashMap<HookEventKind, Vec<HookMatcherEntry>>,
    /// Session-specific hooks that override/extend the base hooks.
    session_hooks: HashMap<HookEventKind, Vec<HookMatcherEntry>>,
}

/// An immutable snapshot of the hook configuration.
///
/// Safe to share across threads without locking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HooksConfigSnapshot {
    /// Hooks indexed by event name (string key for serialization).
    hooks: HashMap<String, Vec<HookMatcherEntry>>,
}

impl HookRegistry {
    /// Create a new empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register hooks for a specific event type.
    ///
    /// Hooks are appended to any existing hooks for the same event.
    pub fn register_hooks(&mut self, event: HookEventKind, matchers: Vec<HookMatcherEntry>) {
        let entry = self.entries.entry(event).or_default();
        entry.extend(matchers);
    }

    /// Register a single matcher for an event.
    pub fn register_matcher(&mut self, event: HookEventKind, matcher: HookMatcherEntry) {
        let entry = self.entries.entry(event).or_default();
        entry.push(matcher);
    }

    /// Get all matchers for a specific event (including session hooks).
    #[must_use]
    pub fn get_hooks_for_event(&self, event: HookEventKind) -> Vec<HookMatcherEntry> {
        let mut result = Vec::new();

        // Base hooks first
        if let Some(base) = self.entries.get(&event) {
            result.extend(base.iter().cloned());
        }

        // Then session hooks
        if let Some(session) = self.session_hooks.get(&event) {
            result.extend(session.iter().cloned());
        }

        result
    }

    /// Check if any hooks are registered for a specific event.
    #[must_use]
    pub fn has_hooks_for_event(&self, event: HookEventKind) -> bool {
        self.entries.get(&event).is_some_and(|h| !h.is_empty())
            || self
                .session_hooks
                .get(&event)
                .is_some_and(|h| !h.is_empty())
    }

    /// Check if any hooks are registered at all.
    #[must_use]
    pub fn has_any_hooks(&self) -> bool {
        !self.entries.is_empty() || !self.session_hooks.is_empty()
    }

    /// Merge session-specific hooks into the registry.
    ///
    /// Session hooks are stored separately and can be cleared independently.
    pub fn merge_session_hooks(&mut self, event: HookEventKind, matchers: Vec<HookMatcherEntry>) {
        let entry = self.session_hooks.entry(event).or_default();
        entry.extend(matchers);
    }

    /// Clear all session-specific hooks.
    pub fn clear_session_hooks(&mut self) {
        self.session_hooks.clear();
    }

    /// Clear session hooks for a specific event.
    pub fn clear_session_hooks_for_event(&mut self, event: HookEventKind) {
        self.session_hooks.remove(&event);
    }

    /// Remove hooks that are marked as `once` and have been fired.
    ///
    /// Returns the number of hooks removed.
    pub fn remove_once_hooks(&mut self, event: HookEventKind) -> usize {
        let mut removed = 0;

        if let Some(matchers) = self.entries.get_mut(&event) {
            for matcher in matchers.iter_mut() {
                let before = matcher.hooks.len();
                matcher.hooks.retain(|h| !h.is_once());
                removed += before - matcher.hooks.len();
            }
            // Clean up empty matchers
            matchers.retain(|m| !m.hooks.is_empty());
        }

        // Also clean up session hooks
        if let Some(matchers) = self.session_hooks.get_mut(&event) {
            for matcher in matchers.iter_mut() {
                let before = matcher.hooks.len();
                matcher.hooks.retain(|h| !h.is_once());
                removed += before - matcher.hooks.len();
            }
            matchers.retain(|m| !m.hooks.is_empty());
        }

        removed
    }

    /// Clear all hooks (both base and session).
    pub fn clear_all(&mut self) {
        self.entries.clear();
        self.session_hooks.clear();
    }

    /// Take an immutable snapshot of the current hook configuration.
    #[must_use]
    pub fn snapshot(&self) -> HooksConfigSnapshot {
        let mut hooks = HashMap::new();

        for (event, matchers) in &self.entries {
            let key = event.as_str().to_string();
            let mut combined = matchers.clone();

            // Merge session hooks
            if let Some(session) = self.session_hooks.get(event) {
                combined.extend(session.iter().cloned());
            }

            hooks.insert(key, combined);
        }

        // Also include events that only have session hooks
        for (event, matchers) in &self.session_hooks {
            let key = event.as_str().to_string();
            hooks.entry(key).or_insert_with(|| matchers.clone());
        }

        HooksConfigSnapshot { hooks }
    }

    /// Get the total number of hooks across all events.
    #[must_use]
    pub fn total_hook_count(&self) -> usize {
        let mut count = 0;
        for matchers in self.entries.values() {
            for matcher in matchers {
                count += matcher.hooks.len();
            }
        }
        for matchers in self.session_hooks.values() {
            for matcher in matchers {
                count += matcher.hooks.len();
            }
        }
        count
    }

    /// Get the list of events that have hooks registered.
    #[must_use]
    pub fn active_events(&self) -> Vec<HookEventKind> {
        let mut events: Vec<HookEventKind> = self.entries.keys().copied().collect();
        for event in self.session_hooks.keys() {
            if !events.contains(event) {
                events.push(*event);
            }
        }
        events
    }

    /// Register hooks from a settings map (event name → matchers).
    ///
    /// This is the primary loading path from configuration files.
    pub fn register_from_settings(&mut self, settings: &HashMap<String, Vec<HookMatcherEntry>>) {
        for (event_name, matchers) in settings {
            if let Some(event) = parse_hook_event(event_name) {
                self.register_hooks(event, matchers.clone());
            }
        }
    }
}

impl HooksConfigSnapshot {
    /// Get hooks for an event name.
    #[must_use]
    pub fn get(&self, event_name: &str) -> Option<&Vec<HookMatcherEntry>> {
        self.hooks.get(event_name)
    }

    /// Check if any hooks exist in this snapshot.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    /// Get the total number of hook entries.
    #[must_use]
    pub fn total_hooks(&self) -> usize {
        self.hooks
            .values()
            .map(|matchers| matchers.iter().map(|m| m.hooks.len()).sum::<usize>())
            .sum()
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook_types::{HookCommand, HookDefinition};

    fn make_command_hook(cmd: &str, once: bool) -> HookDefinition {
        HookDefinition::Command(HookCommand {
            command: cmd.to_string(),
            shell: None,
            timeout: None,
            if_condition: None,
            status_message: None,
            once,
            r#async: false,
            async_rewake: false,
        })
    }

    fn make_matcher(pattern: Option<&str>, cmds: &[&str]) -> HookMatcherEntry {
        HookMatcherEntry {
            matcher: pattern.map(String::from),
            hooks: cmds.iter().map(|c| make_command_hook(c, false)).collect(),
        }
    }

    fn make_once_matcher(pattern: Option<&str>, cmds: &[&str]) -> HookMatcherEntry {
        HookMatcherEntry {
            matcher: pattern.map(String::from),
            hooks: cmds.iter().map(|c| make_command_hook(c, true)).collect(),
        }
    }

    // ── Basic registry tests ─────────────────────────────────────────────

    #[test]
    fn new_registry_is_empty() {
        let registry = HookRegistry::new();
        assert!(!registry.has_any_hooks());
        assert_eq!(registry.total_hook_count(), 0);
    }

    #[test]
    fn register_hooks_for_event() {
        let mut registry = HookRegistry::new();
        registry.register_hooks(
            HookEventKind::PreToolUse,
            vec![make_matcher(Some("Bash"), &["lint.sh"])],
        );
        assert!(registry.has_hooks_for_event(HookEventKind::PreToolUse));
        assert!(!registry.has_hooks_for_event(HookEventKind::PostToolUse));
    }

    #[test]
    fn register_multiple_matchers() {
        let mut registry = HookRegistry::new();
        registry.register_hooks(
            HookEventKind::PreToolUse,
            vec![
                make_matcher(Some("Bash"), &["bash-hook.sh"]),
                make_matcher(Some("Write"), &["write-hook.sh"]),
            ],
        );
        let hooks = registry.get_hooks_for_event(HookEventKind::PreToolUse);
        assert_eq!(hooks.len(), 2);
    }

    #[test]
    fn register_single_matcher() {
        let mut registry = HookRegistry::new();
        registry.register_matcher(
            HookEventKind::SessionStart,
            make_matcher(None, &["init.sh"]),
        );
        assert!(registry.has_hooks_for_event(HookEventKind::SessionStart));
    }

    #[test]
    fn get_hooks_for_empty_event() {
        let registry = HookRegistry::new();
        let hooks = registry.get_hooks_for_event(HookEventKind::Stop);
        assert!(hooks.is_empty());
    }

    #[test]
    fn total_hook_count() {
        let mut registry = HookRegistry::new();
        registry.register_hooks(
            HookEventKind::PreToolUse,
            vec![make_matcher(Some("Bash"), &["a.sh", "b.sh"])],
        );
        registry.register_hooks(
            HookEventKind::PostToolUse,
            vec![make_matcher(None, &["c.sh"])],
        );
        assert_eq!(registry.total_hook_count(), 3);
    }

    #[test]
    fn active_events() {
        let mut registry = HookRegistry::new();
        registry.register_hooks(HookEventKind::PreToolUse, vec![make_matcher(None, &["a"])]);
        registry.register_hooks(
            HookEventKind::SessionStart,
            vec![make_matcher(None, &["b"])],
        );
        let events = registry.active_events();
        assert_eq!(events.len(), 2);
        assert!(events.contains(&HookEventKind::PreToolUse));
        assert!(events.contains(&HookEventKind::SessionStart));
    }

    // ── Session hooks tests ──────────────────────────────────────────────

    #[test]
    fn merge_session_hooks() {
        let mut registry = HookRegistry::new();
        registry.register_hooks(
            HookEventKind::PreToolUse,
            vec![make_matcher(Some("Bash"), &["base.sh"])],
        );
        registry.merge_session_hooks(
            HookEventKind::PreToolUse,
            vec![make_matcher(Some("Write"), &["session.sh"])],
        );

        let hooks = registry.get_hooks_for_event(HookEventKind::PreToolUse);
        assert_eq!(hooks.len(), 2);
    }

    #[test]
    fn clear_session_hooks() {
        let mut registry = HookRegistry::new();
        registry.register_hooks(
            HookEventKind::PreToolUse,
            vec![make_matcher(None, &["base.sh"])],
        );
        registry.merge_session_hooks(
            HookEventKind::PreToolUse,
            vec![make_matcher(None, &["session.sh"])],
        );
        registry.clear_session_hooks();

        let hooks = registry.get_hooks_for_event(HookEventKind::PreToolUse);
        assert_eq!(hooks.len(), 1);
    }

    #[test]
    fn clear_session_hooks_for_event() {
        let mut registry = HookRegistry::new();
        registry.merge_session_hooks(HookEventKind::PreToolUse, vec![make_matcher(None, &["a"])]);
        registry.merge_session_hooks(HookEventKind::PostToolUse, vec![make_matcher(None, &["b"])]);
        registry.clear_session_hooks_for_event(HookEventKind::PreToolUse);

        assert!(!registry.has_hooks_for_event(HookEventKind::PreToolUse));
        assert!(registry.has_hooks_for_event(HookEventKind::PostToolUse));
    }

    // ── Once hooks tests ─────────────────────────────────────────────────

    #[test]
    fn remove_once_hooks() {
        let mut registry = HookRegistry::new();
        registry.register_hooks(
            HookEventKind::PreToolUse,
            vec![make_once_matcher(Some("Bash"), &["once.sh"])],
        );
        assert_eq!(registry.total_hook_count(), 1);

        let removed = registry.remove_once_hooks(HookEventKind::PreToolUse);
        assert_eq!(removed, 1);
        assert_eq!(registry.total_hook_count(), 0);
    }

    #[test]
    fn remove_once_hooks_preserves_normal() {
        let mut registry = HookRegistry::new();
        registry.register_hooks(
            HookEventKind::PreToolUse,
            vec![HookMatcherEntry {
                matcher: Some("Bash".to_string()),
                hooks: vec![
                    make_command_hook("normal.sh", false),
                    make_command_hook("once.sh", true),
                ],
            }],
        );
        let removed = registry.remove_once_hooks(HookEventKind::PreToolUse);
        assert_eq!(removed, 1);
        assert_eq!(registry.total_hook_count(), 1);
    }

    // ── Clear all tests ──────────────────────────────────────────────────

    #[test]
    fn clear_all() {
        let mut registry = HookRegistry::new();
        registry.register_hooks(HookEventKind::PreToolUse, vec![make_matcher(None, &["a"])]);
        registry.merge_session_hooks(HookEventKind::PostToolUse, vec![make_matcher(None, &["b"])]);
        registry.clear_all();
        assert!(!registry.has_any_hooks());
    }

    // ── Snapshot tests ───────────────────────────────────────────────────

    #[test]
    fn snapshot_includes_all_hooks() {
        let mut registry = HookRegistry::new();
        registry.register_hooks(
            HookEventKind::PreToolUse,
            vec![make_matcher(Some("Bash"), &["base.sh"])],
        );
        registry.merge_session_hooks(
            HookEventKind::PreToolUse,
            vec![make_matcher(None, &["session.sh"])],
        );

        let snapshot = registry.snapshot();
        let pre_hooks = snapshot.get("PreToolUse").expect("should exist");
        assert_eq!(pre_hooks.len(), 2);
    }

    #[test]
    fn snapshot_empty_registry() {
        let registry = HookRegistry::new();
        let snapshot = registry.snapshot();
        assert!(snapshot.is_empty());
    }

    #[test]
    fn snapshot_total_hooks() {
        let mut registry = HookRegistry::new();
        registry.register_hooks(
            HookEventKind::PreToolUse,
            vec![make_matcher(None, &["a", "b"])],
        );
        let snapshot = registry.snapshot();
        assert_eq!(snapshot.total_hooks(), 2);
    }

    // ── register_from_settings tests ─────────────────────────────────────

    #[test]
    fn register_from_settings_map() {
        let mut registry = HookRegistry::new();
        let mut settings = HashMap::new();
        settings.insert(
            "PreToolUse".to_string(),
            vec![make_matcher(Some("Bash"), &["hook.sh"])],
        );
        settings.insert(
            "UnknownEvent".to_string(),
            vec![make_matcher(None, &["ignored.sh"])],
        );

        registry.register_from_settings(&settings);
        assert!(registry.has_hooks_for_event(HookEventKind::PreToolUse));
        assert!(!registry.has_hooks_for_event(HookEventKind::PostToolUse));
    }

    #[test]
    fn register_from_settings_all_events() {
        let mut registry = HookRegistry::new();
        let mut settings = HashMap::new();

        for &name in crate::hook_matcher::HOOK_EVENT_NAMES {
            settings.insert(name.to_string(), vec![make_matcher(None, &["generic.sh"])]);
        }

        registry.register_from_settings(&settings);
        assert!(registry.has_any_hooks());
        // All 26+ events should be registered
        assert!(registry.has_hooks_for_event(HookEventKind::PreToolUse));
        assert!(registry.has_hooks_for_event(HookEventKind::SessionStart));
        assert!(registry.has_hooks_for_event(HookEventKind::FileChanged));
    }

    // ── Session-only hooks in snapshot ───────────────────────────────────

    #[test]
    fn snapshot_includes_session_only_hooks() {
        let mut registry = HookRegistry::new();
        registry.merge_session_hooks(
            HookEventKind::Stop,
            vec![make_matcher(None, &["cleanup.sh"])],
        );
        let snapshot = registry.snapshot();
        assert!(snapshot.get("Stop").is_some());
    }
}
