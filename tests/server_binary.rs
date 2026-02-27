//! Integration tests for the EventfoldDB server binary startup, configuration,
//! recovery, and end-to-end round-trip.
//!
//! These tests start a real in-process gRPC server using the same stack as the
//! production binary (Store, Broker, writer task, EventfoldService, tonic Server)
//! and exercise it through a gRPC client.

use std::net::SocketAddr;
use std::path::Path;

use eventfold_db::proto::event_store_client::EventStoreClient;
use eventfold_db::proto::event_store_server::EventStoreServer;
use eventfold_db::proto::{self, expected_version};
use eventfold_db::{
    Broker, EventfoldService, ExpectedVersion, ProposedEvent, ReadIndex, Store, WriterHandle,
    spawn_writer,
};

use tonic::transport::Channel;

/// Handle to a running test server, used for clean shutdown and restart scenarios.
///
/// The server is started with `serve_with_incoming_shutdown` connected to a
/// `tokio::sync::oneshot` channel. Sending on the shutdown channel causes the tonic
/// server to stop accepting new connections and complete its future, which drops
/// the `EventfoldService` and its `WriterHandle` clone.
struct ServerHandle {
    /// Handle to the writer task's mpsc sender. Drop to close the writer channel.
    writer_handle: WriterHandle,
    /// JoinHandle for the writer task. Await after closing the channel.
    writer_join: tokio::task::JoinHandle<()>,
    /// JoinHandle for the tonic server task.
    server_join: tokio::task::JoinHandle<()>,
    /// Oneshot sender to trigger graceful shutdown of the tonic server.
    shutdown_tx: tokio::sync::oneshot::Sender<()>,
}

impl ServerHandle {
    /// Shut down the server and writer task gracefully.
    ///
    /// Sends the shutdown signal to the tonic server, awaits the server task
    /// (which drops the EventfoldService and its WriterHandle clone), drops
    /// our WriterHandle (closing the mpsc channel), then awaits the writer task.
    async fn shutdown(self) {
        // Signal the tonic server to stop accepting new connections.
        let _ = self.shutdown_tx.send(());
        // Await the server task so it drops the EventfoldService + WriterHandle clone.
        let _ = self.server_join.await;
        // Drop our writer handle. This closes the mpsc channel.
        drop(self.writer_handle);
        // The writer task sees the closed channel and exits.
        let _ = self.writer_join.await;
    }
}

/// Start an in-process gRPC server with the full EventfoldDB stack.
///
/// Opens a real `Store` at the given data path, spawns the writer task and broker,
/// builds the `EventfoldService`, binds a tonic server on the given address, and
/// returns a connected gRPC client along with a `ServerHandle` for lifecycle control.
///
/// # Arguments
///
/// * `data_path` - Path to the append-only log file.
/// * `listen_addr` - Socket address to bind (use `[::1]:0` for ephemeral port).
/// * `broker_capacity` - Broadcast channel ring buffer size.
///
/// # Returns
///
/// A tuple of `(EventStoreClient, ServerHandle)`.
///
/// # Panics
///
/// Panics if the store cannot be opened, the listener cannot bind, or the client
/// cannot connect.
async fn start_server_at(
    data_path: &Path,
    listen_addr: SocketAddr,
    broker_capacity: usize,
) -> (EventStoreClient<Channel>, ServerHandle) {
    let store = Store::open(data_path).expect("store open should succeed");
    let broker = Broker::new(broker_capacity);
    let (writer_handle, read_index, writer_join) = spawn_writer(store, 64, broker.clone());

    let service = EventfoldService::new(writer_handle.clone(), read_index, broker);

    let listener = tokio::net::TcpListener::bind(listen_addr)
        .await
        .expect("bind should succeed");
    let addr = listener.local_addr().expect("should have local addr");
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let server_join = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(EventStoreServer::new(service))
            .serve_with_incoming_shutdown(incoming, async {
                let _ = shutdown_rx.await;
            })
            .await
            .expect("server should run");
    });

    // Give the server a moment to start accepting connections.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = EventStoreClient::connect(format!("http://[::1]:{}", addr.port()))
        .await
        .expect("client connect should succeed");

    let handle = ServerHandle {
        writer_handle,
        writer_join,
        server_join,
        shutdown_tx,
    };

    (client, handle)
}

