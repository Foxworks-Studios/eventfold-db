//! Integration tests for the writer task public API.
//!
//! Verifies that `WriterHandle`, `ReadIndex`, and `spawn_writer` are accessible
//! at the crate root and work together end-to-end: spawn the writer against a
//! tempdir-backed store, append events, and read them back via `ReadIndex`.

use eventfold_db::{ExpectedVersion, ProposedEvent, ReadIndex, Store, WriterHandle, spawn_writer};

/// Helper: create a `ProposedEvent` with minimal fields for testing.
fn proposed(event_type: &str) -> ProposedEvent {
    ProposedEvent {
        event_id: uuid::Uuid::new_v4(),
        event_type: event_type.to_string(),
        metadata: bytes::Bytes::new(),
        payload: bytes::Bytes::from_static(b"{}"),
    }
}

#[tokio::test]
async fn spawn_writer_append_two_events_read_back() {
    // Arrange: open a store in a tempdir, spawn the writer.
    let dir = tempfile::tempdir().expect("failed to create tempdir");
    let path = dir.path().join("events.log");
    let store = Store::open(&path).expect("Store::open should succeed");

    let (handle, read_index, join_handle) = spawn_writer(store, 8);

    // Verify the types are the expected crate-root re-exports.
    // These bindings prove `WriterHandle`, `ReadIndex`, and `spawn_writer`
    // are accessible at `eventfold_db::`.
    let _: &WriterHandle = &handle;
    let _: &ReadIndex = &read_index;

    // Act: append 2 events to the same stream.
    let stream_id = uuid::Uuid::new_v4();

    let result_0 = handle
        .append(
            stream_id,
            ExpectedVersion::NoStream,
            vec![proposed("EventA")],
        )
        .await
        .expect("first append should succeed");

    let result_1 = handle
        .append(
            stream_id,
            ExpectedVersion::Exact(0),
            vec![proposed("EventB")],
        )
        .await
        .expect("second append should succeed");

    // Assert: returned events have correct positions.
    assert_eq!(result_0.len(), 1);
    assert_eq!(result_0[0].global_position, 0);
    assert_eq!(result_0[0].stream_version, 0);

    assert_eq!(result_1.len(), 1);
    assert_eq!(result_1[0].global_position, 1);
    assert_eq!(result_1[0].stream_version, 1);

    // Assert: ReadIndex reflects both events.
    let all_events = read_index.read_all(0, 100);
    assert_eq!(all_events.len(), 2);
    assert_eq!(all_events[0].global_position, 0);
    assert_eq!(all_events[1].global_position, 1);
    assert_eq!(all_events[0].event_type, "EventA");
    assert_eq!(all_events[1].event_type, "EventB");

    // Verify stream-level read as well.
    let stream_events = read_index
        .read_stream(stream_id, 0, 100)
        .expect("read_stream should succeed");
    assert_eq!(stream_events.len(), 2);
    assert_eq!(stream_events[0].stream_version, 0);
    assert_eq!(stream_events[1].stream_version, 1);

    // Clean shutdown.
    drop(handle);
    join_handle.await.expect("writer task should exit cleanly");
}
