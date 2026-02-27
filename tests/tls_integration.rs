//! Integration tests for EventfoldDB TLS and mTLS support.
//!
//! Uses `rcgen` to generate self-signed CA, server, and client certificates
//! in-process -- no static fixture files needed. Tests verify that TLS and mTLS
//! servers accept valid clients and reject invalid ones.

use std::net::SocketAddr;
use std::num::NonZeroUsize;

use eventfold_db::proto::event_store_client::EventStoreClient;
use eventfold_db::proto::event_store_server::EventStoreServer;
use eventfold_db::proto::{self, expected_version};
use eventfold_db::{Broker, EventfoldService, Store, spawn_writer};
use tempfile::TempDir;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity, ServerTlsConfig};

/// Default dedup capacity for integration tests.
fn test_dedup_cap() -> NonZeroUsize {
    NonZeroUsize::new(128).expect("nonzero")
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

/// Generated certificate material for a CA and server.
struct TlsCerts {
    /// PEM-encoded CA certificate.
    ca_pem: Vec<u8>,
    /// PEM-encoded server certificate (signed by CA).
    server_cert_pem: Vec<u8>,
    /// PEM-encoded server private key.
    server_key_pem: Vec<u8>,
}

/// Generated certificate material for mTLS (CA + server + client).
struct MtlsCerts {
    /// PEM-encoded CA certificate.
    ca_pem: Vec<u8>,
    /// PEM-encoded server certificate (signed by CA).
    server_cert_pem: Vec<u8>,
    /// PEM-encoded server private key.
    server_key_pem: Vec<u8>,
    /// PEM-encoded client certificate (signed by CA).
    client_cert_pem: Vec<u8>,
    /// PEM-encoded client private key.
    client_key_pem: Vec<u8>,
}

/// Generate a self-signed CA and a server certificate signed by that CA.
///
/// The server certificate includes `SanType::IpAddress` for `[::1]` so that
/// clients connecting to the loopback address pass hostname verification.
fn generate_tls_certs() -> TlsCerts {
    // Generate CA key pair and self-signed certificate.
    let ca_key = rcgen::KeyPair::generate().expect("CA key generation should succeed");
    let mut ca_params = rcgen::CertificateParams::new(vec!["EventfoldDB Test CA".into()])
        .expect("CA params should be valid");
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca_cert = ca_params
        .self_signed(&ca_key)
        .expect("CA self-signing should succeed");

    // Generate server key pair and certificate signed by the CA.
    let server_key = rcgen::KeyPair::generate().expect("server key generation should succeed");
    let mut server_params = rcgen::CertificateParams::new(vec!["localhost".into()])
        .expect("server params should be valid");
    server_params.subject_alt_names = vec![
        rcgen::SanType::IpAddress(std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)),
        rcgen::SanType::IpAddress(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)),
        rcgen::SanType::DnsName("localhost".try_into().expect("localhost is valid DNS")),
    ];
    let server_cert = server_params
        .signed_by(&server_key, &ca_cert, &ca_key)
        .expect("server cert signing should succeed");

    TlsCerts {
        ca_pem: ca_cert.pem().into_bytes(),
        server_cert_pem: server_cert.pem().into_bytes(),
        server_key_pem: server_key.serialize_pem().into_bytes(),
    }
}

