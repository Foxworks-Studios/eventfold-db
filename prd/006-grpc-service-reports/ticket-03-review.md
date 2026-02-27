# Code Review: Ticket 3 -- Implement Append, ReadStream, and ReadAll Unary RPCs

**Ticket:** 3 -- Implement Append, ReadStream, and ReadAll Unary RPCs
**Impl Report:** prd/006-grpc-service-reports/ticket-03-impl.md
**Date:** 2026-02-26 14:30
**Verdict:** APPROVED

---

## AC Coverage

| AC # | Description | Status | Notes |
|------|-------------|--------|-------|
| 1 | EventfoldService implements tonic EventStore trait | Met | `#[tonic::async_trait] impl proto::event_store_server::EventStore for EventfoldService` at service.rs:64-175 |
| 2 | append validates stream_id, expected_version, non-empty events, event_ids; delegates to writer; maps errors | Met | Validation order: parse_uuid (line 77), proto_to_expected_version (line 80), is_empty check (line 83), proto_to_proposed_event on each event (lines 88-92), writer.append (lines 95-99), error_to_status mapping (line 99). Response built from first/last recorded events (lines 102-109). |
| 3 | read_stream validates stream_id; delegates to read_index; maps StreamNotFound to NOT_FOUND | Met | parse_uuid at line 122, read_index.read_stream at lines 124-127, error_to_status maps StreamNotFound to NOT_FOUND (service.rs:196). Verified by integration test ac10_read_stream_not_found. |
| 4 | read_all delegates to read_index (infallible) | Met | Direct call to read_index.read_all at line 144, no error mapping needed, returns ReadAllResponse. Verified by ac14_read_all_empty_store. |
| 5 | Test AC-1: Append 1 event no_stream | Met | grpc_service.rs:72-90. Asserts first/last stream_version=0 and first/last global_position=0. |
| 6 | Test AC-2: Append 3 events batch | Met | grpc_service.rs:95-117. Asserts first_stream_version=0, last=2, first_global_position=0, last=2. |
| 7 | Test AC-3: Append no_stream twice -> FAILED_PRECONDITION | Met | grpc_service.rs:122-148. First append succeeds, second fails with FailedPrecondition. |
| 8 | Test AC-4: Invalid stream_id -> INVALID_ARGUMENT | Met | grpc_service.rs:153-166. Uses "not-a-uuid", asserts InvalidArgument. |
| 9 | Test AC-5: Empty events -> INVALID_ARGUMENT | Met | grpc_service.rs:171-184. Empty vec, asserts InvalidArgument. |
| 10 | Test AC-6: Invalid event_id -> INVALID_ARGUMENT | Met | grpc_service.rs:189-207. Uses "bad-uuid" for event_id, asserts InvalidArgument. |
| 11 | Test AC-7: Oversized payload -> INVALID_ARGUMENT | Met | grpc_service.rs:212-231. 65,537-byte payload exceeds MAX_EVENT_SIZE (65,536). EventTooLarge maps to InvalidArgument. |
| 12 | Test AC-8: ReadStream all 5 events in order | Met | grpc_service.rs:236-264. Appends 5, reads with max_count=100, verifies 5 events in stream_version order 0..4. |
| 13 | Test AC-9: ReadStream partial from version 2, max 2 | Met | grpc_service.rs:269-295. Verifies 2 events at versions 2 and 3. |
| 14 | Test AC-10: ReadStream non-existent -> NOT_FOUND | Met | grpc_service.rs:300-313. Random UUID, asserts NotFound. |
| 15 | Test AC-11: ReadStream invalid stream_id -> INVALID_ARGUMENT | Met | grpc_service.rs:318-331. "not-a-uuid", asserts InvalidArgument. |
| 16 | Test AC-12: ReadAll across 2 streams in global order | Met | grpc_service.rs:336-379. 3 to stream A, 2 to stream B. Verifies 5 events with global_position 0..4, confirms first 3 are stream A. |
| 17 | Test AC-13: ReadAll partial from position 3, max 2 | Met | grpc_service.rs:384-409. Verifies 2 events at positions 3 and 4. |
| 18 | Test AC-14: ReadAll empty store -> 0 events | Met | grpc_service.rs:414-427. No appends, read_all returns 0 events. |
| 19 | start_test_server helper defined | Met | grpc_service.rs:19-50. Returns `(EventStoreClient<Channel>, SocketAddr, TempDir)`. Binds `[::1]:0`, spawns server, connects client. |
| 20 | Quality gates pass | Met | All four gates verified: cargo build (0 warnings), clippy (0 warnings), fmt (clean), test (165 total: 148 unit + 2 broker + 14 gRPC + 1 writer). |

## Issues Found

### Critical (must fix before merge)

None.

### Major (should fix, risk of downstream problems)

None.

### Minor (nice to fix, not blocking)

1. **50ms sleep in start_test_server (grpc_service.rs:43):** The test helper uses a `tokio::time::sleep(50ms)` to wait for the server to start accepting connections. This is a pragmatic choice and works reliably in practice, but under heavy CI load it could theoretically be too short. A retry-loop on client connect (with exponential backoff and a timeout) would be more robust. Not blocking -- the 50ms is generous for a localhost bind and the tests pass reliably.

2. **`recorded[0]` index access without `.first()` (service.rs:102):** The code accesses `recorded[0]` and `recorded[recorded.len() - 1]` directly. This is safe by invariant (the events list is validated as non-empty before the writer call, and the writer returns exactly one RecordedEvent per ProposedEvent). However, using `.first().expect("writer returned empty recorded events")` would make the invariant explicit and match the project convention of `.expect()` for invariant violations. Stylistic only.

## Suggestions (non-blocking)

- The `SubscriptionStream` type alias doc comment (service.rs:55-58) references "Ticket 4" -- this is useful context for developers but will become stale after Ticket 4 is completed. Consider removing the ticket reference once subscriptions are implemented.

## Scope Check

- Files within scope: YES -- `src/service.rs` (modified with trait impl), `src/lib.rs` (added `pub mod service;` and `pub use service::EventfoldService;`), `tests/grpc_service.rs` (created with 14 tests), `Cargo.toml` (added `tokio-stream` dev-dep). All match the ticket's stated scope.
- Scope creep detected: NO -- The `lib.rs` diff also includes proto module and proto test from Ticket 1, and other Ticket 2 artifacts, but these are pre-existing uncommitted working tree changes from prior tickets, not added by this ticket.
- Unauthorized dependencies added: NO -- `tokio-stream = "0.1"` is the only new dependency, added as a dev-dependency as specified in the ticket scope.

## Risk Assessment

- Regression risk: LOW -- The new code adds an `impl EventStore` trait on `EventfoldService` and integration tests. No existing code was modified in ways that could break prior functionality. All 165 tests pass.
- Security concerns: NONE -- Input validation is thorough (UUID parsing, expected_version presence, non-empty events, event_id validation, size limits enforced by the store layer).
- Performance concerns: NONE -- The `.to_vec()` calls in `recorded_to_proto` (metadata, payload) perform copies from `Bytes` to `Vec<u8>`, which is necessary for the proto message type. The `events.iter().map(recorded_to_proto).collect()` pattern is standard and does not introduce unnecessary allocations.
