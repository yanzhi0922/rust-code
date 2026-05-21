//! Provider failover with health tracking and retry logic.
//!
//! [`FailoverProviderClient`] wraps multiple provider configurations and
//! automatically switches to the next healthy provider on transient failures.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::time::Instant;

use anyhow::{Result, anyhow};
use claude_config::{FailoverConfig, ProviderConfig};
use claude_core::{ConversationEntry, ProviderResponse};

use crate::ProviderClient;

/// Which provider operation to perform inside the failover loop.
#[derive(Clone, Copy)]
enum FailoverOp {
    Complete,
    CompleteStreaming,
}

#[derive(Debug, Default, Clone)]
pub struct FailoverStats {
    pub attempts: HashMap<String, usize>,
    pub failures: HashMap<String, usize>,
    pub last_failover_at: Option<Instant>,
}

impl FailoverStats {
    pub(crate) fn record_attempt(&mut self, provider_name: &str) {
        *self.attempts.entry(provider_name.to_owned()).or_insert(0) += 1;
    }

    pub(crate) fn record_failure(&mut self, provider_name: &str) {
        *self.failures.entry(provider_name.to_owned()).or_insert(0) += 1;
    }

    pub(crate) fn record_failover(&mut self) {
        self.last_failover_at = Some(Instant::now());
    }
}

pub struct FailoverProviderClient {
    client: ProviderClient,
    config: FailoverConfig,
    active_index: Mutex<usize>,
    stats: Mutex<FailoverStats>,
}

impl FailoverProviderClient {
    /// # Errors
    /// Returns an error if no providers are configured or the HTTP client cannot be built.
    pub fn new(config: FailoverConfig) -> Result<Self> {
        if config.providers.is_empty() {
            return Err(anyhow!(
                "failover config must include at least one provider"
            ));
        }
        let client = ProviderClient::new()?;
        Ok(Self {
            client,
            config,
            active_index: Mutex::new(0),
            stats: Mutex::new(FailoverStats::default()),
        })
    }

    /// # Errors
    /// Returns an error if all failover providers fail.
    ///
    /// # Panics
    /// Panics if the internal `active_index` mutex is poisoned.
    pub async fn complete(&self, conversation: &[ConversationEntry]) -> Result<ProviderResponse> {
        self.try_with_failover(FailoverOp::Complete, conversation)
            .await
    }

    /// # Errors
    /// Returns an error if all failover providers fail.
    ///
    /// # Panics
    /// Panics if the internal `active_index` mutex is poisoned.
    pub async fn complete_streaming(
        &self,
        conversation: &[ConversationEntry],
    ) -> Result<ProviderResponse> {
        self.try_with_failover(FailoverOp::CompleteStreaming, conversation)
            .await
    }

    /// Generic failover loop: iterate over providers starting from the active
    /// index, dispatch `op` for each, and switch to the next provider on
    /// transient failures.
    ///
    /// # Panics
    /// Panics if the internal `active_index` mutex is poisoned.
    async fn try_with_failover(
        &self,
        op: FailoverOp,
        conversation: &[ConversationEntry],
    ) -> Result<ProviderResponse> {
        let max_attempts = self
            .config
            .max_failover_attempts
            .min(self.config.providers.len());
        let start_index = *self.active_index.lock();

        let mut last_error: Option<anyhow::Error> = None;
        for attempt in 0..max_attempts {
            let provider_index = (start_index + attempt) % self.config.providers.len();
            let provider = &self.config.providers[provider_index];

            self.record_attempt(&provider.name);

            let result = match op {
                FailoverOp::Complete => self.client.complete(provider, conversation).await,
                FailoverOp::CompleteStreaming => {
                    self.client.complete_streaming(provider, conversation).await
                }
            };

            match result {
                Ok(response) => {
                    self.mark_healthy(provider_index);
                    return Ok(response);
                }
                Err(error) => {
                    self.record_failure(&provider.name);
                    if self.should_failover(&error) && attempt + 1 < max_attempts {
                        self.record_failover_event();
                        last_error = Some(error);
                        continue;
                    }
                    return Err(error);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("all failover providers exhausted")))
    }

    /// # Panics
    /// Panics if the internal `active_index` mutex is poisoned.
    pub fn active_provider(&self) -> &ProviderConfig {
        let index = *self.active_index.lock();
        &self.config.providers[index]
    }

    /// # Panics
    /// Panics if the internal stats mutex is poisoned.
    pub fn stats(&self) -> FailoverStats {
        self.stats.lock().clone()
    }

    fn mark_healthy(&self, index: usize) {
        *self.active_index.lock() = index;
    }

    fn record_attempt(&self, provider_name: &str) {
        self.stats.lock().record_attempt(provider_name);
    }

    fn record_failure(&self, provider_name: &str) {
        self.stats.lock().record_failure(provider_name);
    }

    fn record_failover_event(&self) {
        self.stats.lock().record_failover();
    }

    fn should_failover(&self, error: &anyhow::Error) -> bool {
        self.is_failover_status(error) || self.is_timeout_error(error)
    }

    fn is_failover_status(&self, error: &anyhow::Error) -> bool {
        if let Some(status) = extract_status_from_error(error) {
            return self.config.failover_on_status.contains(&status);
        }
        false
    }

    fn is_timeout_error(&self, error: &anyhow::Error) -> bool {
        if !self.config.failover_on_timeout {
            return false;
        }
        for cause in error.chain() {
            if let Some(reqwest_err) = cause.downcast_ref::<reqwest::Error>()
                && reqwest_err.is_timeout()
            {
                return true;
            }
            let msg = cause.to_string().to_lowercase();
            if msg.contains("timeout") {
                return true;
            }
        }
        false
    }
}

fn extract_status_from_error(error: &anyhow::Error) -> Option<u16> {
    let msg = error.to_string();

    // Pattern 1: "provider request failed (STATUS): ..."
    let prefixes = ["provider request failed (", "request failed (", "HTTP "];
    for prefix in &prefixes {
        if let Some(start) = msg.find(prefix) {
            let rest = &msg[start + prefix.len()..];
            if let Some(end) = rest.find(|c: char| !c.is_ascii_digit()) {
                if end > 0 {
                    return rest[..end].parse::<u16>().ok();
                }
            } else if !rest.is_empty() {
                return rest.parse::<u16>().ok();
            }
        }
    }

    // Pattern 2: try to extract from reqwest::Error in the cause chain.
    for cause in error.chain() {
        if let Some(reqwest_err) = cause.downcast_ref::<reqwest::Error>()
            && let Some(status) = reqwest_err.status()
        {
            return Some(status.as_u16());
        }
    }

    None
}
