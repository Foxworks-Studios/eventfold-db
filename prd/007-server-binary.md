# PRD 007: Server Binary and Configuration

**Status:** TICKETS READY

## Summary

Wire everything together into the final `main.rs` binary: read configuration from environment variables, open the storage engine, spawn the writer task and broker, start the gRPC server, and handle graceful shutdown. This is the thin orchestration layer that composes all previous modules into a running server.

## Motivation

The binary is the deployable artifact. It must start quickly, log its configuration, and shut down cleanly when signaled. Configuration via environment variables follows 12-factor principles and works naturally with container deployments (Fly.io, Docker).

## Scope

### In scope

- `main.rs`: async main function that bootstraps the server.
- Configuration from environment variables: `EVENTFOLD_DATA`, `EVENTFOLD_LISTEN`, `EVENTFOLD_BROKER_CAPACITY`.
- Tracing/logging initialization.
- Graceful shutdown on SIGINT/SIGTERM.
- Health logging on startup (data path, listen address, broker capacity, recovered event count).

### Out of scope

- Dockerfile (can be added later without a PRD).
- TLS configuration (future consideration).
- Metrics or health check endpoints (future consideration).

## Detailed Design

### Configuration

| Env Var                    | Required | Default        | Description                          |
|----------------------------|----------|----------------|--------------------------------------|
| `EVENTFOLD_DATA`           | Yes      | --             | Path to the append-only log file     |
| `EVENTFOLD_LISTEN`         | No       | `[::]:2113`    | Socket address to listen on          |
| `EVENTFOLD_BROKER_CAPACITY`| No       | `4096`         | Broadcast channel ring buffer size   |

If `EVENTFOLD_DATA` is not set, the server prints an error and exits with a non-zero status code.

### Startup Sequence

1. Initialize `tracing_subscriber` (with env filter, e.g., `RUST_LOG=info`).
2. Read configuration from environment variables.
3. Log configuration values (data path, listen address, broker capacity).
4. Open the `Store` at the data path. Log the number of recovered events and streams.
5. Create the `Broker` with the configured capacity.
6. Spawn the writer task. Obtain the `WriterHandle` and `ReadIndex`.
7. Build the `EventfoldService` with the writer handle, read index, and broker.
8. Build the tonic `Server` with the service.
9. Start serving on the configured listen address.
10. Log "Server listening on {address}".
11. Await shutdown signal (SIGINT or SIGTERM on Unix).
12. On signal: log "Shutting down", drop the writer handle (closes the channel), await the writer task's join handle, then exit.

### Graceful Shutdown

The server uses `tokio::signal::ctrl_c()` (or `tokio::signal::unix::signal(SignalKind::terminate())` on Unix) to detect shutdown requests. On signal:

1. The gRPC server stops accepting new connections.
2. In-flight RPCs are allowed to complete (tonic's built-in graceful shutdown).
3. The `WriterHandle` is dropped, signaling the writer task to drain and exit.
4. The writer task's `JoinHandle` is awaited.
5. The process exits with code 0.

### Logging

Use `tracing` for all log output. Key log points:

- `info!` on startup: data path, listen address, broker capacity, recovered events/streams.
- `info!` when server starts listening.
- `info!` on shutdown signal received.
- `warn!` if partial record truncated during recovery (from store).
- `error!` if startup fails (bad header, mid-file corruption, I/O error).

## Acceptance Criteria

### AC-1: Server starts with valid config

- **Test**: Set `EVENTFOLD_DATA` to a temp directory path and `EVENTFOLD_LISTEN` to `127.0.0.1:0` (ephemeral port). Start the server. It binds to a port and accepts gRPC connections. A client can call `ReadAll` and get an empty response.

### AC-2: Server requires EVENTFOLD_DATA

- **Test**: Unset `EVENTFOLD_DATA`. Start the server. It exits with a non-zero status code and prints an error message containing "EVENTFOLD_DATA".

### AC-3: Default listen address

- **Test**: Set only `EVENTFOLD_DATA`. Start the server. It binds to `[::]:2113` by default.

### AC-4: Default broker capacity

- **Test**: Set only `EVENTFOLD_DATA` and `EVENTFOLD_LISTEN`. Start the server. The broker uses the default capacity of 4096. (This is implicitly verified by subscription tests working correctly.)

### AC-5: Custom configuration

- **Test**: Set `EVENTFOLD_LISTEN=127.0.0.1:9999` and `EVENTFOLD_BROKER_CAPACITY=16`. Start the server. It binds to `127.0.0.1:9999`. The broker capacity is 16 (verifiable by saturating the broadcast channel with 17 events and observing lag).

### AC-6: Recovery on restart

- **Test**: Start the server with a temp data path. Append 5 events via gRPC. Stop the server. Start a new server at the same data path. `ReadAll` returns all 5 events. Appending continues from the correct position.

### AC-7: Graceful shutdown

- **Test**: Start the server. Send SIGINT (or drop the server handle in test code). The process exits cleanly without panic. No data is lost -- events appended before the signal are durable.

### AC-8: End-to-end round-trip

- **Test**: Start the server. Via a gRPC client:
  1. Append 3 events to stream A with `no_stream`.
  2. Append 2 events to stream B with `no_stream`.
  3. `ReadStream` for A -- returns 3 events.
  4. `ReadStream` for B -- returns 2 events.
  5. `ReadAll` from 0 -- returns 5 events in order.
  6. `SubscribeAll` from 0 -- receives 5 events + `CaughtUp`.
  This is the smoke test that the full stack works.

### AC-9: Build and lint

- `cargo build` completes with zero warnings.
- `cargo clippy --all-targets --all-features --locked -- -D warnings` passes.
- `cargo fmt --check` passes.
- `cargo test` passes with all tests green.

### AC-10: Binary runs

- **Test**: `cargo run --help` or a basic invocation demonstrates the binary is functional. (The binary may not support `--help` -- this criterion is satisfied by AC-1 and AC-8.)

## Dependencies

- **Depends on**: PRD 001 (types), PRD 002 (codec), PRD 003 (store), PRD 004 (writer), PRD 005 (broker), PRD 006 (gRPC service).
- **Depended on by**: Nothing (final PRD).

## Cargo.toml Additions

```toml
[dependencies]
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```
