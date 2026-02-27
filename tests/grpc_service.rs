//! Integration tests for EventfoldDB gRPC service: Append, ReadStream, ReadAll.
//!
//! Each test spins up a real tonic server on an ephemeral port using
//! `start_test_server`, connects a gRPC client, and exercises the RPCs.

use eventfold_db::proto::event_store_client::EventStoreClient;
use eventfold_db::proto::event_store_server::EventStoreServer;
use eventfold_db::proto::{self, expected_version};
use eventfold_db::{Broker, EventfoldService, Store, spawn_writer};

use std::net::SocketAddr;
use tempfile::TempDir;
use tonic::transport::Channel;

/// Spin up an in-process gRPC server on an ephemeral port and return a connected
/// client, the server address, and the temp directory holding the event log.
///
/// The server runs the full stack: Store -> writer task -> EventfoldService -> tonic.
async fn start_test_server() -> (EventStoreClient<Channel>, SocketAddr, TempDir) {
    start_test_server_with_broker_capacity(1024).await
}

/// Like `start_test_server` but allows specifying the broker broadcast channel capacity.
///
/// Useful for testing lag/overflow scenarios with a small capacity.
async fn start_test_server_with_broker_capacity(
    broker_capacity: usize,
) -> (EventStoreClient<Channel>, SocketAddr, TempDir) {
    let dir = tempfile::tempdir().expect("failed to create tempdir");
    let path = dir.path().join("events.log");
    let store = Store::open(&path).expect("open should succeed");
    let broker = Broker::new(broker_capacity);
    let (writer_handle, read_index, _join_handle) = spawn_writer(store, 64, broker.clone());

    let service = EventfoldService::new(writer_handle, read_index, broker);

    let listener = tokio::net::TcpListener::bind("[::1]:0")
        .await
        .expect("bind should succeed");
    let addr = listener.local_addr().expect("should have local addr");
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(EventStoreServer::new(service))
            .serve_with_incoming(incoming)
            .await
            .expect("server should run");
    });

    // Give the server a moment to start accepting connections.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = EventStoreClient::connect(format!("http://[::1]:{}", addr.port()))
        .await
        .expect("client connect should succeed");

    (client, addr, dir)
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

// -- Test AC-1: Append 1 event, no_stream -> correct first/last positions --

#[tokio::test]
async fn ac1_append_single_event_no_stream() {
    let (mut client, _addr, _dir) = start_test_server().await;

    let stream_id = uuid::Uuid::new_v4().to_string();
    let resp = client
        .append(proto::AppendRequest {
            stream_id,
            expected_version: no_stream(),
            events: vec![make_proposed("TestEvent")],
        })
        .await
        .expect("append should succeed");

    let resp = resp.into_inner();
    assert_eq!(resp.first_stream_version, 0);
    assert_eq!(resp.last_stream_version, 0);
    assert_eq!(resp.first_global_position, 0);
    assert_eq!(resp.last_global_position, 0);
}

// -- Test AC-2: Append 3 events batch -> first_stream_version=0, last=2 --

#[tokio::test]
async fn ac2_append_batch_three_events() {
    let (mut client, _addr, _dir) = start_test_server().await;

    let stream_id = uuid::Uuid::new_v4().to_string();
    let resp = client
        .append(proto::AppendRequest {
            stream_id,
            expected_version: no_stream(),
            events: vec![
                make_proposed("Evt0"),
                make_proposed("Evt1"),
                make_proposed("Evt2"),
            ],
        })
        .await
        .expect("append should succeed");

    let resp = resp.into_inner();
    assert_eq!(resp.first_stream_version, 0);
    assert_eq!(resp.last_stream_version, 2);
    assert_eq!(resp.first_global_position, 0);
    assert_eq!(resp.last_global_position, 2);
}

// -- Test AC-3: Append no_stream twice -> FAILED_PRECONDITION --

#[tokio::test]
async fn ac3_append_no_stream_twice_fails() {
    let (mut client, _addr, _dir) = start_test_server().await;

    let stream_id = uuid::Uuid::new_v4().to_string();

    // First append succeeds.
    client
        .append(proto::AppendRequest {
            stream_id: stream_id.clone(),
            expected_version: no_stream(),
            events: vec![make_proposed("First")],
        })
        .await
        .expect("first append should succeed");

    // Second append with no_stream should fail.
    let err = client
        .append(proto::AppendRequest {
            stream_id,
            expected_version: no_stream(),
            events: vec![make_proposed("Second")],
        })
        .await
        .expect_err("second no_stream append should fail");

    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
}

