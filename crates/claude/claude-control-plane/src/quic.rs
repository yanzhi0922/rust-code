//! QUIC server endpoint for the control plane.
//!
//! Accepts QUIC connections from mobile clients and streams timeline events
//! via unidirectional QUIC streams. Events flow server→client, commands
//! can flow client→server on separate streams.

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast;
use tokio::task::JoinSet;
use uuid::Uuid;

use crate::helpers;
use crate::state::ControlPlaneService;
use crate::types::{ApiError, TimelineEvent, TimelineEventDetail, TimelineEventDraft};
use rc_remote_transport::TransportEvent;
use rc_remote_transport::transport::{CommandAck, TransportApprovalDecision, TransportCommand};

const MAX_QUIC_FRAME_BYTES: usize = 1024 * 1024;

/// QUIC server configuration.
pub struct QuicServerConfig {
    pub listen_addr: SocketAddr,
    pub cert_pem: Vec<u8>,
    pub key_pem: Vec<u8>,
}

/// Start the QUIC listener alongside the HTTP server.
pub async fn start_quic_listener(
    service: Arc<ControlPlaneService>,
    config: QuicServerConfig,
) -> anyhow::Result<()> {
    let tls_config = build_quic_server_tls(&config.cert_pem, &config.key_pem)?;
    let server_config = quinn::ServerConfig::with_crypto(Arc::new(tls_config));

    let endpoint = quinn::Endpoint::server(server_config, config.listen_addr)?;
    let local_addr = endpoint.local_addr()?;
    tracing::info!("QUIC server listening on {local_addr}");

    let mut tasks: JoinSet<()> = JoinSet::new();

    loop {
        match endpoint.accept().await {
            Some(incoming) => {
                let service = service.clone();
                tasks.spawn(async move {
                    if let Err(e) = handle_quic_connection(incoming, service).await {
                        tracing::debug!("QUIC connection error: {e}");
                    }
                });
            }
            None => break,
        }

        while tasks.try_join_next().is_some() {}
    }

    Ok(())
}

