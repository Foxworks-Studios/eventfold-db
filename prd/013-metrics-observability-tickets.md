# Tickets for PRD 013: Metrics / Observability

**Source PRD:** prd/013-metrics-observability.md
**Created:** 2026-02-27
**Total Tickets:** 5
**Estimated Total Complexity:** 11 (S=1, M=2, L=3)

---

### Ticket 1: Add metrics dependencies and `src/metrics.rs` module

**Description:**
Add `metrics`, `metrics-exporter-prometheus`, and `axum` to `Cargo.toml`, then create
`src/metrics.rs` with `MetricsHandle`, `MetricsError`, `install_recorder()`, and
`serve_metrics()`. Register `pub mod metrics;` in `src/lib.rs`. This is the foundational
module that every other ticket builds on — nothing else compiles until this exists.

**Scope:**
- Modify: `Cargo.toml` (add `metrics = "0.24"`, `metrics-exporter-prometheus = { version =
  "0.16", default-features = false, features = ["http-listener"] }`, `axum = { version =
  "0.8", default-features = false, features = ["tokio", "http1"] }`)
- Create: `src/metrics.rs`
- Modify: `src/lib.rs` (add `pub mod metrics;`)

**Acceptance Criteria:**
- [ ] `Cargo.toml` `[dependencies]` contains exactly the three new entries listed above with
  the specified version constraints and feature flags.
- [ ] `src/metrics.rs` defines `MetricsError` as a `thiserror`-derived enum with at least
  variant `AlreadyInstalled` and derives `Debug`.
- [ ] `src/metrics.rs` defines `MetricsHandle` as a public struct wrapping
  `Arc<metrics_exporter_prometheus::PrometheusHandle>` (or equivalent); it must be
  `Clone + Send + Sync`.
- [ ] `install_recorder() -> Result<MetricsHandle, MetricsError>` installs a
  `PrometheusRecorder` via `PrometheusBuilder::new().install_recorder()`. Returns
  `MetricsError::AlreadyInstalled` on a second call in the same process (guarded with
  `std::sync::OnceLock` or equivalent to avoid double-installation panics).
- [ ] `serve_metrics(handle: MetricsHandle, addr: SocketAddr) -> tokio::task::JoinHandle<()>`
  spawns a tokio task binding an axum `Router` with route `GET /metrics` that returns
  `handle.render()` with `Content-Type: text/plain; version=0.0.4`. Binds with
  `tokio::net::TcpListener`. On bind failure, logs `tracing::error!` and returns a
  `JoinHandle` that resolves immediately. On success, logs `tracing::info!` with the
  bound address.
- [ ] `serve_metrics` calls `tokio::spawn` internally — it does NOT construct a new
  `tokio::Runtime`. (Verified structurally: no `Runtime::new()` or `Builder::new_*` call
  in `metrics.rs`.)
- [ ] `src/lib.rs` has `pub mod metrics;` so `eventfold_db::metrics` is reachable.
- [ ] Test: in `#[cfg(test)]` in `src/metrics.rs`, call `install_recorder()` twice in the
  same test; assert the first call returns `Ok(_)` and the second call returns
  `Err(MetricsError::AlreadyInstalled)`.
- [ ] Test: bind `serve_metrics` on `127.0.0.1:0` (port 0 = ephemeral); assert the returned
  `JoinHandle` does not complete immediately (i.e., the task is running). Use
  `tokio::time::timeout(Duration::from_millis(20), handle).await` and assert the timeout
  fires (meaning the server is still alive).
- [ ] `cargo build` produces zero warnings after this ticket.
- [ ] `cargo clippy --all-targets --all-features --locked -- -D warnings` passes.

**Dependencies:** None
**Complexity:** L
**Maps to PRD AC:** AC 1, AC 8, AC 10

---

### Ticket 2: Add `Store::log_file_len()` and instrument `run_writer` with metrics