/// Generate a self-signed CA, server certificate, and client certificate.
///
/// Both server and client certs are signed by the same CA. The server cert
/// includes SANs for loopback addresses.
fn generate_mtls_certs() -> MtlsCerts {
    // Generate CA key pair and self-signed certificate.
    let ca_key = rcgen::KeyPair::generate().expect("CA key generation should succeed");
    let mut ca_params = rcgen::CertificateParams::new(vec!["EventfoldDB Test CA".into()])
        .expect("CA params should be valid");
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca_cert = ca_params
        .self_signed(&ca_key)
        .expect("CA self-signing should succeed");

    // Server cert signed by this CA.
    let server_key = rcgen::KeyPair::generate().expect("server key generation should succeed");
    let mut server_params = rcgen::CertificateParams::new(vec!["localhost".into()])
        .expect("server params should be valid");
    server_params.subject_alt_names = vec![
        rcgen::SanType::IpAddress(std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)),
        rcgen::SanType::IpAddress(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)),
        rcgen::SanType::DnsName("localhost".try_into().expect("localhost is valid DNS")),
    ];
    let server_cert = server_params
        .signed_by(&server_key, &ca_cert, &ca_key)
        .expect("server cert signing should succeed");

    // Client cert signed by the same CA.
    let client_key = rcgen::KeyPair::generate().expect("client key generation should succeed");
    let client_params = rcgen::CertificateParams::new(vec!["EventfoldDB Test Client".into()])
        .expect("client params should be valid");
    let client_cert = client_params
        .signed_by(&client_key, &ca_cert, &ca_key)
        .expect("client cert signing should succeed");

    MtlsCerts {
        ca_pem: ca_cert.pem().into_bytes(),
        server_cert_pem: server_cert.pem().into_bytes(),
        server_key_pem: server_key.serialize_pem().into_bytes(),
        client_cert_pem: client_cert.pem().into_bytes(),
        client_key_pem: client_key.serialize_pem().into_bytes(),
    }
}

/// Start an in-process gRPC server with TLS enabled on an ephemeral port.
///
/// Returns the server address, CA PEM bytes for client verification, and the
/// `TempDir` holding the event log (kept alive to prevent cleanup).
async fn start_tls_test_server() -> (SocketAddr, Vec<u8>, TempDir) {
    let certs = generate_tls_certs();

    let dir = tempfile::tempdir().expect("failed to create tempdir");
    let path = dir.path().join("events.log");
    let store = Store::open(&path).expect("open should succeed");
    let broker = Broker::new(1024);
    let (writer_handle, read_index, _join_handle) =
        spawn_writer(store, 64, broker.clone(), test_dedup_cap());
    let service = EventfoldService::new(writer_handle, read_index, broker);

    let identity = Identity::from_pem(&certs.server_cert_pem, &certs.server_key_pem);
    let tls_config = ServerTlsConfig::new().identity(identity);

    let listener = tokio::net::TcpListener::bind("[::1]:0")
        .await
        .expect("bind should succeed");
    let addr = listener.local_addr().expect("should have local addr");
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .tls_config(tls_config)
            .expect("TLS config should be valid")
            .add_service(EventStoreServer::new(service))
            .serve_with_incoming(incoming)
            .await
            .expect("TLS server should run");
    });

    // Give the server a moment to start accepting connections.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    (addr, certs.ca_pem, dir)
}

/// Start an in-process gRPC server with mTLS enabled on an ephemeral port.
///
/// Returns the server address, CA PEM bytes, client cert PEM, client key PEM,
/// and the `TempDir` holding the event log.
async fn start_mtls_test_server() -> (SocketAddr, Vec<u8>, Vec<u8>, Vec<u8>, TempDir) {
    let certs = generate_mtls_certs();

    let dir = tempfile::tempdir().expect("failed to create tempdir");
    let path = dir.path().join("events.log");
    let store = Store::open(&path).expect("open should succeed");
    let broker = Broker::new(1024);
    let (writer_handle, read_index, _join_handle) =
        spawn_writer(store, 64, broker.clone(), test_dedup_cap());
    let service = EventfoldService::new(writer_handle, read_index, broker);

    let identity = Identity::from_pem(&certs.server_cert_pem, &certs.server_key_pem);
    let ca_cert = Certificate::from_pem(&certs.ca_pem);
    let tls_config = ServerTlsConfig::new()
        .identity(identity)
        .client_ca_root(ca_cert);

    let listener = tokio::net::TcpListener::bind("[::1]:0")
        .await
        .expect("bind should succeed");
    let addr = listener.local_addr().expect("should have local addr");
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .tls_config(tls_config)
            .expect("mTLS config should be valid")
            .add_service(EventStoreServer::new(service))
            .serve_with_incoming(incoming)
            .await
            .expect("mTLS server should run");
    });

    // Give the server a moment to start accepting connections.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    (
        addr,
        certs.ca_pem,
        certs.client_cert_pem,
        certs.client_key_pem,
        dir,
    )
}

// -- Test: TLS client can append and read events through a TLS server --

