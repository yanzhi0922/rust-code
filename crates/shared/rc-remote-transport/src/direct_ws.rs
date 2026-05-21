//! Strategy 1: Direct WebSocket connection to the runner.

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_tungstenite::Connector;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{connect_async_tls_with_config, tungstenite};

use crate::reconnect::ReconnectPolicy;
use crate::transport::{CommandAck, HealthStatus, RemoteTransport, TransportCommand};
use crate::{ConnectionState, TransportConfig, TransportEvent, TransportMetrics};

/// Direct WebSocket transport to a runner machine.
pub struct DirectWsTransport {
    state: ConnectionState,
    pub(crate) config: Option<TransportConfig>,
    metrics: TransportMetrics,
    pub(crate) event_rx: Option<mpsc::Receiver<TransportEvent>>,
    client: reqwest::Client,
    #[allow(dead_code)]
    reconnect: ReconnectPolicy,
}

impl DirectWsTransport {
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
impl RemoteTransport for DirectWsTransport {
    async fn connect(&mut self, config: TransportConfig) -> anyhow::Result<()> {
        let runner_url = match &config.strategy {
            crate::TransportStrategy::DirectWebSocket { runner_url } => runner_url.clone(),
            _ => anyhow::bail!("DirectWsTransport requires DirectWebSocket strategy"),
        };

        self.config = Some(config.clone());
        self.state = ConnectionState::Connecting;

        let ws_url = build_runner_ws_url(&runner_url, &config.session_id, config.after_sequence);
        let request = build_authenticated_ws_request(&ws_url, &config.auth_token)?;

        // Build a real TLS connector from the config so self-signed certs
        // and fingerprint pinning actually work for direct connections.
        let tls_config = crate::tls::build_client_tls_config(&config.tls)?;
        let connector = Some(Connector::Rustls(tls_config));

        let (stream, _response) = connect_async_tls_with_config(request, None, false, connector)
            .await
            .map_err(|e| {
                self.state = ConnectionState::Error(e.to_string());
                anyhow::anyhow!("WebSocket connect failed: {e}")
            })?;

        // Spawn a task to read events from the WebSocket.
        let (tx, rx) = mpsc::channel(256);
        self.event_rx = Some(rx);
        tokio::spawn(read_ws_events(stream, tx));

        self.state = ConnectionState::Open {
            active_strategy: "direct_websocket".into(),
            latency_ms: 0,
        };
        Ok(())
    }

    async fn send_command(&self, command: TransportCommand) -> anyhow::Result<CommandAck> {
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("not connected"))?;

        let runner_url = match &config.strategy {
            crate::TransportStrategy::DirectWebSocket { runner_url } => runner_url,
            _ => anyhow::bail!("internal error: strategy mismatch, expected DirectWebSocket"),
        };

        let (path, body) = command_to_request(&command, &config.session_id);
        let url = format!("{runner_url}{path}");

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
            let runner_url = match &config.strategy {
                crate::TransportStrategy::DirectWebSocket { runner_url } => runner_url,
                _ => {
                    return HealthStatus {
                        endpoints: vec![],
                        recommended_strategy: None,
                    };
                }
            };
            let health_url = format!("{runner_url}/healthz");
            let health = crate::health::probe_endpoint(
                &health_url,
                Some(&config.auth_token),
                std::time::Duration::from_secs(2),
            )
            .await;
            HealthStatus {
                endpoints: vec![health],
                recommended_strategy: Some("direct_websocket".into()),
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
        "direct_websocket"
    }

    fn metrics(&self) -> TransportMetrics {
        self.metrics.clone()
    }
}

fn build_runner_ws_url(runner_url: &str, session_id: &str, after: u64) -> String {
    let ws_base = runner_url
        .replace("https://", "wss://")
        .replace("http://", "ws://");
    format!("{ws_base}/v1/sessions/{session_id}/events/stream?after={after}")
}

pub(crate) fn build_authenticated_ws_request(
    ws_url: &str,
    token: &str,
) -> anyhow::Result<tungstenite::handshake::client::Request> {
    let mut request = ws_url.into_client_request()?;
    let auth_value = format!("Bearer {token}").parse()?;
    request
        .headers_mut()
        .insert(tungstenite::http::header::AUTHORIZATION, auth_value);
    Ok(request)
}

pub(crate) async fn read_ws_events(
    stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    tx: mpsc::Sender<TransportEvent>,
) {
    use futures::StreamExt;
    tokio::pin!(stream);
    while let Some(msg) = stream.next().await {
        match msg {
            Ok(tungstenite::Message::Text(text)) => {
                match serde_json::from_str::<TransportEvent>(&text) {
                    Ok(event) => {
                        if tx.send(event).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => tracing::debug!("WS event parse error: {e}"),
                }
            }
            Ok(tungstenite::Message::Close(_)) => break,
            Err(e) => {
                tracing::debug!("WebSocket read error: {e}");
                break;
            }
            _ => continue,
        }
    }
}

pub(crate) fn command_to_request(
    command: &TransportCommand,
    session_id: &str,
) -> (String, serde_json::Value) {
    match command {
        TransportCommand::SendPrompt { content } => (
            format!("/v1/sessions/{session_id}/commands"),
            serde_json::json!({ "kind": "send_prompt", "content": content }),
        ),
        TransportCommand::Interrupt => (
            format!("/v1/sessions/{session_id}/commands"),
            serde_json::json!({ "kind": "interrupt" }),
        ),
        TransportCommand::RespondToApproval {
            approval_id,
            decision,
            note,
        } => (
            format!("/v1/approvals/{approval_id}/decision"),
            serde_json::json!({
                "decision": decision,
                "responder": "rc-remote-transport",
                "note": note,
            }),
        ),
    }
}
