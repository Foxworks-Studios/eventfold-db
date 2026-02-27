# PRD 013: Metrics / Observability

**Status:** TICKETS READY
**Created:** 2026-02-26
**Author:** PRD Writer Agent

---

## Problem Statement

EventfoldDB runs as a black-box process: there is no way for an operator to see
how many events have been written, what the current log size is, or how append
latency is trending over time. Without Prometheus-format metrics, operators
cannot build dashboards, set up alerting thresholds, or diagnose performance
regressions in production on Fly.io or any other deployment target.

## Goals

- Expose eight named Prometheus metrics covering append throughput, latency,
  read traffic, active subscriptions, log head position, stream count, and
  on-disk log size.
- Serve those metrics at `GET /metrics` on a separate HTTP port (default
  `[::]:9090`) using an axum server running in the same tokio runtime as the
  gRPC server.
- Allow operators to disable the metrics endpoint entirely by setting
  `EVENTFOLD_METRICS_LISTEN` to an empty string.

## Non-Goals

- Tracing spans or distributed trace propagation (OpenTelemetry, Jaeger). Tracing
  already exists via the `tracing` crate; this PRD adds counters and gauges only.
- Per-stream or per-event-type metric labels. All metrics are server-wide.
- Push-based metrics (StatsD, InfluxDB line protocol). Prometheus pull only.
- A `/health` or `/ready` endpoint. That is a separate feature.
- Metrics authentication or TLS on the metrics port.
- Histogram bucket customization via environment variable. Default buckets from
  the `metrics-exporter-prometheus` crate are used.
- Integration with the console TUI (PRD 009). The TUI reads gRPC only.

## User Stories

- As an operator, I want to scrape `GET /metrics` from a running EventfoldDB
  instance so that I can ingest append rates and latency into my Prometheus
  server and alert on anomalies.
- As an operator, I want `eventfold_log_bytes` so that I can set a Fly.io volume
  capacity alert before the disk fills up.
- As an operator, I want `eventfold_subscriptions_active` so that I can detect
  when projection services disconnect and stop building read models.
- As a developer, I want to set `EVENTFOLD_METRICS_LISTEN=""` to disable the
  metrics endpoint in test environments where binding a port is unwanted.

## Technical Approach

### New dependencies

Add to `[dependencies]` in the root `Cargo.toml`:

```toml
metrics = "0.24"
metrics-exporter-prometheus = { version = "0.16", default-features = false, features = ["http-listener"] }
axum = { version = "0.8", default-features = false, features = ["tokio", "http1"] }
```

`metrics` provides the zero-cost `counter!`, `gauge!`, `histogram!` macros used
at call sites. `metrics-exporter-prometheus` installs a global recorder and
renders the Prometheus text format. `axum` serves the single `GET /metrics`
handler (per CLAUDE.md preferred tools for HTTP APIs).

### New module: `src/metrics.rs`

A new public module responsible for:

1. **`MetricsHandle`** — an opaque struct that holds an `Arc`-wrapped
   `PrometheusHandle` (the render handle from `metrics-exporter-prometheus`).
   It is `Clone + Send + Sync`.

2. **`install_recorder() -> Result<MetricsHandle, MetricsError>`** — installs
   the `PrometheusRecorder` as the global `metrics` recorder via
   `PrometheusBuilder::new().install_recorder()`. Must be called once at startup
   before any metric macros fire. Returns `MetricsError::AlreadyInstalled` if
   called twice (guarded for test isolation).

3. **`serve_metrics(handle: MetricsHandle, addr: SocketAddr) -> JoinHandle<()>`**
   — spawns a tokio task that binds an axum `Router` with a single route:
   `GET /metrics` returning `handle.render()` as `text/plain; version=0.0.4`.
   The task runs until the process exits. Logs the bound address with
   `tracing::info!`. On bind failure, logs with `tracing::error!` and returns
   a `JoinHandle` that completes immediately.

