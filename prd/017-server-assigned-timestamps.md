# PRD 017: Server-Assigned Timestamps

**Status:** TICKETS READY
**Created:** 2026-02-27
**Revised:** 2026-02-27
**Author:** PRD Writer Agent

---

## Problem Statement

EventfoldDB does not assign timestamps to events, requiring clients to encode timestamps
into opaque metadata bytes. This makes it impossible to display event timing from the
server or console without deserializing client-specific metadata formats.

## Goals

- Add a `recorded_at: u64` field (Unix epoch milliseconds, server-assigned at append time)
  to `RecordedEvent`, persisted in the binary codec and exposed via gRPC.

## Non-Goals

- Migration of old log files. Delete `data/` and start fresh.
- Client-supplied timestamps. The server always assigns `recorded_at`.
- Timezone handling. `recorded_at` is always UTC.

## Technical Approach

### Changes

| File | Change |
|------|--------|
| `src/types.rs` | Add `pub recorded_at: u64` to `RecordedEvent` |
| `src/codec.rs` | Bump `FORMAT_VERSION` to 3; encode/decode `recorded_at` as u64 LE after `global_position`; update record size calculation |
| `src/store.rs` | `append()` gains a `recorded_at: u64` parameter; sets it on each `RecordedEvent` |
| `src/writer.rs` | Stamp `recorded_at` via `SystemTime::now().duration_since(UNIX_EPOCH).as_millis() as u64` before calling `store.append()` |
| `src/service.rs` | Add `recorded_at` to `recorded_to_proto` conversion |
| `proto/eventfold.proto` | Add `uint64 recorded_at = 8` to `RecordedEvent` message |
| `CLAUDE.md` | Update domain rules: server now assigns `recorded_at` |

No changes to `error.rs`, `dedup.rs`, `broker.rs`, `lib.rs`, `main.rs`, `metrics.rs`.

### Binary codec (format v3)

```
global_position (8) + recorded_at (8) + stream_id (16) + stream_version (8)
+ event_id (16) + event_type_len (2) + event_type (N) + metadata_len (4)
+ metadata (M) + payload_len (4) + payload (P) + crc32 (4)
```

Only change from v2: `recorded_at` (8 bytes, u64 LE) inserted after `global_position`.
Fixed overhead per record increases by 8 bytes. CRC covers the full record body as before.

### Timestamp assignment

```rust
let recorded_at = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .expect("system clock before Unix epoch")
    .as_millis() as u64;
```

Stamped in the writer task, not the gRPC handler, so it reflects persistence order.
All events in a single batch share the same `recorded_at`.

### Proto

```proto
message RecordedEvent {
    string event_id = 1;
    string stream_id = 2;
    uint64 stream_version = 3;
    uint64 global_position = 4;
    string event_type = 5;
    bytes metadata = 6;
    bytes payload = 7;
    uint64 recorded_at = 8;  // Unix epoch milliseconds, server-assigned
}
```

## Acceptance Criteria

1. `RecordedEvent` in `src/types.rs` has a `pub recorded_at: u64` field.

2. Every `RecordedEvent` returned by reads and subscriptions has `recorded_at > 0`.

3. All events in a single append batch share the same `recorded_at` value.

4. `FORMAT_VERSION` in `src/codec.rs` is 3. Old data files are rejected by the existing
   header validation (no special v2-specific error handling needed).

5. Encode/decode round-trip: a fresh store, appended to, closed, and re-opened recovers
   all events with correct `recorded_at` millisecond values.

6. The gRPC `RecordedEvent` proto includes `recorded_at` as field 8. All four streaming/
   read RPCs populate the field.

7. All quality gates pass: `cargo build` (zero warnings), `cargo clippy`, `cargo test`,
   `cargo fmt --check`.

## Dependencies

- Depends on PRDs 001-008 (stable codec and writer).
- No new crate dependencies.
