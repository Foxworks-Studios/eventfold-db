# Tickets for PRD 007: Server Binary and Configuration

**Source PRD:** prd/007-server-binary.md
**Created:** 2026-02-26
**Total Tickets:** 4
**Estimated Total Complexity:** 7 (S=1 + M=2 + M=2 + M=2)

---

## Context

PRDs 001-006 are fully implemented. The library crate is complete: `Store`, `Broker`,
`spawn_writer`, `EventfoldService`, and all five gRPC RPCs are working. `src/main.rs` is
currently a placeholder `fn main() { println!("Hello, world!"); }`.

This PRD replaces that placeholder with the production startup sequence. The only new
dependency is `tracing-subscriber`. All work is in `src/main.rs` and
`tests/server_binary.rs`.

---

### Ticket 1: Add `tracing-subscriber` Dependency and `Config` Struct

**Description:**
Add `tracing-subscriber` (with `env-filter` feature) to `Cargo.toml` and create a
`Config` struct in `src/main.rs` (or a small `src/config.rs` module) that reads the
three environment variables from the PRD. This is the foundational ticket: the
`tracing_subscriber` init and config parsing must compile before the startup sequence
(Ticket 2) can be written.

**Scope:**
- Modify: `Cargo.toml` (add `tracing-subscriber = { version = "0.3", features =
  ["env-filter"] }` under `[dependencies]`)
- Modify: `src/main.rs` (replace placeholder; add `Config` struct with fields
  `data_path: PathBuf`, `listen_addr: SocketAddr`, `broker_capacity: usize`; add
  `Config::from_env() -> Result<Config, String>` that reads env vars and applies
  defaults; add `init_tracing()` helper that calls
  `tracing_subscriber::EnvFilter::from_default_env()` and initializes the global
  subscriber)

**Acceptance Criteria:**
- [ ] `Cargo.toml` `[dependencies]` contains `tracing-subscriber` with the `env-filter`
  feature
- [ ] `Config` struct has three fields: `data_path: std::path::PathBuf`,
  `listen_addr: std::net::SocketAddr`, `broker_capacity: usize`
- [ ] `Config::from_env()` returns `Err(String)` containing `"EVENTFOLD_DATA"` when
  `EVENTFOLD_DATA` is not set
- [ ] `Config::from_env()` uses `[::]:2113` as the default `listen_addr` when
  `EVENTFOLD_LISTEN` is not set
- [ ] `Config::from_env()` uses `4096` as the default `broker_capacity` when
  `EVENTFOLD_BROKER_CAPACITY` is not set
- [ ] `Config::from_env()` returns `Err(String)` when `EVENTFOLD_LISTEN` is set to
  a value that is not a valid `SocketAddr`
- [ ] `Config::from_env()` returns `Err(String)` when `EVENTFOLD_BROKER_CAPACITY` is
  set to a value that is not a valid `usize`
- [ ] `init_tracing()` initializes a `tracing_subscriber` with `EnvFilter` from
  the `RUST_LOG` environment variable (or a default of `"info"`) and sets the global
  default; the function is idempotent with respect to compilation (calling it does
  not panic on the first call in a test process)
- [ ] Test: set `EVENTFOLD_DATA` to `"/tmp/x"` in test env; call `Config::from_env()`;
  assert `Ok(config)` with `config.data_path == PathBuf::from("/tmp/x")`,
  `config.listen_addr == "[::]:2113".parse().unwrap()`,
  `config.broker_capacity == 4096`
- [ ] Test: unset `EVENTFOLD_DATA`; call `Config::from_env()`; assert `Err(msg)` where
  `msg.contains("EVENTFOLD_DATA")`
- [ ] Test: set `EVENTFOLD_DATA="/tmp/x"` and `EVENTFOLD_LISTEN="127.0.0.1:9999"`;
  call `Config::from_env()`; assert `config.listen_addr ==
  "127.0.0.1:9999".parse().unwrap()`
- [ ] Test: set `EVENTFOLD_DATA="/tmp/x"` and `EVENTFOLD_BROKER_CAPACITY="16"`; call
  `Config::from_env()`; assert `config.broker_capacity == 16`
- [ ] Test: set `EVENTFOLD_LISTEN="not-an-addr"`; call `Config::from_env()`; assert
  `Err(msg)`
- [ ] Quality gates pass: `cargo build`, `cargo clippy --all-targets --all-features
  --locked -- -D warnings`, `cargo fmt --check`, `cargo test`

**Dependencies:** None (all library modules are already complete)
**Complexity:** S
**Maps to PRD AC:** AC-2, AC-3, AC-4, AC-5

---

### Ticket 2: Implement `main.rs` Startup Sequence and Graceful Shutdown

**Description:**
Replace the stub `main` with the full async `#[tokio::main]` entrypoint that implements
the 12-step startup sequence from the PRD: init tracing, parse config, log config
values, open `Store`, create `Broker`, spawn writer task, build `EventfoldService`,
build tonic `Server`, bind on the configured address, log "Server listening on
{address}", and await shutdown (SIGINT on all platforms; also SIGTERM on Unix). On
shutdown: log "Shutting down", drop the `WriterHandle`, await the writer `JoinHandle`,
then return.

