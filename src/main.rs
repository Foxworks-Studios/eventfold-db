use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::path::PathBuf;

use eventfold_db::proto::event_store_server::EventStoreServer;
use eventfold_db::{Broker, EventfoldService, Store, spawn_writer};

/// Optional TLS configuration parsed from environment variables.
///
/// Present only when `EVENTFOLD_TLS_CERT` and `EVENTFOLD_TLS_KEY` are both set.
/// When `EVENTFOLD_TLS_CA` is also set, the server requires client certificates (mTLS).
#[derive(Debug, Clone, PartialEq)]
struct TlsConfig {
    /// Path to the PEM-encoded server certificate file.
    cert_path: PathBuf,
    /// Path to the PEM-encoded server private key file.
    key_path: PathBuf,
    /// Path to the PEM-encoded CA certificate for client verification (mTLS).
    /// When `Some`, the server requires and verifies client certificates.
    ca_path: Option<PathBuf>,
}

/// Server configuration parsed from environment variables.
///
/// # Environment Variables
///
/// | Variable                    | Required | Default      | Description                          |
/// |-----------------------------|----------|--------------|--------------------------------------|
/// | `EVENTFOLD_DATA`            | Yes      | --           | Path to the append-only log file     |
/// | `EVENTFOLD_LISTEN`          | No       | `[::]:2113`  | Socket address to listen on          |
/// | `EVENTFOLD_BROKER_CAPACITY` | No       | `4096`       | Broadcast channel buffer size        |
/// | `EVENTFOLD_DEDUP_CAPACITY`  | No       | `65536`      | Max event IDs in dedup index         |
/// | `EVENTFOLD_TLS_CERT`        | No       | --           | PEM cert path (enables TLS)          |
/// | `EVENTFOLD_TLS_KEY`         | No       | --           | PEM key path (required with CERT)    |
/// | `EVENTFOLD_TLS_CA`          | No       | --           | PEM CA path (enables mTLS)           |
#[derive(Debug, Clone, PartialEq)]
struct Config {
    /// Path to the append-only event log file.
    data_path: PathBuf,
    /// Socket address the gRPC server listens on.
    listen_addr: SocketAddr,
    /// Broadcast channel ring buffer capacity for live subscriptions.
    broker_capacity: usize,
    /// Maximum number of event IDs tracked in the dedup index.
    dedup_capacity: NonZeroUsize,
    /// Optional TLS configuration. `None` means plaintext mode.
    tls: Option<TlsConfig>,
}

/// Default socket address the server listens on when `EVENTFOLD_LISTEN` is not set.
const DEFAULT_LISTEN_ADDR: &str = "[::]:2113";

/// Default broadcast channel capacity when `EVENTFOLD_BROKER_CAPACITY` is not set.
const DEFAULT_BROKER_CAPACITY: usize = 4096;

/// Default dedup capacity when `EVENTFOLD_DEDUP_CAPACITY` is not set.
const DEFAULT_DEDUP_CAPACITY: usize = 65536;

