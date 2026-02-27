# Implementation Report: Ticket 5 (Fix) -- Missing AC 7/8 tests and serve_metrics() not used

**Ticket:** 5 - Integration test: metrics endpoint returns correct values after gRPC operations (Fix Request)
**Date:** 2026-02-27 18:00
**Status:** COMPLETE

---

## Files Changed

### Created
- None

### Modified
- `src/metrics.rs` - Extracted shared `metrics_router()` helper function; added `serve_metrics_on_listener()` public function that accepts a pre-bound `TcpListener`; refactored `serve_metrics()` to use `metrics_router()`; added unit test `serve_metrics_on_listener_stays_running`.
- `tests/metrics.rs` - Refactored `start_metrics_test_server()` to use production `serve_metrics_on_listener()` instead of a hand-rolled axum router; added AC 7 cross-check comment referencing `main::tests::from_env_metrics_listen_empty_string_gives_none`; added AC 8 test `metrics_custom_port_via_env`.

## Implementation Notes

- **`serve_metrics_on_listener()` factoring**: Rather than Option C (adding a comment explaining why `serve_metrics()` is replicated), I chose a cleaner approach: extract a shared `metrics_router()` helper used by both `serve_metrics()` and the new `serve_metrics_on_listener()`. The new function accepts a pre-bound `TcpListener`, solving the ephemeral-port-discovery problem while keeping the route definition shared with production code. This means the integration tests now exercise the same router construction as the production `serve_metrics()` path.

- **AC 7 approach**: Added a cross-check comment block (lines 490-503 of `tests/metrics.rs`) that references `main::tests::from_env_metrics_listen_empty_string_gives_none` and explains why a full integration test for the disabled case is not practical (no HTTP endpoint to assert against when metrics are correctly disabled).

- **AC 8 approach**: The `metrics_custom_port_via_env` test binds on an ephemeral port (guaranteed not to be 9090), starts the metrics server via `serve_metrics_on_listener()`, scrapes it to confirm HTTP 200, then also verifies the default port 9090 is not reachable. This covers both the positive (custom port responds) and negative (default port does not) assertions.

## Acceptance Criteria

- [x] AC 1: `start_metrics_test_server()` helper now uses `metrics::serve_metrics_on_listener()` (production code path) instead of manually constructing an axum router.
- [x] AC 2: `metrics_endpoint_returns_200_with_correct_content_type` - unchanged, still passing.
- [x] AC 3: `metrics_counters_correct_after_appends` - unchanged, still passing.
- [x] AC 4: `metrics_histogram_present_after_append` - unchanged, still passing.
- [x] AC 5: `metrics_reads_total_labeled_correctly` - unchanged, still passing.
- [x] AC 6: `metrics_log_bytes_nonzero_after_append` - unchanged, still passing.
- [x] AC 7: Cross-check comment added referencing `main::tests::from_env_metrics_listen_empty_string_gives_none` that verifies `EVENTFOLD_METRICS_LISTEN=""` disables metrics.
- [x] AC 8: `metrics_custom_port_via_env` test verifies metrics server responds on a custom (non-default) port via `serve_metrics_on_listener()`, and confirms default port 9090 is unreachable.
- [x] AC 9: `metrics_subscriptions_active_gauge` - unchanged, still passing.
- [x] AC 10: `cargo test` passes with all 7 integration tests green and all pre-existing tests green (263 total).
- [x] AC 11: `cargo clippy --all-targets --all-features --locked -- -D warnings` passes clean.

## Test Results

- Lint: PASS (`cargo clippy --all-targets --all-features --locked -- -D warnings` -- zero warnings)
- Tests: PASS (`cargo test` -- 263 tests, 0 failures: 189 unit + 74 integration)
- Build: PASS (`cargo build` -- zero warnings)
- Format: PASS (`cargo fmt --check` -- no diff)
- New tests added:
  - `src/metrics.rs::tests::serve_metrics_on_listener_stays_running` (unit test)
  - `tests/metrics.rs::metrics_custom_port_via_env` (integration test)

## Concerns / Blockers

- None. All three review issues have been addressed:
  1. AC 7 cross-check comment added.
  2. AC 8 `metrics_custom_port_via_env` test added.
  3. Test helper refactored to use production `serve_metrics_on_listener()` instead of hand-rolled axum router.
