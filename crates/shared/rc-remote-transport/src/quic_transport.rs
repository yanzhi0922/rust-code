//! Strategy 5: QUIC transport via quinn with connection migration.
//!
//! Provides the best mobile stability — QUIC connection migration handles
//! WiFi↔cellular switches without dropping the connection.

use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::reconnect::ReconnectPolicy;
use crate::transport::{CommandAck, HealthStatus, RemoteTransport, TransportCommand};
use crate::{ConnectionState, TransportConfig, TransportEvent, TransportMetrics};

const MAX_QUIC_FRAME_BYTES: usize = 1024 * 1024;

/// QUIC transport using quinn.
pub struct QuicTransport {
    state: ConnectionState,
    config: Option<TransportConfig>,
    metrics: TransportMetrics,
    event_rx: Option<mpsc::Receiver<TransportEvent>>,
    #[allow(dead_code)]
    reconnect: ReconnectPolicy,
    connection: Option<quinn::Connection>,
    endpoint: Option<quinn::Endpoint>,
    cancel: tokio::sync::watch::Sender<bool>,
}

impl QuicTransport {
    pub fn new(reconnect: ReconnectPolicy) -> Self {
        let (cancel, _) = tokio::sync::watch::channel(false);
        Self {
            state: ConnectionState::Disconnected,
            config: None,
            metrics: TransportMetrics::default(),
            event_rx: None,
            reconnect,
            connection: None,
            endpoint: None,
            cancel,
        }
    }

    /// Take the event receiver created during `connect()`.
    /// Returns `None` if not connected or already taken.
    pub fn take_event_receiver(&mut self) -> Option<mpsc::Receiver<TransportEvent>> {
        self.event_rx.take()
    }
}

#[async_trait]
impl RemoteTransport for QuicTransport {
    async fn connect(&mut self, config: TransportConfig) -> anyhow::Result<()> {
        let (server_url, cert_fp) = match &config.strategy {
            crate::TransportStrategy::Quic {
                server_url,
                server_cert_fingerprint,
            } => (server_url.clone(), server_cert_fingerprint.clone()),
            _ => anyhow::bail!("QuicTransport requires Quic strategy"),
        };

        self.config = Some(config.clone());
        self.state = ConnectionState::Connecting;

        // Parse server address.
        let addr = parse_quic_addr(&server_url)?;
        if cert_fp.is_none() && !addr.ip().is_loopback() {
            anyhow::bail!(
                "QUIC requires a pinned server certificate fingerprint for non-loopback hosts"
            );
        }

        // Build TLS config for QUIC.
        let tls = crate::tls::build_client_tls_config(&crate::TlsConfig {
            accept_self_signed: cert_fp.is_some(),
            cert_fingerprints: cert_fp.map(|fp| vec![fp]).unwrap_or_default(),
            enforce_https: false, // QUIC has its own addressing
        })?;
        let mut tls = (*tls).clone();
        tls.alpn_protocols = vec![b"rc-quic/1".to_vec()];

        let client_config = quinn::ClientConfig::new(Arc::new(
            quinn::crypto::rustls::QuicClientConfig::try_from(Arc::new(tls))
                .map_err(|e| anyhow::anyhow!("QUIC TLS config error: {e}"))?,
        ));
        let bind_addr: std::net::SocketAddr =
            "0.0.0.0:0".parse().map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut endpoint = quinn::Endpoint::client(bind_addr)?;
        endpoint.set_default_client_config(client_config);

        // Connect with server name for TLS verification.
        let server_name = extract_server_name(&server_url);
        let conn = endpoint.connect(addr, &server_name)?.await.map_err(|e| {
            self.state = ConnectionState::Error(e.to_string());
            anyhow::anyhow!("QUIC connect failed: {e}")
        })?;

        // Authenticate the QUIC event stream before accepting server events.
        let auth_payload = serde_json::to_vec(&QuicAuthMessage {
            token: config.auth_token.clone(),
            session_id: config.session_id.clone(),
        })?;
        if auth_payload.len() > MAX_QUIC_FRAME_BYTES {
            anyhow::bail!("QUIC auth frame too large: {} bytes", auth_payload.len());
        }
        let mut auth_stream = conn
            .open_uni()
            .await
            .map_err(|e| anyhow::anyhow!("QUIC open auth stream: {e}"))?;
        use tokio::io::AsyncWriteExt;
        auth_stream
            .write_all(&(auth_payload.len() as u32).to_le_bytes())
            .await?;
        auth_stream.write_all(&auth_payload).await?;
        auth_stream.shutdown().await?;

        self.connection = Some(conn.clone());
        self.endpoint = Some(endpoint);

        // Open bidirectional streams: one for events (recv), one for commands (send).
        let (event_tx, event_rx) = mpsc::channel(256);
        self.event_rx = Some(event_rx);

        let cancel_rx = self.cancel.subscribe();
        tokio::spawn(quic_event_reader(conn, event_tx, cancel_rx));

        self.state = ConnectionState::Open {
            active_strategy: "quic".into(),
            latency_ms: 0,
        };
        Ok(())
    }