**Description:**
Add the `log_file_len()` method to `src/store.rs` (a `stat(2)` call on the log file),
then instrument the `run_writer` loop in `src/writer.rs` with the five writer-side
metrics: `eventfold_appends_total`, `eventfold_events_total`,
`eventfold_append_duration_seconds`, `eventfold_global_position`, and
`eventfold_streams_total` and `eventfold_log_bytes`. All metric updates happen after
`store.append()` returns `Ok` — failed appends do not update any counter.

**Scope:**
- Modify: `src/store.rs` (add `pub fn log_file_len(&self) -> Result<u64, Error>`)
- Modify: `src/writer.rs` (add `std::time::Instant` pair around `store.append()` call;
  fire `counter!`, `gauge!`, `histogram!` macros on success)

**Acceptance Criteria:**
- [ ] `Store::log_file_len(&self) -> Result<u64, Error>` is a public method that calls
  `self.file.metadata()` (or `self.file.metadata().map(|m| m.len())`) and returns the
  file length. Returns `Err(Error::Io(_))` on failure.
- [ ] `Store::log_file_len` has a doc comment with `# Arguments`, `# Returns`, and
  `# Errors` sections following project doc comment style.
- [ ] Test (in `src/store.rs` `#[cfg(test)]`): open a fresh store at a tempdir; call
  `log_file_len()`; assert it returns `Ok(n)` where `n >= 8` (at minimum the 8-byte
  file header is written on creation).
- [ ] Test (in `src/store.rs` `#[cfg(test)]`): append one event via `store.append()`,
  then call `log_file_len()` a second time; assert the second result is strictly
  greater than the first (file grew after append).
- [ ] `run_writer` takes a `Instant::now()` immediately before `store.append(req.stream_id,
  ...)` and records `elapsed()` immediately after it returns `Ok`. The histogram name is
  `"eventfold_append_duration_seconds"`. No timing is recorded for failed appends.
- [ ] After a successful `store.append()`, `run_writer` calls `counter!("eventfold_appends_total", 1)` once per request (not once per event).
- [ ] After a successful `store.append()`, `run_writer` calls `counter!("eventfold_events_total", recorded.len() as u64)`.
- [ ] After a successful `store.append()`, `run_writer` reads the stream count from the
  `EventLog` (via `store.log()` read lock) and calls
  `gauge!("eventfold_streams_total", count as f64)`.
- [ ] After a successful `store.append()`, `run_writer` calls `store.log_file_len()` and
  calls `gauge!("eventfold_log_bytes", len as f64)`. If `log_file_len()` returns `Err`,
  logs `tracing::warn!` and skips the gauge update (does not panic).
- [ ] After a successful `store.append()`, `run_writer` sets
  `gauge!("eventfold_global_position", (recorded.last().global_position + 1) as f64)`.
- [ ] Test (in `src/writer.rs` `#[cfg(test)]`): in a `#[tokio::test]` that calls
  `metrics::install_recorder()` (or relies on the global recorder already set by a prior
  test), append 3 events via `spawn_writer`; after all appends succeed, render the
  metrics via `handle.render()`; assert the rendered string contains
  `eventfold_appends_total` with value `3` and `eventfold_events_total` with value `3`.
  Note: because `install_recorder()` is process-global, use `metrics::try_recorder()` or
  wrap the call to be idempotent. (Implementer: see `MetricsError::AlreadyInstalled`
  guard from Ticket 1.)
- [ ] `cargo build` produces zero warnings after this ticket.
- [ ] `cargo clippy --all-targets --all-features --locked -- -D warnings` passes.

**Dependencies:** Ticket 1
**Complexity:** M
**Maps to PRD AC:** AC 2, AC 3, AC 6

---

### Ticket 3: Instrument `service.rs` with read and subscription metrics

**Description:**
Add the three service-side metrics to `src/service.rs`:
`eventfold_reads_total{rpc="read_stream"}`, `eventfold_reads_total{rpc="read_all"}`,
and the `eventfold_subscriptions_active` gauge that increments when a subscription
stream starts and decrements when it ends. This ticket modifies only `src/service.rs`.

