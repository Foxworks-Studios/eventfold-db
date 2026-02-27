# Implementation Report: Ticket 1 -- Add metrics dependencies and `src/metrics.rs` module

**Ticket:** 1 - Add metrics dependencies and `src/metrics.rs` module
**Date:** 2026-02-27 14:00
**Status:** COMPLETE

---

## Files Changed

### Created
- `src/metrics.rs` - New module with `MetricsError`, `MetricsHandle`, `install_recorder()`, `serve_metrics()`, and unit tests.

### Modified
- `Cargo.toml` - Added `metrics = "0.24"`, `metrics-exporter-prometheus = { version = "0.16", default-features = false, features = ["http-listener"] }`, `axum = { version = "0.8", default-features = false, features = ["tokio", "http1"] }` to `[dependencies]`.
- `src/lib.rs` - Added `pub mod metrics;` with doc comment.

## Implementation Notes
- `install_recorder()` uses `std::sync::OnceLock` to guard the global recorder installation. The `OnceLock::get_or_init()` closure sets a `was_set` flag to distinguish first vs. subsequent calls. First call returns `Ok(MetricsHandle)`, subsequent calls return `Err(MetricsError::AlreadyInstalled)`.
- `MetricsHandle` wraps `Arc<PrometheusHandle>` rather than bare `PrometheusHandle` (as specified in the ticket). `PrometheusHandle` itself already wraps `Arc<Inner>` internally, so the extra `Arc` layer is minor overhead but satisfies the ticket's explicit requirement.
- `serve_metrics()` uses `tokio::spawn` (no new runtime). The axum handler captures the `MetricsHandle` via `move` closure and renders on each request. Content-Type is set to `text/plain; version=0.0.4` per Prometheus exposition format.
- On bind failure, the spawned task logs `tracing::error!` and returns immediately (the `JoinHandle` resolves). On success, it logs `tracing::info!` with the bound address and serves indefinitely.
- The `serve_metrics_stays_running` test is order-independent: it calls `install_recorder()` (ignoring errors if already installed) and then reads from the `OnceLock` to get a handle.

## Acceptance Criteria
- [x] AC 1: `Cargo.toml` contains exactly the three new entries with specified versions and features.
- [x] AC 2: `MetricsError` is a thiserror-derived enum with `AlreadyInstalled` variant, derives `Debug`.
- [x] AC 3: `MetricsHandle` wraps `Arc<PrometheusHandle>`, is `Clone + Send + Sync` (Clone derived, Send+Sync auto-derived via Arc).
- [x] AC 4: `install_recorder()` installs via `PrometheusBuilder::new().install_recorder()`, returns `MetricsError::AlreadyInstalled` on second call, guarded with `OnceLock`.
- [x] AC 5: `serve_metrics(handle, addr) -> JoinHandle<()>` spawns tokio task with axum Router, `GET /metrics` returns `handle.render()` with `Content-Type: text/plain; version=0.0.4`, binds via `TcpListener`, logs error on bind failure and returns immediately, logs info on success.
- [x] AC 6: `serve_metrics` calls `tokio::spawn` internally, does NOT construct a new `tokio::Runtime`.
- [x] AC 7: `src/lib.rs` has `pub mod metrics;` so `eventfold_db::metrics` is reachable.
- [x] AC 8: Test `install_recorder_twice_returns_already_installed` -- calls `install_recorder()` twice, first returns `Ok`, second returns `Err(MetricsError::AlreadyInstalled)`.
- [x] AC 9: Test `serve_metrics_stays_running` -- binds on `127.0.0.1:0`, uses `tokio::time::timeout(20ms)` and asserts timeout fires (server still alive).
- [x] AC 10: `cargo build` produces zero warnings.
- [x] AC 11: `cargo clippy --all-targets --all-features --locked -- -D warnings` passes.

## Test Results
- Lint: PASS (`cargo clippy --all-targets --all-features --locked -- -D warnings` -- zero warnings)
- Tests: PASS (all 182+ tests pass including 2 new tests in `src/metrics.rs`)
- Build: PASS (`cargo build` -- zero warnings)
- Format: PASS (`cargo fmt --check` -- no diff)
- New tests added:
  - `src/metrics.rs::tests::install_recorder_twice_returns_already_installed`
  - `src/metrics.rs::tests::serve_metrics_stays_running`

## Concerns / Blockers
- None
