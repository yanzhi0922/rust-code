//! Route definitions and router construction for the control plane.

use axum::Router;
use axum::middleware;
use axum::routing::{delete, get, post};

use crate::auth::{build_cors_layer, require_api_auth};
use crate::download::{download_file, download_page};
use crate::handlers::{
    accept_pairing_offer, apply_approval_decision, claim_bootstrap_device, create_approval,
    create_artifact, create_pairing_offer, create_session, create_session_runtime_event,
    create_stream_ticket, download_artifact, get_approval, get_artifact, get_health, get_meta,
    get_runner, get_session, list_approvals, list_artifacts, list_devices, list_recent_events,
    list_runner_approvals, list_runner_artifacts, list_runner_events, list_runner_sessions,
    list_runners, list_session_approvals, list_session_artifacts, list_session_events,
    list_sessions, post_session_command, pull_runner_commands, refresh_token, register_push_token,
    register_runner, revoke_device, subscribe_approvals, subscribe_events,
    subscribe_runner_approvals, subscribe_runner_events, subscribe_session_approvals,
    subscribe_session_events, update_runner_heartbeat, update_session_state,
};
use crate::state::ControlPlaneService;

impl ControlPlaneService {
    pub fn router(self) -> Router {
        let protected = Router::new()
            .route("/v1/meta", get(get_meta))
            .route("/v1/stream-ticket", post(create_stream_ticket))
            .route("/v1/devices", get(list_devices))
            .route("/v1/devices/{device_id}", delete(revoke_device))
            .route("/v1/events", get(list_recent_events))
            .route("/v1/events/stream", get(subscribe_events))
            .route(
                "/v1/sessions/{session_id}/events/stream",
                get(subscribe_session_events),
            )
            .route("/v1/runners/{runner_id}/events", get(list_runner_events))
            .route(
                "/v1/runners/{runner_id}/events/stream",
                get(subscribe_runner_events),
            )
            .route("/v1/approvals/stream", get(subscribe_approvals))
            .route("/v1/approvals", get(list_approvals))
            .route("/v1/approvals/{approval_id}", get(get_approval))
            .route(
                "/v1/approvals/{approval_id}/decision",
                post(apply_approval_decision),
            )
            .route("/v1/artifacts", get(list_artifacts))
            .route("/v1/artifacts/{artifact_id}", get(get_artifact))
            .route(
                "/v1/artifacts/{artifact_id}/download",
                get(download_artifact),
            )
            .route("/v1/runners", get(list_runners))
            .route("/v1/runners/register", post(register_runner))
            .route("/v1/runners/{runner_id}", get(get_runner))
            .route(
                "/v1/runners/{runner_id}/artifacts",
                get(list_runner_artifacts),
            )
            .route(
                "/v1/runners/{runner_id}/sessions",
                get(list_runner_sessions),
            )
            .route(
                "/v1/runners/{runner_id}/approvals",
                get(list_runner_approvals),
            )
            .route(
                "/v1/runners/{runner_id}/approvals/stream",
                get(subscribe_runner_approvals),
            )
            .route(
                "/v1/runners/{runner_id}/heartbeat",
                post(update_runner_heartbeat),
            )
            .route(
                "/v1/runners/{runner_id}/commands/pull",
                post(pull_runner_commands),
            )
            .route("/v1/devices/push-token", post(register_push_token))
            .route("/v1/pairing/offers", post(create_pairing_offer))
            .route("/v1/sessions", get(list_sessions).post(create_session))
            .route("/v1/sessions/{session_id}", get(get_session))
            .route(
                "/v1/sessions/{session_id}/state",
                post(update_session_state),
            )
            .route(
                "/v1/sessions/{session_id}/commands",
                post(post_session_command),
            )
            .route(
                "/v1/sessions/{session_id}/events",
                get(list_session_events).post(create_session_runtime_event),
            )
            .route(
                "/v1/sessions/{session_id}/approvals",
                get(list_session_approvals).post(create_approval),
            )
            .route(
                "/v1/sessions/{session_id}/approvals/stream",
                get(subscribe_session_approvals),
            )
            .route(
                "/v1/sessions/{session_id}/artifacts",
                get(list_session_artifacts).post(create_artifact),
            )
            // App download page and file serving.
            .route("/download", get(download_page))
            .route("/downloads/{filename}", get(download_file))
            .route_layer(middleware::from_fn_with_state(
                self.clone(),
                require_api_auth,
            ));

        Router::new()
            .route("/healthz", get(get_health))
            .route("/v1/bootstrap/claim", post(claim_bootstrap_device))
            .route("/v1/pairing/accept", post(accept_pairing_offer))
            .route("/v1/auth/refresh", post(refresh_token))
            .merge(protected)
            .layer(build_cors_layer(self.meta.public_base_url.as_deref()))
            .with_state(self)
    }
}
