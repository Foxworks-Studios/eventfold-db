use std::net::SocketAddr;
use std::path::PathBuf;

use eventfold_db::proto::event_store_server::EventStoreServer;
use eventfold_db::{Broker, EventfoldService, Store, spawn_writer};

/// Server configuration parsed from environment variables.
///
/// # Environment Variables
///
/// | Variable                   | Required | Default      | Description                        |
/// |----------------------------|----------|--------------|------------------------------------|
/// | `EVENTFOLD_DATA`           | Yes      | --           | Path to the append-only log file   |
/// | `EVENTFOLD_LISTEN`         | No       | `[::]:2113`  | Socket address to listen on        |
/// | `EVENTFOLD_BROKER_CAPACITY`| No       | `4096`       | Broadcast channel buffer size      |
#[derive(Debug, Clone, PartialEq)]
struct Config {
    /// Path to the append-only event log file.
    data_path: PathBuf,
    /// Socket address the gRPC server listens on.
    listen_addr: SocketAddr,
    /// Broadcast channel ring buffer capacity for live subscriptions.
    broker_capacity: usize,
}

/// Default socket address the server listens on when `EVENTFOLD_LISTEN` is not set.
const DEFAULT_LISTEN_ADDR: &str = "[::]:2113";

/// Default broadcast channel capacity when `EVENTFOLD_BROKER_CAPACITY` is not set.
const DEFAULT_BROKER_CAPACITY: usize = 4096;

impl Config {
    /// Parse server configuration from environment variables.
    ///
    /// # Environment Variables
    ///
    /// * `EVENTFOLD_DATA` (required) - Path to the append-only log file.
    /// * `EVENTFOLD_LISTEN` (optional) - Socket address to listen on. Defaults to `[::]:2113`.
    /// * `EVENTFOLD_BROKER_CAPACITY` (optional) - Broadcast channel buffer size. Defaults to
    ///   `4096`.
    ///
    /// # Errors
    ///
    /// Returns `Err(String)` if:
    /// - `EVENTFOLD_DATA` is not set
    /// - `EVENTFOLD_LISTEN` is set but not a valid `SocketAddr`
    /// - `EVENTFOLD_BROKER_CAPACITY` is set but not a valid `usize`
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

        Ok(Config {
            data_path,
            listen_addr,
            broker_capacity,
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
    let (writer_handle, read_index, join_handle) = spawn_writer(store, 64, broker.clone());

    // 7. Build the EventfoldService.
    let service = EventfoldService::new(writer_handle.clone(), read_index, broker);

    // 8. Build the tonic Server.
    let server = tonic::transport::Server::builder().add_service(EventStoreServer::new(service));

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

    // 11-12. Serve until shutdown signal, then clean up.
    server
        .serve_with_incoming_shutdown(incoming, shutdown_signal())
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

    #[test]
    #[serial]
    fn from_env_defaults_when_only_data_set() {
        // SAFETY: serial test -- no concurrent env mutation.
        unsafe { std::env::set_var("EVENTFOLD_DATA", "/tmp/x") };
        unsafe { std::env::remove_var("EVENTFOLD_LISTEN") };
        unsafe { std::env::remove_var("EVENTFOLD_BROKER_CAPACITY") };

        let config = Config::from_env().expect("should succeed with EVENTFOLD_DATA set");
        assert_eq!(config.data_path, PathBuf::from("/tmp/x"));
        assert_eq!(
            config.listen_addr,
            "[::]:2113".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(config.broker_capacity, 4096);
    }

    #[test]
    #[serial]
    fn from_env_missing_data_returns_err() {
        // SAFETY: serial test -- no concurrent env mutation.
        unsafe { std::env::remove_var("EVENTFOLD_DATA") };
        unsafe { std::env::remove_var("EVENTFOLD_LISTEN") };
        unsafe { std::env::remove_var("EVENTFOLD_BROKER_CAPACITY") };

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

        let result = Config::from_env();
        assert!(result.is_err(), "expected Err for invalid broker capacity");
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
