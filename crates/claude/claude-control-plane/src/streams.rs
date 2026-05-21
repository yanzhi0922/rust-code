//! WebSocket stream serving functions for real-time event delivery.

use axum::extract::ws::{Message, WebSocket};
use futures::SinkExt;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::types::{TimelineEvent, TimelineEventKind};

// ---------------------------------------------------------------------------
// WebSocket stream serving functions
// ---------------------------------------------------------------------------

pub(crate) async fn serve_session_event_stream(
    socket: WebSocket,
    subscription: broadcast::Receiver<TimelineEvent>,
    backlog: Vec<TimelineEvent>,
    session_id: Uuid,
    kind: Option<TimelineEventKind>,
) {
    serve_filtered_event_stream(socket, subscription, backlog, move |event| {
        event.session_id == Some(session_id) && crate::helpers::event_matches_kind(event, kind)
    })
    .await;
}

pub(crate) async fn serve_runner_event_stream(
    socket: WebSocket,
    subscription: broadcast::Receiver<TimelineEvent>,
    backlog: Vec<TimelineEvent>,
    runner_id: String,
    kind: Option<TimelineEventKind>,
) {
    serve_filtered_event_stream(socket, subscription, backlog, move |event| {
        event.runner_id.as_deref() == Some(runner_id.as_str())
            && crate::helpers::event_matches_kind(event, kind)
    })
    .await;
}

pub(crate) async fn serve_runner_approval_stream(
    socket: WebSocket,
    subscription: broadcast::Receiver<TimelineEvent>,
    backlog: Vec<TimelineEvent>,
    runner_id: String,
    kind: Option<TimelineEventKind>,
) {
    serve_filtered_event_stream(socket, subscription, backlog, move |event| {
        event.runner_id.as_deref() == Some(runner_id.as_str())
            && crate::helpers::approval_event_matches(event, kind)
    })
    .await;
}

pub(crate) async fn serve_session_approval_stream(
    socket: WebSocket,
    subscription: broadcast::Receiver<TimelineEvent>,
    backlog: Vec<TimelineEvent>,
    session_id: Uuid,
    kind: Option<TimelineEventKind>,
) {
    serve_filtered_event_stream(socket, subscription, backlog, move |event| {
        event.session_id == Some(session_id) && crate::helpers::approval_event_matches(event, kind)
    })
    .await;
}

pub(crate) async fn serve_filtered_event_stream<F>(
    mut socket: WebSocket,
    mut subscription: broadcast::Receiver<TimelineEvent>,
    backlog: Vec<TimelineEvent>,
    filter: F,
) where
    F: Fn(&TimelineEvent) -> bool,
{
    let mut last_sequence = 0;
    for event in backlog {
        if send_timeline_event(&mut socket, &event).await.is_err() {
            return;
        }
        last_sequence = event.sequence;
    }

    loop {
        let event = match subscription.recv().await {
            Ok(event) => event,
            Err(broadcast::error::RecvError::Closed) => break,
            Err(broadcast::error::RecvError::Lagged(_)) => {
                let _ = socket.close().await;
                break;
            }
        };
        if event.sequence <= last_sequence || !filter(&event) {
            continue;
        }
        if send_timeline_event(&mut socket, &event).await.is_err() {
            break;
        }
        last_sequence = event.sequence;
    }
}

async fn send_timeline_event(
    socket: &mut WebSocket,
    event: &TimelineEvent,
) -> std::result::Result<(), ()> {
    let payload = serde_json::to_string(event).map_err(|_| ())?;
    socket
        .send(Message::Text(payload.into()))
        .await
        .map_err(|_| ())
}