**Scope:**
- Modify: `src/service.rs` (add `counter!` in `read_stream` and `read_all` handlers;
  add `gauge!` increment/decrement in `subscribe_all` and `subscribe_stream`)

**Acceptance Criteria:**
- [ ] In `read_stream`, a `counter!("eventfold_reads_total", 1, "rpc" => "read_stream")`
  call appears at the top of the handler body, before any early-return on error
  (i.e., it fires on every call regardless of outcome).
- [ ] In `read_all`, a `counter!("eventfold_reads_total", 1, "rpc" => "read_all")` call
  appears at the top of the handler body, before any early-return on error.
- [ ] In `subscribe_all`, `gauge!("eventfold_subscriptions_active", 1.0_f64)` is called
  at the start of the handler and `gauge!("eventfold_subscriptions_active", -1.0_f64)`
  is called after the stream finishes (i.e., after the `async_stream::stream!` block
  yields its last item or returns). Use a deferred decrement pattern: emit the increment
  before yielding any items, and emit the decrement in the stream's terminal path.
  A `scopeguard` or an explicit post-loop statement is acceptable; either approach must
  guarantee the decrement fires even when the client disconnects mid-stream.
- [ ] Same pattern as above for `subscribe_stream`.
- [ ] Test (in `src/service.rs` `#[cfg(test)]`): construct a minimal `EventfoldService`
  backed by a `temp_store()` + `spawn_writer` + `Broker`; call the `read_stream` handler
  once (with a valid but empty stream to avoid `StreamNotFound`) then `read_all` twice;
  render the metrics; assert the rendered string contains
  `eventfold_reads_total{rpc="read_stream"} 1` and
  `eventfold_reads_total{rpc="read_all"} 2`.
- [ ] Test (subscription gauge): start a `subscribe_all` call; assert `eventfold_subscriptions_active`
  is `1` (by rendering metrics while the stream is open); drop the stream and await
  its completion; assert `eventfold_subscriptions_active` is `0`. Use a tokio task and
  a `oneshot` to observe the gauge at the right moment.
- [ ] `cargo build` produces zero warnings.
- [ ] `cargo clippy --all-targets --all-features --locked -- -D warnings` passes.

**Dependencies:** Ticket 1
**Complexity:** M
**Maps to PRD AC:** AC 4, AC 5

---

### Ticket 4: Wire metrics into `main.rs` (`Config`, `install_recorder`, `serve_metrics`)

**Description:**
Extend `Config` in `src/main.rs` with a `metrics_listen: Option<SocketAddr>` field
parsed from `EVENTFOLD_METRICS_LISTEN`. Default is `Some("[::]:9090")`. An empty string
disables the endpoint. After `spawn_writer`, call `metrics::install_recorder()`, and if
`config.metrics_listen` is `Some(addr)` call `metrics::serve_metrics(handle, addr)`.
Store the `JoinHandle` and abort it in the shutdown sequence.

**Scope:**
- Modify: `src/main.rs` (add `metrics_listen` field to `Config`; parse
  `EVENTFOLD_METRICS_LISTEN`; add recorder install + conditional serve call; abort
  handle during shutdown; update doc comment table; update existing `#[cfg(test)]` env
  cleanup to also clear `EVENTFOLD_METRICS_LISTEN`)

**Acceptance Criteria:**
- [ ] `Config` struct has field `metrics_listen: Option<SocketAddr>`.
- [ ] `Config::from_env()` reads `EVENTFOLD_METRICS_LISTEN`:
  - Not set or set to any valid `SocketAddr` string: `Some(addr)` where the default is
    `"[::]:9090".parse().unwrap()` when the variable is absent.
  - Set to an empty string `""`: `None`.
  - Set to an invalid (non-empty) string: returns `Err(String)` with a message
    mentioning `EVENTFOLD_METRICS_LISTEN`.
