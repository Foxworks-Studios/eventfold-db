# Code Review (Round 2): Ticket 5 -- Integration test: metrics endpoint returns correct values

**Ticket:** 5 -- Integration test: metrics endpoint returns correct values after gRPC operations
**Impl Report:** prd/013-metrics-observability-reports/ticket-05-impl.md
**Date:** 2026-02-27 18:30
**Verdict:** APPROVED

---

## Round 1 Issues -- Resolution

All three issues from Round 1 are resolved:

1. **AC 7 (missing cross-check comment):** Added at lines 490-503 of `tests/metrics.rs`. References the correct unit test `main::tests::from_env_metrics_listen_empty_string_gives_none` (confirmed at `src/main.rs:771`). The explanation for why a full integration test is impractical (no HTTP endpoint to assert against when disabled) is sound.

2. **AC 8 (missing `metrics_custom_port_via_env` test):** Added at lines 507-569 of `tests/metrics.rs`. Tests both positive (custom port responds with 200) and negative (default port 9090 unreachable) assertions. Uses ephemeral port binding (better than hardcoded 19090 from the AC spec for test isolation).

3. **`start_metrics_test_server()` not using production code path:** Refactored at lines 85-96 of `tests/metrics.rs` to call `metrics::serve_metrics_on_listener(handle, metrics_listener)`. The new `serve_metrics_on_listener()` function in `src/metrics.rs` shares the same `metrics_router()` helper as `serve_metrics()`, guaranteeing identical route definitions.

## AC Coverage

| AC # | Description | Status | Notes |
|------|-------------|--------|-------|
| 1 | `start_metrics_test_server()` helper sets up full stack via production code | Met | Lines 85-96 call `metrics::serve_metrics_on_listener()` which uses shared `metrics_router()` from `src/metrics.rs:99`. Same route definition as `serve_metrics()`. |
| 2 | Test: HTTP 200 with correct content-type | Met | `metrics_endpoint_returns_200_with_correct_content_type` (line 179) checks status 200, `text/plain`, and `version=0.0.4`. |
| 3 | Test: counters correct after appends (delta-based) | Met | `metrics_counters_correct_after_appends` (line 213) uses deltas for counters, absolutes for gauges. 3 appends, 2 streams verified. |
| 4 | Test: histogram present after append | Met | `metrics_histogram_present_after_append` (line 295) checks `_sum`, `_count`, and `{quantile=` lines. Summary format deviation from `_bucket` is documented and reflects actual library behavior. |
| 5 | Test: reads_total labeled correctly | Met | `metrics_reads_total_labeled_correctly` (line 333) verifies `read_stream` delta=1 and `read_all` delta=2. |
| 6 | Test: log_bytes nonzero after append | Met | `metrics_log_bytes_nonzero_after_append` (line 401) asserts `> 0`. |
| 7 | Test: metrics disabled when env var empty | Met | Cross-check comment at lines 490-503 references `main::tests::from_env_metrics_listen_empty_string_gives_none`. Verified reference target exists at `src/main.rs:771`. |
| 8 | Test: metrics on custom port | Met | `metrics_custom_port_via_env` (line 509) binds ephemeral port (asserts != 9090), starts server via `serve_metrics_on_listener()`, scrapes 200 on custom port, confirms 9090 unreachable. |
| 9 | Test: subscriptions_active gauge lifecycle | Met | `metrics_subscriptions_active_gauge` (line 429) checks >= 1 while stream open, == 0 after drop with 200ms grace. |
| 10 | `cargo test` all green, clippy clean | Met | 263 tests pass (189 unit + 74 integration), clippy zero warnings, fmt clean. |

## Issues Found

### Critical (must fix before merge)

None.

### Major (should fix, risk of downstream problems)

None.

### Minor (nice to fix, not blocking)

1. **`unwrap_or_default()` in `scrape_body` (line 143):** If the HTTP response lacks `\r\n\r\n`, this silently returns an empty string rather than failing the test. Using `.expect("HTTP response should contain header/body separator")` would make test failures more diagnostic. Carried forward from Round 1 -- non-blocking.

2. **Health service setup (lines 60-74):** Sets up a health reporter and service that no metrics test exercises. Harmless (mirrors `grpc_service.rs` pattern) but adds unused code. Carried forward from Round 1 -- non-blocking.

## Suggestions (non-blocking)

- The `serve_metrics_on_listener()` function is marked `pub`, which is appropriate for integration test access. However, if this is considered a testing-only API, a `#[doc(hidden)]` or a comment noting its primary use case (tests needing ephemeral port discovery) would prevent confusion. The existing doc comment is good enough for now.

## Scope Check

- Files within scope: YES -- `src/metrics.rs` and `tests/metrics.rs` only.
- Scope creep detected: NO -- The `metrics_router()` extraction and `serve_metrics_on_listener()` addition are minimal, well-justified changes that directly serve AC 1's requirement for the test helper to use the production code path.
- Unauthorized dependencies added: NO

## Risk Assessment

- Regression risk: LOW -- The `metrics_router()` refactor is a pure extraction; `serve_metrics()` still delegates to it and behavior is identical. The new `serve_metrics_on_listener()` is additive. All 263 tests pass including all pre-existing tests.
- Security concerns: NONE
- Performance concerns: NONE -- 50ms sleep for server startup and 200ms for subscription cleanup are reasonable for integration tests.
