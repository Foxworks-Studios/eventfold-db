//! Integration tests for the Prometheus metrics endpoint.
//!
//! Each test spins up a real in-process server (gRPC + metrics HTTP endpoint on
//! ephemeral ports), performs gRPC operations, and scrapes `GET /metrics` to
//! verify all eight metric names appear with correct values.
//!
//! All tests use `#[serial]` because the metrics recorder is process-global.

use std::net::SocketAddr;
use std::num::NonZeroUsize;

use eventfold_db::metrics;
use eventfold_db::proto::event_store_client::EventStoreClient;
use eventfold_db::proto::event_store_server::EventStoreServer;
use eventfold_db::proto::{self, expected_version};
use eventfold_db::{Broker, EventfoldService, Store, spawn_writer};
use serial_test::serial;
use tempfile::TempDir;
use tonic::transport::Channel;

/// Default dedup capacity for integration tests.
fn test_dedup_cap() -> NonZeroUsize {
    NonZeroUsize::new(128).expect("nonzero")
}

/// Return value from `start_metrics_test_server`.
struct TestServer {
    /// gRPC client connected to the in-process server.
    client: EventStoreClient<Channel>,
    /// Address of the metrics HTTP endpoint.
    metrics_addr: SocketAddr,
    /// Temp directory holding the event log (must be kept alive).
    _dir: TempDir,
}

/// Spin up an in-process gRPC server and a separate axum metrics HTTP server,
/// both on ephemeral ports. Returns a connected gRPC client and the metrics
/// server address.
///
/// The metrics recorder is installed (or the existing one is reused if already
/// installed by another test in the same process).
async fn start_metrics_test_server() -> TestServer {
    // Install the metrics recorder (tolerate AlreadyInstalled).
    let _ = metrics::install_recorder();
    let handle = metrics::get_installed_handle()
        .expect("metrics recorder should be installed after install_recorder() call");

    // Open a Store at a tempdir.
    let dir = tempfile::tempdir().expect("failed to create tempdir");
    let path = dir.path().join("events.log");
    let store = Store::open(&path).expect("open should succeed");

    // Spawn writer, broker.
    let broker = Broker::new(1024);
    let (writer_handle, read_index, _join_handle) =
        spawn_writer(store, 64, broker.clone(), test_dedup_cap());

    // Build the gRPC service with health.
    let service = EventfoldService::new(writer_handle, read_index, broker);
    let (health_reporter, health_service) = tonic_health::server::health_reporter();

    // Bind the gRPC server on [::1]:0.
    let grpc_listener = tokio::net::TcpListener::bind("[::1]:0")
        .await
        .expect("grpc bind should succeed");
    let grpc_addr = grpc_listener.local_addr().expect("should have local addr");
    let grpc_incoming = tokio_stream::wrappers::TcpListenerStream::new(grpc_listener);

    health_reporter
        .set_serving::<EventStoreServer<EventfoldService>>()
        .await;
    health_reporter
        .set_service_status("", tonic_health::ServingStatus::Serving)
        .await;

    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(health_service)
            .add_service(EventStoreServer::new(service))
            .serve_with_incoming(grpc_incoming)
            .await
            .expect("grpc server should run");
    });

    // Bind the axum metrics server on 127.0.0.1:0 using the production
    // `serve_metrics_on_listener()` function. We pre-bind the listener to learn
    // the ephemeral port, then pass it to the production code path so this test
    // exercises the same route definition as `serve_metrics()`.
    let metrics_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("metrics bind should succeed");
    let metrics_addr = metrics_listener
        .local_addr()
        .expect("should have metrics local addr");

    let _metrics_handle = metrics::serve_metrics_on_listener(handle, metrics_listener);

    // Give both servers a moment to start.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = EventStoreClient::connect(format!("http://[::1]:{}", grpc_addr.port()))
        .await
        .expect("client connect should succeed");

    TestServer {
        client,
        metrics_addr,
        _dir: dir,
    }
}

/// Scrape `GET /metrics` from the given address using a raw HTTP/1.1 request.
///
/// Returns the full HTTP response as a string (headers + body).
async fn scrape_raw(addr: SocketAddr) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = tokio::net::TcpStream::connect(addr)
        .await
        .expect("should connect to metrics endpoint");

    let request = format!("GET /metrics HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("should write request");

    let mut buf = Vec::with_capacity(4096);
    stream
        .read_to_end(&mut buf)
        .await
        .expect("should read response");

    String::from_utf8(buf).expect("response should be valid UTF-8")
}

