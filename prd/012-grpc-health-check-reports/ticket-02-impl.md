# Implementation Report: Ticket 2 -- Integration tests for health check

**Ticket:** 2 - Integration test -- health check returns SERVING for registered service names
**Date:** 2026-02-27 12:00
**Status:** COMPLETE

---

## Files Changed

### Created
- `tests/health_check.rs` - Integration tests for the gRPC health check service with 3 tests and a `start_health_test_server()` helper

### Modified
- None (no changes to `Cargo.toml` needed -- `tonic-health` is already in `[dependencies]` and available to integration tests)

## Implementation Notes
- `start_health_test_server()` follows the exact pattern from `tests/grpc_service.rs`: creates Store, Broker, spawns writer, creates EventfoldService, binds TcpListener on `[::1]:0`, spawns tonic server with `tokio::spawn`, and connects a client after a 50ms sleep.
- The health service is added identically to `main.rs`: `health_reporter` and `health_service` created via `tonic_health::server::health_reporter()`, both `""` and `"eventfold.EventStore"` set to SERVING after bind, health service added to tonic builder before EventStoreServer.
- `HealthClient` does not have a `connect()` convenience method like `EventStoreClient`. Instead, a `Channel` is created via `Channel::from_shared(...).connect()` and passed to `HealthClient::new()`.
- The `status()` method on `HealthCheckResponse` returns a `ServingStatus` enum (prost-generated), compared directly against `ServingStatus::Serving`.
- For the unknown service test, `tonic-health` returns `tonic::Code::NotFound` for unregistered service names, which is the standard gRPC health check behavior.

## Acceptance Criteria
- [x] AC 1: `tests/health_check.rs` has `start_health_test_server()` async helper that spins up a real tonic server with both `health_service` and `EventStoreServer` on ephemeral `[::1]:0`, following the existing `start_test_server` pattern (Store + writer + EventfoldService + TcpListenerStream + `tokio::spawn`).
- [x] AC 2: Test `health_check_empty_service_name_returns_serving` calls `start_health_test_server()`, issues `HealthClient::check(HealthCheckRequest { service: "" })`, and asserts `status() == ServingStatus::Serving`.
- [x] AC 3: Test `health_check_eventstore_service_name_returns_serving` calls `start_health_test_server()`, issues `HealthClient::check(HealthCheckRequest { service: "eventfold.EventStore" })`, and asserts `status() == ServingStatus::Serving`.
- [x] AC 4: Test `health_check_unknown_service_returns_not_found` calls `start_health_test_server()`, issues `HealthClient::check(HealthCheckRequest { service: "nonexistent.Service" })`, and asserts the call returns `Err(status)` where `status.code() == tonic::Code::NotFound`.
- [x] AC 5: `cargo test` runs all three new tests and they pass (green). Full suite: 243 tests, 0 failures.
- [x] AC 6: `cargo clippy --all-targets --all-features --locked -- -D warnings` passes with zero diagnostics.

## Test Results
- Lint: PASS (`cargo clippy --all-targets --all-features --locked -- -D warnings` -- zero diagnostics)
- Tests: PASS (`cargo test` -- 243 passed, 0 failed, including 3 new health check tests)
- Build: PASS (`cargo build` -- zero warnings)
- Format: PASS (`cargo fmt --check` -- no diffs)
- New tests added: `tests/health_check.rs` (3 tests: `health_check_empty_service_name_returns_serving`, `health_check_eventstore_service_name_returns_serving`, `health_check_unknown_service_returns_not_found`)

## Concerns / Blockers
- None
