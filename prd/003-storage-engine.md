# PRD 003: Storage Engine

**Status:** TICKETS READY

## Summary

Implement the core storage engine: the in-memory index, log file management, startup recovery, append operations with optimistic concurrency, and read operations (ReadStream, ReadAll). This is the heart of EventfoldDB -- the stateful component that owns event data.

## Motivation

The storage engine is where durability, correctness, and concurrency guarantees are enforced. It must:
- Recover the full in-memory index from the on-disk log on startup.
- Detect and truncate partial trailing records left by crashes.
- Validate optimistic concurrency (ExpectedVersion) on every append.
- Provide efficient reads by stream and globally via direct indexing.

## Scope

### In scope

- `store.rs`: The `Store` struct with methods for opening a log file, recovering state, appending events, and reading events.
- In-memory index: `Vec<RecordedEvent>` (global log) + `HashMap<Uuid, Vec<u64>>` (stream index).
- Startup recovery: read all records from disk, build index, truncate partial trailing record if found.
- Append: validate `ExpectedVersion`, serialize records, write to file, fsync, update index. Returns the recorded events.
- ReadStream: look up stream, return events from a given version with a max count.
- ReadAll: return events from a given global position with a max count.
- Event size validation on append (64 KB limit, 256-byte event type limit).

### Out of scope

- Async writer task (PRD 004) -- the store's append method is synchronous. The writer task wraps it.
- Broadcast notifications (PRD 005).
- gRPC layer (PRD 006).
- Concurrent read access patterns (PRD 004 adds the `Arc`/`RwLock` layer).

## Detailed Design

### `Store` struct

```rust
pub struct Store {
    /// Append-only log file handle.
    file: File,
    /// Global event log. Index i = event at global position i.
    events: Vec<RecordedEvent>,
    /// Stream index. Maps stream ID to list of global positions.
    /// Index j in the vec = event at stream version j.
    streams: HashMap<Uuid, Vec<u64>>,
}
```

### `Store::open(path: &Path) -> Result<Store, Error>`

Opens or creates the log file at `path`.

- If the file does not exist: create it, write the file header, fsync, return an empty store.
- If the file exists: validate the header, then read records sequentially, building the in-memory index.
- If a partial trailing record is detected (not enough bytes or CRC mismatch at the very end of the file): truncate the file to the end of the last valid record and log a warning via `tracing::warn!`.
- If a corrupt record is found before the end of the file (i.e., there are valid records after it): return `Error::CorruptRecord`. Mid-file corruption is unrecoverable.

### `Store::append`

```rust
pub fn append(
    &mut self,
    stream_id: Uuid,
    expected_version: ExpectedVersion,
    proposed_events: Vec<ProposedEvent>,
) -> Result<Vec<RecordedEvent>, Error>
```

1. Validate `expected_version` against current stream state:
   - `Any`: always passes.
   - `NoStream`: fails if the stream exists (has any events).
   - `Exact(n)`: fails if the stream does not exist or its current version != n. Current version = `stream_positions.len() - 1` (zero-based).
2. Validate each proposed event:
   - Event type must be non-empty and at most `MAX_EVENT_TYPE_LEN` bytes.
   - Total record size must not exceed `MAX_EVENT_SIZE`.
3. For each proposed event, construct a `RecordedEvent` with:
   - `global_position` = current `events.len()`.
   - `stream_version` = current stream length (0 for new stream, or last version + 1).
   - All other fields copied from the proposed event.
4. Serialize all new records using the codec.
5. Write the serialized bytes to the file.
6. Fsync the file.
7. Update the in-memory index (push to `events` vec, push global positions to stream vec).
8. Return the recorded events.

If any validation fails, no events are written (atomic -- all or nothing for a single append call).

### `Store::read_stream`

```rust
pub fn read_stream(
    &self,
    stream_id: Uuid,
    from_version: u64,
    max_count: u64,
) -> Result<Vec<RecordedEvent>, Error>
```

- If the stream does not exist, return `Error::StreamNotFound`.
- Look up the stream's global position list.
- Slice from `from_version` to `min(from_version + max_count, stream_length)`.
- For each global position in the slice, index into `events` and clone the event.
- Return the cloned events.

### `Store::read_all`

```rust
pub fn read_all(
    &self,
    from_position: u64,
    max_count: u64,
) -> Vec<RecordedEvent>
```

- Slice `events` from `from_position` to `min(from_position + max_count, events.len())`.
- Clone and return. (An empty result is valid -- it means the caller is at the head.)

### `Store::stream_version`

```rust
pub fn stream_version(&self, stream_id: &Uuid) -> Option<u64>
```

Returns the current version of a stream (last index, zero-based), or `None` if the stream does not exist. Useful for concurrency checks and subscription logic.

### `Store::global_position`

```rust
pub fn global_position(&self) -> u64
```

Returns the next global position (i.e., `events.len()` as `u64`). If the log is empty, returns 0.

