//! Broadcast broker for live event subscriptions.
//!
//! The `Broker` wraps a `tokio::broadcast` channel that carries `Arc<RecordedEvent>` messages.
//! The writer task publishes newly appended events after fsync, and all active subscribers
//! receive them. Using `Arc` ensures that events are shared across subscribers without
//! deep-cloning the event data.

use std::sync::Arc;

use async_stream::stream;
use tokio::sync::broadcast;

use uuid::Uuid;

use crate::error::Error;
use crate::reader::ReadIndex;
use crate::types::{RecordedEvent, SubscriptionMessage};

/// Broadcast broker for pushing newly appended events to live subscribers.
///
/// The `Broker` holds the sending half of a `tokio::broadcast` channel. Each call to
/// [`publish`](Broker::publish) wraps the events in `Arc` and sends them to all active
/// receivers. Subscribers obtain a receiver via [`subscribe`](Broker::subscribe).
///
/// # Design
///
/// The broadcast channel clones every message to every receiver. Wrapping events in `Arc`
/// ensures that all subscribers share the same heap allocation rather than deep-cloning
/// event data (payload, metadata, event type).
#[derive(Clone)]
pub struct Broker {
    tx: broadcast::Sender<Arc<RecordedEvent>>,
}

impl Broker {
    /// Create a new broker with the given broadcast channel capacity.
    ///
    /// # Arguments
    ///
    /// * `capacity` - Maximum number of events the broadcast channel can buffer before
    ///   lagging subscribers are dropped. Must be greater than zero.
    pub fn new(capacity: usize) -> Self {
        // `broadcast::channel` returns (Sender, Receiver). We discard the initial receiver
        // because subscribers obtain their own via `subscribe()`.
        let (tx, _rx) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Publish events to all active subscribers.
    ///
    /// Each event is wrapped in `Arc::new` before sending so that all subscribers share the
    /// same allocation. If no subscribers are currently active, the send error is logged as
    /// a warning (not a fatal error) because publishing to an empty channel is expected
    /// during startup or when no clients are connected.
    ///
    /// # Arguments
    ///
    /// * `events` - Slice of recorded events to publish. Typically called by the writer
    ///   task after a successful append + fsync.
    pub fn publish(&self, events: &[RecordedEvent]) {
        for event in events {
            let arc_event = Arc::new(event.clone());
            if let Err(_err) = self.tx.send(arc_event) {
                tracing::warn!("broker publish: no active receivers");
            }
        }
    }

    /// Create a new broadcast receiver for live events.
    ///
    /// The returned receiver will receive all events published after this call. Events
    /// published before subscription are not replayed -- catch-up logic is handled
    /// separately by the subscription stream functions.
    ///
    /// # Returns
    ///
    /// A `broadcast::Receiver<Arc<RecordedEvent>>` that yields events as they are published.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<RecordedEvent>> {
        self.tx.subscribe()
    }
}

/// Number of events to read per batch during the catch-up phase of a subscription.
///
/// Catch-up reads events from the in-memory index in chunks of this size rather than
/// loading the entire history at once. This keeps memory bounded for large catch-up ranges.
const CATCHUP_BATCH_SIZE: u64 = 500;

