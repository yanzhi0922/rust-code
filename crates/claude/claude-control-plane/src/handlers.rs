//! HTTP handler functions for the control plane axum router.

use std::collections::{BTreeSet, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use axum::Json;
use axum::extract::{Extension, Path as AxumPath, Query, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use chrono::Utc;
use claude_runner::{
    ApprovalCreateRequest, ApprovalDecision, ApprovalDecisionRequest, ApprovalState, ListResponse,
    RunnerHeartbeat, RunnerRegistrationRequest, RunnerSessionCommandRequest,
    RunnerSessionCommandResponse, RunnerSessionCreateRequest, RunnerSessionRecord,
    RunnerSessionStateUpdateRequest, RunnerSnapshot,
};
use uuid::Uuid;

use crate::helpers::{
    artifact_file_path, build_content_disposition, dispatch_session_command_to_runner,
    dispatch_session_to_runner, relay_approval_decision_to_runner, relay_approval_to_runner,
    runner_is_available, runner_uses_pull_commands, session_state_from_runner,
    session_state_to_runner, update_runner_session_state,
};
use crate::streams::{
    serve_filtered_event_stream, serve_runner_approval_stream, serve_runner_event_stream,
    serve_session_approval_stream, serve_session_event_stream,
};
use crate::types::{
    ApiError, ArtifactCreateRequest, ArtifactRecord, BootstrapClaimRequest, BootstrapClaimResponse,
    ControlPlaneHealth, ControlPlaneMeta, CreateSessionRequest, DEFAULT_EVENT_LIST_LIMIT,
    EventStreamQuery, ListSessionsQuery, MAX_ARTIFACT_SIZE_BYTES, PairingAcceptRequest,
    PairingAcceptResponse, PairingOfferCreateRequest, PairingOfferCreateResponse,
    PushTokenRegistrationRequest, PushTokenRegistrationResponse, RecentEventsQuery,
    RunnerCommandPullQuery, RunnerCommandPullResponse, RunnerQueuedCommandBody,
    RunnerRegistrationResponse, RuntimeEventCreateRequest, RuntimeEventDetail,
    STREAM_TICKET_TTL_SECS, SessionRecord, SessionState, SessionStateUpdateRequest, SessionView,
    StreamTicketRequest, StreamTicketResponse, TimelineEvent, TimelineEventDetail,
    TimelineEventDraft, TokenRefreshRequest, TokenRefreshResponse, TrustedDeviceRecord,
};
use crate::{AuthPrincipal, ControlPlaneService, PersistedEventQuery};

/// Persist control plane state, logging any errors instead of silently discarding them.
async fn persist_state_logged(service: &ControlPlaneService) {
    if let Err(e) = service.persist_state().await {
        tracing::error!("Failed to persist control plane state: {e:#}");
    }
}

/// Extract the tenant-scoping `owner_user_id` from the auth principal.
/// Returns `None` for `SharedToken` (admin — sees all) and `Device` (legacy).
fn user_id_from_principal(principal: &AuthPrincipal) -> Option<&str> {
    principal.user_id()
}

fn require_owner_or_shared(principal: &AuthPrincipal) -> Result<(), ApiError> {
    match principal {
        AuthPrincipal::SharedToken => Ok(()),
        AuthPrincipal::Device(device) if device.owner => Ok(()),
        _ => Err(ApiError::forbidden(
            "owner or shared-token access is required".to_owned(),
        )),
    }
}

/// Snapshot of ownership data for tenant-filtered event streams.
/// Pre-computed before entering the stream loop so the sync filter closure
/// doesn't need async lock access.
struct TenantFilter {
    runner_ids: HashSet<String>,
    session_ids: HashSet<Uuid>,
}

impl TenantFilter {
    /// Build a tenant filter snapshot.  Returns `None` for admin/legacy principals
    /// (no filtering needed).
    fn from_registry(registry: &crate::registry::Registry, user_id: Option<&str>) -> Option<Self> {
        let uid = user_id?;
        Some(Self {
            runner_ids: registry
                .runner_owners
                .iter()
                .filter(|(_, owner)| owner.as_str() == uid)
                .map(|(rid, _)| rid.clone())
                .collect(),
            session_ids: registry
                .sessions
                .iter()
                .filter(|(_, s)| s.owner_user_id.as_deref() == Some(uid))
                .map(|(id, _)| *id)
                .collect(),
        })
    }

    fn event_visible(&self, event: &TimelineEvent) -> bool {
        if let Some(ref rid) = event.runner_id {
            return self.runner_ids.contains(rid);
        }
        if let Some(sid) = event.session_id {
            return self.session_ids.contains(&sid);
        }
        false
    }
}

/// Filter a backlog of events by tenant visibility.
fn filter_events_by_tenant(
    events: Vec<TimelineEvent>,
    filter: &Option<TenantFilter>,
) -> Vec<TimelineEvent> {
    match filter {
        None => events,
        Some(f) => events.into_iter().filter(|e| f.event_visible(e)).collect(),
    }
}

// ---------------------------------------------------------------------------
// Health / meta
// ---------------------------------------------------------------------------

fn build_session_view(
    service: &ControlPlaneService,
    registry: &crate::registry::Registry,
    session: SessionRecord,
) -> SessionView {
    let (
        owner_runner_available,
        owner_runner_state,
        owner_runner_last_seen_at,
        owner_runner_public_base_url,
    ) = session
        .owner_runner_id
        .as_deref()
        .and_then(|runner_id| registry.runners.get(runner_id))
        .map(|runner| {
            (
                runner_is_available(runner, service.runner_lease_ttl_secs),
                Some(runner.state),
                Some(runner.last_seen_at),
                runner.registration.public_base_url.clone(),
            )
        })
        .unwrap_or((false, None, None, None));

    SessionView {
        session,
        owner_runner_available,
        owner_runner_state,
        owner_runner_last_seen_at,
        owner_runner_public_base_url,
    }
}

fn build_session_views(
    service: &ControlPlaneService,
    registry: &crate::registry::Registry,
    sessions: Vec<SessionRecord>,
) -> Vec<SessionView> {
    sessions
        .into_iter()
        .map(|session| build_session_view(service, registry, session))
        .collect()
}

pub(crate) async fn get_health(
    State(service): State<ControlPlaneService>,
) -> Json<ControlPlaneHealth> {
    let auth_required = service.auth_required().await;
    let registry = service.registry.read().await;
    let available_runner_count = registry
        .runners
        .values()
        .filter(|snapshot| runner_is_available(snapshot, service.runner_lease_ttl_secs))
        .count();

    Json(ControlPlaneHealth {
        ok: true,
        service: service.meta.service.clone(),
        phase: service.meta.phase.clone(),
        runner_count: registry.runners.len(),
        available_runner_count,
        session_count: registry.sessions.len(),
        artifact_count: registry.artifacts.len(),
        queued_runner_command_count: registry.queued_runner_command_count(),
        auth_required,
        bootstrap_secret_configured: service.bootstrap_secret_hash.is_some(),
        owner_claimed: registry.owner_claimed(),
        device_count: registry.trusted_device_count(),
    })
}

pub(crate) async fn get_meta(State(service): State<ControlPlaneService>) -> Json<ControlPlaneMeta> {
    Json(service.meta.clone())
}

fn build_pairing_url(base: &str, offer_id: Uuid, pairing_secret: &str) -> Option<String> {
    let mut url = reqwest::Url::parse(base).ok()?;
    url.query_pairs_mut()
        .append_pair("mode", "remote")
        .append_pair("pairing_offer", &offer_id.to_string())
        .append_pair("pairing_secret", pairing_secret);
    Some(url.to_string())
}

pub(crate) async fn claim_bootstrap_device(
    State(service): State<ControlPlaneService>,
    Json(request): Json<BootstrapClaimRequest>,
) -> Result<(StatusCode, Json<BootstrapClaimResponse>), ApiError> {
    let response = {
        let mut registry = service.registry.write().await;
        let result = registry.bootstrap_claim(service.bootstrap_secret_hash.as_deref(), request)?;
        BootstrapClaimResponse {
            device: result.device,
            access_token: result.access_token,
            refresh_token: result.refresh_token,
        }
    };
    persist_state_logged(&service).await;
    Ok((StatusCode::CREATED, Json(response)))
}

pub(crate) async fn list_devices(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
) -> Result<Json<ListResponse<TrustedDeviceRecord>>, ApiError> {
    require_owner_or_shared(&principal)?;
    let registry = service.registry.read().await;
    Ok(Json(ListResponse {
        items: registry.list_trusted_devices(),
        latest_sequence: None,
    }))
}

pub(crate) async fn revoke_device(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    AxumPath(device_id): AxumPath<Uuid>,
) -> Result<StatusCode, ApiError> {
    require_owner_or_shared(&principal)?;
    {
        let mut registry = service.registry.write().await;
        registry.revoke_device(device_id)?;
    }
    persist_state_logged(&service).await;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn create_pairing_offer(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    Json(request): Json<PairingOfferCreateRequest>,
) -> Result<(StatusCode, Json<PairingOfferCreateResponse>), ApiError> {
    require_owner_or_shared(&principal)?;
    let (offer, pairing_secret) = {
        let mut registry = service.registry.write().await;
        registry.create_pairing_offer(principal.created_by_device_id(), request)?
    };
    let pairing_url = service
        .meta
        .public_base_url
        .as_deref()
        .and_then(|base| build_pairing_url(base, offer.offer_id, &pairing_secret));
    let response = PairingOfferCreateResponse {
        offer_id: offer.offer_id,
        device_name: offer.device_name,
        device_kind: offer.device_kind,
        created_at: offer.created_at,
        expires_at: offer.expires_at,
        pairing_secret,
        pairing_url,
    };
    persist_state_logged(&service).await;
    Ok((StatusCode::CREATED, Json(response)))
}

pub(crate) async fn accept_pairing_offer(
    State(service): State<ControlPlaneService>,
    Json(request): Json<PairingAcceptRequest>,
) -> Result<(StatusCode, Json<PairingAcceptResponse>), ApiError> {
    let response = {
        let mut registry = service.registry.write().await;
        let result = registry.accept_pairing_offer(request)?;
        PairingAcceptResponse {
            device: result.device,
            access_token: result.access_token,
            refresh_token: result.refresh_token,
        }
    };
    persist_state_logged(&service).await;
    Ok((StatusCode::CREATED, Json(response)))
}

pub(crate) async fn refresh_token(
    State(service): State<ControlPlaneService>,
    Json(request): Json<TokenRefreshRequest>,
) -> Result<Json<TokenRefreshResponse>, ApiError> {
    let response = {
        let mut registry = service.registry.write().await;
        let (_device, access_token) = registry.refresh_access_token(&request.refresh_token)?;
        TokenRefreshResponse { access_token }
    };
    persist_state_logged(&service).await;
    Ok(Json(response))
}

pub(crate) async fn create_stream_ticket(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    Json(request): Json<StreamTicketRequest>,
) -> Result<Json<StreamTicketResponse>, ApiError> {
    let path = normalize_stream_ticket_path(&request.path)?;
    let stream_ticket = service
        .mint_stream_ticket(principal, path, STREAM_TICKET_TTL_SECS)
        .await;
    Ok(Json(StreamTicketResponse {
        stream_ticket,
        expires_in_secs: STREAM_TICKET_TTL_SECS,
    }))
}

fn normalize_stream_ticket_path(raw: &str) -> Result<String, ApiError> {
    let path = raw.trim();
    if !path.starts_with("/v1/")
        || !path.ends_with("/stream")
        || path.contains('?')
        || path.contains('#')
        || path.contains("://")
    {
        return Err(ApiError::bad_request(
            "stream ticket path must be a /v1/*/stream path without query parameters".to_owned(),
        ));
    }
    Ok(path.to_owned())
}

// ---------------------------------------------------------------------------
// Event handlers
// ---------------------------------------------------------------------------

pub(crate) async fn list_recent_events(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    Query(query): Query<RecentEventsQuery>,
) -> Result<Json<ListResponse<TimelineEvent>>, ApiError> {
    let user_id = user_id_from_principal(&principal);
    let tenant_filter = {
        let registry = service.registry.read().await;
        TenantFilter::from_registry(&registry, user_id)
    };
    let latest_sequence = service
        .latest_persisted_event_sequence(PersistedEventQuery {
            kind: query.kind,
            ..PersistedEventQuery::default()
        })
        .await
        .map_err(|error| ApiError::internal(format!("failed to read persisted events: {error}")))?;
    let items = service
        .list_persisted_events(PersistedEventQuery {
            after: query.after,
            limit: Some(query.limit.unwrap_or(DEFAULT_EVENT_LIST_LIMIT)),
            kind: query.kind,
            ..PersistedEventQuery::default()
        })
        .await
        .map_err(|error| ApiError::internal(format!("failed to read persisted events: {error}")))?;
    Ok(Json(ListResponse {
        items: filter_events_by_tenant(items, &tenant_filter),
        latest_sequence,
    }))
}

pub(crate) async fn list_session_events(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    AxumPath(session_id): AxumPath<Uuid>,
    Query(query): Query<RecentEventsQuery>,
) -> Result<Json<ListResponse<TimelineEvent>>, ApiError> {
    {
        let registry = service.registry.read().await;
        let user_id = user_id_from_principal(&principal);
        registry.get_session_for_user(session_id, user_id)?;
    }
    let latest_sequence = service
        .latest_persisted_event_sequence(PersistedEventQuery {
            kind: query.kind,
            session_id: Some(session_id),
            ..PersistedEventQuery::default()
        })
        .await;
    let latest_sequence = latest_sequence
        .map_err(|error| ApiError::internal(format!("failed to read session events: {error}")))?;
    let items = service
        .list_persisted_events(PersistedEventQuery {
            after: query.after,
            limit: Some(query.limit.unwrap_or(DEFAULT_EVENT_LIST_LIMIT)),
            kind: query.kind,
            session_id: Some(session_id),
            ..PersistedEventQuery::default()
        })
        .await
        .map_err(|error| ApiError::internal(format!("failed to read session events: {error}")))?;
    Ok(Json(ListResponse {
        items,
        latest_sequence,
    }))
}

pub(crate) async fn list_runner_events(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    AxumPath(runner_id): AxumPath<String>,
    Query(query): Query<RecentEventsQuery>,
) -> Result<Json<ListResponse<TimelineEvent>>, ApiError> {
    let user_id = user_id_from_principal(&principal);
    {
        let registry = service.registry.read().await;
        if !registry.runner_visible_to(&runner_id, user_id) {
            return Err(ApiError::not_found(format!(
                "runner `{runner_id}` was not found"
            )));
        }
    }
    let latest_sequence = service
        .latest_persisted_event_sequence(PersistedEventQuery {
            kind: query.kind,
            runner_id: Some(runner_id.clone()),
            ..PersistedEventQuery::default()
        })
        .await;
    let latest_sequence = latest_sequence
        .map_err(|error| ApiError::internal(format!("failed to read runner events: {error}")))?;
    let items = service
        .list_persisted_events(PersistedEventQuery {
            after: query.after,
            limit: Some(query.limit.unwrap_or(DEFAULT_EVENT_LIST_LIMIT)),
            kind: query.kind,
            runner_id: Some(runner_id),
            ..PersistedEventQuery::default()
        })
        .await
        .map_err(|error| ApiError::internal(format!("failed to read runner events: {error}")))?;
    Ok(Json(ListResponse {
        items,
        latest_sequence,
    }))
}

// ---------------------------------------------------------------------------
// Event stream (WebSocket) handlers
// ---------------------------------------------------------------------------

pub(crate) async fn subscribe_events(
    ws: WebSocketUpgrade,
    Query(query): Query<EventStreamQuery>,
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
) -> Response {
    let user_id = user_id_from_principal(&principal);
    let tenant_filter = {
        let registry = service.registry.read().await;
        TenantFilter::from_registry(&registry, user_id)
    };
    let subscription = service.timeline.subscribe();
    let backlog = if query.after.is_some() {
        match service
            .list_persisted_events(PersistedEventQuery {
                after: query.after,
                kind: query.kind,
                ..PersistedEventQuery::default()
            })
            .await
        {
            Ok(backlog) => filter_events_by_tenant(backlog, &tenant_filter),
            Err(error) => {
                return ApiError::internal(format!("failed to replay persisted events: {error}"))
                    .into_response();
            }
        }
    } else {
        Vec::new()
    };
    let kind = query.kind;
    ws.on_upgrade(move |socket| {
        serve_filtered_event_stream(socket, subscription, backlog, move |event| {
            if !crate::helpers::event_matches_kind(event, kind) {
                return false;
            }
            match &tenant_filter {
                None => true,
                Some(f) => f.event_visible(event),
            }
        })
    })
}

pub(crate) async fn subscribe_session_events(
    ws: WebSocketUpgrade,
    Query(query): Query<EventStreamQuery>,
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    AxumPath(session_id): AxumPath<Uuid>,
) -> Response {
    let user_id = user_id_from_principal(&principal);
    {
        let registry = service.registry.read().await;
        if registry.get_session_for_user(session_id, user_id).is_err() {
            return ApiError::not_found(format!("session `{session_id}` was not found"))
                .into_response();
        }
    }
    let subscription = service.timeline.subscribe();
    let backlog = if query.after.is_some() {
        match service
            .list_persisted_events(PersistedEventQuery {
                after: query.after,
                kind: query.kind,
                session_id: Some(session_id),
                ..PersistedEventQuery::default()
            })
            .await
        {
            Ok(backlog) => backlog,
            Err(error) => {
                return ApiError::internal(format!(
                    "failed to replay persisted session events: {error}"
                ))
                .into_response();
            }
        }
    } else {
        Vec::new()
    };
    ws.on_upgrade(move |socket| {
        serve_session_event_stream(socket, subscription, backlog, session_id, query.kind)
    })
}

pub(crate) async fn subscribe_runner_events(
    ws: WebSocketUpgrade,
    Query(query): Query<EventStreamQuery>,
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    AxumPath(runner_id): AxumPath<String>,
) -> Response {
    let user_id = user_id_from_principal(&principal);
    {
        let registry = service.registry.read().await;
        if !registry.runner_visible_to(&runner_id, user_id) {
            return ApiError::not_found(format!("runner `{runner_id}` was not found"))
                .into_response();
        }
    }
    let subscription = service.timeline.subscribe();
    let backlog = if query.after.is_some() {
        match service
            .list_persisted_events(PersistedEventQuery {
                after: query.after,
                kind: query.kind,
                runner_id: Some(runner_id.clone()),
                ..PersistedEventQuery::default()
            })
            .await
        {
            Ok(backlog) => backlog,
            Err(error) => {
                return ApiError::internal(format!(
                    "failed to replay persisted runner events: {error}"
                ))
                .into_response();
            }
        }
    } else {
        Vec::new()
    };
    ws.on_upgrade(move |socket| {
        serve_runner_event_stream(socket, subscription, backlog, runner_id, query.kind)
    })
}

pub(crate) async fn subscribe_approvals(
    ws: WebSocketUpgrade,
    Query(query): Query<EventStreamQuery>,
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
) -> Response {
    let user_id = user_id_from_principal(&principal);
    let tenant_filter = {
        let registry = service.registry.read().await;
        TenantFilter::from_registry(&registry, user_id)
    };
    let subscription = service.timeline.subscribe();
    let backlog = if query.after.is_some() {
        match service
            .list_persisted_events(PersistedEventQuery {
                after: query.after,
                kind: query.kind,
                approvals_only: true,
                ..PersistedEventQuery::default()
            })
            .await
        {
            Ok(backlog) => filter_events_by_tenant(backlog, &tenant_filter),
            Err(error) => {
                return ApiError::internal(format!(
                    "failed to replay persisted approval events: {error}"
                ))
                .into_response();
            }
        }
    } else {
        Vec::new()
    };
    let kind = query.kind;
    ws.on_upgrade(move |socket| {
        serve_filtered_event_stream(socket, subscription, backlog, move |event| {
            if !crate::helpers::approval_event_matches(event, kind) {
                return false;
            }
            match &tenant_filter {
                None => true,
                Some(f) => f.event_visible(event),
            }
        })
    })
}

// ---------------------------------------------------------------------------
// Runner handlers
// ---------------------------------------------------------------------------

pub(crate) async fn list_runners(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
) -> Json<ListResponse<RunnerSnapshot>> {
    let registry = service.registry.read().await;
    let user_id = user_id_from_principal(&principal);
    Json(ListResponse {
        items: registry.list_runners_for_user(user_id),
        latest_sequence: None,
    })
}

pub(crate) async fn list_runner_approvals(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    AxumPath(runner_id): AxumPath<String>,
) -> Result<Json<ListResponse<claude_runner::ApprovalRequestRecord>>, ApiError> {
    let user_id = user_id_from_principal(&principal);
    let registry = service.registry.read().await;
    if !registry.runner_visible_to(&runner_id, user_id) {
        return Err(ApiError::not_found(format!(
            "runner `{runner_id}` was not found"
        )));
    }
    let items = registry.list_runner_approvals(&runner_id)?;
    drop(registry);
    let latest_sequence = service
        .latest_persisted_event_sequence(PersistedEventQuery {
            runner_id: Some(runner_id.clone()),
            approvals_only: true,
            ..PersistedEventQuery::default()
        })
        .await
        .map_err(|error| ApiError::internal(format!("failed to read runner approvals: {error}")))?;
    Ok(Json(ListResponse {
        items,
        latest_sequence,
    }))
}

pub(crate) async fn subscribe_runner_approvals(
    ws: WebSocketUpgrade,
    Query(query): Query<EventStreamQuery>,
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    AxumPath(runner_id): AxumPath<String>,
) -> Response {
    let user_id = user_id_from_principal(&principal);
    {
        let registry = service.registry.read().await;
        if !registry.runner_visible_to(&runner_id, user_id) {
            return ApiError::not_found(format!("runner `{runner_id}` was not found"))
                .into_response();
        }
    }
    let subscription = service.timeline.subscribe();
    let backlog = if query.after.is_some() {
        match service
            .list_persisted_events(PersistedEventQuery {
                after: query.after,
                kind: query.kind,
                runner_id: Some(runner_id.clone()),
                approvals_only: true,
                ..PersistedEventQuery::default()
            })
            .await
        {
            Ok(backlog) => backlog,
            Err(error) => {
                return ApiError::internal(format!(
                    "failed to replay persisted runner approvals: {error}"
                ))
                .into_response();
            }
        }
    } else {
        Vec::new()
    };
    ws.on_upgrade(move |socket| {
        serve_runner_approval_stream(socket, subscription, backlog, runner_id, query.kind)
    })
}

pub(crate) async fn subscribe_session_approvals(
    ws: WebSocketUpgrade,
    Query(query): Query<EventStreamQuery>,
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    AxumPath(session_id): AxumPath<Uuid>,
) -> Response {
    let user_id = user_id_from_principal(&principal);
    {
        let registry = service.registry.read().await;
        if registry.get_session_for_user(session_id, user_id).is_err() {
            return ApiError::not_found(format!("session `{session_id}` was not found"))
                .into_response();
        }
    }
    let subscription = service.timeline.subscribe();
    let backlog = if query.after.is_some() {
        match service
            .list_persisted_events(PersistedEventQuery {
                after: query.after,
                kind: query.kind,
                session_id: Some(session_id),
                approvals_only: true,
                ..PersistedEventQuery::default()
            })
            .await
        {
            Ok(backlog) => backlog,
            Err(error) => {
                return ApiError::internal(format!(
                    "failed to replay persisted session approvals: {error}"
                ))
                .into_response();
            }
        }
    } else {
        Vec::new()
    };
    ws.on_upgrade(move |socket| {
        serve_session_approval_stream(socket, subscription, backlog, session_id, query.kind)
    })
}

pub(crate) async fn get_runner(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    AxumPath(runner_id): AxumPath<String>,
) -> Result<Json<RunnerSnapshot>, ApiError> {
    let user_id = user_id_from_principal(&principal);
    let registry = service.registry.read().await;
    if !registry.runner_visible_to(&runner_id, user_id) {
        return Err(ApiError::not_found(format!(
            "runner `{runner_id}` was not found"
        )));
    }
    let snapshot = registry
        .runners
        .get(&runner_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found(format!("runner `{runner_id}` was not found")))?;
    Ok(Json(snapshot))
}

pub(crate) async fn register_runner(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    Json(request): Json<RunnerRegistrationRequest>,
) -> Result<Json<RunnerRegistrationResponse>, ApiError> {
    validate_runner_registration_public_base_url(
        request.public_base_url.as_deref(),
        service.meta.public_base_url.as_deref(),
    )?;
    let user_id = user_id_from_principal(&principal);
    let mut response = {
        let mut registry = service.registry.write().await;
        registry.register_runner(request, service.runner_lease_ttl_secs, user_id)
    };
    let _event = service
        .publish_event(TimelineEventDraft {
            runner_id: Some(response.runner_id.clone()),
            session_id: None,
            detail: TimelineEventDetail::RunnerRegistered {
                lease_ttl_secs: response.lease_ttl_secs,
                workspace_ids: response
                    .snapshot
                    .registration
                    .workspaces
                    .iter()
                    .map(|workspace| workspace.workspace_id.clone())
                    .collect(),
                state: response.snapshot.state,
            },
        })
        .await;
    dispatch_pending_sessions_for_runner(&service, &response.runner_id).await;
    if let Ok(snapshot) = {
        let registry = service.registry.read().await;
        registry.get_runner_snapshot(&response.runner_id)
    } {
        response.snapshot = snapshot;
    }
    Ok(Json(response))
}

fn validate_runner_registration_public_base_url(
    public_base_url: Option<&str>,
    control_plane_public_base_url: Option<&str>,
) -> Result<(), ApiError> {
    let Some(raw) = public_base_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };

    let url = reqwest::Url::parse(raw)
        .map_err(|_| ApiError::bad_request("runner public_base_url is invalid".to_owned()))?;
    let scheme = url.scheme();
    let host = url.host_str().ok_or_else(|| {
        ApiError::bad_request("runner public_base_url must include a host".to_owned())
    })?;

    let loopback = is_loopback_host(host);
    if !(scheme == "https" || (cfg!(debug_assertions) && scheme == "http" && loopback)) {
        return Err(ApiError::bad_request(
            "runner public_base_url must use https outside loopback development".to_owned(),
        ));
    }

    if !cfg!(debug_assertions) && is_forbidden_runner_public_host(host) {
        return Err(ApiError::bad_request(
            "runner public_base_url must not target loopback, private, link-local, multicast, or unspecified hosts"
                .to_owned(),
        ));
    }

    if let Some(control_plane_url) = control_plane_public_base_url
        && let Ok(control_plane) = reqwest::Url::parse(control_plane_url)
        && let Some(control_plane_host) = control_plane.host_str()
        && host.eq_ignore_ascii_case(control_plane_host)
    {
        return Err(ApiError::bad_request(
            "runner public_base_url must not point at the control plane host".to_owned(),
        ));
    }

    if let Some(allowed_hosts) = std::env::var("REMOTE_CODE_ALLOWED_RUNNER_PUBLIC_HOSTS")
        .ok()
        .map(|value| parse_allowed_runner_hosts(&value))
        .filter(|hosts| !hosts.is_empty())
        && !allowed_runner_host_matches(host, &allowed_hosts)
    {
        return Err(ApiError::bad_request(
            "runner public_base_url host is not in REMOTE_CODE_ALLOWED_RUNNER_PUBLIC_HOSTS"
                .to_owned(),
        ));
    }

    Ok(())
}

fn parse_allowed_runner_hosts(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|host| host.trim().trim_start_matches('.').to_ascii_lowercase())
        .filter(|host| !host.is_empty())
        .collect()
}

fn allowed_runner_host_matches(host: &str, allowed_hosts: &[String]) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    allowed_hosts
        .iter()
        .any(|allowed| host == *allowed || host.ends_with(&format!(".{allowed}")))
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| match ip {
            IpAddr::V4(ip) => ip.is_loopback(),
            IpAddr::V6(ip) => ip.is_loopback(),
        })
}

