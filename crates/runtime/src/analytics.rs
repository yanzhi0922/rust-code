use anyhow::Result;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyticsEvent {
    pub event_type: String,
    pub timestamp: DateTime<Utc>,
    pub properties: HashMap<String, serde_json::Value>,
    pub user_id: Option<String>,
    pub session_id: Option<String>,
}

impl AnalyticsEvent {
    pub fn new(event_type: impl Into<String>) -> Self {
        Self {
            event_type: event_type.into(),
            timestamp: Utc::now(),
            properties: HashMap::new(),
            user_id: None,
            session_id: None,
        }
    }

    pub fn with_property(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.properties.insert(key.into(), value);
        self
    }

    pub fn with_user_id(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = Some(user_id.into());
        self
    }

    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureFlag {
    pub key: String,
    pub enabled: bool,
    pub value: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct GrowthBookConfig {
    pub api_key: Option<String>,
    pub api_host: String,
    pub refresh_interval_seconds: u64,
}

impl Default for GrowthBookConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            api_host: "https://cdn.growthbook.io".to_string(),
            refresh_interval_seconds: 300,
        }
    }
}

pub struct GrowthBookClient {
    config: GrowthBookConfig,
    features: Arc<RwLock<HashMap<String, FeatureFlag>>>,
}

