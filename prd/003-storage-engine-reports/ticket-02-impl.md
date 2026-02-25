# Implementation Report: Ticket 2 -- Startup Recovery -- Index Rebuild, Partial Trailing Truncation, Mid-File Corruption

**Ticket:** 2 - Startup Recovery -- Index Rebuild, Partial Trailing Truncation, Mid-File Corruption
**Date:** 2026-02-25 15:30
**Status:** COMPLETE

---

## Files Changed

### Created
- None

### Modified
- `src/store.rs` - Extended `Store::open()` to handle existing files: header validation, sequential record recovery, trailing truncation with `tracing::warn!`, mid-file corruption detection. Added `has_valid_record_after()` helper. Added 7 new tests.

## Implementation Notes

- **Existing-file detection**: Changed `open()` from unconditionally creating a new file to first checking `path.exists()`. New files follow the original create-header-fsync path. Existing files follow the recovery path.
- **Recovery approach**: Reads the entire file into memory via `std::fs::read()`, validates the 8-byte header, then decodes records sequentially. This is efficient for the expected file sizes and avoids complex cursor management on the file handle.
- **Trailing vs. mid-file corruption**: When `decode_record()` returns `Incomplete` or `CorruptRecord`, the code calls `has_valid_record_after()` which scans forward byte-by-byte looking for any valid record. If found, the corruption is mid-file (fatal error). If not found, the corruption is trailing (truncate and continue).
- **File handle management**: After recovery, the file is reopened with `OpenOptions::new().read(true).write(true)` to support future `append()` operations (Ticket 3). The initial `std::fs::read()` closes its handle automatically.
- **Pattern followed**: Recovery logic lives entirely in `Store::open()` as specified. The `has_valid_record_after()` helper is a module-level private function, keeping `open()` readable.

## Acceptance Criteria

- [x] AC-1 (Header validation): Existing file header is read and validated via `codec::decode_header()`; returns `Error::InvalidHeader` for wrong magic, wrong version, or file too short. Tested by `recovery_rejects_invalid_header_magic`, `recovery_rejects_invalid_header_version`, `recovery_rejects_file_too_short_for_header`.
- [x] AC-2 (Recovery loop): `codec::decode_record()` called on remaining bytes, advancing offset on each `Complete`; stops on `Incomplete` or `CorruptRecord`. Events pushed into `events`, global positions pushed into `streams[stream_id]`. Tested by `recovery_rebuilds_index_from_5_events_across_2_streams`.
- [x] AC-3 (Complete handling): On `DecodeOutcome::Complete`, event pushed to `events`, `global_position` pushed to `streams[stream_id]`. Covered by AC-2 test.
- [x] AC-4 (Trailing truncation): On trailing incomplete/corrupt record, file truncated via `file.set_len()`, `file.sync_all()` called, `tracing::warn!` logged. Tested by `recovery_truncates_trailing_garbage_bytes` (10 garbage bytes) and `recovery_truncates_crc_corrupt_last_record` (CRC fail on last record).
- [x] AC-5 (Mid-file corruption): When corrupt record has valid records after it, returns `Err(Error::CorruptRecord { position, detail })`. Tested by `recovery_returns_error_on_mid_file_corruption`.
- [x] Test AC-2: 5 events across 2 streams seeded, reopened -- all 5 recovered with correct positions. PASS.
- [x] Test AC-3: 3 events + 10 garbage bytes -- only 3 recovered, file truncated, no error. PASS.
- [x] Test AC-4: 3 events, last byte of 3rd record flipped -- 2 recovered, file truncated. PASS.
- [x] Test AC-5: 3 events, byte in 2nd record flipped -- returns `Err(Error::CorruptRecord)`. PASS.
- [x] Quality gates pass.

## Test Results

- Lint: PASS (`cargo clippy --all-targets --all-features --locked -- -D warnings` -- zero warnings)
- Tests: PASS (60 total: 53 existing + 7 new, all green)
- Build: PASS (`cargo build` -- zero warnings)
- Fmt: PASS (`cargo fmt --check` -- clean)
- New tests added:
  - `src/store.rs::tests::recovery_rebuilds_index_from_5_events_across_2_streams` (AC-2)
  - `src/store.rs::tests::recovery_truncates_trailing_garbage_bytes` (AC-3)
  - `src/store.rs::tests::recovery_truncates_crc_corrupt_last_record` (AC-4)
  - `src/store.rs::tests::recovery_returns_error_on_mid_file_corruption` (AC-5)
  - `src/store.rs::tests::recovery_rejects_invalid_header_magic` (AC-1)
  - `src/store.rs::tests::recovery_rejects_invalid_header_version` (AC-1)
  - `src/store.rs::tests::recovery_rejects_file_too_short_for_header` (AC-1)

## Concerns / Blockers

- The `has_valid_record_after()` scan is O(n * m) where n is remaining bytes and m is the cost of `decode_record()` per probe. For typical files this is negligible, but for very large log files with corruption near the beginning, it could be slow. This is an acceptable tradeoff for correctness -- and the design doc does not specify performance constraints on recovery.
- The `#[allow(dead_code)]` on `Store::file` remains from Ticket 1. It will be removed when Ticket 3 adds `append()`.
