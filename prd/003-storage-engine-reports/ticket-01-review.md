# Code Review: Ticket 1 -- Store struct scaffold + `open()` (new file path)

**Ticket:** 1 -- Store struct scaffold + `open()` (new file path)
**Impl Report:** prd/003-storage-engine-reports/ticket-01-impl.md
**Date:** 2026-02-25 14:30
**Verdict:** APPROVED

---

## AC Coverage

| AC # | Description | Status | Notes |
|------|-------------|--------|-------|
| 1 | `Store` struct has fields `file: File`, `events: Vec<RecordedEvent>`, `streams: HashMap<Uuid, Vec<u64>>` and is `pub` | Met | Lines 30-39 of `src/store.rs`. All three fields present with correct types. Struct is `pub`. `file` has `#[allow(dead_code)]` scoped to the field -- acceptable since `append()` is Ticket 3. |
| 2 | `Store::open(path: &Path) -> Result<Store, Error>` creates file, writes header, fsyncs, returns empty Store | Met | Lines 61-72. `File::create(path)` creates the file, `write_all(&codec::encode_header())` writes the 8-byte header, `sync_all()` fsyncs. Returns `Store` with empty `events` and `streams`. Error propagation via `?` correctly converts `io::Error` to `Error::Io`. |
| 3 | `stream_version(&self, stream_id: &Uuid) -> Option<u64>` returns `None` on empty store | Met | Lines 87-91. `HashMap::get()` returns `None` for absent keys, which the `map()` correctly propagates. |
| 4 | `global_position(&self) -> u64` returns `events.len() as u64` | Met | Lines 101-103. Exact match to spec. |
| 5 | Test: `open()` on non-existent path creates file with correct header, `global_position()` returns 0 | Met | Test `open_creates_file_with_header_and_empty_store` (lines 111-129). Asserts file didn't exist, opens store, asserts file exists, reads file contents and checks first 8 bytes against `codec::encode_header()`, asserts `global_position() == 0`. Note: AC also mentions `read_all(0, 100)` returning an empty Vec, but that method does not exist yet (future ticket). Not a gap -- the method is out of scope. |
| 6 | Test: `global_position()` on fresh store returns 0 | Met | Test `global_position_on_empty_store_returns_zero` (lines 132-138). |
| 7 | Test: `stream_version()` on fresh store with random UUID returns `None` | Met | Test `stream_version_on_empty_store_returns_none` (lines 140-148). Uses `Uuid::new_v4()`. |
| 8 | Quality gates pass | Met | Verified independently: `cargo test` = 53 passed/0 failed, `cargo clippy --all-targets --all-features --locked -- -D warnings` = clean, `cargo fmt --check` = clean, `cargo build` = zero warnings. |

## Issues Found

### Critical (must fix before merge)

None.

### Major (should fix, risk of downstream problems)

None.

### Minor (nice to fix, not blocking)

1. **Doc comment overpromises on `open()`** (`src/store.rs` line 47): The doc says "If the file exists, validates the header and recovers events from the log (not yet implemented)." But the code unconditionally calls `File::create()` which truncates existing files. A caller reading the doc might believe existing files are handled (or at least safely rejected), when in reality calling `open()` on an existing file would silently destroy its contents. Suggest either removing the existing-file description from the doc comment until Ticket 2 implements it, or adding a clear `// TODO: Ticket 2` inline comment and a more explicit doc note like "Currently only handles new files; calling on an existing file will truncate it."

2. **Potential underflow in `stream_version`** (`src/store.rs` line 90): `positions.len() as u64 - 1` would underflow to `u64::MAX` if a stream somehow had an empty positions Vec in the map. This cannot happen in the current code (streams are only added during append with at least one position), but it is a latent defect that could surface if future code ever inserts an empty Vec. A safer alternative is `(positions.len() as u64).checked_sub(1)` or using `positions.len().checked_sub(1).map(|v| v as u64)` and flattening with the outer Option. Very low risk given the private field, but worth noting for defensive coding.

## Suggestions (non-blocking)

- The test `open_creates_file_with_header_and_empty_store` (line 125) uses `&contents[..8]` which would panic if the file is shorter than 8 bytes. This is acceptable in test code (test would fail with a clear panic), but `assert_eq!(contents.len(), 8)` before the slice comparison would make failure messages more descriptive.

## Scope Check

- Files within scope: YES -- `Cargo.toml`, `src/lib.rs`, `src/store.rs` are exactly the scoped files.
- Scope creep detected: NO
- Unauthorized dependencies added: NO -- `tracing = "0.1"` and `tempfile = "3"` are both specified in the ticket scope.

## Risk Assessment

- Regression risk: LOW -- This adds a new module and two new dependency entries. No existing code was changed beyond adding a `pub mod store;` declaration and a `pub use store::Store;` re-export in `lib.rs`. Pre-existing 50 tests still pass.
- Security concerns: NONE
- Performance concerns: NONE