/// Helper: create a proto ProposedEvent with a random UUID and given event type.
fn make_proposed(event_type: &str) -> proto::ProposedEvent {
    proto::ProposedEvent {
        event_id: uuid::Uuid::new_v4().to_string(),
        event_type: event_type.to_string(),
        metadata: vec![],
        payload: b"{}".to_vec(),
    }
}

/// Helper: create an ExpectedVersion::NoStream.
fn no_stream() -> Option<proto::ExpectedVersion> {
    Some(proto::ExpectedVersion {
        kind: Some(expected_version::Kind::NoStream(proto::Empty {})),
    })
}

/// Ephemeral IPv6 loopback address for test servers.
const EPHEMERAL_ADDR: &str = "[::1]:0";

// -- Test AC-1: Server starts and accepts gRPC connections --

#[tokio::test]
async fn ac1_server_starts_and_accepts_grpc() {
    let dir = tempfile::tempdir().expect("failed to create tempdir");
    let data_path = dir.path().join("events.log");
    let listen_addr: SocketAddr = EPHEMERAL_ADDR.parse().expect("valid addr");

    let (mut client, handle) = start_server_at(&data_path, listen_addr, 1024).await;

    // ReadAll on an empty store should return 0 events.
    let resp = client
        .read_all(proto::ReadAllRequest {
            from_position: 0,
            max_count: 100,
        })
        .await
        .expect("read_all should succeed on empty store");

    assert_eq!(resp.into_inner().events.len(), 0);

    handle.shutdown().await;
}

// -- Test AC-5: Custom broker capacity causes lag when overflowed --

#[tokio::test]
async fn ac5_small_broker_capacity_causes_lag() {
    let dir = tempfile::tempdir().expect("failed to create tempdir");
    let data_path = dir.path().join("events.log");
    let listen_addr: SocketAddr = EPHEMERAL_ADDR.parse().expect("valid addr");

    // Use a small broker capacity of 16.
    let (mut client, _handle) = start_server_at(&data_path, listen_addr, 16).await;

    // Start a SubscribeAll subscription on the empty store.
    let mut sub = client
        .subscribe_all(proto::SubscribeAllRequest { from_position: 0 })
        .await
        .expect("subscribe_all should succeed")
        .into_inner();

    let timeout_dur = std::time::Duration::from_secs(5);

    // Receive the initial CaughtUp marker (store is empty).
    let msg = tokio::time::timeout(timeout_dur, sub.message())
        .await
        .expect("should not timeout")
        .expect("message should succeed")
        .expect("stream should not end");
    assert!(
        matches!(
            msg.content,
            Some(proto::subscribe_response::Content::CaughtUp(_))
        ),
        "expected CaughtUp on empty store"
    );

    // Append 20 events WITHOUT reading from the subscription. With a broker
    // capacity of 16, the broadcast channel should overflow and cause lag.
    let stream_id = uuid::Uuid::new_v4().to_string();
    client
        .append(proto::AppendRequest {
            stream_id,
            expected_version: no_stream(),
            events: (0..20)
                .map(|i| make_proposed(&format!("Flood{i}")))
                .collect(),
        })
        .await
        .expect("append should succeed");

    // Now read from the subscription. It should eventually yield an error
    // (lag detected) or the stream should end.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let item = tokio::time::timeout_at(deadline, sub.message()).await;
        match item {
            Ok(Ok(Some(msg))) => {
                // May receive some events before the lag is detected.
                if msg.content.is_none() {
                    break;
                }
            }
            Ok(Ok(None)) => {
                // Stream ended (server closed it after lag error).
                break;
            }
            Ok(Err(_status)) => {
                // gRPC error status from the server (lag mapped to error).
                break;
            }
            Err(_) => {
                panic!("timed out waiting for lag error or stream end");
            }
        }
    }
    // If we reach here, the stream terminated as expected (lag overflow).
}

// -- Test AC-6: Recovery on restart preserves events and positions --

