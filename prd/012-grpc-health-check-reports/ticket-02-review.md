# Code Review: Ticket 2 -- Integration tests for health check

**Ticket:** 2 -- Integration test -- health check returns SERVING for registered service names
**Impl Report:** prd/012-grpc-health-check-reports/ticket-02-impl.md
**Date:** 2026-02-27 14:30
**Verdict:** APPROVED

---

## AC Coverage

| AC # | Description | Status | Notes |
|------|-------------|--------|-------|
| 1 | `tests/health_check.rs` has `start_health_test_server()` async helper on `[::1]:0` following `start_test_server` pattern | Met | Lines 30-75: creates Store+Broker+writer+EventfoldService+TcpListener+TcpListenerStream+tokio::spawn, returns `(HealthClient<Channel>, SocketAddr, TempDir)`. Pattern matches `tests/grpc_service.rs` exactly (tempdir, Store::open, Broker::new, spawn_writer, TcpListener::bind("[::1]:0"), TcpListenerStream, tokio::spawn, 50ms sleep). |
| 2 | Test `health_check_empty_service_name_returns_serving` returns `ServingStatus::Serving` | Met | Lines 77-91: calls `start_health_test_server()`, issues `check(HealthCheckRequest { service: "".into() })`, asserts `resp.into_inner().status() == ServingStatus::Serving`. |
| 3 | Test `health_check_eventstore_service_name_returns_serving` returns `ServingStatus::Serving` | Met | Lines 93-109: calls `start_health_test_server()`, issues `check(HealthCheckRequest { service: "eventfold.EventStore".into() })`, asserts `ServingStatus::Serving`. |
| 4 | Test `health_check_unknown_service_returns_not_found` returns `tonic::Code::NotFound` | Met | Lines 111-127: calls `start_health_test_server()`, issues `check(HealthCheckRequest { service: "nonexistent.Service".into() })`, uses `expect_err()`, asserts `err.code() == tonic::Code::NotFound`. |
| 5 | `cargo test` runs all three tests and they pass | Met | Verified by running `cargo test --test health_check`: 3 passed, 0 failed. |
| 6 | `cargo clippy --all-targets --all-features --locked -- -D warnings` passes | Met | Verified: zero diagnostics. |

## Issues Found

### Critical (must fix before merge)
- None.

### Major (should fix, risk of downstream problems)
- None.

### Minor (nice to fix, not blocking)
- None.

## Suggestions (non-blocking)

1. **Health service ordering vs. main.rs**: In `main.rs`, services are added to the tonic builder (lines 298-300) *before* the TCP listener is bound (line 303), and then SERVING is set *after* bind (lines 324-329). In the test, the TCP listener is bound (line 41) *before* the server is spawned with services (line 55), and SERVING is set between bind and spawn (lines 48-53). This is functionally equivalent and correct (the critical invariant -- SERVING after bind -- is preserved), but if strict mirroring of `main.rs` step ordering were desired, the builder assembly could be reordered. Not actionable; both orderings are correct.

2. **`test_dedup_cap()` duplication**: The helper `test_dedup_cap()` at line 19 is duplicated identically from `tests/grpc_service.rs` line 17. If a third integration test file appears, extracting it into a shared `tests/common/mod.rs` would reduce repetition. Acceptable for two files.

## Scope Check
- Files within scope: YES -- only `tests/health_check.rs` was created.
- Scope creep detected: NO
- Unauthorized dependencies added: NO

## Risk Assessment
- Regression risk: LOW -- this is a purely additive integration test file. No existing code was modified. The test helper spins up independent servers on ephemeral ports.
- Security concerns: NONE
- Performance concerns: NONE -- three tests each spin up a server on `[::1]:0`, connect, issue one RPC, and tear down. Total execution time observed: ~50ms.