**Scope:**
- Modify: `src/main.rs` (implement `async fn main()` with `#[tokio::main]`; use
  `Config::from_env()` from Ticket 1; call `init_tracing()`; call `Store::open()`;
  call `Broker::new()`; call `spawn_writer()`; construct `EventfoldService::new()`;
  build `tonic::transport::Server`; bind with `tokio::net::TcpListener`; use
  `serve_with_incoming` with a `TcpListenerStream`; add shutdown signal handling via
  `tokio::signal`)

**Acceptance Criteria:**
- [ ] `main` is declared `#[tokio::main] async fn main()` and calls `init_tracing()`,
  `Config::from_env()`, `Store::open()`, `Broker::new()`, `spawn_writer()`,
  `EventfoldService::new()`, and starts the tonic server
- [ ] If `Config::from_env()` returns `Err(msg)`, the process prints `msg` to stderr
  and exits with code 1 (use `eprintln!` + `std::process::exit(1)`)
- [ ] If `Store::open()` returns `Err(e)`, the process logs `tracing::error!` and
  exits with code 1
- [ ] Startup emits `tracing::info!` lines for: data path, listen address, broker
  capacity, recovered event count (from `read_index.event_count()`), and recovered
  stream count (from `read_index.stream_count()`)
- [ ] The server binds using `tokio::net::TcpListener::bind(config.listen_addr)` and
  passes the listener to `serve_with_incoming` so an OS-assigned port (`0`) works
  correctly in tests
- [ ] After `serve_with_incoming` begins, emits `tracing::info!("Server listening on
  {addr}")` where `addr` is the actual bound address from
  `listener.local_addr()`
- [ ] Graceful shutdown: on SIGINT (`tokio::signal::ctrl_c()`) or SIGTERM (Unix:
  `tokio::signal::unix::signal(SignalKind::terminate())`), log `tracing::info!
  ("Shutting down")`, drop the `WriterHandle`, and `join_handle.await` the writer
  task
- [ ] On non-Unix platforms (e.g. Windows) the code compiles without errors; SIGTERM
  handling is conditionally compiled with `#[cfg(unix)]`
- [ ] Test (compile-time): `cargo build --bin eventfold-db` produces zero warnings
  and zero errors
- [ ] Test: run `cargo run --bin eventfold-db` without `EVENTFOLD_DATA` set; assert
  the process exits with non-zero status and stderr contains `"EVENTFOLD_DATA"`
  (this can be verified by `std::process::Command` in an integration test or
  manually)
- [ ] Quality gates pass: `cargo build`, `cargo clippy --all-targets --all-features
  --locked -- -D warnings`, `cargo fmt --check`, `cargo test`

**Dependencies:** Ticket 1
**Complexity:** M
**Maps to PRD AC:** AC-1, AC-2, AC-3, AC-4, AC-5, AC-7, AC-9, AC-10

---

### Ticket 3: Integration Tests for Server Binary Startup, Config, Recovery, and Round-Trip

**Description:**
Create `tests/server_binary.rs` with integration tests that start the server binary
in-process (by calling the same helpers from the existing `start_test_server` pattern
but wired through the `Config`-driven path), verify AC-1, AC-5, AC-6, AC-8, and confirm
that the full stack works end-to-end. The key new tests here are: the server starts
with valid config, the default listen address is used when `EVENTFOLD_LISTEN` is unset,
recovery on restart, and the AC-8 end-to-end round-trip.

**Scope:**
- Create: `tests/server_binary.rs` (new integration test file with a helper
  `start_server_at(data_path, listen_addr, broker_capacity)` that calls
  `Store::open`, `Broker::new`, `spawn_writer`, `EventfoldService::new`, binds the
  tonic server, and returns a connected client + `TempDir`; contains tests for
  AC-1, AC-5, AC-6, AC-8)

**Acceptance Criteria:**
- [ ] `start_server_at` helper in `tests/server_binary.rs` accepts a `&Path` (data
  path), `SocketAddr` (listen addr), and `usize` (broker capacity); opens a real
  `Store`, spawns the full stack, and returns
  `(EventStoreClient<Channel>, TempDir)` (TempDir returned to keep it alive)
- [ ] Test (AC-1): call `start_server_at` with a tempdir path and `[::1]:0`; send a
  `ReadAll` request from position 0; assert response has 0 events and the RPC
  succeeds (proves the server accepted a gRPC connection)
- [ ] Test (AC-5 custom config): call `start_server_at` with broker capacity 16;
  append 20 events without consuming subscription responses; start a
  `SubscribeAll`; poll it; assert the stream eventually terminates or returns an
  error (proves the small broker capacity 16 causes lag with 20 unread events)
