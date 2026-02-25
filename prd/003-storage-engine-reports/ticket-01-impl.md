# Implementation Report: Ticket 1 -- Store struct scaffold + `open()` (new file path)

**Ticket:** 1 - Store struct scaffold + `open()` (new file path)
**Date:** 2026-02-25 12:00
**Status:** COMPLETE

---

## Files Changed

### Created
- `src/store.rs` - Storage engine module with `Store` struct, `open()` (new-file path), `stream_version()`, `global_position()`, and 3 unit tests.

### Modified
- `Cargo.toml` - Added `tracing = "0.1"` to `[dependencies]`, `tempfile = "3"` to `[dev-dependencies]`.
- `src/lib.rs` - Added `pub mod store;` and `pub use store::Store;` re-export.

## Implementation Notes
- `Store::open()` currently only handles the new-file path (file does not exist). It uses `File::create()` which always creates/truncates. The existing-file recovery path will be added by Ticket 2.
- The `file` field has `#[allow(dead_code)]` because it is stored for use by `append()` (Ticket 3) but not yet read by any method in this ticket. The suppression is scoped to just that field.
- `stream_version()` maps the stream's position list length to a zero-based version via `len() - 1`. Since `HashMap::get()` returns `None` for missing streams, this correctly returns `None` for non-existent streams without needing a branch.
- `global_position()` returns `events.len() as u64`, matching the PRD specification of "next global position."
- Followed existing codebase patterns: `thiserror` error propagation via `?`, `tempfile::tempdir()` for test isolation, `#[cfg(test)] mod tests { use super::*; }` for co-located tests.

## Acceptance Criteria
- [x] AC 1: `Store` struct has fields `file: File`, `events: Vec<RecordedEvent>`, `streams: HashMap<Uuid, Vec<u64>>` and is `pub` - Defined in `src/store.rs` lines 30-39. All fields present with correct types. Struct is `pub`.
- [x] AC 2: `Store::open(path: &Path) -> Result<Store, Error>` creates file, writes 8-byte header, fsyncs, returns empty Store - Implemented in lines 61-72. Uses `File::create()`, `write_all(&codec::encode_header())`, `sync_all()`.
- [x] AC 3: `Store::stream_version(&self, stream_id: &Uuid) -> Option<u64>` returns `None` on empty store - Implemented in lines 87-91. Returns `None` via `HashMap::get()` when stream is absent.
- [x] AC 4: `Store::global_position(&self) -> u64` returns `events.len() as u64` - Implemented in lines 101-103.
- [x] AC 5: Test: `open()` on non-existent path creates file with correct header, `global_position()` returns 0 - Test `open_creates_file_with_header_and_empty_store` (line 111). Verified empty store via `global_position() == 0` as instructed (no `read_all` call).
- [x] AC 6: Test: `global_position()` on freshly opened empty store returns 0 - Test `global_position_on_empty_store_returns_zero` (line 132).
- [x] AC 7: Test: `stream_version()` with random UUID returns `None` - Test `stream_version_on_empty_store_returns_none` (line 140).
- [x] AC 8: Quality gates pass - All four gates confirmed (see test results below).

## Test Results
- Build: PASS (`cargo build` -- zero warnings)
- Lint: PASS (`cargo clippy --all-targets --all-features --locked -- -D warnings` -- clean)
- Fmt: PASS (`cargo fmt --check` -- no issues)
- Tests: PASS (`cargo test` -- 53 passed, 0 failed; 50 pre-existing + 3 new)
- New tests added:
  - `src/store.rs::tests::open_creates_file_with_header_and_empty_store`
  - `src/store.rs::tests::global_position_on_empty_store_returns_zero`
  - `src/store.rs::tests::stream_version_on_empty_store_returns_none`

## Concerns / Blockers
- The `#[allow(dead_code)]` on `file` field should be removed once Ticket 3 (append) is implemented and reads from the field. Downstream implementer should clean this up.
- `Store::open()` currently always creates a new file (via `File::create`). Ticket 2 must add the existing-file recovery path with `OpenOptions::new().read(true).write(true).open()` logic.