// -- Test AC-4: Append with stream_id="not-a-uuid" -> INVALID_ARGUMENT --

#[tokio::test]
async fn ac4_append_invalid_stream_id() {
    let (mut client, _addr, _dir) = start_test_server().await;

    let err = client
        .append(proto::AppendRequest {
            stream_id: "not-a-uuid".to_string(),
            expected_version: no_stream(),
            events: vec![make_proposed("Evt")],
        })
        .await
        .expect_err("invalid stream_id should fail");

    assert_eq!(err.code(), tonic::Code::InvalidArgument);
}

// -- Test AC-5: Append with empty events list -> INVALID_ARGUMENT --

#[tokio::test]
async fn ac5_append_empty_events() {
    let (mut client, _addr, _dir) = start_test_server().await;

    let err = client
        .append(proto::AppendRequest {
            stream_id: uuid::Uuid::new_v4().to_string(),
            expected_version: no_stream(),
            events: vec![],
        })
        .await
        .expect_err("empty events should fail");

    assert_eq!(err.code(), tonic::Code::InvalidArgument);
}

// -- Test AC-6: Append with event_id="bad-uuid" -> INVALID_ARGUMENT --

#[tokio::test]
async fn ac6_append_invalid_event_id() {
    let (mut client, _addr, _dir) = start_test_server().await;

    let err = client
        .append(proto::AppendRequest {
            stream_id: uuid::Uuid::new_v4().to_string(),
            expected_version: no_stream(),
            events: vec![proto::ProposedEvent {
                event_id: "bad-uuid".to_string(),
                event_type: "Evt".to_string(),
                metadata: vec![],
                payload: b"{}".to_vec(),
            }],
        })
        .await
        .expect_err("invalid event_id should fail");

    assert_eq!(err.code(), tonic::Code::InvalidArgument);
}

// -- Test AC-7: Append with oversized payload (>64KB) -> INVALID_ARGUMENT --

#[tokio::test]
async fn ac7_append_oversized_payload() {
    let (mut client, _addr, _dir) = start_test_server().await;

    let err = client
        .append(proto::AppendRequest {
            stream_id: uuid::Uuid::new_v4().to_string(),
            expected_version: no_stream(),
            events: vec![proto::ProposedEvent {
                event_id: uuid::Uuid::new_v4().to_string(),
                event_type: "BigEvent".to_string(),
                metadata: vec![],
                // 64 KB + 1 byte payload exceeds MAX_EVENT_SIZE
                payload: vec![0u8; 65_537],
            }],
        })
        .await
        .expect_err("oversized payload should fail");

    assert_eq!(err.code(), tonic::Code::InvalidArgument);
}

// -- Test AC-8: Append 5 events; ReadStream from 0, max 100 -> 5 events in order --

#[tokio::test]
async fn ac8_read_stream_all_events() {
    let (mut client, _addr, _dir) = start_test_server().await;

    let stream_id = uuid::Uuid::new_v4().to_string();
    client
        .append(proto::AppendRequest {
            stream_id: stream_id.clone(),
            expected_version: no_stream(),
            events: (0..5).map(|i| make_proposed(&format!("Evt{i}"))).collect(),
        })
        .await
        .expect("append should succeed");

    let resp = client
        .read_stream(proto::ReadStreamRequest {
            stream_id,
            from_version: 0,
            max_count: 100,
        })
        .await
        .expect("read_stream should succeed");

    let events = resp.into_inner().events;
    assert_eq!(events.len(), 5);
    for (i, event) in events.iter().enumerate() {
        assert_eq!(event.stream_version, i as u64);
        assert_eq!(event.event_type, format!("Evt{i}"));
    }
}

// -- Test AC-9: ReadStream from version 2, max 2 -> 2 events at versions 2,3 --

#[tokio::test]
async fn ac9_read_stream_partial() {
    let (mut client, _addr, _dir) = start_test_server().await;

    let stream_id = uuid::Uuid::new_v4().to_string();
    client
        .append(proto::AppendRequest {
            stream_id: stream_id.clone(),
            expected_version: no_stream(),
            events: (0..5).map(|i| make_proposed(&format!("Evt{i}"))).collect(),
        })
        .await
        .expect("append should succeed");

    let resp = client
        .read_stream(proto::ReadStreamRequest {
            stream_id,
            from_version: 2,
            max_count: 2,
        })
        .await
        .expect("read_stream should succeed");

    let events = resp.into_inner().events;
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].stream_version, 2);
    assert_eq!(events[1].stream_version, 3);
}