- [ ] The doc comment table on `Config` includes a row for `EVENTFOLD_METRICS_LISTEN`
  documenting default and meaning.
- [ ] In `main()`, after the `spawn_writer` call (step 6 in the existing sequence), the
  code calls `eventfold_db::metrics::install_recorder()`. On `Err`, logs
  `tracing::error!` and calls `std::process::exit(1)`.
- [ ] If `config.metrics_listen` is `Some(addr)`, the code calls
  `eventfold_db::metrics::serve_metrics(handle, addr)` and stores the resulting
  `JoinHandle` in a local variable.
- [ ] If `config.metrics_listen` is `None`, the code logs `tracing::info!("Metrics endpoint disabled")` and does not call `serve_metrics`.
- [ ] In the shutdown sequence (after the gRPC server exits), the metrics
  `JoinHandle` is aborted: `metrics_join_handle.abort()`. If the endpoint was disabled
  (`None`), no abort is needed.
- [ ] Test (`from_env_metrics_listen_default`): with `EVENTFOLD_METRICS_LISTEN` unset and
  `EVENTFOLD_DATA` set, `Config::from_env()` returns `Ok` with
  `config.metrics_listen == Some("[::]:9090".parse().unwrap())`.
- [ ] Test (`from_env_metrics_listen_empty_string_gives_none`): set
  `EVENTFOLD_METRICS_LISTEN=""`, assert `config.metrics_listen == None`.
- [ ] Test (`from_env_metrics_listen_custom_addr`): set
  `EVENTFOLD_METRICS_LISTEN="127.0.0.1:19090"`, assert
  `config.metrics_listen == Some("127.0.0.1:19090".parse().unwrap())`.
- [ ] Test (`from_env_metrics_listen_invalid_addr`): set
  `EVENTFOLD_METRICS_LISTEN="not-an-addr"`, assert `Config::from_env()` returns `Err`
  with message containing `"EVENTFOLD_METRICS_LISTEN"`.
- [ ] All existing `from_env_*` tests still pass (no env isolation regressions — make sure
  each existing `#[serial]` test also clears `EVENTFOLD_METRICS_LISTEN`).
- [ ] `cargo build` produces zero warnings.
- [ ] `cargo clippy --all-targets --all-features --locked -- -D warnings` passes.

**Dependencies:** Ticket 1
**Complexity:** M
**Maps to PRD AC:** AC 7, AC 8

---

### Ticket 5: Integration test — metrics endpoint returns correct values after gRPC operations

**Description:**
Add `tests/metrics.rs` with an integration test that spins up a real in-process server
(gRPC + metrics HTTP endpoint on ephemeral ports), performs a known sequence of gRPC
operations (appends, reads, subscription), and then scrapes `GET /metrics` with `reqwest`
(or `hyper`) to assert all eight metric names appear with correct values.

**Scope:**
- Create: `tests/metrics.rs`
- Modify: `Cargo.toml` (add `reqwest = { version = "0.12", default-features = false,
  features = ["blocking"] }` to `[dev-dependencies]`, or use `hyper`/`axum`'s test client
  if already available — implementer may use whichever HTTP client compiles cleanly with
  the existing workspace; `reqwest` blocking is simplest)

**Acceptance Criteria:**
- [ ] `tests/metrics.rs` has a helper `start_metrics_test_server()` that:
  - Calls `metrics::install_recorder()` (tolerates `AlreadyInstalled`).
  - Opens a `Store` at a `tempdir`.
  - Spawns writer, broker, service, health service.
  - Binds an axum metrics server on `127.0.0.1:0` (ephemeral port) via
    `metrics::serve_metrics(handle, addr)`.
  - Binds a tonic gRPC server on `[::1]:0` (ephemeral port).
  - Returns both addresses and relevant handles.
