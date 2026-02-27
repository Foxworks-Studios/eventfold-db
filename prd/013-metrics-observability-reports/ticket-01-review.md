# Code Review: Ticket 1 -- Add metrics dependencies and `src/metrics.rs` module

**Ticket:** 1 -- Add metrics dependencies and `src/metrics.rs` module
**Impl Report:** prd/013-metrics-observability-reports/ticket-01-impl.md
**Date:** 2026-02-27 15:30
**Verdict:** APPROVED

---

## AC Coverage

| AC # | Description | Status | Notes |
|------|-------------|--------|-------|
| 1 | `Cargo.toml` contains the three new entries with specified versions and features | Met | Verified in `Cargo.toml` lines 13, 17, 18: `axum = { version = "0.8", default-features = false, features = ["tokio", "http1"] }`, `metrics = "0.24"`, `metrics-exporter-prometheus = { version = "0.16", default-features = false, features = ["http-listener"] }`. Exact match to spec. |
| 2 | `MetricsError` is a thiserror-derived enum with `AlreadyInstalled` variant, derives `Debug` | Met | `src/metrics.rs` lines 16-21: `#[derive(Debug, thiserror::Error)]` with `AlreadyInstalled` variant. |
| 3 | `MetricsHandle` wraps `Arc<PrometheusHandle>`, is `Clone + Send + Sync` | Met | `src/metrics.rs` lines 27-30: `#[derive(Clone, Debug)]` struct with `inner: Arc<PrometheusHandle>`. `Send + Sync` auto-derived via `Arc<T>` where `PrometheusHandle: Send + Sync`. |
| 4 | `install_recorder()` uses `OnceLock` guard, returns `AlreadyInstalled` on second call | Met | `src/metrics.rs` lines 48, 62-80: `static RECORDER_HANDLE: OnceLock<MetricsHandle>` with `get_or_init` closure setting `was_set` flag. Second call returns `Err(MetricsError::AlreadyInstalled)`. |
| 5 | `serve_metrics()` spawns tokio task with axum Router, `GET /metrics`, correct content type | Met | `src/metrics.rs` lines 96-132: `tokio::spawn` with `Router::new().route("/metrics", get(...))`, content type `"text/plain; version=0.0.4"`, `TcpListener::bind`, `tracing::error!` on bind failure, `tracing::info!` on success. |
| 6 | No new `tokio::Runtime` in `serve_metrics` | Met | Structurally verified: no `Runtime::new()`, `Builder::new_*` calls anywhere in `src/metrics.rs`. Uses `tokio::spawn` only. |
| 7 | `src/lib.rs` has `pub mod metrics;` | Met | `src/lib.rs` lines 7-8: `/// Prometheus metrics infrastructure for EventfoldDB.` / `pub mod metrics;`. |
| 8 | Test: double-install returns `AlreadyInstalled` | Met | `src/metrics.rs` lines 140-156: `install_recorder_twice_returns_already_installed` calls `install_recorder()` twice, asserts first is `Ok`, second is `Err(MetricsError::AlreadyInstalled)`. |
| 9 | Test: server stays running (timeout-based) | Met | `src/metrics.rs` lines 158-177: `serve_metrics_stays_running` binds on `127.0.0.1:0`, uses `tokio::time::timeout(Duration::from_millis(20), join_handle)` and asserts timeout fires. |
| 10 | `cargo build` zero warnings | Met | Confirmed: `cargo build` completes with zero warnings. |
| 11 | `cargo clippy` passes | Met | Confirmed: `cargo clippy --all-targets --all-features --locked -- -D warnings` passes clean. |

## Issues Found

### Critical (must fix before merge)

None.

### Major (should fix, risk of downstream problems)

- **Test ordering fragility in `install_recorder_twice_returns_already_installed`** (`src/metrics.rs` lines 140-156): This test asserts the first call to `install_recorder()` returns `Ok`. However, `install_recorder()` mutates process-global state via `OnceLock`. If `serve_metrics_stays_running` (line 163: `let _ = install_recorder();`) runs first in parallel, the `OnceLock` is already set, and the first assertion fails. The `serial_test` crate is already a dev-dependency. Both tests should be annotated with `#[serial]` (or a shared serial group) to prevent non-deterministic failures. This will become increasingly fragile as Tickets 2-5 add more tests that call `install_recorder()`. Currently passes by luck of execution order; not guaranteed across platforms, Rust versions, or `--test-threads` settings.

### Minor (nice to fix, not blocking)

- **`MetricsError` is separate from `eventfold_db::Error`**: CLAUDE.md convention says "All fallible operations return `Result<T, eventfold_db::Error>`." `install_recorder()` returns `Result<MetricsHandle, MetricsError>` instead. This is a reasonable design choice since `AlreadyInstalled` is infrastructure-specific and does not map to any existing `Error` variant, but it is technically a convention deviation. The ticket explicitly specifies `MetricsError` as its own type, so this is by design.

## Suggestions (non-blocking)

- Consider adding `#[serial]` from `serial_test` to both metrics tests now, before Tickets 2-5 add additional tests that interact with the global recorder. This is a small change that prevents a class of flaky test failures.

## Scope Check

- Files within scope: YES -- only `Cargo.toml`, `src/lib.rs`, and `src/metrics.rs` were touched, exactly matching the ticket scope.
- Scope creep detected: NO
- Unauthorized dependencies added: NO -- exactly the three specified dependencies were added.

## Risk Assessment

- Regression risk: LOW -- No existing code was modified in a behavioral way. The `lib.rs` change adds a new module declaration only. `Cargo.toml` adds dependencies that do not conflict with existing ones. All 245 tests pass.
- Security concerns: NONE -- The metrics endpoint serves read-only Prometheus text format. No authentication is specified in the ticket (metrics endpoints are typically internal-only). No secrets or PII exposed.
- Performance concerns: NONE -- `Arc<PrometheusHandle>` is a cheap clone. The `OnceLock` guard has negligible overhead. The axum server runs in the existing tokio runtime via `tokio::spawn`.

## Verification

All quality gates confirmed passing:
- `cargo build`: zero warnings
- `cargo clippy --all-targets --all-features --locked -- -D warnings`: zero diagnostics
- `cargo test`: 245 tests, all green (including 2 new metrics tests)
- `cargo fmt --check`: no diff