async fn handle_quic_connection(
    incoming: quinn::Incoming,
    service: Arc<ControlPlaneService>,
) -> anyhow::Result<()> {
    let conn = incoming.await?;

    // Accept the first unidirectional stream — expect an auth message.
    let mut auth_stream = conn.accept_uni().await?;
    let auth_buf = read_len_prefixed_payload(&mut auth_stream).await?;
    let auth: QuicAuthMessage = serde_json::from_slice(&auth_buf)?;

    // Validate auth token — QUIC requires either the shared control-plane token
    // or a short-lived device access token. Refresh tokens are intentionally
    // rejected for stream connections.
    if !authenticate_quic_token(&service, &auth.token).await {
        conn.close(1u32.into(), b"auth failed");
        return Err(anyhow::anyhow!("QUIC auth token mismatch"));
    }

    let target_session: Option<Uuid> = auth.session_id.parse().ok();
    tracing::debug!(
        "QUIC client authenticated for session {}",
        target_session.map_or("all".to_owned(), |s| s.to_string())
    );

    // Subscribe to events via the shared timeline broadcast channel.
    let mut event_rx = service.timeline.subscribe();

    loop {
        tokio::select! {
            event = event_rx.recv() => {
                match event {
                    Ok(event) => {
                        let matches_session = event.session_id == target_session
                            || event.session_id.is_none();
                        if matches_session
                            && let Err(e) = send_quic_event(&conn, &event).await
                        {
                            tracing::debug!("QUIC event send error: {e}");
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("QUIC client lagged {n} events");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            // Accept command streams from client.
            cmd_stream = conn.accept_bi() => {
                match cmd_stream {
                    Ok((mut send, mut recv)) => {
                        let ack = match read_len_prefixed_payload(&mut recv).await {
                            Ok(buf) => dispatch_quic_command(&service, target_session, &buf)
                                .await
                                .unwrap_or_else(|e| {
                                    tracing::warn!("QUIC command dispatch error: {e}");
                                    CommandAck {
                                        accepted: false,
                                        message: e.to_string(),
                                    }
                                }),
                            Err(e) => CommandAck {
                                accepted: false,
                                message: format!("invalid QUIC command frame: {e}"),
                            },
                        };
                        if let Err(e) = write_len_prefixed_payload(&mut send, &ack).await {
                            tracing::debug!("QUIC command ack write error: {e}");
                            break;
                        }
                    }
                    Err(quinn::ConnectionError::ApplicationClosed(_)) => break,
                    Err(e) => {
                        tracing::debug!("QUIC command accept error: {e}");
                        break;
                    }
                }
            }
        }
    }

    conn.close(0u32.into(), b"done");
    Ok(())
}

async fn authenticate_quic_token(service: &ControlPlaneService, provided: &str) -> bool {
    if service
        .auth_token
        .as_deref()
        .is_some_and(|expected| constant_time_eq(provided, expected))
    {
        return true;
    }

    let authenticated_device = {
        let mut registry = service.registry.write().await;
        registry.authenticate_device_token(provided)
    };
    matches!(authenticated_device, Some((_device, true)))
}

/// Deserialize a QUIC command and enqueue it for the appropriate runner.
async fn dispatch_quic_command(
    service: &ControlPlaneService,
    authenticated_session_id: Option<Uuid>,
    payload: &[u8],
) -> anyhow::Result<CommandAck> {
    let cmd: TransportCommand = serde_json::from_slice(payload)?;

    match cmd {
        TransportCommand::SendPrompt { content } => {
            let session_id = authenticated_session_id
                .ok_or_else(|| anyhow::anyhow!("QUIC send_prompt requires a bound session"))?;
            dispatch_runner_session_command(
                service,
                session_id,
                claude_runner::RunnerSessionCommandRequest::SendPrompt { content },
            )
            .await?;
            Ok(CommandAck {
                accepted: true,
                message: "queued".into(),
            })
        }
        TransportCommand::Interrupt => {
            let session_id = authenticated_session_id
                .ok_or_else(|| anyhow::anyhow!("QUIC interrupt requires a bound session"))?;
            dispatch_runner_session_command(
                service,
                session_id,
                claude_runner::RunnerSessionCommandRequest::Interrupt,
            )
            .await?;
            Ok(CommandAck {
                accepted: true,
                message: "queued".into(),
            })
        }
        TransportCommand::RespondToApproval {
            approval_id,
            decision,
            note,
        } => {
            let approval_id = approval_id.parse::<Uuid>()?;
            dispatch_approval_decision(
                service,
                authenticated_session_id,
                approval_id,
                claude_runner::ApprovalDecisionRequest {
                    decision: runner_approval_decision(decision),
                    responder: Some("remote-code-quic".into()),
                    note,
                },
            )
            .await?;
            Ok(CommandAck {
                accepted: true,
                message: "queued".into(),
            })
        }
    }
}

async fn dispatch_runner_session_command(
    service: &ControlPlaneService,
    session_id: Uuid,
    request: claude_runner::RunnerSessionCommandRequest,
) -> anyhow::Result<()> {
    // Look up session → runner → enqueue.
    let runner_id = {
        let registry = service.registry.read().await;
        let session = registry
            .get_session(session_id)
            .map_err(|e: ApiError| anyhow::anyhow!("{}", e.message))?;
        session
            .owner_runner_id
            .ok_or_else(|| anyhow::anyhow!("session `{session_id}` is not assigned to a runner"))?
    };

    let runner = {
        let registry = service.registry.read().await;
        registry
            .runners
            .get(&runner_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("runner `{runner_id}` was not found"))?
    };

    if helpers::runner_uses_pull_commands(&runner) {
        let body = crate::types::RunnerQueuedCommandBody::SessionCommand {
            session_id,
            request,
        };
        let mut registry = service.registry.write().await;
        registry
            .enqueue_runner_command(&runner_id, body)
            .map_err(|e: ApiError| anyhow::anyhow!("{}", e.message))?;
    } else {
        helpers::dispatch_session_command_to_runner(
            &service.http_client,
            &runner,
            session_id,
            &request,
        )
        .await
        .map_err(|e: ApiError| {
            anyhow::anyhow!(
                "runner command relay failed for session {session_id}: {}",
                e.message
            )
        })?;
    }

    if let Err(e) = service.persist_state().await {
        tracing::error!("Failed to persist control plane state: {e:#}");
    }
    tracing::debug!("QUIC command dispatched to runner {runner_id} for session {session_id}");
    Ok(())
}

async fn dispatch_approval_decision(
    service: &ControlPlaneService,
    authenticated_session_id: Option<Uuid>,
    approval_id: Uuid,
    request: claude_runner::ApprovalDecisionRequest,
) -> anyhow::Result<()> {
    let planned = {
        let registry = service.registry.read().await;
        let approval = registry
            .get_approval(approval_id)
            .map_err(|e: ApiError| anyhow::anyhow!("{}", e.message))?;
        if authenticated_session_id.is_some_and(|session_id| approval.session_id != session_id) {
            anyhow::bail!(
                "approval `{approval_id}` does not belong to the authenticated QUIC session"
            );
        }
        registry
            .plan_approval_decision(approval_id, request)
            .map_err(|e: ApiError| anyhow::anyhow!("{}", e.message))?
    };

    let queue_for_runner = planned
        .owner_runner
        .as_ref()
        .is_some_and(helpers::runner_uses_pull_commands);
    if let Some(runner) = planned.owner_runner.as_ref()
        && !queue_for_runner
    {
        let relay_request =
            decision_request_from_resolved_approval(approval_id, &planned.approval)?;
        let relayed = helpers::relay_approval_decision_to_runner(
            &service.http_client,
            runner,
            planned.approval.approval_id,
            &relay_request,
        )
        .await
        .map_err(|e: ApiError| anyhow::anyhow!("{}", e.message))?;
        if relayed.approval_id != planned.approval.approval_id {
            anyhow::bail!(
                "runner `{}` acknowledged approval decision for `{}` instead of `{}`",
                runner.registration.runner_id,
                relayed.approval_id,
                planned.approval.approval_id
            );
        }
        if relayed.state != planned.approval.state {
            anyhow::bail!(
                "runner `{}` returned approval state `{:?}` instead of `{:?}` for `{}`",
                runner.registration.runner_id,
                relayed.state,
                planned.approval.state,
                planned.approval.approval_id
            );
        }
    }

    let (approval, transition) = {
        let mut registry = service.registry.write().await;
        registry
            .commit_planned_approval_decision(planned)
            .map_err(|e: ApiError| anyhow::anyhow!("{}", e.message))?
    };

    let _event = service
        .publish_event(TimelineEventDraft {
            runner_id: (!approval.runner_id.is_empty()).then(|| approval.runner_id.clone()),
            session_id: Some(approval.session_id),
            detail: TimelineEventDetail::ApprovalResolved {
                approval_id: approval.approval_id,
                state: approval.state,
                responder: approval.responder.clone(),
            },
        })
        .await;
    if let Some(transition) = transition {
        let _event = service
            .publish_event(TimelineEventDraft {
                runner_id: transition.runner_id,
                session_id: Some(transition.session_id),
                detail: TimelineEventDetail::SessionStateChanged {
                    previous_state: transition.previous_state,
                    state: transition.state,
                },
            })
            .await;
    }

    if queue_for_runner {
        let body = crate::types::RunnerQueuedCommandBody::ApplyApprovalDecision {
            approval_id: approval.approval_id,
            request: decision_request_from_resolved_approval(approval.approval_id, &approval)?,
        };
        let mut registry = service.registry.write().await;
        registry
            .enqueue_runner_command(&approval.runner_id, body)
            .map_err(|e: ApiError| anyhow::anyhow!("{}", e.message))?;
    }

    if let Err(e) = service.persist_state().await {
        tracing::error!("Failed to persist control plane state: {e:#}");
    }
    tracing::debug!(
        "QUIC approval decision dispatched for approval {}",
        approval.approval_id
    );
    Ok(())
}

async fn send_quic_event(conn: &quinn::Connection, event: &TimelineEvent) -> anyhow::Result<()> {
    let mut stream = conn.open_uni().await?;
    let payload = TransportEvent {
        sequence: event.sequence,
        payload: serde_json::to_value(event)?,
    };
    write_len_prefixed_payload(&mut stream, &payload).await
}

fn runner_approval_decision(
    decision: TransportApprovalDecision,
) -> claude_runner::ApprovalDecision {
    match decision {
        TransportApprovalDecision::Approved => claude_runner::ApprovalDecision::Approved,
        TransportApprovalDecision::Denied => claude_runner::ApprovalDecision::Denied,
        TransportApprovalDecision::Cancelled => claude_runner::ApprovalDecision::Cancelled,
    }
}

fn decision_request_from_resolved_approval(
    approval_id: Uuid,
    approval: &claude_runner::ApprovalRequestRecord,
) -> anyhow::Result<claude_runner::ApprovalDecisionRequest> {
    let decision = match approval.state {
        claude_runner::ApprovalState::Approved => claude_runner::ApprovalDecision::Approved,
        claude_runner::ApprovalState::Denied => claude_runner::ApprovalDecision::Denied,
        claude_runner::ApprovalState::Cancelled => claude_runner::ApprovalDecision::Cancelled,
        claude_runner::ApprovalState::Pending => {
            anyhow::bail!("approval `{approval_id}` remained pending during decision dispatch")
        }
    };
    Ok(claude_runner::ApprovalDecisionRequest {
        decision,
        responder: approval.responder.clone(),
        note: approval.note.clone(),
    })
}

async fn read_len_prefixed_payload(stream: &mut quinn::RecvStream) -> anyhow::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_QUIC_FRAME_BYTES {
        anyhow::bail!("frame too large: {len} bytes");
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    Ok(buf)
}

async fn write_len_prefixed_payload<T: serde::Serialize>(
    stream: &mut quinn::SendStream,
    payload: &T,
) -> anyhow::Result<()> {
    let buf = serde_json::to_vec(payload)?;
    if buf.len() > MAX_QUIC_FRAME_BYTES {
        anyhow::bail!("frame too large: {} bytes", buf.len());
    }
    stream.write_all(&(buf.len() as u32).to_le_bytes()).await?;
    stream.write_all(&buf).await?;
    stream.shutdown().await?;
    Ok(())
}

fn build_quic_server_tls(
    cert_pem: &[u8],
    key_pem: &[u8],
) -> anyhow::Result<quinn::crypto::rustls::QuicServerConfig> {
    let cert = rustls_pemfile::certs(&mut std::io::Cursor::new(cert_pem))
        .collect::<Result<Vec<_>, _>>()?;
    let key = rustls_pemfile::private_key(&mut std::io::Cursor::new(key_pem))
        .map_err(|e| anyhow::anyhow!("read private key: {e}"))?
        .ok_or_else(|| anyhow::anyhow!("no private key found"))?;

    let mut tls_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert, key)
        .map_err(|e| anyhow::anyhow!("TLS config: {e}"))?;
    tls_config.alpn_protocols = vec![b"rc-quic/1".to_vec()];

    quinn::crypto::rustls::QuicServerConfig::try_from(Arc::new(tls_config))
        .map_err(|e| anyhow::anyhow!("QUIC server TLS: {e}"))
}

#[derive(serde::Deserialize)]
struct QuicAuthMessage {
    token: String,
    session_id: String,
}

/// Constant-time comparison for auth tokens.
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut result = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        result |= x ^ y;
    }
    result == 0
}
