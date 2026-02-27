//! gRPC client wrapper for connecting to EventfoldDB.
//!
//! Provides a thin wrapper around the generated gRPC client that maps proto
//! types to the TUI's [`EventRecord`] and [`StreamInfo`] types.

use tokio::sync::mpsc;
use tonic::Streaming;

use eventfold_db::proto::event_store_client::EventStoreClient;
use eventfold_db::proto::{
    ReadAllRequest, ReadAllResponse, ReadStreamRequest, ReadStreamResponse, SubscribeAllRequest,
    SubscribeResponse,
};

use crate::app::{EventRecord, StreamInfo};
use crate::error::ConsoleError;

/// A wrapper around the tonic gRPC client for EventfoldDB.
///
/// Provides high-level methods that return TUI-friendly types rather than
/// raw proto messages.
#[derive(Debug, Clone)]
pub struct Client {
    /// The underlying tonic gRPC client.
    inner: EventStoreClient<tonic::transport::Channel>,
}

impl Client {
    /// Connect to an EventfoldDB server at the given address.
    ///
    /// # Arguments
    ///
    /// * `addr` - The server address (e.g. "http://127.0.0.1:2113").
    ///
    /// # Returns
    ///
    /// A connected `Client`.
    ///
    /// # Errors
    ///
    /// Returns [`ConsoleError::ConnectionFailed`] if the connection cannot be
    /// established.
    pub async fn connect(addr: &str) -> Result<Self, ConsoleError> {
        let inner = EventStoreClient::connect(addr.to_string())
            .await
            .map_err(|e| ConsoleError::ConnectionFailed(e.to_string()))?;
        Ok(Self { inner })
    }

    /// Read all events from the global log, paginated.
    ///
    /// # Arguments
    ///
    /// * `from_position` - Starting global position.
    /// * `max_count` - Maximum number of events to return.
    ///
    /// # Returns
    ///
    /// A vector of [`EventRecord`] in global position order.
    ///
    /// # Errors
    ///
    /// Returns [`ConsoleError::Grpc`] on server or transport errors.
    pub async fn read_all(
        &mut self,
        from_position: u64,
        max_count: u64,
    ) -> Result<Vec<EventRecord>, ConsoleError> {
        let resp: ReadAllResponse = self
            .inner
            .read_all(ReadAllRequest {
                from_position,
                max_count,
            })
            .await?
            .into_inner();
        Ok(resp.events.into_iter().map(proto_to_event_record).collect())
    }

    /// Read events from a specific stream, paginated.
    ///
    /// # Arguments
    ///
    /// * `stream_id` - The stream UUID string.
    /// * `from_version` - Starting stream version.
    /// * `max_count` - Maximum number of events to return.
    ///
    /// # Returns
    ///
    /// A vector of [`EventRecord`] in stream version order.
    ///
    /// # Errors
    ///
    /// Returns [`ConsoleError::Grpc`] on server or transport errors.
    pub async fn read_stream(
        &mut self,
        stream_id: &str,
        from_version: u64,
        max_count: u64,
    ) -> Result<Vec<EventRecord>, ConsoleError> {
        let resp: ReadStreamResponse = self
            .inner
            .read_stream(ReadStreamRequest {
                stream_id: stream_id.to_string(),
                from_version,
                max_count,
            })
            .await?
            .into_inner();
        Ok(resp.events.into_iter().map(proto_to_event_record).collect())
    }

    /// Derive a list of all streams by scanning the entire global log.
    ///
    /// Since there is no `ListStreams` RPC, this fetches all events in pages
    /// of `page_size` and collects unique stream IDs with counts.
    ///
    /// # Arguments
    ///
    /// * `page_size` - Number of events per page during the scan.
    ///
    /// # Returns
    ///
    /// A vector of [`StreamInfo`] sorted by stream ID.
    ///
    /// # Errors
    ///
    /// Returns [`ConsoleError::Grpc`] on server or transport errors.
    pub async fn list_streams(&mut self, page_size: u64) -> Result<Vec<StreamInfo>, ConsoleError> {
        let mut all_events = Vec::new();
        let mut from_position = 0u64;
        loop {
            let page = self.read_all(from_position, page_size).await?;
            if page.is_empty() {
                break;
            }
            from_position += page.len() as u64;
            all_events.extend(page);
        }
        Ok(crate::app::AppState::collect_streams(&all_events))
    }

    /// Open a `SubscribeAll` streaming subscription.
    ///
    /// Returns a tonic `Streaming` that yields `SubscribeResponse` messages.
    /// The caller is responsible for reading from the stream and handling
    /// `Event` and `CaughtUp` variants.
    ///
    /// # Arguments
    ///
    /// * `from_position` - Starting global position for the catch-up phase.
    ///
    /// # Returns
    ///
    /// A streaming response.
    ///
    /// # Errors
    ///
    /// Returns [`ConsoleError::Grpc`] on server or transport errors.
    pub async fn subscribe_all(
        &mut self,
        from_position: u64,
    ) -> Result<Streaming<SubscribeResponse>, ConsoleError> {
        let stream = self
            .inner
            .subscribe_all(SubscribeAllRequest { from_position })
            .await?
            .into_inner();
        Ok(stream)
    }
}

