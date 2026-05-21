//! Strategy 2: Server-relayed WebSocket via control plane.

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::reconnect::ReconnectPolicy;
use crate::transport::{CommandAck, HealthStatus, RemoteTransport, TransportCommand};
use crate::{ConnectionState, TransportConfig, TransportEvent, TransportMetrics};

/// Server-relayed transport through the control plane.
pub struct RelayWsTransport {
    state: ConnectionState,
    pub(crate) config: Option<TransportConfig>,
    metrics: TransportMetrics,
    pub(crate) event_rx: Option<mpsc::Receiver<TransportEvent>>,
    client: reqwest::Client,
    #[allow(dead_code)]
    reconnect: ReconnectPolicy,
}

impl RelayWsTransport {
    pub fn new(reconnect: ReconnectPolicy) -> Self {
        Self {
            state: ConnectionState::Disconnected,
            config: None,
            metrics: TransportMetrics::default(),
            event_rx: None,
            client: reqwest::Client::new(),
            reconnect,
        }
    }
}

#[async_trait]
impl RemoteTransport for RelayWsTransport {
    async fn connect(&mut self, config: TransportConfig) -> anyhow::Result<()> {
        let cp_url = match &config.strategy {
            crate::TransportStrategy::ServerRelay { control_plane_url } => {
                control_plane_url.clone()
            }
            _ => anyhow::bail!("RelayWsTransport requires ServerRelay strategy"),
        };

        self.config = Some(config.clone());
        self.state = ConnectionState::Connecting;

        let ws_url = build_cp_ws_url(&cp_url, &config.session_id, config.after_sequence);
        let request =
            super::direct_ws::build_authenticated_ws_request(&ws_url, &config.auth_token)?;

        let (stream, _response) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|e| {
                self.state = ConnectionState::Error(e.to_string());
                anyhow::anyhow!("WebSocket connect to control plane failed: {e}")
            })?;

        let (tx, rx) = mpsc::channel(256);
        self.event_rx = Some(rx);
        tokio::spawn(super::direct_ws::read_ws_events(stream, tx));

        self.state = ConnectionState::Open {
            active_strategy: "server_relay".into(),
            latency_ms: 0,
        };
        Ok(())
    }

    async fn send_command(&self, command: TransportCommand) -> anyhow::Result<CommandAck> {
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("not connected"))?;
        let cp_url = match &config.strategy {
            crate::TransportStrategy::ServerRelay { control_plane_url } => control_plane_url,
            _ => anyhow::bail!("internal error: strategy mismatch, expected ServerRelay"),
        };

        let (path, body) = super::direct_ws::command_to_request(&command, &config.session_id);
        let url = format!("{cp_url}{path}");

        let response = self
            .client
            .post(&url)
            .bearer_auth(&config.auth_token)
            .json(&body)
            .send()
            .await?;

        if response.status().is_success() {
            Ok(CommandAck {
                accepted: true,
                message: "ok".into(),
            })
        } else {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            Ok(CommandAck {
                accepted: false,
                message: format!("HTTP {status}: {text}"),
            })
        }
    }

    async fn health_probe(&self) -> HealthStatus {
        let config = self.config.as_ref();
        if let Some(config) = config {
            let cp_url = match &config.strategy {
                crate::TransportStrategy::ServerRelay { control_plane_url } => control_plane_url,
                _ => {
                    return HealthStatus {
                        endpoints: vec![],
                        recommended_strategy: None,
                    };
                }
            };
            let health_url = format!("{cp_url}/healthz");
            let health = crate::health::probe_endpoint(
                &health_url,
                Some(&config.auth_token),
                std::time::Duration::from_secs(5),
            )
            .await;
            HealthStatus {
                endpoints: vec![health],
                recommended_strategy: Some("server_relay".into()),
            }
        } else {
            HealthStatus {
                endpoints: vec![],
                recommended_strategy: None,
            }
        }
    }

    async fn disconnect(&mut self) -> anyhow::Result<()> {
        self.event_rx = None;
        self.state = ConnectionState::Closed;
        Ok(())
    }

    fn state(&self) -> ConnectionState {
        self.state.clone()
    }

    fn active_strategy(&self) -> &str {
        "server_relay"
    }

    fn metrics(&self) -> TransportMetrics {
        self.metrics.clone()
    }
}

fn build_cp_ws_url(cp_url: &str, session_id: &str, after: u64) -> String {
    let ws_base = cp_url
        .replace("https://", "wss://")
        .replace("http://", "ws://");
    format!("{ws_base}/v1/sessions/{session_id}/events/stream?after={after}")
}