    async fn send_command(&self, command: TransportCommand) -> anyhow::Result<CommandAck> {
        let conn = self
            .connection
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("not connected"))?;

        let mut stream = conn
            .open_bi()
            .await
            .map_err(|e| anyhow::anyhow!("QUIC open stream: {e}"))?;

        let payload = serde_json::to_vec(&command)?;
        if payload.len() > MAX_QUIC_FRAME_BYTES {
            anyhow::bail!("QUIC command frame too large: {} bytes", payload.len());
        }
        let len = (payload.len() as u32).to_le_bytes();
        use tokio::io::AsyncWriteExt;
        stream.0.write_all(&len).await?;
        stream.0.write_all(&payload).await?;
        stream.0.shutdown().await?;

        // Read response.
        let mut len_buf = [0u8; 4];
        stream.1.read_exact(&mut len_buf).await?;
        let resp_len = u32::from_le_bytes(len_buf) as usize;
        if resp_len > MAX_QUIC_FRAME_BYTES {
            anyhow::bail!("QUIC response frame too large: {resp_len} bytes");
        }
        let mut resp_buf = vec![0u8; resp_len];
        stream.1.read_exact(&mut resp_buf).await?;

        let ack: CommandAck = serde_json::from_slice(&resp_buf)?;
        Ok(ack)
    }

    async fn health_probe(&self) -> HealthStatus {
        let has_conn = self.connection.is_some();
        HealthStatus {
            endpoints: vec![crate::EndpointHealth {
                url: "quic://connection".into(),
                reachable: has_conn,
                latency_ms: self.metrics.latency_ms.into(),
                auth_valid: has_conn,
                error: if has_conn {
                    None
                } else {
                    Some("no QUIC connection".into())
                },
            }],
            recommended_strategy: if has_conn { Some("quic".into()) } else { None },
        }
    }

    async fn disconnect(&mut self) -> anyhow::Result<()> {
        let _ = self.cancel.send(true);
        if let Some(conn) = self.connection.take() {
            conn.close(0u32.into(), b"disconnect");
        }
        if let Some(endpoint) = self.endpoint.take() {
            endpoint.close(0u32.into(), b"shutdown");
        }
        self.event_rx = None;
        self.state = ConnectionState::Closed;
        Ok(())
    }

    fn state(&self) -> ConnectionState {
        self.state.clone()
    }

    fn active_strategy(&self) -> &str {
        "quic"
    }

    fn metrics(&self) -> TransportMetrics {
        self.metrics.clone()
    }
}

#[derive(serde::Serialize)]
struct QuicAuthMessage {
    token: String,
    session_id: String,
}

/// Read events from a QUIC unidirectional stream.
async fn quic_event_reader(
    conn: quinn::Connection,
    tx: mpsc::Sender<TransportEvent>,
    mut cancel: tokio::sync::watch::Receiver<bool>,
) {
    // Accept incoming streams from the server (events).
    loop {
        tokio::select! {
            stream_result = conn.accept_uni() => {
                match stream_result {
                    Ok(mut stream) => {
                        let mut len_buf = [0u8; 4];
                        if let Err(e) = tokio::io::AsyncReadExt::read_exact(&mut stream, &mut len_buf).await {
                            tracing::debug!("QUIC stream read error: {e}");
                            break;
                        }
                        let len = u32::from_le_bytes(len_buf) as usize;
                        if len > MAX_QUIC_FRAME_BYTES {
                            tracing::debug!("QUIC event frame too large: {len} bytes");
                            break;
                        }
                        let mut buf = vec![0u8; len];
                        if let Err(e) = tokio::io::AsyncReadExt::read_exact(&mut stream, &mut buf).await {
                            tracing::debug!("QUIC payload read error: {e}");
                            break;
                        }
                        if let Ok(event) = serde_json::from_slice::<TransportEvent>(&buf)
                            && tx.send(event).await.is_err()
                        {
                            break;
                        }
                    }
                    Err(quinn::ConnectionError::ApplicationClosed(_)) => break,
                    Err(e) => {
                        tracing::debug!("QUIC accept error: {e}");
                        break;
                    }
                }
            }
            _ = cancel.changed() => {
                if *cancel.borrow() {
                    break;
                }
            }
        }
    }
}

fn parse_quic_addr(url: &str) -> anyhow::Result<std::net::SocketAddr> {
    // Accept formats: "quic://host:port", "host:port"
    let stripped = strip_quic_scheme(url);

    stripped
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid QUIC address: {url}"))
}

fn extract_server_name(url: &str) -> String {
    let stripped = strip_quic_scheme(url);
    stripped.split(':').next().unwrap_or("localhost").to_owned()
}

fn strip_quic_scheme(url: &str) -> &str {
    url.strip_prefix("quic://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url)
}
