# PRD 008: Batch Atomicity and Crash Consistency Hardening

**Status:** DRAFT
**Created:** 2026-02-25
**Author:** PRD Writer Agent

---

## Problem Statement

EventfoldDB's `Append` RPC promises that a batch of events is committed atomically: either all events in the call are durable or none are. The current on-disk format encodes each event as an individually-checksummed record with no shared batch envelope. If the process crashes mid-write (e.g., 2 of 3 records hit disk before power loss), startup recovery reads the 2 valid records and keeps them, silently breaking the atomicity guarantee. Additionally, newly-created log files are not directory-fsynced, which means the directory entry itself may be lost on crash, and the code carries no documented filesystem assumptions, making it impossible to reason about the durability model.

## Goals

- Introduce a batch envelope (header + footer) so recovery can distinguish a complete committed batch from a partial write and truncate the latter.
- Guarantee that a newly created log file is visible after a crash by directory-fsyncing on initial file creation.
- Document the filesystem assumptions (ext4 `data=ordered`) the durability model depends on so that operators and future maintainers can make informed decisions.

## Non-Goals

- Idempotency / duplicate-event detection using client-supplied event IDs. Event IDs are preserved on disk as of PRD 002 but deduplication is explicitly deferred to a future PRD.
- Multi-file or write-ahead-log (WAL) storage formats. This PRD extends the existing single append-only file format only.
- Cross-batch atomicity (spanning multiple `Append` RPC calls). Each gRPC `Append` call is one batch; multi-call transactions are not in scope.
- Performance optimisation of the write path (e.g., `O_DIRECT`, `io_uring`). The existing `write_all` + `sync_all` approach is retained.
- Changing the `FORMAT_VERSION` bump strategy. This PRD increments the version from `1` to `2` and documents the migration path, but does not implement a live upgrade tool.

## User Stories

- As a server operator, I want EventfoldDB to truncate any partial batch on restart after a crash, so that the event log contains only fully-committed batches and the `Append` atomicity guarantee is never violated.
- As a server operator, I want EventfoldDB to fsync the parent directory when creating a new log file, so that the log file is guaranteed to be findable after a crash even if the OS had not yet flushed the directory entry.
- As a future maintainer, I want the source code and documentation to state which filesystem features EventfoldDB relies on (e.g., ext4 `data=ordered`), so that I can evaluate portability and operational requirements without guessing.

## Technical Approach

### Overview of affected files

| File | Change |
|------|--------|
| `src/codec.rs` | Add `encode_batch_header`, `decode_batch_header`, `encode_batch_footer`, `decode_batch_footer`; bump `FORMAT_VERSION` to `2` |
| `src/store.rs` | Wrap each `append` write in a batch envelope; directory-fsync on new file creation; update recovery loop to detect and truncate partial batches |
| `docs/design.md` | Add a "Filesystem Assumptions" subsection documenting ext4 `data=ordered`, directory fsync rationale, and fsync semantics |

No changes to `error.rs`, `types.rs`, `writer.rs`, `broker.rs`, `service.rs`, or `main.rs`.

---

### Batch envelope format (codec.rs)

The new on-disk layout for a batch is:

```
[BATCH_HEADER]
  [RECORD_0]
  [RECORD_1]
  ...
  [RECORD_N-1]
[BATCH_FOOTER]
```

#### Batch header (12 bytes)

| Offset | Size | Field | Value |
|--------|------|-------|-------|
| 0 | 4 | Batch magic | `0x45464242` (ASCII `EFBB`) |
| 4 | 4 | Record count | `u32` LE — number of event records in this batch |
| 8 | 4 | First global position | `u32` LE — `global_position` of the first event record (`u32` is sufficient; use `u64` LE if the global log is expected to exceed 2^32 events — see Open Questions) |

> Note: use `u64` LE for the first global position field to be consistent with the record body format and to avoid overflow on large logs. The header is therefore **16 bytes** (magic 4 + count 4 + first_global_pos 8).

**Revised batch header (16 bytes)**:

| Offset | Size | Field | Value |
|--------|------|-------|-------|
| 0 | 4 | Batch magic | `[0x45, 0x46, 0x42, 0x42]` (ASCII `EFBB`) |
| 4 | 4 | Record count | `u32` LE |
| 8 | 8 | First global position | `u64` LE |

New constant: `BATCH_HEADER_MAGIC: [u8; 4] = [0x45, 0x46, 0x42, 0x42]`.

Functions:
- `encode_batch_header(record_count: u32, first_global_pos: u64) -> [u8; 16]`
- `decode_batch_header(buf: &[u8]) -> Result<BatchHeader, Error>` — returns `DecodeOutcome::Incomplete` if `buf.len() < 16`, returns `Error::CorruptRecord` if the magic bytes do not match.

