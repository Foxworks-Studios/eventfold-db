# Tickets for PRD 003: Storage Engine

**Source PRD:** prd/003-storage-engine.md
**Created:** 2026-02-25
**Total Tickets:** 5
**Estimated Total Complexity:** 12 (M=2 + L=3 + L=3 + M=2 + M=2)

---

### Ticket 1: Store Struct Scaffold, Cargo.toml Dependencies, and `open()` (New File Path)

**Description:**
Add `tracing` and `tempfile` to `Cargo.toml`, declare `pub mod store` in `lib.rs`, and create
`src/store.rs` with the `Store` struct definition plus the `open()` implementation for the
new-file-only path (create file, write header, fsync, return empty store). Also implement the
two trivial accessor methods `stream_version()` and `global_position()`.

**Scope:**
- Modify: `Cargo.toml` (add `tracing = "0.1"` to `[dependencies]`, `tempfile = "3"` to
  `[dev-dependencies]`)
- Modify: `src/lib.rs` (add `pub mod store;` and re-export `Store`)
- Create: `src/store.rs` (struct definition, `open()` for new-file path, `stream_version()`,
  `global_position()`)

**Acceptance Criteria:**
- [ ] `Store` struct has fields `file: File`, `events: Vec<RecordedEvent>`,
  `streams: HashMap<Uuid, Vec<u64>>` and is `pub` (not `pub(crate)`)
- [ ] `Store::open(path: &Path) -> Result<Store, Error>` — when `path` does not exist: creates
  the file, writes the 8-byte file header via `codec::encode_header()`, calls `file.sync_all()`,
  returns `Store` with empty `events` and `streams`
- [ ] `Store::stream_version(&self, stream_id: &Uuid) -> Option<u64>` returns `None` on empty
  store and is consistent with `streams.len() - 1` semantics (not yet tested with events)
- [ ] `Store::global_position(&self) -> u64` returns `events.len() as u64`
- [ ] Test: `open()` on a non-existent path in a `tempdir()` -> file exists on disk, first 8
  bytes equal `codec::encode_header()`, `global_position()` returns 0, `read_all(0, 100)`
  returns an empty `Vec`
- [ ] Test: `global_position()` on a freshly opened empty store returns 0
- [ ] Test: `stream_version()` on a freshly opened store with a random UUID returns `None`
- [ ] Quality gates pass: `cargo build`, `cargo clippy -- -D warnings`, `cargo fmt --check`,
  `cargo test`

**Dependencies:** PRD 001 (types, errors), PRD 002 (codec) — both already implemented
**Complexity:** M
**Maps to PRD AC:** AC-1, AC-15 (partial: empty-store queries)

---

### Ticket 2: Startup Recovery — Index Rebuild, Partial Trailing Truncation, Mid-File Corruption

**Description:**
Extend `Store::open()` to handle the existing-file path: validate the file header, read records
sequentially using `codec::decode_record()`, rebuild the in-memory index, detect and truncate a
partial trailing record (with a `tracing::warn!` log), and return `Error::CorruptRecord` for
mid-file corruption. All four recovery cases from the PRD must be covered by tests.

**Scope:**
- Modify: `src/store.rs` (extend `open()` with existing-file recovery branch)

**Acceptance Criteria:**
- [ ] When an existing file is opened: the 8-byte header is read and validated via
  `codec::decode_header()`; returns `Error::InvalidHeader` if magic or version is wrong
- [ ] Recovery loop calls `codec::decode_record()` on the remaining bytes, advancing the offset
  on each `DecodeOutcome::Complete`; stops on `DecodeOutcome::Incomplete` (partial trailing
  record) or `Error::CorruptRecord`
- [ ] On `DecodeOutcome::Complete`: pushes the event into `events`, pushes its `global_position`
  into `streams[stream_id]`, using `global_position` directly from the decoded event (not
  re-assigned)
- [ ] On trailing incomplete/corrupt record (at the tail of the file with no valid records after
  it): truncates the file to the last valid record boundary via `file.set_len()`, calls
  `file.sync_all()`, and logs a `tracing::warn!` message
- [ ] On mid-file corruption (a corrupt record with valid records after it): returns
  `Err(Error::CorruptRecord { position, detail })`
