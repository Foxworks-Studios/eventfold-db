# EventfoldDB — Design

EventfoldDB is a lightweight, single-node event store built in Rust. It provides the minimum viable surface for event sourcing and CQRS: append-only persistence of domain events with optimistic concurrency, ordered reads by stream and globally, and catch-up-then-live subscriptions for building read models.

It is not a general-purpose database. It stores events. Backend projection services subscribe to the event log, fold events into read models, and serve those read models to client applications through separate query APIs. Desktop and web clients never interact with EventfoldDB directly.

## Context

EventfoldDB serves as the central data layer for in-house applications. A desktop CRM (or similar) communicates with a command API that validates and appends events, and a query API that reads from projection-maintained read models. EventfoldDB is the durable event log underneath.

```
Desktop App
  │
  ├── command ──► Command API ──► EventfoldDB
  │
  └── query  ──► Query API   ──► Read Model (Postgres, SQLite, etc.)
                                      ▲
                                      │
                               Projection Service
                                      │
                                      │ catch-up subscription
                                      │
                                 EventfoldDB
```

The projection service is the only consumer of EventfoldDB's subscription API. It is a long-running backend process that maintains a checkpoint, catches up on missed events after a restart, and transitions to live push once current. The read model database is whatever shape the query side needs — Postgres tables, SQLite, denormalized views. EventfoldDB does not care about or participate in the read side.

## Scope

EventfoldDB provides five operations, exposed as a gRPC service:

**Append** — Write one or more events to a named stream atomically, with an optimistic concurrency check. This is the only write path. Every event gets a contiguous, zero-based stream version (scoped to its stream) and a contiguous, zero-based global position (scoped to the entire log). The first event ever written has global position 0; the first event in a stream has stream version 0. The caller provides an expected version: "this stream must not exist," "this stream must be at version N," or "I don't care." If the check fails, the append is rejected with `FAILED_PRECONDITION`.

Optimistic concurrency is a whole-stream check, not a field-level merge. If two callers both read a stream at version 5 and both attempt to write version 6, the first succeeds and the second is rejected — even if the events touch logically independent data. This is intentional: each command decision is made against the full aggregate state at a specific version. A concurrent write invalidates that decision basis, regardless of whether the changes "conflict" at the field level. The correct recovery is for the caller to re-read the stream at its new version, re-evaluate the business rules against the updated state, and retry the append. In practice, conflicts are rare for in-house workloads and the retry adds milliseconds. EventfoldDB does not attempt merge, delta, or CRDT-style conflict resolution — that complexity belongs in domains where concurrent writes to the same aggregate are frequent (collaborative editing, counters), not in a general-purpose event store.

**ReadStream** — Read events from a single stream, forward from a given version, up to a maximum count. This is what the command side uses to rehydrate an aggregate before processing a command. Backward reads are not in scope for v1.

**ReadAll** — Read events from the global log, forward from a given global position, up to a maximum count. This is the building block for projections — a projection service can poll this endpoint to process events it hasn't seen. Backward reads are not in scope for v1.

**SubscribeAll** — A server-streaming RPC. The client provides a starting global position. The server replays all events from that position forward (the catch-up phase), sends a `CaughtUp` marker when it reaches the head of the log, then pushes new events in real-time as they are appended (the live phase). If the subscriber falls behind the live buffer, the stream terminates and the client must re-subscribe from its last checkpointed position. This is the primary mechanism for projection services that need to process events across all streams.

**SubscribeStream** — A server-streaming RPC, identical in mechanics to SubscribeAll but scoped to a single stream. The client provides a stream ID and a starting stream version. The server replays all events in that stream from the starting version forward (the catch-up phase), sends a `CaughtUp` marker when it reaches the head of the stream, then pushes new events in real-time as they are appended to that stream (the live phase). Filtering happens server-side so the client does not receive and discard irrelevant events. If the subscriber falls behind the live buffer, the stream terminates and the client must re-subscribe from its last checkpointed version. This is useful for process managers, sagas, projections scoped to a single stream, or any consumer that does not need the full global log.

### Deliberately excluded

These features exist in KurrentDB (formerly EventStoreDB) and are intentionally omitted:

**Server-side projections.** Projections run as standalone services in the application's own language and runtime. This keeps EventfoldDB simple and gives projection authors full control over schema, testing, and deployment.