#[tokio::test]
async fn ac6_recovery_on_restart() {
    let dir = tempfile::tempdir().expect("failed to create tempdir");
    let data_path = dir.path().join("events.log");
    let listen_addr: SocketAddr = EPHEMERAL_ADDR.parse().expect("valid addr");

    let stream_id = uuid::Uuid::new_v4().to_string();

    // First server: append 5 events, then shut down.
    {
        let (mut client, handle) = start_server_at(&data_path, listen_addr, 1024).await;

        client
            .append(proto::AppendRequest {
                stream_id: stream_id.clone(),
                expected_version: no_stream(),
                events: (0..5).map(|i| make_proposed(&format!("Evt{i}"))).collect(),
            })
            .await
            .expect("append should succeed");

        handle.shutdown().await;
    }

    // Second server: open at the same data path and verify recovery.
    {
        let (mut client, handle) = start_server_at(&data_path, listen_addr, 1024).await;

        // ReadAll from position 0 should return all 5 recovered events.
        let resp = client
            .read_all(proto::ReadAllRequest {
                from_position: 0,
                max_count: 100,
            })
            .await
            .expect("read_all should succeed after recovery");

        let events = resp.into_inner().events;
        assert_eq!(events.len(), 5, "expected 5 recovered events");
        for (i, event) in events.iter().enumerate() {
            assert_eq!(event.global_position, i as u64);
        }

        // Appending a new event should start at global position 5.
        let new_stream_id = uuid::Uuid::new_v4().to_string();
        let resp = client
            .append(proto::AppendRequest {
                stream_id: new_stream_id,
                expected_version: no_stream(),
                events: vec![make_proposed("PostRecovery")],
            })
            .await
            .expect("append after recovery should succeed");

        let resp = resp.into_inner();
        assert_eq!(
            resp.first_global_position, 5,
            "new event should start at global position 5"
        );

        handle.shutdown().await;
    }
}

// -- Test AC-8: End-to-end round-trip across two streams --

#[tokio::test]
async fn ac8_end_to_end_round_trip() {
    let dir = tempfile::tempdir().expect("failed to create tempdir");
    let data_path = dir.path().join("events.log");
    let listen_addr: SocketAddr = EPHEMERAL_ADDR.parse().expect("valid addr");

    let (mut client, _handle) = start_server_at(&data_path, listen_addr, 1024).await;

    let stream_a = uuid::Uuid::new_v4().to_string();
    let stream_b = uuid::Uuid::new_v4().to_string();

    // 1. Append 3 events to stream A with no_stream.
    client
        .append(proto::AppendRequest {
            stream_id: stream_a.clone(),
            expected_version: no_stream(),
            events: (0..3).map(|i| make_proposed(&format!("A{i}"))).collect(),
        })
        .await
        .expect("append to stream A should succeed");

    // 2. Append 2 events to stream B with no_stream.
    client
        .append(proto::AppendRequest {
            stream_id: stream_b.clone(),
            expected_version: no_stream(),
            events: (0..2).map(|i| make_proposed(&format!("B{i}"))).collect(),
        })
        .await
        .expect("append to stream B should succeed");

    // 3. ReadStream for A from version 0 -- assert 3 events.
    let resp = client
        .read_stream(proto::ReadStreamRequest {
            stream_id: stream_a.clone(),
            from_version: 0,
            max_count: 100,
        })
        .await
        .expect("read_stream A should succeed");

    let events_a = resp.into_inner().events;
    assert_eq!(events_a.len(), 3, "stream A should have 3 events");
    for (i, event) in events_a.iter().enumerate() {
        assert_eq!(event.stream_version, i as u64);
        assert_eq!(event.stream_id, stream_a);
    }

    // 4. ReadStream for B from version 0 -- assert 2 events.
    let resp = client
        .read_stream(proto::ReadStreamRequest {
            stream_id: stream_b.clone(),
            from_version: 0,
            max_count: 100,
        })
        .await
        .expect("read_stream B should succeed");

    let events_b = resp.into_inner().events;
    assert_eq!(events_b.len(), 2, "stream B should have 2 events");
    for (i, event) in events_b.iter().enumerate() {
        assert_eq!(event.stream_version, i as u64);
        assert_eq!(event.stream_id, stream_b);
    }

    // 5. ReadAll from position 0 -- assert 5 events in global_position order 0..4.
    let resp = client
        .read_all(proto::ReadAllRequest {
            from_position: 0,
            max_count: 100,
        })
        .await
        .expect("read_all should succeed");

    let all_events = resp.into_inner().events;
    assert_eq!(all_events.len(), 5, "should have 5 total events");
    for (i, event) in all_events.iter().enumerate() {
        assert_eq!(
            event.global_position, i as u64,
            "global position should be contiguous"
        );
    }

    // 6. SubscribeAll from position 0 -- collect until CaughtUp, assert 5 events.
    let mut sub = client
        .subscribe_all(proto::SubscribeAllRequest { from_position: 0 })
        .await
        .expect("subscribe_all should succeed")
        .into_inner();

    let timeout_dur = std::time::Duration::from_secs(5);
    let mut sub_events = Vec::new();

    loop {
        let msg = tokio::time::timeout(timeout_dur, sub.message())
            .await
            .expect("should not timeout")
            .expect("message should succeed")
            .expect("stream should not end");

        match msg.content.expect("content should be set") {
            proto::subscribe_response::Content::Event(e) => {
                sub_events.push(e);
            }
            proto::subscribe_response::Content::CaughtUp(_) => break,
        }
    }

    assert_eq!(
        sub_events.len(),
        5,
        "subscription should receive 5 events before CaughtUp"
    );
    for (i, event) in sub_events.iter().enumerate() {
        assert_eq!(
            event.global_position, i as u64,
            "subscription event positions should be contiguous"
        );
    }
}