- [ ] Test: open a store, append 5 events across 2 streams (using raw `encode_record` writes +
  header for setup, or use `Store::open` + manual `append` once that is implemented — note: may
  need to use raw writes here since `append` is not yet implemented; alternatively, structure the
  test to use `Store` from Ticket 3 once available). **Implementer note:** since `append()` does
  not exist yet, seed the file by opening a fresh store in Ticket 1 and directly writing encoded
  records + header using `codec::encode_record` + `codec::encode_header` — then reopen. Verify
  all 5 events recovered, stream versions correct, global positions correct, and a subsequent
  open continues from the correct index.
- [ ] Test (AC-3): open a store, write 3 events by seeding the file, close; append 10 garbage
  bytes to the file's end using `std::fs::OpenOptions`; reopen — only 3 events recovered, file
  length equals the sum of the 3 record sizes + 8-byte header, no error returned
- [ ] Test (AC-4): open a store with 3 seeded events; flip the last byte of the 3rd record's
  payload (before the checksum) in the raw file; reopen — only 2 events recovered, 3rd record
  treated as corrupt trailing record, no error returned
- [ ] Test (AC-5): open a store with 3 seeded events; corrupt a byte in the 2nd record (not the
  last record); reopen — returns `Err(Error::CorruptRecord { .. })` because valid data follows the
  corruption
- [ ] Quality gates pass: `cargo build`, `cargo clippy -- -D warnings`, `cargo fmt --check`,
  `cargo test`

**Dependencies:** Ticket 1
**Complexity:** L
**Maps to PRD AC:** AC-2, AC-3, AC-4, AC-5

---

### Ticket 3: `append()` — ExpectedVersion Validation, Size Validation, Write, Fsync, Index Update

**Description:**
Implement `Store::append()` in full: validate `ExpectedVersion` against current stream state,
validate each proposed event's type length and total encoded size, construct `RecordedEvent`
values with assigned positions, serialize all records via `codec::encode_record()`, write and
fsync, update the in-memory index, and return the recorded events. Appends are atomic — nothing
is written if any validation fails.

**Scope:**
- Modify: `src/store.rs` (add `append()` method and its tests)

**Acceptance Criteria:**
- [ ] `append()` signature: `pub fn append(&mut self, stream_id: Uuid, expected_version:
  ExpectedVersion, proposed_events: Vec<ProposedEvent>) -> Result<Vec<RecordedEvent>, Error>`
- [ ] `ExpectedVersion::Any`: always passes regardless of whether stream exists or its version
- [ ] `ExpectedVersion::NoStream`: passes only if `streams` does not contain `stream_id`; returns
  `Err(Error::WrongExpectedVersion { .. })` if stream already exists
- [ ] `ExpectedVersion::Exact(n)`: passes only if stream exists and
  `streams[stream_id].len() - 1 == n`; returns `Err(Error::WrongExpectedVersion { .. })` if
  stream does not exist or version does not match
- [ ] Event type validation: empty event type returns `Err(Error::InvalidArgument(..))`;
  event type exceeding `MAX_EVENT_TYPE_LEN` bytes returns `Err(Error::InvalidArgument(..))`
- [ ] Event size validation: total encoded record size (via `codec::encode_record`) exceeding
  `MAX_EVENT_SIZE` returns `Err(Error::EventTooLarge { size, max })` — validation happens
  before any bytes are written
- [ ] On success: all records serialized with `codec::encode_record()`, written contiguously to
  the file, `file.sync_all()` called once after the batch, `events` and `streams` updated, and
  the returned `Vec<RecordedEvent>` has `global_position` and `stream_version` matching the
  index state after the append
- [ ] Test (AC-6): append one event with `ExpectedVersion::NoStream` to a new stream — returns
  `RecordedEvent` with `stream_version = 0`, `global_position = 0`; `read_all(0, 100)` contains
  it; `read_stream(stream_id, 0, 100)` contains it (using `read_stream` once implemented in
  Ticket 4 — for now, verify via `read_all`)
- [ ] Test (AC-7): append 3 events in one call — all 3 returned with `stream_version` 0, 1, 2
  and `global_position` 0, 1, 2 respectively
