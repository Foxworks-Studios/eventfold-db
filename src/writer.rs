//! Single-writer task types for EventfoldDB.
//!
//! This module provides the `AppendRequest` struct and the `WriterHandle`
//! that gRPC handlers use to submit append requests to the writer task via
//! a bounded `tokio::mpsc` channel.

use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use metrics::{counter, gauge, histogram};
use uuid::Uuid;

use crate::broker::Broker;
use crate::dedup::DedupIndex;
use crate::error::Error;
use crate::types::{ExpectedVersion, ProposedEvent, RecordedEvent};

/// A request to append events to a stream, sent to the writer task via the mpsc channel.
///
/// The writer task processes each request sequentially, validates the expected version,
/// appends events to the log, and sends the result back through `response_tx`.
///
/// # Fields
///
/// * `stream_id` - UUID of the target stream.
/// * `expected_version` - Optimistic concurrency check for the stream.
/// * `events` - Events the client wants to append.
/// * `response_tx` - Oneshot channel for sending the result back to the caller.
pub struct AppendRequest {
    /// UUID of the target stream.
    pub stream_id: Uuid,
    /// Optimistic concurrency check for the stream.
    pub expected_version: ExpectedVersion,
    /// Events the client wants to append.
    pub events: Vec<ProposedEvent>,
    /// Oneshot channel for sending the result back to the caller.
    pub response_tx: tokio::sync::oneshot::Sender<Result<Vec<RecordedEvent>, Error>>,
}

/// Cloneable handle for submitting append requests to the writer task.
///
/// gRPC handlers hold a `WriterHandle` and call `append` to enqueue work.
/// The writer task processes requests sequentially on the other end of the
/// bounded `tokio::mpsc` channel, ensuring serialized writes and durability.
///
/// Cloning a `WriterHandle` produces a second sender into the same channel,
/// allowing multiple handlers to submit requests concurrently.
#[derive(Clone)]
pub struct WriterHandle {
    /// Sender half of the bounded mpsc channel to the writer task.
    tx: tokio::sync::mpsc::Sender<AppendRequest>,
}

impl WriterHandle {
    /// Create a new `WriterHandle` from the sender half of an mpsc channel.
    ///
    /// # Arguments
    ///
    /// * `tx` - Sender half of the bounded mpsc channel to the writer task.
    pub fn new(tx: tokio::sync::mpsc::Sender<AppendRequest>) -> Self {
        Self { tx }
    }

    /// Submit an append request to the writer task and await the result.
    ///
    /// Creates a oneshot channel, packages the request as an `AppendRequest`,
    /// sends it over the mpsc channel, and awaits the response. If the writer
    /// task has shut down (channel closed), returns `Error::InvalidArgument`.
    ///
    /// # Arguments
    ///
    /// * `stream_id` - UUID of the target stream.
    /// * `expected_version` - Optimistic concurrency check.
    /// * `events` - Events to append.
    ///
    /// # Returns
    ///
    /// The recorded events with server-assigned positions on success.
    ///
    /// # Errors
    ///
    /// - Returns the writer task's error (e.g., `WrongExpectedVersion`, `EventTooLarge`)
    ///   if the append fails.
    /// - Returns `Error::InvalidArgument("writer task closed")` if the channel is closed.
    pub async fn append(
        &self,
        stream_id: Uuid,
        expected_version: ExpectedVersion,
        events: Vec<ProposedEvent>,
    ) -> Result<Vec<RecordedEvent>, Error> {
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        let request = AppendRequest {
            stream_id,
            expected_version,
            events,
            response_tx,
        };

        // Send the request to the writer task. If the channel is closed,
        // the writer task has shut down.
        self.tx
            .send(request)
            .await
            .map_err(|_| Error::InvalidArgument("writer task closed".into()))?;

        // Await the response from the writer task. If the oneshot is dropped
        // without sending, the writer task panicked or was cancelled.
        response_rx
            .await
            .map_err(|_| Error::InvalidArgument("writer task closed".into()))?
    }
}

/// Validate that no two events in a proposed batch share the same `event_id`.
///
/// Returns `Ok(())` if all event IDs are unique, or `Err(Error::InvalidArgument)`
/// if a duplicate is found. This is a caller error (not a dedup hit) and is
/// rejected before any write occurs.
///
/// # Arguments
///
/// * `events` - The proposed events to validate.
///
/// # Errors
///
/// Returns [`Error::InvalidArgument`] if two events have the same `event_id`.
fn validate_batch_unique_ids(events: &[ProposedEvent]) -> Result<(), Error> {
    if events.len() <= 1 {
        return Ok(());
    }
    let mut seen = HashSet::with_capacity(events.len());
    for event in events {
        if !seen.insert(event.event_id) {
            return Err(Error::InvalidArgument(format!(
                "duplicate event_id {} within batch",
                event.event_id
            )));
        }
    }
    Ok(())
}

