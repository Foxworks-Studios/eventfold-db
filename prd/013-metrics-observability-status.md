# Build Status: PRD 013 -- Metrics / Observability

**Source PRD:** prd/013-metrics-observability.md
**Tickets:** prd/013-metrics-observability-tickets.md
**Started:** 2026-02-27 09:30
**Last Updated:** 2026-02-27 11:00
**Overall Status:** QA READY

---

## Ticket Tracker

| Ticket | Title | Status | Impl Report | Review Report | Notes |
|--------|-------|--------|-------------|---------------|-------|
| 1 | Add metrics dependencies and `src/metrics.rs` module | DONE | ticket-01-impl.md | ticket-01-review.md | APPROVED |
| 2 | Add `Store::log_file_len()` and instrument `run_writer` | DONE | ticket-02-impl.md | ticket-02-review.md | APPROVED (round 2) |
| 3 | Instrument `service.rs` with read and subscription metrics | DONE | ticket-03-impl.md | ticket-03-review.md | APPROVED |
| 4 | Wire metrics into `main.rs` (Config, install_recorder, serve_metrics) | DONE | ticket-04-impl.md | ticket-04-review.md | APPROVED |
| 5 | Integration test -- metrics endpoint returns correct values | DONE | ticket-05-impl.md | ticket-05-review.md | APPROVED (round 2) |
| 6 | Verification and integration check | DONE | -- | -- | All gates pass, all ACs covered |

## Prior Work Summary

- `Cargo.toml`: Added `metrics = "0.24"`, `metrics-exporter-prometheus`, `axum` dependencies.
- `src/metrics.rs`: `MetricsError`, `MetricsHandle`, `install_recorder()` (OnceLock), `serve_metrics()` (axum), `get_installed_handle()`.
- `src/lib.rs`: Added `pub mod metrics;`.
- `src/store.rs`: Added `Store::log_file_len()` method (stat on log file), 2 unit tests.
- `src/writer.rs`: Instrumented `run_writer` with 6 metrics (appends_total, events_total, append_duration_seconds, global_position, streams_total, log_bytes). All fire only on successful append. 1 unit test (delta-based).
- `src/service.rs`: Added `eventfold_reads_total` counter (with rpc label) in read_stream/read_all. Added `SubscriptionGauge` RAII guard for subscriptions_active gauge in subscribe_all/subscribe_stream. 3 unit tests.
- `src/main.rs`: Added `metrics_listen: Option<SocketAddr>` to Config. Parses `EVENTFOLD_METRICS_LISTEN` (default `[::]:9090`, empty=disabled). Calls `install_recorder()` + `serve_metrics()` in main(). Aborts metrics handle on shutdown. 4 new tests, updated 18 existing tests to clear new env var.
- 260 tests passing. Build, clippy, fmt all clean.

## Follow-Up Tickets

[None.]

## Completion Report

**Completed:** 2026-02-27 11:00
**Tickets Completed:** 6/6

### Summary of Changes

**Files created:**
- `src/metrics.rs` -- MetricsHandle, MetricsError, install_recorder(), serve_metrics(), serve_metrics_on_listener()
- `tests/metrics.rs` -- 7 integration tests for metrics endpoint

**Files modified:**
- `Cargo.toml` -- Added `metrics = "0.24"`, `metrics-exporter-prometheus`, `axum` dependencies
- `src/lib.rs` -- Added `pub mod metrics;`
- `src/store.rs` -- Added `Store::log_file_len()` method + 2 unit tests
- `src/writer.rs` -- Instrumented `run_writer` with 6 metrics + 1 unit test
- `src/service.rs` -- Added reads_total counter (labeled) + subscriptions_active gauge (RAII) + 3 unit tests
- `src/main.rs` -- Added `metrics_listen` to Config, env var parsing, install_recorder + serve_metrics wiring, shutdown abort, 4 new tests + 18 existing tests updated

### Key Architectural Decisions
- `metrics` crate for zero-cost macro-based instrumentation
- `metrics-exporter-prometheus` for Prometheus text format rendering
- axum for HTTP endpoint (single GET /metrics route)
- OnceLock guard for safe process-global recorder installation
- SubscriptionGauge RAII guard for guaranteed decrement on client disconnect
- Delta-based test assertions for process-global metric counters
- serve_metrics_on_listener() for test-friendly ephemeral port binding

### AC Coverage Matrix
| AC | Verified |
|----|----------|
| 1 | Yes -- integration test scrapes all 8 metrics |
| 2 | Yes -- integration test verifies counter values after appends |
| 3 | Yes -- integration test verifies histogram present |
| 4 | Yes -- integration test verifies labeled reads_total |
| 5 | Yes -- integration test verifies subscriptions_active lifecycle |
| 6 | Yes -- integration test verifies log_bytes > 0 |
| 7 | Yes -- unit test in main.rs + cross-check comment |
| 8 | Yes -- integration test verifies custom port |
| 9 | Yes -- build/clippy/test/fmt all clean, 263 tests |
| 10 | Yes -- grep confirms no Runtime::new in metrics.rs |

### Ready for QA: YES