// -- Test AC-10: ReadStream non-existent stream -> NOT_FOUND --

#[tokio::test]
async fn ac10_read_stream_not_found() {
    let (mut client, _addr, _dir) = start_test_server().await;

    let err = client
        .read_stream(proto::ReadStreamRequest {
            stream_id: uuid::Uuid::new_v4().to_string(),
            from_version: 0,
            max_count: 100,
        })
        .await
        .expect_err("non-existent stream should fail");

    assert_eq!(err.code(), tonic::Code::NotFound);
}

// -- Test AC-11: ReadStream with stream_id="not-a-uuid" -> INVALID_ARGUMENT --

#[tokio::test]
async fn ac11_read_stream_invalid_stream_id() {
    let (mut client, _addr, _dir) = start_test_server().await;

    let err = client
        .read_stream(proto::ReadStreamRequest {
            stream_id: "not-a-uuid".to_string(),
            from_version: 0,
            max_count: 100,
        })
        .await
        .expect_err("invalid stream_id should fail");

    assert_eq!(err.code(), tonic::Code::InvalidArgument);
}

// -- Test AC-12: Append across 2 streams; ReadAll from 0 -> all events in global order --

#[tokio::test]
async fn ac12_read_all_two_streams() {
    let (mut client, _addr, _dir) = start_test_server().await;

    let stream_a = uuid::Uuid::new_v4().to_string();
    let stream_b = uuid::Uuid::new_v4().to_string();

    // Append 3 events to stream A.
    client
        .append(proto::AppendRequest {
            stream_id: stream_a.clone(),
            expected_version: no_stream(),
            events: (0..3).map(|i| make_proposed(&format!("A{i}"))).collect(),
        })
        .await
        .expect("append A should succeed");

    // Append 2 events to stream B.
    client
        .append(proto::AppendRequest {
            stream_id: stream_b,
            expected_version: no_stream(),
            events: (0..2).map(|i| make_proposed(&format!("B{i}"))).collect(),
        })
        .await
        .expect("append B should succeed");

    let resp = client
        .read_all(proto::ReadAllRequest {
            from_position: 0,
            max_count: 100,
        })
        .await
        .expect("read_all should succeed");

    let events = resp.into_inner().events;
    assert_eq!(events.len(), 5);
    // Verify global positions are 0..4 in order.
    for (i, event) in events.iter().enumerate() {
        assert_eq!(event.global_position, i as u64);
    }
    // First 3 are stream A, next 2 are stream B.
    assert_eq!(events[0].stream_id, stream_a);
    assert_eq!(events[2].stream_id, stream_a);
}

// -- Test AC-13: ReadAll from position 3, max 2 -> events at positions 3,4 --

#[tokio::test]
async fn ac13_read_all_partial() {
    let (mut client, _addr, _dir) = start_test_server().await;

    let stream_id = uuid::Uuid::new_v4().to_string();
    client
        .append(proto::AppendRequest {
            stream_id,
            expected_version: no_stream(),
            events: (0..5).map(|i| make_proposed(&format!("Evt{i}"))).collect(),
        })
        .await
        .expect("append should succeed");

    let resp = client
        .read_all(proto::ReadAllRequest {
            from_position: 3,
            max_count: 2,
        })
        .await
        .expect("read_all should succeed");

    let events = resp.into_inner().events;
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].global_position, 3);
    assert_eq!(events[1].global_position, 4);
}

// -- Test AC-14: ReadAll on empty store -> 0 events --

#[tokio::test]
async fn ac14_read_all_empty_store() {
    let (mut client, _addr, _dir) = start_test_server().await;

    let resp = client
        .read_all(proto::ReadAllRequest {
            from_position: 0,
            max_count: 100,
        })
        .await
        .expect("read_all on empty store should succeed");

    let events = resp.into_inner().events;
    assert_eq!(events.len(), 0);
}

// -- Test AC-15: Append 3 events; SubscribeAll from 0; receive 3 events + CaughtUp;
// append 2 more; receive 2 live events --