impl Config {
    /// Parse server configuration from environment variables.
    ///
    /// # Environment Variables
    ///
    /// * `EVENTFOLD_DATA` (required) - Path to the append-only log file.
    /// * `EVENTFOLD_LISTEN` (optional) - Socket address to listen on. Defaults to `[::]:2113`.
    /// * `EVENTFOLD_BROKER_CAPACITY` (optional) - Broadcast channel buffer size. Defaults to
    ///   `4096`.
    /// * `EVENTFOLD_DEDUP_CAPACITY` (optional) - Max event IDs in dedup index. Defaults to
    ///   `65536`.
    ///
    /// # Errors
    ///
    /// Returns `Err(String)` if:
    /// - `EVENTFOLD_DATA` is not set
    /// - `EVENTFOLD_LISTEN` is set but not a valid `SocketAddr`
    /// - `EVENTFOLD_BROKER_CAPACITY` is set but not a valid `usize`
    /// - `EVENTFOLD_DEDUP_CAPACITY` is set but not a valid nonzero `usize`
    /// - `EVENTFOLD_TLS_CERT` is set without `EVENTFOLD_TLS_KEY` (or vice versa)
    /// - `EVENTFOLD_TLS_CA` is set without both `EVENTFOLD_TLS_CERT` and `EVENTFOLD_TLS_KEY`
    fn from_env() -> Result<Config, String> {
        let data_path = std::env::var("EVENTFOLD_DATA")
            .map(PathBuf::from)
            .map_err(|_| "EVENTFOLD_DATA environment variable is required".to_string())?;

        let listen_addr = match std::env::var("EVENTFOLD_LISTEN") {
            Ok(val) => val
                .parse::<SocketAddr>()
                .map_err(|e| format!("EVENTFOLD_LISTEN is not a valid socket address: {e}"))?,
            Err(_) => DEFAULT_LISTEN_ADDR
                .parse::<SocketAddr>()
                .expect("default listen address is valid"),
        };

        let broker_capacity = match std::env::var("EVENTFOLD_BROKER_CAPACITY") {
            Ok(val) => val
                .parse::<usize>()
                .map_err(|e| format!("EVENTFOLD_BROKER_CAPACITY is not a valid usize: {e}"))?,
            Err(_) => DEFAULT_BROKER_CAPACITY,
        };

        let dedup_capacity = match std::env::var("EVENTFOLD_DEDUP_CAPACITY") {
            Ok(val) => {
                let raw: usize = val
                    .parse()
                    .map_err(|e| format!("EVENTFOLD_DEDUP_CAPACITY is not a valid usize: {e}"))?;
                NonZeroUsize::new(raw)
                    .ok_or_else(|| "EVENTFOLD_DEDUP_CAPACITY must be nonzero".to_string())?
            }
            Err(_) => NonZeroUsize::new(DEFAULT_DEDUP_CAPACITY)
                .expect("default dedup capacity is nonzero"),
        };

        // Parse optional TLS configuration from environment variables.
        // Both CERT and KEY must be set together; CA is optional (enables mTLS).
        let tls_cert = std::env::var("EVENTFOLD_TLS_CERT").ok();
        let tls_key = std::env::var("EVENTFOLD_TLS_KEY").ok();
        let tls_ca = std::env::var("EVENTFOLD_TLS_CA").ok();

        let tls = match (tls_cert, tls_key, tls_ca) {
            // All three absent: plaintext mode.
            (None, None, None) => None,
            // Both cert and key present, CA absent: TLS without mTLS.
            (Some(cert), Some(key), None) => Some(TlsConfig {
                cert_path: PathBuf::from(cert),
                key_path: PathBuf::from(key),
                ca_path: None,
            }),
            // All three present: TLS with mTLS.
            (Some(cert), Some(key), Some(ca)) => Some(TlsConfig {
                cert_path: PathBuf::from(cert),
                key_path: PathBuf::from(key),
                ca_path: Some(PathBuf::from(ca)),
            }),
            // Cert set, key missing.
            (Some(_), None, _) => {
                return Err(
                    "EVENTFOLD_TLS_CERT is set but EVENTFOLD_TLS_KEY is missing".to_string()
                );
            }
            // Key set, cert missing.
            (None, Some(_), _) => {
                return Err(
                    "EVENTFOLD_TLS_KEY is set but EVENTFOLD_TLS_CERT is missing".to_string()
                );
            }
            // CA set without cert and key.
            (None, None, Some(_)) => {
                return Err("EVENTFOLD_TLS_CA is set but EVENTFOLD_TLS_CERT and \
                     EVENTFOLD_TLS_KEY are both missing"
                    .to_string());
            }
        };

        Ok(Config {
            data_path,
            listen_addr,
            broker_capacity,
            dedup_capacity,
            tls,
        })
    }
}

