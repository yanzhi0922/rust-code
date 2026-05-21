//! Authentication middleware, CORS configuration, and token validation.

use axum::extract::{Request, State};
use axum::http::{
    HeaderValue, Method,
    header::{AUTHORIZATION, CONTENT_DISPOSITION, CONTENT_TYPE},
};
use axum::middleware::Next;
use axum::response::IntoResponse;
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::AuthPrincipal;
use crate::state::ControlPlaneService;
use crate::types::ApiError;

// ---------------------------------------------------------------------------
// CORS
// ---------------------------------------------------------------------------

pub(crate) fn build_cors_layer(public_base_url: Option<&str>) -> CorsLayer {
    let policy = CorsOriginPolicy::from_env(public_base_url);

    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(
            move |origin: &HeaderValue, _request_parts: &axum::http::request::Parts| {
                policy.allows(origin)
            },
        ))
        .allow_methods([Method::GET, Method::POST, Method::DELETE, Method::OPTIONS])
        .allow_headers([CONTENT_TYPE, AUTHORIZATION, axum::http::header::ACCEPT])
        .expose_headers([CONTENT_DISPOSITION, CONTENT_TYPE])
}

#[derive(Clone, Debug)]
struct CorsOriginPolicy {
    exact_origins: Vec<HeaderValue>,
    allow_http_loopback: bool,
    allow_https_loopback: bool,
}

impl CorsOriginPolicy {
    fn from_env(public_base_url: Option<&str>) -> Self {
        let mut policy = Self {
            exact_origins: Vec::new(),
            allow_http_loopback: true,
            allow_https_loopback: false,
        };

        if let Some(origin) = public_base_url.and_then(origin_from_url) {
            policy.add_exact_origin(&origin);
        }

        // The desktop WebView uses a non-HTTP origin.  It still needs a bearer
        // token; CORS just decides whether browser-style clients may send it.
        policy.add_exact_origin("tauri://localhost");
        policy.add_exact_origin("http://tauri.localhost");

        if let Ok(raw_origins) = std::env::var("REMOTE_CODE_CORS_ORIGINS") {
            policy.apply_origin_list(&raw_origins);
        }

        policy
    }

    fn apply_origin_list(&mut self, raw_origins: &str) {
        self.exact_origins.clear();
        self.allow_http_loopback = false;
        self.allow_https_loopback = false;

        for origin in raw_origins.split(',').map(str::trim) {
            if origin.is_empty() {
                continue;
            }
            match origin {
                "*" => tracing::warn!(
                    "Ignoring REMOTE_CODE_CORS_ORIGINS=*; configure explicit trusted origins"
                ),
                "http://localhost:*" | "http://127.0.0.1:*" | "http://[::1]:*" => {
                    self.allow_http_loopback = true;
                }
                "https://localhost:*" | "https://127.0.0.1:*" | "https://[::1]:*" => {
                    self.allow_https_loopback = true;
                }
                _ => self.add_exact_origin(origin),
            }
        }
    }

    fn add_exact_origin(&mut self, origin: &str) {
        match origin.parse::<HeaderValue>() {
            Ok(header) if !self.exact_origins.contains(&header) => {
                self.exact_origins.push(header);
            }
            Ok(_) => {}
            Err(_) => tracing::warn!(origin, "Ignoring invalid CORS origin"),
        }
    }

    fn allows(&self, origin: &HeaderValue) -> bool {
        if self.exact_origins.iter().any(|allowed| allowed == origin) {
            return true;
        }

        let Ok(raw_origin) = origin.to_str() else {
            return false;
        };
        is_allowed_loopback_origin(
            raw_origin,
            self.allow_http_loopback,
            self.allow_https_loopback,
        )
    }
}

fn origin_from_url(raw_url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(raw_url).ok()?;
    let host = parsed.host_str()?;
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    Some(match parsed.port() {
        Some(port) => format!("{}://{}:{port}", parsed.scheme(), host),
        None => format!("{}://{}", parsed.scheme(), host),
    })
}