#[tokio::test]
async fn ac15_subscribe_all_catchup_then_live() {
    let (mut client, _addr, _dir) = start_test_server().await;

    let stream_id = uuid::Uuid::new_v4().to_string();

    // Append 3 events.
    client
        .append(proto::AppendRequest {
            stream_id: stream_id.clone(),
            expected_version: no_stream(),
            events: (0..3).map(|i| make_proposed(&format!("Evt{i}"))).collect(),
        })
        .await
        .expect("append should succeed");

    // SubscribeAll from position 0.
    let mut sub = client
        .subscribe_all(proto::SubscribeAllRequest { from_position: 0 })
        .await
        .expect("subscribe_all should succeed")
        .into_inner();

    let timeout_dur = std::time::Duration::from_secs(5);

    // Receive 3 catch-up events.
    for i in 0u64..3 {
        let msg = tokio::time::timeout(timeout_dur, sub.message())
            .await
            .expect("should not timeout")
            .expect("message should succeed")
            .expect("stream should not end");
        let content = msg.content.expect("content should be set");
        match content {
            proto::subscribe_response::Content::Event(e) => {
                assert_eq!(e.global_position, i);
            }
            proto::subscribe_response::Content::CaughtUp(_) => {
                panic!("expected Event, got CaughtUp at position {i}");
            }
        }
    }

    // Receive CaughtUp marker.
    let msg = tokio::time::timeout(timeout_dur, sub.message())
        .await
        .expect("should not timeout")
        .expect("message should succeed")
        .expect("stream should not end");
    let content = msg.content.expect("content should be set");
    assert!(
        matches!(content, proto::subscribe_response::Content::CaughtUp(_)),
        "expected CaughtUp marker"
    );

    // Append 2 more events.
    client
        .append(proto::AppendRequest {
            stream_id,
            expected_version: Some(proto::ExpectedVersion {
                kind: Some(expected_version::Kind::Exact(2)),
            }),
            events: (3..5).map(|i| make_proposed(&format!("Evt{i}"))).collect(),
        })
        .await
        .expect("second append should succeed");

    // Receive 2 live events.
    for i in 3u64..5 {
        let msg = tokio::time::timeout(timeout_dur, sub.message())
            .await
            .expect("should not timeout")
            .expect("message should succeed")
            .expect("stream should not end");
        let content = msg.content.expect("content should be set");
        match content {
            proto::subscribe_response::Content::Event(e) => {
                assert_eq!(e.global_position, i);
            }
            proto::subscribe_response::Content::CaughtUp(_) => {
                panic!("expected live Event, got CaughtUp at position {i}");
            }
        }
    }
}

// -- Test AC-16: Append 5 events; SubscribeAll from 3; receive 2 events + CaughtUp --

#[tokio::test]
async fn ac16_subscribe_all_from_middle() {
    let (mut client, _addr, _dir) = start_test_server().await;

    let stream_id = uuid::Uuid::new_v4().to_string();

    // Append 5 events.
    client
        .append(proto::AppendRequest {
            stream_id,
            expected_version: no_stream(),
            events: (0..5).map(|i| make_proposed(&format!("Evt{i}"))).collect(),
        })
        .await
        .expect("append should succeed");

    // SubscribeAll from position 3.
    let mut sub = client
        .subscribe_all(proto::SubscribeAllRequest { from_position: 3 })
        .await
        .expect("subscribe_all should succeed")
        .into_inner();

    let timeout_dur = std::time::Duration::from_secs(5);

    // Receive 2 catch-up events at positions 3 and 4.
    for expected_pos in [3u64, 4] {
        let msg = tokio::time::timeout(timeout_dur, sub.message())
            .await
            .expect("should not timeout")
            .expect("message should succeed")
            .expect("stream should not end");
        let content = msg.content.expect("content should be set");
        match content {
            proto::subscribe_response::Content::Event(e) => {
                assert_eq!(e.global_position, expected_pos);
            }
            proto::subscribe_response::Content::CaughtUp(_) => {
                panic!("expected Event at position {expected_pos}, got CaughtUp");
            }
        }
    }

    // Receive CaughtUp marker.
    let msg = tokio::time::timeout(timeout_dur, sub.message())
        .await
        .expect("should not timeout")
        .expect("message should succeed")
        .expect("stream should not end");
    let content = msg.content.expect("content should be set");
    assert!(
        matches!(content, proto::subscribe_response::Content::CaughtUp(_)),
        "expected CaughtUp marker"
    );
}

// -- Test AC-17: Append to A, B, A; SubscribeStream for A from 0;
// receive 2 events + CaughtUp; append to A; receive live event; no B events --