/// Scrape `GET /metrics` and return only the body (after the blank line).
async fn scrape_body(addr: SocketAddr) -> String {
    let raw = scrape_raw(addr).await;
    // HTTP response: headers separated from body by \r\n\r\n.
    raw.split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or_default()
}

/// Extract the numeric value for a metric line from Prometheus-format text.
///
/// Searches for a line starting with `prefix` (e.g.,
/// `eventfold_reads_total{rpc="read_all"} `) and returns its parsed `f64`
/// value. Returns `None` if not found.
fn parse_metric_value(rendered: &str, prefix: &str) -> Option<f64> {
    rendered.lines().find_map(|line| {
        line.strip_prefix(prefix)
            .and_then(|rest| rest.trim().parse::<f64>().ok())
    })
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

// -- Test: metrics endpoint returns 200 with correct content type --

#[tokio::test]
#[serial]
async fn metrics_endpoint_returns_200_with_correct_content_type() {
    let server = start_metrics_test_server().await;

    let raw = scrape_raw(server.metrics_addr).await;

    // First line should be "HTTP/1.1 200 OK".
    let first_line = raw
        .lines()
        .next()
        .expect("response should have a first line");
    assert!(
        first_line.contains("200"),
        "expected 200 status, got: {first_line}"
    );

    // Headers should contain content-type with text/plain and version=0.0.4.
    let headers = raw
        .split_once("\r\n\r\n")
        .map(|(h, _)| h.to_lowercase())
        .unwrap_or_default();
    assert!(
        headers.contains("text/plain"),
        "content-type should contain text/plain, got headers: {headers}"
    );
    assert!(
        headers.contains("version=0.0.4"),
        "content-type should contain version=0.0.4, got headers: {headers}"
    );
}

// -- Test: metrics counters correct after appends --

#[tokio::test]
#[serial]
async fn metrics_counters_correct_after_appends() {
    let mut server = start_metrics_test_server().await;

    // Snapshot counters before operations (delta-safe for counters which use increment).
    // Gauges that use set() are checked as absolute values after operations.
    let before = scrape_body(server.metrics_addr).await;
    let appends_before = parse_metric_value(&before, "eventfold_appends_total ").unwrap_or(0.0);
    let events_before = parse_metric_value(&before, "eventfold_events_total ").unwrap_or(0.0);

    // Append 1 event to stream A (first call).
    let stream_a = uuid::Uuid::new_v4().to_string();
    server
        .client
        .append(proto::AppendRequest {
            stream_id: stream_a.clone(),
            expected_version: no_stream(),
            events: vec![make_proposed("A0")],
        })
        .await
        .expect("append A0 should succeed");

    // Append 1 event to stream A (second call, version 0 -> 1).
    server
        .client
        .append(proto::AppendRequest {
            stream_id: stream_a,
            expected_version: Some(proto::ExpectedVersion {
                kind: Some(expected_version::Kind::Exact(0)),
            }),
            events: vec![make_proposed("A1")],
        })
        .await
        .expect("append A1 should succeed");

    // Append 1 event to stream B.
    let stream_b = uuid::Uuid::new_v4().to_string();
    server
        .client
        .append(proto::AppendRequest {
            stream_id: stream_b,
            expected_version: no_stream(),
            events: vec![make_proposed("B0")],
        })
        .await
        .expect("append B0 should succeed");

    // Scrape after operations.
    let after = scrape_body(server.metrics_addr).await;
    let appends_after = parse_metric_value(&after, "eventfold_appends_total ")
        .expect("eventfold_appends_total should exist");
    let events_after = parse_metric_value(&after, "eventfold_events_total ")
        .expect("eventfold_events_total should exist");

    // Counters use increment(), so delta is reliable across process-global state.
    // 3 separate Append RPC calls.
    assert_eq!(
        appends_after - appends_before,
        3.0,
        "eventfold_appends_total delta should be 3"
    );
    // 3 total events (1 + 1 + 1).
    assert_eq!(
        events_after - events_before,
        3.0,
        "eventfold_events_total delta should be 3"
    );

    // Gauges use set(), so the absolute value reflects the last writer task's
    // state. After our 3 appends (positions 0, 1, 2), global_position = 3
    // and streams_total = 2.
    let global_pos = parse_metric_value(&after, "eventfold_global_position ")
        .expect("eventfold_global_position should exist");
    assert_eq!(global_pos, 3.0, "eventfold_global_position should be 3");
    let streams = parse_metric_value(&after, "eventfold_streams_total ")
        .expect("eventfold_streams_total should exist");
    assert_eq!(streams, 2.0, "eventfold_streams_total should be 2");
}

// -- Test: histogram present after append --

#[tokio::test]
#[serial]
async fn metrics_histogram_present_after_append() {
    let mut server = start_metrics_test_server().await;

    // Append 1 event.
    server
        .client
        .append(proto::AppendRequest {
            stream_id: uuid::Uuid::new_v4().to_string(),
            expected_version: no_stream(),
            events: vec![make_proposed("Evt")],
        })
        .await
        .expect("append should succeed");

    let body = scrape_body(server.metrics_addr).await;

    // The metrics-exporter-prometheus crate renders histogram! data as a
    // summary (quantile lines + _sum + _count) by default in v0.16.
    // Check for the summary suffix lines that prove the histogram recorded data.
    assert!(
        body.contains("eventfold_append_duration_seconds_sum"),
        "body should contain eventfold_append_duration_seconds_sum"
    );
    assert!(
        body.contains("eventfold_append_duration_seconds_count"),
        "body should contain eventfold_append_duration_seconds_count"
    );
    // Also verify quantile lines are present.
    assert!(
        body.contains("eventfold_append_duration_seconds{quantile="),
        "body should contain quantile lines for append duration"
    );
}

// -- Test: reads_total labeled correctly --

#[tokio::test]
#[serial]
async fn metrics_reads_total_labeled_correctly() {
    let mut server = start_metrics_test_server().await;

    // Need to create a stream to read from (ReadStream requires it to exist).
    let stream_id = uuid::Uuid::new_v4().to_string();
    server
        .client
        .append(proto::AppendRequest {
            stream_id: stream_id.clone(),
            expected_version: no_stream(),
            events: vec![make_proposed("Evt")],
        })
        .await
        .expect("append should succeed");

    // Snapshot before read operations.
    let before = scrape_body(server.metrics_addr).await;
    let rs_before =
        parse_metric_value(&before, r#"eventfold_reads_total{rpc="read_stream"} "#).unwrap_or(0.0);
    let ra_before =
        parse_metric_value(&before, r#"eventfold_reads_total{rpc="read_all"} "#).unwrap_or(0.0);

    // Call ReadStream once.
    server
        .client
        .read_stream(proto::ReadStreamRequest {
            stream_id,
            from_version: 0,
            max_count: 100,
        })
        .await
        .expect("read_stream should succeed");

    // Call ReadAll twice.
    for _ in 0..2 {
        server
            .client
            .read_all(proto::ReadAllRequest {
                from_position: 0,
                max_count: 100,
            })
            .await
            .expect("read_all should succeed");
    }

    // Scrape after operations.
    let after = scrape_body(server.metrics_addr).await;
    let rs_after = parse_metric_value(&after, r#"eventfold_reads_total{rpc="read_stream"} "#)
        .expect("read_stream counter should exist");
    let ra_after = parse_metric_value(&after, r#"eventfold_reads_total{rpc="read_all"} "#)
        .expect("read_all counter should exist");

    assert_eq!(
        rs_after - rs_before,
        1.0,
        "eventfold_reads_total{{rpc=\"read_stream\"}} delta should be 1"
    );
    assert_eq!(
        ra_after - ra_before,
        2.0,
        "eventfold_reads_total{{rpc=\"read_all\"}} delta should be 2"
    );
}

// -- Test: log_bytes nonzero after append --

#[tokio::test]
#[serial]
async fn metrics_log_bytes_nonzero_after_append() {
    let mut server = start_metrics_test_server().await;

    // Append 1 event.
    server
        .client
        .append(proto::AppendRequest {
            stream_id: uuid::Uuid::new_v4().to_string(),
            expected_version: no_stream(),
            events: vec![make_proposed("Evt")],
        })
        .await
        .expect("append should succeed");

    let body = scrape_body(server.metrics_addr).await;
    let log_bytes = parse_metric_value(&body, "eventfold_log_bytes ")
        .expect("eventfold_log_bytes should exist");

    assert!(
        log_bytes > 0.0,
        "eventfold_log_bytes should be > 0 after append, got: {log_bytes}"
    );
}

// -- Test: subscriptions_active gauge --

#[tokio::test]
#[serial]
async fn metrics_subscriptions_active_gauge() {
    let mut server = start_metrics_test_server().await;

    // Append 1 event so there's something for the subscription to receive.
    let stream_id = uuid::Uuid::new_v4().to_string();
    server
        .client
        .append(proto::AppendRequest {
            stream_id,
            expected_version: no_stream(),
            events: vec![make_proposed("Evt")],
        })
        .await
        .expect("append should succeed");

    // Open a SubscribeAll stream from position 0.
    let mut sub = server
        .client
        .subscribe_all(proto::SubscribeAllRequest { from_position: 0 })
        .await
        .expect("subscribe_all should succeed")
        .into_inner();

    let timeout_dur = std::time::Duration::from_secs(5);

    // Read the first event to confirm the stream is connected.
    let msg = tokio::time::timeout(timeout_dur, sub.message())
        .await
        .expect("should not timeout")
        .expect("message should succeed")
        .expect("stream should not end");
    assert!(msg.content.is_some(), "first message should have content");

    // Small delay to ensure the gauge is updated.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Scrape: subscriptions_active should be >= 1.
    let body = scrape_body(server.metrics_addr).await;
    let active = parse_metric_value(&body, "eventfold_subscriptions_active ")
        .expect("eventfold_subscriptions_active should exist");
    assert!(
        active >= 1.0,
        "eventfold_subscriptions_active should be >= 1 while stream is open, got: {active}"
    );

    // Drop the subscription stream.
    drop(sub);

    // Give the runtime time to run the cleanup path.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Scrape again: subscriptions_active should be 0.
    let body = scrape_body(server.metrics_addr).await;
    let active = parse_metric_value(&body, "eventfold_subscriptions_active ")
        .expect("eventfold_subscriptions_active should exist after drop");
    assert_eq!(
        active, 0.0,
        "eventfold_subscriptions_active should be 0 after stream is dropped"
    );
}

// -- AC 7: metrics_disabled_when_env_var_empty --
//
// The behavior of `EVENTFOLD_METRICS_LISTEN=""` disabling the metrics endpoint
// is tested at the Config-parsing level in `src/main.rs`:
//
//   - `main::tests::from_env_metrics_listen_empty_string_gives_none` asserts that
//     setting `EVENTFOLD_METRICS_LISTEN=""` produces `config.metrics_listen == None`.
//
//   - `main()` checks `config.metrics_listen` and only calls `serve_metrics()` when
//     it is `Some(addr)`, logging "Metrics endpoint disabled" when `None`.
//
// This is not duplicated here because spinning up a full integration server with
// metrics disabled would leave no HTTP endpoint to assert against -- the absence
// of a listener is the correct behavior and is verified by the unit test above.

// -- Test: metrics_custom_port_via_env --

#[tokio::test]
#[serial]
async fn metrics_custom_port_via_env() {
    // Verify that the metrics server can be started on a specific (non-default)
    // address and responds there. This exercises the production code path
    // `serve_metrics_on_listener()` with a caller-chosen address.
    let _ = metrics::install_recorder();
    let handle = metrics::get_installed_handle()
        .expect("metrics recorder should be installed after install_recorder() call");

    // Bind on an ephemeral port -- this simulates a user setting
    // EVENTFOLD_METRICS_LISTEN to a custom address. The key assertion is that the
    // server responds on exactly the address we gave it, not on the default 9090.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind should succeed");
    let custom_addr = listener.local_addr().expect("should have local addr");

    // The custom port must NOT be the default (9090).
    assert_ne!(
        custom_addr.port(),
        9090,
        "ephemeral port should not be the default metrics port"
    );

    let _server_handle = metrics::serve_metrics_on_listener(handle, listener);

    // Give the server a moment to start.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Scrape the custom address -- it should respond with HTTP 200.
    let raw = scrape_raw(custom_addr).await;
    let first_line = raw
        .lines()
        .next()
        .expect("response should have a first line");
    assert!(
        first_line.contains("200"),
        "metrics server on custom port should return 200, got: {first_line}"
    );

    // Verify the default port (9090) is NOT serving. A connection attempt to
    // 127.0.0.1:9090 should fail (unless something else happens to be on 9090,
    // which is unlikely in a test environment).
    let default_addr: SocketAddr = "127.0.0.1:9090".parse().expect("valid addr");
    let connect_result = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        tokio::net::TcpStream::connect(default_addr),
    )
    .await;

    // Either the connection is refused or the attempt times out -- both confirm
    // the default port is not serving our metrics.
    let default_unreachable = match connect_result {
        Err(_) => true,     // Timeout: nothing listening.
        Ok(Err(_)) => true, // Connection refused.
        Ok(Ok(_)) => false, // Something responded -- unexpected.
    };
    assert!(
        default_unreachable,
        "default port 9090 should not be serving when metrics are on a custom port"
    );
}
