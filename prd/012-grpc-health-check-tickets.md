# Tickets for PRD 012: gRPC Health Check / Readiness Probe

**Source PRD:** prd/012-grpc-health-check.md
**Created:** 2026-02-27
**Total Tickets:** 3
**Estimated Total Complexity:** 6 (S=1, M=2, L=3)

---

### Ticket 1: Add `tonic-health` dependency and wire health service into `main.rs`

**Description:**
Add `tonic-health = "0.13"` to `Cargo.toml`, then modify `src/main.rs` to construct a
`(HealthReporter, HealthService)` pair, register both `""` and `"eventfold.EventStore"` as
`SERVING` after the TCP listener is bound, add the `HealthService` to the tonic server
builder, and transition both service names to `NOT_SERVING` inside the
`serve_with_incoming_shutdown` shutdown future before the server drains.

**Scope:**
- Modify: `Cargo.toml` (add `tonic-health = "0.13"` to `[dependencies]`)
- Modify: `src/main.rs` (construct reporter/service, register names, add to builder, shutdown
  transition)

**Acceptance Criteria:**
- [ ] `Cargo.toml` `[dependencies]` includes `tonic-health = "0.13"`.
- [ ] `src/main.rs` calls `tonic_health::server::health_reporter()` to obtain
  `(mut health_reporter, health_service)`.
- [ ] `health_reporter.set_serving::<EventStoreServer<EventfoldService>>().await` and
  `health_reporter.set_service_status("", tonic_health::ServingStatus::Serving).await` are
  called **after** `tokio::net::TcpListener::bind` succeeds (step 9 in the existing numbered
  sequence), not before.
- [ ] `health_service` is added to the tonic server builder before
  `EventStoreServer::new(service)`, i.e.:
  `builder.add_service(health_service).add_service(EventStoreServer::new(service))`.
- [ ] The shutdown future passed to `serve_with_incoming_shutdown` transitions both names to
  `NOT_SERVING` before resolving: calls both
  `health_reporter.set_service_status("", tonic_health::ServingStatus::NotServing).await`
  and `health_reporter.set_not_serving::<EventStoreServer<EventfoldService>>().await`.
- [ ] `HealthReporter` is moved (not cloned) into the shutdown future; no `Arc` wrapping is
  needed because `HealthReporter` is already `Clone + Send`.
- [ ] Test (unit, in `#[cfg(test)]` in `src/main.rs`): existing `Config::from_env_*` tests
  continue to compile and pass without modification -- no regression from the new imports.
- [ ] `cargo build` produces zero warnings after this change.
- [ ] `cargo clippy --all-targets --all-features --locked -- -D warnings` passes.

**Dependencies:** None
**Complexity:** M
**Maps to PRD AC:** AC 1, AC 2, AC 6, AC 9

---

### Ticket 2: Integration test — health check returns SERVING for registered service names

**Description:**
Add `tests/health_check.rs` with an integration test that spins up a real in-process tonic
server (following the `start_test_server` pattern from `tests/grpc_service.rs`) with the
health service registered, then uses `tonic_health`'s generated client
(`tonic_health::pb::health_client::HealthClient`) to verify that the `Check` RPC returns
`SERVING` for both `""` and `"eventfold.EventStore"`, and returns a `NOT_FOUND` gRPC status
for `"nonexistent.Service"`.

**Scope:**
- Create: `tests/health_check.rs`
- Modify: `Cargo.toml` (add `tonic-health` to `[dev-dependencies]` so the health client is
  available in integration tests — note: `tonic-health` in `[dependencies]` already brings it
  in transitively, but an explicit `[dev-dependencies]` entry is cleaner for test-only client
  use; verify whether it is needed by attempting `cargo test` first)

**Acceptance Criteria:**
- [ ] `tests/health_check.rs` has a `start_health_test_server()` async helper that spins up
  a real tonic server with both `health_service` and `EventStoreServer` on an ephemeral
  `[::1]:0` port, following the existing `start_test_server` pattern (Store + writer +
  EventfoldService + TcpListenerStream + `tokio::spawn`).
- [ ] Test `health_check_empty_service_name_returns_serving`: setup: call
  `start_health_test_server()`; action: call
  `HealthClient::check(HealthCheckRequest { service: "".into() })`; assertion: response
  `.status()` == `ServingStatus::Serving` (integer value 1).
