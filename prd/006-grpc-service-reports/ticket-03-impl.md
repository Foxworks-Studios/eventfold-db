# Implementation Report: Ticket 3 -- Implement Append, ReadStream, and ReadAll Unary RPCs

**Ticket:** 3 - Implement Append, ReadStream, and ReadAll Unary RPCs
**Date:** 2026-02-26 12:00
**Status:** COMPLETE

---

## Files Changed

### Created
- `tests/grpc_service.rs` - Integration tests with `start_test_server` helper and 14 test cases covering all AC test requirements.

### Modified
- `src/service.rs` - Added `#[tonic::async_trait] impl EventStore for EventfoldService` with `append`, `read_stream`, `read_all` methods, and `subscribe_all`/`subscribe_stream` stubs. Added `SubscriptionStream` type alias for the subscription associated types.
- `src/lib.rs` - Added `pub use service::EventfoldService;` re-export so integration tests (and downstream consumers) can access the type at the crate root.
- `Cargo.toml` - Added `tokio-stream = "0.1"` as a dev-dependency for `TcpListenerStream` used in the `start_test_server` helper.

## Implementation Notes
- The `append` handler validates in this order: stream_id UUID, expected_version presence, events non-empty, each event_id UUID. Then delegates to `WriterHandle::append` and maps errors via `error_to_status`.
- The `read_stream` handler validates stream_id UUID, then delegates to `ReadIndex::read_stream`, mapping `StreamNotFound` to `NOT_FOUND`.
- The `read_all` handler delegates directly to `ReadIndex::read_all` (infallible), no validation needed.
- Subscription methods are stubbed with `unimplemented!("PRD 006 Ticket 4")` as specified. The associated stream types use `Pin<Box<dyn Stream<Item = Result<SubscribeResponse, Status>> + Send>>`.
- The `start_test_server` helper follows the exact pattern from the ticket: `TcpListener::bind("[::1]:0")` -> `TcpListenerStream` -> `Server::builder().serve_with_incoming()`. A 50ms sleep ensures the server is accepting before client connects.
- The oversized payload test (AC-7) uses 65,537 bytes, which exceeds MAX_EVENT_SIZE (65,536). The error propagates from the Store's validation through the writer task back to the gRPC handler as `EventTooLarge`, which maps to `INVALID_ARGUMENT`.
- Added `tokio-stream` as a dev-dependency rather than a regular dependency, since it's only needed by the test helper for `TcpListenerStream`. It was already a transitive dependency through tonic.

## Acceptance Criteria
- [x] AC: `EventfoldService` implements the tonic-generated `EventStore` trait - Done via `#[tonic::async_trait] impl proto::event_store_server::EventStore for EventfoldService`
- [x] AC: `append` handler validates, delegates, returns AppendResponse, maps errors - All validation steps implemented with proper error mapping
- [x] AC: `read_stream` handler validates stream_id, delegates, maps StreamNotFound to NOT_FOUND - Implemented
- [x] AC: `read_all` handler calls read_index.read_all (infallible), returns ReadAllResponse - Implemented
- [x] Test AC-1: Append 1 event, no_stream -> correct first/last positions - `ac1_append_single_event_no_stream`
- [x] Test AC-2: Append 3 events batch -> first_stream_version=0, last=2 - `ac2_append_batch_three_events`
- [x] Test AC-3: Append no_stream twice -> FAILED_PRECONDITION - `ac3_append_no_stream_twice_fails`
- [x] Test AC-4: Append with stream_id="not-a-uuid" -> INVALID_ARGUMENT - `ac4_append_invalid_stream_id`
- [x] Test AC-5: Append with empty events list -> INVALID_ARGUMENT - `ac5_append_empty_events`
- [x] Test AC-6: Append with event_id="bad-uuid" -> INVALID_ARGUMENT - `ac6_append_invalid_event_id`
- [x] Test AC-7: Append with oversized payload (>64KB) -> INVALID_ARGUMENT - `ac7_append_oversized_payload`
- [x] Test AC-8: Append 5 events; ReadStream from 0, max 100 -> 5 events in order - `ac8_read_stream_all_events`
- [x] Test AC-9: ReadStream from version 2, max 2 -> 2 events at versions 2,3 - `ac9_read_stream_partial`
- [x] Test AC-10: ReadStream non-existent stream -> NOT_FOUND - `ac10_read_stream_not_found`
- [x] Test AC-11: ReadStream with stream_id="not-a-uuid" -> INVALID_ARGUMENT - `ac11_read_stream_invalid_stream_id`
- [x] Test AC-12: Append across 2 streams; ReadAll from 0 -> 5 events in global order - `ac12_read_all_two_streams`
- [x] Test AC-13: ReadAll from position 3, max 2 -> events at positions 3,4 - `ac13_read_all_partial`
- [x] Test AC-14: ReadAll on empty store -> 0 events - `ac14_read_all_empty_store`
- [x] `start_test_server` helper defined in tests/grpc_service.rs - Returns `(EventStoreClient<Channel>, SocketAddr, TempDir)`
- [x] Quality gates pass - All four gates clean

## Test Results
- Lint: PASS (`cargo clippy --all-targets --all-features --locked -- -D warnings` -- zero warnings)
- Tests: PASS (165 total: 148 unit + 2 broker integration + 14 new gRPC integration + 1 writer integration)
- Build: PASS (`cargo build` -- zero warnings)
- Format: PASS (`cargo fmt --check` -- clean)
- New tests added: 14 tests in `tests/grpc_service.rs`

## Concerns / Blockers
- None. All acceptance criteria met. The subscription stubs are in place for Ticket 4 to implement.