**Persistent subscriptions and competing consumers.** These provide server-managed consumer groups with ack/nack and fan-out. For in-house use with singleton projection services, each service tracks its own checkpoint. If a service crashes, it restarts and catches up from its last checkpoint. Server-managed subscription state is unnecessary complexity.

**Clustering and replication.** EventfoldDB runs as a single node. If the process dies, it restarts and recovers from the durable log on disk. For in-house tooling, a few seconds of downtime during restart is acceptable.

**Stream deletion, scavenging, and compaction.** Events are immutable and permanent. Disk is cheap. The full history is always available.

**ACLs and multi-tenancy.** It is a single-tenant, in-house service. Access control belongs at the network layer.

**Stream metadata and system events.** No internal system streams, no `$` prefixed streams, no metadata streams. The global log contains only user-appended domain events.

**Backward reads.** Forward reads cover all essential use cases: aggregate rehydration and projection catch-up. Backward reads can be added later if needed.

## Identifiers and Size Limits

**Stream IDs** are UUIDs (v4 or v7), represented as the standard 36-character hyphenated lowercase hex string (e.g., `550e8400-e29b-41d4-a716-446655440000`). The server validates the format on append and rejects malformed IDs with `INVALID_ARGUMENT`. Using UUIDs eliminates ambiguity about valid characters, encoding, and maximum length. Client applications generate stream IDs; the server never mints them.

**Event IDs** are UUIDs assigned by the client, included in each proposed event. They serve as an idempotency key — the server may use them in the future to detect duplicate appends, though v1 does not enforce uniqueness. Clients should generate a unique ID per event.

**Event size limit.** The maximum size of a single event record (payload + metadata + fixed fields) is 64 KB. The server rejects any append containing an event that exceeds this limit with `INVALID_ARGUMENT`. Events are domain facts — small, structured data (typically JSON). Large artifacts like files, images, or documents belong in external storage (S3, a file server, etc.); the event carries a reference (a URL or object key) to the artifact, not the artifact itself. 64 KB is generous for JSON-shaped domain events while preventing accidental misuse.

## On-Disk Format

The durable storage is a single append-only binary file. No WAL, no B-tree, no page structure. Just a header followed by a sequence of length-prefixed, checksummed records.

The file starts with a fixed-size header containing a magic number and a format version. This allows the server to detect corruption or version mismatch immediately on open.

Each record contains: a length prefix (so the reader knows how many bytes to consume), the event's global position, the stream ID (UUID, stored as 16 raw bytes), the stream version, the event type tag (length-prefixed UTF-8, max 256 bytes), metadata bytes, payload bytes, and a CRC32 checksum over the record body. The checksum covers everything after the length prefix and before the checksum itself.

**Payload** is the serialized domain event body — the facts of what happened. For example: `{"amount": 100, "currency": "USD", "recipient": "acct_123"}`. The expected serialization format is JSON, though EventfoldDB treats it as opaque bytes. The store does not parse, validate, or index payload contents.

**Metadata** is ancillary context about the event, not part of the domain fact itself. Examples: correlation ID (to trace a chain of causally related events), causation ID (the event or command that triggered this one), the authenticated user or service that issued the command, a client-assigned timestamp, or a reference to an external artifact. Like payload, metadata is opaque bytes — the store does not interpret it. The distinction exists so that infrastructure concerns (tracing, auditing, timestamps) stay separated from domain data in the serialization layer, even though the store treats both identically.

**Timestamps.** EventfoldDB does not assign or store timestamps. Timestamps are a consumer concern — the client includes a timestamp in the event's metadata or payload as appropriate. The server uses global position and stream version for ordering and concurrency control, never wall-clock time. This avoids issues with clock skew, NTP adjustments, and ambiguous time zones. A projection that needs to display "when" an event happened reads the timestamp from the event data, which was set by the command service at the time of the original action.

The format must support detection and truncation of a partial trailing record. If the process crashes mid-write, the next startup must identify the incomplete record (via a short read or CRC mismatch at the tail), truncate it, and proceed. This is the critical correctness property that separates "works on the happy path" from "trustworthy."

## In-Memory Model

On startup, the server reads the entire log file from beginning to end, deserializing each record and building an in-memory index. This index has two structures:

A `Vec<RecordedEvent>` holding every event in global order. Index `i` is the event at global position `i`. This makes ReadAll trivially efficient — it's a slice operation.

A `HashMap<Uuid, Vec<u64>>` mapping each stream ID to the global positions of its events, in stream order. Index `j` in the vector is the event at stream version `j`. ReadStream is two lookups: find the stream's position list, then index into the global vector.