/// Create an async stream that replays historical events (catch-up), emits a `CaughtUp`
/// marker, then forwards live events from the broadcast channel.
///
/// The broadcast receiver is registered **before** any historical read begins, preventing
/// a race where events appended between the end of catch-up and the start of live listening
/// would be lost.
///
/// # Arguments
///
/// * `read_index` - Shared read-only handle to the in-memory event log.
/// * `broker` - Reference to the broadcast broker for subscribing to live events.
/// * `from_position` - Zero-based global position to start the catch-up replay from.
///
/// # Returns
///
/// A stream yielding `Result<SubscriptionMessage, Error>`. The stream yields `Event` variants
/// during catch-up and live phases, a single `CaughtUp` marker between them, and terminates
/// with `Err(Error::InvalidArgument(...))` if the broadcast receiver lags.
///
/// # Errors
///
/// Yields `Error::InvalidArgument` if the broadcast receiver falls behind and the channel
/// reports a lag. The consumer should re-subscribe from their last processed position.
pub async fn subscribe_all(
    read_index: ReadIndex,
    broker: &Broker,
    from_position: u64,
) -> impl futures_core::Stream<Item = Result<SubscriptionMessage, Error>> {
    // Step 1: Register broadcast receiver BEFORE reading history.
    let mut rx = broker.subscribe();

    stream! {
        // Step 2: Catch-up phase -- read historical events in batches.
        let mut cursor = from_position;
        let mut last_catchup_position: Option<u64> = None;

        loop {
            let batch = read_index.read_all(cursor, CATCHUP_BATCH_SIZE);
            let batch_len = batch.len() as u64;

            for event in batch {
                last_catchup_position = Some(event.global_position);
                cursor = event.global_position + 1;
                yield Ok(SubscriptionMessage::Event(Arc::new(event)));
            }

            // When a batch returns fewer than CATCHUP_BATCH_SIZE events, we've
            // reached the head of the log.
            if batch_len < CATCHUP_BATCH_SIZE {
                break;
            }
        }

        // Step 3: Emit CaughtUp marker.
        yield Ok(SubscriptionMessage::CaughtUp);

        // Step 4: Live phase -- drain broadcast receiver with deduplication.
        loop {
            match rx.recv().await {
                Ok(arc_event) => {
                    // Deduplication: skip events already sent during catch-up.
                    if let Some(last_pos) = last_catchup_position
                        && arc_event.global_position <= last_pos
                    {
                        continue;
                    }
                    yield Ok(SubscriptionMessage::Event(arc_event));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    // Step 5: Lag termination.
                    yield Err(Error::InvalidArgument(
                        "subscription lagged: re-subscribe from last checkpoint".into(),
                    ));
                    return;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    // Broker shut down -- end the stream.
                    return;
                }
            }
        }
    }
}

