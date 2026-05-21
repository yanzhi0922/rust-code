//! Strategy 4: Hybrid — direct preferred, server relay fallback with auto-switching.

use crate::reconnect::ReconnectPolicy;
use crate::transport::{CommandAck, HealthStatus, RemoteTransport, TransportCommand};
use crate::{ConnectionState, TransportConfig, TransportMetrics};
use async_trait::async_trait;

/// Active sub-strategy in hybrid mode.
#[derive(Debug, Clone, PartialEq)]
enum HybridMode {
    /// Trying to connect or connected directly to runner.
    Direct,
    /// Fallback: connected via control plane relay.
    Relay,
}

/// Hybrid transport with automatic path switching.
pub struct HybridTransport {
    mode: HybridMode,
    direct: super::direct_ws::DirectWsTransport,
    relay: super::relay_ws::RelayWsTransport,
    runner_url: Option<String>,
    cp_url: Option<String>,
    metrics: TransportMetrics,
    #[allow(dead_code)]
    reconnect: ReconnectPolicy,
    #[allow(dead_code)]
    probe_interval: std::time::Duration,
}

impl HybridTransport {
    pub fn new(reconnect: ReconnectPolicy) -> Self {
        Self {
            mode: HybridMode::Direct,
            direct: super::direct_ws::DirectWsTransport::new(reconnect.clone()),
            relay: super::relay_ws::RelayWsTransport::new(reconnect.clone()),
            runner_url: None,
            cp_url: None,
            metrics: TransportMetrics::default(),
            reconnect,
            probe_interval: std::time::Duration::from_secs(30),
        }
    }
}

#[async_trait]
impl RemoteTransport for HybridTransport {
    async fn connect(&mut self, config: TransportConfig) -> anyhow::Result<()> {
        let (runner_url, cp_url) = match &config.strategy {
            crate::TransportStrategy::Hybrid {
                runner_url,
                control_plane_url,
            } => (runner_url.clone(), control_plane_url.clone()),
            _ => anyhow::bail!("HybridTransport requires Hybrid strategy"),
        };

        self.runner_url = Some(runner_url.clone());
        self.cp_url = Some(cp_url.clone());

        let runner_health_url = format!("{runner_url}/healthz");
        let cp_health_url = format!("{cp_url}/healthz");
        let auth_token = config.auth_token.clone();

        // Probe both endpoints concurrently.
        let (runner_health, _cp_health) = tokio::join!(
            crate::health::probe_endpoint(
                &runner_health_url,
                Some(&auth_token),
                std::time::Duration::from_secs(2),
            ),
            crate::health::probe_endpoint(
                &cp_health_url,
                Some(&auth_token),
                std::time::Duration::from_secs(5),
            ),
        );

        // Try direct first if reachable and auth-valid.
        if runner_health.reachable && runner_health.auth_valid {
            let direct_config = TransportConfig {
                strategy: crate::TransportStrategy::DirectWebSocket { runner_url },
                ..config.clone()
            };
            match self.direct.connect(direct_config).await {
                Ok(()) => {
                    self.mode = HybridMode::Direct;
                    return Ok(());
                }
                Err(e) => {
                    tracing::warn!("direct connect failed, falling back to relay: {e}");
                }
            }
        }

        // Fall back to relay.
        let relay_config = TransportConfig {
            strategy: crate::TransportStrategy::ServerRelay {
                control_plane_url: cp_url,
            },
            ..config
        };
        self.relay.connect(relay_config).await?;
        self.mode = HybridMode::Relay;
        Ok(())
    }

    async fn send_command(&self, command: TransportCommand) -> anyhow::Result<CommandAck> {
        match self.mode {
            HybridMode::Direct => {
                match self.direct.send_command(command.clone()).await {
                    Ok(ack) if ack.accepted => Ok(ack),
                    _ => {
                        // Direct failed — try relay if available.
                        tracing::warn!("direct send failed, trying relay");
                        self.relay.send_command(command).await
                    }
                }
            }
            HybridMode::Relay => self.relay.send_command(command).await,
        }
    }

    async fn health_probe(&self) -> HealthStatus {
        let config = self.direct.config.as_ref().or(self.relay.config.as_ref());
        if let Some(config) = config {
            let (runner_url, cp_url) = match &config.strategy {
                crate::TransportStrategy::Hybrid {
                    runner_url,
                    control_plane_url,
                } => (runner_url.clone(), control_plane_url.clone()),
                _ => {
                    return HealthStatus {
                        endpoints: vec![],
                        recommended_strategy: None,
                    };
                }
            };

            let runner_health_url = format!("{runner_url}/healthz");
            let cp_health_url = format!("{cp_url}/healthz");
            let token = config.auth_token.clone();

            let endpoints = crate::health::probe_endpoints(
                &[
                    (runner_health_url.as_str(), Some(token.as_str())),
                    (cp_health_url.as_str(), Some(token.as_str())),
                ],
                std::time::Duration::from_secs(3),
            )
            .await;

            let recommended = if endpoints
                .first()
                .is_some_and(|e| e.reachable && e.auth_valid)
            {
                "direct_websocket"
            } else {
                "server_relay"
            };

            HealthStatus {
                endpoints,
                recommended_strategy: Some(recommended.into()),
            }
        } else {
            HealthStatus {
                endpoints: vec![],
                recommended_strategy: None,
            }
        }
    }

    async fn disconnect(&mut self) -> anyhow::Result<()> {
        self.direct.disconnect().await.ok();
        self.relay.disconnect().await.ok();
        Ok(())
    }

    fn state(&self) -> ConnectionState {
        match self.mode {
            HybridMode::Direct => self.direct.state(),
            HybridMode::Relay => self.relay.state(),
        }
    }

    fn active_strategy(&self) -> &str {
        match self.mode {
            HybridMode::Direct => "hybrid/direct",
            HybridMode::Relay => "hybrid/relay",
        }
    }

    fn metrics(&self) -> TransportMetrics {
        self.metrics.clone()
    }
}