## Acceptance Criteria

All tests use `tempfile::tempdir()` for isolation.

### AC-1: Open new store

- **Test**: Open a store with a path to a non-existent file. The file is created with the correct header. `global_position()` returns 0. `read_all(0, 100)` returns an empty vec.

### AC-2: Open existing store (recovery)

- **Test**: Open a store, append 5 events to 2 streams, drop the store. Open a new store at the same path. All 5 events are recovered. Stream versions and global positions are correct. Appending after recovery continues from the correct positions.

### AC-3: Partial trailing record truncation

- **Test**: Open a store, append 3 events. Close. Append 10 random bytes to the end of the file (simulating a crash mid-write). Open a new store at the same path. Only the 3 valid events are recovered. The file is truncated to remove the garbage bytes. A subsequent append succeeds and produces the correct positions.

### AC-4: CRC mismatch at tail on recovery

- **Test**: Open a store, append 3 events. Close. Corrupt the last byte of the last record's payload (CRC will fail). Open a new store. The first 2 events are recovered; the corrupted 3rd record is treated as a partial/corrupt trailing record and truncated.

### AC-5: Mid-file corruption is fatal

- **Test**: Open a store, append 3 events. Close. Corrupt a byte in the 2nd record. Open a new store. Returns `Err(Error::CorruptRecord)` because the corruption is not at the tail.

### AC-6: Append single event

- **Test**: Append one event to a new stream with `ExpectedVersion::NoStream`. Returns a `RecordedEvent` with `stream_version = 0`, `global_position = 0`. `read_stream` returns it. `read_all` returns it.

### AC-7: Append batch

- **Test**: Append 3 events atomically to a new stream. All 3 have contiguous stream versions (0, 1, 2) and contiguous global positions.

### AC-8: ExpectedVersion::Any

- **Test**: Append with `Any` to a non-existent stream -- succeeds, creates the stream.
- **Test**: Append with `Any` to an existing stream -- succeeds, appends at the correct version.

### AC-9: ExpectedVersion::NoStream

- **Test**: Append with `NoStream` to a non-existent stream -- succeeds.
- **Test**: Append with `NoStream` to an existing stream -- returns `Err(Error::WrongExpectedVersion)`.

### AC-10: ExpectedVersion::Exact

- **Test**: Append with `Exact(0)` after a single event (stream at version 0) -- succeeds, new event is at version 1.
- **Test**: Append with `Exact(5)` when stream is at version 3 -- returns `Err(Error::WrongExpectedVersion)`.
- **Test**: Append with `Exact(0)` to a non-existent stream -- returns `Err(Error::WrongExpectedVersion)`.

### AC-11: Event size validation

- **Test**: Append an event with a payload that makes the total record exceed `MAX_EVENT_SIZE`. Returns `Err(Error::EventTooLarge)`.
- **Test**: Append an event with an event type longer than `MAX_EVENT_TYPE_LEN`. Returns `Err(Error::InvalidArgument)`.
- **Test**: Append an event with an empty event type. Returns `Err(Error::InvalidArgument)`.

### AC-12: ReadStream

- **Test**: Read from version 0 with max_count 100 on a stream with 5 events -- returns all 5.
- **Test**: Read from version 2 with max_count 2 on a stream with 5 events -- returns events at versions 2 and 3.
- **Test**: Read from a version beyond the stream length -- returns empty vec.
- **Test**: Read from a non-existent stream -- returns `Err(Error::StreamNotFound)`.

### AC-13: ReadAll

- **Test**: Read from position 0 with max_count 100 on a log with 5 events -- returns all 5.
- **Test**: Read from position 3 with max_count 2 on a log with 5 events -- returns events at positions 3 and 4.
- **Test**: Read from a position beyond the log length -- returns empty vec.
- **Test**: Read from an empty log -- returns empty vec.

### AC-14: Multi-stream interleaving

- **Test**: Append events to 3 different streams in interleaved order. `read_all` returns them in global position order. `read_stream` for each stream returns only that stream's events in stream version order.

### AC-15: Stream version and global position queries

- **Test**: `stream_version` on a non-existent stream returns `None`.
- **Test**: `stream_version` on a stream with 3 events returns `Some(2)`.
- **Test**: `global_position` on an empty store returns 0.
- **Test**: `global_position` after 5 appends returns 5.

### AC-16: Build and lint

- `cargo build` completes with zero warnings.
- `cargo clippy --all-targets --all-features --locked -- -D warnings` passes.
- `cargo fmt --check` passes.
- `cargo test` passes with all tests green.

## Dependencies

- **Depends on**: PRD 001 (types, errors), PRD 002 (codec).
- **Depended on by**: PRD 004, 005, 006, 007.

## Cargo.toml Additions

```toml
[dependencies]
tracing = "0.1"

[dev-dependencies]
tempfile = "3"
```