fn is_forbidden_runner_public_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let Ok(ip) = host.parse::<IpAddr>() else {
        return false;
    };
    match ip {
        IpAddr::V4(ip) => is_forbidden_ipv4(ip),
        IpAddr::V6(ip) => is_forbidden_ipv6(ip),
    }
}

fn is_forbidden_ipv4(ip: Ipv4Addr) -> bool {
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || ip.is_multicast()
}

fn is_forbidden_ipv6(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    let unique_local = (first & 0xfe00) == 0xfc00;
    let link_local = (first & 0xffc0) == 0xfe80;
    ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() || unique_local || link_local
}

pub(crate) async fn update_runner_heartbeat(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    AxumPath(runner_id): AxumPath<String>,
    Json(heartbeat): Json<RunnerHeartbeat>,
) -> Result<Json<RunnerSnapshot>, ApiError> {
    let user_id = user_id_from_principal(&principal);
    {
        let registry = service.registry.read().await;
        if !registry.runner_visible_to(&runner_id, user_id) {
            return Err(ApiError::not_found(format!(
                "runner `{runner_id}` was not found"
            )));
        }
    }
    let snapshot = {
        let mut registry = service.registry.write().await;
        registry.apply_heartbeat(&runner_id, heartbeat)?
    };
    let _event = service
        .publish_event(TimelineEventDraft {
            runner_id: Some(snapshot.registration.runner_id.clone()),
            session_id: None,
            detail: TimelineEventDetail::RunnerHeartbeat {
                state: snapshot.state,
                active_sessions: snapshot.active_sessions,
                queued_sessions: snapshot.queued_sessions,
                reported_at: snapshot.last_seen_at,
            },
        })
        .await;

    // Reap sessions stuck in Assigned whose runner has gone offline (3x TTL without heartbeat)
    {
        let mut registry = service.registry.write().await;
        let reaped = registry.reap_orphaned_assigned_sessions(service.runner_lease_ttl_secs);
        if !reaped.is_empty() {
            for (session_id, old_runner_id, previous_state) in &reaped {
                let session = registry.sessions.get(session_id);
                let new_state = session.map(|s| s.state).unwrap_or(SessionState::Pending);
                let _ = service
                    .publish_event(TimelineEventDraft {
                        runner_id: Some(old_runner_id.clone()),
                        session_id: Some(*session_id),
                        detail: TimelineEventDetail::SessionStateChanged {
                            previous_state: *previous_state,
                            state: new_state,
                        },
                    })
                    .await;
            }
        }
    }

    dispatch_pending_sessions_for_runner(&service, &runner_id).await;
    let snapshot = {
        let registry = service.registry.read().await;
        registry.get_runner_snapshot(&runner_id)?
    };
    Ok(Json(snapshot))
}

