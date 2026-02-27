# Project: EventfoldDB

## What This Is

A lightweight, single-node event store built in Rust for event sourcing and CQRS. Provides append-only persistence of domain events with optimistic concurrency, ordered reads by stream and globally, and catch-up-then-live subscriptions for building read models via external projection services.

## Tech Stack

- Language: Rust (2024 edition, stable toolchain)
- Async runtime: tokio
- gRPC: tonic + prost (code-gen from `.proto`)
- Checksums: crc32fast
- Error types: thiserror
- Logging: tracing
- Subscriptions: tokio::broadcast (live push), async-stream (server-streaming helpers)
- Test isolation: tempfile (dev-dependency)

## Architecture

EventfoldDB is both a library crate and a binary. The binary is a thin `main` that reads config, opens the storage engine, and starts the gRPC server.

```
src/
  main.rs          -- binary entrypoint: config, engine init, gRPC server
  lib.rs           -- crate root, re-exports public API
  types.rs         -- RecordedEvent, ProposedEvent, ExpectedVersion, etc.
  error.rs         -- Error enum (thiserror), maps to gRPC status codes
  store.rs         -- storage engine: append, read_stream, read_all, startup recovery
  writer.rs        -- single writer task: mpsc receiver, batching, fsync, index update
  broker.rs        -- broadcast channel for live subscriptions
  service.rs       -- tonic gRPC service impl (Append, ReadStream, ReadAll, SubscribeAll, SubscribeStream)
  proto/           -- .proto file(s) and build.rs output

tests/             -- integration tests: full gRPC client-server round-trips
docs/
  design.md        -- authoritative design document (read this first)
```

### Data flow

1. gRPC `Append` handler validates the request and sends it to the writer task via a bounded `tokio::mpsc` channel.
2. The writer task drains the channel, validates expected versions, serializes records to the append-only log file, fsyncs, updates the in-memory index, notifies the broadcast channel, and responds via oneshot.
3. Reads (`ReadStream`, `ReadAll`) go directly to the in-memory index -- no locking, no disk I/O.
4. Subscriptions (`SubscribeAll`, `SubscribeStream`) replay from the in-memory index (catch-up), send a `CaughtUp` marker, then forward from the broadcast channel (live).

### Key data structures

- `Vec<RecordedEvent>` -- global log, index `i` = global position `i` (contiguous, zero-based)
- `HashMap<Uuid, Vec<u64>>` -- stream ID to list of global positions, index `j` = stream version `j` (contiguous, zero-based)

## Commands

- `cargo build` -- compile the project
- `cargo test` -- run all unit and integration tests
- `cargo clippy --all-targets --all-features --locked -- -D warnings` -- lint (must pass clean)
- `cargo fmt --check` -- formatting check
- `cargo run` -- start the server (requires `EVENTFOLD_DATA` env var)

## Conventions

### Domain rules

- Stream IDs and event IDs are UUIDs (v4 or v7). The server validates format on append.
- Maximum event size is 64 KB (payload + metadata + fixed fields). Large blobs belong in external storage (S3, etc.); events carry references, not files.
- Event type tags are UTF-8 strings, max 256 bytes.
- Payload = domain event body (the facts). Metadata = infrastructure context (correlation ID, causation ID, client timestamp, user identity). Both are opaque bytes to the store.
- The server assigns `recorded_at` (Unix epoch milliseconds, u64) to every event at append time. All events in a single batch share the same `recorded_at`. Ordering still uses global position and stream version; `recorded_at` is informational.
- Optimistic concurrency is whole-stream, not field-level. On version conflict, the caller re-reads, re-evaluates, and retries.

### Code style

- Use the `/rust-best-practices` skill when writing, reviewing, or refactoring Rust code.
- snake_case for functions/variables/modules, PascalCase for types/traits, SCREAMING_SNAKE_CASE for constants.
- All public items must have doc comments.
- No `.unwrap()` in library code. Use `.expect()` only for invariant violations with a descriptive message.
- All fallible operations return `Result<T, eventfold_db::Error>`.
- Prefer borrowing over ownership. Use `&str` over `String` where possible.
- No wildcard imports (`use module::*`) except in `#[cfg(test)]` modules (`use super::*`).
- One responsibility per module/file.
- Public API types live in `types.rs` and are re-exported from `lib.rs`.