- [ ] Test (AC-6 recovery): open a `Store` at a fixed tempdir path and append 5
  events via the gRPC client; drop the server (via `JoinHandle` cancellation or
  by dropping the `TempDir` guard's path while keeping the dir alive); open a new
  `Store` at the same path, start a new server; call `ReadAll` from position 0;
  assert 5 events are returned and that `Append` returns `first_global_position ==
  5` for a new event (proves recovery positions are correct)
- [ ] Test (AC-8 end-to-end round-trip): append 3 events to stream A with
  `no_stream`; append 2 events to stream B with `no_stream`; call `ReadStream` for
  A (from version 0) and assert 3 events; call `ReadStream` for B (from version 0)
  and assert 2 events; call `ReadAll` from position 0 and assert 5 events in
  global_position order 0..4; start `SubscribeAll` from position 0 and collect
  until `CaughtUp`, asserting 5 events were received
- [ ] Test (EVENTFOLD_DATA missing): run the server binary as a subprocess via
  `std::process::Command::new("cargo").args(["run", "--bin", "eventfold-db"])`
  without `EVENTFOLD_DATA` in the env; wait for exit; assert the exit status is
  non-zero and stderr contains `"EVENTFOLD_DATA"`
- [ ] All existing tests in `tests/grpc_service.rs` continue to pass without
  modification
- [ ] Quality gates pass: `cargo build`, `cargo clippy --all-targets --all-features
  --locked -- -D warnings`, `cargo fmt --check`, `cargo test`

**Dependencies:** Ticket 2
**Complexity:** M
**Maps to PRD AC:** AC-1, AC-2, AC-5, AC-6, AC-8

---

### Ticket 4: Verification and Integration

**Description:**
Run the complete PRD 007 acceptance criteria checklist end-to-end. Confirm the binary
compiles cleanly, all new and existing tests pass, config parsing behaves correctly
under all specified scenarios, recovery works across restarts, graceful shutdown does
not lose data, and the AC-8 multi-stream round-trip smoke test passes. This ticket
adds any remaining gap tests and confirms the full PRD is satisfied.

**Scope:**
- Modify: `tests/server_binary.rs` (add final gap-filling test: AC-7 graceful
  shutdown -- start server, append 1 event, drop the `WriterHandle` to trigger
  writer shutdown, then re-open at same path and verify event is durable)

**Acceptance Criteria:**
- [ ] All PRD 007 ACs (AC-1 through AC-10) pass via `cargo test`
- [ ] Test (AC-7 graceful shutdown durability): open store at temp path; append 1
  event via `WriterHandle::append`; drop the `WriterHandle` (closes writer task
  channel); await the `JoinHandle`; re-open a new `Store` at the same path; call
  `read_all(0, 10)` and assert 1 event is returned -- proves events are durable
  before the handle is dropped
- [ ] `cargo test` output shows zero failures across the full crate (all PRDs
  001-007 tests green)
- [ ] `cargo build --bin eventfold-db` produces zero warnings
- [ ] `cargo clippy --all-targets --all-features --locked -- -D warnings` passes
  with zero diagnostics
- [ ] `cargo fmt --check` passes
- [ ] The binary can be started manually: `EVENTFOLD_DATA=/tmp/test.log
  EVENTFOLD_LISTEN=127.0.0.1:2113 cargo run` starts without error (smoke-checked
  by the implementer)

**Dependencies:** Tickets 1, 2, 3
**Complexity:** M
**Maps to PRD AC:** AC-7, AC-9, AC-10

---

## AC Coverage Matrix

| PRD AC # | Description                                                                 | Covered By Ticket(s)       | Status  |
|----------|-----------------------------------------------------------------------------|----------------------------|---------|
| AC-1     | Server starts with valid config; accepts gRPC connections; ReadAll works    | Ticket 2, Ticket 3         | Covered |
| AC-2     | Missing EVENTFOLD_DATA -> non-zero exit + error message containing field name | Ticket 1, Ticket 2, Ticket 3 | Covered |
| AC-3     | Default listen address is `[::]:2113` when EVENTFOLD_LISTEN is unset        | Ticket 1, Ticket 2         | Covered |
| AC-4     | Default broker capacity is 4096 when EVENTFOLD_BROKER_CAPACITY is unset     | Ticket 1, Ticket 2         | Covered |
| AC-5     | Custom config: EVENTFOLD_LISTEN and EVENTFOLD_BROKER_CAPACITY are respected | Ticket 1, Ticket 3         | Covered |
| AC-6     | Recovery on restart: 5 appended events survive a server restart             | Ticket 3                   | Covered |
| AC-7     | Graceful shutdown: process exits cleanly; events appended before signal are durable | Ticket 4            | Covered |
| AC-8     | End-to-end round-trip: Append/ReadStream/ReadAll/SubscribeAll/SubscribeStream all work | Ticket 3            | Covered |
| AC-9     | Build and lint: zero warnings, clippy clean, fmt, all tests green           | Ticket 4                   | Covered |
| AC-10    | Binary runs: functional binary (satisfied by AC-1 and AC-8)                 | Ticket 2, Ticket 3, Ticket 4 | Covered |