/// Run the writer task loop.
///
/// Receives `AppendRequest`s from the bounded mpsc channel, processes each by
/// calling `store.append()`, and sends the result back via the request's
/// `response_tx`. On each iteration, the first request is received via a
/// blocking `recv()` then additional pending requests are drained with
/// `try_recv()` for batching. The loop exits cleanly when all senders are
/// dropped (i.e., `rx.recv()` returns `None`).
///
/// Before each `store.append()`, the dedup index is checked. If the first
/// event ID in the proposed batch is already cached, the cached
/// `Vec<RecordedEvent>` is returned immediately without writing to disk or
/// publishing to the broker.
///
/// After each successful append (store.append returns Ok), the newly recorded
/// events are recorded in the dedup index and published to the broker before
/// the response is sent to the caller. Failed appends do not publish to the
/// broker.
///
/// If a response receiver has been dropped before the result is sent, a
/// `tracing::warn!` is logged and the result is discarded.
///
/// # Arguments
///
/// * `store` - The storage engine that processes appends.
/// * `rx` - Receiver half of the bounded mpsc channel carrying append requests.
/// * `broker` - Broadcast broker for publishing newly appended events to subscribers.
/// * `dedup` - Bounded LRU dedup index for idempotent append detection.
pub(crate) async fn run_writer(
    mut store: crate::store::Store,
    mut rx: tokio::sync::mpsc::Receiver<AppendRequest>,
    broker: Broker,
    dedup: &mut DedupIndex,
) {
    // Block on the first request; exit when channel is closed.
    while let Some(first) = rx.recv().await {
        // Drain any additional pending requests for batching.
        let mut batch = vec![first];
        while let Ok(req) = rx.try_recv() {
            batch.push(req);
        }

        // Stamp once per batch iteration so all requests drained together
        // share the same millisecond timestamp.
        let recorded_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before Unix epoch")
            .as_millis() as u64;

        // Process each request sequentially. Each call to store.append()
        // writes to disk, fsyncs, and updates the in-memory index.
        for req in batch {
            // Step 0: Reject batches with duplicate event IDs within the batch.
            if let Err(e) = validate_batch_unique_ids(&req.events) {
                if req.response_tx.send(Err(e)).is_err() {
                    tracing::warn!(
                        "writer: response receiver dropped for stream {}",
                        req.stream_id
                    );
                }
                continue;
            }

            // Step 1: Check the dedup index. A hit means this exact batch was
            // already written -- return the cached result without touching disk
            // or the broker.
            if let Some(cached) = dedup.check(&req.events) {
                let result = Ok(cached.as_ref().clone());
                if req.response_tx.send(result).is_err() {
                    tracing::warn!(
                        "writer: response receiver dropped for stream {}",
                        req.stream_id
                    );
                }
                continue;
            }

            // Step 2: Not a dedup hit -- perform the actual append.
            let start = Instant::now();
            let result = store.append(req.stream_id, req.expected_version, recorded_at, req.events);

            // On success: update metrics, record in dedup index, then publish
            // to broker. Failed appends do not update any metric.
            if let Ok(ref recorded) = result {
                let elapsed = start.elapsed();
                histogram!("eventfold_append_duration_seconds").record(elapsed.as_secs_f64());
                counter!("eventfold_appends_total").increment(1);
                counter!("eventfold_events_total").increment(recorded.len() as u64);

                // Stream count from the in-memory index.
                {
                    let log = store.log();
                    let log_guard = log.read().expect("EventLog RwLock poisoned");
                    gauge!("eventfold_streams_total").set(log_guard.streams.len() as f64);
                }

                // Log file size on disk.
                match store.log_file_len() {
                    Ok(len) => gauge!("eventfold_log_bytes").set(len as f64),
                    Err(e) => tracing::warn!(
                        error = %e,
                        "failed to read log file length for metrics"
                    ),
                }

                // Global head position (one past the last recorded event).
                if let Some(last) = recorded.last() {
                    gauge!("eventfold_global_position").set((last.global_position + 1) as f64);
                }

                dedup.record(recorded.clone());
                broker.publish(recorded);
            }

            // Send the result back to the caller.
            if req.response_tx.send(result).is_err() {
                tracing::warn!(
                    "writer: response receiver dropped for stream {}",
                    req.stream_id
                );
            }
        }
    }
    // Channel closed -- all WriterHandle senders have been dropped. Exit cleanly.
}

/// Spawn the writer task on the tokio runtime.
///
/// Creates a bounded mpsc channel, clones the shared event log `Arc` from the
/// store (for the `ReadIndex`), constructs and seeds a `DedupIndex`, moves
/// the store, broker, and dedup index into the spawned writer task, and returns
/// a triple of `(WriterHandle, ReadIndex, JoinHandle<()>)`.
///
/// # Arguments
///
/// * `store` - The storage engine to move into the writer task.
/// * `channel_capacity` - Bound on the mpsc channel. Controls backpressure.
/// * `broker` - Broadcast broker moved into the writer task for publishing events.
/// * `dedup_capacity` - Maximum number of event IDs tracked in the dedup index.
///
/// # Returns
///
/// A tuple of:
/// - `WriterHandle` -- cloneable sender for submitting append requests.
/// - `ReadIndex` -- shared, read-only view of the in-memory event log.
/// - `JoinHandle<()>` -- handle to await graceful shutdown of the writer task.
pub fn spawn_writer(
    store: crate::store::Store,
    channel_capacity: usize,
    broker: Broker,
    dedup_capacity: NonZeroUsize,
) -> (
    WriterHandle,
    crate::reader::ReadIndex,
    tokio::task::JoinHandle<()>,
) {
    // Clone the Arc BEFORE moving store into the task.
    let log_arc = store.log();
    let read_index = crate::reader::ReadIndex::new(log_arc.clone());

    // Build and seed the dedup index from the recovered log.
    let mut dedup = DedupIndex::new(dedup_capacity);
    {
        let log = log_arc.read().expect("EventLog RwLock poisoned");
        dedup.seed_from_log(&log.events);
    }

    let (tx, rx) = tokio::sync::mpsc::channel(channel_capacity);
    let writer_handle = WriterHandle::new(tx);

    let join_handle = tokio::spawn(async move {
        run_writer(store, rx, broker, &mut dedup).await;
    });

    (writer_handle, read_index, join_handle)
}