pub(crate) async fn pull_runner_commands(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    AxumPath(runner_id): AxumPath<String>,
    Query(query): Query<RunnerCommandPullQuery>,
) -> Result<Json<RunnerCommandPullResponse>, ApiError> {
    let user_id = user_id_from_principal(&principal);
    {
        let registry = service.registry.read().await;
        if !registry.runner_visible_to(&runner_id, user_id) {
            return Err(ApiError::not_found(format!(
                "runner `{runner_id}` was not found"
            )));
        }
    }
    let limit = query.limit.unwrap_or(16).clamp(1, 64);
    let deadline = tokio::time::Instant::now()
        + std::time::Duration::from_secs(query.timeout.unwrap_or(0).min(30));
    loop {
        let commands = {
            let mut registry = service.registry.write().await;
            registry.pull_runner_commands(&runner_id, limit)?
        };
        if !commands.is_empty() {
            persist_state_logged(&service).await;
            return Ok(Json(RunnerCommandPullResponse { commands }));
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(Json(RunnerCommandPullResponse { commands }));
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

// ---------------------------------------------------------------------------
// Session handlers
// ---------------------------------------------------------------------------

pub(crate) async fn list_sessions(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    Query(query): Query<ListSessionsQuery>,
) -> Json<ListResponse<SessionView>> {
    let registry = service.registry.read().await;
    let user_id = user_id_from_principal(&principal);
    Json(ListResponse {
        items: build_session_views(
            &service,
            &registry,
            registry.list_sessions_filtered(&query, user_id),
        ),
        latest_sequence: None,
    })
}

pub(crate) async fn get_session(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    AxumPath(session_id): AxumPath<Uuid>,
) -> Result<Json<SessionView>, ApiError> {
    let registry = service.registry.read().await;
    let user_id = user_id_from_principal(&principal);
    Ok(Json(build_session_view(
        &service,
        &registry,
        registry.get_session_for_user(session_id, user_id)?,
    )))
}

pub(crate) async fn list_runner_sessions(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    AxumPath(runner_id): AxumPath<String>,
    Query(mut query): Query<ListSessionsQuery>,
) -> Result<Json<ListResponse<SessionView>>, ApiError> {
    let user_id = user_id_from_principal(&principal);
    let registry = service.registry.read().await;
    if !registry.runner_visible_to(&runner_id, user_id) {
        return Err(ApiError::not_found(format!(
            "runner `{runner_id}` was not found"
        )));
    }
    query.runner_id = Some(runner_id);
    Ok(Json(ListResponse {
        items: build_session_views(
            &service,
            &registry,
            registry.list_sessions_filtered(&query, user_id),
        ),
        latest_sequence: None,
    }))
}

pub(crate) async fn list_runner_artifacts(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    AxumPath(runner_id): AxumPath<String>,
) -> Result<Json<ListResponse<ArtifactRecord>>, ApiError> {
    let user_id = user_id_from_principal(&principal);
    let registry = service.registry.read().await;
    if !registry.runner_visible_to(&runner_id, user_id) {
        return Err(ApiError::not_found(format!(
            "runner `{runner_id}` was not found"
        )));
    }
    Ok(Json(ListResponse {
        items: registry.list_runner_artifacts(&runner_id)?,
        latest_sequence: None,
    }))
}

pub(crate) async fn update_session_state(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    AxumPath(session_id): AxumPath<Uuid>,
    Json(request): Json<SessionStateUpdateRequest>,
) -> Result<Json<SessionView>, ApiError> {
    let user_id = user_id_from_principal(&principal);
    let existing = {
        let registry = service.registry.read().await;
        registry.get_session_for_user(session_id, user_id)?
    };
    let requested_state = request.state;
    let metadata = request.metadata.clone();

    let runner_update = if let Some(runner_id) = existing.owner_runner_id.as_deref() {
        let runner =
            {
                let registry = service.registry.read().await;
                registry.runners.get(runner_id).cloned().ok_or_else(|| {
                    ApiError::not_found(format!("runner `{runner_id}` was not found"))
                })?
            };
        if runner_uses_pull_commands(&runner) {
            {
                let mut registry = service.registry.write().await;
                registry.enqueue_runner_command(
                    runner_id,
                    RunnerQueuedCommandBody::UpdateSessionState {
                        session_id,
                        request: RunnerSessionStateUpdateRequest {
                            state: session_state_to_runner(requested_state),
                            metadata: metadata.clone(),
                        },
                    },
                )?;
            }
            persist_state_logged(&service).await;
            None
        } else {
            Some(
                update_runner_session_state(
                    &service.http_client,
                    &runner,
                    session_id,
                    &RunnerSessionStateUpdateRequest {
                        state: session_state_to_runner(requested_state),
                        metadata: metadata.clone(),
                    },
                )
                .await?,
            )
        }
    } else {
        None
    };

    let (updated, previous_state) = {
        let mut registry = service.registry.write().await;
        let updated_at = runner_update
            .as_ref()
            .map_or_else(Utc::now, |record| record.updated_at);
        registry.apply_session_state_update(
            session_id,
            runner_update.as_ref().map_or(requested_state, |record| {
                session_state_from_runner(record.state)
            }),
            runner_update
                .as_ref()
                .map(|record| record.metadata.clone())
                .unwrap_or(metadata),
            updated_at,
        )?
    };
    let _event = service
        .publish_event(TimelineEventDraft {
            runner_id: updated.owner_runner_id.clone(),
            session_id: Some(updated.session_id),
            detail: TimelineEventDetail::SessionStateChanged {
                previous_state,
                state: updated.state,
            },
        })
        .await;
    let registry = service.registry.read().await;
    Ok(Json(build_session_view(&service, &registry, updated)))
}

pub(crate) async fn post_session_command(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    AxumPath(session_id): AxumPath<Uuid>,
    Json(request): Json<RunnerSessionCommandRequest>,
) -> Result<Json<RunnerSessionCommandResponse>, ApiError> {
    let user_id = user_id_from_principal(&principal);
    let session = {
        let registry = service.registry.read().await;
        registry.get_session_for_user(session_id, user_id)?
    };
    let Some(owner_runner_id) = session.owner_runner_id.as_deref() else {
        return Err(ApiError::service_unavailable(format!(
            "session `{session_id}` is not assigned to a runner"
        )));
    };
    let runner = {
        let registry = service.registry.read().await;
        registry
            .runners
            .get(owner_runner_id)
            .cloned()
            .ok_or_else(|| {
                ApiError::not_found(format!("runner `{owner_runner_id}` was not found"))
            })?
    };
    if runner_uses_pull_commands(&runner) {
        let runner_available = runner_is_available(&runner, service.runner_lease_ttl_secs);
        {
            let mut registry = service.registry.write().await;
            registry.enqueue_runner_command(
                owner_runner_id,
                RunnerQueuedCommandBody::SessionCommand {
                    session_id,
                    request: request.clone(),
                },
            )?;
        }
        persist_state_logged(&service).await;
        let message = match (request, runner_available) {
            (RunnerSessionCommandRequest::SendPrompt { .. }, true) => {
                "prompt queued for runner delivery"
            }
            (RunnerSessionCommandRequest::Interrupt, true) => {
                "interrupt queued for runner delivery"
            }
            (RunnerSessionCommandRequest::SendPrompt { .. }, false) => {
                "prompt queued; runner currently unavailable"
            }
            (RunnerSessionCommandRequest::Interrupt, false) => {
                "interrupt queued; runner currently unavailable"
            }
        };
        return Ok(Json(RunnerSessionCommandResponse {
            session_id,
            accepted: true,
            message: message.to_owned(),
        }));
    }
    let response =
        dispatch_session_command_to_runner(&service.http_client, &runner, session_id, &request)
            .await?;
    Ok(Json(response))
}

pub(crate) async fn list_session_approvals(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    AxumPath(session_id): AxumPath<Uuid>,
) -> Result<Json<ListResponse<claude_runner::ApprovalRequestRecord>>, ApiError> {
    let user_id = user_id_from_principal(&principal);
    let registry = service.registry.read().await;
    registry.get_session_for_user(session_id, user_id)?;
    let items = registry.list_session_approvals(session_id)?;
    drop(registry);
    let latest_sequence = service
        .latest_persisted_event_sequence(PersistedEventQuery {
            session_id: Some(session_id),
            approvals_only: true,
            ..PersistedEventQuery::default()
        })
        .await
        .map_err(|error| {
            ApiError::internal(format!("failed to read session approvals: {error}"))
        })?;
    Ok(Json(ListResponse {
        items,
        latest_sequence,
    }))
}

pub(crate) async fn list_session_artifacts(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    AxumPath(session_id): AxumPath<Uuid>,
) -> Result<Json<ListResponse<ArtifactRecord>>, ApiError> {
    let user_id = user_id_from_principal(&principal);
    let registry = service.registry.read().await;
    registry.get_session_for_user(session_id, user_id)?;
    Ok(Json(ListResponse {
        items: registry.list_session_artifacts(session_id)?,
        latest_sequence: None,
    }))
}

pub(crate) async fn create_session(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    Json(request): Json<CreateSessionRequest>,
) -> Result<(StatusCode, Json<SessionView>), ApiError> {
    let user_id = user_id_from_principal(&principal);
    let planned = {
        let registry = service.registry.read().await;
        registry.plan_session(&request, service.runner_lease_ttl_secs, user_id)?
    };
    let mut record = planned.record;

    if let Some(owner_runner) = planned.owner_runner {
        let dispatch_request = RunnerSessionCreateRequest {
            session_id: Some(record.session_id),
            workspace_id: record.workspace_id.clone(),
            metadata: record.metadata.clone(),
        };
        if runner_uses_pull_commands(&owner_runner) {
            let mut registry = service.registry.write().await;
            registry.enqueue_runner_command(
                &owner_runner.registration.runner_id,
                RunnerQueuedCommandBody::CreateSession {
                    request: dispatch_request,
                },
            )?;
        } else {
            let dispatched =
                dispatch_session_to_runner(&service.http_client, &owner_runner, &dispatch_request)
                    .await?;
            record.state = session_state_from_runner(dispatched.state);
            record.updated_at = dispatched.updated_at;
        }
    }

    let record = {
        let mut registry = service.registry.write().await;
        registry.commit_session(record)?
    };
    let _event = service
        .publish_event(TimelineEventDraft {
            runner_id: record.owner_runner_id.clone(),
            session_id: Some(record.session_id),
            detail: TimelineEventDetail::SessionCreated {
                workspace_id: record.workspace_id.clone(),
                owner_runner_id: record.owner_runner_id.clone(),
                state: record.state,
            },
        })
        .await;
    let registry = service.registry.read().await;
    Ok((
        StatusCode::CREATED,
        Json(build_session_view(&service, &registry, record)),
    ))
}

pub(crate) async fn create_session_runtime_event(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    AxumPath(session_id): AxumPath<Uuid>,
    Json(request): Json<RuntimeEventCreateRequest>,
) -> Result<(StatusCode, Json<TimelineEvent>), ApiError> {
    let user_id = user_id_from_principal(&principal);
    let session = {
        let registry = service.registry.read().await;
        let session = registry.get_session_for_user(session_id, user_id)?;
        validate_runtime_event_request(&registry, session_id, &request.detail)?;
        session
    };
    let event = service
        .publish_event(TimelineEventDraft {
            runner_id: session.owner_runner_id,
            session_id: Some(session_id),
            detail: request.detail.into(),
        })
        .await;
    Ok((StatusCode::CREATED, Json(event)))
}

// ---------------------------------------------------------------------------
// Artifact handlers
// ---------------------------------------------------------------------------

pub(crate) async fn list_artifacts(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
) -> Json<ListResponse<ArtifactRecord>> {
    let user_id = user_id_from_principal(&principal);
    let registry = service.registry.read().await;
    Json(ListResponse {
        items: registry.list_artifacts_for_user(user_id),
        latest_sequence: None,
    })
}

pub(crate) async fn get_artifact(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    AxumPath(artifact_id): AxumPath<Uuid>,
) -> Result<Json<ArtifactRecord>, ApiError> {
    let user_id = user_id_from_principal(&principal);
    let registry = service.registry.read().await;
    let artifact = registry.get_artifact(artifact_id)?;
    registry.get_session_for_user(artifact.session_id, user_id)?;
    Ok(Json(artifact))
}

pub(crate) async fn create_artifact(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    AxumPath(session_id): AxumPath<Uuid>,
    Json(request): Json<ArtifactCreateRequest>,
) -> Result<(StatusCode, Json<ArtifactRecord>), ApiError> {
    let user_id = user_id_from_principal(&principal);
    {
        let registry = service.registry.read().await;
        registry.get_session_for_user(session_id, user_id)?;
    }
    let encoded_len = request.content_base64.len();
    if encoded_len > MAX_ARTIFACT_SIZE_BYTES {
        return Err(ApiError::bad_request(format!(
            "artifact exceeds {MAX_ARTIFACT_SIZE_BYTES} byte limit (got {encoded_len} bytes encoded)"
        )));
    }
    let contents = BASE64_STANDARD
        .decode(request.content_base64.as_bytes())
        .map_err(|error| {
            ApiError::bad_request(format!("artifact content is not valid base64: {error}"))
        })?;
    let artifact = {
        let mut registry = service.registry.write().await;
        registry.register_artifact(session_id, &request, contents.len() as u64)?
    };
    let path = artifact_file_path(&service.artifact_root_dir, &artifact);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|error| {
            ApiError::internal(format!("failed to create {}: {error}", parent.display()))
        })?;
    }
    tokio::fs::write(&path, &contents).await.map_err(|error| {
        ApiError::internal(format!("failed to write {}: {error}", path.display()))
    })?;
    let _event = service
        .publish_event(TimelineEventDraft {
            runner_id: artifact.runner_id.clone(),
            session_id: Some(artifact.session_id),
            detail: TimelineEventDetail::ArtifactCreated {
                artifact_id: artifact.artifact_id,
                name: artifact.name.clone(),
                file_name: artifact.file_name.clone(),
                media_type: artifact.media_type.clone(),
                size_bytes: artifact.size_bytes,
            },
        })
        .await;
    Ok((StatusCode::CREATED, Json(artifact)))
}

pub(crate) async fn download_artifact(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    AxumPath(artifact_id): AxumPath<Uuid>,
) -> Result<Response, ApiError> {
    let user_id = user_id_from_principal(&principal);
    let artifact = {
        let registry = service.registry.read().await;
        let artifact = registry.get_artifact(artifact_id)?;
        registry.get_session_for_user(artifact.session_id, user_id)?;
        artifact
    };
    let path = artifact_file_path(&service.artifact_root_dir, &artifact);
    let bytes = tokio::fs::read(&path).await.map_err(|error| {
        ApiError::internal(format!("failed to read {}: {error}", path.display()))
    })?;
    Ok((
        [
            (CONTENT_TYPE, artifact.media_type.clone()),
            (
                CONTENT_DISPOSITION,
                build_content_disposition(&artifact.file_name),
            ),
        ],
        bytes,
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// Approval handlers
// ---------------------------------------------------------------------------

pub(crate) async fn list_approvals(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
) -> Json<ListResponse<claude_runner::ApprovalRequestRecord>> {
    let user_id = user_id_from_principal(&principal);
    let registry = service.registry.read().await;
    let items = registry.list_approvals_for_user(user_id);
    drop(registry);
    let latest_sequence = service
        .latest_persisted_event_sequence(PersistedEventQuery {
            approvals_only: true,
            ..PersistedEventQuery::default()
        })
        .await
        .ok()
        .flatten();
    Json(ListResponse {
        items,
        latest_sequence,
    })
}

pub(crate) async fn get_approval(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    AxumPath(approval_id): AxumPath<Uuid>,
) -> Result<Json<claude_runner::ApprovalRequestRecord>, ApiError> {
    let user_id = user_id_from_principal(&principal);
    let registry = service.registry.read().await;
    let approval = registry.get_approval(approval_id)?;
    registry.get_session_for_user(approval.session_id, user_id)?;
    Ok(Json(approval))
}

pub(crate) async fn create_approval(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    AxumPath(session_id): AxumPath<Uuid>,
    Json(request): Json<ApprovalCreateRequest>,
) -> Result<(StatusCode, Json<claude_runner::ApprovalRequestRecord>), ApiError> {
    let user_id = user_id_from_principal(&principal);
    {
        let registry = service.registry.read().await;
        registry.get_session_for_user(session_id, user_id)?;
    }
    let planned = {
        let registry = service.registry.read().await;
        registry.plan_approval(session_id, request)?
    };
    if let Some(runner) = planned.owner_runner.as_ref()
        && !runner_uses_pull_commands(runner)
    {
        let relay_request = ApprovalCreateRequest {
            approval_id: Some(planned.approval.approval_id),
            title: planned.approval.title.clone(),
            description: planned.approval.description.clone(),
            metadata: planned.approval.metadata.clone(),
        };
        let relayed =
            relay_approval_to_runner(&service.http_client, runner, session_id, &relay_request)
                .await?;
        if relayed.approval_id != planned.approval.approval_id {
            return Err(ApiError::bad_gateway(format!(
                "runner `{}` acknowledged approval `{}` instead of `{}`",
                runner.registration.runner_id, relayed.approval_id, planned.approval.approval_id
            )));
        }
        if relayed.session_id != session_id || relayed.runner_id != runner.registration.runner_id {
            return Err(ApiError::bad_gateway(format!(
                "runner `{}` returned mismatched approval routing for session `{session_id}`",
                runner.registration.runner_id
            )));
        }
    }
    let (approval, transition) = {
        let mut registry = service.registry.write().await;
        registry.commit_planned_approval(planned)?
    };
    let _event = service
        .publish_event(TimelineEventDraft {
            runner_id: (!approval.runner_id.is_empty()).then(|| approval.runner_id.clone()),
            session_id: Some(approval.session_id),
            detail: TimelineEventDetail::ApprovalRequested {
                approval_id: approval.approval_id,
                title: approval.title.clone(),
                state: approval.state,
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
    if !approval.runner_id.is_empty() {
        let runner = {
            let registry = service.registry.read().await;
            registry.runners.get(&approval.runner_id).cloned()
        };
        if let Some(runner) = runner
            && runner_uses_pull_commands(&runner)
        {
            {
                let mut registry = service.registry.write().await;
                registry.enqueue_runner_command(
                    &approval.runner_id,
                    RunnerQueuedCommandBody::CreateApproval {
                        session_id,
                        request: ApprovalCreateRequest {
                            approval_id: Some(approval.approval_id),
                            title: approval.title.clone(),
                            description: approval.description.clone(),
                            metadata: approval.metadata.clone(),
                        },
                    },
                )?;
            }
            persist_state_logged(&service).await;
        }
    }
    Ok((StatusCode::CREATED, Json(approval)))
}

pub(crate) async fn apply_approval_decision(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    AxumPath(approval_id): AxumPath<Uuid>,
    Json(request): Json<ApprovalDecisionRequest>,
) -> Result<Json<claude_runner::ApprovalRequestRecord>, ApiError> {
    let user_id = user_id_from_principal(&principal);
    // Verify the approval belongs to a session owned by this user.
    {
        let registry = service.registry.read().await;
        let approval = registry.get_approval(approval_id)?;
        registry.get_session_for_user(approval.session_id, user_id)?;
    }
    let planned = {
        let registry = service.registry.read().await;
        registry.plan_approval_decision(approval_id, request)?
    };
    let queue_for_runner = planned
        .owner_runner
        .as_ref()
        .is_some_and(runner_uses_pull_commands);
    if let Some(runner) = planned.owner_runner.as_ref()
        && !queue_for_runner
    {
        let relay_request = ApprovalDecisionRequest {
            decision: match planned.approval.state {
                ApprovalState::Approved => ApprovalDecision::Approved,
                ApprovalState::Denied => ApprovalDecision::Denied,
                ApprovalState::Cancelled => ApprovalDecision::Cancelled,
                ApprovalState::Pending => {
                    return Err(ApiError::internal(format!(
                        "approval `{approval_id}` remained pending during decision relay"
                    )));
                }
            },
            responder: planned.approval.responder.clone(),
            note: planned.approval.note.clone(),
        };
        let relayed = relay_approval_decision_to_runner(
            &service.http_client,
            runner,
            planned.approval.approval_id,
            &relay_request,
        )
        .await?;
        if relayed.approval_id != planned.approval.approval_id {
            return Err(ApiError::bad_gateway(format!(
                "runner `{}` acknowledged approval decision for `{}` instead of `{}`",
                runner.registration.runner_id, relayed.approval_id, planned.approval.approval_id
            )));
        }
        if relayed.state != planned.approval.state {
            return Err(ApiError::bad_gateway(format!(
                "runner `{}` returned approval state `{:?}` instead of `{:?}` for `{}`",
                runner.registration.runner_id,
                relayed.state,
                planned.approval.state,
                planned.approval.approval_id
            )));
        }
    }
    let (approval, transition) = {
        let mut registry = service.registry.write().await;
        registry.commit_planned_approval_decision(planned)?
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
        {
            let mut registry = service.registry.write().await;
            registry.enqueue_runner_command(
                &approval.runner_id,
                RunnerQueuedCommandBody::ApplyApprovalDecision {
                    approval_id: approval.approval_id,
                    request: ApprovalDecisionRequest {
                        decision: match approval.state {
                            ApprovalState::Approved => ApprovalDecision::Approved,
                            ApprovalState::Denied => ApprovalDecision::Denied,
                            ApprovalState::Cancelled => ApprovalDecision::Cancelled,
                            ApprovalState::Pending => {
                                return Err(ApiError::internal(format!(
                                    "approval `{approval_id}` remained pending after commit"
                                )));
                            }
                        },
                        responder: approval.responder.clone(),
                        note: approval.note.clone(),
                    },
                },
            )?;
        }
        persist_state_logged(&service).await;
    }
    Ok(Json(approval))
}

// ---------------------------------------------------------------------------
// Internal dispatch helper
// ---------------------------------------------------------------------------

async fn dispatch_pending_sessions_for_runner(service: &ControlPlaneService, runner_id: &str) {
    let mut skipped_session_ids = BTreeSet::new();

    loop {
        let planned = {
            let registry = service.registry.read().await;
            registry
                .plan_next_pending_session_for_runner(
                    runner_id,
                    service.runner_lease_ttl_secs,
                    &skipped_session_ids,
                )
                .ok()
                .flatten()
        };
        let Some(planned) = planned else {
            break;
        };

        let request = RunnerSessionCreateRequest {
            session_id: Some(planned.session_id),
            workspace_id: planned.workspace_id.clone(),
            metadata: planned.metadata.clone(),
        };
        let dispatched = if runner_uses_pull_commands(&planned.runner) {
            let committed = {
                let mut registry = service.registry.write().await;
                if registry
                    .enqueue_runner_command(
                        &planned.runner.registration.runner_id,
                        RunnerQueuedCommandBody::CreateSession {
                            request: request.clone(),
                        },
                    )
                    .is_err()
                {
                    None
                } else {
                    registry
                        .commit_pending_session_dispatch(
                            planned.session_id,
                            &planned.runner.registration.runner_id,
                            &RunnerSessionRecord {
                                session_id: planned.session_id,
                                runner_id: planned.runner.registration.runner_id.clone(),
                                workspace_id: planned.workspace_id.clone(),
                                state: claude_runner::SessionState::Pending,
                                metadata: planned.metadata.clone(),
                                created_at: Utc::now(),
                                updated_at: Utc::now(),
                            },
                        )
                        .ok()
                }
            };
            let Some((record, previous_state)) = committed else {
                skipped_session_ids.insert(planned.session_id);
                continue;
            };
            persist_state_logged(service).await;
            let _event = service
                .publish_event(TimelineEventDraft {
                    runner_id: record.owner_runner_id.clone(),
                    session_id: Some(record.session_id),
                    detail: TimelineEventDetail::SessionStateChanged {
                        previous_state,
                        state: record.state,
                    },
                })
                .await;
            continue;
        } else if let Ok(dispatched) =
            dispatch_session_to_runner(&service.http_client, &planned.runner, &request).await
        {
            dispatched
        } else {
            skipped_session_ids.insert(planned.session_id);
            continue;
        };

        let committed = {
            let mut registry = service.registry.write().await;
            registry
                .commit_pending_session_dispatch(
                    planned.session_id,
                    &planned.runner.registration.runner_id,
                    &dispatched,
                )
                .ok()
        };
        let Some((record, previous_state)) = committed else {
            skipped_session_ids.insert(planned.session_id);
            continue;
        };

        let _event = service
            .publish_event(TimelineEventDraft {
                runner_id: record.owner_runner_id.clone(),
                session_id: Some(record.session_id),
                detail: TimelineEventDetail::SessionStateChanged {
                    previous_state,
                    state: record.state,
                },
            })
            .await;
    }
}

fn validate_runtime_event_request(
    registry: &crate::registry::Registry,
    session_id: Uuid,
    detail: &RuntimeEventDetail,
) -> Result<(), ApiError> {
    match detail {
        RuntimeEventDetail::MessageDelta { delta, .. } => {
            require_non_empty_runtime_field("delta", delta)?;
        }
        RuntimeEventDetail::MessageCommitted { text, .. } => {
            require_non_empty_runtime_field("text", text)?;
        }
        RuntimeEventDetail::ToolStarted {
            tool_call_id,
            tool_name,
        } => {
            require_non_empty_runtime_field("tool_call_id", tool_call_id)?;
            require_non_empty_runtime_field("tool_name", tool_name)?;
        }
        RuntimeEventDetail::ToolProgress {
            tool_call_id,
            tool_name,
            delta,
            elapsed_time_seconds,
        } => {
            if tool_call_id.as_deref().is_none_or(str::is_empty)
                && tool_name.as_deref().is_none_or(str::is_empty)
            {
                return Err(ApiError::bad_request(
                    "runtime tool_progress events require tool_call_id or tool_name".to_owned(),
                ));
            }
            if delta.as_deref().is_none_or(str::is_empty) && elapsed_time_seconds.is_none() {
                return Err(ApiError::bad_request(
                    "runtime tool_progress events require delta or elapsed_time_seconds".to_owned(),
                ));
            }
        }
        RuntimeEventDetail::ToolFinished {
            tool_call_id,
            tool_name,
            ..
        } => {
            require_non_empty_runtime_field("tool_call_id", tool_call_id)?;
            require_non_empty_runtime_field("tool_name", tool_name)?;
        }
        RuntimeEventDetail::ArtifactManifest { artifact_ids } => {
            if artifact_ids.is_empty() {
                return Err(ApiError::bad_request(
                    "runtime artifact_manifest events require at least one artifact_id".to_owned(),
                ));
            }
            for artifact_id in artifact_ids {
                let artifact = registry.get_artifact(*artifact_id)?;
                if artifact.session_id != session_id {
                    return Err(ApiError::conflict(format!(
                        "artifact `{artifact_id}` does not belong to session `{session_id}`"
                    )));
                }
            }
        }
        RuntimeEventDetail::RuntimeError { message } => {
            require_non_empty_runtime_field("message", message)?;
        }
        RuntimeEventDetail::DaemonPresenceChanged { .. } => {}
        RuntimeEventDetail::SubtaskStarted { description, .. } => {
            require_non_empty_runtime_field("description", description)?;
        }
        RuntimeEventDetail::SubtaskProgress { task_id, .. } => {
            require_non_empty_runtime_field("task_id", task_id)?;
        }
        RuntimeEventDetail::SubtaskCompleted { task_id, .. } => {
            require_non_empty_runtime_field("task_id", task_id)?;
        }
        RuntimeEventDetail::BatchProgress { .. } => {}
        RuntimeEventDetail::ContextUsage { .. } => {}
        RuntimeEventDetail::ContextOverflow { .. } => {}
        RuntimeEventDetail::ContextCompacted { .. } => {}
    }
    Ok(())
}

fn require_non_empty_runtime_field(field: &str, value: &str) -> Result<(), ApiError> {
    if value.trim().is_empty() {
        return Err(ApiError::bad_request(format!(
            "runtime event field `{field}` cannot be empty"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Push-token registration (mobile devices)
// ---------------------------------------------------------------------------

pub(crate) async fn register_push_token(
    State(service): State<ControlPlaneService>,
    Extension(principal): Extension<AuthPrincipal>,
    Json(body): Json<PushTokenRegistrationRequest>,
) -> Result<Json<PushTokenRegistrationResponse>, ApiError> {
    if body.push_token.trim().is_empty() {
        return Err(ApiError::bad_request(
            "push_token cannot be empty".to_owned(),
        ));
    }
    // Device-based registration (legacy pairing flow) uses the device_id as key.
    // User-based registration (tenant flow) uses the user_key as key.
    let mut registry = service.registry.write().await;
    if let Some(device_id) = principal.created_by_device_id() {
        let _is_new = registry.register_push_token(device_id, body)?;
    } else if let Some(user_id) = principal.user_id() {
        let _is_new = registry.register_user_push_token(user_id.to_owned(), body);
    } else {
        drop(registry);
        return Err(ApiError::unauthorized(
            "no device or user identity".to_owned(),
        ));
    }
    drop(registry);

    service
        .persist_state()
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    Ok(Json(PushTokenRegistrationResponse { registered: true }))
}