#[tokio::test]
async fn tls_client_append_and_read_all() {
    let (addr, ca_pem, _dir) = start_tls_test_server().await;

    let tls_config = ClientTlsConfig::new()
        .ca_certificate(Certificate::from_pem(&ca_pem))
        .domain_name("localhost");

    let channel = Channel::from_shared(format!("https://[::1]:{}", addr.port()))
        .expect("valid URI")
        .tls_config(tls_config)
        .expect("TLS config should be valid")
        .connect()
        .await
        .expect("TLS connection should succeed");

    let mut client = EventStoreClient::new(channel);

    let stream_id = uuid::Uuid::new_v4().to_string();
    client
        .append(proto::AppendRequest {
            stream_id: stream_id.clone(),
            expected_version: no_stream(),
            events: vec![make_proposed("TlsEvent")],
        })
        .await
        .expect("append over TLS should succeed");

    let resp = client
        .read_all(proto::ReadAllRequest {
            from_position: 0,
            max_count: 100,
        })
        .await
        .expect("read_all over TLS should succeed");

    let events = resp.into_inner().events;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "TlsEvent");
    assert_eq!(events[0].stream_id, stream_id);
}

// -- Test: plaintext client is rejected by a TLS server --

#[tokio::test]
async fn tls_plaintext_client_rejected() {
    let (addr, _ca_pem, _dir) = start_tls_test_server().await;

    // Attempt to connect with a plaintext (http://) client to a TLS server.
    let result = EventStoreClient::connect(format!("http://[::1]:{}", addr.port())).await;

    match result {
        Ok(mut client) => {
            // Connection may appear to succeed at the TCP level, but the first RPC
            // should fail with a transport error since the TLS handshake never happened.
            let err = client
                .read_all(proto::ReadAllRequest {
                    from_position: 0,
                    max_count: 1,
                })
                .await
                .expect_err("plaintext RPC to TLS server should fail");
            // The error should be a transport-level error, not a gRPC status.
            // tonic maps connection/protocol errors to transport errors.
            assert!(
                err.code() == tonic::Code::Unavailable
                    || err.code() == tonic::Code::Internal
                    || err.code() == tonic::Code::Unknown,
                "expected transport-level error code, got: {:?}",
                err.code()
            );
        }
        Err(_) => {
            // Connection itself failed, which is also acceptable.
        }
    }
}

// -- Test: mTLS server rejects TLS client that presents no client certificate --

#[tokio::test]
async fn mtls_client_no_cert_rejected() {
    let (addr, ca_pem, _client_cert, _client_key, _dir) = start_mtls_test_server().await;

    // Connect with TLS (correct CA) but without a client identity.
    let tls_config = ClientTlsConfig::new()
        .ca_certificate(Certificate::from_pem(&ca_pem))
        .domain_name("localhost");

    let channel = Channel::from_shared(format!("https://[::1]:{}", addr.port()))
        .expect("valid URI")
        .tls_config(tls_config)
        .expect("TLS config should be valid")
        .connect()
        .await;

    match channel {
        Ok(channel) => {
            // TCP/TLS connection may appear to succeed, but the RPC should fail
            // because the server requires a client certificate.
            let mut client = EventStoreClient::new(channel);
            let err = client
                .read_all(proto::ReadAllRequest {
                    from_position: 0,
                    max_count: 1,
                })
                .await
                .expect_err("RPC without client cert to mTLS server should fail");
            assert!(
                err.code() == tonic::Code::Unavailable
                    || err.code() == tonic::Code::Internal
                    || err.code() == tonic::Code::Unknown,
                "expected transport-level error code, got: {:?}",
                err.code()
            );
        }
        Err(_) => {
            // Connection failed during TLS handshake, which is also acceptable.
        }
    }
}

// -- Test: mTLS server accepts client with valid certificate --