/// Create an async stream that replays historical events for a single stream (catch-up),
/// emits a `CaughtUp` marker, then forwards live events filtered by `stream_id`.
///
/// The broadcast receiver is registered **before** any historical read begins, preventing
/// a race where events appended between the end of catch-up and the start of live listening
/// would be lost.
///
/// If the stream does not exist, the catch-up phase terminates immediately and `CaughtUp`
/// is yielded with no preceding events.
///
/// # Arguments
///
/// * `read_index` - Shared read-only handle to the in-memory event log.
/// * `broker` - Reference to the broadcast broker for subscribing to live events.
/// * `stream_id` - UUID of the stream to subscribe to.
/// * `from_version` - Zero-based stream version to start the catch-up replay from.
///
/// # Returns
///
/// A stream yielding `Result<SubscriptionMessage, Error>`. The stream yields `Event` variants
/// during catch-up and live phases, a single `CaughtUp` marker between them, and terminates
/// with `Err(Error::InvalidArgument(...))` if the broadcast receiver lags.
///
/// # Errors
///
/// Yields `Error::InvalidArgument` if the broadcast receiver falls behind and the channel
/// reports a lag. The consumer should re-subscribe from their last processed stream version.
pub async fn subscribe_stream(
    read_index: ReadIndex,
    broker: &Broker,
    stream_id: Uuid,
    from_version: u64,
) -> impl futures_core::Stream<Item = Result<SubscriptionMessage, Error>> {
    // Step 1: Register broadcast receiver BEFORE reading history.
    let mut rx = broker.subscribe();

    stream! {
        // Step 2: Catch-up phase -- read historical events for this stream in batches.
        let mut cursor = from_version;
        let mut last_catchup_version: Option<u64> = None;

        // Attempt to read stream events; if the stream doesn't exist, skip catch-up.
        loop {
            match read_index.read_stream(stream_id, cursor, CATCHUP_BATCH_SIZE) {
                Ok(batch) => {
                    let batch_len = batch.len() as u64;

                    for event in batch {
                        last_catchup_version = Some(event.stream_version);
                        cursor = event.stream_version + 1;
                        yield Ok(SubscriptionMessage::Event(Arc::new(event)));
                    }

                    // When a batch returns fewer than CATCHUP_BATCH_SIZE events,
                    // we've reached the head of this stream's log.
                    if batch_len < CATCHUP_BATCH_SIZE {
                        break;
                    }
                }
                Err(Error::StreamNotFound { .. }) => {
                    // Stream does not exist -- no catch-up events.
                    break;
                }
                Err(e) => {
                    // Unexpected error during catch-up -- propagate and end.
                    yield Err(e);
                    return;
                }
            }
        }

        // Step 3: Emit CaughtUp marker.
        yield Ok(SubscriptionMessage::CaughtUp);

        // Step 4: Live phase -- drain broadcast receiver, filtering by stream_id.
        loop {
            match rx.recv().await {
                Ok(arc_event) => {
                    // Filter: only events for this stream.
                    if arc_event.stream_id != stream_id {
                        continue;
                    }

                    // Deduplication: skip events already sent during catch-up.
                    if let Some(last_ver) = last_catchup_version
                        && arc_event.stream_version <= last_ver
                    {
                        continue;
                    }

                    yield Ok(SubscriptionMessage::Event(arc_event));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    // Step 5: Lag termination.
                    yield Err(Error::InvalidArgument(
                        "subscription lagged: re-subscribe from last checkpoint".into(),
                    ));
                    return;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    // Broker shut down -- end the stream.
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{RecordedEvent, SubscriptionMessage};
    use bytes::Bytes;
    use futures::StreamExt;
    use uuid::Uuid;

    /// Helper to create a test `RecordedEvent` with a given event type and global position.
    fn make_event(event_type: &str, global_position: u64) -> RecordedEvent {
        RecordedEvent {
            event_id: Uuid::new_v4(),
            stream_id: Uuid::new_v4(),
            stream_version: 0,
            global_position,
            recorded_at: 0,
            event_type: event_type.to_string(),
            metadata: Bytes::new(),
            payload: Bytes::from_static(b"{}"),
        }
    }

    // AC-1: Broker publish and receive -- 3 events arrive with correct event_type values.
    #[tokio::test]
    async fn publish_three_events_received_by_subscriber() {
        let broker = Broker::new(16);
        let mut rx = broker.subscribe();

        let events = vec![
            make_event("EventA", 0),
            make_event("EventB", 1),
            make_event("EventC", 2),
        ];
        broker.publish(&events);

        let a = rx.recv().await.expect("should receive first event");
        let b = rx.recv().await.expect("should receive second event");
        let c = rx.recv().await.expect("should receive third event");

        assert_eq!(a.event_type, "EventA");
        assert_eq!(b.event_type, "EventB");
        assert_eq!(c.event_type, "EventC");
    }

    // AC-2: Multiple subscribers each receive all published events.
    #[tokio::test]
    async fn multiple_subscribers_each_receive_all_events() {
        let broker = Broker::new(16);
        let mut rx1 = broker.subscribe();
        let mut rx2 = broker.subscribe();

        let events = vec![make_event("Alpha", 0), make_event("Beta", 1)];
        broker.publish(&events);

        // Both receivers should get exactly 2 events.
        let rx1_a = rx1.recv().await.expect("rx1 first");
        let rx1_b = rx1.recv().await.expect("rx1 second");
        let rx2_a = rx2.recv().await.expect("rx2 first");
        let rx2_b = rx2.recv().await.expect("rx2 second");

        assert_eq!(rx1_a.event_type, "Alpha");
        assert_eq!(rx1_b.event_type, "Beta");
        assert_eq!(rx2_a.event_type, "Alpha");
        assert_eq!(rx2_b.event_type, "Beta");
    }

    // AC-3: Lagged subscriber receives RecvError::Lagged when buffer overflows.
    #[tokio::test]
    async fn lagged_subscriber_receives_lagged_error() {
        let broker = Broker::new(2);
        let mut rx = broker.subscribe();

        // Publish 5 events without receiving -- exceeds capacity of 2.
        let events: Vec<RecordedEvent> = (0..5).map(|i| make_event("Overflow", i)).collect();
        broker.publish(&events);

        // The receiver should eventually return a Lagged error because the oldest
        // messages were dropped to make room for newer ones.
        let mut got_lagged = false;
        for _ in 0..6 {
            match rx.recv().await {
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    got_lagged = true;
                    break;
                }
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
        assert!(got_lagged, "expected a Lagged error from the receiver");
    }

    // AC-12: Arc sharing -- both subscribers receive the same heap allocation.
    #[tokio::test]
    async fn arc_sharing_across_subscribers() {
        let broker = Broker::new(16);
        let mut rx1 = broker.subscribe();
        let mut rx2 = broker.subscribe();

        broker.publish(&[make_event("Shared", 0)]);

        let arc1 = rx1.recv().await.expect("rx1 should receive");
        let arc2 = rx2.recv().await.expect("rx2 should receive");

        // Both receivers must hold pointers to the same Arc allocation.
        assert!(
            Arc::ptr_eq(&arc1, &arc2),
            "expected both subscribers to share the same Arc allocation"
        );
    }

    // --- subscribe_all helper ---

    /// Helper: create a `ProposedEvent` with minimal fields for testing.
    fn proposed(event_type: &str) -> crate::types::ProposedEvent {
        crate::types::ProposedEvent {
            event_id: Uuid::new_v4(),
            event_type: event_type.to_string(),
            metadata: Bytes::new(),
            payload: Bytes::from_static(b"{}"),
        }
    }

    /// Helper: open a Store at a temp dir and return (store, tempdir).
    fn temp_store() -> (crate::store::Store, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");
        let store = crate::store::Store::open(&path).expect("open should succeed");
        (store, dir)
    }

    // AC-4: subscribe_all -- catch-up only. Append 5 events, subscribe from 0,
    // collect until CaughtUp. Expect 5 Event variants at positions 0..4.
    #[tokio::test]
    async fn ac4_subscribe_all_catchup_only() {
        let (store, _dir) = temp_store();
        let broker = Broker::new(64);
        let (handle, read_index, join_handle) = crate::writer::spawn_writer(
            store,
            8,
            broker.clone(),
            std::num::NonZeroUsize::new(128).expect("nonzero"),
        );

        let stream_id = Uuid::new_v4();
        for i in 0u64..5 {
            let ev = if i == 0 {
                crate::types::ExpectedVersion::NoStream
            } else {
                crate::types::ExpectedVersion::Exact(i - 1)
            };
            handle
                .append(stream_id, ev, vec![proposed("TestEvt")])
                .await
                .expect("append should succeed");
        }

        let stream = subscribe_all(read_index, &broker, 0).await;
        tokio::pin!(stream);

        let mut positions = Vec::new();
        while let Some(msg) = stream.next().await {
            match msg.expect("stream item should be Ok") {
                SubscriptionMessage::Event(arc_event) => {
                    positions.push(arc_event.global_position);
                }
                SubscriptionMessage::CaughtUp => break,
            }
        }

        assert_eq!(positions, vec![0, 1, 2, 3, 4]);

        drop(handle);
        join_handle.await.expect("writer task should exit cleanly");
    }

    // AC-5: subscribe_all -- catch-up from middle. Append 10 events, subscribe from 5,
    // collect until CaughtUp. Expect 5 Event variants at positions 5..9.
    #[tokio::test]
    async fn ac5_subscribe_all_catchup_from_middle() {
        let (store, _dir) = temp_store();
        let broker = Broker::new(64);
        let (handle, read_index, join_handle) = crate::writer::spawn_writer(
            store,
            8,
            broker.clone(),
            std::num::NonZeroUsize::new(128).expect("nonzero"),
        );

        let stream_id = Uuid::new_v4();
        for i in 0u64..10 {
            let ev = if i == 0 {
                crate::types::ExpectedVersion::NoStream
            } else {
                crate::types::ExpectedVersion::Exact(i - 1)
            };
            handle
                .append(stream_id, ev, vec![proposed("TestEvt")])
                .await
                .expect("append should succeed");
        }

        let stream = subscribe_all(read_index, &broker, 5).await;
        tokio::pin!(stream);

        let mut positions = Vec::new();
        while let Some(msg) = stream.next().await {
            match msg.expect("stream item should be Ok") {
                SubscriptionMessage::Event(arc_event) => {
                    positions.push(arc_event.global_position);
                }
                SubscriptionMessage::CaughtUp => break,
            }
        }

        assert_eq!(positions, vec![5, 6, 7, 8, 9]);

        drop(handle);
        join_handle.await.expect("writer task should exit cleanly");
    }

    // AC-6: subscribe_all -- live events after catch-up. Append 3 events, subscribe,
    // drive until CaughtUp, then append 2 more. The 2 new events should arrive live
    // with global positions 3 and 4 (no gap, no duplicate).
    #[tokio::test]
    async fn ac6_subscribe_all_live_after_catchup() {
        let (store, _dir) = temp_store();
        let broker = Broker::new(64);
        let (handle, read_index, join_handle) = crate::writer::spawn_writer(
            store,
            8,
            broker.clone(),
            std::num::NonZeroUsize::new(128).expect("nonzero"),
        );

        let stream_id = Uuid::new_v4();
        for i in 0u64..3 {
            let ev = if i == 0 {
                crate::types::ExpectedVersion::NoStream
            } else {
                crate::types::ExpectedVersion::Exact(i - 1)
            };
            handle
                .append(stream_id, ev, vec![proposed("TestEvt")])
                .await
                .expect("append should succeed");
        }

        let stream = subscribe_all(read_index, &broker, 0).await;
        tokio::pin!(stream);

        // Drain until CaughtUp.
        let mut catchup_positions = Vec::new();
        while let Some(msg) = stream.next().await {
            match msg.expect("stream item should be Ok") {
                SubscriptionMessage::Event(arc_event) => {
                    catchup_positions.push(arc_event.global_position);
                }
                SubscriptionMessage::CaughtUp => break,
            }
        }
        assert_eq!(catchup_positions, vec![0, 1, 2]);

        // Append 2 more events.
        handle
            .append(
                stream_id,
                crate::types::ExpectedVersion::Exact(2),
                vec![proposed("LiveEvt")],
            )
            .await
            .expect("append should succeed");
        handle
            .append(
                stream_id,
                crate::types::ExpectedVersion::Exact(3),
                vec![proposed("LiveEvt")],
            )
            .await
            .expect("append should succeed");

        // Drive the stream for 2 more messages (with timeout).
        let mut live_positions = Vec::new();
        for _ in 0..2 {
            let msg = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
                .await
                .expect("should not timeout")
                .expect("stream should yield");
            match msg.expect("stream item should be Ok") {
                SubscriptionMessage::Event(arc_event) => {
                    live_positions.push(arc_event.global_position);
                }
                SubscriptionMessage::CaughtUp => {
                    panic!("unexpected CaughtUp during live phase");
                }
            }
        }

        assert_eq!(live_positions, vec![3, 4]);

        drop(handle);
        join_handle.await.expect("writer task should exit cleanly");
    }

    // AC-7: subscribe_all -- no duplicates during transition. Start subscription on
    // empty store, then concurrently append 5 events. Collect all messages until CaughtUp.
    // Verify exactly 5 unique Event variants with no repeated global_position values.
    #[tokio::test]
    async fn ac7_subscribe_all_no_duplicates_during_transition() {
        let (store, _dir) = temp_store();
        let broker = Broker::new(64);
        let (handle, read_index, join_handle) = crate::writer::spawn_writer(
            store,
            8,
            broker.clone(),
            std::num::NonZeroUsize::new(128).expect("nonzero"),
        );

        // Start subscription on empty store.
        let stream = subscribe_all(read_index, &broker, 0).await;
        tokio::pin!(stream);

        // Concurrently append 5 events.
        let stream_id = Uuid::new_v4();
        for i in 0u64..5 {
            let ev = if i == 0 {
                crate::types::ExpectedVersion::NoStream
            } else {
                crate::types::ExpectedVersion::Exact(i - 1)
            };
            handle
                .append(stream_id, ev, vec![proposed("ConcEvt")])
                .await
                .expect("append should succeed");
        }

        // Collect all messages. CaughtUp may appear anywhere in the sequence.
        // After CaughtUp we still need to collect any remaining events from the
        // live phase, so we continue until we have 5 unique events.
        let mut seen_positions = std::collections::HashSet::new();
        let mut got_caught_up = false;

        // We use a timeout to avoid hanging forever if the logic is wrong.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if seen_positions.len() == 5 && got_caught_up {
                break;
            }
            let msg = tokio::time::timeout_at(deadline, stream.next())
                .await
                .expect("should not timeout")
                .expect("stream should yield");
            match msg.expect("stream item should be Ok") {
                SubscriptionMessage::Event(arc_event) => {
                    let was_new = seen_positions.insert(arc_event.global_position);
                    assert!(
                        was_new,
                        "duplicate global_position: {}",
                        arc_event.global_position
                    );
                }
                SubscriptionMessage::CaughtUp => {
                    got_caught_up = true;
                }
            }
        }

        assert_eq!(seen_positions.len(), 5, "expected exactly 5 unique events");
        // Verify positions are 0..4.
        let expected: std::collections::HashSet<u64> = (0..5).collect();
        assert_eq!(seen_positions, expected);

        drop(handle);
        join_handle.await.expect("writer task should exit cleanly");
    }

    // AC-8: subscribe_stream -- catch-up with interleaved streams. Append events to
    // streams A, B, A, B, A; subscribe to stream A from version 0; collect until CaughtUp.
    // Expect exactly 3 Event variants with stream_id == A and stream_version 0, 1, 2.
    #[tokio::test]
    async fn ac8_subscribe_stream_catchup_interleaved() {
        let (store, _dir) = temp_store();
        let broker = Broker::new(64);
        let (handle, read_index, join_handle) = crate::writer::spawn_writer(
            store,
            8,
            broker.clone(),
            std::num::NonZeroUsize::new(128).expect("nonzero"),
        );

        let stream_a = Uuid::new_v4();
        let stream_b = Uuid::new_v4();

        // Append interleaved: A, B, A, B, A
        let interleave = [
            (stream_a, "EvtA0"),
            (stream_b, "EvtB0"),
            (stream_a, "EvtA1"),
            (stream_b, "EvtB1"),
            (stream_a, "EvtA2"),
        ];

        for (stream_id, event_type) in &interleave {
            let version = match read_index.stream_version(stream_id) {
                Some(v) => crate::types::ExpectedVersion::Exact(v),
                None => crate::types::ExpectedVersion::NoStream,
            };
            handle
                .append(*stream_id, version, vec![proposed(event_type)])
                .await
                .expect("append should succeed");
        }

        let stream = subscribe_stream(read_index, &broker, stream_a, 0).await;
        tokio::pin!(stream);

        let mut versions = Vec::new();
        while let Some(msg) = stream.next().await {
            match msg.expect("stream item should be Ok") {
                SubscriptionMessage::Event(arc_event) => {
                    assert_eq!(
                        arc_event.stream_id, stream_a,
                        "expected stream A, got different stream"
                    );
                    versions.push(arc_event.stream_version);
                }
                SubscriptionMessage::CaughtUp => break,
            }
        }

        assert_eq!(versions, vec![0, 1, 2]);

        drop(handle);
        join_handle.await.expect("writer task should exit cleanly");
    }

    // AC-9: subscribe_stream -- live filtering. Start on empty store, drive until CaughtUp,
    // then append to stream B, stream A, stream B. Only stream A's event should arrive.
    #[tokio::test]
    async fn ac9_subscribe_stream_live_filtering() {
        let (store, _dir) = temp_store();
        let broker = Broker::new(64);
        let (handle, read_index, join_handle) = crate::writer::spawn_writer(
            store,
            8,
            broker.clone(),
            std::num::NonZeroUsize::new(128).expect("nonzero"),
        );

        let stream_a = Uuid::new_v4();
        let stream_b = Uuid::new_v4();

        // Subscribe to stream A on an empty store.
        let stream = subscribe_stream(read_index, &broker, stream_a, 0).await;
        tokio::pin!(stream);

        // Drive until CaughtUp (should be immediate since store is empty).
        let mut catchup_events = Vec::new();
        while let Some(msg) = stream.next().await {
            match msg.expect("stream item should be Ok") {
                SubscriptionMessage::Event(arc_event) => {
                    catchup_events.push(arc_event);
                }
                SubscriptionMessage::CaughtUp => break,
            }
        }
        assert!(
            catchup_events.is_empty(),
            "expected no catch-up events on empty store"
        );

        // Append: stream B, stream A, stream B (3 appends total).
        handle
            .append(
                stream_b,
                crate::types::ExpectedVersion::NoStream,
                vec![proposed("EvtB0")],
            )
            .await
            .expect("append B should succeed");

        handle
            .append(
                stream_a,
                crate::types::ExpectedVersion::NoStream,
                vec![proposed("EvtA0")],
            )
            .await
            .expect("append A should succeed");

        handle
            .append(
                stream_b,
                crate::types::ExpectedVersion::Exact(0),
                vec![proposed("EvtB1")],
            )
            .await
            .expect("append B should succeed");

        // Drive the stream for 1 live event (with timeout). Only stream A's event should come.
        let msg = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .expect("should not timeout")
            .expect("stream should yield");
        match msg.expect("stream item should be Ok") {
            SubscriptionMessage::Event(arc_event) => {
                assert_eq!(arc_event.stream_id, stream_a);
                assert_eq!(arc_event.event_type, "EvtA0");
                assert_eq!(arc_event.stream_version, 0);
            }
            SubscriptionMessage::CaughtUp => panic!("unexpected CaughtUp during live phase"),
        }

        drop(handle);
        join_handle.await.expect("writer task should exit cleanly");
    }

    // AC-10: subscribe_stream -- non-existent stream. Subscribe to a stream with no events,
    // collect until CaughtUp (expect zero Event variants). Then append one event to that
    // stream; drive the stream; assert it arrives with stream_version == 0.
    #[tokio::test]
    async fn ac10_subscribe_stream_nonexistent_then_live() {
        let (store, _dir) = temp_store();
        let broker = Broker::new(64);
        let (handle, read_index, join_handle) = crate::writer::spawn_writer(
            store,
            8,
            broker.clone(),
            std::num::NonZeroUsize::new(128).expect("nonzero"),
        );

        let stream_id = Uuid::new_v4();

        // Subscribe to a stream that does not exist.
        let stream = subscribe_stream(read_index, &broker, stream_id, 0).await;
        tokio::pin!(stream);

        // Drive until CaughtUp. Expect zero Event variants.
        let mut catchup_count = 0u64;
        while let Some(msg) = stream.next().await {
            match msg.expect("stream item should be Ok") {
                SubscriptionMessage::Event(_) => {
                    catchup_count += 1;
                }
                SubscriptionMessage::CaughtUp => break,
            }
        }
        assert_eq!(
            catchup_count, 0,
            "expected zero catch-up events for non-existent stream"
        );

        // Now append one event to that stream.
        handle
            .append(
                stream_id,
                crate::types::ExpectedVersion::NoStream,
                vec![proposed("FirstEvt")],
            )
            .await
            .expect("append should succeed");

        // Drive the stream for 1 live event.
        let msg = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .expect("should not timeout")
            .expect("stream should yield");
        match msg.expect("stream item should be Ok") {
            SubscriptionMessage::Event(arc_event) => {
                assert_eq!(arc_event.stream_id, stream_id);
                assert_eq!(arc_event.stream_version, 0);
                assert_eq!(arc_event.event_type, "FirstEvt");
            }
            SubscriptionMessage::CaughtUp => panic!("unexpected CaughtUp during live phase"),
        }

        drop(handle);
        join_handle.await.expect("writer task should exit cleanly");
    }

    // AC-11: subscribe_all -- lag termination. Create broker with capacity 4,
    // start subscription on empty store, append 10 events without polling the
    // stream, then poll. The stream should yield Err(Error::InvalidArgument)
    // and then None (stream ends).
    #[tokio::test]
    async fn ac11_subscribe_all_lag_termination() {
        let (store, _dir) = temp_store();
        let broker = Broker::new(4);
        let (handle, read_index, join_handle) = crate::writer::spawn_writer(
            store,
            8,
            broker.clone(),
            std::num::NonZeroUsize::new(128).expect("nonzero"),
        );

        // Start subscription on empty store (catch-up is instant).
        let stream = subscribe_all(read_index, &broker, 0).await;
        tokio::pin!(stream);

        // Append 10 events WITHOUT polling the subscription stream.
        // This saturates the broadcast buffer (capacity=4).
        let stream_id = Uuid::new_v4();
        for i in 0u64..10 {
            let ev = if i == 0 {
                crate::types::ExpectedVersion::NoStream
            } else {
                crate::types::ExpectedVersion::Exact(i - 1)
            };
            handle
                .append(stream_id, ev, vec![proposed("LagEvt")])
                .await
                .expect("append should succeed");
        }

        // Now poll the stream. It should:
        // 1. Do catch-up (empty store at subscribe time, but events were appended
        //    after -- catch-up reads the current index, so it may see some events).
        // 2. Emit CaughtUp.
        // 3. Start draining broadcast receiver which will have lagged.
        // 4. Yield Err(Error::InvalidArgument(...)).
        // 5. Return None (stream ends).
        let mut got_error = false;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);

        loop {
            let item = tokio::time::timeout_at(deadline, stream.next())
                .await
                .expect("should not timeout");

            match item {
                Some(Ok(SubscriptionMessage::Event(_))) => continue,
                Some(Ok(SubscriptionMessage::CaughtUp)) => continue,
                Some(Err(ref e)) => {
                    assert!(
                        matches!(e, crate::error::Error::InvalidArgument(_)),
                        "expected InvalidArgument error, got: {e:?}"
                    );
                    got_error = true;
                }
                None => break, // Stream ended
            }
        }

        assert!(got_error, "expected the stream to yield a lag error");

        drop(handle);
        join_handle.await.expect("writer task should exit cleanly");
    }
}
