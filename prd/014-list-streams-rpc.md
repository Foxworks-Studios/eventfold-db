# PRD 014: ListStreams RPC

**Status:** TICKETS READY
**Created:** 2026-02-26
**Author:** PRD Writer Agent

---

## Problem Statement

The console TUI (`eventfold-console`) currently implements "list streams" by calling `ReadAll` in a paginated loop, deserializing every event in the log just to collect unique stream IDs, counts, and latest versions. This is O(n) in total event count. The in-memory index already maintains a `HashMap<Uuid, Vec<u64>>` mapping each stream ID to its global positions, making this query O(s) where s is the number of streams — a constant-time read that never touches event data. Adding a `ListStreams` RPC exposes this cheap path over gRPC and lets the console (and any future client) drop the scan entirely.

## Goals

- Add a `ListStreams` unary RPC to the EventfoldDB gRPC service that returns all known stream IDs with their event counts and latest stream versions in a single round trip.
- Expose a `list_streams()` method on `ReadIndex` that reads directly from `EventLog::streams` without touching event data.
- Update `eventfold-console`'s `Client::list_streams` to call the new RPC instead of the `ReadAll` scan, removing `collect_streams` dead code.

## Non-Goals

- Pagination (`max_count`, `continuation_token`): v1 returns all streams in one response. In-house workloads will not reach a count where this matters; pagination can be added in a future PRD.
- Filtering by stream ID prefix or pattern.
- Returning event data (payload, metadata, event type) alongside stream metadata. Stream metadata only: ID, count, latest version.
- Any changes to the on-disk format or write path.
- Server-side sorting beyond lexicographic by stream ID string. Clients needing a different order should sort locally.

## User Stories

- As a developer using `eventfold-console`, I want the Streams tab to load instantly regardless of how many events are in the log, so I can inspect my streams without waiting for a full event scan.
- As a client application developer, I want a single `ListStreams` RPC call that returns all stream metadata, so I can build stream-browsing features without implementing a client-side scan.
- As an operator, I want `ListStreams` to be O(number of streams) not O(number of events), so listing streams on a large log does not cause measurable server load.

## Technical Approach

### Overview of affected files

| File | Change |
|---|---|
| `proto/eventfold.proto` | Add `ListStreams` RPC, `ListStreamsRequest`, `ListStreamsResponse`, `StreamInfo` message |
| `build.rs` | No change (existing `tonic_build` invocation regenerates from the proto automatically) |
| `src/reader.rs` | Add `ReadIndex::list_streams() -> Vec<StreamInfo>` |
| `src/types.rs` | Add `StreamInfo` domain type |
| `src/service.rs` | Implement `list_streams` handler on `EventfoldService`; add `stream_info_to_proto` conversion helper |
| `src/lib.rs` | Re-export `StreamInfo` from `types` |
| `eventfold-console/src/client.rs` | Replace `ReadAll` scan loop in `Client::list_streams` with a single `ListStreams` RPC call |
| `eventfold-console/src/app.rs` | Remove `collect_streams` method and its tests; `StreamInfo` is now populated directly from the RPC response |
| `tests/integration.rs` | Add gRPC integration test for `ListStreams` |

### Proto changes (`proto/eventfold.proto`)

Add to the `EventStore` service:

```proto
rpc ListStreams(ListStreamsRequest) returns (ListStreamsResponse);
```

Add messages:

```proto
message ListStreamsRequest {}

message StreamInfo {
    string stream_id = 1;       // UUID string
    uint64 event_count = 2;
    uint64 latest_version = 3;
}

message ListStreamsResponse {
    repeated StreamInfo streams = 1;
}
```

`ListStreamsRequest` is an empty message (not reusing `Empty` to keep it extensible for future pagination fields).

### Domain type (`src/types.rs`)

Add:

```rust
/// Metadata about a single stream returned by [`ReadIndex::list_streams`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamInfo {
    /// Stream UUID.
    pub stream_id: Uuid,
    /// Total number of events in the stream.
    pub event_count: u64,
    /// Zero-based version of the last event written to the stream.
    pub latest_version: u64,
}
```

Re-export from `src/lib.rs` alongside the other public types.

### ReadIndex method (`src/reader.rs`)

Add to `impl ReadIndex`:

```rust
/// Return metadata for all known streams, sorted lexicographically by stream ID string.
///
/// Acquires a single read lock for the full operation. Result is O(s) where
/// s is the number of distinct streams. Event data is never accessed.
///
/// # Returns
///
/// A `Vec<StreamInfo>` sorted by `stream_id.to_string()` (UUID hyphenated lowercase).
pub fn list_streams(&self) -> Vec<StreamInfo> {
    let log = self.log.read().expect("EventLog RwLock poisoned");
    let mut streams: Vec<StreamInfo> = log
        .streams
        .iter()
        .map(|(id, positions)| StreamInfo {
            stream_id: *id,
            event_count: positions.len() as u64,
            latest_version: positions.len() as u64 - 1,
        })
        .collect();
    streams.sort_by(|a, b| a.stream_id.to_string().cmp(&b.stream_id.to_string()));
    streams
}
```

The `latest_version` formula `positions.len() - 1` is safe because the in-memory invariant guarantees that any stream in `EventLog::streams` has at least one event (stream entries are created on first append and never removed). This must be documented and enforced by an assertion in tests.

