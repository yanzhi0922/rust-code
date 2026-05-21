use tokio::sync::broadcast;

use crate::types::EngineEvent;

/// Broadcast-backed event stream for the Phase 1 engine event layer.
#[derive(Debug, Clone)]
pub struct EventStream {
    sender: broadcast::Sender<EngineEvent>,
}

impl EventStream {
    /// Create a new event stream with the requested ring buffer size.
    #[must_use]
    pub fn new(buffer: usize) -> Self {
        let (sender, _) = broadcast::channel(buffer.max(1));
        Self { sender }
    }

    /// Subscribe to future engine events.
    pub fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        self.sender.subscribe()
    }

    /// Emit an event and intentionally ignore the no-receiver case.
    pub fn emit(&self, event: EngineEvent) {
        let _ = self.sender.send(event);
    }

    /// Emit an event and return the broadcast result to callers that care.
    pub fn send(
        &self,
        event: EngineEvent,
    ) -> Result<usize, broadcast::error::SendError<EngineEvent>> {
        self.sender.send(event)
    }

    /// Expose receiver count for diagnostics and tests.
    #[must_use]
    pub fn receiver_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{Duration, timeout};
    use uuid::Uuid;

    #[tokio::test]
    async fn broadcast_subscribers_receive_same_event() {
        let stream = EventStream::new(8);
        let mut rx1 = stream.subscribe();
        let mut rx2 = stream.subscribe();
        let event = EngineEvent::QueryStarted {
            session_id: Uuid::new_v4(),
        };

        stream.emit(event.clone());

        let first = timeout(Duration::from_millis(100), rx1.recv())
            .await
            .expect("timeout")
            .expect("recv should succeed");
        let second = timeout(Duration::from_millis(100), rx2.recv())
            .await
            .expect("timeout")
            .expect("recv should succeed");

        assert_eq!(first, event);
        assert_eq!(second, event);
    }

    #[tokio::test]
    async fn emit_without_subscribers_is_non_fatal() {
        let stream = EventStream::new(1);
        stream.emit(EngineEvent::StreamMessageStop);
        assert_eq!(stream.receiver_count(), 0);
    }

    #[test]
    fn zero_buffer_is_coerced_to_a_valid_channel_size() {
        let stream = EventStream::new(0);
        assert_eq!(stream.receiver_count(), 0);
    }
}