#[tokio::test]
async fn ac17_subscribe_stream_catchup_and_live() {
    let (mut client, _addr, _dir) = start_test_server().await;

    let stream_a = uuid::Uuid::new_v4().to_string();
    let stream_b = uuid::Uuid::new_v4().to_string();

    // Append to A (version 0).
    client
        .append(proto::AppendRequest {
            stream_id: stream_a.clone(),
            expected_version: no_stream(),
            events: vec![make_proposed("A0")],
        })
        .await
        .expect("append A0 should succeed");

    // Append to B (version 0).
    client
        .append(proto::AppendRequest {
            stream_id: stream_b.clone(),
            expected_version: no_stream(),
            events: vec![make_proposed("B0")],
        })
        .await
        .expect("append B0 should succeed");

    // Append to A (version 1).
    client
        .append(proto::AppendRequest {
            stream_id: stream_a.clone(),
            expected_version: Some(proto::ExpectedVersion {
                kind: Some(expected_version::Kind::Exact(0)),
            }),
            events: vec![make_proposed("A1")],
        })
        .await
        .expect("append A1 should succeed");

    // SubscribeStream for A from version 0.
    let mut sub = client
        .subscribe_stream(proto::SubscribeStreamRequest {
            stream_id: stream_a.clone(),
            from_version: 0,
        })
        .await
        .expect("subscribe_stream should succeed")
        .into_inner();

    let timeout_dur = std::time::Duration::from_secs(5);

    // Receive 2 catch-up events for stream A (versions 0 and 1).
    for expected_ver in [0u64, 1] {
        let msg = tokio::time::timeout(timeout_dur, sub.message())
            .await
            .expect("should not timeout")
            .expect("message should succeed")
            .expect("stream should not end");
        let content = msg.content.expect("content should be set");
        match content {
            proto::subscribe_response::Content::Event(e) => {
                assert_eq!(e.stream_id, stream_a, "should be stream A");
                assert_eq!(e.stream_version, expected_ver);
            }
            proto::subscribe_response::Content::CaughtUp(_) => {
                panic!("expected Event version {expected_ver}, got CaughtUp");
            }
        }
    }

    // Receive CaughtUp marker.
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
        "expected CaughtUp marker"
    );

    // Append another event to A (version 2).
    client
        .append(proto::AppendRequest {
            stream_id: stream_a.clone(),
            expected_version: Some(proto::ExpectedVersion {
                kind: Some(expected_version::Kind::Exact(1)),
            }),
            events: vec![make_proposed("A2")],
        })
        .await
        .expect("append A2 should succeed");

    // Receive live event for stream A (version 2).
    let msg = tokio::time::timeout(timeout_dur, sub.message())
        .await
        .expect("should not timeout")
        .expect("message should succeed")
        .expect("stream should not end");
    let content = msg.content.expect("content should be set");
    match content {
        proto::subscribe_response::Content::Event(e) => {
            assert_eq!(e.stream_id, stream_a, "live event should be stream A");
            assert_eq!(e.stream_version, 2);
        }
        proto::subscribe_response::Content::CaughtUp(_) => {
            panic!("expected live Event, got CaughtUp");
        }
    }
}

// -- Test AC-18: SubscribeStream for A; append to B, C, A, B; only A's event after CaughtUp --

#[tokio::test]
async fn ac18_subscribe_stream_filters_other_streams() {
    let (mut client, _addr, _dir) = start_test_server().await;

    let stream_a = uuid::Uuid::new_v4().to_string();
    let stream_b = uuid::Uuid::new_v4().to_string();
    let stream_c = uuid::Uuid::new_v4().to_string();

    // Subscribe to stream A (empty store).
    let mut sub = client
        .subscribe_stream(proto::SubscribeStreamRequest {
            stream_id: stream_a.clone(),
            from_version: 0,
        })
        .await
        .expect("subscribe_stream should succeed")
        .into_inner();

    let timeout_dur = std::time::Duration::from_secs(5);

    // Receive CaughtUp immediately (stream A does not exist yet).
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
        "expected CaughtUp for non-existent stream"
    );

    // Append to B.
    client
        .append(proto::AppendRequest {
            stream_id: stream_b.clone(),
            expected_version: no_stream(),
            events: vec![make_proposed("B0")],
        })
        .await
        .expect("append B should succeed");

    // Append to C.
    client
        .append(proto::AppendRequest {
            stream_id: stream_c.clone(),
            expected_version: no_stream(),
            events: vec![make_proposed("C0")],
        })
        .await
        .expect("append C should succeed");

    // Append to A.
    client
        .append(proto::AppendRequest {
            stream_id: stream_a.clone(),
            expected_version: no_stream(),
            events: vec![make_proposed("A0")],
        })
        .await
        .expect("append A should succeed");

    // Append to B again.
    client
        .append(proto::AppendRequest {
            stream_id: stream_b.clone(),
            expected_version: Some(proto::ExpectedVersion {
                kind: Some(expected_version::Kind::Exact(0)),
            }),
            events: vec![make_proposed("B1")],
        })
        .await
        .expect("append B1 should succeed");

    // Only A's event should arrive on the subscription.
    let msg = tokio::time::timeout(timeout_dur, sub.message())
        .await
        .expect("should not timeout")
        .expect("message should succeed")
        .expect("stream should not end");
    let content = msg.content.expect("content should be set");
    match content {
        proto::subscribe_response::Content::Event(e) => {
            assert_eq!(e.stream_id, stream_a, "should only receive stream A events");
            assert_eq!(e.stream_version, 0);
            assert_eq!(e.event_type, "A0");
        }
        proto::subscribe_response::Content::CaughtUp(_) => {
            panic!("expected Event, got CaughtUp");
        }
    }
}

