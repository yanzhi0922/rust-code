use axum::body::Body;
use axum::http::{
    Method, Request, StatusCode,
    header::{ACCESS_CONTROL_REQUEST_METHOD, AUTHORIZATION, ORIGIN},
};
use claude_control_plane::{
    BootstrapClaimRequest, BootstrapClaimResponse, ControlPlaneConfig, ControlPlaneHealth,
    ControlPlaneService, PairingAcceptRequest, PairingAcceptResponse, PairingOfferCreateRequest,
    PairingOfferCreateResponse, PushTokenRegistrationRequest, PushTokenRegistrationResponse,
    TrustedDeviceRecord,
};
use claude_runner::ListResponse;
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
        public_base_url: Some("https://remote.example.com".to_owned()),
        service_name: "test-control-plane".to_owned(),
        runner_lease_ttl_secs: 30,
        profile_dir: root.clone(),
        state_db_path: root.join("state.sqlite3"),
        artifact_root_dir,
        auth_token: None,
        bootstrap_secret: Some("bootstrap-secret".to_owned()),
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
async fn bootstrap_claim_mints_device_token_and_locks_protected_routes() {
    let app = ControlPlaneService::new(test_config(), "test-version").router();

    let health_response = app
        .clone()
        .oneshot(
            Request::get("/healthz")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("health request should succeed");
    assert_eq!(health_response.status(), StatusCode::OK);
    let health: ControlPlaneHealth = read_json(health_response).await;
    assert!(health.auth_required);
    assert!(!health.owner_claimed);
    assert_eq!(health.device_count, 0);

    let unauthorized = app
        .clone()
        .oneshot(
            Request::get("/v1/devices")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("request should complete");
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let bootstrap_response = app
        .clone()
        .oneshot(
            Request::post("/v1/bootstrap/claim")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&BootstrapClaimRequest {
                        bootstrap_secret: "bootstrap-secret".to_owned(),
                        device_name: "Owner laptop".to_owned(),
                        device_kind: claude_control_plane::DeviceKind::Cli,
                    })
                    .expect("request should serialize"),
                ))
                .expect("request should build"),
        )
        .await
        .expect("bootstrap request should succeed");
    assert_eq!(bootstrap_response.status(), StatusCode::CREATED);
    let bootstrap: BootstrapClaimResponse = read_json(bootstrap_response).await;
    assert!(bootstrap.device.owner);
    assert_eq!(bootstrap.device.name, "Owner laptop");
    assert!(!bootstrap.access_token.trim().is_empty());

    let devices_response = app
        .oneshot(
            Request::get("/v1/devices")
                .header(AUTHORIZATION, format!("Bearer {}", bootstrap.access_token))
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("devices request should succeed");
    assert_eq!(devices_response.status(), StatusCode::OK);
    let devices: ListResponse<TrustedDeviceRecord> = read_json(devices_response).await;
    assert_eq!(devices.items.len(), 1);
    assert!(devices.items[0].owner);
}