- [ ] Test `metrics_endpoint_returns_200_with_correct_content_type`: setup: call
  `start_metrics_test_server()`; action: `GET http://127.0.0.1:{metrics_port}/metrics`;
  assertion: HTTP status == 200, `Content-Type` header contains `text/plain` and
  `version=0.0.4`.
- [ ] Test `metrics_counters_correct_after_appends`: setup: start server, append 2 events
  to stream A (two separate `Append` RPC calls, each with 1 event) and 1 event to
  stream B (1 call, 1 event); action: scrape `/metrics`; assertion: body contains
  `eventfold_appends_total` with value 3, `eventfold_events_total` with value 3,
  `eventfold_global_position` with value 3, `eventfold_streams_total` with value 2.
- [ ] Test `metrics_histogram_present_after_append`: setup: start server, append 1 event;
  action: scrape `/metrics`; assertion: body contains both
  `eventfold_append_duration_seconds_bucket` and
  `eventfold_append_duration_seconds_count 1`.
- [ ] Test `metrics_reads_total_labeled_correctly`: setup: start server, append 1 event;
  then call `ReadStream` once and `ReadAll` twice; action: scrape `/metrics`; assertion:
  body contains a line matching `eventfold_reads_total{rpc="read_stream"} 1` and a line
  matching `eventfold_reads_total{rpc="read_all"} 2`.
- [ ] Test `metrics_log_bytes_nonzero_after_append`: setup: start server, append 1 event;
  action: scrape `/metrics`; assertion: body contains `eventfold_log_bytes` with a
  numeric value > 0 (parse the value from the text format).
- [ ] Test `metrics_subscriptions_active_gauge`: setup: start server; open a `SubscribeAll`
  gRPC streaming call; scrape `/metrics` while the stream is open and assert
  `eventfold_subscriptions_active 1`; drop/cancel the stream and wait ~50 ms; scrape
  again and assert `eventfold_subscriptions_active 0`.
- [ ] Test `metrics_disabled_when_env_var_empty`: verify structurally that
  `Config::from_env()` with `EVENTFOLD_METRICS_LISTEN=""` produces `config.metrics_listen == None`
  (unit test in `main.rs` from Ticket 4 already covers this; reference it or duplicate
  the assertion here as a cross-check comment).
- [ ] Test `metrics_custom_port_via_env`: verify that a server started with metrics on
  `127.0.0.1:19090` responds on that port and does NOT respond on `9090` — by attempting
  a connection to both ports and asserting the custom port succeeds and `9090` fails
  (or, simpler: assert the `start_metrics_test_server()` helper's returned metrics
  address matches the address passed to `serve_metrics`).
- [ ] `cargo test` passes with all new integration tests green and all pre-existing tests
  still green.
- [ ] `cargo clippy --all-targets --all-features --locked -- -D warnings` passes.

**Dependencies:** Ticket 1, Ticket 2, Ticket 3, Ticket 4
**Complexity:** L
**Maps to PRD AC:** AC 1, AC 2, AC 3, AC 4, AC 5, AC 6, AC 7, AC 8, AC 10

---

### Ticket 6: Verification and integration check

**Description:**
Run the full PRD 013 acceptance criteria checklist end-to-end. Verify all five tickets
integrate correctly — metrics module, store surface, writer instrumentation, service
instrumentation, config parsing, and the integration test all hang together with no
regressions.

**Scope:**
- No new files. Read-only verification pass.

**Acceptance Criteria:**
- [ ] `cargo build` completes with zero errors and zero warnings.
- [ ] `cargo clippy --all-targets --all-features --locked -- -D warnings` passes with zero
  diagnostics.
- [ ] `cargo test` passes with all tests green (all pre-existing tests plus the new unit
  tests in `metrics.rs`, `store.rs`, `writer.rs`, `service.rs`, `main.rs`, and the new
  integration test in `tests/metrics.rs`).
