//! Plugin telemetry — track plugin usage events.
//!
//! Records plugin usage events for analytics and provides aggregated
//! usage statistics.

use std::collections::HashMap;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A plugin usage event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginUsageEvent {
    /// Plugin ID (`"name@marketplace"`).
    pub plugin_id: String,
    /// Event type (e.g., "invoke", "load", "install").
    pub event_type: String,
    /// Timestamp (seconds since epoch).
    pub timestamp_secs: u64,
    /// Additional metadata.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// Plugin usage statistics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginUsageStats {
    /// Plugin ID.
    pub plugin_id: String,
    /// Total number of events.
    pub total_events: usize,
    /// Number of events by type.
    pub events_by_type: HashMap<String, usize>,
    /// First event timestamp.
    pub first_event_secs: Option<u64>,
    /// Last event timestamp.
    pub last_event_secs: Option<u64>,
}

/// Plugin telemetry tracker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginTelemetry {
    /// Recorded events.
    events: Vec<PluginUsageEvent>,
    /// Maximum number of events to retain.
    max_events: usize,
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

impl PluginTelemetry {
    /// Create a new telemetry tracker with a maximum event count.
    #[must_use]
    pub fn new(max_events: usize) -> Self {
        Self {
            events: Vec::new(),
            max_events,
        }
    }

    /// Record a plugin usage event.
    pub fn record_plugin_usage(
        &mut self,
        plugin_id: &str,
        event_type: &str,
        metadata: HashMap<String, String>,
    ) {
        let timestamp_secs = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        self.events.push(PluginUsageEvent {
            plugin_id: plugin_id.to_owned(),
            event_type: event_type.to_owned(),
            timestamp_secs,
            metadata,
        });

        // Trim old events if over limit
        if self.events.len() > self.max_events {
            let excess = self.events.len() - self.max_events;
            self.events.drain(..excess);
        }
    }

    /// Get usage statistics for a specific plugin.
    pub fn get_plugin_usage_stats(&self, plugin_id: &str) -> PluginUsageStats {
        let plugin_events: Vec<&PluginUsageEvent> = self
            .events
            .iter()
            .filter(|e| e.plugin_id == plugin_id)
            .collect();

        let mut events_by_type: HashMap<String, usize> = HashMap::new();
        for event in &plugin_events {
            *events_by_type.entry(event.event_type.clone()).or_insert(0) += 1;
        }

        let first_event_secs = plugin_events.first().map(|e| e.timestamp_secs);
        let last_event_secs = plugin_events.last().map(|e| e.timestamp_secs);

        PluginUsageStats {
            plugin_id: plugin_id.to_owned(),
            total_events: plugin_events.len(),
            events_by_type,
            first_event_secs,
            last_event_secs,
        }
    }

    /// Get usage statistics for all plugins.
    pub fn get_all_usage_stats(&self) -> HashMap<String, PluginUsageStats> {
        let plugin_ids: HashSet<String> = self.events.iter().map(|e| e.plugin_id.clone()).collect();

        plugin_ids
            .into_iter()
            .map(|id| (id.clone(), self.get_plugin_usage_stats(&id)))
            .collect()
    }

    /// Get the total number of recorded events.
    pub fn event_count(&self) -> usize {
        self.events.len()
    }

    /// Clear all recorded events.
    pub fn clear(&mut self) {
        self.events.clear();
    }
}

use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_telemetry_is_empty() {
        let telemetry = PluginTelemetry::new(100);
        assert_eq!(telemetry.event_count(), 0);
    }

    #[test]
    fn record_and_get_stats() {
        let mut telemetry = PluginTelemetry::new(100);
        telemetry.record_plugin_usage("test-plugin@mkt", "invoke", HashMap::new());
        telemetry.record_plugin_usage("test-plugin@mkt", "invoke", HashMap::new());
        telemetry.record_plugin_usage("test-plugin@mkt", "load", HashMap::new());

        assert_eq!(telemetry.event_count(), 3);

        let stats = telemetry.get_plugin_usage_stats("test-plugin@mkt");
        assert_eq!(stats.total_events, 3);
        assert_eq!(stats.events_by_type.get("invoke"), Some(&2));
        assert_eq!(stats.events_by_type.get("load"), Some(&1));
        assert!(stats.first_event_secs.is_some());
        assert!(stats.last_event_secs.is_some());
    }

    #[test]
    fn get_stats_for_unknown_plugin() {
        let telemetry = PluginTelemetry::new(100);
        let stats = telemetry.get_plugin_usage_stats("unknown@mkt");
        assert_eq!(stats.total_events, 0);
        assert!(stats.events_by_type.is_empty());
        assert!(stats.first_event_secs.is_none());
    }

    #[test]
    fn max_events_trims_old() {
        let mut telemetry = PluginTelemetry::new(3);
        for i in 0..5 {
            telemetry.record_plugin_usage(&format!("plugin-{i}@mkt"), "invoke", HashMap::new());
        }
        assert_eq!(telemetry.event_count(), 3);
    }

    #[test]
    fn get_all_stats() {
        let mut telemetry = PluginTelemetry::new(100);
        telemetry.record_plugin_usage("a@mkt", "invoke", HashMap::new());
        telemetry.record_plugin_usage("b@mkt", "invoke", HashMap::new());

        let all = telemetry.get_all_usage_stats();
        assert_eq!(all.len(), 2);
        assert!(all.contains_key("a@mkt"));
        assert!(all.contains_key("b@mkt"));
    }

    #[test]
    fn clear_removes_all() {
        let mut telemetry = PluginTelemetry::new(100);
        telemetry.record_plugin_usage("a@mkt", "invoke", HashMap::new());
        telemetry.clear();
        assert_eq!(telemetry.event_count(), 0);
    }

    #[test]
    fn record_with_metadata() {
        let mut telemetry = PluginTelemetry::new(100);
        let mut meta = HashMap::new();
        meta.insert("action".to_owned(), "echo".to_owned());
        telemetry.record_plugin_usage("a@mkt", "invoke", meta);

        let stats = telemetry.get_plugin_usage_stats("a@mkt");
        assert_eq!(stats.total_events, 1);
    }
}