// -- Test AC-19: SubscribeStream for non-existent stream; CaughtUp immediately;
// then append; receive live event --

#[tokio::test]
async fn ac19_subscribe_stream_nonexistent_then_live() {
    let (mut client, _addr, _dir) = start_test_server().await;

    let stream_a = uuid::Uuid::new_v4().to_string();

    // Subscribe to a stream that does not exist.
    let mut sub = client
        .subscribe_stream(proto::SubscribeStreamRequest {
            stream_id: stream_a.clone(),
            from_version: 0,
        })
        .await
        .expect("subscribe_stream should succeed")
        .into_inner();

    let timeout_dur = std::time::Duration::from_secs(5);

    // CaughtUp should arrive immediately.
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
        "expected CaughtUp for non-existent stream"
    );

    // Now append an event to this stream.
    client
        .append(proto::AppendRequest {
            stream_id: stream_a.clone(),
            expected_version: no_stream(),
            events: vec![make_proposed("FirstEvt")],
        })
        .await
        .expect("append should succeed");

    // Receive the live event.
    let msg = tokio::time::timeout(timeout_dur, sub.message())
        .await
        .expect("should not timeout")
        .expect("message should succeed")
        .expect("stream should not end");
    let content = msg.content.expect("content should be set");
    match content {
        proto::subscribe_response::Content::Event(e) => {
            assert_eq!(e.stream_id, stream_a);
            assert_eq!(e.stream_version, 0);
            assert_eq!(e.event_type, "FirstEvt");
        }
        proto::subscribe_response::Content::CaughtUp(_) => {
            panic!("expected live Event, got CaughtUp");
        }
    }
}

// -- Test AC-20: SubscribeAll with small broker capacity; overflow -> stream terminates --

#[tokio::test]
async fn ac20_subscribe_all_lag_terminates_stream() {
    // Use a broker with capacity 4 to trigger lag quickly.
    let (mut client, _addr, _dir) = start_test_server_with_broker_capacity(4).await;

    // Start a subscription on the empty store. The subscription registers a broadcast
    // receiver, does catch-up (empty), emits CaughtUp, then enters live phase.
    let mut sub = client
        .subscribe_all(proto::SubscribeAllRequest { from_position: 0 })
        .await
        .expect("subscribe_all should succeed")
        .into_inner();

    let timeout_dur = std::time::Duration::from_secs(5);

    // Receive CaughtUp (empty store).
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
        "expected CaughtUp"
    );

    // Now flood the broker WITHOUT reading from the subscription. Append many events
    // to overflow the broadcast buffer (capacity=4).
    let stream_id = uuid::Uuid::new_v4().to_string();
    client
        .append(proto::AppendRequest {
            stream_id: stream_id.clone(),
            expected_version: no_stream(),
            events: (0..20)
                .map(|i| make_proposed(&format!("Flood{i}")))
                .collect(),
        })
        .await
        .expect("append should succeed");

    // Now try to read from the subscription. It should eventually yield an error
    // (gRPC status from the lag) or the stream should end (None).
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

// -- Test AC-21: 2 concurrent SubscribeAll subscriptions; both receive same events --