fn is_allowed_loopback_origin(raw_origin: &str, allow_http: bool, allow_https: bool) -> bool {
    let Ok(parsed) = reqwest::Url::parse(raw_origin) else {
        return false;
    };

    match parsed.scheme() {
        "http" if allow_http => {}
        "https" if allow_https => {}
        _ => return false,
    }

    match parsed.host_str() {
        Some("localhost") => true,
        Some(host) => host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback()),
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Secret hashing
// ---------------------------------------------------------------------------

pub(crate) fn hash_secret_value(raw: &str) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        #[allow(clippy::format_push_string)]
        encoded.push_str(&format!("{byte:02x}"));
    }
    encoded
}

// ---------------------------------------------------------------------------
// Auth middleware
// ---------------------------------------------------------------------------

pub(crate) async fn require_api_auth(
    State(service): State<ControlPlaneService>,
    mut request: Request,
    next: Next,
) -> axum::response::Response {
    // Explicit disable via env var (local dev only — compiled out in release).
    if cfg!(debug_assertions)
        && std::env::var("REMOTE_CODE_REQUIRE_AUTH")
            .as_deref()
            .is_ok_and(|v| v.eq_ignore_ascii_case("false"))
    {
        return next.run(request).await;
    }

    // Bootstrap mode: no shared token, no bootstrap secret, AND no trusted
    // devices registered — allow open access.  This covers fresh instances
    // that have no auth configuration at all (e.g. unit tests).
    // When a bootstrap_secret IS configured, the instance is in "waiting for
    // owner claim" mode and protected routes still require auth.
    // IMPORTANT: drop the read lock before calling next.run() to avoid
    // deadlocking with handlers that need a write lock on the registry.
    if service.auth_token.is_none() && service.bootstrap_secret_hash.is_none() {
        let is_empty = {
            let registry = service.registry.read().await;
            registry.trusted_devices.is_empty()
        };
        if is_empty {
            request.extensions_mut().insert(AuthPrincipal::SharedToken);
            return next.run(request).await;
        }
    }

    if let Some(principal) = consume_stream_ticket(&service, &mut request).await {
        request.extensions_mut().insert(principal);
        return next.run(request).await;
    }

    let Some(provided) = extract_request_auth_token(&mut request) else {
        return ApiError::unauthorized("missing or invalid control plane bearer token".to_owned())
            .into_response();
    };

    if service
        .auth_token
        .as_deref()
        .is_some_and(|expected| constant_time_token_eq(&provided, expected))
    {
        request.extensions_mut().insert(AuthPrincipal::SharedToken);
        return next.run(request).await;
    }

    let authenticated_device = {
        let mut registry = service.registry.write().await;
        registry.authenticate_device_token(&provided)
    };
    if let Some((device, is_access_token)) = authenticated_device {
        if !is_access_token {
            return ApiError::unauthorized(
                "refresh tokens must be exchanged at /v1/auth/refresh before calling protected APIs"
                    .to_owned(),
            )
            .into_response();
        }
        request
            .extensions_mut()
            .insert(AuthPrincipal::Device(device));
        return next.run(request).await;
    }

    if request_allows_tenant_user_auth(&request) && service.accepts_derived_user_key(&provided) {
        request
            .extensions_mut()
            .insert(AuthPrincipal::User { user_id: provided });
        return next.run(request).await;
    }

    ApiError::unauthorized("missing or invalid control plane bearer token".to_owned())
        .into_response()
}

async fn consume_stream_ticket(
    service: &ControlPlaneService,
    request: &mut Request,
) -> Option<AuthPrincipal> {
    if !request_allows_query_auth(request) {
        return None;
    }
    let ticket = request
        .uri()
        .query()
        .and_then(extract_stream_ticket_from_query)?;
    strip_auth_from_request_uri(request);
    service
        .consume_stream_ticket(&ticket, request.uri().path())
        .await
}

