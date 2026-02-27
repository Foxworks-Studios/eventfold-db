# Implementation Report: Ticket 3 -- Integration Tests for Server Binary Startup, Config, Recovery, and Round-Trip

**Ticket:** 3 - Integration Tests for Server Binary Startup, Config, Recovery, and Round-Trip
**Date:** 2026-02-26 16:00
**Status:** COMPLETE

---

## Files Changed

### Created
- `tests/server_binary.rs` - Integration tests with `start_server_at` helper and 5 tests covering AC-1, AC-2, AC-5, AC-6, AC-8

### Modified
- None

## Implementation Notes

- **`start_server_at` helper** accepts `&Path`, `SocketAddr`, and `usize` as specified. Opens a real `Store`, spawns the full stack (writer task, broker, EventfoldService, tonic Server), and returns a connected `EventStoreClient<Channel>` plus a `ServerHandle`. The helper follows the same wiring pattern as `start_test_server` in `tests/grpc_service.rs` but is parameterized.

- **`ServerHandle` struct** provides clean lifecycle management. Uses `serve_with_incoming_shutdown` with a `tokio::sync::oneshot` channel (instead of `serve_with_incoming` + abort) for reliable shutdown. The `shutdown()` method signals the tonic server to stop, awaits it (which drops the service's `WriterHandle` clone), drops our `WriterHandle` (closing the mpsc channel), then awaits the writer task. This avoids the hang that occurs with `JoinHandle::abort()` on the tonic server future.

- **AC signature deviation**: The ticket AC says `start_server_at` returns `(EventStoreClient<Channel>, TempDir)`. However, the recovery test (AC-6) requires explicit server shutdown and restart at the same data path, which requires lifecycle control beyond what a TempDir provides. The helper instead returns `(EventStoreClient<Channel>, ServerHandle)` where `ServerHandle` provides `shutdown()`. The caller manages the `TempDir` externally. This is functionally equivalent -- the TempDir is created in each test and kept alive for the test's duration.

- **`binary_exits_nonzero_without_eventfold_data`** test is complementary to the unit test in `src/main.rs::tests`. Both run the binary as a subprocess and check exit code + stderr. The existing unit test already covers AC-2 adequately; this integration test provides additional coverage from the external test harness perspective.

- **No files modified outside scope**. All changes are confined to the new `tests/server_binary.rs` file.

## Acceptance Criteria

- [x] AC: `start_server_at` helper accepts `&Path`, `SocketAddr`, `usize`; opens real Store; spawns full stack; returns client + handle -- implemented with `ServerHandle` instead of raw `TempDir` (see notes above)
- [x] AC (AC-1): test sends ReadAll from position 0 to fresh server, asserts 0 events and RPC success -- `ac1_server_starts_and_accepts_grpc`
- [x] AC (AC-5): test uses broker capacity 16, appends 20 events without consuming, starts SubscribeAll, asserts stream terminates or errors (lag) -- `ac5_small_broker_capacity_causes_lag`
- [x] AC (AC-6): test appends 5 events, shuts down server, opens new server at same path, ReadAll returns 5, new Append returns first_global_position==5 -- `ac6_recovery_on_restart`
- [x] AC (AC-8): test appends 3 to A, 2 to B with no_stream; ReadStream A=3, B=2; ReadAll=5 in order 0..4; SubscribeAll from 0 collects 5 events + CaughtUp -- `ac8_end_to_end_round_trip`
- [x] AC (EVENTFOLD_DATA missing): test runs binary subprocess without EVENTFOLD_DATA, asserts non-zero exit and stderr contains "EVENTFOLD_DATA" -- `binary_exits_nonzero_without_eventfold_data`
- [x] AC: All existing tests in `tests/grpc_service.rs` continue to pass without modification -- 23 tests, all green
- [x] AC: Quality gates pass -- cargo build (0 warnings), cargo clippy (0 warnings), cargo fmt --check (clean), cargo test (189 tests passing)

## Test Results

- Lint: PASS (`cargo clippy --all-targets --all-features --locked -- -D warnings` -- zero warnings)
- Tests: PASS (189 tests: 150 lib + 8 main + 2 broker_integration + 23 grpc_service + 5 server_binary + 1 writer_integration)
- Build: PASS (`cargo build` -- zero warnings)
- Format: PASS (`cargo fmt --check` -- clean)
- New tests added:
  - `tests/server_binary.rs::ac1_server_starts_and_accepts_grpc`
  - `tests/server_binary.rs::ac5_small_broker_capacity_causes_lag`
  - `tests/server_binary.rs::ac6_recovery_on_restart`
  - `tests/server_binary.rs::ac8_end_to_end_round_trip`
  - `tests/server_binary.rs::binary_exits_nonzero_without_eventfold_data`

## Concerns / Blockers

- The `start_server_at` return type is `(EventStoreClient<Channel>, ServerHandle)` instead of the AC-specified `(EventStoreClient<Channel>, TempDir)`. This change was necessary because the recovery test (AC-6) requires explicit shutdown control to restart the server at the same data path. The `ServerHandle` struct provides `shutdown()` for clean lifecycle management. The `TempDir` is created and managed by each test function directly.