- [ ] Test (AC-8a): `ExpectedVersion::Any` on a non-existent stream — succeeds, creates stream
  at version 0
- [ ] Test (AC-8b): `ExpectedVersion::Any` on an existing stream at version 2 — succeeds,
  appended event has `stream_version = 3`
- [ ] Test (AC-9a): `ExpectedVersion::NoStream` on a non-existent stream — succeeds
- [ ] Test (AC-9b): `ExpectedVersion::NoStream` on an existing stream — returns
  `Err(Error::WrongExpectedVersion { .. })`
- [ ] Test (AC-10a): `ExpectedVersion::Exact(0)` after a single event (stream at version 0) —
  succeeds, new event has `stream_version = 1`
- [ ] Test (AC-10b): `ExpectedVersion::Exact(5)` when stream is at version 3 — returns
  `Err(Error::WrongExpectedVersion { .. })`
- [ ] Test (AC-10c): `ExpectedVersion::Exact(0)` on a non-existent stream — returns
  `Err(Error::WrongExpectedVersion { .. })`
- [ ] Test (AC-11a): event with payload making total encoded size exceed `MAX_EVENT_SIZE` returns
  `Err(Error::EventTooLarge { .. })`; no bytes written to file (file size unchanged)
- [ ] Test (AC-11b): event type of exactly `MAX_EVENT_TYPE_LEN + 1` bytes returns
  `Err(Error::InvalidArgument(..))`; no bytes written
- [ ] Test (AC-11c): event with empty event type returns `Err(Error::InvalidArgument(..))`
- [ ] Quality gates pass: `cargo build`, `cargo clippy -- -D warnings`, `cargo fmt --check`,
  `cargo test`

**Dependencies:** Ticket 1, Ticket 2 (for recovery-after-append tests to be meaningful)
**Complexity:** L
**Maps to PRD AC:** AC-6, AC-7, AC-8, AC-9, AC-10, AC-11

---

### Ticket 4: `read_stream()`, `read_all()`, `stream_version()` (full), and Multi-Stream Tests

**Description:**
Implement `Store::read_stream()` and `Store::read_all()`, and complete the `stream_version()`
tests that require events to be present. Write tests for all read-path edge cases (pagination,
out-of-range positions, non-existent stream) and multi-stream interleaving correctness.

**Scope:**
- Modify: `src/store.rs` (add `read_stream()`, `read_all()` methods and their tests; add
  remaining `stream_version()` / `global_position()` tests that require a populated store)

**Acceptance Criteria:**
- [ ] `read_stream()` signature: `pub fn read_stream(&self, stream_id: Uuid, from_version: u64,
  max_count: u64) -> Result<Vec<RecordedEvent>, Error>`
- [ ] `read_stream()` returns `Err(Error::StreamNotFound { stream_id })` if the stream does not
  exist in `streams`
- [ ] `read_stream()` slices the stream's position list from `from_version` to
  `min(from_version + max_count, stream_len)`, clones each event from `events` by global
  position, and returns them
- [ ] `read_all()` signature: `pub fn read_all(&self, from_position: u64, max_count: u64) ->
  Vec<RecordedEvent>`
- [ ] `read_all()` slices `events` from `from_position` to
  `min(from_position + max_count, events.len())` and returns cloned events; never errors
- [ ] Test (AC-12a): `read_stream` from version 0 with `max_count = 100` on a stream with 5
  events returns all 5 in stream version order
- [ ] Test (AC-12b): `read_stream` from version 2 with `max_count = 2` on a 5-event stream
  returns the 2 events at stream versions 2 and 3
- [ ] Test (AC-12c): `read_stream` from version 10 on a 5-event stream returns an empty `Vec`
  (not an error)
- [ ] Test (AC-12d): `read_stream` on a UUID that has never been appended to returns
  `Err(Error::StreamNotFound { .. })`
- [ ] Test (AC-13a): `read_all(0, 100)` on a 5-event log returns all 5 events in global position
  order