#[tokio::test]
async fn ac21_two_concurrent_subscribe_all() {
    let (mut client, _addr, _dir) = start_test_server().await;

    let stream_id = uuid::Uuid::new_v4().to_string();

    // Append 3 events before subscribing.
    client
        .append(proto::AppendRequest {
            stream_id: stream_id.clone(),
            expected_version: no_stream(),
            events: (0..3).map(|i| make_proposed(&format!("Evt{i}"))).collect(),
        })
        .await
        .expect("append should succeed");

    // Start two concurrent subscriptions from position 0.
    let mut sub1 = client
        .subscribe_all(proto::SubscribeAllRequest { from_position: 0 })
        .await
        .expect("subscribe_all 1 should succeed")
        .into_inner();

    let mut sub2 = client
        .subscribe_all(proto::SubscribeAllRequest { from_position: 0 })
        .await
        .expect("subscribe_all 2 should succeed")
        .into_inner();

    let timeout_dur = std::time::Duration::from_secs(5);

    // Helper to drain catch-up events + CaughtUp from a subscription,
    // returning the event types received during catch-up.
    async fn drain_catchup(
        sub: &mut tonic::Streaming<proto::SubscribeResponse>,
        timeout_dur: std::time::Duration,
    ) -> Vec<u64> {
        let mut positions = Vec::new();
        loop {
            let msg = tokio::time::timeout(timeout_dur, sub.message())
                .await
                .expect("should not timeout")
                .expect("message should succeed")
                .expect("stream should not end");
            match msg.content.expect("content should be set") {
                proto::subscribe_response::Content::Event(e) => {
                    positions.push(e.global_position);
                }
                proto::subscribe_response::Content::CaughtUp(_) => break,
            }
        }
        positions
    }

    // Both should receive the same 3 catch-up events.
    let positions1 = drain_catchup(&mut sub1, timeout_dur).await;
    let positions2 = drain_catchup(&mut sub2, timeout_dur).await;

    assert_eq!(positions1, vec![0, 1, 2]);
    assert_eq!(positions2, vec![0, 1, 2]);

    // Append 2 more events.
    client
        .append(proto::AppendRequest {
            stream_id,
            expected_version: Some(proto::ExpectedVersion {
                kind: Some(expected_version::Kind::Exact(2)),
            }),
            events: (3..5).map(|i| make_proposed(&format!("Evt{i}"))).collect(),
        })
        .await
        .expect("second append should succeed");

    // Both should receive the same 2 live events.
    for sub in [&mut sub1, &mut sub2] {
        for expected_pos in [3u64, 4] {
            let msg = tokio::time::timeout(timeout_dur, sub.message())
                .await
                .expect("should not timeout")
                .expect("message should succeed")
                .expect("stream should not end");
            let content = msg.content.expect("content should be set");
            match content {
                proto::subscribe_response::Content::Event(e) => {
                    assert_eq!(e.global_position, expected_pos);
                }
                proto::subscribe_response::Content::CaughtUp(_) => {
                    panic!("expected Event at position {expected_pos}, got CaughtUp");
                }
            }
        }
    }
}

// -- Smoke test: exercise all 5 RPCs in a single test --