impl GrowthBookClient {
    pub fn new(config: GrowthBookConfig) -> Self {
        Self {
            config,
            features: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn from_env() -> Self {
        Self::new(GrowthBookConfig {
            api_key: std::env::var("GROWTHBOOK_API_KEY").ok(),
            api_host: std::env::var("GROWTHBOOK_API_HOST")
                .unwrap_or_else(|_| "https://cdn.growthbook.io".to_string()),
            refresh_interval_seconds: 300,
        })
    }

    pub async fn is_feature_enabled(&self, key: &str) -> bool {
        let features = self.features.read().await;
        features.get(key).map(|f| f.enabled).unwrap_or(false)
    }

    pub async fn get_feature_value(&self, key: &str) -> Option<serde_json::Value> {
        let features = self.features.read().await;
        features.get(key).and_then(|f| f.value.clone())
    }

    pub async fn set_feature(&self, key: String, enabled: bool, value: Option<serde_json::Value>) {
        let mut features = self.features.write().await;
        features.insert(key.clone(), FeatureFlag { key, enabled, value });
    }

    pub async fn refresh_features(&self) -> Result<()> {
        if let Some(api_key) = &self.config.api_key {
            let url = format!("{}/api/features/{}", self.config.api_host, api_key);
            let response = reqwest::get(&url).await?;
            let features: HashMap<String, FeatureFlag> = response.json().await?;
            let mut current = self.features.write().await;
            *current = features;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct DatadogConfig {
    pub api_key: Option<String>,
    pub app_key: Option<String>,
    pub api_host: String,
    pub service_name: String,
    pub environment: String,
}

impl Default for DatadogConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            app_key: None,
            api_host: "https://api.datadoghq.com".to_string(),
            service_name: "claude-code".to_string(),
            environment: "development".to_string(),
        }
    }
}

pub struct DatadogClient {
    config: DatadogConfig,
    events_buffer: Arc<RwLock<Vec<AnalyticsEvent>>>,
}

impl DatadogClient {
    pub fn new(config: DatadogConfig) -> Self {
        Self {
            config,
            events_buffer: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub fn from_env() -> Self {
        Self::new(DatadogConfig {
            api_key: std::env::var("DD_API_KEY").ok(),
            app_key: std::env::var("DD_APP_KEY").ok(),
            api_host: std::env::var("DD_API_HOST")
                .unwrap_or_else(|_| "https://api.datadoghq.com".to_string()),
            service_name: std::env::var("DD_SERVICE")
                .unwrap_or_else(|_| "claude-code".to_string()),
            environment: std::env::var("DD_ENV")
                .unwrap_or_else(|_| "development".to_string()),
        })
    }

    pub async fn track_event(&self, event: AnalyticsEvent) -> Result<()> {
        let mut buffer = self.events_buffer.write().await;
        buffer.push(event);

        if buffer.len() >= 100 {
            self.flush().await?;
        }

        Ok(())
    }

    pub async fn flush(&self) -> Result<()> {
        let mut buffer = self.events_buffer.write().await;

        if buffer.is_empty() || self.config.api_key.is_none() {
            return Ok(());
        }

        let events = std::mem::take(&mut *buffer);
        let events_json = serde_json::to_string(&events)?;

        let client = Client::new();
        let url = format!("{}/api/v1/events", self.config.api_host);

        client
            .post(&url)
            .header("DD-API-KEY", self.config.api_key.as_ref().unwrap())
            .header("Content-Type", "application/json")
            .body(events_json)
            .send()
            .await?;

        Ok(())
    }
}

pub struct AnalyticsService {
    growthbook: GrowthBookClient,
    datadog: DatadogClient,
    user_id: Option<String>,
    session_id: Option<String>,
}

impl AnalyticsService {
    pub fn new() -> Self {
        Self {
            growthbook: GrowthBookClient::from_env(),
            datadog: DatadogClient::from_env(),
            user_id: None,
            session_id: None,
        }
    }

    pub fn with_user_id(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = Some(user_id.into());
        self
    }

    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub async fn is_feature_enabled(&self, key: &str) -> bool {
        self.growthbook.is_feature_enabled(key).await
    }

    pub async fn track_event(&self, event_type: &str, properties: HashMap<String, serde_json::Value>) -> Result<()> {
        let mut event = AnalyticsEvent::new(event_type);

        if let Some(user_id) = &self.user_id {
            event = event.with_user_id(user_id.clone());
        }

        if let Some(session_id) = &self.session_id {
            event = event.with_session_id(session_id.clone());
        }

        for (key, value) in properties {
            event = event.with_property(key, value);
        }

        self.datadog.track_event(event).await
    }

    pub async fn flush(&self) -> Result<()> {
        self.datadog.flush().await
    }

    pub async fn refresh_features(&self) -> Result<()> {
        self.growthbook.refresh_features().await
    }
}

impl Default for AnalyticsService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analytics_event_creation() {
        let event = AnalyticsEvent::new("test_event")
            .with_property("key", serde_json::json!("value"))
            .with_user_id("user123");

        assert_eq!(event.event_type, "test_event");
        assert_eq!(event.user_id, Some("user123".to_string()));
        assert!(event.properties.contains_key("key"));
    }

    #[tokio::test]
    async fn test_growthbook_client_creation() {
        let client = GrowthBookClient::from_env();
        assert!(!client.is_feature_enabled("nonexistent").await);
    }

    #[tokio::test]
    async fn test_growthbook_set_feature() {
        let client = GrowthBookClient::new(GrowthBookConfig::default());
        client.set_feature("test_flag".to_string(), true, Some(serde_json::json!("value"))).await;

        assert!(client.is_feature_enabled("test_flag").await);
        assert_eq!(
            client.get_feature_value("test_flag").await,
            Some(serde_json::json!("value"))
        );
    }

    #[test]
    fn test_datadog_client_creation() {
        let client = DatadogClient::from_env();
        assert!(client.config.api_key.is_none());
    }

    #[test]
    fn test_analytics_service_creation() {
        let service = AnalyticsService::new()
            .with_user_id("user123")
            .with_session_id("session456");

        assert!(service.user_id.is_some());
        assert!(service.session_id.is_some());
    }

    #[tokio::test]
    async fn test_analytics_service_track_event() {
        let service = AnalyticsService::new();
        let mut props = HashMap::new();
        props.insert("test_key".to_string(), serde_json::json!("test_value"));

        let result = service.track_event("test_event", props).await;
        assert!(result.is_ok());
    }
}