#[cfg(test)]
mod tests {
    /// Default dedup capacity for tests. Large enough to avoid eviction in
    /// standard test scenarios.
    fn test_dedup_cap() -> std::num::NonZeroUsize {
        std::num::NonZeroUsize::new(128).expect("nonzero")
    }

    #[test]
    fn append_request_has_required_fields() {
        use crate::error::Error;
        use crate::types::{ExpectedVersion, ProposedEvent, RecordedEvent};
        use uuid::Uuid;

        let (response_tx, _response_rx) =
            tokio::sync::oneshot::channel::<Result<Vec<RecordedEvent>, Error>>();

        let stream_id = Uuid::new_v4();
        let expected_version = ExpectedVersion::Any;
        let events = vec![ProposedEvent {
            event_id: Uuid::new_v4(),
            event_type: "TestEvent".to_string(),
            metadata: bytes::Bytes::new(),
            payload: bytes::Bytes::from_static(b"{}"),
        }];

        let req = super::AppendRequest {
            stream_id,
            expected_version,
            events: events.clone(),
            response_tx,
        };

        assert_eq!(req.stream_id, stream_id);
        assert_eq!(req.expected_version, expected_version);
        assert_eq!(req.events, events);
        // response_tx is consumed (moved into req), so we just verify it exists
        // by the fact that the struct constructed successfully.
    }

    #[tokio::test]
    async fn writer_handle_append_loopback() {
        use crate::types::{ExpectedVersion, ProposedEvent};
        use uuid::Uuid;

        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let handle = super::WriterHandle::new(tx);

        let stream_id = Uuid::new_v4();
        let event_id = Uuid::new_v4();
        let events = vec![ProposedEvent {
            event_id,
            event_type: "TestEvent".to_string(),
            metadata: bytes::Bytes::new(),
            payload: bytes::Bytes::from_static(b"{}"),
        }];

        // Spawn a task that mimics the writer: receive the request, verify
        // fields, and reply with Ok(vec![]).
        let expected_stream_id = stream_id;
        let expected_event_id = event_id;
        tokio::spawn(async move {
            let req = rx.recv().await.expect("should receive a request");
            assert_eq!(req.stream_id, expected_stream_id);
            assert_eq!(req.expected_version, ExpectedVersion::Any);
            assert_eq!(req.events.len(), 1);
            assert_eq!(req.events[0].event_id, expected_event_id);
            // Reply with success (empty vec for simplicity).
            let _ = req.response_tx.send(Ok(vec![]));
        });

        let result = handle.append(stream_id, ExpectedVersion::Any, events).await;
        assert!(result.is_ok());
        assert!(result.expect("should be Ok").is_empty());
    }

