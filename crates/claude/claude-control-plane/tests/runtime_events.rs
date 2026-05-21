use std::collections::BTreeMap;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use claude_control_plane::{
    ControlPlaneConfig, ControlPlaneService, CreateSessionRequest, MessageRole,
    RuntimeEventCreateRequest, RuntimeEventDetail, SessionRecord, TimelineEvent,
    TimelineEventDetail,
};
use serde::de::DeserializeOwned;
use tempfile::tempdir;
use tower::ServiceExt;

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
        auth_token: None,
        bootstrap_secret: None,
        downloads_dir: None,
        quic_bind: None,
        quic_cert_pem: None,
        quic_key_pem: None,
    }
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

#[tokio::test]
async fn runtime_session_event_endpoint_publishes_message_events() {
    let app = ControlPlaneService::new(test_config(), "test-version").router();

    let create_response = app
        .clone()
        .oneshot(
            Request::post("/v1/sessions")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&CreateSessionRequest {
                        session_id: None,
                        workspace_id: "workspace-alpha".to_owned(),
                        preferred_runner_id: None,
                        metadata: BTreeMap::new(),
                    })
                    .expect("request should serialize"),
                ))
                .expect("request should build"),
        )
        .await
        .expect("session request should succeed");
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let session: SessionRecord = read_json(create_response).await;

    let publish_response = app
        .clone()
        .oneshot(
            Request::post(format!("/v1/sessions/{}/events", session.session_id))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&RuntimeEventCreateRequest {
                        detail: RuntimeEventDetail::MessageCommitted {
                            role: MessageRole::Assistant,
                            text: "remote timeline message".to_owned(),
                            message_id: Some("msg-1".to_owned()),
                        },
                    })
                    .expect("request should serialize"),
                ))
                .expect("request should build"),
        )
        .await
        .expect("runtime event request should succeed");
    assert_eq!(publish_response.status(), StatusCode::CREATED);
    let event: TimelineEvent = read_json(publish_response).await;
    match event.detail {
        TimelineEventDetail::MessageCommitted {
            role,
            text,
            message_id,
        } => {
            assert_eq!(role, MessageRole::Assistant);
            assert_eq!(text, "remote timeline message");
            assert_eq!(message_id.as_deref(), Some("msg-1"));
        }
        other => panic!("unexpected event detail: {other:?}"),
    }

    let list_response = app
        .oneshot(
            Request::get(format!(
                "/v1/sessions/{}/events?kind=message_committed&limit=10",
                session.session_id
            ))
            .body(Body::empty())
            .expect("request should build"),
        )
        .await
        .expect("events request should succeed");
    assert_eq!(list_response.status(), StatusCode::OK);
    let response: claude_runner::ListResponse<TimelineEvent> = read_json(list_response).await;
    assert_eq!(response.items.len(), 1);
    assert!(matches!(
        response.items[0].detail,
        TimelineEventDetail::MessageCommitted { .. }
    ));
}

#[tokio::test]
async fn runtime_tool_progress_requires_identity_and_payload() {
    let app = ControlPlaneService::new(test_config(), "test-version").router();

    let create_response = app
        .clone()
        .oneshot(
            Request::post("/v1/sessions")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&CreateSessionRequest {
                        session_id: None,
                        workspace_id: "workspace-beta".to_owned(),
                        preferred_runner_id: None,
                        metadata: BTreeMap::new(),
                    })
                    .expect("request should serialize"),
                ))
                .expect("request should build"),
        )
        .await
        .expect("session request should succeed");
    let session: SessionRecord = read_json(create_response).await;

    let publish_response = app
        .oneshot(
            Request::post(format!("/v1/sessions/{}/events", session.session_id))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&RuntimeEventCreateRequest {
                        detail: RuntimeEventDetail::ToolProgress {
                            tool_call_id: None,
                            tool_name: None,
                            delta: None,
                            elapsed_time_seconds: None,
                        },
                    })
                    .expect("request should serialize"),
                ))
                .expect("request should build"),
        )
        .await
        .expect("runtime event request should succeed");
    assert_eq!(publish_response.status(), StatusCode::BAD_REQUEST);
}