/// Initialize the global `tracing` subscriber with an `EnvFilter`.
///
/// Reads the `RUST_LOG` environment variable to configure log level filtering. If `RUST_LOG`
/// is not set, defaults to `"info"`. Uses `try_init()` so that repeated calls (e.g., across
/// tests in the same process) do not panic -- the second call is a silent no-op.
fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    // `try_init` returns Err if a global subscriber is already set. We ignore that
    // error because test processes may call init_tracing() from multiple tests.
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

/// Waits for a shutdown signal: SIGINT on all platforms, plus SIGTERM on Unix.
///
/// Returns once the first signal is received.
async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to register SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {}
            _ = sigterm.recv() => {}
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.expect("failed to listen for Ctrl+C");
    }
}

#[tokio::main]
async fn main() {
    // 1. Initialize tracing.
    init_tracing();

    // 2. Read configuration from environment variables.
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(1);
        }
    };

    // 3. Log configuration values.
    tracing::info!(data_path = %config.data_path.display(), "Data path");
    tracing::info!(listen_addr = %config.listen_addr, "Listen address");
    tracing::info!(broker_capacity = config.broker_capacity, "Broker capacity");

    // 4. Open the Store. Log recovered event and stream counts.
    let store = match Store::open(&config.data_path) {
        Ok(store) => store,
        Err(e) => {
            tracing::error!(error = %e, "Failed to open store");
            std::process::exit(1);
        }
    };

    // Read event and stream counts from the recovered log before spawning the writer.
    {
        let log_arc = store.log();
        let log = log_arc
            .read()
            .expect("EventLog RwLock poisoned during startup");
        tracing::info!(events = log.events.len(), "Recovered events");
        tracing::info!(streams = log.streams.len(), "Recovered streams");
    }

    // 5. Create the Broker.
    let broker = Broker::new(config.broker_capacity);

    // 6. Spawn the writer task.
    tracing::info!(dedup_capacity = %config.dedup_capacity, "Dedup capacity");
    let (writer_handle, read_index, join_handle) =
        spawn_writer(store, 64, broker.clone(), config.dedup_capacity);

    // 7. Build the EventfoldService and health reporter.
    let service = EventfoldService::new(writer_handle.clone(), read_index, broker);
    let (health_reporter, health_service) = tonic_health::server::health_reporter();

    // 8. Build the tonic Server, optionally with TLS.
    let mut builder = tonic::transport::Server::builder();

    if let Some(ref tls) = config.tls {
        let cert = tokio::fs::read(&tls.cert_path).await.unwrap_or_else(|e| {
            tracing::error!(
                path = %tls.cert_path.display(),
                error = %e,
                "Failed to read TLS certificate"
            );
            std::process::exit(1);
        });

        let key = tokio::fs::read(&tls.key_path).await.unwrap_or_else(|e| {
            tracing::error!(
                path = %tls.key_path.display(),
                error = %e,
                "Failed to read TLS private key"
            );
            std::process::exit(1);
        });

        let identity = tonic::transport::Identity::from_pem(cert, key);
        let mut tls_config = tonic::transport::ServerTlsConfig::new().identity(identity);

        if let Some(ref ca_path) = tls.ca_path {
            let ca = tokio::fs::read(ca_path).await.unwrap_or_else(|e| {
                tracing::error!(
                    path = %ca_path.display(),
                    error = %e,
                    "Failed to read TLS CA certificate"
                );
                std::process::exit(1);
            });
            let ca_cert = tonic::transport::Certificate::from_pem(ca);
            tls_config = tls_config.client_ca_root(ca_cert);
            tracing::info!("mTLS enabled");
        } else {
            tracing::info!("TLS enabled");
        }

        builder = builder.tls_config(tls_config).unwrap_or_else(|e| {
            tracing::error!(error = %e, "Failed to configure TLS");
            std::process::exit(1);
        });
    }

    let server = builder
        .add_service(health_service)
        .add_service(EventStoreServer::new(service));

    // 9. Bind on the configured address.
    let listener = tokio::net::TcpListener::bind(config.listen_addr)
        .await
        .unwrap_or_else(|e| {
            tracing::error!(
                addr = %config.listen_addr,
                error = %e,
                "Failed to bind listener"
            );
            std::process::exit(1);
        });

    let addr = listener
        .local_addr()
        .expect("bound listener should have a local address");

    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    // 10. Log the actual bound address.
    tracing::info!("Server listening on {addr}");

    // 11. Mark health service as SERVING now that the listener is bound.
    health_reporter
        .set_serving::<EventStoreServer<EventfoldService>>()
        .await;
    health_reporter
        .set_service_status("", tonic_health::ServingStatus::Serving)
        .await;

    // 12-13. Serve until shutdown signal, then clean up.
    // The shutdown future transitions health status to NOT_SERVING before the
    // server begins draining connections.
    server
        .serve_with_incoming_shutdown(incoming, async {
            shutdown_signal().await;
            health_reporter
                .set_service_status("", tonic_health::ServingStatus::NotServing)
                .await;
            health_reporter
                .set_not_serving::<EventStoreServer<EventfoldService>>()
                .await;
        })
        .await
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "Server error");
            std::process::exit(1);
        });

    // Shutdown sequence: log, drop writer handle, await writer task.
    tracing::info!("Shutting down");
    drop(writer_handle);
    join_handle
        .await
        .expect("writer task should exit without panicking");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// Clear all TLS-related environment variables so they do not leak between tests.
    fn clear_tls_env() {
        // SAFETY: serial test -- no concurrent env mutation.
        unsafe { std::env::remove_var("EVENTFOLD_TLS_CERT") };
        unsafe { std::env::remove_var("EVENTFOLD_TLS_KEY") };
        unsafe { std::env::remove_var("EVENTFOLD_TLS_CA") };
    }

    #[test]
    #[serial]
    fn from_env_defaults_when_only_data_set() {
        // SAFETY: serial test -- no concurrent env mutation.
        unsafe { std::env::set_var("EVENTFOLD_DATA", "/tmp/x") };
        unsafe { std::env::remove_var("EVENTFOLD_LISTEN") };
        unsafe { std::env::remove_var("EVENTFOLD_BROKER_CAPACITY") };
        unsafe { std::env::remove_var("EVENTFOLD_DEDUP_CAPACITY") };
        clear_tls_env();

        let config = Config::from_env().expect("should succeed with EVENTFOLD_DATA set");
        assert_eq!(config.data_path, PathBuf::from("/tmp/x"));
        assert_eq!(
            config.listen_addr,
            "[::]:2113".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(config.broker_capacity, 4096);
        assert_eq!(config.dedup_capacity.get(), 65536);
    }

    #[test]
    #[serial]
    fn from_env_missing_data_returns_err() {
        // SAFETY: serial test -- no concurrent env mutation.
        unsafe { std::env::remove_var("EVENTFOLD_DATA") };
        unsafe { std::env::remove_var("EVENTFOLD_LISTEN") };
        unsafe { std::env::remove_var("EVENTFOLD_BROKER_CAPACITY") };
        unsafe { std::env::remove_var("EVENTFOLD_DEDUP_CAPACITY") };
        clear_tls_env();

        let result = Config::from_env();
        assert!(result.is_err(), "expected Err when EVENTFOLD_DATA is unset");
        let msg = result.unwrap_err();
        assert!(
            msg.contains("EVENTFOLD_DATA"),
            "error message should mention EVENTFOLD_DATA, got: {msg}"
        );
    }

    #[test]
    #[serial]
    fn from_env_custom_listen_addr() {
        // SAFETY: serial test -- no concurrent env mutation.
        unsafe { std::env::set_var("EVENTFOLD_DATA", "/tmp/x") };
        unsafe { std::env::set_var("EVENTFOLD_LISTEN", "127.0.0.1:9999") };
        unsafe { std::env::remove_var("EVENTFOLD_BROKER_CAPACITY") };
        unsafe { std::env::remove_var("EVENTFOLD_DEDUP_CAPACITY") };
        clear_tls_env();

        let config = Config::from_env().expect("should succeed");
        assert_eq!(
            config.listen_addr,
            "127.0.0.1:9999".parse::<SocketAddr>().unwrap()
        );
    }

    #[test]
    #[serial]
    fn from_env_custom_broker_capacity() {
        // SAFETY: serial test -- no concurrent env mutation.
        unsafe { std::env::set_var("EVENTFOLD_DATA", "/tmp/x") };
        unsafe { std::env::remove_var("EVENTFOLD_LISTEN") };
        unsafe { std::env::set_var("EVENTFOLD_BROKER_CAPACITY", "16") };
        unsafe { std::env::remove_var("EVENTFOLD_DEDUP_CAPACITY") };
        clear_tls_env();

        let config = Config::from_env().expect("should succeed");
        assert_eq!(config.broker_capacity, 16);
    }

    #[test]
    #[serial]
    fn from_env_invalid_listen_addr_returns_err() {
        // SAFETY: serial test -- no concurrent env mutation.
        unsafe { std::env::set_var("EVENTFOLD_DATA", "/tmp/x") };
        unsafe { std::env::set_var("EVENTFOLD_LISTEN", "not-an-addr") };
        unsafe { std::env::remove_var("EVENTFOLD_BROKER_CAPACITY") };
        unsafe { std::env::remove_var("EVENTFOLD_DEDUP_CAPACITY") };
        clear_tls_env();

        let result = Config::from_env();
        assert!(result.is_err(), "expected Err for invalid listen address");
    }

    #[test]
    fn init_tracing_does_not_panic() {
        // init_tracing() should be safe to call. The global subscriber may already
        // be set by another test, so we accept try_init failure silently.
        init_tracing();
    }

    #[test]
    #[serial]
    fn from_env_invalid_broker_capacity_returns_err() {
        // SAFETY: serial test -- no concurrent env mutation.
        unsafe { std::env::set_var("EVENTFOLD_DATA", "/tmp/x") };
        unsafe { std::env::remove_var("EVENTFOLD_LISTEN") };
        unsafe { std::env::set_var("EVENTFOLD_BROKER_CAPACITY", "not-a-number") };
        unsafe { std::env::remove_var("EVENTFOLD_DEDUP_CAPACITY") };
        clear_tls_env();

        let result = Config::from_env();
        assert!(result.is_err(), "expected Err for invalid broker capacity");
    }

    #[test]
    #[serial]
    fn from_env_tls_none_when_no_tls_vars() {
        // SAFETY: serial test -- no concurrent env mutation.
        unsafe { std::env::set_var("EVENTFOLD_DATA", "/tmp/x") };
        unsafe { std::env::remove_var("EVENTFOLD_LISTEN") };
        unsafe { std::env::remove_var("EVENTFOLD_BROKER_CAPACITY") };
        unsafe { std::env::remove_var("EVENTFOLD_DEDUP_CAPACITY") };
        clear_tls_env();

        let config = Config::from_env().expect("should succeed without TLS vars");
        assert_eq!(config.tls, None);
    }

    #[test]
    #[serial]
    fn from_env_tls_ca_only_returns_err() {
        // SAFETY: serial test -- no concurrent env mutation.
        unsafe { std::env::set_var("EVENTFOLD_DATA", "/tmp/x") };
        unsafe { std::env::remove_var("EVENTFOLD_LISTEN") };
        unsafe { std::env::remove_var("EVENTFOLD_BROKER_CAPACITY") };
        unsafe { std::env::remove_var("EVENTFOLD_DEDUP_CAPACITY") };
        unsafe { std::env::remove_var("EVENTFOLD_TLS_CERT") };
        unsafe { std::env::remove_var("EVENTFOLD_TLS_KEY") };
        unsafe { std::env::set_var("EVENTFOLD_TLS_CA", "/tmp/ca.crt") };

        let result = Config::from_env();
        assert!(
            result.is_err(),
            "expected Err when CA is set without cert+key"
        );
        let msg = result.unwrap_err();
        assert!(
            msg.contains("EVENTFOLD_TLS_CERT"),
            "error should mention EVENTFOLD_TLS_CERT, got: {msg}"
        );
        assert!(
            msg.contains("EVENTFOLD_TLS_KEY"),
            "error should mention EVENTFOLD_TLS_KEY, got: {msg}"
        );
    }

    #[test]
    #[serial]
    fn from_env_tls_key_without_cert_returns_err() {
        // SAFETY: serial test -- no concurrent env mutation.
        unsafe { std::env::set_var("EVENTFOLD_DATA", "/tmp/x") };
        unsafe { std::env::remove_var("EVENTFOLD_LISTEN") };
        unsafe { std::env::remove_var("EVENTFOLD_BROKER_CAPACITY") };
        unsafe { std::env::remove_var("EVENTFOLD_DEDUP_CAPACITY") };
        unsafe { std::env::remove_var("EVENTFOLD_TLS_CERT") };
        unsafe { std::env::set_var("EVENTFOLD_TLS_KEY", "/tmp/k.key") };
        unsafe { std::env::remove_var("EVENTFOLD_TLS_CA") };

        let result = Config::from_env();
        assert!(result.is_err(), "expected Err when CERT is missing");
        let msg = result.unwrap_err();
        assert!(
            msg.contains("EVENTFOLD_TLS_CERT"),
            "error should mention EVENTFOLD_TLS_CERT, got: {msg}"
        );
    }

    #[test]
    #[serial]
    fn from_env_tls_cert_without_key_returns_err() {
        // SAFETY: serial test -- no concurrent env mutation.
        unsafe { std::env::set_var("EVENTFOLD_DATA", "/tmp/x") };
        unsafe { std::env::remove_var("EVENTFOLD_LISTEN") };
        unsafe { std::env::remove_var("EVENTFOLD_BROKER_CAPACITY") };
        unsafe { std::env::remove_var("EVENTFOLD_DEDUP_CAPACITY") };
        unsafe { std::env::set_var("EVENTFOLD_TLS_CERT", "/tmp/c.crt") };
        unsafe { std::env::remove_var("EVENTFOLD_TLS_KEY") };
        unsafe { std::env::remove_var("EVENTFOLD_TLS_CA") };

        let result = Config::from_env();
        assert!(result.is_err(), "expected Err when KEY is missing");
        let msg = result.unwrap_err();
        assert!(
            msg.contains("EVENTFOLD_TLS_KEY"),
            "error should mention EVENTFOLD_TLS_KEY, got: {msg}"
        );
    }

    #[test]
    #[serial]
    fn from_env_tls_cert_key_and_ca() {
        // SAFETY: serial test -- no concurrent env mutation.
        unsafe { std::env::set_var("EVENTFOLD_DATA", "/tmp/x") };
        unsafe { std::env::remove_var("EVENTFOLD_LISTEN") };
        unsafe { std::env::remove_var("EVENTFOLD_BROKER_CAPACITY") };
        unsafe { std::env::remove_var("EVENTFOLD_DEDUP_CAPACITY") };
        unsafe { std::env::set_var("EVENTFOLD_TLS_CERT", "/tmp/c.crt") };
        unsafe { std::env::set_var("EVENTFOLD_TLS_KEY", "/tmp/k.key") };
        unsafe { std::env::set_var("EVENTFOLD_TLS_CA", "/tmp/ca.crt") };

        let config = Config::from_env().expect("should succeed with cert, key, and CA");
        assert_eq!(
            config.tls,
            Some(TlsConfig {
                cert_path: PathBuf::from("/tmp/c.crt"),
                key_path: PathBuf::from("/tmp/k.key"),
                ca_path: Some(PathBuf::from("/tmp/ca.crt")),
            })
        );
    }

    #[test]
    #[serial]
    fn from_env_tls_cert_and_key_without_ca() {
        // SAFETY: serial test -- no concurrent env mutation.
        unsafe { std::env::set_var("EVENTFOLD_DATA", "/tmp/x") };
        unsafe { std::env::remove_var("EVENTFOLD_LISTEN") };
        unsafe { std::env::remove_var("EVENTFOLD_BROKER_CAPACITY") };
        unsafe { std::env::remove_var("EVENTFOLD_DEDUP_CAPACITY") };
        unsafe { std::env::set_var("EVENTFOLD_TLS_CERT", "/tmp/c.crt") };
        unsafe { std::env::set_var("EVENTFOLD_TLS_KEY", "/tmp/k.key") };
        unsafe { std::env::remove_var("EVENTFOLD_TLS_CA") };

        let config = Config::from_env().expect("should succeed with cert and key");
        assert_eq!(
            config.tls,
            Some(TlsConfig {
                cert_path: PathBuf::from("/tmp/c.crt"),
                key_path: PathBuf::from("/tmp/k.key"),
                ca_path: None,
            })
        );
    }

    #[test]
    fn binary_exits_nonzero_when_tls_cert_file_missing() {
        // When EVENTFOLD_TLS_CERT and EVENTFOLD_TLS_KEY point to nonexistent files,
        // the binary should log an error (via tracing::error!) and exit non-zero.
        // We only assert on exit status because tracing output may not be fully
        // flushed before process::exit(1) terminates the process.
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let data_path = dir.path().join("events.log");

        let output = std::process::Command::new("cargo")
            .args(["run", "--bin", "eventfold-db", "--quiet"])
            .env("EVENTFOLD_DATA", data_path.as_os_str())
            .env("EVENTFOLD_LISTEN", "[::1]:0")
            .env("EVENTFOLD_TLS_CERT", "/tmp/nonexistent-cert.pem")
            .env("EVENTFOLD_TLS_KEY", "/tmp/nonexistent-key.pem")
            .env_remove("EVENTFOLD_TLS_CA")
            .env_remove("EVENTFOLD_BROKER_CAPACITY")
            .output()
            .expect("failed to execute cargo run");

        assert!(
            !output.status.success(),
            "expected non-zero exit when TLS cert file does not exist"
        );
    }

    #[tokio::test]
    async fn health_reporter_can_set_serving_and_not_serving() {
        // Verify that tonic-health dependency is available and the API works
        // for both the empty-string service name and the typed service name.
        use eventfold_db::proto::event_store_server::EventStoreServer;

        let (reporter, _service) = tonic_health::server::health_reporter();

        // Set serving for both service names (same pattern as main.rs wiring).
        reporter
            .set_serving::<EventStoreServer<EventfoldService>>()
            .await;
        reporter
            .set_service_status("", tonic_health::ServingStatus::Serving)
            .await;

        // Transition to NOT_SERVING (same pattern as shutdown path).
        reporter
            .set_service_status("", tonic_health::ServingStatus::NotServing)
            .await;
        reporter
            .set_not_serving::<EventStoreServer<EventfoldService>>()
            .await;
    }

    #[test]
    fn binary_exits_nonzero_without_eventfold_data() {
        // Run the binary via `cargo run` without EVENTFOLD_DATA. The binary should
        // print an error to stderr mentioning EVENTFOLD_DATA and exit non-zero.
        // We clear inherited env vars so the test is deterministic.
        let output = std::process::Command::new("cargo")
            .args(["run", "--bin", "eventfold-db", "--quiet"])
            .env_remove("EVENTFOLD_DATA")
            .env_remove("EVENTFOLD_LISTEN")
            .env_remove("EVENTFOLD_BROKER_CAPACITY")
            .env_remove("EVENTFOLD_TLS_CERT")
            .env_remove("EVENTFOLD_TLS_KEY")
            .env_remove("EVENTFOLD_TLS_CA")
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
}
