//! Integration tests for the subscription broker public API.
//!
//! Verifies that `Broker`, `SubscriptionMessage`, `subscribe_all`, and `subscribe_stream`
//! are accessible at the crate root (`eventfold_db::`) and work together end-to-end:
//! spawn a writer with a broker, append events across multiple streams, and consume them
//! through subscription streams.

use eventfold_db::{
    Broker, ExpectedVersion, ProposedEvent, Store, SubscriptionMessage, WriterHandle, spawn_writer,
    subscribe_all, subscribe_stream,
};
use futures::StreamExt;

/// Helper: create a `ProposedEvent` with minimal fields for testing.
fn proposed(event_type: &str) -> ProposedEvent {
    ProposedEvent {
        event_id: uuid::Uuid::new_v4(),
        event_type: event_type.to_string(),
        metadata: bytes::Bytes::new(),
        payload: bytes::Bytes::from_static(b"{}"),
    }
}

/// Helper: open a Store in a tempdir and return (store, tempdir).
fn temp_store() -> (Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("failed to create tempdir");
    let path = dir.path().join("events.log");
    let store = Store::open(&path).expect("Store::open should succeed");
    (store, dir)
}

/// Helper: append `n` events to a stream via the writer handle, using sequential
/// expected versions (NoStream for the first, Exact(i-1) for subsequent).
async fn append_n(handle: &WriterHandle, stream_id: uuid::Uuid, n: u64) {
    for i in 0..n {
        let ev = if i == 0 {
            ExpectedVersion::NoStream
        } else {
            ExpectedVersion::Exact(i - 1)
        };
        handle
            .append(stream_id, ev, vec![proposed("TestEvt")])
            .await
            .expect("append should succeed");
    }
}

/// Full subscription flow: append events to two streams, then verify `subscribe_all`
/// returns all events in global-position order, and `subscribe_stream` returns only
/// the targeted stream's events.
///
/// Exercises AC-4 and AC-8 semantics from outside the crate.
#[tokio::test]
async fn subscribe_all_and_subscribe_stream_full_flow() {
    let (store, _dir) = temp_store();
    let broker = Broker::new(64);
    let (handle, read_index, join_handle) = spawn_writer(store, 8, broker.clone());

    let stream_x = uuid::Uuid::new_v4();
    let stream_y = uuid::Uuid::new_v4();

    // Append 3 events to stream X.
    append_n(&handle, stream_x, 3).await;

    // Append 2 events to stream Y.
    append_n(&handle, stream_y, 2).await;

    // --- subscribe_all from position 0 ---
    let all_stream = subscribe_all(read_index.clone(), &broker, 0).await;
    tokio::pin!(all_stream);

    let mut all_positions = Vec::new();
    while let Some(msg) = all_stream.next().await {
        match msg.expect("subscribe_all item should be Ok") {
            SubscriptionMessage::Event(arc_event) => {
                all_positions.push(arc_event.global_position);
            }
            SubscriptionMessage::CaughtUp => break,
        }
    }

    // All 5 events should be received in global-position order.
    assert_eq!(all_positions, vec![0, 1, 2, 3, 4]);

    // --- subscribe_stream for stream X from version 0 ---
    let x_stream = subscribe_stream(read_index, &broker, stream_x, 0).await;
    tokio::pin!(x_stream);

    let mut x_events = Vec::new();
    while let Some(msg) = x_stream.next().await {
        match msg.expect("subscribe_stream item should be Ok") {
            SubscriptionMessage::Event(arc_event) => {
                assert_eq!(
                    arc_event.stream_id, stream_x,
                    "expected stream X, got different stream"
                );
                x_events.push(arc_event.stream_version);
            }
            SubscriptionMessage::CaughtUp => break,
        }
    }

    // Stream X has 3 events with versions 0, 1, 2.
    assert_eq!(x_events, vec![0, 1, 2]);

    // Clean shutdown.
    drop(handle);
    join_handle.await.expect("writer task should exit cleanly");
}

/// Verify `CaughtUp` ordering: when subscribing to a store with pre-existing history,
/// all catch-up events arrive as `Event` variants, followed by a single `CaughtUp`
/// marker, followed by silence (no more messages) until additional events are appended.
#[tokio::test]
async fn caught_up_is_yielded_after_catchup_events_before_live() {
    let (store, _dir) = temp_store();
    let broker = Broker::new(64);
    let (handle, read_index, join_handle) = spawn_writer(store, 8, broker.clone());

    let stream_id = uuid::Uuid::new_v4();

    // Pre-populate with 3 events.
    append_n(&handle, stream_id, 3).await;

    // Start subscription after events exist.
    let stream = subscribe_all(read_index, &broker, 0).await;
    tokio::pin!(stream);

    // Collect all messages until CaughtUp, tracking the order of message types.
    // "E" = Event, "C" = CaughtUp.
    let mut message_types: Vec<&str> = Vec::new();
    let mut event_positions = Vec::new();

    while let Some(msg) = stream.next().await {
        match msg.expect("stream item should be Ok") {
            SubscriptionMessage::Event(arc_event) => {
                message_types.push("E");
                event_positions.push(arc_event.global_position);
            }
            SubscriptionMessage::CaughtUp => {
                message_types.push("C");
                break;
            }
        }
    }

    // Catch-up events must come first, then CaughtUp.
    assert_eq!(
        message_types,
        vec!["E", "E", "E", "C"],
        "expected 3 Events followed by CaughtUp"
    );
    assert_eq!(event_positions, vec![0, 1, 2]);

    // After CaughtUp, the stream should be idle (no more messages) until
    // additional events are appended. Use a short timeout to confirm silence.
    let silence = tokio::time::timeout(std::time::Duration::from_millis(100), stream.next()).await;
    assert!(
        silence.is_err(),
        "expected timeout (silence) after CaughtUp, but received a message"
    );

    // Now append one more event and confirm it arrives as a live event.
    handle
        .append(
            stream_id,
            ExpectedVersion::Exact(2),
            vec![proposed("LiveEvt")],
        )
        .await
        .expect("append should succeed");

    let live_msg = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
        .await
        .expect("should not timeout waiting for live event")
        .expect("stream should yield a message");

    match live_msg.expect("live message should be Ok") {
        SubscriptionMessage::Event(arc_event) => {
            assert_eq!(arc_event.global_position, 3);
            assert_eq!(arc_event.stream_id, stream_id);
        }
        SubscriptionMessage::CaughtUp => {
            panic!("unexpected second CaughtUp during live phase");
        }
    }

    // Clean shutdown.
    drop(handle);
    join_handle.await.expect("writer task should exit cleanly");
}
