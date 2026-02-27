# Code Review: Ticket 2 -- Implement `main.rs` Startup Sequence and Graceful Shutdown

**Ticket:** 2 -- Implement `main.rs` Startup Sequence and Graceful Shutdown
**Impl Report:** prd/007-server-binary-reports/ticket-02-impl.md
**Date:** 2026-02-26 17:45
**Verdict:** APPROVED

---

## AC Coverage

| AC # | Description | Status | Notes |
|------|-------------|--------|-------|
| 1 | `main` is `#[tokio::main] async fn main()` and calls init_tracing, Config::from_env, Store::open, Broker::new, spawn_writer, EventfoldService::new, starts tonic server | Met | Lines 114-200 of `src/main.rs`. All seven calls present in sequence: `init_tracing()` (117), `Config::from_env()` (120), `Store::open()` (134), `Broker::new()` (153), `spawn_writer()` (156), `EventfoldService::new()` (159), `server.serve_with_incoming_shutdown()` (186-192). |
| 2 | Config::from_env() Err -> eprintln + exit(1) | Met | Lines 120-126: `match Config::from_env()` with `Err(msg) => { eprintln!("{msg}"); std::process::exit(1); }`. |
| 3 | Store::open() Err -> tracing::error! + exit(1) | Met | Lines 134-140: `match Store::open()` with `Err(e) => { tracing::error!(error = %e, "Failed to open store"); std::process::exit(1); }`. |
| 4 | Startup emits tracing::info! for data path, listen address, broker capacity, recovered event count, recovered stream count | Met | Lines 129-131: data_path, listen_addr, broker_capacity. Lines 148-149: events count, streams count. All five info lines present. Implementation reads from `store.log().read().events.len()` and `.streams.len()` since `ReadIndex` lacks `event_count()`/`stream_count()` methods -- functionally equivalent and within scope. |
| 5 | Server binds via TcpListener::bind(config.listen_addr) and passes to serve_with_incoming | Met | Lines 165-180: `tokio::net::TcpListener::bind(config.listen_addr)` followed by `TcpListenerStream::new(listener)` passed to `serve_with_incoming_shutdown()`. Uses `_shutdown` variant (strictly better -- integrates graceful shutdown). OS-assigned port 0 works correctly. |
| 6 | After binding, emits tracing::info!("Server listening on {addr}") with actual bound address | Met | Line 176-178: `listener.local_addr().expect(...)` captures actual bound addr. Line 183: `tracing::info!("Server listening on {addr}")`. |
| 7 | Graceful shutdown: SIGINT/SIGTERM -> log "Shutting down", drop WriterHandle, await JoinHandle | Met | Lines 95-112: `shutdown_signal()` uses `tokio::select!` on Unix for SIGINT+SIGTERM. Lines 195-199: after server returns, logs "Shutting down", `drop(writer_handle)`, `join_handle.await.expect(...)`. The `serve_with_incoming_shutdown` returns once the signal fires, which is the correct integration point. |
| 8 | Non-Unix platforms: SIGTERM conditionally compiled with #[cfg(unix)] | Met | Lines 98-106: `#[cfg(unix)]` block with SIGTERM. Lines 108-111: `#[cfg(not(unix))]` fallback to ctrl_c only. |
| 9 | `cargo build --bin eventfold-db` produces zero warnings and errors | Met | Verified independently: `cargo build --bin eventfold-db` completes with "Finished" and zero warnings. |
| 10 | Binary exits non-zero without EVENTFOLD_DATA, stderr contains "EVENTFOLD_DATA" | Met | Test `binary_exits_nonzero_without_eventfold_data` (lines 300-322) runs the binary as a subprocess via `std::process::Command`, asserts non-zero exit and stderr contains "EVENTFOLD_DATA". |
| 11 | Quality gates pass | Met | Verified independently: `cargo build` (0 warnings), `cargo clippy --all-targets --all-features --locked -- -D warnings` (clean), `cargo fmt --check` (clean), `cargo test` (184 passed, 0 failed). |

## Issues Found

### Critical (must fix before merge)
- None.

### Major (should fix, risk of downstream problems)
- None.

### Minor (nice to fix, not blocking)
- **Hardcoded writer channel capacity.** Line 156: `spawn_writer(store, 64, broker.clone())` uses a hardcoded `64` for the mpsc channel capacity. This is not configurable via environment variable. The PRD only specifies `EVENTFOLD_BROKER_CAPACITY` for the broadcast channel, so this is within spec. However, a named constant (e.g., `const WRITER_CHANNEL_CAPACITY: usize = 64;`) would improve readability and make it easier to find/change later.

## Suggestions (non-blocking)
- The `#[allow(dead_code)]` annotations from Ticket 1 have been correctly removed now that all items are used in `main()`. Good cleanup.
- The use of `serve_with_incoming_shutdown` instead of `serve_with_incoming` is the right choice -- it integrates the shutdown signal directly into tonic's serve loop, allowing graceful connection draining before the shutdown sequence begins.
- The `WriterHandle` clone/drop pattern (clone before passing to service, drop original after server exits) correctly ensures the mpsc channel closes only after all senders are dropped. The service's clone is dropped when `serve_with_incoming_shutdown` returns (server structure is dropped), and the explicit `drop(writer_handle)` closes the last sender, signaling the writer task to drain and exit.
- The `expect` on `join_handle.await` (line 199) is appropriate: a panic in the writer task is an invariant violation (programmer error), not an operational failure.

## Scope Check
- Files within scope: YES. `src/main.rs` and `Cargo.toml` modified as specified.
- Scope creep detected: NO. The `Cargo.toml` changes are: (a) `tracing-subscriber` added (from Ticket 1, already reviewed and approved), (b) `tokio-stream` promoted from dev-dep to regular dep (required for `TcpListenerStream` in the binary, explicitly noted in ticket scope), (c) `serial_test` added to dev-deps (from Ticket 1, already reviewed and approved). Ticket 2's own Cargo.toml change is solely the `tokio-stream` promotion, which is both necessary and noted in the ticket description.
- Unauthorized dependencies added: NO.

## Risk Assessment
- Regression risk: LOW. All 184 existing + new tests pass. The change replaces the placeholder `fn main()` with the production entrypoint. No library code was modified.
- Security concerns: NONE. Configuration reads from environment variables (trusted process-level input). No secrets handling, no user-facing input validation issues.
- Performance concerns: NONE. Startup is a one-time sequence. The `std::sync::RwLock::read()` on the EventLog (line 146) is held briefly in a scoped block before spawning the writer task -- no contention possible at this point.
