# Implementation Report: Ticket 3 -- `append()` -- ExpectedVersion Validation, Size Validation, Write, Fsync, Index Update

**Ticket:** 3 - `append()` -- ExpectedVersion Validation, Size Validation, Write, Fsync, Index Update
**Date:** 2026-02-25 12:00
**Status:** COMPLETE

---

## Files Changed

### Modified
- `src/store.rs` - Added `append()` method with full optimistic concurrency, event validation, serialization, fsync, and index update. Added 12 new tests. Fixed `File::create` in `open()` to use `OpenOptions` with `read(true).write(true).create(true).truncate(true)` so the file handle supports appending. Removed `#[allow(dead_code)]` on the `file` field. Added imports for `ExpectedVersion`, `ProposedEvent`, `MAX_EVENT_SIZE`, `MAX_EVENT_TYPE_LEN`.

## Implementation Notes
- The new-file path in `open()` was changed from `File::create()` (write-only) to `OpenOptions::new().read(true).write(true).create(true).truncate(true)` so the file handle supports both reading and writing/appending. The `.truncate(true)` is required by clippy's `suspicious_open_options` lint when `.create(true)` is used without `.append(true)`.
- Event size validation encodes each `RecordedEvent` via `codec::encode_record()` and checks the encoded length against `MAX_EVENT_SIZE`. All validation (version, type, size) happens before any bytes are written to disk, ensuring atomicity.
- The append uses `Seek::seek(SeekFrom::End(0))` before `write_all()` to position the file cursor at the end. This is necessary because the file is opened with `read(true).write(true)` (not `.append(true)`) and the cursor position may not be at the end after `open()`.
- `sync_all()` is called once after the entire batch is written, satisfying the fsync-per-batch requirement from the design doc.
- The `recorded` vector is cloned when extending `self.events` because the same events are both stored in the index and returned to the caller. This is the minimal-clone approach given the ownership model.

## Acceptance Criteria
- [x] AC (signature): `pub fn append(&mut self, stream_id: Uuid, expected_version: ExpectedVersion, proposed_events: Vec<ProposedEvent>) -> Result<Vec<RecordedEvent>, Error>` - exact match
- [x] AC (Any): always passes regardless of stream state - line 257, no-op match arm
- [x] AC (NoStream): passes only if stream doesn't exist; returns `WrongExpectedVersion` if stream exists - lines 258-266
- [x] AC (Exact): passes only if stream exists and version matches; returns `WrongExpectedVersion` otherwise - lines 267-283
- [x] AC (event type validation): empty -> `InvalidArgument`, too long -> `InvalidArgument` - lines 295-307
- [x] AC (event size validation): total encoded size > `MAX_EVENT_SIZE` -> `EventTooLarge`; all validation before any writes - lines 320-326
- [x] AC (on success): records serialized, written contiguously, `sync_all()` once, index updated, correct positions returned - lines 334-347
- [x] Test AC-6: `append_single_event_no_stream` - stream_version=0, global_position=0
- [x] Test AC-7: `append_batch_three_events` - versions 0,1,2 and positions 0,1,2
- [x] Test AC-8a: `append_any_on_nonexistent_stream_succeeds` - Any on new stream works
- [x] Test AC-8b: `append_any_on_existing_stream_appends_at_correct_version` - Any on existing stream at version 2 yields stream_version=3
- [x] Test AC-9a: `append_no_stream_on_nonexistent_stream_succeeds` - NoStream on new stream works
- [x] Test AC-9b: `append_no_stream_on_existing_stream_returns_error` - NoStream on existing stream returns WrongExpectedVersion
- [x] Test AC-10a: `append_exact_version_matches_succeeds` - Exact(0) after single event succeeds, new event at version 1
- [x] Test AC-10b: `append_exact_version_mismatch_returns_error` - Exact(5) when stream at version 3 returns WrongExpectedVersion
- [x] Test AC-10c: `append_exact_on_nonexistent_stream_returns_error` - Exact(0) on non-existent stream returns WrongExpectedVersion
- [x] Test AC-11a: `append_oversized_payload_returns_event_too_large` - oversized payload returns EventTooLarge, file size unchanged
- [x] Test AC-11b: `append_event_type_too_long_returns_invalid_argument` - event type of MAX_EVENT_TYPE_LEN+1 bytes returns InvalidArgument
- [x] Test AC-11c: `append_empty_event_type_returns_invalid_argument` - empty event type returns InvalidArgument
- [x] Quality gates pass

## Test Results
- Lint: PASS (`cargo clippy --all-targets --all-features --locked -- -D warnings` clean)
- Tests: PASS (72 tests: 60 existing + 12 new, all green)
- Build: PASS (`cargo build` zero warnings)
- Fmt: PASS (`cargo fmt --check` clean)
- New tests added:
  - `src/store.rs::tests::append_single_event_no_stream` (AC-6)
  - `src/store.rs::tests::append_batch_three_events` (AC-7)
  - `src/store.rs::tests::append_any_on_nonexistent_stream_succeeds` (AC-8a)
  - `src/store.rs::tests::append_any_on_existing_stream_appends_at_correct_version` (AC-8b)
  - `src/store.rs::tests::append_no_stream_on_nonexistent_stream_succeeds` (AC-9a)
  - `src/store.rs::tests::append_no_stream_on_existing_stream_returns_error` (AC-9b)
  - `src/store.rs::tests::append_exact_version_matches_succeeds` (AC-10a)
  - `src/store.rs::tests::append_exact_version_mismatch_returns_error` (AC-10b)
  - `src/store.rs::tests::append_exact_on_nonexistent_stream_returns_error` (AC-10c)
  - `src/store.rs::tests::append_oversized_payload_returns_event_too_large` (AC-11a)
  - `src/store.rs::tests::append_event_type_too_long_returns_invalid_argument` (AC-11b)
  - `src/store.rs::tests::append_empty_event_type_returns_invalid_argument` (AC-11c)

## Concerns / Blockers
- None
