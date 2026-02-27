//! Prometheus metrics infrastructure for EventfoldDB.
//!
//! This module provides the foundational types for exposing Prometheus-format metrics
//! via an HTTP endpoint. It installs a global metrics recorder and serves the rendered
//! output on a configurable socket address.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::routing::get;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use tokio::task::JoinHandle;

/// Error type for metrics installation.
#[derive(Debug, thiserror::Error)]
pub enum MetricsError {
    /// The global metrics recorder has already been installed.
    #[error("metrics recorder already installed")]
    AlreadyInstalled,
}

/// Handle to the installed Prometheus metrics recorder.
///
/// This is a cheaply cloneable reference to the underlying recorder. It can be
/// used to render the current metrics snapshot in Prometheus exposition format.
#[derive(Clone, Debug)]
pub struct MetricsHandle {
    inner: Arc<PrometheusHandle>,
}

impl MetricsHandle {
    /// Render the current metrics snapshot in Prometheus exposition format.
    ///
    /// # Returns
    ///
    /// A string containing all registered metrics in the Prometheus text format.
    pub fn render(&self) -> String {
        self.inner.render()
    }
}

/// Guard to ensure the global recorder is installed at most once.
///
/// `OnceLock` is used instead of relying on the `metrics` crate's own
/// set_global_recorder error because `install_recorder()` can panic on
/// double-install in some versions. The `OnceLock` makes idempotency safe.
static RECORDER_HANDLE: std::sync::OnceLock<MetricsHandle> = std::sync::OnceLock::new();

/// Install the global Prometheus metrics recorder.
///
/// This function must be called once at startup before any `metrics` macros fire.
/// A second call in the same process returns [`MetricsError::AlreadyInstalled`].
///
/// # Returns
///
/// A [`MetricsHandle`] that can render the current metrics snapshot.
///
/// # Errors
///
/// Returns [`MetricsError::AlreadyInstalled`] if the recorder has already been installed.
pub fn install_recorder() -> Result<MetricsHandle, MetricsError> {
    // Try to initialize the OnceLock. If it's already set, another call got there first.
    let mut was_set = false;
    let handle = RECORDER_HANDLE.get_or_init(|| {
        was_set = true;
        let prom_handle = PrometheusBuilder::new()
            .install_recorder()
            .expect("PrometheusBuilder::install_recorder should succeed on first call");
        MetricsHandle {
            inner: Arc::new(prom_handle),
        }
    });

    if was_set {
        Ok(handle.clone())
    } else {
        Err(MetricsError::AlreadyInstalled)
    }
}

/// Returns the previously installed [`MetricsHandle`], if any.
///
/// This is useful in test code where `install_recorder()` may have already been
/// called by another test in the same process. Returns `None` if
/// `install_recorder()` has never been called.
///
/// # Returns
///
/// `Some(MetricsHandle)` if the recorder has been installed, `None` otherwise.
pub fn get_installed_handle() -> Option<MetricsHandle> {
    RECORDER_HANDLE.get().cloned()
}

/// Build the axum [`Router`] that serves Prometheus metrics at `GET /metrics`.
///
/// This is used by both [`serve_metrics`] and [`serve_metrics_on_listener`] to
/// ensure the route definition is shared and consistent.
fn metrics_router(handle: MetricsHandle) -> Router {
    Router::new().route(
        "/metrics",
        get(move || {
            let h = handle.clone();
            async move {
                let body = h.render();
                (
                    [(
                        axum::http::header::CONTENT_TYPE,
                        "text/plain; version=0.0.4",
                    )],
                    body,
                )
            }
        }),
    )
}

/// Spawn an axum HTTP server that serves Prometheus metrics at `GET /metrics`.
///
/// The server binds to the given address using `tokio::net::TcpListener` and runs
/// in a spawned tokio task (no new runtime is created). On bind failure, logs
/// `tracing::error!` and returns a `JoinHandle` that resolves immediately.
///
/// # Arguments
///
/// * `handle` - The metrics handle from [`install_recorder()`].
/// * `addr` - The socket address to bind on (use port 0 for ephemeral).
///
/// # Returns
///
/// A `JoinHandle<()>` for the spawned server task.
pub fn serve_metrics(handle: MetricsHandle, addr: SocketAddr) -> JoinHandle<()> {
    tokio::spawn(async move {
        let app = metrics_router(handle);

        let listener = match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => l,
            Err(e) => {
                tracing::error!(addr = %addr, error = %e, "Failed to bind metrics listener");
                return;
            }
        };

        let bound_addr = listener
            .local_addr()
            .expect("bound listener should have a local address");
        tracing::info!(addr = %bound_addr, "Metrics server listening");

        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!(error = %e, "Metrics server error");
        }
    })
}

/// Spawn an axum HTTP server on an already-bound [`tokio::net::TcpListener`].
///
/// This is useful in tests where the caller needs to know the ephemeral port
/// before the server starts. The route served is identical to [`serve_metrics`].
///
/// # Arguments
///
/// * `handle` - The metrics handle from [`install_recorder()`].
/// * `listener` - A pre-bound TCP listener to serve on.
///
/// # Returns
///
/// A `JoinHandle<()>` for the spawned server task.
pub fn serve_metrics_on_listener(
    handle: MetricsHandle,
    listener: tokio::net::TcpListener,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let app = metrics_router(handle);

        let bound_addr = listener
            .local_addr()
            .expect("bound listener should have a local address");
        tracing::info!(addr = %bound_addr, "Metrics server listening");

        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!(error = %e, "Metrics server error");
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn install_recorder_twice_returns_already_installed() {
        // First call should succeed.
        let result = install_recorder();
        assert!(result.is_ok(), "first install_recorder() should return Ok");

        // Second call should fail with AlreadyInstalled.
        let result = install_recorder();
        assert!(
            result.is_err(),
            "second install_recorder() should return Err"
        );
        let err = result.unwrap_err();
        assert!(
            matches!(err, MetricsError::AlreadyInstalled),
            "error should be AlreadyInstalled, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn serve_metrics_stays_running() {
        // Ensure the recorder is installed. If it was already installed by another
        // test in this process, install_recorder() returns Err -- that's fine, we
        // just grab the handle from the OnceLock.
        let _ = install_recorder();
        let handle = RECORDER_HANDLE
            .get()
            .expect("recorder should be installed after install_recorder() call")
            .clone();

        let join_handle = serve_metrics(handle, "127.0.0.1:0".parse().unwrap());

        // The server task should NOT complete within 20ms -- it should be running.
        let timeout_result = tokio::time::timeout(Duration::from_millis(20), join_handle).await;
        assert!(
            timeout_result.is_err(),
            "serve_metrics task should still be running after 20ms (timeout should fire)"
        );
    }

    #[tokio::test]
    async fn serve_metrics_on_listener_stays_running() {
        let _ = install_recorder();
        let handle = RECORDER_HANDLE
            .get()
            .expect("recorder should be installed after install_recorder() call")
            .clone();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind should succeed");
        let addr = listener.local_addr().expect("should have local addr");

        let join_handle = serve_metrics_on_listener(handle, listener);

        // The server task should NOT complete within 20ms -- it should be running.
        let timeout_result = tokio::time::timeout(Duration::from_millis(20), join_handle).await;
        assert!(
            timeout_result.is_err(),
            "serve_metrics_on_listener task should still be running after 20ms"
        );

        // Verify we can identify the address it's serving on.
        assert_ne!(addr.port(), 0, "ephemeral port should be nonzero");
    }
}
