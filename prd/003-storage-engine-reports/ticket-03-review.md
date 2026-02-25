# Code Review: Ticket 3 -- `append()` -- ExpectedVersion Validation, Size Validation, Write, Fsync, Index Update

**Ticket:** 3 -- `append()` -- ExpectedVersion Validation, Size Validation, Write, Fsync, Index Update
**Impl Report:** prd/003-storage-engine-reports/ticket-03-impl.md
**Date:** 2026-02-25 18:45
**Verdict:** APPROVED

---

## AC Coverage

| AC # | Description | Status | Notes |
|------|-------------|--------|-------|
| Signature | `append()` matches PRD signature | Met | Line 248-253 of `src/store.rs`: `pub fn append(&mut self, stream_id: Uuid, expected_version: ExpectedVersion, proposed_events: Vec<ProposedEvent>) -> Result<Vec<RecordedEvent>, Error>`. Exact match to PRD. |
| Any | `ExpectedVersion::Any` always passes | Met | Line 257: empty match arm `ExpectedVersion::Any => {}`. Tested by `append_any_on_nonexistent_stream_succeeds` and `append_any_on_existing_stream_appends_at_correct_version`. |
| NoStream | Rejects existing streams | Met | Lines 258-266: checks `if let Some(positions) = stream_positions`, returns `WrongExpectedVersion` with correct `expected`/`actual` fields. Tested by `append_no_stream_on_existing_stream_returns_error`. |
| Exact(n) | Validates stream exists and version matches | Met | Lines 267-283: `None` -> error (stream doesn't exist), `Some(positions)` -> computes `positions.len() as u64 - 1` and compares against `n`. Both failure modes tested by AC-10b and AC-10c. |
| Empty event type | Returns `InvalidArgument` | Met | Lines 295-298: `if proposed.event_type.is_empty()` returns `Err(Error::InvalidArgument(...))`. Tested by `append_empty_event_type_returns_invalid_argument`. |
| Event type > MAX_EVENT_TYPE_LEN | Returns `InvalidArgument` | Met | Lines 301-307: `if proposed.event_type.len() > MAX_EVENT_TYPE_LEN`. Tested by `append_event_type_too_long_returns_invalid_argument`. |
| Oversized record | Returns `EventTooLarge`, file unchanged | Met | Lines 320-326: encodes via `codec::encode_record()`, checks `encoded.len() > MAX_EVENT_SIZE`. Validation happens in the same loop before any writes. Tested by `append_oversized_payload_returns_event_too_large` which also asserts file size is unchanged. |
| On success | Write + fsync + index update + correct positions | Met | Lines 334-347: `seek(End(0))`, `write_all`, `sync_all()` once, then `streams.entry(...).or_default()` + push global positions, `events.extend(recorded.clone())`. Returned events have correct `global_position` and `stream_version` verified in multiple tests. |
| Test AC-6 | Append single event to new stream | Met | `append_single_event_no_stream` (line 633): verifies `stream_version=0`, `global_position=0`, `stream_id` match, `event_type` match, `global_position()=1`, `stream_version()=Some(0)`. Note: does not call `read_all` as AC suggests, but `read_all` is not implemented until Ticket 4. Verification via returned events and index state is sufficient for this ticket. |
| Test AC-7 | Append batch of 3 events | Met | `append_batch_three_events` (line 656): verifies contiguous `stream_version` (0,1,2) and `global_position` (0,1,2) for all 3 events, plus `global_position()=3` and `stream_version()=Some(2)`. |
| Test AC-8a | Any on non-existent stream | Met | `append_any_on_nonexistent_stream_succeeds` (line 690): verifies `stream_version=0`, `global_position=0`. |
| Test AC-8b | Any on existing stream at version 2 | Met | `append_any_on_existing_stream_appends_at_correct_version` (line 709): first batch of 3 events, verifies `stream_version=Some(2)`, then second append with `Any` yields `stream_version=3`, `global_position=3`. |
| Test AC-9a | NoStream on non-existent stream | Met | `append_no_stream_on_nonexistent_stream_succeeds` (line 741): verifies `stream_version=0`. |
| Test AC-9b | NoStream on existing stream | Met | `append_no_stream_on_existing_stream_returns_error` (line 759): creates stream, then tries `NoStream` again, asserts `Error::WrongExpectedVersion`. |
| Test AC-10a | Exact(0) after single event | Met | `append_exact_version_matches_succeeds` (line 783): appends one event, then Exact(0), verifies new event at `stream_version=1`. |
| Test AC-10b | Exact(5) when stream at version 3 | Met | `append_exact_version_mismatch_returns_error` (line 808): appends 4 events (version 3), tries `Exact(5)`, asserts `Error::WrongExpectedVersion`. |
| Test AC-10c | Exact(0) on non-existent stream | Met | `append_exact_on_nonexistent_stream_returns_error` (line 839): tries `Exact(0)` on fresh store, asserts `Error::WrongExpectedVersion`. |
| Test AC-11a | Oversized payload -> EventTooLarge, file unchanged | Met | `append_oversized_payload_returns_event_too_large` (line 856): creates `MAX_EVENT_SIZE`-byte payload (which will exceed limit once fixed fields are added), asserts `Error::EventTooLarge`, asserts file size unchanged. |
| Test AC-11b | Event type > MAX_EVENT_TYPE_LEN -> InvalidArgument | Met | `append_event_type_too_long_returns_invalid_argument` (line 890): uses `"A".repeat(MAX_EVENT_TYPE_LEN + 1)`, asserts `Error::InvalidArgument` with message containing "event type". |
| Test AC-11c | Empty event type -> InvalidArgument | Met | `append_empty_event_type_returns_invalid_argument` (line 914): uses empty string event type, asserts `Error::InvalidArgument` with message containing "event type". |
| Quality gates | cargo build, clippy, fmt, test all pass | Met | Independently verified: 72 tests pass (60 existing + 12 new), clippy clean, fmt clean, zero build warnings. |

## Issues Found

### Critical (must fix before merge)

None.

### Major (should fix, risk of downstream problems)

None.

### Minor (nice to fix, not blocking)

1. **Inconsistent `Seek` import location (line 335):** `std::io::Write` is imported at the file top (line 9), but `std::io::Seek` is imported inline within the `append()` method body at line 335. Both are used on `self.file`. Moving the `Seek` import to the top-level imports alongside `Write` would be more consistent.

2. **AC-6 test does not call `read_all` as specified in the AC:** The AC text says to "verify via `read_all`," but `read_all` is not implemented until Ticket 4. The test correctly verifies via the returned events and index state, which is the best available verification. Ticket 5's integration tests will cover the full round-trip. Not a correctness gap, just a deviation from the AC's literal wording due to ticket ordering.

## Suggestions (non-blocking)

- **Empty `proposed_events` input:** If `append()` is called with an empty `Vec<ProposedEvent>`, it will seek to end of file, call `write_all` with an empty buffer, call `sync_all()`, and return `Ok(vec![])`. This is a benign no-op but wastes a syscall (fsync). An early return for empty input would be a micro-optimization. Not important for correctness.

- **Clone ordering on line 345:** `self.events.extend(recorded.clone())` followed by `Ok(recorded)` clones the vector for the index and returns the originals. An alternative is `self.events.extend_from_slice(&recorded)` which is semantically identical but slightly more idiomatic when extending from a slice reference. Either way, one clone per event is unavoidable given that both the index and the return value need ownership. No action needed.

## Scope Check

- Files within scope: YES -- only `src/store.rs` was modified, which is the sole file listed in the ticket scope.
- Scope creep detected: NO -- all changes serve the ticket's acceptance criteria. The 12 new tests map 1:1 to the required ACs. The `OpenOptions` fix in `open()` was necessary to support `append()` writing to the file handle (noted as a concern in the Ticket 2 review). The `make_proposed` helper is minimal test infrastructure.
- Unauthorized dependencies added: NO -- no changes to `Cargo.toml`.

## Risk Assessment

- Regression risk: LOW -- The only change to existing code is the `open()` new-file path switching from `File::create()` to `OpenOptions::new().read(true).write(true).create(true).truncate(true)`. This is a strictly more capable file handle (adds read permission). All 60 pre-existing tests continue to pass, confirming no regression. The `append()` method is purely additive.
- Security concerns: NONE -- Store operates on local files with no network exposure at this layer.
- Performance concerns: NONE -- The `encode_record` call during validation creates a temporary Vec per event (for size checking), which is then accumulated into `encoded_batch`. This is one allocation per event in the batch plus one clone of the result. For the expected workload (small events, reasonable batch sizes), this is negligible. The single `sync_all()` per batch is the dominant cost, which is correct per the design doc.