/// Message sent from the subscription background task to the render loop.
#[derive(Debug)]
pub enum SubscriptionMsg {
    /// A new event was received from the subscription.
    Event(EventRecord),
    /// The subscription has caught up with historical events.
    CaughtUp,
    /// The subscription encountered an error.
    Error(String),
}

/// Spawn a background task that reads from a `SubscribeAll` stream and sends
/// events to the render loop via an mpsc channel.
///
/// The task runs until the subscription stream ends or the channel is closed.
///
/// # Arguments
///
/// * `mut client` - A connected client (will be consumed).
/// * `from_position` - Starting position for the subscription.
/// * `tx` - Sender end of the mpsc channel.
pub async fn spawn_subscription(
    mut client: Client,
    from_position: u64,
    tx: mpsc::Sender<SubscriptionMsg>,
) {
    let stream = match client.subscribe_all(from_position).await {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(SubscriptionMsg::Error(e.to_string())).await;
            return;
        }
    };

    let mut stream = stream;
    while let Ok(Some(resp)) = stream.message().await.map(Some).or_else(|e| {
        // If the stream errors, report it and stop.
        let _ = tx.blocking_send(SubscriptionMsg::Error(e.to_string()));
        Err(e)
    }) {
        let Some(resp) = resp else { break };
        match resp.content {
            Some(eventfold_db::proto::subscribe_response::Content::Event(proto_event)) => {
                let event = proto_to_event_record(proto_event);
                if tx.send(SubscriptionMsg::Event(event)).await.is_err() {
                    return; // channel closed, render loop exited
                }
            }
            Some(eventfold_db::proto::subscribe_response::Content::CaughtUp(_)) => {
                if tx.send(SubscriptionMsg::CaughtUp).await.is_err() {
                    return;
                }
            }
            None => {}
        }
    }
}

/// Convert a proto `RecordedEvent` to the TUI's [`EventRecord`].
fn proto_to_event_record(p: eventfold_db::proto::RecordedEvent) -> EventRecord {
    EventRecord {
        event_id: p.event_id,
        stream_id: p.stream_id,
        stream_version: p.stream_version,
        global_position: p.global_position,
        event_type: p.event_type,
        metadata: p.metadata,
        payload: p.payload,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- proto_to_event_record --

    #[test]
    fn proto_to_event_record_maps_all_fields() {
        let proto = eventfold_db::proto::RecordedEvent {
            event_id: "eid-1".into(),
            stream_id: "sid-1".into(),
            stream_version: 5,
            global_position: 42,
            event_type: "OrderPlaced".into(),
            metadata: vec![1, 2, 3],
            payload: vec![4, 5, 6],
        };
        let record = proto_to_event_record(proto);
        assert_eq!(record.event_id, "eid-1");
        assert_eq!(record.stream_id, "sid-1");
        assert_eq!(record.stream_version, 5);
        assert_eq!(record.global_position, 42);
        assert_eq!(record.event_type, "OrderPlaced");
        assert_eq!(record.metadata, vec![1, 2, 3]);
        assert_eq!(record.payload, vec![4, 5, 6]);
    }

    // -- Client is Debug and Clone --

    #[test]
    fn client_is_debug() {
        // We can't construct a real client without a server, but we can verify
        // the type implements Debug by checking the derive attribute compiles.
        // This test is a compile-time check.
        fn _assert_debug<T: std::fmt::Debug>() {}
        _assert_debug::<Client>();
    }

    #[test]
    fn client_is_clone() {
        fn _assert_clone<T: Clone>() {}
        _assert_clone::<Client>();
    }

    // -- SubscriptionMsg variants --

    #[test]
    fn subscription_msg_event_debug() {
        let event = EventRecord {
            event_id: "eid".into(),
            stream_id: "sid".into(),
            stream_version: 0,
            global_position: 0,
            event_type: "Test".into(),
            metadata: vec![],
            payload: vec![],
        };
        let msg = SubscriptionMsg::Event(event);
        let debug = format!("{msg:?}");
        assert!(!debug.is_empty());
    }

    #[test]
    fn subscription_msg_caught_up_debug() {
        let msg = SubscriptionMsg::CaughtUp;
        let debug = format!("{msg:?}");
        assert!(debug.contains("CaughtUp"));
    }

    #[test]
    fn subscription_msg_error_debug() {
        let msg = SubscriptionMsg::Error("test error".into());
        let debug = format!("{msg:?}");
        assert!(debug.contains("test error"));
    }
}
