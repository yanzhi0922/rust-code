use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyticsEvent {
    pub name: String,
    pub properties: HashMap<String, serde_json::Value>,
    pub timestamp: DateTime<Utc>,
}

impl AnalyticsEvent {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            properties: HashMap::new(),
            timestamp: Utc::now(),
        }
    }

    pub fn property(mut self, key: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        self.properties.insert(key.into(), value.into());
        self
    }
}

pub trait TelemetrySink: Send + Sync {
    fn record_event(&self, event: AnalyticsEvent);
    fn flush(&self);
}

#[derive(Debug, Clone, Default)]
pub struct MemoryTelemetrySink {
    events: std::sync::Arc<std::sync::Mutex<Vec<AnalyticsEvent>>>,
}

impl MemoryTelemetrySink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn events(&self) -> Vec<AnalyticsEvent> {
        self.events.lock().unwrap().clone()
    }

    pub fn clear(&self) {
        self.events.lock().unwrap().clear();
    }
}

impl TelemetrySink for MemoryTelemetrySink {
    fn record_event(&self, event: AnalyticsEvent) {
        self.events.lock().unwrap().push(event);
    }

    fn flush(&self) {}
}

pub struct NoopTelemetrySink;

impl TelemetrySink for NoopTelemetrySink {
    fn record_event(&self, _event: AnalyticsEvent) {}
    fn flush(&self) {}
}

pub struct SessionTracer {
    sink: Box<dyn TelemetrySink>,
}

impl SessionTracer {
    pub fn new(sink: Box<dyn TelemetrySink>) -> Self {
        Self { sink }
    }

    pub fn record(&self, event: AnalyticsEvent) {
        self.sink.record_event(event);
    }

    pub fn flush(&self) {
        self.sink.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_sink_records_events() {
        let sink = MemoryTelemetrySink::new();
        assert!(sink.events().is_empty());

        let event1 = AnalyticsEvent::new("test_event").property("key1", "value1");
        sink.record_event(event1);
        assert_eq!(sink.events().len(), 1);

        let event2 = AnalyticsEvent::new("another_event").property("count", 42);
        sink.record_event(event2);
        assert_eq!(sink.events().len(), 2);

        let events = sink.events();
        assert_eq!(events[0].name, "test_event");
        assert_eq!(events[1].name, "another_event");

        sink.clear();
        assert!(sink.events().is_empty());
    }

    #[test]
    fn test_session_tracer() {
        let sink = MemoryTelemetrySink::new();
        let tracer = SessionTracer::new(Box::new(sink.clone()));

        let event = AnalyticsEvent::new("query_start").property("model", "claude-3");
        tracer.record(event);
        tracer.flush();

        let events = sink.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].name, "query_start");
    }

    #[test]
    fn test_noop_sink() {
        let sink = NoopTelemetrySink;
        let event = AnalyticsEvent::new("should_be_ignored");
        sink.record_event(event);
        sink.flush();
    }
}