- [ ] Test `health_check_eventstore_service_name_returns_serving`: setup: call
  `start_health_test_server()`; action: call
  `HealthClient::check(HealthCheckRequest { service: "eventfold.EventStore".into() })`;
  assertion: response `.status()` == `ServingStatus::Serving`.
- [ ] Test `health_check_unknown_service_returns_not_found`: setup: call
  `start_health_test_server()`; action: call
  `HealthClient::check(HealthCheckRequest { service: "nonexistent.Service".into() })`;
  assertion: the call returns `Err(status)` where `status.code() == tonic::Code::NotFound`.
- [ ] `cargo test` runs all three new tests and they pass (green).
- [ ] `cargo clippy --all-targets --all-features --locked -- -D warnings` passes.

**Dependencies:** Ticket 1
**Complexity:** M
**Maps to PRD AC:** AC 3, AC 4, AC 5, AC 8

---

### Ticket 3: Verification and integration check

**Description:**
Run the full PRD 012 acceptance criteria checklist end-to-end. Verify all tickets integrate
correctly: the health service is reachable on the same port as `EventStore`, the status
transitions are wired in the correct order, and no existing tests regress.

**Scope:**
- No new files. Read-only verification pass.

**Acceptance Criteria:**
- [ ] `cargo build` completes with zero errors and zero warnings.
- [ ] `cargo clippy --all-targets --all-features --locked -- -D warnings` passes with zero
  diagnostics.
- [ ] `cargo test` passes with all tests green (existing 210+ tests plus 3 new health check
  tests).
- [ ] `cargo fmt --check` passes.
- [ ] Grep confirms `SERVING` is set after the `TcpListener::bind` call and not before:
  `grep -n "set_serving\|set_service_status" src/main.rs` output shows both calls appear
  after the line containing `TcpListener::bind` in file order.
- [ ] Grep confirms `NOT_SERVING` is set inside the shutdown future:
  `grep -n "NotServing\|set_not_serving" src/main.rs` output shows calls inside the async
  block passed to `serve_with_incoming_shutdown`.
- [ ] Grep confirms `health_service` is added to the server builder before `EventStoreServer`:
  `grep -n "add_service" src/main.rs` output shows `health_service` appears before
  `EventStoreServer::new`.
- [ ] All five existing `EventStore` RPCs (`Append`, `ReadStream`, `ReadAll`, `SubscribeAll`,
  `SubscribeStream`) pass all existing integration tests without modification (confirmed by
  `cargo test` output showing zero failures in `tests/grpc_service.rs`).
- [ ] PRD AC 7 confirmed: no changes were made to `src/service.rs`, `src/store.rs`,
  `src/broker.rs`, `src/writer.rs`, `src/types.rs`, or `src/error.rs`.

**Dependencies:** Ticket 1, Ticket 2
**Complexity:** S
**Maps to PRD AC:** AC 1, AC 2, AC 3, AC 4, AC 5, AC 6, AC 7, AC 8, AC 9

---

## AC Coverage Matrix

| PRD AC # | Description | Covered By Ticket(s) | Status |
|----------|-------------|----------------------|--------|
| 1 | `cargo build` completes with zero errors and zero warnings after adding `tonic-health` | Ticket 1, Ticket 3 | Covered |
| 2 | `cargo clippy --all-targets --all-features --locked -- -D warnings` passes after all changes | Ticket 1, Ticket 3 | Covered |
| 3 | `grpc.health.v1.Health/Check` with `service: ""` returns `status: SERVING` | Ticket 2, Ticket 3 | Covered |
| 4 | `grpc.health.v1.Health/Check` with `service: "eventfold.EventStore"` returns `status: SERVING` | Ticket 2, Ticket 3 | Covered |
| 5 | `grpc.health.v1.Health/Check` with `service: "nonexistent.Service"` returns gRPC `NOT_FOUND` | Ticket 2, Ticket 3 | Covered |
| 6 | `HealthReporter` transitions both names to `NOT_SERVING` inside the `serve_with_incoming_shutdown` shutdown future before the server stops accepting connections | Ticket 1, Ticket 3 | Covered |
| 7 | Existing five `EventStore` RPCs pass all existing integration tests without modification | Ticket 3 | Covered |
| 8 | `cargo test` passes with all tests green, including the new health check integration test | Ticket 2, Ticket 3 | Covered |
| 9 | `SERVING` status is set **after** `tokio::net::TcpListener::bind` succeeds | Ticket 1, Ticket 3 | Covered |
