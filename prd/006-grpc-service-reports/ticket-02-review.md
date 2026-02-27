# Code Review: Ticket 2 -- EventfoldService Struct, Error Mapping, and Request Validation Helpers

**Ticket:** 2 -- EventfoldService Struct, Error Mapping, and Request Validation Helpers
**Impl Report:** prd/006-grpc-service-reports/ticket-02-impl.md
**Date:** 2026-02-26 15:45
**Verdict:** APPROVED

---

## AC Coverage

| AC # | Description | Status | Notes |
|------|-------------|--------|-------|
| 1 | `EventfoldService` pub struct with `writer: WriterHandle`, `read_index: ReadIndex`, `broker: Broker` | Met | Lines 29-36 of `src/service.rs`. All three fields are `pub` and use the correct types from `crate::writer::WriterHandle`, `crate::reader::ReadIndex`, `crate::broker::Broker`. |
| 2 | `EventfoldService::new(writer, read_index, broker) -> Self` | Met | Lines 46-52. Standard constructor, doc comments present. |
| 3 | `error_to_status` maps all 7 Error variants to correct gRPC codes | Met | Lines 70-81. Exhaustive match with no wildcard arm. Verified each mapping: WrongExpectedVersion->FailedPrecondition, StreamNotFound->NotFound, Io->Internal, CorruptRecord->DataLoss, InvalidHeader->DataLoss, EventTooLarge->InvalidArgument, InvalidArgument->InvalidArgument. Status message uses `err.to_string()`. |
| 4 | `parse_uuid` validates UUID strings, returns INVALID_ARGUMENT with field name | Met | Lines 98-101. Includes field name in error message via format string. |
| 5 | `proto_to_expected_version` converts proto oneof to domain enum | Met | Lines 120-132. Handles outer `None`, inner `kind: None`, and all three `Kind` variants (Any, NoStream, Exact). Returns INVALID_ARGUMENT for missing fields. |
| 6 | `proto_to_proposed_event` converts proto to domain with UUID validation | Met | Lines 175-183. Delegates UUID parsing to `parse_uuid`, converts `Vec<u8>` to `Bytes` via `Bytes::from()`. |
| 7 | `recorded_to_proto` converts domain to proto | Met | Lines 146-156. UUIDs serialized via `.to_string()` (hyphenated lowercase). `Bytes` converted to `Vec<u8>` via `.to_vec()`. All 7 proto fields mapped. |
| 8 | Tests for all 7 error_to_status mappings | Met | Lines 192-258. Seven tests, one per variant. Each asserts correct `Code` and verifies the message contains relevant content (e.g., the variant's `Display` output). |
| 9 | Tests for parse_uuid (valid + invalid) | Met | Lines 263-279. Valid test round-trips a `Uuid::new_v4()`. Invalid test checks `Code::InvalidArgument` and verifies field name appears in message. |
| 10 | Tests for proto_to_expected_version (None, any, no_stream, exact) | Met | Lines 283-315. Four tests covering all branches. |
| 11 | Tests for proto_to_proposed_event (invalid UUID, valid) | Met | Lines 320-351. Invalid UUID test verifies `Code::InvalidArgument` and "event_id" in message. Valid test checks all four fields. |
| 12 | Test for recorded_to_proto round-trip | Met | Lines 356-378. Constructs a domain `RecordedEvent`, converts to proto, asserts all 7 fields match. |
| 13 | Quality gates pass | Met | Verified: `cargo build` (0 warnings), `cargo clippy --all-targets --all-features --locked -- -D warnings` (clean), `cargo test` (151 total, 0 failures), `cargo fmt --check` (clean). |

## Issues Found

### Critical (must fix before merge)
- None.

### Major (should fix, risk of downstream problems)
- None.

### Minor (nice to fix, not blocking)
- **`#![allow(clippy::result_large_err)]` scope** (`src/service.rs`, line 10): This is an inner attribute that suppresses the lint for the entire module. The justification (tonic::Status is 176 bytes) is valid and well-documented in the comment at lines 7-9. However, this also suppresses the lint for any future non-tonic functions added to this module. A per-function `#[allow(...)]` would be more precise, but given that this is a gRPC service module where essentially all functions will return `Result<T, tonic::Status>`, module-level suppression is pragmatic and acceptable.
- **`.unwrap()` in tests** (`src/service.rs`, lines 266, 272, etc.): Test code uses `.unwrap()` which is fine per CLAUDE.md conventions (the no-unwrap rule applies to library code, not tests). Just noting for completeness.

## Suggestions (non-blocking)
- The `error_to_status` function takes ownership of `Error` (`err: Error`) rather than a reference. This is fine since it calls `err.to_string()` and then pattern-matches to construct the status -- the error is consumed. If callers ever need to inspect the error after mapping, a `&Error` signature would be preferable, but for the gRPC service pattern (map-and-return), ownership is correct.
- The `recorded_to_proto` function clones `event_type` via `.clone()` (line 152) and copies `metadata`/`payload` via `.to_vec()` (lines 153-154). This is necessary because the proto types require owned `String` and `Vec<u8>` respectively. No optimization possible here given the proto-generated type requirements.

## Scope Check
- Files within scope: YES -- `src/service.rs` (created) and `src/lib.rs` (modified with `pub mod service;` at line 11).
- Scope creep detected: NO. The `lib.rs` diff also contains changes from Ticket 1 (proto module declaration, `proto_append_request_default` test), but these are layered working-tree changes from the prior ticket, not scope creep from this ticket. The impl report correctly claims only the `pub mod service;` addition.
- Unauthorized dependencies added: NO. No changes to `Cargo.toml` from this ticket (Cargo.toml changes in the working tree belong to Ticket 1).

## Risk Assessment
- Regression risk: LOW. The 16 new unit tests cover all conversion functions. Full test suite (151 tests) passes with 0 failures. No existing code was modified beyond adding the module declaration.
- Security concerns: NONE. UUID parsing uses the standard `uuid` crate parser. No user input reaches unsafe operations.
- Performance concerns: NONE. All conversions are O(1) per event. `Bytes::from(Vec<u8>)` is zero-copy (takes ownership of the Vec's allocation). `Bytes::to_vec()` does copy, but is unavoidable given proto type requirements.