#[tokio::test]
async fn pairing_offer_accept_adds_second_device_and_persists_across_restart() {
    let config = test_config();
    let app = ControlPlaneService::new(config.clone(), "test-version").router();

    let bootstrap_response = app
        .clone()
        .oneshot(
            Request::post("/v1/bootstrap/claim")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&BootstrapClaimRequest {
                        bootstrap_secret: "bootstrap-secret".to_owned(),
                        device_name: "Owner laptop".to_owned(),
                        device_kind: claude_control_plane::DeviceKind::Cli,
                    })
                    .expect("request should serialize"),
                ))
                .expect("request should build"),
        )
        .await
        .expect("bootstrap request should succeed");
    let bootstrap: BootstrapClaimResponse = read_json(bootstrap_response).await;

    let offer_response = app
        .clone()
        .oneshot(
            Request::post("/v1/pairing/offers")
                .header(AUTHORIZATION, format!("Bearer {}", bootstrap.access_token))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&PairingOfferCreateRequest {
                        device_name: "iPhone".to_owned(),
                        device_kind: claude_control_plane::DeviceKind::Browser,
                        expires_in_secs: Some(300),
                    })
                    .expect("request should serialize"),
                ))
                .expect("request should build"),
        )
        .await
        .expect("offer request should succeed");
    assert_eq!(offer_response.status(), StatusCode::CREATED);
    let offer: PairingOfferCreateResponse = read_json(offer_response).await;
    assert_eq!(offer.device_name, "iPhone");
    assert!(
        offer
            .pairing_url
            .as_deref()
            .is_some_and(|url| url.contains("pairing_offer="))
    );

    let accept_response = app
        .clone()
        .oneshot(
            Request::post("/v1/pairing/accept")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&PairingAcceptRequest {
                        offer_id: offer.offer_id,
                        pairing_secret: offer.pairing_secret.clone(),
                        device_name: Some("Travel phone".to_owned()),
                        device_kind: Some(claude_control_plane::DeviceKind::Browser),
                    })
                    .expect("request should serialize"),
                ))
                .expect("request should build"),
        )
        .await
        .expect("pairing accept should succeed");
    assert_eq!(accept_response.status(), StatusCode::CREATED);
    let accepted: PairingAcceptResponse = read_json(accept_response).await;
    assert_eq!(accepted.device.name, "Travel phone");
    assert!(!accepted.device.owner);

    let restarted = ControlPlaneService::new(config, "test-version").router();
    let meta_response = restarted
        .clone()
        .oneshot(
            Request::get("/v1/meta")
                .header(AUTHORIZATION, format!("Bearer {}", accepted.access_token))
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("meta request should succeed");
    assert_eq!(meta_response.status(), StatusCode::OK);

    let devices_response = restarted
        .oneshot(
            Request::get("/v1/devices")
                .header(AUTHORIZATION, format!("Bearer {}", bootstrap.access_token))
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("devices request should succeed");
    assert_eq!(devices_response.status(), StatusCode::OK);
    let devices: ListResponse<TrustedDeviceRecord> = read_json(devices_response).await;
    assert_eq!(devices.items.len(), 2);
    assert!(devices.items.iter().any(|device| device.owner));
    assert!(
        devices
            .items
            .iter()
            .any(|device| device.name == "Travel phone")
    );
}

#[tokio::test]
async fn cors_preflight_only_allows_trusted_origins() {
    let app = ControlPlaneService::new(test_config(), "test-version").router();

    let trusted_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::OPTIONS)
                .uri("/v1/devices")
                .header(ORIGIN, "https://remote.example.com")
                .header(ACCESS_CONTROL_REQUEST_METHOD, "GET")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("preflight should succeed");

    assert_eq!(trusted_response.status(), StatusCode::OK);
    assert_eq!(
        trusted_response
            .headers()
            .get("access-control-allow-origin")
            .expect("allow origin header should exist"),
        "https://remote.example.com"
    );

    let untrusted_response = app
        .oneshot(
            Request::builder()
                .method(Method::OPTIONS)
                .uri("/v1/devices")
                .header(ORIGIN, "https://evil.example.com")
                .header(ACCESS_CONTROL_REQUEST_METHOD, "GET")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("preflight should succeed");

    assert_eq!(untrusted_response.status(), StatusCode::OK);
    assert!(
        untrusted_response
            .headers()
            .get("access-control-allow-origin")
            .is_none()
    );
}

#[tokio::test]
async fn push_token_registration_succeeds_for_authenticated_device() {
    let app = ControlPlaneService::new(test_config(), "test-version").router();

    // Bootstrap a device first
    let bootstrap_response = app
        .clone()
        .oneshot(
            Request::post("/v1/bootstrap/claim")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&BootstrapClaimRequest {
                        bootstrap_secret: "bootstrap-secret".to_owned(),
                        device_name: "Mobile phone".to_owned(),
                        device_kind: claude_control_plane::DeviceKind::Cli,
                    })
                    .expect("request should serialize"),
                ))
                .expect("request should build"),
        )
        .await
        .expect("bootstrap request should succeed");
    let bootstrap: BootstrapClaimResponse = read_json(bootstrap_response).await;
    let token = bootstrap.access_token;

    // Register push token without auth → should fail
    let unauthorized = app
        .clone()
        .oneshot(
            Request::post("/v1/devices/push-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&PushTokenRegistrationRequest {
                        push_token: "fcm-test-token-123".to_owned(),
                        platform: claude_control_plane::PushPlatform::Fcm,
                    })
                    .expect("request should serialize"),
                ))
                .expect("request should build"),
        )
        .await
        .expect("request should complete");
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    // Register push token with auth → should succeed
    let register_response = app
        .clone()
        .oneshot(
            Request::post("/v1/devices/push-token")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {}", token))
                .body(Body::from(
                    serde_json::to_vec(&PushTokenRegistrationRequest {
                        push_token: "fcm-test-token-123".to_owned(),
                        platform: claude_control_plane::PushPlatform::Fcm,
                    })
                    .expect("request should serialize"),
                ))
                .expect("request should build"),
        )
        .await
        .expect("push token request should succeed");
    assert_eq!(register_response.status(), StatusCode::OK);
    let result: PushTokenRegistrationResponse = read_json(register_response).await;
    assert!(result.registered);

    // Register with empty token → should fail
    let empty_response = app
        .clone()
        .oneshot(
            Request::post("/v1/devices/push-token")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {}", token))
                .body(Body::from(
                    serde_json::to_vec(&PushTokenRegistrationRequest {
                        push_token: "  ".to_owned(),
                        platform: claude_control_plane::PushPlatform::Apns,
                    })
                    .expect("request should serialize"),
                ))
                .expect("request should build"),
        )
        .await
        .expect("empty token request should complete");
    assert_eq!(empty_response.status(), StatusCode::BAD_REQUEST);

    // Update with a new token → should succeed
    let update_response = app
        .oneshot(
            Request::post("/v1/devices/push-token")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {}", token))
                .body(Body::from(
                    serde_json::to_vec(&PushTokenRegistrationRequest {
                        push_token: "apns-updated-token-456".to_owned(),
                        platform: claude_control_plane::PushPlatform::Apns,
                    })
                    .expect("request should serialize"),
                ))
                .expect("request should build"),
        )
        .await
        .expect("push token update should succeed");
    assert_eq!(update_response.status(), StatusCode::OK);
    let update_result: PushTokenRegistrationResponse = read_json(update_response).await;
    assert!(update_result.registered);
}
