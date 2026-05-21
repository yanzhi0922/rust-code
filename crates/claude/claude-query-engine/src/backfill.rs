//! Tool input backfill for observable input tracking.
//!
//! When tool calls are streamed incrementally, the full input may not be
//! available until the stream completes. This module tracks pending inputs
//! and backfills them when the complete input arrives.

use std::collections::HashMap;

use serde_json::Value;

/// Manages pending tool inputs that need to be backfilled.
#[derive(Debug, Clone, Default)]
pub struct InputBackfillManager {
    /// Partial inputs accumulated during streaming.
    pending_inputs: HashMap<String, Value>,
    /// Fully resolved inputs.
    resolved_inputs: HashMap<String, Value>,
}

impl InputBackfillManager {
    /// Create a new empty backfill manager.
    #[must_use]
    pub fn new() -> Self {
        Self {
            pending_inputs: HashMap::new(),
            resolved_inputs: HashMap::new(),
        }
    }

    /// Register a partial input for a tool call.
    pub fn register_partial(&mut self, tool_call_id: impl Into<String>, partial: Value) {
        self.pending_inputs.insert(tool_call_id.into(), partial);
    }

    /// Backfill a complete input for a tool call, resolving it from pending.
    pub fn backfill(&mut self, tool_call_id: &str, complete_input: Value) {
        self.pending_inputs.remove(tool_call_id);
        self.resolved_inputs
            .insert(tool_call_id.to_string(), complete_input);
    }

    /// Get the resolved input for a tool call.
    #[must_use]
    pub fn get_resolved(&self, tool_call_id: &str) -> Option<&Value> {
        self.resolved_inputs.get(tool_call_id)
    }

    /// Get the pending (partial) input for a tool call.
    #[must_use]
    pub fn get_pending(&self, tool_call_id: &str) -> Option<&Value> {
        self.pending_inputs.get(tool_call_id)
    }

    /// Returns true if a tool call has a resolved input.
    #[must_use]
    pub fn is_resolved(&self, tool_call_id: &str) -> bool {
        self.resolved_inputs.contains_key(tool_call_id)
    }

    /// Returns the number of pending (unresolved) inputs.
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.pending_inputs.len()
    }

    /// Returns the number of resolved inputs.
    #[must_use]
    pub fn resolved_count(&self) -> usize {
        self.resolved_inputs.len()
    }

    /// Clear all pending and resolved inputs.
    pub fn clear(&mut self) {
        self.pending_inputs.clear();
        self.resolved_inputs.clear();
    }

    /// Merge a partial delta into an existing pending input.
    /// If no pending input exists, creates one.
    pub fn merge_delta(&mut self, tool_call_id: impl Into<String>, delta: Value) {
        let id = tool_call_id.into();
        match self.pending_inputs.get_mut(&id) {
            Some(existing) => {
                merge_json_values(existing, &delta);
            }
            None => {
                self.pending_inputs.insert(id, delta);
            }
        }
    }

    /// Remove a tool call's data entirely.
    pub fn remove(&mut self, tool_call_id: &str) {
        self.pending_inputs.remove(tool_call_id);
        self.resolved_inputs.remove(tool_call_id);
    }
}

/// Recursively merge `delta` into `base`. String values are concatenated.
fn merge_json_values(base: &mut Value, delta: &Value) {
    match (base, delta) {
        (Value::Object(base_map), Value::Object(delta_map)) => {
            for (key, delta_val) in delta_map {
                match base_map.get_mut(key) {
                    Some(base_val) => merge_json_values(base_val, delta_val),
                    None => {
                        base_map.insert(key.clone(), delta_val.clone());
                    }
                }
            }
        }
        (Value::String(base_str), Value::String(delta_str)) => {
            base_str.push_str(delta_str);
        }
        (base, delta) => {
            *base = delta.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::InputBackfillManager;

    #[test]
    fn backfill_registers_and_resolves() {
        let mut mgr = InputBackfillManager::new();
        mgr.register_partial("tc-1", json!({"command": "ec"}));
        assert!(!mgr.is_resolved("tc-1"));
        assert_eq!(mgr.pending_count(), 1);

        mgr.backfill("tc-1", json!({"command": "echo hello"}));
        assert!(mgr.is_resolved("tc-1"));
        assert_eq!(mgr.resolved_count(), 1);
        assert_eq!(mgr.pending_count(), 0);
        assert_eq!(
            mgr.get_resolved("tc-1"),
            Some(&json!({"command": "echo hello"}))
        );
    }

    #[test]
    fn backfill_merge_delta_concatenates_strings() {
        let mut mgr = InputBackfillManager::new();
        mgr.merge_delta("tc-1", json!({"command": "echo"}));
        mgr.merge_delta("tc-1", json!({"command": " hello"}));
        let pending = mgr.get_pending("tc-1").expect("should exist");
        assert_eq!(pending["command"].as_str(), Some("echo hello"));
    }

    #[test]
    fn backfill_merge_delta_adds_new_keys() {
        let mut mgr = InputBackfillManager::new();
        mgr.merge_delta("tc-1", json!({"command": "ls"}));
        mgr.merge_delta("tc-1", json!({"cwd": "/tmp"}));
        let pending = mgr.get_pending("tc-1").expect("should exist");
        assert_eq!(pending["command"].as_str(), Some("ls"));
        assert_eq!(pending["cwd"].as_str(), Some("/tmp"));
    }

    #[test]
    fn backfill_remove_clears_both() {
        let mut mgr = InputBackfillManager::new();
        mgr.register_partial("tc-1", json!({}));
        mgr.backfill("tc-1", json!({"done": true}));
        mgr.remove("tc-1");
        assert!(mgr.get_resolved("tc-1").is_none());
        assert!(mgr.get_pending("tc-1").is_none());
    }

    #[test]
    fn backfill_clear_removes_all() {
        let mut mgr = InputBackfillManager::new();
        mgr.register_partial("tc-1", json!({}));
        mgr.register_partial("tc-2", json!({}));
        mgr.backfill("tc-1", json!({}));
        mgr.clear();
        assert_eq!(mgr.pending_count(), 0);
        assert_eq!(mgr.resolved_count(), 0);
    }

    #[test]
    fn backfill_nonexistent_returns_none() {
        let mgr = InputBackfillManager::new();
        assert!(mgr.get_pending("nonexistent").is_none());
        assert!(mgr.get_resolved("nonexistent").is_none());
        assert!(!mgr.is_resolved("nonexistent"));
    }
}
