use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex, Notify, RwLock};
use tokio::time::{interval, timeout};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
    MaybeTlsStream, WebSocketStream,
};
use tracing::{debug, error, info, warn};

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

const RECONNECT_DELAY_MS: u64 = 2000;
const MAX_RECONNECT_ATTEMPTS: u32 = 5;
const PING_INTERVAL_MS: u64 = 30_000;
const MAX_SESSION_NOT_FOUND_RETRIES: u32 = 3;

const PERMANENT_CLOSE_CODES: [u16; 1] = [4003];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct RemoteContext {
    pub is_remote: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteConfig {
    pub session_id: String,
    pub base_url: String,
    pub org_uuid: String,
    pub access_token: String,
    #[serde(default)]
    pub has_initial_prompt: bool,
    #[serde(default)]
    pub viewer_only: bool,
}

impl RemoteConfig {
    pub fn ws_subscribe_url(&self) -> String {
        let ws_base = self
            .base_url
            .replace("https://", "wss://")
            .replace("http://", "ws://");
        format!(
            "{}/v1/sessions/ws/{}/subscribe?organization_uuid={}",
            ws_base, self.session_id, self.org_uuid
        )
    }

    pub fn validate(&self) -> Result<(), RemoteError> {
        if self.session_id.is_empty() {
            return Err(RemoteError::Config("session_id is empty".into()));
        }
        if self.base_url.is_empty() {
            return Err(RemoteError::Config("base_url is empty".into()));
        }
        if self.access_token.is_empty() {
            return Err(RemoteError::Config("access_token is empty".into()));
        }
        if self.org_uuid.is_empty() {
            return Err(RemoteError::Config("org_uuid is empty".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RemoteMessage {
    Assistant {
        message: serde_json::Value,
        uuid: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    User {
        message: serde_json::Value,
        uuid: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_use_result: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<String>,
    },
    StreamEvent {
        event: serde_json::Value,
        uuid: String,
    },
    Result {
        subtype: String,
        uuid: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        errors: Option<Vec<String>>,
    },
    System {
        subtype: String,
        uuid: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<String>,
    },
    ToolProgress {
        uuid: String,
        tool_name: String,
        tool_use_id: String,
        elapsed_time_seconds: f64,
    },
    AuthStatus {
        uuid: String,
        #[serde(flatten)]
        extra: HashMap<String, serde_json::Value>,
    },
    RateLimitEvent {
        uuid: String,
        #[serde(flatten)]
        extra: HashMap<String, serde_json::Value>,
    },
    ToolUseSummary {
        uuid: String,
        #[serde(flatten)]
        extra: HashMap<String, serde_json::Value>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ControlMessage {
    #[serde(rename = "control_request")]
    ControlRequest {
        request_id: String,
        #[serde(flatten)]
        request: ControlRequestInner,
    },
    #[serde(rename = "control_response")]
    ControlResponse {
        response: ControlResponseBody,
    },
    #[serde(rename = "control_cancel_request")]
    ControlCancelRequest {
        request_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub enum ControlRequestInner {
    CanUseTool {
        tool_name: String,
        tool_use_id: String,
        input: serde_json::Value,
    },
    Interrupt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlResponseBody {
    pub subtype: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<PermissionResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "behavior", rename_all = "snake_case")]
pub enum PermissionResult {
    Allow {
        updated_input: serde_json::Value,
    },
    Deny {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SessionMessage {
    Remote(RemoteMessage),
    Control(ControlMessage),
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum RemoteError {
    #[error("WebSocket error: {0}")]
    WebSocket(String),
    #[error("Connection error: {0}")]
    Connection(String),
    #[error("Authentication error: {0}")]
    Auth(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("Configuration error: {0}")]
    Config(String),
    #[error("Session not found: {0}")]
    SessionNotFound(String),
    #[error("Timeout: {0}")]
    Timeout(String),
    #[error("Not connected")]
    NotConnected,
    #[error("Closed permanently")]
    PermanentClose,
}

impl From<serde_json::Error> for RemoteError {
    fn from(e: serde_json::Error) -> Self {
        RemoteError::Serialization(e.to_string())
    }
}

impl From<tokio_tungstenite::tungstenite::Error> for RemoteError {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        RemoteError::WebSocket(e.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct RemoteConnection {
    pub session_id: String,
    pub url: String,
    pub state: ConnectionState,
}

#[derive(Debug, Clone)]
pub enum RemoteEvent {
    Message(RemoteMessage),
    ControlRequest {
        request_id: String,
        request: ControlRequestInner,
    },
    ControlCancel {
        request_id: String,
    },
    ControlResponse(ControlResponseBody),
    Connected,
    Disconnected,
    Reconnecting,
    Error(RemoteError),
}

type EventSender = mpsc::UnboundedSender<RemoteEvent>;

struct Inner {
    config: RemoteConfig,
    ws: Arc<Mutex<Option<WsStream>>>,
    state: Arc<AtomicU32>,
    event_tx: EventSender,
    reconnect_attempts: Arc<AtomicU32>,
    session_not_found_retries: Arc<AtomicU32>,
    shutdown: Arc<Notify>,
    running: Arc<AtomicBool>,
    pending_permissions: Arc<RwLock<HashMap<String, ControlRequestInner>>>,
}

impl ConnectionState {
    fn from_u32(v: u32) -> Self {
        match v {
            0 => ConnectionState::Disconnected,
            1 => ConnectionState::Connecting,
            2 => ConnectionState::Connected,
            3 => ConnectionState::Reconnecting,
            _ => ConnectionState::Disconnected,
        }
    }

    fn to_u32(self) -> u32 {
        match self {
            ConnectionState::Disconnected => 0,
            ConnectionState::Connecting => 1,
            ConnectionState::Connected => 2,
            ConnectionState::Reconnecting => 3,
        }
    }
}

pub struct RemoteSessionManager {
    inner: Arc<Inner>,
    event_rx: Mutex<mpsc::UnboundedReceiver<RemoteEvent>>,
}

impl RemoteSessionManager {
    pub fn new(config: RemoteConfig) -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let state_code = ConnectionState::Disconnected.to_u32();

        let inner = Inner {
            config,
            ws: Arc::new(Mutex::new(None)),
            state: Arc::new(AtomicU32::new(state_code)),
            event_tx,
            reconnect_attempts: Arc::new(AtomicU32::new(0)),
            session_not_found_retries: Arc::new(AtomicU32::new(0)),
            shutdown: Arc::new(Notify::new()),
            running: Arc::new(AtomicBool::new(false)),
            pending_permissions: Arc::new(RwLock::new(HashMap::new())),
        };

        Self {
            inner: Arc::new(inner),
            event_rx: Mutex::new(event_rx),
        }
    }

    pub fn connection_state(&self) -> ConnectionState {
        ConnectionState::from_u32(self.inner.state.load(Ordering::Relaxed))
    }

    pub fn is_connected(&self) -> bool {
        self.connection_state() == ConnectionState::Connected
    }

    pub fn session_id(&self) -> &str {
        &self.inner.config.session_id
    }

    pub fn connection_info(&self) -> RemoteConnection {
        RemoteConnection {
            session_id: self.inner.config.session_id.clone(),
            url: self.inner.config.ws_subscribe_url(),
            state: self.connection_state(),
        }
    }

    pub async fn connect(&self) -> Result<(), RemoteError> {
        self.inner.config.validate()?;
        let current = self.connection_state();
        if current == ConnectionState::Connecting || current == ConnectionState::Connected {
            return Ok(());
        }

        self.set_state(ConnectionState::Connecting);
        self.inner.running.store(true, Ordering::Relaxed);
        self.inner.reconnect_attempts.store(0, Ordering::Relaxed);
        self.inner
            .session_not_found_retries
            .store(0, Ordering::Relaxed);

        let inner = Arc::clone(&self.inner);
        self.spawn_connection_loop(inner);
        Ok(())
    }

    fn spawn_connection_loop(&self, inner: Arc<Inner>) {
        tokio::spawn(async move {
            loop {
                if !inner.running.load(Ordering::Relaxed) {
                    break;
                }

                let config = inner.config.clone();
                let url = config.ws_subscribe_url();

                debug!(target: "remote", "Connecting to {}", url);

                let mut request = match url.into_client_request() {
                    Ok(r) => r,
                    Err(e) => {
                        error!(target: "remote", "Failed to build request: {}", e);
                        inner
                            .event_tx
                            .send(RemoteEvent::Error(RemoteError::Connection(
                                e.to_string(),
                            )))
                            .ok();
                        break;
                    }
                };

                let headers = request.headers_mut();
                if let Ok(auth_val) = format!("Bearer {}", config.access_token).parse() {
                    headers.insert("Authorization", auth_val);
                }
                if let Ok(version_val) = "2023-06-01".parse() {
                    headers.insert("anthropic-version", version_val);
                }

                match connect_async(request).await {
                    Ok((ws, _)) => {
                        inner.reconnect_attempts.store(0, Ordering::Relaxed);
                        inner
                            .session_not_found_retries
                            .store(0, Ordering::Relaxed);
                        inner.state.store(
                            ConnectionState::Connected.to_u32(),
                            Ordering::Relaxed,
                        );

                        debug!(target: "remote", "WebSocket connected");
                        inner.event_tx.send(RemoteEvent::Connected).ok();

                        let (sink, recv) = ws.split();
                        let ws_guard = Arc::clone(&inner.ws);
                        {
                            let mut guard = ws_guard.lock().await;
                            *guard = Some(sink.reunite(recv).expect("reunite ws halves"));
                        }

                        let ping_inner = Arc::clone(&inner);
                        let ping_shutdown = Arc::clone(&inner.shutdown);
                        let ping_running = Arc::clone(&inner.running);
                        let ping_handle = tokio::spawn(async move {
                            let mut ticker = interval(Duration::from_millis(PING_INTERVAL_MS));
                            loop {
                                ticker.tick().await;
                                if !ping_running.load(Ordering::Relaxed) {
                                    break;
                                }
                                let ws_guard = ping_inner.ws.lock().await;
                                if ws_guard.is_some() {
                                    debug!(target: "remote", "Ping tick (keepalive)");
                                }
                                drop(ws_guard);
                                tokio::select! {
                                    _ = ping_shutdown.notified() => break,
                                    _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                                }
                            }
                        });

                        let read_state = Arc::clone(&inner.state);
                        let read_tx = inner.event_tx.clone();
                        let read_reconnect = Arc::clone(&inner.reconnect_attempts);
                        let read_session_nf = Arc::clone(&inner.session_not_found_retries);
                        let read_running = Arc::clone(&inner.running);
                        let read_shutdown = Arc::clone(&inner.shutdown);
                        let read_pending = Arc::clone(&inner.pending_permissions);
                        let read_ws = Arc::clone(&inner.ws);

                        {
                            let mut guard = read_ws.lock().await;
                            if let Some(ws) = guard.take() {
                                let (mut write_sink, mut recv) = ws.split();

                                let event_tx_clone = read_tx.clone();
                                let state_clone = Arc::clone(&read_state);
                                let reconnect_clone = Arc::clone(&read_reconnect);
                                let session_nf_clone = Arc::clone(&read_session_nf);
                                let running_clone = Arc::clone(&read_running);
                                let shutdown_clone = Arc::clone(&read_shutdown);
                                let pending_clone = Arc::clone(&read_pending);

                                tokio::spawn(async move {
                                    while let Some(msg_result) = recv.next().await {
                                        match msg_result {
                                            Ok(Message::Text(text)) => {
                                                Self::handle_raw_message(
                                                    &text,
                                                    &event_tx_clone,
                                                    &pending_clone,
                                                );
                                            }
                                            Ok(Message::Ping(_)) => {
                                                debug!(target: "remote", "Received ping");
                                                let _ = write_sink
                                                    .send(Message::Pong(vec![]))
                                                    .await;
                                            }
                                            Ok(Message::Close(close_frame)) => {
                                                let code: u16 = close_frame
                                                    .as_ref()
                                                    .map(|f| u16::from(f.code))
                                                    .unwrap_or(0);
                                                debug!(target: "remote", "WebSocket closed: code={}", code);

                                                state_clone.store(
                                                    ConnectionState::Disconnected.to_u32(),
                                                    Ordering::Relaxed,
                                                );

                                                if Self::is_permanent_close(code) {
                                                    info!(target: "remote", "Permanent close, stopping");
                                                    running_clone.store(false, Ordering::Relaxed);
                                                    event_tx_clone.send(RemoteEvent::Disconnected).ok();
                                                    break;
                                                }

                                                if code == 4001 {
                                                    let retries = session_nf_clone
                                                        .fetch_add(1, Ordering::Relaxed)
                                                        + 1;
                                                    if retries > MAX_SESSION_NOT_FOUND_RETRIES {
                                                        info!(target: "remote", "4001 retry budget exhausted");
                                                        running_clone.store(false, Ordering::Relaxed);
                                                        event_tx_clone.send(RemoteEvent::Disconnected).ok();
                                                        break;
                                                    }
                                                }

                                                let _prev = state_clone.load(Ordering::Relaxed);
                                                let attempts = reconnect_clone
                                                    .fetch_add(1, Ordering::Relaxed)
                                                    + 1;
                                                if attempts > MAX_RECONNECT_ATTEMPTS {
                                                    running_clone.store(false, Ordering::Relaxed);
                                                    event_tx_clone.send(RemoteEvent::Disconnected).ok();
                                                    break;
                                                }

                                                state_clone.store(
                                                    ConnectionState::Reconnecting.to_u32(),
                                                    Ordering::Relaxed,
                                                );
                                                event_tx_clone.send(RemoteEvent::Reconnecting).ok();

                                                tokio::time::sleep(Duration::from_millis(
                                                    RECONNECT_DELAY_MS,
                                                ))
                                                .await;
                                                break;
                                            }
                                            Ok(_) => {}
                                            Err(e) => {
                                                error!(target: "remote", "WS read error: {}", e);
                                                event_tx_clone
                                                    .send(RemoteEvent::Error(
                                                        RemoteError::WebSocket(e.to_string()),
                                                    ))
                                                    .ok();
                                                break;
                                            }
                                        }
                                    }

                                    shutdown_clone.notify_waiters();
                                    ping_handle.abort();
                                });
                            }
                        }

                        tokio::select! {
                            _ = inner.shutdown.notified() => {
                                debug!(target: "remote", "Connection loop shutting down");
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        error!(target: "remote", "Connection failed: {}", e);
                        let attempts =
                            inner.reconnect_attempts.fetch_add(1, Ordering::Relaxed) + 1;
                        inner
                            .event_tx
                            .send(RemoteEvent::Error(RemoteError::Connection(
                                e.to_string(),
                            )))
                            .ok();

                        if attempts > MAX_RECONNECT_ATTEMPTS {
                            inner.state.store(
                                ConnectionState::Disconnected.to_u32(),
                                Ordering::Relaxed,
                            );
                            inner.event_tx.send(RemoteEvent::Disconnected).ok();
                            break;
                        }

                        inner.state.store(
                            ConnectionState::Reconnecting.to_u32(),
                            Ordering::Relaxed,
                        );
                        inner.event_tx.send(RemoteEvent::Reconnecting).ok();

                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_millis(RECONNECT_DELAY_MS)) => {}
                            _ = inner.shutdown.notified() => break,
                        }
                    }
                }
            }

            inner.state.store(
                ConnectionState::Disconnected.to_u32(),
                Ordering::Relaxed,
            );
            debug!(target: "remote", "Connection loop exited");
        });
    }

    fn handle_raw_message(
        text: &str,
        tx: &EventSender,
        pending: &Arc<RwLock<HashMap<String, ControlRequestInner>>>,
    ) {
        let parsed: serde_json::Result<serde_json::Value> = serde_json::from_str(text);
        match parsed {
            Ok(val) => {
                let msg_type = val
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                match msg_type.as_str() {
                    "control_request" => {
                        if let Ok(ctrl) =
                            serde_json::from_value::<ControlMessage>(val.clone())
                        {
                            if let ControlMessage::ControlRequest {
                                request_id,
                                request,
                            } = ctrl
                            {
                                debug!(target: "remote", "Control request: request_id={}", request_id);
                                let mut guard = pending.blocking_write();
                                guard.insert(request_id.clone(), request.clone());
                                drop(guard);
                                tx.send(RemoteEvent::ControlRequest {
                                    request_id,
                                    request,
                                })
                                .ok();
                            }
                        }
                    }
                    "control_cancel_request" => {
                        if let Some(rid) = val.get("request_id").and_then(|v| v.as_str()) {
                            debug!(target: "remote", "Control cancel: request_id={}", rid);
                            let mut guard = pending.blocking_write();
                            guard.remove(rid);
                            drop(guard);
                            tx.send(RemoteEvent::ControlCancel {
                                request_id: rid.to_string(),
                            })
                            .ok();
                        }
                    }
                    "control_response" => {
                        if let Ok(resp) = serde_json::from_value::<ControlResponseBody>(
                            val.get("response").cloned().unwrap_or_default(),
                        ) {
                            debug!(target: "remote", "Control response received");
                            tx.send(RemoteEvent::ControlResponse(resp)).ok();
                        }
                    }
                    _ => {
                        if let Ok(msg) = serde_json::from_value::<RemoteMessage>(val) {
                            tx.send(RemoteEvent::Message(msg)).ok();
                        } else {
                            debug!(target: "remote", "Ignoring unknown message type: {}", msg_type);
                        }
                    }
                }
            }
            Err(e) => {
                error!(target: "remote", "Failed to parse message: {}", e);
            }
        }
    }

    fn is_permanent_close(code: u16) -> bool {
        PERMANENT_CLOSE_CODES.contains(&code)
    }

    fn set_state(&self, state: ConnectionState) {
        self.inner.state.store(state.to_u32(), Ordering::Relaxed);
    }

    pub async fn send_control_response(
        &self,
        request_id: &str,
        result: PermissionResult,
    ) -> Result<(), RemoteError> {
        if !self.is_connected() {
            return Err(RemoteError::NotConnected);
        }

        {
            let mut pending = self.inner.pending_permissions.write().await;
            if pending.remove(request_id).is_none() {
                warn!(target: "remote", "No pending permission request: {}", request_id);
            }
        }

        let response = ControlMessage::ControlResponse {
            response: ControlResponseBody {
                subtype: "success".to_string(),
                request_id: Some(request_id.to_string()),
                error: None,
                response: Some(result),
            },
        };

        self.send_raw(&response).await
    }

    pub async fn send_interrupt(&self) -> Result<(), RemoteError> {
        if !self.is_connected() {
            return Err(RemoteError::NotConnected);
        }

        let request = ControlMessage::ControlRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            request: ControlRequestInner::Interrupt,
        };

        self.send_raw(&request).await
    }

    async fn send_raw<T: Serialize>(&self, msg: &T) -> Result<(), RemoteError> {
        let text = serde_json::to_string(msg)?;
        let mut guard = self.inner.ws.lock().await;
        if let Some(ws) = guard.as_mut() {
            ws.send(Message::Text(text))
                .await
                .map_err(|e| RemoteError::WebSocket(e.to_string()))?;
            Ok(())
        } else {
            Err(RemoteError::NotConnected)
        }
    }

    pub async fn disconnect(&self) {
        self.inner.running.store(false, Ordering::Relaxed);
        self.inner.shutdown.notify_waiters();
        self.set_state(ConnectionState::Disconnected);

        let mut guard = self.inner.ws.lock().await;
        if let Some(ws) = guard.take() {
            let (mut sink, _) = ws.split();
            let _ = sink.send(Message::Close(None)).await;
        }

        self.inner.pending_permissions.write().await.clear();
        debug!(target: "remote", "Disconnected");
    }

    pub async fn reconnect(&self) -> Result<(), RemoteError> {
        self.disconnect().await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        self.connect().await
    }

    pub async fn next_event(&self) -> Option<RemoteEvent> {
        let mut rx = self.event_rx.lock().await;
        rx.recv().await
    }

    pub async fn recv_timeout(&self, dur: Duration) -> Option<RemoteEvent> {
        let mut rx = self.event_rx.lock().await;
        timeout(dur, rx.recv()).await.ok().flatten()
    }

    pub async fn pending_permission_count(&self) -> usize {
        self.inner.pending_permissions.read().await.len()
    }

    pub async fn send_message(
        &self,
        content: serde_json::Value,
    ) -> Result<(), RemoteError> {
        if !self.is_connected() {
            return Err(RemoteError::NotConnected);
        }

        let msg = RemoteMessage::User {
            message: content,
            uuid: uuid::Uuid::new_v4().to_string(),
            tool_use_result: None,
            timestamp: Some(chrono::Utc::now().to_rfc3339()),
        };

        self.send_raw(&msg).await
    }
}

pub struct SdkMessageAdapter;

impl SdkMessageAdapter {
    pub fn convert(msg: &RemoteMessage) -> ConvertedMessage {
        match msg {
            RemoteMessage::Assistant { uuid, .. } => ConvertedMessage::Message {
                display: format!("[Assistant] uuid={}", uuid),
            },
            RemoteMessage::StreamEvent { event, .. } => ConvertedMessage::StreamEvent {
                event_type: event
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
            },
            RemoteMessage::Result {
                subtype, errors, ..
            } => {
                if subtype != "success" {
                    ConvertedMessage::Message {
                        display: format!(
                            "Error: {}",
                            errors
                                .as_ref()
                                .map(|e| e.join(", "))
                                .unwrap_or_else(|| "Unknown error".into())
                        ),
                    }
                } else {
                    ConvertedMessage::Ignored
                }
            }
            RemoteMessage::System {
                subtype, model, ..
            } => match subtype.as_str() {
                "init" => ConvertedMessage::Message {
                    display: format!(
                        "Remote session initialized (model: {})",
                        model.as_deref().unwrap_or("unknown")
                    ),
                },
                "status" => ConvertedMessage::Ignored,
                "compact_boundary" => ConvertedMessage::Message {
                    display: "Conversation compacted".to_string(),
                },
                _ => ConvertedMessage::Ignored,
            },
            RemoteMessage::ToolProgress {
                tool_name,
                elapsed_time_seconds,
                ..
            } => ConvertedMessage::Message {
                display: format!(
                    "Tool {} running for {:.1}s",
                    tool_name, elapsed_time_seconds
                ),
            },
            RemoteMessage::User { .. } => ConvertedMessage::Ignored,
            RemoteMessage::AuthStatus { .. } => ConvertedMessage::Ignored,
            RemoteMessage::RateLimitEvent { .. } => ConvertedMessage::Ignored,
            RemoteMessage::ToolUseSummary { .. } => ConvertedMessage::Ignored,
        }
    }

    pub fn is_session_end(msg: &RemoteMessage) -> bool {
        matches!(msg, RemoteMessage::Result { .. })
    }

    pub fn is_success_result(msg: &RemoteMessage) -> bool {
        matches!(
            msg,
            RemoteMessage::Result { subtype, .. } if subtype == "success"
        )
    }

    pub fn get_result_text(msg: &RemoteMessage) -> Option<String> {
        match msg {
            RemoteMessage::Result {
                subtype, result, ..
            } if subtype == "success" => result.clone(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ConvertedMessage {
    Message { display: String },
    StreamEvent { event_type: String },
    Ignored,
}

pub fn create_remote_config(
    session_id: impl Into<String>,
    base_url: impl Into<String>,
    access_token: impl Into<String>,
    org_uuid: impl Into<String>,
) -> RemoteConfig {
    RemoteConfig {
        session_id: session_id.into(),
        base_url: base_url.into(),
        org_uuid: org_uuid.into(),
        access_token: access_token.into(),
        has_initial_prompt: false,
        viewer_only: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remote_context_default() {
        let ctx = RemoteContext::default();
        assert!(!ctx.is_remote);
        assert!(ctx.upstream_url.is_none());
        assert!(ctx.session_id.is_none());
    }

    #[test]
    fn test_remote_config_ws_url() {
        let config =
            create_remote_config("sess-123", "https://api.example.com", "token", "org-456");
        assert_eq!(
            config.ws_subscribe_url(),
            "wss://api.example.com/v1/sessions/ws/sess-123/subscribe?organization_uuid=org-456"
        );
    }

    #[test]
    fn test_remote_config_ws_url_http() {
        let config =
            create_remote_config("sess-123", "http://localhost:8080", "token", "org-456");
        assert_eq!(
            config.ws_subscribe_url(),
            "ws://localhost:8080/v1/sessions/ws/sess-123/subscribe?organization_uuid=org-456"
        );
    }

    #[test]
    fn test_remote_config_validate_ok() {
        let config =
            create_remote_config("sess-123", "https://api.example.com", "token", "org-456");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_remote_config_validate_empty_session() {
        let config =
            create_remote_config("", "https://api.example.com", "token", "org-456");
        assert!(matches!(config.validate(), Err(RemoteError::Config(_))));
    }

    #[test]
    fn test_remote_config_validate_empty_url() {
        let config = create_remote_config("sess-123", "", "token", "org-456");
        assert!(matches!(config.validate(), Err(RemoteError::Config(_))));
    }

    #[test]
    fn test_remote_config_validate_empty_token() {
        let config =
            create_remote_config("sess-123", "https://api.example.com", "", "org-456");
        assert!(matches!(config.validate(), Err(RemoteError::Config(_))));
    }

    #[test]
    fn test_remote_config_validate_empty_org() {
        let config =
            create_remote_config("sess-123", "https://api.example.com", "token", "");
        assert!(matches!(config.validate(), Err(RemoteError::Config(_))));
    }

    #[test]
    fn test_connection_state_roundtrip() {
        assert_eq!(
            ConnectionState::from_u32(ConnectionState::Disconnected.to_u32()),
            ConnectionState::Disconnected
        );
        assert_eq!(
            ConnectionState::from_u32(ConnectionState::Connecting.to_u32()),
            ConnectionState::Connecting
        );
        assert_eq!(
            ConnectionState::from_u32(ConnectionState::Connected.to_u32()),
            ConnectionState::Connected
        );
        assert_eq!(
            ConnectionState::from_u32(ConnectionState::Reconnecting.to_u32()),
            ConnectionState::Reconnecting
        );
    }

    #[test]
    fn test_session_message_deserialization() {
        let json = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hello"}]},"uuid":"abc-123"}"#;
        let msg: RemoteMessage = serde_json::from_str(json).unwrap();
        match msg {
            RemoteMessage::Assistant { uuid, .. } => assert_eq!(uuid, "abc-123"),
            _ => panic!("Expected Assistant variant"),
        }
    }

    #[test]
    fn test_result_message_deserialization() {
        let json = r#"{"type":"result","subtype":"success","uuid":"res-1","result":"done"}"#;
        let msg: RemoteMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(
            msg,
            RemoteMessage::Result { subtype, .. } if subtype == "success"
        ));
    }

    #[test]
    fn test_result_message_error() {
        let json = r#"{"type":"result","subtype":"error","uuid":"res-2","errors":["bad thing"]}"#;
        let msg: RemoteMessage = serde_json::from_str(json).unwrap();
        match msg {
            RemoteMessage::Result { subtype, errors, .. } => {
                assert_eq!(subtype, "error");
                assert_eq!(errors.unwrap(), vec!["bad thing"]);
            }
            _ => panic!("Expected Result variant"),
        }
    }

    #[test]
    fn test_system_init_message() {
        let json = r#"{"type":"system","subtype":"init","uuid":"sys-1","model":"claude-sonnet"}"#;
        let msg: RemoteMessage = serde_json::from_str(json).unwrap();
        match msg {
            RemoteMessage::System { subtype, model, .. } => {
                assert_eq!(subtype, "init");
                assert_eq!(model.unwrap(), "claude-sonnet");
            }
            _ => panic!("Expected System variant"),
        }
    }

    #[test]
    fn test_tool_progress_message() {
        let json = r#"{"type":"tool_progress","uuid":"tp-1","tool_name":"Bash","tool_use_id":"tu-1","elapsed_time_seconds":3.5}"#;
        let msg: RemoteMessage = serde_json::from_str(json).unwrap();
        match msg {
            RemoteMessage::ToolProgress {
                tool_name,
                elapsed_time_seconds,
                ..
            } => {
                assert_eq!(tool_name, "Bash");
                assert!((elapsed_time_seconds - 3.5).abs() < f64::EPSILON);
            }
            _ => panic!("Expected ToolProgress variant"),
        }
    }

    #[test]
    fn test_control_request_deserialization() {
        let json = r#"{"type":"control_request","request_id":"req-1","subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"tu-1","input":{"command":"ls"}}"#;
        let msg: ControlMessage = serde_json::from_str(json).unwrap();
        match msg {
            ControlMessage::ControlRequest {
                request_id,
                request,
            } => {
                assert_eq!(request_id, "req-1");
                match request {
                    ControlRequestInner::CanUseTool {
                        tool_name,
                        tool_use_id,
                        input,
                    } => {
                        assert_eq!(tool_name, "Bash");
                        assert_eq!(tool_use_id, "tu-1");
                        assert_eq!(input["command"], "ls");
                    }
                    _ => panic!("Expected CanUseTool"),
                }
            }
            _ => panic!("Expected ControlRequest"),
        }
    }

    #[test]
    fn test_control_interrupt() {
        let json = r#"{"type":"control_request","request_id":"req-2","subtype":"interrupt"}"#;
        let msg: ControlMessage = serde_json::from_str(json).unwrap();
        match msg {
            ControlMessage::ControlRequest { request, .. } => {
                assert!(matches!(request, ControlRequestInner::Interrupt));
            }
            _ => panic!("Expected ControlRequest"),
        }
    }

    #[test]
    fn test_control_cancel_request() {
        let json = r#"{"type":"control_cancel_request","request_id":"req-3"}"#;
        let msg: ControlMessage = serde_json::from_str(json).unwrap();
        match msg {
            ControlMessage::ControlCancelRequest { request_id } => {
                assert_eq!(request_id, "req-3");
            }
            _ => panic!("Expected ControlCancelRequest"),
        }
    }

    #[test]
    fn test_permission_result_serialization() {
        let allow = PermissionResult::Allow {
            updated_input: serde_json::json!({"key": "value"}),
        };
        let json = serde_json::to_string(&allow).unwrap();
        assert!(json.contains("\"behavior\":\"allow\""));

        let deny = PermissionResult::Deny {
            message: "forbidden".to_string(),
        };
        let json = serde_json::to_string(&deny).unwrap();
        assert!(json.contains("\"behavior\":\"deny\""));
    }

    #[test]
    fn test_sdk_message_adapter_assistant() {
        let msg = RemoteMessage::Assistant {
            message: serde_json::json!({"role": "assistant"}),
            uuid: "u-1".to_string(),
            error: None,
        };
        let converted = SdkMessageAdapter::convert(&msg);
        match converted {
            ConvertedMessage::Message { display } => assert!(display.contains("u-1")),
            _ => panic!("Expected Message"),
        }
    }

    #[test]
    fn test_sdk_message_adapter_result_success_ignored() {
        let msg = RemoteMessage::Result {
            subtype: "success".to_string(),
            uuid: "u-1".to_string(),
            result: Some("done".to_string()),
            errors: None,
        };
        let converted = SdkMessageAdapter::convert(&msg);
        assert!(matches!(converted, ConvertedMessage::Ignored));
    }

    #[test]
    fn test_sdk_message_adapter_result_error() {
        let msg = RemoteMessage::Result {
            subtype: "error".to_string(),
            uuid: "u-1".to_string(),
            result: None,
            errors: Some(vec!["fail".to_string()]),
        };
        let converted = SdkMessageAdapter::convert(&msg);
        match converted {
            ConvertedMessage::Message { display } => assert!(display.contains("fail")),
            _ => panic!("Expected Message"),
        }
    }

    #[test]
    fn test_sdk_message_adapter_system_init() {
        let msg = RemoteMessage::System {
            subtype: "init".to_string(),
            uuid: "u-1".to_string(),
            model: Some("claude-sonnet".to_string()),
            status: None,
        };
        let converted = SdkMessageAdapter::convert(&msg);
        match converted {
            ConvertedMessage::Message { display } => {
                assert!(display.contains("claude-sonnet"));
            }
            _ => panic!("Expected Message"),
        }
    }

    #[test]
    fn test_sdk_message_adapter_tool_progress() {
        let msg = RemoteMessage::ToolProgress {
            uuid: "u-1".to_string(),
            tool_name: "Bash".to_string(),
            tool_use_id: "tu-1".to_string(),
            elapsed_time_seconds: 5.0,
        };
        let converted = SdkMessageAdapter::convert(&msg);
        match converted {
            ConvertedMessage::Message { display } => {
                assert!(display.contains("Bash"));
                assert!(display.contains("5.0"));
            }
            _ => panic!("Expected Message"),
        }
    }

    #[test]
    fn test_sdk_message_adapter_ignored_types() {
        let msg = RemoteMessage::AuthStatus {
            uuid: "u-1".to_string(),
            extra: HashMap::new(),
        };
        assert!(matches!(
            SdkMessageAdapter::convert(&msg),
            ConvertedMessage::Ignored
        ));

        let msg = RemoteMessage::RateLimitEvent {
            uuid: "u-1".to_string(),
            extra: HashMap::new(),
        };
        assert!(matches!(
            SdkMessageAdapter::convert(&msg),
            ConvertedMessage::Ignored
        ));
    }

    #[test]
    fn test_is_session_end() {
        let msg = RemoteMessage::Result {
            subtype: "success".to_string(),
            uuid: "u-1".to_string(),
            result: None,
            errors: None,
        };
        assert!(SdkMessageAdapter::is_session_end(&msg));

        let msg = RemoteMessage::Assistant {
            message: serde_json::json!({}),
            uuid: "u-1".to_string(),
            error: None,
        };
        assert!(!SdkMessageAdapter::is_session_end(&msg));
    }

    #[test]
    fn test_is_success_result() {
        let msg = RemoteMessage::Result {
            subtype: "success".to_string(),
            uuid: "u-1".to_string(),
            result: None,
            errors: None,
        };
        assert!(SdkMessageAdapter::is_success_result(&msg));

        let msg = RemoteMessage::Result {
            subtype: "error".to_string(),
            uuid: "u-1".to_string(),
            result: None,
            errors: None,
        };
        assert!(!SdkMessageAdapter::is_success_result(&msg));
    }

    #[test]
    fn test_get_result_text() {
        let msg = RemoteMessage::Result {
            subtype: "success".to_string(),
            uuid: "u-1".to_string(),
            result: Some("done".to_string()),
            errors: None,
        };
        assert_eq!(
            SdkMessageAdapter::get_result_text(&msg),
            Some("done".to_string())
        );

        let msg = RemoteMessage::Result {
            subtype: "error".to_string(),
            uuid: "u-1".to_string(),
            result: Some("fail".to_string()),
            errors: None,
        };
        assert_eq!(SdkMessageAdapter::get_result_text(&msg), None);
    }

    #[test]
    fn test_control_response_body_serialization() {
        let body = ControlResponseBody {
            subtype: "success".to_string(),
            request_id: Some("req-1".to_string()),
            error: None,
            response: Some(PermissionResult::Allow {
                updated_input: serde_json::json!({"command": "ls -la"}),
            }),
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains("\"subtype\":\"success\""));
        assert!(json.contains("\"request_id\":\"req-1\""));
        assert!(json.contains("\"behavior\":\"allow\""));
    }

    #[test]
    fn test_control_response_error() {
        let body = ControlResponseBody {
            subtype: "error".to_string(),
            request_id: Some("req-1".to_string()),
            error: Some("unsupported".to_string()),
            response: None,
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains("\"error\":\"unsupported\""));
    }

    #[test]
    fn test_remote_error_display() {
        let err = RemoteError::WebSocket("conn refused".to_string());
        assert!(err.to_string().contains("conn refused"));

        let err = RemoteError::Auth("bad token".to_string());
        assert!(err.to_string().contains("bad token"));

        let err = RemoteError::NotConnected;
        assert!(err.to_string().contains("Not connected"));
    }

    #[test]
    fn test_manager_initial_state() {
        let config = create_remote_config("sess-1", "https://api.example.com", "tok", "org-1");
        let manager = RemoteSessionManager::new(config);
        assert!(!manager.is_connected());
        assert_eq!(manager.connection_state(), ConnectionState::Disconnected);
        assert_eq!(manager.session_id(), "sess-1");
    }

    #[test]
    fn test_manager_connection_info() {
        let config = create_remote_config("sess-1", "https://api.example.com", "tok", "org-1");
        let manager = RemoteSessionManager::new(config);
        let info = manager.connection_info();
        assert_eq!(info.session_id, "sess-1");
        assert!(info.url.contains("wss://"));
        assert_eq!(info.state, ConnectionState::Disconnected);
    }

    #[tokio::test]
    async fn test_manager_disconnect_when_not_connected() {
        let config = create_remote_config("sess-1", "https://api.example.com", "tok", "org-1");
        let manager = RemoteSessionManager::new(config);
        manager.disconnect().await;
        assert!(!manager.is_connected());
    }

    #[tokio::test]
    async fn test_send_interrupt_not_connected() {
        let config = create_remote_config("sess-1", "https://api.example.com", "tok", "org-1");
        let manager = RemoteSessionManager::new(config);
        let result = manager.send_interrupt().await;
        assert!(matches!(result, Err(RemoteError::NotConnected)));
    }

    #[tokio::test]
    async fn test_send_control_response_not_connected() {
        let config = create_remote_config("sess-1", "https://api.example.com", "tok", "org-1");
        let manager = RemoteSessionManager::new(config);
        let result = manager
            .send_control_response(
                "req-1",
                PermissionResult::Deny {
                    message: "no".into(),
                },
            )
            .await;
        assert!(matches!(result, Err(RemoteError::NotConnected)));
    }

    #[tokio::test]
    async fn test_send_message_not_connected() {
        let config = create_remote_config("sess-1", "https://api.example.com", "tok", "org-1");
        let manager = RemoteSessionManager::new(config);
        let result = manager
            .send_message(serde_json::json!({"text": "hello"}))
            .await;
        assert!(matches!(result, Err(RemoteError::NotConnected)));
    }

    #[tokio::test]
    async fn test_pending_permissions_empty() {
        let config = create_remote_config("sess-1", "https://api.example.com", "tok", "org-1");
        let manager = RemoteSessionManager::new(config);
        assert_eq!(manager.pending_permission_count().await, 0);
    }

    #[test]
    fn test_permanent_close_codes() {
        assert!(RemoteSessionManager::is_permanent_close(4003));
        assert!(!RemoteSessionManager::is_permanent_close(4001));
        assert!(!RemoteSessionManager::is_permanent_close(1000));
    }

    #[test]
    fn test_stream_event_message() {
        let json = r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"hi"}},"uuid":"se-1"}"#;
        let msg: RemoteMessage = serde_json::from_str(json).unwrap();
        match msg {
            RemoteMessage::StreamEvent { event, uuid } => {
                assert_eq!(uuid, "se-1");
                assert_eq!(event["type"], "content_block_delta");
            }
            _ => panic!("Expected StreamEvent"),
        }
    }

    #[test]
    fn test_sdk_adapter_stream_event() {
        let msg = RemoteMessage::StreamEvent {
            event: serde_json::json!({"type": "content_block_delta"}),
            uuid: "se-1".to_string(),
        };
        let converted = SdkMessageAdapter::convert(&msg);
        match converted {
            ConvertedMessage::StreamEvent { event_type } => {
                assert_eq!(event_type, "content_block_delta");
            }
            _ => panic!("Expected StreamEvent"),
        }
    }

    #[test]
    fn test_user_message_serialization() {
        let msg = RemoteMessage::User {
            message: serde_json::json!({"content": "hello"}),
            uuid: "u-1".to_string(),
            tool_use_result: None,
            timestamp: Some("2024-01-01T00:00:00Z".to_string()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"user\""));
        assert!(json.contains("\"uuid\":\"u-1\""));
    }

    #[test]
    fn test_remote_context_serialization() {
        let ctx = RemoteContext {
            is_remote: true,
            upstream_url: Some("wss://api.example.com".to_string()),
            session_id: Some("sess-123".to_string()),
        };
        let json = serde_json::to_string(&ctx).unwrap();
        assert!(json.contains("\"is_remote\":true"));
        assert!(json.contains("wss://api.example.com"));

        let decoded: RemoteContext = serde_json::from_str(&json).unwrap();
        assert!(decoded.is_remote);
        assert_eq!(decoded.session_id.unwrap(), "sess-123");
    }

    #[test]
    fn test_handle_raw_message_control_request() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let pending: Arc<RwLock<HashMap<String, ControlRequestInner>>> =
            Arc::new(RwLock::new(HashMap::new()));

        let text = r#"{"type":"control_request","request_id":"r-1","subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"tu-1","input":{}}"#;
        RemoteSessionManager::handle_raw_message(text, &tx, &pending);

        let event = rx.try_recv().unwrap();
        match event {
            RemoteEvent::ControlRequest { request_id, .. } => {
                assert_eq!(request_id, "r-1");
            }
            _ => panic!("Expected ControlRequest event"),
        }
        assert_eq!(pending.blocking_read().len(), 1);
    }

    #[test]
    fn test_handle_raw_message_control_cancel() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let pending: Arc<RwLock<HashMap<String, ControlRequestInner>>> =
            Arc::new(RwLock::new(HashMap::new()));

        let text = r#"{"type":"control_cancel_request","request_id":"r-2"}"#;
        RemoteSessionManager::handle_raw_message(text, &tx, &pending);

        let event = rx.try_recv().unwrap();
        match event {
            RemoteEvent::ControlCancel { request_id } => {
                assert_eq!(request_id, "r-2");
            }
            _ => panic!("Expected ControlCancel event"),
        }
    }

    #[test]
    fn test_handle_raw_message_sdk_message() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let pending: Arc<RwLock<HashMap<String, ControlRequestInner>>> =
            Arc::new(RwLock::new(HashMap::new()));

        let text = r#"{"type":"assistant","message":{"role":"assistant"},"uuid":"u-1"}"#;
        RemoteSessionManager::handle_raw_message(text, &tx, &pending);

        let event = rx.try_recv().unwrap();
        match event {
            RemoteEvent::Message(msg) => match msg {
                RemoteMessage::Assistant { uuid, .. } => assert_eq!(uuid, "u-1"),
                _ => panic!("Expected Assistant"),
            },
            _ => panic!("Expected Message event"),
        }
    }

    #[test]
    fn test_handle_raw_message_invalid_json() {
        let (tx, _) = mpsc::unbounded_channel();
        let pending: Arc<RwLock<HashMap<String, ControlRequestInner>>> =
            Arc::new(RwLock::new(HashMap::new()));

        RemoteSessionManager::handle_raw_message("not json", &tx, &pending);
    }
}