### Write serialization

Appends are serialized through a single writer task that owns exclusive access to the log file and in-memory index. gRPC handlers do not write directly. Instead, each `Append` request is sent to the writer via a bounded `tokio::mpsc` channel. The writer drains the channel in a loop, processing appends sequentially: validate the expected version against the current in-memory state, serialize the event records, write them to the file, fsync, update the in-memory index, notify the broadcast channel, and send the result back to the caller via a oneshot channel.

This design has several properties:

- **No read-side locking.** The in-memory index is append-only (new events are pushed to the end of vectors; the HashMap only gains entries, never mutates existing ones). Reads can proceed concurrently with writes without locks, as long as readers use the index length at the time of the read as their upper bound. In Rust terms, the index structures are behind an `Arc` and use atomic lengths or `RwLock` with minimal write-side contention.
- **Batching.** When multiple appends arrive concurrently, they queue in the channel. The writer can drain several pending requests per loop iteration, coalescing their disk writes into a single `writev` + `fsync`. This amortizes the fsync cost — the dominant latency — across multiple appends under load, while still guaranteeing durability for each batch.
- **Backpressure.** The bounded channel naturally applies backpressure: if the writer falls behind, callers block (async await) on channel send until capacity is available. This prevents unbounded memory growth from a burst of appends.

This model works when the event log fits comfortably in memory. For an in-house CRM, this is likely millions of events before it becomes a concern. If the log outgrows memory, the index structure can be changed to store file offsets instead of full events, and reads can go to disk. That is a future optimization, not a v1 concern.

## Subscription Mechanics

The catch-up-then-live subscription is the most subtle piece. The correctness requirement: no events may be missed during the transition from historical replay to live push, and no events may be delivered twice (or if duplicates are possible, the client must be able to deduplicate).

The sequence:

1. The subscriber registers with the broadcast channel **before** any historical events are read. This ensures that any event appended from this moment forward will be buffered in the subscriber's channel.

2. The server reads historical events from the requested starting position forward, streaming them to the client in batches.

3. When there are no more historical events, the server sends a `CaughtUp` marker. The client now knows all subsequent events are live.

4. The server switches to draining the broadcast channel. Events with a global position less than or equal to the last historically-sent position are skipped (deduplication). All others are forwarded to the client.

5. If the broadcast channel's ring buffer overflows (the subscriber fell too far behind during catch-up or live processing), the stream is terminated. The client re-subscribes from its last checkpointed position.

The broadcast channel is a bounded ring buffer. Its capacity is a server configuration parameter. It does not need to be large — it only needs to cover the time between the end of catch-up and the start of live draining. A few thousand slots is generous for in-house workloads.

## gRPC Service

The service definition is a single `.proto` file with five RPCs:

- `Append` — unary. Request contains stream ID, expected version, and a list of proposed events (each with an event ID, event type, metadata bytes, and payload bytes). Response contains the first and last stream version and global position of the written events.
- `ReadStream` — unary. Request contains stream ID, starting version, and max count. Response contains a list of recorded events.
- `ReadAll` — unary. Request contains starting global position and max count. Response contains a list of recorded events.
- `SubscribeAll` — server-streaming. Request contains an optional starting global position (defaults to 0). Response is a stream of messages, each of which is either a recorded event or a `CaughtUp` marker.
- `SubscribeStream` — server-streaming. Request contains a stream ID and an optional starting stream version (defaults to 0). Response is a stream of messages, each of which is either a recorded event or a `CaughtUp` marker. Only events belonging to the specified stream are delivered.

The expected version on `Append` is a `oneof`: `any` (no check), `no_stream` (stream must not exist), or `exact(uint64)` (stream must be at exactly this version). Violation returns `FAILED_PRECONDITION`.

Recorded events in all responses include: event ID (UUID), stream ID (UUID), stream version, global position, event type string, metadata bytes, and payload bytes. There is no server-assigned timestamp — timestamps are a client concern, carried in metadata or payload.

The server listens on a single port (default 2113, matching KurrentDB convention for familiarity). TLS can be added later via tonic's built-in TLS support.

## Deployment

EventfoldDB runs as a single long-lived process with access to persistent disk. The deployment target is Fly.io with a persistent volume, though any environment with durable storage works (a VPS, ECS with EBS, a bare metal box under a desk).