```rust
pub struct BatchHeader {
    pub record_count: u32,
    pub first_global_pos: u64,
}
```

#### Batch footer (8 bytes)

The footer is a commit marker. Its presence at the expected byte offset signals that all records in the batch were written completely before the process crashed.

| Offset | Size | Field | Value |
|--------|------|-------|-------|
| 0 | 4 | Footer magic | `[0x45, 0x46, 0x42, 0x46]` (ASCII `EFBF`) |
| 4 | 4 | CRC32 | CRC32 LE over the batch header bytes concatenated with all record bytes (does not include the footer itself) |

New constant: `BATCH_FOOTER_MAGIC: [u8; 4] = [0x45, 0x46, 0x42, 0x46]`.

Functions:
- `encode_batch_footer(batch_crc: u32) -> [u8; 8]`
- `decode_batch_footer(buf: &[u8]) -> Result<BatchFooter, Error>` — returns `DecodeOutcome::Incomplete` if `buf.len() < 8`, returns `Error::CorruptRecord` if the magic bytes do not match.

```rust
pub struct BatchFooter {
    pub batch_crc: u32,
}
```

The `batch_crc` is computed over: `batch_header_bytes || record_0_bytes || record_1_bytes || ... || record_N-1_bytes`. This gives a second layer of integrity above the per-record CRC32s: if any record within the batch is silently corrupted after an acknowledged write, the batch CRC will catch it on the next recovery scan.

---

### Format version bump (codec.rs)

`FORMAT_VERSION` is incremented from `1` to `2`. The `decode_header` function is updated to accept version `2` only (existing files at version `1` are rejected with `Error::InvalidHeader` mentioning the version mismatch). Operators must run a one-time migration tool (out of scope for this PRD) or start a fresh log. This is documented in `docs/design.md`.

---

### Store::append changes (store.rs)

The write sequence in `Store::append` (currently step 3) becomes:

```
1. encode batch header  (16 bytes)
2. encode each record   (as today)
3. compute batch_crc over header bytes || all record bytes
4. encode batch footer  (8 bytes)
5. seek to end
6. write_all(batch_header || records || batch_footer)
7. sync_all()
8. update in-memory index (write lock)
```

The single `write_all` call writes the entire batch — header, records, and footer — atomically from the OS's perspective. A crash after a partial write leaves a batch without a valid footer; recovery detects this and truncates.

The existing `encoded_batch: Vec<u8>` accumulation in `Store::append` is extended to prepend the batch header and append the batch footer before the `write_all` call.

---

### Store::open — recovery loop changes (store.rs)

The recovery loop currently decodes individual records. With the batch envelope, the loop reads one batch at a time:

```
loop:
  1. Read and decode batch header from current offset.
     - Incomplete → trailing partial batch; truncate to offset; warn; break.
     - CorruptRecord (bad magic) → treat as trailing corruption;
       if valid data follows, return Error::CorruptRecord (mid-file); else truncate; break.
  2. Read and decode N records (N = batch_header.record_count).
     - Any Incomplete or CorruptRecord → trailing partial batch; truncate to offset; warn; break.
  3. Read and decode batch footer.
     - Incomplete → trailing partial batch; truncate to offset; warn; break.
     - CorruptRecord (bad magic or CRC mismatch) → trailing partial batch; truncate to offset; warn; break.
  4. Accept the batch: add all N events to in-memory index. Advance offset.
```

The existing `has_valid_record_after` helper is retained unchanged. After any truncation decision, the function checks whether valid data (a complete batch) follows the truncation point using a variant of this helper; if so, it escalates to `Error::CorruptRecord` (mid-file corruption, unrecoverable).

The truncation path (`file.set_len` + `sync_all`) is unchanged.

---

### Directory fsync on new file creation (store.rs)

When `Store::open` creates a new log file (the `!path.exists()` branch), after the file `sync_all()`, it must also fsync the parent directory:

```rust
// After file.sync_all():
let parent = path.parent().expect("log path must have a parent directory");
let dir = std::fs::File::open(parent)?;
dir.sync_all()?;
```

This ensures the directory entry pointing at the new file is durable. On ext4 `data=ordered`, a file fsync guarantees the file's data and inode are durable, but not that the directory entry is committed. Without directory fsync, a crash between creating the file and the OS writing the directory entry leaves the file inaccessible on restart.

This call is only needed on initial file creation, not on every append.

---

### Filesystem assumptions documentation (docs/design.md)

A new "Filesystem Assumptions" subsection is added to `docs/design.md` under the "On-Disk Format" section. It must state:

