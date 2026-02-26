# Code Review: Ticket 5 -- `subscribe_stream`: Stream-Scoped Catch-Up, Filtering, and Non-Existent Stream

**Ticket:** 5 -- subscribe_stream
**Impl Report:** prd/005-subscription-broker-reports/ticket-05-impl.md
**Date:** 2026-02-26 15:30
**Verdict:** APPROVED

---

## AC Coverage

| AC # | Description | Status | Notes |
|------|-------------|--------|-------|
| 1 | `subscribe_stream(read_index, broker, stream_id, from_version)` returns `impl Stream<Item = Result<SubscriptionMessage, Error>>` | Met | Signature at `src/broker.rs:200-205` matches exactly. Uses `futures_core::Stream` as return type, consistent with `subscribe_all`. |
| 2 | Broadcast receiver registered BEFORE historical read | Met | Line 207: `let mut rx = broker.subscribe();` is called before entering the `stream!` macro where catch-up reads occur (line 216). Same ordering invariant as `subscribe_all`. |
| 3 | Catch-up reads via `read_index.read_stream`; StreamNotFound yields immediate CaughtUp | Met | Lines 216-241: `read_stream(stream_id, cursor, CATCHUP_BATCH_SIZE)` in a loop. `Err(Error::StreamNotFound { .. })` breaks the loop (line 232-235), then CaughtUp is yielded at line 245. Unexpected errors are propagated (lines 236-239). |
| 4 | `last_catchup_version` tracked; live phase filters by stream_id AND dedup by stream_version | Met | `last_catchup_version: Option<u64>` (line 212). Updated to `Some(event.stream_version)` on each catch-up event (line 221). Live phase filters `arc_event.stream_id != stream_id` (line 252) then deduplicates `arc_event.stream_version <= last_ver` (lines 257-261). The use of `Option<u64>` instead of the ticket's `u64::MAX` sentinel is a correct improvement: `None` means "accept all live events" which is correct for no-catch-up scenarios, whereas `u64::MAX` would have incorrectly rejected all live events. |
| 5 | Lag termination identical to subscribe_all | Met | Lines 265-270: exact same pattern -- `RecvError::Lagged(_)` yields `Err(Error::InvalidArgument("subscription lagged: re-subscribe from last checkpoint"))` and returns. Closed channel also terminates (lines 271-274). |
| AC-8 | Test: interleaved streams, only stream A's 3 events returned | Met | Test `ac8_subscribe_stream_catchup_interleaved` (lines 643-694): appends A,B,A,B,A interleaved; subscribes to stream A from version 0; asserts exactly 3 events with stream_id == A and stream_versions [0, 1, 2]. Uses `read_index.stream_version()` for correct expected version tracking. |
| AC-9 | Test: live filtering, only stream A's event appears | Met | Test `ac9_subscribe_stream_live_filtering` (lines 698-771): subscribes to empty store, drives until CaughtUp, appends B,A,B, asserts exactly 1 live event for stream A with correct event_type and stream_version. Uses timeout to prevent hangs. |
| AC-10 | Test: non-existent stream, immediate CaughtUp, then live event arrives | Met | Test `ac10_subscribe_stream_nonexistent_then_live` (lines 776-830): subscribes to UUID with no events, asserts zero catch-up events before CaughtUp, appends one event to that stream, asserts it arrives live with stream_version == 0. Uses timeout. |
| 9 | `subscribe_stream` accessible at crate root | Met | `src/lib.rs:11`: `pub use broker::{Broker, subscribe_all, subscribe_stream};` re-exports all three. |
| 10 | Quality gates pass | Met | Verified independently: `cargo test` (131 tests, all green), `cargo clippy --all-targets --all-features --locked -- -D warnings` (clean), `cargo fmt --check` (clean). |

## Issues Found

### Critical (must fix before merge)
- None

### Major (should fix, risk of downstream problems)
- None

### Minor (nice to fix, not blocking)
- None

## Suggestions (non-blocking)

1. **Dedup boundary documentation:** The deviation from the ticket's `u64::MAX` sentinel to `Option<u64>` is correct (using `u64::MAX` would have been a bug), but a brief inline comment explaining why `None` means "accept all live events" would help future readers understand the design choice. The current comment at line 256 ("skip events already sent during catch-up") partially covers this, but does not explain the `None` case.

2. **Structural similarity with `subscribe_all`:** The two subscription functions share significant structural patterns (broadcast registration, batch loop, CaughtUp emission, live phase with dedup, lag termination). If more subscription variants are added in the future, extracting the shared skeleton into a helper could reduce duplication. Not necessary now with only two functions.

## Scope Check
- Files within scope: YES -- `src/broker.rs` (modified) and `src/lib.rs` (modified) are the only files listed in the ticket scope
- Scope creep detected: NO -- the `lib.rs` changes include Ticket 1-4 additions (`pub mod broker`, `Broker`, `subscribe_all`, `SubscriptionMessage`) but these are part of the same uncommitted working tree from earlier tickets in this PRD. The Ticket 5 delta is specifically `subscribe_stream` in the re-export line, which is in scope.
- Unauthorized dependencies added: NO

## Risk Assessment
- Regression risk: LOW -- the new function is additive (no existing code modified). All 131 tests pass including pre-existing tests from PRDs 001-004.
- Security concerns: NONE
- Performance concerns: NONE -- follows the same batched catch-up pattern as `subscribe_all` with bounded memory usage via `CATCHUP_BATCH_SIZE`. Live-phase filtering adds a stream_id comparison per broadcast event, which is O(1) per event.
