use std::collections::BTreeMap;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::{Request, StatusCode};
use claude_control_plane::quic::{QuicServerConfig, start_quic_listener};
use claude_control_plane::{
    ControlPlaneConfig, ControlPlaneService, CreateSessionRequest, RunnerQueuedCommandBody,
    RuntimeEventCreateRequest, RuntimeEventDetail, SessionRecord, TimelineEvent,
    TimelineEventDetail,
};
use claude_runner::{
    ApprovalCreateRequest, ApprovalRequestRecord, RunnerCapabilities, RunnerPlatform,
    RunnerRegistrationRequest, RunnerSessionCommandRequest, RunnerWorkspace,
};
use rc_remote_transport::{
    QuicTransport, ReconnectPolicy, RemoteTransport, TlsConfig, TransportApprovalDecision,
    TransportCommand, TransportConfig, TransportStrategy,
};
use rcgen::generate_simple_self_signed;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use tempfile::tempdir;
use tokio::time::timeout;
use tower::ServiceExt;

const AUTH_TOKEN: &str = "quic-test-token";

fn test_config() -> ControlPlaneConfig {
    let dir = tempdir().expect("tempdir should succeed");
    let root = dir.keep();
    let artifact_root_dir = root.join("artifacts");
    std::fs::create_dir_all(&artifact_root_dir).expect("artifact dir should exist");
    ControlPlaneConfig {
        bind: "127.0.0.1:0".parse().expect("bind should parse"),
        public_base_url: None,
        service_name: "test-control-plane".to_owned(),
        runner_lease_ttl_secs: 30,
        profile_dir: root.clone(),
        state_db_path: root.join("state.sqlite3"),
        artifact_root_dir,
        auth_token: Some(AUTH_TOKEN.to_owned()),
        bootstrap_secret: None,
        downloads_dir: None,
        quic_bind: None,
        quic_cert_pem: None,
        quic_key_pem: None,
    }
}

fn runner_registration() -> RunnerRegistrationRequest {
    RunnerRegistrationRequest {
        runner_id: "runner-quic".to_owned(),
        control_plane_url: Some("http://127.0.0.1:8787".to_owned()),
        public_base_url: None,
        workspaces: vec![RunnerWorkspace {
            workspace_id: "default".to_owned(),
            root_dir: "C:/workspace/quic".into(),
            writable: true,
        }],
        labels: BTreeMap::new(),
        auth_token: None,
        capabilities: RunnerCapabilities {
            interactive_approvals: true,
            background_sessions: true,
            artifact_uploads: true,
            max_parallel_sessions: 4,
        },
        platform: RunnerPlatform {
            os: "windows".to_owned(),
            arch: "x86_64".to_owned(),
            family: "windows".to_owned(),
        },
    }
}

fn unused_udp_addr() -> SocketAddr {
    let socket = UdpSocket::bind("127.0.0.1:0").expect("UDP port should bind");
    socket.local_addr().expect("local UDP address should exist")
}

fn test_quic_cert() -> (Vec<u8>, Vec<u8>, String) {
    let material = generate_simple_self_signed(["localhost".to_owned(), "127.0.0.1".to_owned()])
        .expect("test QUIC certificate should generate");
    let cert_pem = material.cert.pem();
    let key_pem = material.signing_key.serialize_pem();
    let cert = rustls_pemfile::certs(&mut std::io::Cursor::new(&cert_pem))
        .next()
        .expect("test cert should exist")
        .expect("test cert should parse");
    let fingerprint = hex_sha256(cert.as_ref());
    (cert_pem.into_bytes(), key_pem.into_bytes(), fingerprint)
}

fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

async fn read_json<T>(response: axum::response::Response) -> T
where
    T: DeserializeOwned,
{
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should decode");
    serde_json::from_slice(&bytes).expect("json should decode")
}

async fn post_json<T: serde::Serialize>(
    app: axum::Router,
    uri: String,
    body: &T,
) -> axum::response::Response {
    app.oneshot(
        Request::post(uri)
            .header(CONTENT_TYPE, "application/json")
            .header(AUTHORIZATION, format!("Bearer {AUTH_TOKEN}"))
            .body(Body::from(
                serde_json::to_vec(body).expect("request body should serialize"),
            ))
            .expect("request should build"),
    )
    .await
    .expect("request should complete")
}

