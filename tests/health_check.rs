//! Integration tests for the gRPC health check service.
//!
//! Each test spins up a real tonic server with both the health service and the
//! EventStore service on an ephemeral port, then uses the tonic-health generated
//! client to verify health check responses.

use std::net::SocketAddr;
use std::num::NonZeroUsize;

use eventfold_db::proto::event_store_server::EventStoreServer;
use eventfold_db::{Broker, EventfoldService, Store, spawn_writer};
use tempfile::TempDir;
use tonic::transport::Channel;
use tonic_health::pb::HealthCheckRequest;
use tonic_health::pb::health_check_response::ServingStatus;
use tonic_health::pb::health_client::HealthClient;

/// Default dedup capacity for integration tests.
fn test_dedup_cap() -> NonZeroUsize {
    NonZeroUsize::new(128).expect("nonzero")
}

/// Spin up an in-process gRPC server on an ephemeral port with both the health
/// service and EventStoreServer registered. Returns a connected health client,
/// the server address, and the temp directory holding the event log.
///
/// The health service is configured identically to `main.rs`: both the empty
/// service name `""` and `"eventfold.EventStore"` are set to SERVING after the
/// TCP listener is bound.
async fn start_health_test_server() -> (HealthClient<Channel>, SocketAddr, TempDir) {
    let dir = tempfile::tempdir().expect("failed to create tempdir");
    let path = dir.path().join("events.log");
    let store = Store::open(&path).expect("open should succeed");
    let broker = Broker::new(1024);
    let (writer_handle, read_index, _join_handle) =
        spawn_writer(store, 64, broker.clone(), test_dedup_cap());

    let service = EventfoldService::new(writer_handle, read_index, broker);
    let (health_reporter, health_service) = tonic_health::server::health_reporter();

    let listener = tokio::net::TcpListener::bind("[::1]:0")
        .await
        .expect("bind should succeed");
    let addr = listener.local_addr().expect("should have local addr");
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    // Set SERVING after bind, matching main.rs ordering.
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
            .serve_with_incoming(incoming)
            .await
            .expect("server should run");
    });

    // Give the server a moment to start accepting connections.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let channel = Channel::from_shared(format!("http://[::1]:{}", addr.port()))
        .expect("valid URI")
        .connect()
        .await
        .expect("channel connect should succeed");
    let client = HealthClient::new(channel);

    (client, addr, dir)
}

#[tokio::test]
async fn health_check_empty_service_name_returns_serving() {
    let (mut client, _addr, _dir) = start_health_test_server().await;

    let resp = client
        .check(HealthCheckRequest { service: "".into() })
        .await
        .expect("health check for empty service should succeed");

    assert_eq!(
        resp.into_inner().status(),
        ServingStatus::Serving,
        "empty service name should report SERVING"
    );
}

#[tokio::test]
async fn health_check_eventstore_service_name_returns_serving() {
    let (mut client, _addr, _dir) = start_health_test_server().await;

    let resp = client
        .check(HealthCheckRequest {
            service: "eventfold.EventStore".into(),
        })
        .await
        .expect("health check for eventfold.EventStore should succeed");

    assert_eq!(
        resp.into_inner().status(),
        ServingStatus::Serving,
        "eventfold.EventStore should report SERVING"
    );
}

#[tokio::test]
async fn health_check_unknown_service_returns_not_found() {
    let (mut client, _addr, _dir) = start_health_test_server().await;

    let err = client
        .check(HealthCheckRequest {
            service: "nonexistent.Service".into(),
        })
        .await
        .expect_err("health check for unknown service should fail");

    assert_eq!(
        err.code(),
        tonic::Code::NotFound,
        "unknown service should return NOT_FOUND"
    );
}
