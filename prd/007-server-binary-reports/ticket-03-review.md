# Code Review: Ticket 3 -- Integration Tests for Server Binary Startup, Config, Recovery, and Round-Trip

**Ticket:** 3 -- Integration Tests for Server Binary Startup, Config, Recovery, and Round-Trip
**Impl Report:** prd/007-server-binary-reports/ticket-03-impl.md
**Date:** 2026-02-26 17:30
**Verdict:** APPROVED

---

## AC Coverage

| AC # | Description | Status | Notes |
|------|-------------|--------|-------|
| 1 | `start_server_at` accepts `&Path`, `SocketAddr`, `usize`; opens real Store; spawns full stack; returns client + handle | Met | Function signature at line 73-77 matches: `(data_path: &Path, listen_addr: SocketAddr, broker_capacity: usize) -> (EventStoreClient<Channel>, ServerHandle)`. Opens real `Store::open`, spawns writer via `spawn_writer`, creates `Broker`, `EventfoldService`, binds tonic server with `serve_with_incoming_shutdown`. Returns `ServerHandle` instead of `TempDir` -- justified deviation for AC-6 recovery test requiring explicit shutdown. TempDir managed by each test function directly. |
| 2 | Test (AC-1): start server, ReadAll from 0, assert 0 events | Met | `ac1_server_starts_and_accepts_grpc` at line 142: creates tempdir, starts server on `[::1]:0`, sends `ReadAll { from_position: 0, max_count: 100 }`, asserts `events.len() == 0`, calls `handle.shutdown()`. |
| 3 | Test (AC-5): broker capacity 16, append 20 events, SubscribeAll shows lag | Met | `ac5_small_broker_capacity_causes_lag` at line 166: starts server with capacity 16, subscribes, receives CaughtUp on empty store, appends 20 events without consuming, then reads subscription in a loop with 5-second deadline -- breaks on stream end, gRPC error, or `None` content. Correctly proves broadcast overflow behavior. |
| 4 | Test (AC-6): append 5 events, restart at same path, ReadAll returns 5, new Append starts at position 5 | Met | `ac6_recovery_on_restart` at line 242: scoped blocks for first/second server. First block appends 5 events + graceful shutdown. Second block re-opens at same `data_path`, asserts `ReadAll` returns 5 events with contiguous global positions 0..4, then appends a new event and asserts `first_global_position == 5`. Clean recovery verification. |
| 5 | Test (AC-8): multi-stream round-trip with all RPCs | Met | `ac8_end_to_end_round_trip` at line 308: appends 3 to stream A, 2 to stream B (both `no_stream`); `ReadStream` A asserts 3 events with correct stream_version and stream_id; `ReadStream` B asserts 2 events; `ReadAll` asserts 5 events with contiguous global positions; `SubscribeAll` from 0 collects events until CaughtUp, asserts 5 events with contiguous positions. All 4 RPCs exercised (Append, ReadStream, ReadAll, SubscribeAll). |
| 6 | Test (EVENTFOLD_DATA missing): binary exits non-zero, stderr contains "EVENTFOLD_DATA" | Met | `binary_exits_nonzero_without_eventfold_data` at line 434: runs `cargo run --bin eventfold-db --quiet` via `std::process::Command`, removes `EVENTFOLD_DATA`/`EVENTFOLD_LISTEN`/`EVENTFOLD_BROKER_CAPACITY` from env, asserts non-zero exit and stderr contains "EVENTFOLD_DATA". Uses `#[test]` (not `#[tokio::test]`) -- appropriate since it's a synchronous subprocess test. |
| 7 | All existing tests in `tests/grpc_service.rs` continue to pass | Met | Verified via `cargo test`: 23 grpc_service tests pass. `git diff HEAD -- tests/grpc_service.rs` shows no modifications. |
| 8 | Quality gates pass | Met | Verified independently: `cargo test` (189 tests, all green), `cargo clippy --all-targets --all-features --locked -- -D warnings` (zero warnings), `cargo fmt --check` (clean), `cargo build` (zero warnings). |

## Issues Found

### Critical (must fix before merge)

None.

### Major (should fix, risk of downstream problems)

None.

### Minor (nice to fix, not blocking)

1. **Duplicated helpers `make_proposed` and `no_stream`** -- These are identical to the helpers in `tests/grpc_service.rs` (lines 62-76 of `grpc_service.rs` vs lines 120-134 of `server_binary.rs`). Each integration test file independently defines the same two functions. This is a minor DRY concern; extracting to a shared test utility module (`tests/common/mod.rs`) would eliminate the duplication. Not blocking because test helper duplication is common in Rust integration tests (each file in `tests/` is a separate crate), and the helpers are small (< 15 lines each).

2. **`_handle` not shut down in some tests** -- In `ac5_small_broker_capacity_causes_lag` (line 172) and `ac8_end_to_end_round_trip` (line 313), the `ServerHandle` is bound to `_handle` but never explicitly shut down. When the test ends, the `ServerHandle` is dropped, which drops the `oneshot::Sender` (causing the tonic server to stop) and drops the `WriterHandle`, but the `JoinHandle`s are dropped without being awaited. This means the writer task and server task are detached rather than gracefully joined. In practice this is harmless for tests (the tokio runtime shuts down the tasks), but it's inconsistent with `ac1_server_starts_and_accepts_grpc` and `ac6_recovery_on_restart` which call `handle.shutdown().await`. Could add a `Drop` impl or always call shutdown for consistency.

## Suggestions (non-blocking)

1. **Consider `EPHEMERAL_ADDR` as a `SocketAddr` constant instead of `&str`** -- `EPHEMERAL_ADDR` is defined as `&str` and parsed in every test. Since `SocketAddr` doesn't implement `const` construction easily, this is fine as-is, but a `fn ephemeral_addr() -> SocketAddr` helper would avoid the `.parse().expect()` in each test.

2. **The `ServerHandle::shutdown` method is well-designed** -- The explicit ordering (signal tonic -> await server -> drop writer handle -> await writer) is correct and avoids the deadlock that would occur if the writer handle were dropped before the server (since the server's `EventfoldService` clone holds another `WriterHandle`). Good engineering decision.

## Scope Check

- Files within scope: YES -- Only `tests/server_binary.rs` was created. No other files were modified.
- Scope creep detected: NO
- Unauthorized dependencies added: NO

## Risk Assessment

- Regression risk: LOW -- No existing code was modified. All 23 `grpc_service` tests continue to pass. The new tests are additive and isolated via `tempfile::tempdir()`.
- Security concerns: NONE
- Performance concerns: NONE -- The 50ms sleep in `start_server_at` (line 103) matches the pattern in `grpc_service.rs` and is standard for test server startup. The `binary_exits_nonzero_without_eventfold_data` test invokes `cargo run` as a subprocess, which is slow (~1-2s) but acceptable for a single test.