- EventfoldDB is tested and supported on **ext4 with `data=ordered`** (the Linux default). Other filesystems (XFS, btrfs, ZFS, tmpfs) may work but are not validated.
- `data=ordered` guarantees that file data is written to disk before the metadata (inode) is committed. This means a file `fsync` is sufficient to make written data durable; there is no risk of a zero-length inode with unflushed data after a crash.
- A directory fsync is required after creating a new file to make the directory entry (the name-to-inode link) durable.
- All fsync calls in EventfoldDB use `File::sync_all()` (maps to `fsync(2)` on Linux, not `fdatasync(2)`), which flushes both data and metadata.
- Network-attached and distributed filesystems (NFS, CIFS, FUSE-based) are explicitly not supported; their fsync semantics may differ from POSIX requirements.

## Acceptance Criteria

1. After `Store::append` writes a batch of N events and `sync_all()` completes without error, the log file contains a valid batch header (correct magic), exactly N individually-checksummed records, and a valid batch footer (correct magic + correct batch CRC) immediately following the last record. Verified by reading the raw bytes of the file in a unit test.

2. When the log file is truncated to remove the batch footer only (simulating a crash after all records but before the footer was flushed), `Store::open` truncates the file to the byte offset of the batch header, logs a `tracing::warn!`, and returns a store with zero events (or the events from any previously complete batches). The gRPC `Append` response for the in-flight request is never sent, so no client ever observes the partial batch.

3. When the log file is truncated to the middle of a record within a batch (simulating a crash mid-record), `Store::open` truncates the file to the byte offset of the incomplete batch's header, logs a `tracing::warn!`, and returns a store containing only the events from fully committed prior batches.

4. When the log file contains two complete batches followed by a partial third batch (batch header present, records partially written, footer absent), `Store::open` returns a store containing exactly the events from the two complete batches and truncates the file to the byte after the second batch's footer.

5. When a log file written at `FORMAT_VERSION = 1` (old format, no batch envelopes) is opened by the new code, `Store::open` returns `Err(Error::InvalidHeader)` with a message that mentions "version". The file is not modified.

6. When `Store::open` creates a new log file (file did not previously exist), the parent directory receives a successful `fsync` call. Verified in a unit test by asserting the returned store is empty and that a second `Store::open` on the same path after simulating an immediate process exit (i.e., without any writes after open) recovers an empty-but-valid log file.

7. When `Store::open` opens an existing log file with two complete batches, the recovered store's `read_all` returns all events from both batches in global-position order, with no gaps in global position numbering.

8. `encode_batch_header(count, first_pos)` produces exactly 16 bytes: bytes 0..4 are `[0x45, 0x46, 0x42, 0x42]`, bytes 4..8 are `count` as `u32` LE, bytes 8..16 are `first_pos` as `u64` LE. Verified by a unit test that encodes known values and inspects byte offsets.

9. `encode_batch_footer(crc)` produces exactly 8 bytes: bytes 0..4 are `[0x45, 0x46, 0x42, 0x46]`, bytes 4..8 are `crc` as `u32` LE. Verified by a unit test that encodes a known CRC and inspects byte offsets.

10. `cargo build`, `cargo clippy --all-targets --all-features --locked -- -D warnings`, `cargo fmt --check`, and `cargo test` all pass with zero warnings or failures after this change.

## Open Questions

- **u64 vs u32 for first_global_pos in batch header**: This PRD uses `u64` (8 bytes) for consistency with the record body format. If the log will never exceed 2^32 events in practice, `u32` (4 bytes) saves 4 bytes per batch. The choice affects the batch header size constant. Recommend `u64` to avoid a future format break.
- **Batch CRC scope**: The batch CRC currently covers the header bytes plus all record bytes. An alternative is to cover only the record bytes (excluding the header). The chosen scope is header + records because it ensures the record count and first position are also integrity-checked at the batch level. If this causes implementation friction (e.g., the header must be fully encoded before the CRC accumulator is started), the scope can be narrowed to records only.
- **Migration tooling for FORMAT_VERSION 1 files**: This PRD does not implement a migration tool. Operators with existing production data at version 1 will need one before upgrading. A future PRD should cover `eventfold-db migrate` or an automatic upgrade path in `Store::open`.
- **`data=writeback` compatibility**: On ext4 with `data=writeback`, file data can appear after metadata commits, meaning a crash could leave stale data visible in a file that the inode says is non-empty. EventfoldDB's per-record CRC32 and batch footer CRC would catch this, but the recovery path needs a test on `data=writeback` to confirm correctness. Out of scope here but worth flagging.

## Dependencies

- **Depends on**: PRD 001 (types, error enum), PRD 002 (codec — `encode_record`, `decode_record`, `DecodeOutcome`), PRD 003 (store — `Store::open`, `Store::append`).
- **Depended on by**: PRD 004 (writer task uses `Store::append` — no interface change, but integration tests should be re-run), PRD 007 (server binary opens the store on startup — `FORMAT_VERSION` bump affects existing data files).
- **External**: `crc32fast` (already a dependency from PRD 002). No new crate dependencies required.