    #[tokio::test]
    async fn append_returns_error_when_receiver_dropped() {
        use crate::error::Error;
        use crate::types::{ExpectedVersion, ProposedEvent};
        use uuid::Uuid;

        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let handle = super::WriterHandle::new(tx);

        // Drop the receiver before sending -- the channel is closed.
        drop(rx);

        let events = vec![ProposedEvent {
            event_id: Uuid::new_v4(),
            event_type: "TestEvent".to_string(),
            metadata: bytes::Bytes::new(),
            payload: bytes::Bytes::from_static(b"{}"),
        }];

        let result = handle
            .append(Uuid::new_v4(), ExpectedVersion::Any, events)
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, Error::InvalidArgument(ref msg) if msg.contains("writer task closed")),
            "expected InvalidArgument('writer task closed'), got: {err:?}"
        );
    }

    #[tokio::test]
    async fn cloned_handles_send_to_same_channel() {
        use crate::types::{ExpectedVersion, ProposedEvent};
        use uuid::Uuid;

        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let handle_a = super::WriterHandle::new(tx);
        let handle_b = handle_a.clone();

        // Spawn a responder that handles exactly two requests.
        tokio::spawn(async move {
            for _ in 0..2 {
                let req = rx.recv().await.expect("should receive a request");
                let _ = req.response_tx.send(Ok(vec![]));
            }
        });

        let make_events = || {
            vec![ProposedEvent {
                event_id: Uuid::new_v4(),
                event_type: "TestEvent".to_string(),
                metadata: bytes::Bytes::new(),
                payload: bytes::Bytes::from_static(b"{}"),
            }]
        };

        // Both handles independently send requests to the same channel.
        let result_a = handle_a
            .append(Uuid::new_v4(), ExpectedVersion::Any, make_events())
            .await;
        let result_b = handle_b
            .append(Uuid::new_v4(), ExpectedVersion::Any, make_events())
            .await;

        assert!(result_a.is_ok(), "handle_a append should succeed");
        assert!(result_b.is_ok(), "handle_b append should succeed");
    }

    // --- Integration tests for run_writer / spawn_writer ---

    /// Helper: create a `ProposedEvent` with minimal fields for testing.
    fn proposed(event_type: &str) -> crate::types::ProposedEvent {
        crate::types::ProposedEvent {
            event_id: uuid::Uuid::new_v4(),
            event_type: event_type.to_string(),
            metadata: bytes::Bytes::new(),
            payload: bytes::Bytes::from_static(b"{}"),
        }
    }

    /// Helper: open a Store at a temp dir and return (store, tempdir).
    fn temp_store() -> (crate::store::Store, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");
        let store = crate::store::Store::open(&path).expect("open should succeed");
        (store, dir)
    }

    #[tokio::test]
    async fn ac1_basic_append_through_writer() {
        let (store, _dir) = temp_store();
        let (handle, _read_index, join_handle) =
            super::spawn_writer(store, 8, crate::broker::Broker::new(64), test_dedup_cap());

        let stream_id = uuid::Uuid::new_v4();
        let result = handle
            .append(
                stream_id,
                crate::types::ExpectedVersion::NoStream,
                vec![proposed("TestEvent")],
            )
            .await;

        let events = result.expect("append should succeed");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].global_position, 0);
        assert_eq!(events[0].stream_version, 0);

        drop(handle);
        join_handle.await.expect("writer task should exit cleanly");
    }

    #[tokio::test]
    async fn ac2_sequential_appends_have_contiguous_positions() {
        let (store, _dir) = temp_store();
        let (handle, _read_index, join_handle) =
            super::spawn_writer(store, 8, crate::broker::Broker::new(64), test_dedup_cap());

        let stream_id = uuid::Uuid::new_v4();

        // First append: NoStream
        let r0 = handle
            .append(
                stream_id,
                crate::types::ExpectedVersion::NoStream,
                vec![proposed("Evt0")],
            )
            .await
            .expect("append 0 should succeed");
        assert_eq!(r0[0].global_position, 0);
        assert_eq!(r0[0].stream_version, 0);

        // Second append: Exact(0)
        let r1 = handle
            .append(
                stream_id,
                crate::types::ExpectedVersion::Exact(0),
                vec![proposed("Evt1")],
            )
            .await
            .expect("append 1 should succeed");
        assert_eq!(r1[0].global_position, 1);
        assert_eq!(r1[0].stream_version, 1);

        // Third append: Exact(1)
        let r2 = handle
            .append(
                stream_id,
                crate::types::ExpectedVersion::Exact(1),
                vec![proposed("Evt2")],
            )
            .await
            .expect("append 2 should succeed");
        assert_eq!(r2[0].global_position, 2);
        assert_eq!(r2[0].stream_version, 2);

        drop(handle);
        join_handle.await.expect("writer task should exit cleanly");
    }

    #[tokio::test]
    async fn ac3_concurrent_appends_serialized() {
        let (store, _dir) = temp_store();
        let (handle, _read_index, join_handle) =
            super::spawn_writer(store, 16, crate::broker::Broker::new(64), test_dedup_cap());

        let mut tasks = Vec::with_capacity(10);
        for _ in 0..10 {
            let h = handle.clone();
            tasks.push(tokio::spawn(async move {
                h.append(
                    uuid::Uuid::new_v4(),
                    crate::types::ExpectedVersion::Any,
                    vec![proposed("ConcurrentEvt")],
                )
                .await
            }));
        }

        let mut positions = std::collections::HashSet::new();
        for task in tasks {
            let result = task.await.expect("task should not panic");
            let events = result.expect("append should succeed");
            assert_eq!(events.len(), 1);
            positions.insert(events[0].global_position);
        }

        // All 10 global positions should be unique and form {0..9}.
        let expected: std::collections::HashSet<u64> = (0..10).collect();
        assert_eq!(positions, expected);

        drop(handle);
        join_handle.await.expect("writer task should exit cleanly");
    }

    #[tokio::test]
    async fn ac4a_nostream_twice_returns_wrong_expected_version() {
        let (store, _dir) = temp_store();
        let (handle, _read_index, join_handle) =
            super::spawn_writer(store, 8, crate::broker::Broker::new(64), test_dedup_cap());

        let stream_id = uuid::Uuid::new_v4();
        handle
            .append(
                stream_id,
                crate::types::ExpectedVersion::NoStream,
                vec![proposed("First")],
            )
            .await
            .expect("first append should succeed");

        let result = handle
            .append(
                stream_id,
                crate::types::ExpectedVersion::NoStream,
                vec![proposed("Second")],
            )
            .await;
        assert!(
            matches!(
                result,
                Err(crate::error::Error::WrongExpectedVersion { .. })
            ),
            "expected WrongExpectedVersion, got: {result:?}"
        );

        drop(handle);
        join_handle.await.expect("writer task should exit cleanly");
    }

    #[tokio::test]
    async fn ac4b_exact_0_after_nostream_succeeds() {
        let (store, _dir) = temp_store();
        let (handle, _read_index, join_handle) =
            super::spawn_writer(store, 8, crate::broker::Broker::new(64), test_dedup_cap());

        let stream_id = uuid::Uuid::new_v4();
        handle
            .append(
                stream_id,
                crate::types::ExpectedVersion::NoStream,
                vec![proposed("First")],
            )
            .await
            .expect("first append should succeed");

        let result = handle
            .append(
                stream_id,
                crate::types::ExpectedVersion::Exact(0),
                vec![proposed("Second")],
            )
            .await;
        assert!(result.is_ok(), "Exact(0) after NoStream should succeed");

        drop(handle);
        join_handle.await.expect("writer task should exit cleanly");
    }

    #[tokio::test]
    async fn ac4c_exact_5_after_nostream_returns_wrong_expected_version() {
        let (store, _dir) = temp_store();
        let (handle, _read_index, join_handle) =
            super::spawn_writer(store, 8, crate::broker::Broker::new(64), test_dedup_cap());

        let stream_id = uuid::Uuid::new_v4();
        handle
            .append(
                stream_id,
                crate::types::ExpectedVersion::NoStream,
                vec![proposed("First")],
            )
            .await
            .expect("first append should succeed");

        let result = handle
            .append(
                stream_id,
                crate::types::ExpectedVersion::Exact(5),
                vec![proposed("Second")],
            )
            .await;
        assert!(
            matches!(
                result,
                Err(crate::error::Error::WrongExpectedVersion { .. })
            ),
            "expected WrongExpectedVersion, got: {result:?}"
        );

        drop(handle);
        join_handle.await.expect("writer task should exit cleanly");
    }

    #[tokio::test]
    async fn ac5_read_index_reflects_writes() {
        let (store, _dir) = temp_store();
        let (handle, read_index, join_handle) =
            super::spawn_writer(store, 8, crate::broker::Broker::new(64), test_dedup_cap());

        let stream_id = uuid::Uuid::new_v4();
        for i in 0..3u64 {
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

        // read_all should return all 3 events.
        let all = read_index.read_all(0, 100);
        assert_eq!(all.len(), 3);

        // read_stream should return 3 events for this stream.
        let stream_events = read_index
            .read_stream(stream_id, 0, 100)
            .expect("read_stream should succeed");
        assert_eq!(stream_events.len(), 3);
        for (i, event) in stream_events.iter().enumerate() {
            assert_eq!(event.stream_version, i as u64);
            assert_eq!(event.stream_id, stream_id);
        }

        drop(handle);
        join_handle.await.expect("writer task should exit cleanly");
    }

    #[tokio::test]
    async fn ac6_durability_survives_restart() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        // First run: append 5 events and shut down cleanly.
        {
            let store = crate::store::Store::open(&path).expect("open should succeed");
            let (handle, _read_index, join_handle) =
                super::spawn_writer(store, 8, crate::broker::Broker::new(64), test_dedup_cap());

            let stream_id = uuid::Uuid::new_v4();
            for _ in 0..5u64 {
                handle
                    .append(
                        stream_id,
                        crate::types::ExpectedVersion::Any,
                        vec![proposed("Durable")],
                    )
                    .await
                    .expect("append should succeed");
            }

            drop(handle);
            join_handle.await.expect("writer task should exit cleanly");
        }

        // Second run: open at same path and verify all 5 events recovered.
        {
            let store = crate::store::Store::open(&path).expect("reopen should succeed");
            let read_index = crate::reader::ReadIndex::new(store.log());
            let all = read_index.read_all(0, 100);
            assert_eq!(
                all.len(),
                5,
                "expected 5 recovered events, got {}",
                all.len()
            );
        }
    }

    #[tokio::test]
    async fn ac7_graceful_shutdown_on_handle_drop() {
        let (store, _dir) = temp_store();
        let (handle, _read_index, join_handle) =
            super::spawn_writer(store, 8, crate::broker::Broker::new(64), test_dedup_cap());

        // Drop all WriterHandle clones. This closes the channel.
        drop(handle);

        // The writer task should exit within 1 second.
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), join_handle).await;
        assert!(result.is_ok(), "join_handle should resolve within 1 second");
        result
            .expect("should not timeout")
            .expect("writer task should not panic");
    }

    #[tokio::test]
    async fn ac8_backpressure_bounded_channel() {
        let (store, _dir) = temp_store();
        let (handle, _read_index, join_handle) =
            super::spawn_writer(store, 1, crate::broker::Broker::new(64), test_dedup_cap());

        // With capacity=1, the channel holds exactly one message. Fill it
        // using try_send (synchronous, non-blocking) to avoid yielding to
        // the runtime, which would allow the writer task to drain the slot.
        let (response_tx, _response_rx) = tokio::sync::oneshot::channel();
        handle
            .tx
            .try_send(super::AppendRequest {
                stream_id: uuid::Uuid::new_v4(),
                expected_version: crate::types::ExpectedVersion::Any,
                events: vec![proposed("Fill")],
                response_tx,
            })
            .expect("first try_send should succeed (channel empty)");

        // Second try_send should fail immediately because the channel is full.
        let (response_tx2, _response_rx2) = tokio::sync::oneshot::channel();
        let send_result = handle.tx.try_send(super::AppendRequest {
            stream_id: uuid::Uuid::new_v4(),
            expected_version: crate::types::ExpectedVersion::Any,
            events: vec![proposed("Block")],
            response_tx: response_tx2,
        });

        assert!(
            matches!(
                send_result,
                Err(tokio::sync::mpsc::error::TrySendError::Full(_))
            ),
            "second try_send should fail with Full, got: {send_result:?}"
        );

        drop(handle);
        join_handle.await.expect("writer task should exit cleanly");
    }

    #[tokio::test]
    async fn ac9a_event_too_large_returns_error() {
        let (store, _dir) = temp_store();
        let (handle, _read_index, join_handle) =
            super::spawn_writer(store, 8, crate::broker::Broker::new(64), test_dedup_cap());

        // Create an event whose payload exceeds MAX_EVENT_SIZE (64 KB).
        let oversized_payload = bytes::Bytes::from(vec![0u8; crate::types::MAX_EVENT_SIZE + 1]);
        let event = crate::types::ProposedEvent {
            event_id: uuid::Uuid::new_v4(),
            event_type: "BigEvent".to_string(),
            metadata: bytes::Bytes::new(),
            payload: oversized_payload,
        };

        let result = handle
            .append(
                uuid::Uuid::new_v4(),
                crate::types::ExpectedVersion::Any,
                vec![event],
            )
            .await;

        assert!(
            matches!(result, Err(crate::error::Error::EventTooLarge { .. })),
            "expected EventTooLarge, got: {result:?}"
        );

        drop(handle);
        join_handle.await.expect("writer task should exit cleanly");
    }

    #[tokio::test]
    async fn ac9b_writer_not_poisoned_after_error() {
        let (store, _dir) = temp_store();
        let (handle, _read_index, join_handle) =
            super::spawn_writer(store, 8, crate::broker::Broker::new(64), test_dedup_cap());

        // First: send an oversized event that fails.
        let oversized_payload = bytes::Bytes::from(vec![0u8; crate::types::MAX_EVENT_SIZE + 1]);
        let bad_event = crate::types::ProposedEvent {
            event_id: uuid::Uuid::new_v4(),
            event_type: "BigEvent".to_string(),
            metadata: bytes::Bytes::new(),
            payload: oversized_payload,
        };

        let result = handle
            .append(
                uuid::Uuid::new_v4(),
                crate::types::ExpectedVersion::Any,
                vec![bad_event],
            )
            .await;
        assert!(result.is_err(), "oversized event should fail");

        // Second: a valid append should still succeed.
        let ok_result = handle
            .append(
                uuid::Uuid::new_v4(),
                crate::types::ExpectedVersion::Any,
                vec![proposed("AfterError")],
            )
            .await;
        assert!(ok_result.is_ok(), "valid append after error should succeed");

        drop(handle);
        join_handle.await.expect("writer task should exit cleanly");
    }

    // --- Broker integration tests (PRD 005, Ticket 3) ---

    #[tokio::test]
    async fn ac13_writer_publishes_to_broker() {
        use crate::broker::Broker;
        use std::sync::Arc;

        let (store, _dir) = temp_store();
        let broker = Broker::new(64);
        let mut rx = broker.subscribe();

        let (handle, _read_index, join_handle) =
            super::spawn_writer(store, 8, broker, test_dedup_cap());

        let stream_id = uuid::Uuid::new_v4();
        handle
            .append(
                stream_id,
                crate::types::ExpectedVersion::NoStream,
                vec![proposed("BrokerEvent")],
            )
            .await
            .expect("append should succeed");

        let received: Arc<crate::types::RecordedEvent> =
            rx.recv().await.expect("should receive event from broker");
        assert_eq!(received.event_type, "BrokerEvent");

        drop(handle);
        join_handle.await.expect("writer task should exit cleanly");
    }

    #[tokio::test]
    async fn broker_receives_three_events_in_order() {
        use crate::broker::Broker;

        let (store, _dir) = temp_store();
        let broker = Broker::new(64);
        let mut rx = broker.subscribe();

        let (handle, _read_index, join_handle) =
            super::spawn_writer(store, 8, broker, test_dedup_cap());

        let stream_id = uuid::Uuid::new_v4();
        for i in 0u64..3 {
            let ev = if i == 0 {
                crate::types::ExpectedVersion::NoStream
            } else {
                crate::types::ExpectedVersion::Exact(i - 1)
            };
            handle
                .append(stream_id, ev, vec![proposed(&format!("Evt{i}"))])
                .await
                .expect("append should succeed");
        }

        // Receive 3 events and verify global positions 0, 1, 2 in order.
        for expected_pos in 0u64..3 {
            let received = rx.recv().await.expect("should receive event");
            assert_eq!(
                received.global_position, expected_pos,
                "expected position {expected_pos}, got {}",
                received.global_position
            );
        }

        drop(handle);
        join_handle.await.expect("writer task should exit cleanly");
    }

    #[tokio::test]
    async fn failed_append_does_not_publish_to_broker() {
        use crate::broker::Broker;
        use tokio::sync::broadcast::error::TryRecvError;

        let (store, _dir) = temp_store();
        let broker = Broker::new(64);
        let mut rx = broker.subscribe();

        let (handle, _read_index, join_handle) =
            super::spawn_writer(store, 8, broker, test_dedup_cap());

        let stream_id = uuid::Uuid::new_v4();

        // First: succeed with NoStream to create the stream.
        handle
            .append(
                stream_id,
                crate::types::ExpectedVersion::NoStream,
                vec![proposed("First")],
            )
            .await
            .expect("first append should succeed");

        // Drain the one successful event from the broker.
        let _ = rx.recv().await.expect("should receive the first event");

        // Second: attempt a conflicting append (NoStream again) -- must fail.
        let result = handle
            .append(
                stream_id,
                crate::types::ExpectedVersion::NoStream,
                vec![proposed("Conflict")],
            )
            .await;
        assert!(result.is_err(), "conflicting append should fail");

        // The broker should NOT have received anything from the failed append.
        assert_eq!(
            rx.try_recv(),
            Err(TryRecvError::Empty),
            "broker should have no events from failed append"
        );

        drop(handle);
        join_handle.await.expect("writer task should exit cleanly");
    }

    // --- Dedup integration tests (PRD 010, Ticket 2) ---

    /// Helper: create a `ProposedEvent` with a specific event ID.
    fn proposed_with_id(event_id: uuid::Uuid, event_type: &str) -> crate::types::ProposedEvent {
        crate::types::ProposedEvent {
            event_id,
            event_type: event_type.to_string(),
            metadata: bytes::Bytes::new(),
            payload: bytes::Bytes::from_static(b"{}"),
        }
    }

    #[tokio::test]
    async fn dedup_hit_returns_same_positions() {
        let (store, _dir) = temp_store();
        let dedup_cap = std::num::NonZeroUsize::new(128).expect("nonzero");
        let (handle, _read_index, join_handle) =
            super::spawn_writer(store, 8, crate::broker::Broker::new(64), dedup_cap);

        let stream_id = uuid::Uuid::new_v4();
        let event_id = uuid::Uuid::new_v4();

        // First append: creates the event.
        let first = handle
            .append(
                stream_id,
                crate::types::ExpectedVersion::Any,
                vec![proposed_with_id(event_id, "TestEvent")],
            )
            .await
            .expect("first append should succeed");
        assert_eq!(first.len(), 1);

        // Second append: identical event_id -- should be a dedup hit.
        let second = handle
            .append(
                stream_id,
                crate::types::ExpectedVersion::Any,
                vec![proposed_with_id(event_id, "TestEvent")],
            )
            .await
            .expect("dedup hit should return Ok");

        // Same global_position values.
        assert_eq!(first.len(), second.len());
        assert_eq!(first[0].global_position, second[0].global_position);

        drop(handle);
        join_handle.await.expect("writer task should exit cleanly");
    }

    #[tokio::test]
    async fn dedup_hit_does_not_duplicate_events_in_log() {
        let (store, _dir) = temp_store();
        let dedup_cap = std::num::NonZeroUsize::new(128).expect("nonzero");
        let (handle, read_index, join_handle) =
            super::spawn_writer(store, 8, crate::broker::Broker::new(64), dedup_cap);

        let stream_id = uuid::Uuid::new_v4();
        let event_id = uuid::Uuid::new_v4();

        // First append.
        handle
            .append(
                stream_id,
                crate::types::ExpectedVersion::Any,
                vec![proposed_with_id(event_id, "TestEvent")],
            )
            .await
            .expect("first append should succeed");

        // Second append with same event_id (dedup hit).
        handle
            .append(
                stream_id,
                crate::types::ExpectedVersion::Any,
                vec![proposed_with_id(event_id, "TestEvent")],
            )
            .await
            .expect("dedup hit should return Ok");

        // read_all should return exactly 1 event, not 2.
        let all = read_index.read_all(0, 1000);
        assert_eq!(all.len(), 1, "expected 1 event in log, got {}", all.len());

        drop(handle);
        join_handle.await.expect("writer task should exit cleanly");
    }

    #[tokio::test]
    async fn dedup_hit_does_not_publish_to_broker() {
        use crate::broker::Broker;
        use tokio::sync::broadcast::error::TryRecvError;

        let (store, _dir) = temp_store();
        let broker = Broker::new(64);
        let mut rx = broker.subscribe();
        let dedup_cap = std::num::NonZeroUsize::new(128).expect("nonzero");

        let (handle, _read_index, join_handle) = super::spawn_writer(store, 8, broker, dedup_cap);

        let stream_id = uuid::Uuid::new_v4();
        let event_id = uuid::Uuid::new_v4();

        // First append: creates the event.
        handle
            .append(
                stream_id,
                crate::types::ExpectedVersion::Any,
                vec![proposed_with_id(event_id, "TestEvent")],
            )
            .await
            .expect("first append should succeed");

        // Drain the broker from the first (real) append.
        let _ = rx.recv().await.expect("should receive the first event");

        // Second append with same event_id (dedup hit).
        handle
            .append(
                stream_id,
                crate::types::ExpectedVersion::Any,
                vec![proposed_with_id(event_id, "TestEvent")],
            )
            .await
            .expect("dedup hit should return Ok");

        // The broker should NOT have received anything from the dedup hit.
        assert_eq!(
            rx.try_recv(),
            Err(TryRecvError::Empty),
            "broker should have no events from dedup hit"
        );

        drop(handle);
        join_handle.await.expect("writer task should exit cleanly");
    }

    #[tokio::test]
    async fn duplicate_event_id_within_batch_rejected() {
        let (store, _dir) = temp_store();
        let dedup_cap = std::num::NonZeroUsize::new(128).expect("nonzero");
        let (handle, _read_index, join_handle) =
            super::spawn_writer(store, 8, crate::broker::Broker::new(64), dedup_cap);

        let stream_id = uuid::Uuid::new_v4();
        let shared_id = uuid::Uuid::new_v4();

        // A batch with two events sharing the same event_id is a caller error.
        let result = handle
            .append(
                stream_id,
                crate::types::ExpectedVersion::Any,
                vec![
                    proposed_with_id(shared_id, "EventA"),
                    proposed_with_id(shared_id, "EventB"),
                ],
            )
            .await;

        assert!(
            matches!(result, Err(crate::error::Error::InvalidArgument(ref msg)) if msg.contains("duplicate event_id")),
            "expected InvalidArgument with 'duplicate event_id', got: {result:?}"
        );

        // Writer should not be poisoned -- a subsequent valid append should succeed.
        let ok_result = handle
            .append(
                stream_id,
                crate::types::ExpectedVersion::Any,
                vec![proposed("ValidEvent")],
            )
            .await;
        assert!(
            ok_result.is_ok(),
            "valid append after duplicate rejection should succeed"
        );

        drop(handle);
        join_handle.await.expect("writer task should exit cleanly");
    }

    // --- Timestamp tests (PRD 017, Ticket 4) ---

    #[tokio::test]
    async fn recorded_at_is_nonzero_for_single_event() {
        let (store, _dir) = temp_store();
        let (handle, _read_index, join_handle) =
            super::spawn_writer(store, 8, crate::broker::Broker::new(64), test_dedup_cap());

        let stream_id = uuid::Uuid::new_v4();
        let result = handle
            .append(
                stream_id,
                crate::types::ExpectedVersion::NoStream,
                vec![proposed("TimestampEvt")],
            )
            .await
            .expect("append should succeed");

        assert_eq!(result.len(), 1);
        assert!(
            result[0].recorded_at > 0,
            "recorded_at should be nonzero, got {}",
            result[0].recorded_at
        );

        drop(handle);
        join_handle.await.expect("writer task should exit cleanly");
    }

    #[tokio::test]
    async fn batch_of_three_events_share_same_recorded_at() {
        let (store, _dir) = temp_store();
        let (handle, _read_index, join_handle) =
            super::spawn_writer(store, 8, crate::broker::Broker::new(64), test_dedup_cap());

        let stream_id = uuid::Uuid::new_v4();
        let events = vec![
            proposed("BatchEvt1"),
            proposed("BatchEvt2"),
            proposed("BatchEvt3"),
        ];

        let result = handle
            .append(stream_id, crate::types::ExpectedVersion::NoStream, events)
            .await
            .expect("append should succeed");

        assert_eq!(result.len(), 3);

        // All three events should have the same recorded_at, and it should be nonzero.
        let ts = result[0].recorded_at;
        assert!(ts > 0, "recorded_at should be nonzero, got {ts}");
        assert_eq!(
            result[1].recorded_at, ts,
            "event 1 recorded_at ({}) should match event 0 ({ts})",
            result[1].recorded_at
        );
        assert_eq!(
            result[2].recorded_at, ts,
            "event 2 recorded_at ({}) should match event 0 ({ts})",
            result[2].recorded_at
        );

        drop(handle);
        join_handle.await.expect("writer task should exit cleanly");
    }

    // --- Metrics integration test (PRD 013, Ticket 2 AC 11) ---

    /// Parse a Prometheus counter value from rendered text output.
    ///
    /// Scans each line for `<metric_name> <value>` and returns the parsed `u64`.
    /// Returns `None` if the metric is not present or cannot be parsed.
    fn parse_counter(rendered: &str, metric_name: &str) -> Option<u64> {
        for line in rendered.lines() {
            // Skip comment lines (# TYPE, # HELP).
            if line.starts_with('#') {
                continue;
            }
            // Counter lines are formatted as "<metric_name> <value>" where
            // value is an integer or float (e.g., "3" or "3.0").
            if let Some(rest) = line.strip_prefix(metric_name) {
                // Ensure the match is exact (not a prefix of a longer name).
                let rest = rest.trim_start();
                if rest.is_empty() || rest.starts_with('{') || rest.as_bytes()[0].is_ascii_digit() {
                    let value_str = if rest.starts_with('{') {
                        // Has labels: "metric{label="val"} 3"
                        rest.split('}').nth(1).map(|s| s.trim())
                    } else {
                        Some(rest.trim())
                    };
                    if let Some(s) = value_str {
                        // Handle float representation (e.g., "3.0" -> 3).
                        return s.parse::<f64>().ok().map(|f| f as u64);
                    }
                }
            }
        }
        None
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn ac11_writer_metrics_appends_and_events_total() {
        // Ensure the global recorder is installed. Tolerates AlreadyInstalled
        // from other tests in this process (OnceLock guard).
        let _ = crate::metrics::install_recorder();
        let metrics_handle = crate::metrics::get_installed_handle()
            .expect("recorder should be installed after install_recorder()");

        // Snapshot BEFORE: capture current counter values so we can compute
        // deltas. This makes the test resilient to process-global metric
        // accumulation from other tests running in the same process.
        let before = metrics_handle.render();
        let appends_before = parse_counter(&before, "eventfold_appends_total").unwrap_or(0);
        let events_before = parse_counter(&before, "eventfold_events_total").unwrap_or(0);

        // Set up the writer and append 3 events (one per request).
        let (store, _dir) = temp_store();
        let (handle, _read_index, join_handle) =
            super::spawn_writer(store, 8, crate::broker::Broker::new(64), test_dedup_cap());

        let stream_id = uuid::Uuid::new_v4();
        for i in 0u64..3 {
            let ev = if i == 0 {
                crate::types::ExpectedVersion::NoStream
            } else {
                crate::types::ExpectedVersion::Exact(i - 1)
            };
            handle
                .append(stream_id, ev, vec![proposed("MetricsEvt")])
                .await
                .expect("append should succeed");
        }

        // Shut down the writer so all metrics are flushed.
        drop(handle);
        join_handle.await.expect("writer task should exit cleanly");

        // Snapshot AFTER and compute deltas.
        let after = metrics_handle.render();
        let appends_after = parse_counter(&after, "eventfold_appends_total")
            .expect("eventfold_appends_total should be present in rendered metrics");
        let events_after = parse_counter(&after, "eventfold_events_total")
            .expect("eventfold_events_total should be present in rendered metrics");

        let appends_delta = appends_after - appends_before;
        let events_delta = events_after - events_before;

        assert_eq!(
            appends_delta, 3,
            "expected 3 new appends, got delta {appends_delta} (before={appends_before}, after={appends_after})"
        );
        assert_eq!(
            events_delta, 3,
            "expected 3 new events, got delta {events_delta} (before={events_before}, after={events_after})"
        );
    }
}
