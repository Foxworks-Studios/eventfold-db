# EventfoldDB

A lightweight, single-node event store built in Rust for event sourcing and CQRS.

EventfoldDB provides the minimum viable surface for event-sourced systems: append-only persistence of domain events with optimistic concurrency, ordered reads by stream and globally, and catch-up-then-live subscriptions for building read models.

## How it works

```
Desktop App
  |
  |-- command --> Command API --> EventfoldDB
  |
  +-- query  --> Query API   --> Read Model (Postgres, SQLite, etc.)
                                      ^
                                      |
                               Projection Service
                                      |
                                      | catch-up subscription
                                      |
                                 EventfoldDB
```

EventfoldDB is the durable event log. Projection services subscribe to it, fold events into read models, and serve those models through separate query APIs. Clients never interact with EventfoldDB directly.

## Operations

Five gRPC operations:

| RPC | Type | Purpose |
|-----|------|---------|
| **Append** | Unary | Write events to a stream with optimistic concurrency |
| **ReadStream** | Unary | Read events from a single stream by version |
| **ReadAll** | Unary | Read events from the global log by position |
| **SubscribeAll** | Server-streaming | Catch-up + live subscription across all streams |
| **SubscribeStream** | Server-streaming | Catch-up + live subscription for a single stream |

## Key design choices

- **Append-only binary log.** Single file, length-prefixed, CRC32-checksummed records. No WAL, no B-tree.
- **In-memory index.** Full event log loaded into memory on startup. Reads are slice operations.
- **Single writer task.** All appends go through a serialized writer with batched fsync for durability.
- **No server timestamps.** Ordering uses global position and stream version. Timestamps are a client concern.
- **64 KB event limit.** Events are small, structured domain facts. Large artifacts belong in external storage.
- **UUID identifiers.** Stream IDs and event IDs are UUIDs (v4 or v7).

## Building

Requires stable Rust toolchain (2024 edition).

```sh
cargo build
cargo test
cargo clippy --all-targets --all-features --locked -- -D warnings
```

## Running

```sh
EVENTFOLD_DATA=/path/to/log.bin \
EVENTFOLD_LISTEN=[::]:2113 \
EVENTFOLD_BROKER_CAPACITY=4096 \
cargo run
```

## Design

See [docs/design.md](docs/design.md) for the full design document.

## License

Licensed under either of [Apache License, Version 2.0](http://www.apache.org/licenses/LICENSE-2.0) or [MIT license](http://opensource.org/licenses/MIT) at your option.
