# Code Review: Ticket 1 -- Add tonic-health dependency and wire health service into main.rs

**Ticket:** 1 -- Add tonic-health dependency and wire health service into main.rs
**Impl Report:** prd/012-grpc-health-check-reports/ticket-01-impl.md
**Date:** 2026-02-27 14:30
**Verdict:** APPROVED

---

## AC Coverage

| AC # | Description | Status | Notes |
|------|-------------|--------|-------|
| 1 | `Cargo.toml` `[dependencies]` includes `tonic-health = "0.13"` | Met | Line 20 of `Cargo.toml`: `tonic-health = "0.13"` |
| 2 | `src/main.rs` calls `tonic_health::server::health_reporter()` to obtain `(mut health_reporter, health_service)` | Met | Line 249: `let (health_reporter, health_service) = tonic_health::server::health_reporter();` -- `mut` correctly omitted because `set_serving`/`set_service_status` take `&self`, and `unused_mut` would fail clippy. Documented deviation. |
| 3 | `set_serving` and `set_service_status("", Serving)` called after `TcpListener::bind` | Met | Bind at line 303; `set_serving` at line 325; `set_service_status(Serving)` at line 328. Both after bind, both after `local_addr()` call. |
| 4 | `health_service` added to builder before `EventStoreServer` | Met | Line 299: `.add_service(health_service)` then line 300: `.add_service(EventStoreServer::new(service))` |
| 5 | Shutdown future transitions both names to NOT_SERVING | Met | Lines 337-342 inside the `async { }` block: `set_service_status("", NotServing)` then `set_not_serving::<EventStoreServer<EventfoldService>>()` |
| 6 | `HealthReporter` moved (not cloned) into shutdown future; no Arc wrapping | Met | No `Arc` or `.clone()` on `health_reporter`. Used by shared reference for SERVING calls (lines 324-329), then moved into async block (line 335). |
| 7 | Existing `Config::from_env_*` tests pass without modification | Met | All 16 binary tests pass. No existing tests were modified (diff confirms only additions). |
| 8 | `cargo build` produces zero warnings | Met | Verified: `Finished dev profile` with no warnings. |
| 9 | `cargo clippy --all-targets --all-features --locked -- -D warnings` passes | Met | Verified: clean output, no diagnostics. |

## Issues Found

### Critical (must fix before merge)
- None

### Major (should fix, risk of downstream problems)
- None

### Minor (nice to fix, not blocking)
- None

## Suggestions (non-blocking)
- The unit test `health_reporter_can_set_serving_and_not_serving` (lines 632-655) validates the tonic-health API surface but does not verify state transitions (i.e., it does not assert that the status actually changed). This is a limitation of the `HealthReporter` API which has no getter method, so the test is doing the best it can -- confirming the calls compile and do not panic. The real verification will come from the integration test in Ticket 2, which uses a `HealthClient` to observe the actual gRPC response. Acceptable as-is.

## Scope Check
- Files within scope: YES -- Only `Cargo.toml` and `src/main.rs` were modified (plus auto-generated `Cargo.lock`)
- Scope creep detected: NO
- Unauthorized dependencies added: NO -- `tonic-health = "0.13"` is explicitly called for by the ticket

## Risk Assessment
- Regression risk: LOW -- The diff is additive. The `health_service` is added as an additional gRPC service via `.add_service()` before the existing `EventStoreServer`. The shutdown future wraps the existing `shutdown_signal()` in an async block that adds health transitions before the future resolves. No existing behavior is altered.
- Security concerns: NONE -- Health check endpoint is unauthenticated, which matches all other RPCs in the current design (PRD explicitly states "no auth changes").
- Performance concerns: NONE -- `HealthReporter` uses `tokio::sync::watch` internally; setting status is O(1) with no allocation.

## Quality Gates

| Check | Result |
|-------|--------|
| `cargo build` | PASS (zero warnings) |
| `cargo clippy --all-targets --all-features --locked -- -D warnings` | PASS (zero diagnostics) |
| `cargo test` | PASS (240 tests: 180 lib + 16 binary + 44 integration) |
| `cargo fmt --check` | PASS |
