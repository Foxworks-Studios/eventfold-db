# Implementation Report: Ticket 2 -- Implement `main.rs` Startup Sequence and Graceful Shutdown

**Ticket:** 2 - Implement `main.rs` Startup Sequence and Graceful Shutdown
**Date:** 2026-02-27 01:25
**Status:** COMPLETE

---

## Files Changed

### Modified
- `src/main.rs` - Replaced placeholder `fn main()` with full `#[tokio::main] async fn main()` implementing the 12-step startup sequence and graceful shutdown. Removed all `#[allow(dead_code)]` annotations from `Config`, `DEFAULT_LISTEN_ADDR`, `DEFAULT_BROKER_CAPACITY`, `Config::from_env()`, and `init_tracing()`. Added `shutdown_signal()` async helper and `binary_exits_nonzero_without_eventfold_data` test.
- `Cargo.toml` - Moved `tokio-stream = "0.1"` from `[dev-dependencies]` to `[dependencies]` (required for `TcpListenerStream` in the binary entrypoint).

## Implementation Notes
- The startup sequence follows the PRD's 12 steps exactly: init tracing, parse config, log config values, open Store, log recovered counts, create Broker, spawn writer, build EventfoldService, build tonic Server, bind TcpListener, log bound address, serve with shutdown signal.
- For recovered event and stream counts, the implementation reads directly from `Store::log()` (the `Arc<RwLock<EventLog>>`) before spawning the writer, since `ReadIndex` does not have `event_count()` or `stream_count()` methods. This is equivalent and stays within the `src/main.rs` scope.
- The `shutdown_signal()` helper uses `tokio::select!` on Unix to await either SIGINT or SIGTERM. On non-Unix platforms, it only awaits SIGINT via `tokio::signal::ctrl_c()`. The SIGTERM branch is conditionally compiled with `#[cfg(unix)]`.
- Uses `serve_with_incoming_shutdown` (not `serve_with_incoming`) to integrate the shutdown signal. When the signal fires, tonic stops accepting new connections, allows in-flight RPCs to complete, then returns. After that, the code logs "Shutting down", drops the `WriterHandle`, and awaits the writer task's `JoinHandle`.
- The `WriterHandle` is cloned before being passed to `EventfoldService::new()`. The service's clone is dropped when `serve_with_incoming_shutdown` returns (the server structure is dropped). The explicit `drop(writer_handle)` then drops the last reference, closing the mpsc channel and signaling the writer task to drain and exit.
- `tokio-stream` was promoted from dev-dependency to regular dependency. This is necessary because `TcpListenerStream` is used in the binary's `main()` function, not just in tests. The ticket scope implicitly requires this since it calls for `serve_with_incoming` with a `TcpListenerStream`.

## Acceptance Criteria
- [x] AC: `main` is declared `#[tokio::main] async fn main()` and calls `init_tracing()`, `Config::from_env()`, `Store::open()`, `Broker::new()`, `spawn_writer()`, `EventfoldService::new()`, and starts the tonic server - Lines 114-199 of `src/main.rs`
- [x] AC: If `Config::from_env()` returns `Err(msg)`, the process prints `msg` to stderr and exits with code 1 - Lines 120-126, uses `eprintln!` + `std::process::exit(1)`
- [x] AC: If `Store::open()` returns `Err(e)`, the process logs `tracing::error!` and exits with code 1 - Lines 134-140
- [x] AC: Startup emits `tracing::info!` lines for data path, listen address, broker capacity, recovered event count, and recovered stream count - Lines 129-131 (config) and 142-150 (recovered counts)
- [x] AC: The server binds using `tokio::net::TcpListener::bind(config.listen_addr)` and passes the listener to `serve_with_incoming` so an OS-assigned port (`0`) works correctly in tests - Lines 165-187
- [x] AC: After `serve_with_incoming` begins, emits `tracing::info!("Server listening on {addr}")` where `addr` is the actual bound address from `listener.local_addr()` - Line 183
- [x] AC: Graceful shutdown on SIGINT/SIGTERM, logs "Shutting down", drops WriterHandle, awaits JoinHandle - Lines 95-112 (shutdown_signal), 185-199 (serve + cleanup)
- [x] AC: On non-Unix platforms, code compiles without errors; SIGTERM handling conditionally compiled with `#[cfg(unix)]` - Lines 98-111
- [x] AC: `cargo build --bin eventfold-db` produces zero warnings and zero errors - Verified
- [x] AC: Binary exits non-zero without EVENTFOLD_DATA, stderr contains "EVENTFOLD_DATA" - Tested in `binary_exits_nonzero_without_eventfold_data`
- [x] AC: Quality gates pass - All four gates verified clean

## Test Results
- Build: PASS (zero warnings, zero errors)
- Clippy: PASS (`cargo clippy --all-targets --all-features --locked -- -D warnings`)
- Fmt: PASS (`cargo fmt --check`)
- Tests: PASS (184 total: 150 lib + 8 main + 2 broker_integration + 23 grpc_service + 1 writer_integration)
- New tests added: `src/main.rs::tests::binary_exits_nonzero_without_eventfold_data`
- Manual smoke test: `EVENTFOLD_DATA=/tmp/test.log EVENTFOLD_LISTEN=127.0.0.1:0 cargo run` starts successfully, logs all expected info lines, binds to ephemeral port, and shuts down cleanly on SIGTERM

## Concerns / Blockers
- `Cargo.toml` was modified to promote `tokio-stream` from dev-dependency to regular dependency. This is outside the explicit ticket scope of "Modify: `src/main.rs`" but is required by the ticket's instructions to use `TcpListenerStream` in the binary. The change is minimal and correct.
- The ticket's AC mentions `read_index.event_count()` and `read_index.stream_count()` methods which do not exist on `ReadIndex`. The implementation uses `store.log().read().events.len()` and `.streams.len()` instead, which provides the same information. Adding those methods to `ReadIndex` would require modifying `src/reader.rs` (out of scope). Downstream tickets can add convenience methods if desired.