// -- Test AC-2: Binary exits non-zero without EVENTFOLD_DATA --
//
// Complementary to the unit test in src/main.rs (binary_exits_nonzero_without_eventfold_data).
// This version runs from the integration test harness and verifies both exit code and stderr.

#[test]
fn binary_exits_nonzero_without_eventfold_data() {
    let output = std::process::Command::new("cargo")
        .args(["run", "--bin", "eventfold-db", "--quiet"])
        .env_remove("EVENTFOLD_DATA")
        .env_remove("EVENTFOLD_LISTEN")
        .env_remove("EVENTFOLD_BROKER_CAPACITY")
        .output()
        .expect("failed to execute cargo run");

    assert!(
        !output.status.success(),
        "expected non-zero exit when EVENTFOLD_DATA is unset"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("EVENTFOLD_DATA"),
        "stderr should mention EVENTFOLD_DATA, got: {stderr}"
    );
}

// -- Test AC-7: Graceful shutdown durability at the writer level --
//
// Verifies that events appended via WriterHandle are durable on disk after
// the writer task shuts down. This exercises the writer-level lifecycle
// directly (no gRPC layer) to confirm fsync + clean drain before exit.

#[tokio::test]
async fn ac7_graceful_shutdown_durability() {
    let dir = tempfile::tempdir().expect("failed to create tempdir");
    let data_path = dir.path().join("events.log");

    // First run: open store, spawn writer, append 1 event, shut down cleanly.
    {
        let store = Store::open(&data_path).expect("store open should succeed");
        let broker = Broker::new(64);
        let (writer_handle, _read_index, writer_join) = spawn_writer(store, 8, broker);

        let stream_id = uuid::Uuid::new_v4();
        let event = ProposedEvent {
            event_id: uuid::Uuid::new_v4(),
            event_type: "ShutdownTest".to_string(),
            metadata: bytes::Bytes::new(),
            payload: bytes::Bytes::from_static(b"{\"key\":\"value\"}"),
        };

        let result = writer_handle
            .append(stream_id, ExpectedVersion::NoStream, vec![event])
            .await
            .expect("append should succeed");
        assert_eq!(result.len(), 1, "should have appended exactly 1 event");
        assert_eq!(result[0].global_position, 0);

        // Drop the WriterHandle to close the mpsc channel, signaling the writer
        // task to drain remaining work and exit.
        drop(writer_handle);

        // Await the writer task's JoinHandle to ensure it finishes cleanly.
        writer_join
            .await
            .expect("writer task should exit without panic");
    }

    // Second run: re-open store at the same path and verify the event is durable.
    {
        let store = Store::open(&data_path).expect("store reopen should succeed");
        let read_index = ReadIndex::new(store.log());

        let events = read_index.read_all(0, 10);
        assert_eq!(
            events.len(),
            1,
            "expected 1 durable event after restart, got {}",
            events.len()
        );
        assert_eq!(events[0].global_position, 0);
        assert_eq!(events[0].event_type, "ShutdownTest");
    }
}
