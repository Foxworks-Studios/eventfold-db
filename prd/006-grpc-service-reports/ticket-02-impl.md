# Implementation Report: Ticket 2 -- EventfoldService Struct, Error Mapping, and Request Validation Helpers

**Ticket:** 2 - EventfoldService Struct, Error Mapping, and Request Validation Helpers
**Date:** 2026-02-26 14:30
**Status:** COMPLETE

---

## Files Changed

### Created
- `src/service.rs` - gRPC service struct, error-to-status mapping, and proto/domain conversion helpers with 16 unit tests

### Modified
- `src/lib.rs` - Added `pub mod service;` declaration (line 11)

## Implementation Notes
- All functions are `pub` free functions (not methods on `EventfoldService`) since subsequent tickets need them as standalone helpers for RPC handlers.
- `EventfoldService` has public fields (`pub writer`, `pub read_index`, `pub broker`) to allow RPC handler methods to access them directly.
- The module uses `#![allow(clippy::result_large_err)]` because `tonic::Status` is 176 bytes, which triggers the lint on every function returning `Result<T, tonic::Status>`. This is inherent to tonic's API and the standard suppression pattern for gRPC service modules.
- `parse_uuid` includes the field name in the error message for debuggability (e.g., "invalid stream_id: ...").
- `proto_to_expected_version` handles both the outer `None` (field not set) and inner `kind: None` (oneof not set) cases.
- `recorded_to_proto` converts `Bytes` to `Vec<u8>` via `.to_vec()` and UUIDs to hyphenated lowercase strings via `.to_string()`.
- `proto_to_proposed_event` converts `Vec<u8>` to `Bytes` via `Bytes::from()` (zero-copy when possible).

## Acceptance Criteria
- [x] AC 1: `EventfoldService` is a `pub struct` with fields `writer: WriterHandle`, `read_index: ReadIndex`, `broker: Broker` -- defined at line 29-36
- [x] AC 2: `EventfoldService::new(writer, read_index, broker) -> Self` -- implemented at line 46-52
- [x] AC 3: `fn error_to_status(err: Error) -> tonic::Status` maps all 7 Error variants correctly -- implemented at line 70-81 with exhaustive match
- [x] AC 4: `fn parse_uuid(s: &str, field_name: &str) -> Result<Uuid, tonic::Status>` -- implemented at line 98-101
- [x] AC 5: `fn proto_to_expected_version(ev: Option<proto::ExpectedVersion>) -> Result<ExpectedVersion, tonic::Status>` -- implemented at line 120-132
- [x] AC 6: `fn proto_to_proposed_event(p: proto::ProposedEvent) -> Result<ProposedEvent, tonic::Status>` -- implemented at line 175-183
- [x] AC 7: `fn recorded_to_proto(e: &RecordedEvent) -> proto::RecordedEvent` -- implemented at line 146-156
- [x] AC 8: Tests for all error_to_status mappings (7 tests) -- lines 192-258
- [x] AC 9: Tests for parse_uuid (valid + invalid) -- lines 263-279
- [x] AC 10: Tests for proto_to_expected_version (None, any, no_stream, exact) -- lines 283-315
- [x] AC 11: Tests for proto_to_proposed_event (invalid UUID, valid) -- lines 320-351
- [x] AC 12: Test for recorded_to_proto round-trip -- lines 356-378
- [x] AC 13: Quality gates pass -- all four checks pass clean

## Test Results
- Lint: PASS (`cargo clippy --all-targets --all-features --locked -- -D warnings`)
- Tests: PASS (148 lib tests + 3 integration tests = 151 total, including 16 new service tests)
- Build: PASS (zero warnings)
- Format: PASS (`cargo fmt --check`)
- New tests added:
  - `src/service.rs::tests::error_to_status_wrong_expected_version`
  - `src/service.rs::tests::error_to_status_stream_not_found`
  - `src/service.rs::tests::error_to_status_io`
  - `src/service.rs::tests::error_to_status_corrupt_record`
  - `src/service.rs::tests::error_to_status_invalid_header`
  - `src/service.rs::tests::error_to_status_event_too_large`
  - `src/service.rs::tests::error_to_status_invalid_argument`
  - `src/service.rs::tests::parse_uuid_valid`
  - `src/service.rs::tests::parse_uuid_invalid`
  - `src/service.rs::tests::proto_to_expected_version_none`
  - `src/service.rs::tests::proto_to_expected_version_any`
  - `src/service.rs::tests::proto_to_expected_version_no_stream`
  - `src/service.rs::tests::proto_to_expected_version_exact`
  - `src/service.rs::tests::proto_to_proposed_event_invalid_uuid`
  - `src/service.rs::tests::proto_to_proposed_event_valid`
  - `src/service.rs::tests::recorded_to_proto_round_trip`

## Concerns / Blockers
- None
