# Code Review: Ticket 4 -- Implement SubscribeAll and SubscribeStream Server-Streaming RPCs

**Ticket:** 4 -- Implement SubscribeAll and SubscribeStream Server-Streaming RPCs
**Impl Report:** prd/006-grpc-service-reports/ticket-04-impl.md
**Date:** 2026-02-26 19:15
**Verdict:** APPROVED

---

## AC Coverage

| AC # | Description | Status | Notes |
|------|-------------|--------|-------|
| 1 (subscribe_all handler) | Maps Event/CaughtUp/Err correctly | Met | Lines 160-205 of `src/service.rs`: `subscribe_all` clones `read_index` and `broker`, moves them into `async_stream::stream!`, calls `crate::subscribe_all(...)`, maps all three `SubscriptionMessage` variants plus `Err` and `None` correctly. |
| 2 (subscribe_stream handler) | Validates stream_id, maps identically | Met | Lines 214-262 of `src/service.rs`: validates `stream_id` via `parse_uuid`, clones handles, calls `crate::subscribe_stream(...)`, mapping logic is identical to `subscribe_all`. |
| AC-15 | Catch-up + live flow | Met | Test `ac15_subscribe_all_catchup_then_live` (lines 441-525 of `tests/grpc_service.rs`): appends 3 events, subscribes from 0, verifies 3 events at positions 0-2 + CaughtUp, appends 2 more with `Exact(2)`, verifies live events at positions 3-4. |
| AC-16 | Catch-up from middle | Met | Test `ac16_subscribe_all_from_middle` (lines 529-583): appends 5 events, subscribes from position 3, verifies 2 events at positions 3-4 + CaughtUp. |
| AC-17 | Per-stream catch-up + live + filtering | Met | Test `ac17_subscribe_stream_catchup_and_live` (lines 588-700): appends A, B, A interleaved, subscribes to stream A from version 0, verifies 2 events with `stream_id == A` at versions 0-1, CaughtUp, appends A (version 2), verifies live event with `stream_id == A`. |
| AC-18 | Live filtering | Met | Test `ac18_subscribe_stream_filters_other_streams` (lines 704-797): subscribes to A on empty store, receives CaughtUp, appends B, C, A, B, verifies only A's event arrives (first message after CaughtUp is stream A, implicitly proving B/C are filtered since they were appended first). |
| AC-19 | Non-existent stream | Met | Test `ac19_subscribe_stream_nonexistent_then_live` (lines 802-861): subscribes to non-existent stream, receives CaughtUp immediately, appends to that stream, verifies live event with `stream_version == 0`. |
| AC-20 | Lag termination | Met | Test `ac20_subscribe_all_lag_terminates_stream` (lines 866-935): uses `start_test_server_with_broker_capacity(4)`, subscribes, reads CaughtUp, appends 20 events without reading, then polls until stream terminates (via lag error or None). Lag is guaranteed with 20 events in a capacity-4 buffer. |
| AC-21 | Concurrent subscriptions | Met | Test `ac21_two_concurrent_subscribe_all` (lines 939-1031): starts 2 `SubscribeAll` subscriptions, appends 3 events, verifies both receive positions 0-2 during catch-up, appends 2 more, verifies both receive positions 3-4 live. Uses a `drain_catchup` helper function. |
| Quality gates | Build, clippy, fmt, test all pass | Met | Verified: `cargo test` (172 total: 148 unit + 24 integration), `cargo clippy -- -D warnings` clean, `cargo fmt --check` clean, `cargo build` zero warnings. |

## Issues Found

### Critical (must fix before merge)
- None

### Major (should fix, risk of downstream problems)
- None

### Minor (nice to fix, not blocking)

1. **Stale doc comment on `SubscriptionStream` type alias** (`src/service.rs`, lines 57-61): The doc comment still reads "Subscription handlers (Ticket 4) will return..." and "The stubs below use this type..." -- both references are outdated now that the stubs have been replaced with real implementations. Should say something like "Subscription handlers return a pinned, boxed, `Send` stream..." and remove the "stubs" language.

2. **AC-20 test assertion could be stronger** (`tests/grpc_service.rs`, lines 912-934): The loop breaks on any of three conditions (received all events, stream ended, gRPC error) without explicitly asserting WHICH exit condition was hit. A flag tracking whether a lag error or stream-end was observed (similar to the unit test `ac11_subscribe_all_lag_termination` which does `assert!(got_error)`) would make the test more self-documenting. In practice, lag is guaranteed with capacity=4 and 20 events, so this is not a correctness concern.

## Suggestions (non-blocking)

1. **Code duplication in mapping logic**: The `async_stream::stream!` bodies in `subscribe_all` and `subscribe_stream` (lines 171-202 and 226-259) are identical except for the initial `crate::subscribe_all(...)` vs `crate::subscribe_stream(...)` call. A private helper function that takes a `Stream<Item = Result<SubscriptionMessage, Error>>` and returns a `SubscriptionStream` could eliminate the duplication. However, `async_stream::stream!` macros do not compose easily with trait objects, and the duplication is only ~30 lines in a two-method trait impl, so this is acceptable as-is.

2. **`poll_fn` pattern vs. `StreamExt::next()`**: The implementer correctly noted that `futures` (providing `StreamExt`) is a dev-dependency only, and used `std::future::poll_fn` with `Stream::poll_next` to avoid adding a production dependency. This is the right call, though if `futures-core` ever re-exports `StreamExt`, it could be simplified. Good engineering judgment here.

## Scope Check
- Files within scope: YES -- only `src/service.rs` and `tests/grpc_service.rs` were modified.
- Scope creep detected: NO -- the test helper refactoring (`start_test_server` delegating to `start_test_server_with_broker_capacity`) is a minimal, necessary change to support AC-20 and stays within the test file scope.
- Unauthorized dependencies added: NO -- no new dependencies. `futures-core` and `async-stream` were already in `Cargo.toml`.

## Risk Assessment
- Regression risk: LOW -- the changes are additive (two new trait method implementations replacing stubs). All 172 tests pass, including all pre-existing tests from prior PRDs. The `start_test_server` refactoring is backward-compatible (the original function delegates to the new one with the same default capacity of 1024).
- Security concerns: NONE -- subscription handlers validate `stream_id` via `parse_uuid` (for `subscribe_stream`); `subscribe_all` has no user-provided UUID to validate.
- Performance concerns: NONE -- the `poll_fn` approach adds no overhead vs. `StreamExt::next()`. The `Arc`-based event sharing in the broker is correct per CLAUDE.md guidance. Cloning `ReadIndex` and `Broker` are cheap `Arc`-based clones.