### Error handling

- Use `thiserror` for the error enum. Variants: `WrongExpectedVersion`, `StreamNotFound`, `Io`, `CorruptRecord`, `InvalidHeader`, `EventTooLarge`, `InvalidArgument`.
- gRPC layer maps errors to status codes: `FAILED_PRECONDITION`, `NOT_FOUND`, `INTERNAL`, `DATA_LOSS`, `INVALID_ARGUMENT`.
- Panics are reserved for programmer errors (violated invariants), never operational failures.

### Testing (red/green TDD)

Every module is developed using strict red/green TDD:

1. Write a failing test that describes the expected behavior.
2. Write the minimum code to make it pass.
3. Refactor while keeping all tests green.

Unit tests live in `#[cfg(test)]` modules alongside the code. Integration tests live in `tests/`. Use `tempfile::tempdir()` for test isolation -- never write to fixed paths.

### Verification before completion

Before any ticket or feature is considered complete, all of the following must pass:

```sh
cargo build 2>&1 | tail -1          # no errors
cargo clippy --all-targets --all-features --locked -- -D warnings  # no warnings
cargo test                           # all green
cargo fmt --check                    # formatted
```

Run `cargo clippy --all-targets --all-features --locked -- -D warnings` frequently during development, not just at the end.

## Landmines / Gotchas

- **Single writer task.** gRPC handlers never touch the log file or mutate the index directly. All writes go through the `tokio::mpsc` channel to the writer task. Violating this breaks durability and concurrency guarantees.
- **Subscribe registers before reading history.** The broadcast channel subscription must be established *before* the catch-up read begins. Reversing this order creates a race where events appended between the end of catch-up and the start of live listening are lost.
- **Contiguous positions.** Global positions are 0, 1, 2, ... with no gaps. Stream versions are 0, 1, 2, ... with no gaps. The in-memory index relies on this for direct indexing (`events[position]`). If this invariant breaks, reads return wrong data silently.
- **Partial trailing record on crash.** The startup recovery path must detect and truncate incomplete records at the tail of the log file. This is the critical correctness boundary -- test it thoroughly.
- **fsync is non-negotiable.** Every batch of appended records must be fsynced before the writer responds to callers. Without fsync, acknowledged events can be lost on power failure.
- **Broadcast channel cloning.** `tokio::broadcast` clones every message to every receiver. Use `Arc<RecordedEvent>` in the broadcast channel to share the allocation instead of cloning event data.
- **Edition is 2024.** The design doc references 2021 edition but `Cargo.toml` uses the 2024 edition. Follow `Cargo.toml`.

## Agent Workflow File Structure

All agent artifacts live under `/prd/`. The expected structure for a feature:

```
/prd/
  NNN-feature-name.md                    # PRD (source of truth)
  NNN-feature-name-tickets.md            # Ticket breakdown
  NNN-feature-name-status.md             # Orchestrator's live status tracker
  NNN-feature-name-qa.md                 # Final QA report
  NNN-feature-name-reports/              # Per-ticket reports
    ticket-01-impl.md                    # Implementer completion report
    ticket-01-review.md                  # Code review result
    ticket-02-impl.md
    ticket-02-review.md
```

### Agent rules

- Follow red/green TDD: write the failing test first, then the implementation, then refactor.
- `cargo clippy --all-targets --all-features --locked -- -D warnings` must pass before a ticket is marked complete.
- `cargo test` must pass before a ticket is marked complete.
- `cargo build` must produce zero warnings before a ticket is marked complete.
- Do not mark a ticket complete if any of these checks fail. Report the failure and fix it.
- The design doc (`docs/design.md`) is the authoritative specification. When in doubt, defer to it.