#[tokio::test]
async fn quic_transport_covers_event_prompt_and_approval_e2e() {
    let service = ControlPlaneService::new(test_config(), "0.1.0");
    let app = service.clone().router();

    let register_response = post_json(
        app.clone(),
        "/v1/runners/register".to_owned(),
        &runner_registration(),
    )
    .await;
    assert_eq!(register_response.status(), StatusCode::OK);

    let create_response = post_json(
        app.clone(),
        "/v1/sessions".to_owned(),
        &CreateSessionRequest {
            session_id: None,
            workspace_id: "default".to_owned(),
            preferred_runner_id: Some("runner-quic".to_owned()),
            metadata: BTreeMap::new(),
        },
    )
    .await;
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let session: SessionRecord = read_json(create_response).await;

    let (cert_pem, key_pem, fingerprint) = test_quic_cert();
    let quic_addr = unused_udp_addr();
    let quic_task = tokio::spawn(start_quic_listener(
        Arc::new(service.clone()),
        QuicServerConfig {
            listen_addr: quic_addr,
            cert_pem,
            key_pem,
        },
    ));

    let mut transport = QuicTransport::new(ReconnectPolicy::default());
    timeout(
        Duration::from_secs(5),
        transport.connect(TransportConfig {
            strategy: TransportStrategy::Quic {
                server_url: format!("quic://{quic_addr}"),
                server_cert_fingerprint: Some(fingerprint.clone()),
            },
            auth_token: AUTH_TOKEN.to_owned(),
            session_id: session.session_id.to_string(),
            after_sequence: 0,
            tls: TlsConfig {
                accept_self_signed: true,
                cert_fingerprints: vec![fingerprint],
                enforce_https: false,
            },
            reconnect: ReconnectPolicy::default(),
        }),
    )
    .await
    .expect("QUIC connect should not time out")
    .expect("QUIC connect should succeed");

    let mut events = transport
        .take_event_receiver()
        .expect("QUIC event receiver should exist");
    tokio::time::sleep(Duration::from_millis(100)).await;

    let event_response = post_json(
        app.clone(),
        format!("/v1/sessions/{}/events", session.session_id),
        &RuntimeEventCreateRequest {
            detail: RuntimeEventDetail::MessageCommitted {
                role: claude_control_plane::MessageRole::Assistant,
                text: "hello over quic".to_owned(),
                message_id: Some("msg-quic".to_owned()),
            },
        },
    )
    .await;
    assert_eq!(event_response.status(), StatusCode::CREATED);

    let transport_event = timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("QUIC event should arrive before timeout")
        .expect("QUIC event stream should stay open");
    let timeline_event: TimelineEvent =
        serde_json::from_value(transport_event.payload).expect("timeline event should decode");
    assert_eq!(transport_event.sequence, timeline_event.sequence);
    assert_eq!(timeline_event.session_id, Some(session.session_id));
    assert!(matches!(
        timeline_event.detail,
        TimelineEventDetail::MessageCommitted { .. }
    ));

    let prompt_ack = transport
        .send_command(TransportCommand::SendPrompt {
            content: "ship via quic".to_owned(),
        })
        .await
        .expect("prompt command should send");
    assert!(prompt_ack.accepted, "{prompt_ack:?}");

    let approval_response = post_json(
        app.clone(),
        format!("/v1/sessions/{}/approvals", session.session_id),
        &ApprovalCreateRequest {
            approval_id: None,
            title: "Run gated command".to_owned(),
            description: "Approval over QUIC".to_owned(),
            metadata: BTreeMap::new(),
        },
    )
    .await;
    assert_eq!(approval_response.status(), StatusCode::CREATED);
    let approval: ApprovalRequestRecord = read_json(approval_response).await;

    let approval_ack = transport
        .send_command(TransportCommand::RespondToApproval {
            approval_id: approval.approval_id.to_string(),
            decision: TransportApprovalDecision::Approved,
            note: Some("approved over quic".to_owned()),
        })
        .await
        .expect("approval command should send");
    assert!(approval_ack.accepted, "{approval_ack:?}");

    let pull_response = app
        .clone()
        .oneshot(
            Request::post("/v1/runners/runner-quic/commands/pull?limit=10")
                .header(AUTHORIZATION, format!("Bearer {AUTH_TOKEN}"))
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("pull request should complete");
    assert_eq!(pull_response.status(), StatusCode::OK);
    let pulled: claude_control_plane::RunnerCommandPullResponse = read_json(pull_response).await;
    assert!(pulled.commands.iter().any(|command| {
        matches!(
            &command.body,
            RunnerQueuedCommandBody::SessionCommand {
                session_id,
                request: RunnerSessionCommandRequest::SendPrompt { content },
            } if *session_id == session.session_id && content == "ship via quic"
        )
    }));
    assert!(pulled.commands.iter().any(|command| {
        matches!(
            &command.body,
            RunnerQueuedCommandBody::ApplyApprovalDecision {
                approval_id,
                request,
            } if *approval_id == approval.approval_id
                && request.decision == claude_runner::ApprovalDecision::Approved
        )
    }));

    transport
        .disconnect()
        .await
        .expect("QUIC transport should disconnect");
    quic_task.abort();
    let _ = quic_task.await;
}