#[tokio::test]
async fn mtls_client_valid_cert_accepted() {
    let (addr, ca_pem, client_cert_pem, client_key_pem, _dir) = start_mtls_test_server().await;

    let client_identity = Identity::from_pem(&client_cert_pem, &client_key_pem);
    let tls_config = ClientTlsConfig::new()
        .ca_certificate(Certificate::from_pem(&ca_pem))
        .domain_name("localhost")
        .identity(client_identity);

    let channel = Channel::from_shared(format!("https://[::1]:{}", addr.port()))
        .expect("valid URI")
        .tls_config(tls_config)
        .expect("TLS config should be valid")
        .connect()
        .await
        .expect("mTLS connection with valid client cert should succeed");

    let mut client = EventStoreClient::new(channel);

    let stream_id = uuid::Uuid::new_v4().to_string();
    client
        .append(proto::AppendRequest {
            stream_id: stream_id.clone(),
            expected_version: no_stream(),
            events: vec![make_proposed("MtlsEvent")],
        })
        .await
        .expect("append over mTLS should succeed");

    let resp = client
        .read_all(proto::ReadAllRequest {
            from_position: 0,
            max_count: 100,
        })
        .await
        .expect("read_all over mTLS should succeed");

    let events = resp.into_inner().events;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "MtlsEvent");
    assert_eq!(events[0].stream_id, stream_id);
}

// -- Test: plaintext server + plaintext client still works (regression guard) --

#[tokio::test]
async fn plaintext_server_plaintext_client_still_works() {
    let dir = tempfile::tempdir().expect("failed to create tempdir");
    let path = dir.path().join("events.log");
    let store = Store::open(&path).expect("open should succeed");
    let broker = Broker::new(1024);
    let (writer_handle, read_index, _join_handle) =
        spawn_writer(store, 64, broker.clone(), test_dedup_cap());
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
            .expect("plaintext server should run");
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let mut client = EventStoreClient::connect(format!("http://[::1]:{}", addr.port()))
        .await
        .expect("plaintext connection should succeed");

    let stream_id = uuid::Uuid::new_v4().to_string();
    client
        .append(proto::AppendRequest {
            stream_id: stream_id.clone(),
            expected_version: no_stream(),
            events: vec![make_proposed("PlaintextEvent")],
        })
        .await
        .expect("append over plaintext should succeed");

    let resp = client
        .read_all(proto::ReadAllRequest {
            from_position: 0,
            max_count: 100,
        })
        .await
        .expect("read_all over plaintext should succeed");

    let events = resp.into_inner().events;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "PlaintextEvent");
    assert_eq!(events[0].stream_id, stream_id);
}

// -- Test: TLS client with wrong CA is rejected by TLS server --

#[tokio::test]
async fn tls_wrong_ca_rejected() {
    let (addr, _correct_ca_pem, _dir) = start_tls_test_server().await;

    // Generate a completely separate CA that did NOT sign the server cert.
    let wrong_ca_key = rcgen::KeyPair::generate().expect("wrong CA key generation should succeed");
    let mut wrong_ca_params = rcgen::CertificateParams::new(vec!["Wrong CA".into()])
        .expect("wrong CA params should be valid");
    wrong_ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let wrong_ca_cert = wrong_ca_params
        .self_signed(&wrong_ca_key)
        .expect("wrong CA self-signing should succeed");
    let wrong_ca_pem = wrong_ca_cert.pem().into_bytes();

    // Connect with TLS using the wrong CA.
    let tls_config = ClientTlsConfig::new()
        .ca_certificate(Certificate::from_pem(&wrong_ca_pem))
        .domain_name("localhost");

    let result = Channel::from_shared(format!("https://[::1]:{}", addr.port()))
        .expect("valid URI")
        .tls_config(tls_config)
        .expect("TLS config should be valid")
        .connect()
        .await;

    match result {
        Ok(channel) => {
            // TCP connection may succeed, but the RPC should fail because the
            // server's certificate is not trusted by the wrong CA.
            let mut client = EventStoreClient::new(channel);
            let err = client
                .read_all(proto::ReadAllRequest {
                    from_position: 0,
                    max_count: 1,
                })
                .await
                .expect_err("RPC with wrong CA should fail");
            assert!(
                err.code() == tonic::Code::Unavailable
                    || err.code() == tonic::Code::Internal
                    || err.code() == tonic::Code::Unknown,
                "expected transport-level error code, got: {:?}",
                err.code()
            );
        }
        Err(_) => {
            // TLS handshake failed at connection time, which is also acceptable.
        }
    }
}