Configuration is via environment variables:
- `EVENTFOLD_DATA` — path to the log file
- `EVENTFOLD_LISTEN` — listen address (e.g. `[::]:2113`)
- `EVENTFOLD_BROKER_CAPACITY` — ring buffer size for live subscriptions

The Dockerfile is a two-stage build: compile the Rust binary in a builder image, copy it into a minimal runtime image. The Fly configuration mounts a persistent volume at `/data`.

The process is single-writer by design. Do not run multiple instances against the same log file. Scaling reads happens through projections and read model databases, not through database replicas.

## Development Practices

EventfoldDB is written in Rust, targeting stable Rust with the 2024 edition.

### Structure

The crate is both a library and a binary. The library exposes the storage engine, subscription broker, and type definitions. The binary is a thin main that reads configuration, opens the engine, and starts the gRPC server. Integration tests exercise the full path from gRPC client through the server to the store and back.

### Error handling

All fallible operations return `Result<T, eventfold_db::Error>`. The error type is an enum covering: wrong expected version, stream not found, IO errors, corrupt records, and invalid headers. The gRPC layer maps these to appropriate status codes (FAILED_PRECONDITION, NOT_FOUND, INTERNAL, DATA_LOSS). Panics are reserved for programmer errors (violated invariants), never for operational failures.

### Testing

Every module is developed using red/green TDD:

1. Write a failing test that describes the expected behavior.
2. Write the minimum code to make it pass.
3. Refactor while keeping all tests green.

Unit tests live alongside the code they test (in `#[cfg(test)]` modules). Integration tests live in `tests/` and exercise the public API boundaries — the store's Rust API and the gRPC service.

Test categories to cover for each component:

**Append and concurrency** — single event, batch append, ExpectedVersion::Any succeeds on new and existing streams, NoStream fails if stream exists, Exact(n) fails on mismatch, Exact(n) succeeds on match.

**Reads** — forward from start, forward from a specific version, max count limits, reading a nonexistent stream returns an error, empty store returns empty results.

**Global log** — events from multiple streams interleave correctly, global positions are monotonically increasing, ReadAll from a given position returns the correct slice.

**Persistence** — write events, close the store, reopen it, verify all events and stream versions are recovered. Append after reopen continues from the correct position. Concurrency checks survive reopen.

**Corruption resilience** — a flipped byte in a record body is detected by CRC mismatch. A truncated trailing record is detected and handled on startup. A clean EOF (no partial record) results in normal operation.

**Subscriptions** — historical events are delivered in order, CaughtUp is sent after history, live events arrive after CaughtUp, events from other streams are filtered (for SubscribeStream), a subscriber that falls behind gets terminated.

**gRPC integration** — spin up a real server on an ephemeral port, exercise every RPC through a tonic client, verify status codes for error cases.

### Dependencies

Keep the dependency tree minimal:
- `tonic` and `prost` for gRPC
- `tokio` for the async runtime
- `crc32fast` for checksums
- `thiserror` for error types
- `async-stream` for ergonomic async generators in subscription handlers
- `tracing` for structured logging
- `tempfile` in dev-dependencies for test isolation

### Naming and style

Follow standard Rust conventions: snake_case for functions and variables, CamelCase for types, SCREAMING_CASE for constants. Prefer descriptive names over abbreviations. Public API types go in a `types` module and are re-exported from the crate root. Keep modules focused — one responsibility per file.

## Future Considerations

These are not in scope for v1 but are worth noting so the design doesn't accidentally preclude them.

**Snapshotting.** If aggregate rehydration becomes slow (hundreds of thousands of events in a single stream), the command side may want to store periodic snapshots. This can be handled entirely outside EventfoldDB — the command service stores snapshots in its own database and only reads events from the snapshot's version forward. EventfoldDB does not need a native snapshot concept.

**Checkpointing in the subscription protocol.** The server could periodically send a `Checkpoint` message containing the current global position, giving the client a signal to persist its progress. This is a small protocol addition that doesn't affect the storage engine.

**File-offset-based index.** If the event log outgrows memory, the in-memory Vec of events can be replaced with a Vec of file offsets. Reads would seek to the offset and deserialize on demand. The index (stream ID → positions) would remain in memory. This changes the read path but not the write path or the API.

**TLS and authentication.** Tonic supports TLS natively. Mutual TLS (mTLS) is the simplest auth model for in-house services — issue client certificates to authorized services. No need for a user/password system.