fn extract_request_auth_token(request: &mut Request) -> Option<String> {
    let bearer = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    if bearer.is_some() {
        return bearer;
    }
    if !legacy_query_access_tokens_enabled() || !request_allows_query_auth(request) {
        return None;
    }
    let token = request
        .uri()
        .query()
        .and_then(extract_auth_token_from_query);
    // Strip token from URI to prevent it from appearing in access logs
    if token.is_some() {
        strip_auth_from_request_uri(request);
    }
    token
}

fn request_allows_query_auth(request: &Request) -> bool {
    // Only allow query-string auth for WebSocket upgrade endpoints
    let is_stream_path = request.uri().path().ends_with("/stream");
    if !is_stream_path {
        return false;
    }
    // Must be a WebSocket upgrade or a normal GET (for SSE)
    let is_ws_upgrade = request.headers().get("upgrade").is_some_and(|v| {
        v.to_str()
            .is_ok_and(|v| v.eq_ignore_ascii_case("websocket"))
    });
    let is_get = request.method() == Method::GET;
    is_ws_upgrade || is_get
}

fn request_allows_tenant_user_auth(request: &Request) -> bool {
    let path = request.uri().path();
    let method = request.method();

    if path == "/v1/devices/push-token" && method == Method::POST {
        return true;
    }
    if path == "/v1/stream-ticket" && method == Method::POST {
        return true;
    }
    if path.starts_with("/v1/runners") {
        return true;
    }
    if path.starts_with("/v1/sessions") {
        return true;
    }
    if path.starts_with("/v1/approvals") {
        return true;
    }
    if path.starts_with("/v1/artifacts") {
        return true;
    }
    if path == "/v1/events" || path == "/v1/events/stream" {
        return true;
    }

    false
}

fn extract_auth_token_from_query(query: &str) -> Option<String> {
    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next()?.trim();
        let value = parts.next().unwrap_or_default().trim();
        if matches!(key, "token" | "access_token") && !value.is_empty() {
            return Some(percent_decode_query_value(value));
        }
    }
    None
}

fn extract_stream_ticket_from_query(query: &str) -> Option<String> {
    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next()?.trim();
        let value = parts.next().unwrap_or_default().trim();
        if key == "stream_ticket" && !value.is_empty() {
            return Some(percent_decode_query_value(value));
        }
    }
    None
}

fn percent_decode_query_value(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let high = bytes[index + 1] as char;
                let low = bytes[index + 2] as char;
                if let (Some(high), Some(low)) = (high.to_digit(16), low.to_digit(16)) {
                    decoded.push(((high << 4) | low) as u8);
                    index += 3;
                } else {
                    decoded.push(bytes[index]);
                    index += 1;
                }
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// Strip auth query parameters from the request URI
/// so they don't appear in access logs or error messages.
fn strip_auth_from_request_uri(request: &mut Request) {
    let uri = request.uri().clone();
    let Some(query) = uri.query() else {
        return;
    };
    let cleaned: String = query
        .split('&')
        .filter(|pair| {
            let key = pair.split('=').next().unwrap_or("").trim();
            !matches!(key, "token" | "access_token" | "stream_ticket")
        })
        .collect::<Vec<_>>()
        .join("&");
    let new_uri = if cleaned.is_empty() {
        uri.path().to_owned()
    } else {
        format!("{}?{cleaned}", uri.path())
    };
    if let Ok(parsed) = new_uri.parse::<axum::http::Uri>() {
        *request.uri_mut() = parsed;
    }
}

fn legacy_query_access_tokens_enabled() -> bool {
    std::env::var("REMOTE_CODE_ALLOW_QUERY_ACCESS_TOKEN")
        .as_deref()
        .is_ok_and(|value| value.eq_ignore_ascii_case("true"))
}

fn constant_time_token_eq(provided: &str, expected: &str) -> bool {
    use sha2::{Digest, Sha256};

    let provided_digest: [u8; 32] = Sha256::digest(provided.as_bytes()).into();
    let expected_digest: [u8; 32] = Sha256::digest(expected.as_bytes()).into();
    constant_time_eq::constant_time_eq_32(&provided_digest, &expected_digest)
}