- [ ] `cargo fmt --check` passes.
- [ ] Grep confirms all eight metric names appear in the codebase:
  `grep -rn "eventfold_appends_total\|eventfold_events_total\|eventfold_append_duration_seconds\|eventfold_reads_total\|eventfold_subscriptions_active\|eventfold_global_position\|eventfold_streams_total\|eventfold_log_bytes" src/` should return at least one hit per metric name.
- [ ] Grep confirms `serve_metrics` calls `tokio::spawn` and does not call `Runtime::new`
  or `Builder::new_current_thread` or `Builder::new_multi_thread`:
  `grep -n "Runtime::new\|new_current_thread\|new_multi_thread" src/metrics.rs` returns
  no output.
- [ ] Grep confirms `eventfold_reads_total` counters in `service.rs` fire before any
  early return: `grep -n "reads_total\|return Err\|parse_uuid" src/service.rs` shows
  `reads_total` appearing before `parse_uuid` / `map_err` in both `read_stream` and
  `read_all` functions.
- [ ] Grep confirms the `EVENTFOLD_METRICS_LISTEN` variable is cleared in all
  `#[serial]` env tests in `main.rs`:
  `grep -n "EVENTFOLD_METRICS_LISTEN\|remove_var\|set_var" src/main.rs` output shows
  the variable is removed or set in each `#[serial]` test setup block.
- [ ] All five existing `EventStore` RPCs (`Append`, `ReadStream`, `ReadAll`,
  `SubscribeAll`, `SubscribeStream`) pass all existing integration tests without
  modification (confirmed by `cargo test` output showing zero failures in
  `tests/grpc_service.rs`).
- [ ] PRD AC 9 confirmed: `cargo build` zero warnings, `cargo clippy` zero diagnostics,
  `cargo test` all green, `cargo fmt --check` passes.

**Dependencies:** Ticket 1, Ticket 2, Ticket 3, Ticket 4, Ticket 5
**Complexity:** S
**Maps to PRD AC:** AC 1, AC 2, AC 3, AC 4, AC 5, AC 6, AC 7, AC 8, AC 9, AC 10

---

## AC Coverage Matrix

| PRD AC # | Description | Covered By Ticket(s) | Status |
|----------|-------------|----------------------|--------|
| 1 | `GET /metrics` returns HTTP 200 with `Content-Type: text/plain; version=0.0.4` and all eight metric names | Ticket 1, Ticket 5, Ticket 6 | Covered |
| 2 | After N events across M streams: `appends_total` = RPC calls succeeded, `events_total` = N, `global_position` = N, `streams_total` = M | Ticket 2, Ticket 5, Ticket 6 | Covered |
| 3 | `append_duration_seconds_bucket` lines present; `_count` non-zero after one append | Ticket 2, Ticket 5, Ticket 6 | Covered |
| 4 | `reads_total{rpc="read_stream"}` = 1 and `reads_total{rpc="read_all"}` = 2 after one ReadStream + two ReadAll calls | Ticket 3, Ticket 5, Ticket 6 | Covered |
| 5 | `subscriptions_active` = 1 while SubscribeAll open; = 0 after disconnect | Ticket 3, Ticket 5, Ticket 6 | Covered |
| 6 | `log_bytes` > 0 after one append; increases monotonically | Ticket 2, Ticket 5, Ticket 6 | Covered |
| 7 | `EVENTFOLD_METRICS_LISTEN=""` disables endpoint; log contains "Metrics endpoint disabled" | Ticket 4, Ticket 6 | Covered |
| 8 | `EVENTFOLD_METRICS_LISTEN=127.0.0.1:19090` serves on 19090, not 9090 | Ticket 1, Ticket 4, Ticket 5, Ticket 6 | Covered |
| 9 | `cargo build` zero warnings; `clippy` passes; `cargo test` all green; `cargo fmt --check` passes | Ticket 6 | Covered |
| 10 | Metrics HTTP server runs in the same tokio runtime (verified structurally: `serve_metrics` calls `tokio::spawn`, not a new `Runtime`) | Ticket 1, Ticket 5, Ticket 6 | Covered |