- [ ] Test (AC-13b): `read_all(3, 2)` on a 5-event log returns events at positions 3 and 4
- [ ] Test (AC-13c): `read_all(100, 10)` on a 5-event log returns an empty `Vec`
- [ ] Test (AC-13d): `read_all(0, 100)` on an empty log returns an empty `Vec`
- [ ] Test (AC-14): append events to 3 streams in interleaved order (stream A, stream B, stream
  A, stream C, stream B); assert `read_all(0, 100)` returns all 5 in global position order;
  assert `read_stream(A, 0, 100)` returns only A's events in stream version order; same for B
  and C
- [ ] Test (AC-15a): `stream_version()` on a stream with 3 appended events returns `Some(2)`
- [ ] Test (AC-15b): `global_position()` after 5 separate `append` calls (one event each)
  returns 5
- [ ] Quality gates pass: `cargo build`, `cargo clippy -- -D warnings`, `cargo fmt --check`,
  `cargo test`

**Dependencies:** Ticket 1, Ticket 2, Ticket 3
**Complexity:** M
**Maps to PRD AC:** AC-12, AC-13, AC-14, AC-15 (populated-store tests)

---

### Ticket 5: Verification and Integration

**Description:**
Run the full PRD 003 acceptance criteria checklist end-to-end. Verify that all tickets integrate
correctly: recovery after real appends (not just seeded files), round-trip persistence, and that
all quality gates pass clean across the entire crate.

**Acceptance Criteria:**
- [ ] AC-2 (recovery with real appends): open a store, use `Store::append()` to write 5 events
  across 2 streams, drop the store, reopen — all 5 events recovered, stream versions and global
  positions match, subsequent append continues from the correct positions
- [ ] AC-3 (truncation after real appends): use `Store::append()` to write 3 events, close,
  append 10 garbage bytes to the file, reopen — 3 events recovered, garbage truncated, next
  append succeeds at `global_position = 3`
- [ ] All 16 PRD acceptance criteria are covered by passing tests (verified by `cargo test`)
- [ ] No regressions in PRD 001 or PRD 002 tests (`cargo test` passes all)
- [ ] `cargo build` completes with zero warnings
- [ ] `cargo clippy --all-targets --all-features --locked -- -D warnings` passes with zero
  warnings
- [ ] `cargo fmt --check` passes
- [ ] `cargo test` — all tests green

**Dependencies:** Tickets 1, 2, 3, 4
**Complexity:** M
**Maps to PRD AC:** AC-16, plus integration verification of AC-2, AC-3

---

## AC Coverage Matrix

| PRD AC # | Description                                         | Covered By Ticket(s) | Status  |
|----------|-----------------------------------------------------|----------------------|---------|
| AC-1     | Open new store: file created, header written, empty index | Ticket 1        | Covered |
| AC-2     | Open existing store (recovery): 5 events, 2 streams | Ticket 2, Ticket 5   | Covered |
| AC-3     | Partial trailing record truncation                  | Ticket 2, Ticket 5   | Covered |
| AC-4     | CRC mismatch at tail treated as partial trailing    | Ticket 2             | Covered |
| AC-5     | Mid-file corruption is fatal (CorruptRecord)        | Ticket 2             | Covered |
| AC-6     | Append single event, correct positions              | Ticket 3             | Covered |
| AC-7     | Append batch of 3, contiguous versions/positions    | Ticket 3             | Covered |
| AC-8     | ExpectedVersion::Any (new and existing stream)      | Ticket 3             | Covered |
| AC-9     | ExpectedVersion::NoStream (success and failure)     | Ticket 3             | Covered |
| AC-10    | ExpectedVersion::Exact (success, wrong version, no stream) | Ticket 3      | Covered |
| AC-11    | Event size validation (too large, type too long, empty type) | Ticket 3    | Covered |
| AC-12    | ReadStream (all, paginated, beyond end, not found)  | Ticket 4             | Covered |
| AC-13    | ReadAll (all, paginated, beyond end, empty log)     | Ticket 4             | Covered |
| AC-14    | Multi-stream interleaving correctness               | Ticket 4             | Covered |
| AC-15    | stream_version and global_position queries          | Ticket 1, Ticket 4   | Covered |
| AC-16    | Build, lint, fmt, test all pass                     | Ticket 5             | Covered |