#[tokio::test]
async fn all_five_rpcs_smoke_test() {
    let (mut client, _addr, _dir) = start_test_server().await;

    let stream_id = uuid::Uuid::new_v4().to_string();
    let timeout_dur = std::time::Duration::from_secs(5);

    // 1. Append: 1 event with no_stream.
    let append_resp = client
        .append(proto::AppendRequest {
            stream_id: stream_id.clone(),
            expected_version: no_stream(),
            events: vec![make_proposed("SmokeEvt")],
        })
        .await
        .expect("append should succeed")
        .into_inner();

    assert_eq!(append_resp.first_stream_version, 0);
    assert_eq!(append_resp.last_stream_version, 0);
    assert_eq!(append_resp.first_global_position, 0);
    assert_eq!(append_resp.last_global_position, 0);

    // 2. ReadStream: from version 0.
    let read_stream_resp = client
        .read_stream(proto::ReadStreamRequest {
            stream_id: stream_id.clone(),
            from_version: 0,
            max_count: 100,
        })
        .await
        .expect("read_stream should succeed")
        .into_inner();

    assert_eq!(read_stream_resp.events.len(), 1);
    assert_eq!(read_stream_resp.events[0].stream_version, 0);
    assert_eq!(read_stream_resp.events[0].event_type, "SmokeEvt");
    assert_eq!(read_stream_resp.events[0].stream_id, stream_id);

    // 3. ReadAll: from position 0.
    let read_all_resp = client
        .read_all(proto::ReadAllRequest {
            from_position: 0,
            max_count: 100,
        })
        .await
        .expect("read_all should succeed")
        .into_inner();

    assert_eq!(read_all_resp.events.len(), 1);
    assert_eq!(read_all_resp.events[0].global_position, 0);
    assert_eq!(read_all_resp.events[0].event_type, "SmokeEvt");

    // 4. SubscribeAll: from position 0, collect until CaughtUp.
    let mut sub_all = client
        .subscribe_all(proto::SubscribeAllRequest { from_position: 0 })
        .await
        .expect("subscribe_all should succeed")
        .into_inner();

    // Expect 1 event then CaughtUp.
    let msg = tokio::time::timeout(timeout_dur, sub_all.message())
        .await
        .expect("should not timeout")
        .expect("message should succeed")
        .expect("stream should not end");
    match msg.content.expect("content should be set") {
        proto::subscribe_response::Content::Event(e) => {
            assert_eq!(e.global_position, 0);
            assert_eq!(e.event_type, "SmokeEvt");
        }
        proto::subscribe_response::Content::CaughtUp(_) => {
            panic!("expected Event, got CaughtUp");
        }
    }

    let msg = tokio::time::timeout(timeout_dur, sub_all.message())
        .await
        .expect("should not timeout")
        .expect("message should succeed")
        .expect("stream should not end");
    assert!(
        matches!(
            msg.content,
            Some(proto::subscribe_response::Content::CaughtUp(_))
        ),
        "expected CaughtUp marker after catch-up events"
    );

    // 5. SubscribeStream: for our stream from version 0, collect until CaughtUp.
    let mut sub_stream = client
        .subscribe_stream(proto::SubscribeStreamRequest {
            stream_id: stream_id.clone(),
            from_version: 0,
        })
        .await
        .expect("subscribe_stream should succeed")
        .into_inner();

    // Expect 1 event then CaughtUp.
    let msg = tokio::time::timeout(timeout_dur, sub_stream.message())
        .await
        .expect("should not timeout")
        .expect("message should succeed")
        .expect("stream should not end");
    match msg.content.expect("content should be set") {
        proto::subscribe_response::Content::Event(e) => {
            assert_eq!(e.stream_id, stream_id);
            assert_eq!(e.stream_version, 0);
            assert_eq!(e.event_type, "SmokeEvt");
        }
        proto::subscribe_response::Content::CaughtUp(_) => {
            panic!("expected Event, got CaughtUp");
        }
    }

    let msg = tokio::time::timeout(timeout_dur, sub_stream.message())
        .await
        .expect("should not timeout")
        .expect("message should succeed")
        .expect("stream should not end");
    assert!(
        matches!(
            msg.content,
            Some(proto::subscribe_response::Content::CaughtUp(_))
        ),
        "expected CaughtUp marker after stream catch-up"
    );
}

// -- Error codes test: verify INVALID_ARGUMENT, NOT_FOUND, FAILED_PRECONDITION in one test --

#[tokio::test]
async fn error_codes_end_to_end() {
    let (mut client, _addr, _dir) = start_test_server().await;

    // 1. Append with invalid stream_id -> INVALID_ARGUMENT.
    let err = client
        .append(proto::AppendRequest {
            stream_id: "not-a-uuid".to_string(),
            expected_version: no_stream(),
            events: vec![make_proposed("Evt")],
        })
        .await
        .expect_err("invalid stream_id should fail");
    assert_eq!(
        err.code(),
        tonic::Code::InvalidArgument,
        "invalid stream_id should yield INVALID_ARGUMENT"
    );

    // 2. ReadStream for non-existent stream -> NOT_FOUND.
    let err = client
        .read_stream(proto::ReadStreamRequest {
            stream_id: uuid::Uuid::new_v4().to_string(),
            from_version: 0,
            max_count: 100,
        })
        .await
        .expect_err("non-existent stream should fail");
    assert_eq!(
        err.code(),
        tonic::Code::NotFound,
        "non-existent stream should yield NOT_FOUND"
    );

    // 3. Append no_stream twice -> FAILED_PRECONDITION.
    let stream_id = uuid::Uuid::new_v4().to_string();

    client
        .append(proto::AppendRequest {
            stream_id: stream_id.clone(),
            expected_version: no_stream(),
            events: vec![make_proposed("First")],
        })
        .await
        .expect("first append should succeed");

    let err = client
        .append(proto::AppendRequest {
            stream_id,
            expected_version: no_stream(),
            events: vec![make_proposed("Second")],
        })
        .await
        .expect_err("second no_stream append should fail");
    assert_eq!(
        err.code(),
        tonic::Code::FailedPrecondition,
        "duplicate no_stream should yield FAILED_PRECONDITION"
    );
}