### Service handler (`src/service.rs`)

Add the handler to the `EventStore` trait impl:

```rust
async fn list_streams(
    &self,
    _request: tonic::Request<proto::ListStreamsRequest>,
) -> Result<tonic::Response<proto::ListStreamsResponse>, tonic::Status> {
    let streams = self
        .read_index
        .list_streams()
        .into_iter()
        .map(stream_info_to_proto)
        .collect();
    Ok(tonic::Response::new(proto::ListStreamsResponse { streams }))
}
```

Add a conversion helper following the pattern of `recorded_to_proto`:

```rust
/// Convert a domain [`StreamInfo`] to the protobuf `StreamInfo` type.
pub fn stream_info_to_proto(s: StreamInfo) -> proto::StreamInfo {
    proto::StreamInfo {
        stream_id: s.stream_id.to_string(),
        event_count: s.event_count,
        latest_version: s.latest_version,
    }
}
```

### Console client update (`eventfold-console/src/client.rs`)

Replace the existing `Client::list_streams` implementation:

```rust
pub async fn list_streams(&mut self) -> Result<Vec<StreamInfo>, ConsoleError> {
    let response = self
        .inner
        .list_streams(ListStreamsRequest {})
        .await?
        .into_inner();
    Ok(response
        .streams
        .into_iter()
        .map(|s| StreamInfo {
            stream_id: s.stream_id,
            event_count: s.event_count,
            latest_version: s.latest_version,
        })
        .collect())
}
```

The `page_size` parameter is removed since the new RPC returns all streams in one response. All call sites in `app.rs` and `tui.rs` that pass `page_size` must be updated to call `list_streams()` without arguments.

### Dead code removal (`eventfold-console/src/app.rs`)

Remove `AppState::collect_streams` and its unit tests (`collect_streams_groups_by_stream_id`, `collect_streams_empty_input_returns_empty`). The `StreamInfo` type in `app.rs` is populated directly from the RPC response and needs no derivation from raw event data.

### Integration test (`tests/integration.rs`)

Add a test that:
1. Starts a real gRPC server on an ephemeral port using `tempfile::tempdir()` for the log.
2. Appends events to three distinct streams (varying event counts).
3. Calls `ListStreams` via a tonic client.
4. Asserts: response contains exactly 3 entries; each entry's `stream_id`, `event_count`, and `latest_version` match what was appended; entries are sorted lexicographically by `stream_id`.
5. Verifies that calling `ListStreams` on an empty store returns an empty `streams` list (not an error).

## Acceptance Criteria

1. `proto/eventfold.proto` defines `rpc ListStreams(ListStreamsRequest) returns (ListStreamsResponse)`, `message ListStreamsRequest {}`, `message StreamInfo { string stream_id = 1; uint64 event_count = 2; uint64 latest_version = 3; }`, and `message ListStreamsResponse { repeated StreamInfo streams = 1; }`.
2. Calling `ListStreams` on a store with N distinct streams returns exactly N `StreamInfo` entries, each with the correct `stream_id` (UUID hyphenated lowercase string), `event_count`, and `latest_version` matching the number of events appended to that stream.
3. Calling `ListStreams` on an empty store returns `OK` with an empty `streams` list (not `NOT_FOUND` or any error status).
4. The returned `StreamInfo` entries are sorted in ascending lexicographic order by `stream_id` string. Given two streams whose UUIDs sort as A < B, A appears at index 0 and B at index 1 in the response.
5. `ReadIndex::list_streams()` acquires exactly one `RwLock` read guard for the duration of the call and does not clone or iterate over event payloads, metadata, or event type fields.
6. `eventfold-console`'s `Client::list_streams` calls the `ListStreams` RPC and does not call `ReadAll` or `read_all`. The `page_size` parameter is removed from the method signature.
7. `AppState::collect_streams` is deleted from `eventfold-console/src/app.rs` with no remaining references to it.
8. `cargo build` at the workspace root produces zero warnings for both `eventfold-db` and `eventfold-console`.
9. `cargo clippy --all-targets --all-features --locked -- -D warnings` passes clean for the entire workspace.
10. `cargo test` passes for the entire workspace, including the new `ListStreams` gRPC integration test.

## Open Questions

- Should `latest_version` be an `Option<uint64>` to distinguish "stream has no events" from "stream is at version 0"? Currently, the invariant is that a stream entry in `EventLog::streams` always has at least one position, so `latest_version = 0` means exactly one event, never zero. This is safe under the current implementation, but if the invariant ever relaxes, `Option` would be needed. Defaulting to `uint64` (non-optional) for v1; the zero-event case cannot occur in the current write path.
- If `eventfold-console` is not yet fully implemented (PRD 009 is DRAFT), the console update described here should be applied to whatever state `eventfold-console/src/client.rs` is in at implementation time. The PRD describes the target end state.

## Dependencies

- **Depends on**: PRDs 001-009 (core types, on-disk format, storage engine, writer task, subscription broker, gRPC service, server binary, batch atomicity, console TUI crate structure)
- **Proto regeneration**: `build.rs` uses `tonic_build::compile_protos`; adding the new messages and RPC requires no `build.rs` changes — the existing invocation regenerates `src/proto/` automatically on `cargo build`
- **Depended on by**: `eventfold-console` (removes its O(n) `ReadAll` scan workaround once this RPC is available)