### Metric names and call sites

| Metric name | Kind | Labels | Call site |
|---|---|---|---|
| `eventfold_appends_total` | counter | — | `writer.rs` `run_writer`: increment once per successful `store.append()` call |
| `eventfold_events_total` | counter | — | `writer.rs` `run_writer`: increment by `recorded.len()` per successful append |
| `eventfold_append_duration_seconds` | histogram | — | `writer.rs` `run_writer`: record elapsed time from batch-start to post-fsync (i.e., after `store.append()` returns `Ok`) |
| `eventfold_reads_total` | counter | `rpc="read_stream"` or `rpc="read_all"` | `service.rs` `read_stream` and `read_all` handlers, incremented on every call regardless of outcome |
| `eventfold_subscriptions_active` | gauge | — | `service.rs` `subscribe_all` and `subscribe_stream`: `+1` when stream begins, `-1` when stream ends |
| `eventfold_global_position` | gauge | — | `writer.rs` `run_writer`: set to `recorded.last().global_position + 1` after each successful append |
| `eventfold_streams_total` | gauge | — | `writer.rs` `run_writer`: set to the number of unique stream IDs after each successful append (read from `store`'s `EventLog`) |
| `eventfold_log_bytes` | gauge | — | `writer.rs` `run_writer`: set to the log file's byte length after each successful append (obtained via `store.log_file_len()` — see below) |

All metric updates happen **after** `store.append()` returns and the result is
known. Failed appends (`WrongExpectedVersion`, `EventTooLarge`, etc.) do not
increment any counter. The `eventfold_append_duration_seconds` histogram records
the wall-clock time of the `store.append()` call itself (including fsync), using
`std::time::Instant::now()` taken immediately before the call and elapsed
immediately after.

Because all metric updates in `run_writer` happen in the same single-threaded
task loop (no concurrency inside the task), there is no contention on the
`metrics` macros. The macros themselves are designed to be low-overhead and do
not allocate on the hot path once the metric label key is registered.

### Store surface change: `log_file_len()`

`src/store.rs` gains one new public method:

```rust
/// Returns the current byte length of the log file.
///
/// Called after each successful append to update the `eventfold_log_bytes` gauge.
/// Uses `File::metadata()` which does not require a seek.
pub fn log_file_len(&self) -> Result<u64, Error>
```

This avoids exposing the raw `File` handle and keeps the metric update inside
the writer task where all other post-append work already lives.

### `main.rs` changes

1. Add `Config::metrics_listen: Option<SocketAddr>` — `None` when
   `EVENTFOLD_METRICS_LISTEN` is set to an empty string, `Some(addr)` otherwise.
   Default: `Some("[::]:9090".parse().unwrap())`.

2. After `spawn_writer`, call `metrics::install_recorder()`. On error, log and
   exit non-zero.

3. If `config.metrics_listen` is `Some(addr)`, call
   `metrics::serve_metrics(handle, addr)` and store the `JoinHandle`. If
   `None`, log `"Metrics endpoint disabled"` at `info` level and skip.

4. Update the shutdown sequence: after the gRPC server exits, abort the metrics
   server `JoinHandle` (it has no state to flush).

5. Document `EVENTFOLD_METRICS_LISTEN` in the `Config` doc comment table.

### File-change summary

| File | Change |
|---|---|
| `Cargo.toml` | Add `metrics`, `metrics-exporter-prometheus`, `axum` dependencies |
| `src/metrics.rs` | New module: `MetricsHandle`, `install_recorder`, `serve_metrics`, `MetricsError` |
| `src/lib.rs` | Add `pub mod metrics;` |
| `src/store.rs` | Add `pub fn log_file_len(&self) -> Result<u64, Error>` |
| `src/writer.rs` | Instrument `run_writer`: timing, counters, gauges after each successful append |
| `src/service.rs` | Instrument `read_stream`, `read_all`, `subscribe_all`, `subscribe_stream` |
| `src/main.rs` | Parse `EVENTFOLD_METRICS_LISTEN`; call `install_recorder` + `serve_metrics` |

### Hot-path impact

`counter!` and `gauge!` in the `metrics` crate resolve to a single atomic
integer update after the first call (the label map is cached). `histogram!`
updates an atomic bucket array. None of these allocate on the steady-state path.
The `std::time::Instant` pair around `store.append()` adds two syscalls per
append batch, which is negligible compared to the fsync that dominates append
latency.

## Acceptance Criteria

1. `GET http://[::]:9090/metrics` on a running server returns HTTP 200 with
   `Content-Type: text/plain; version=0.0.4` and a body containing all eight
   metric names (`eventfold_appends_total`, `eventfold_events_total`,
   `eventfold_append_duration_seconds`, `eventfold_reads_total`,
   `eventfold_subscriptions_active`, `eventfold_global_position`,
   `eventfold_streams_total`, `eventfold_log_bytes`).

2. After appending N events across M streams via the gRPC `Append` RPC,
   `eventfold_appends_total` equals the number of `Append` RPC calls that
   returned success, `eventfold_events_total` equals N, `eventfold_global_position`
   equals N, and `eventfold_streams_total` equals M.

3. `eventfold_append_duration_seconds_bucket` lines are present in the
   `/metrics` output and `eventfold_append_duration_seconds_count` is non-zero
   after at least one successful append.

4. After calling `ReadStream` once and `ReadAll` twice,
   `eventfold_reads_total{rpc="read_stream"}` equals 1 and
   `eventfold_reads_total{rpc="read_all"}` equals 2 in the `/metrics` output.

5. While one `SubscribeAll` stream is open, `eventfold_subscriptions_active`
   is 1. After the client disconnects, `eventfold_subscriptions_active` is 0.

6. `eventfold_log_bytes` is greater than 0 after at least one successful
   append, and increases monotonically with further appends.

7. Setting `EVENTFOLD_METRICS_LISTEN=""` causes the server to start without
   binding port 9090; a connection attempt to `[::]:9090` returns a connection
   refused error, and the server log contains `"Metrics endpoint disabled"` at
   info level.

8. Setting `EVENTFOLD_METRICS_LISTEN=127.0.0.1:19090` causes `GET /metrics` to
   respond on port 19090 instead of 9090, and port 9090 is not bound.

9. `cargo build` produces zero warnings. `cargo clippy --all-targets
   --all-features --locked -- -D warnings` passes. `cargo test` passes with all
   pre-existing tests still green. `cargo fmt --check` passes.

10. The metrics HTTP server runs in the same tokio runtime as the gRPC server
    (verified structurally: `serve_metrics` calls `tokio::spawn` without
    creating a new `Runtime`).

## Open Questions

- The `metrics-exporter-prometheus` crate's default histogram buckets
  (`[0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0]` seconds)
  may be too coarse for sub-millisecond appends. Consider whether to hardcode
  finer buckets (e.g., starting at 0.0001s) or leave the default. This PRD
  leaves the default to keep scope tight; a follow-on PRD can tune buckets once
  real latency data is available.

- `store.log_file_len()` calls `File::metadata()` which makes a `stat(2)`
  syscall after every append. Under very high append rates this adds a syscall
  per batch. An alternative is tracking bytes written in a `u64` field on
  `Store`. The simpler `metadata()` approach is used here; if benchmarking shows
  it matters it can be replaced.

## Dependencies

- **Depends on**: PRDs 001-008 (complete EventfoldDB with gRPC server and writer
  task)
- **Workspace**: PRD 009 converted the project to a Cargo workspace; the new
  dependencies are added to the root `Cargo.toml` `[dependencies]` only (not
  the console crate)
- **External crates**: `metrics 0.24`, `metrics-exporter-prometheus 0.16`,
  `axum 0.8`
