# PRD 012: gRPC Health Check / Readiness Probe

**Status:** TICKETS READY
**Created:** 2026-02-26
**Author:** PRD Writer Agent

---

## Problem Statement

Load balancers, container runtimes, and systemd service managers need a standard machine-readable
signal to determine whether EventfoldDB is ready to serve traffic. Without it, operators must
rely on TCP port checks (which succeed as soon as the socket is bound, before the store has
finished startup recovery) or ad-hoc monitoring scripts. The gRPC health checking protocol
(`grpc.health.v1.Health`) is the industry standard, natively understood by grpc-health-probe,
Kubernetes liveness/readiness probes, and Fly.io health checks.

## Goals

- Implement the `grpc.health.v1.Health` service on the existing gRPC port so any standard
  gRPC health-check client can query server status without configuration changes.
- Report `SERVING` only after the store is opened, the writer task is spawned, and the TCP
  listener is bound -- ensuring the probe reflects true readiness, not just process liveness.
- Report `NOT_SERVING` after a shutdown signal is received, before the server stops accepting
  connections, so the load balancer or orchestrator drains traffic cleanly.

## Non-Goals

- No new port, no separate HTTP/REST health endpoint. The health service runs on the same
  `[::]:2113` gRPC port as the `EventStore` service.
- No liveness vs. readiness distinction at the protocol level. Both map to the single
  `grpc.health.v1.HealthCheckResponse` status field.
- No per-stream or per-writer-queue health signals (e.g., "writer channel is full").
  Only two states are exposed: `SERVING` and `NOT_SERVING`.
- No TLS or authentication changes. Health checks are unauthenticated, same as all other RPCs
  in v1.
- No changes to the on-disk format, storage engine, broker, or service layer beyond `main.rs`.

## User Stories

- As a systemd service manager on Oracle Cloud ARM, I want to poll the gRPC health endpoint so
  that I know when the server is ready before routing requests to it after a restart.
- As an operator, I want the server to report `NOT_SERVING` when it receives SIGTERM so that
  a load balancer stops sending new requests before the process shuts down.
- As a developer, I want a `grpc_health_probe -addr=[::]:2113` command to return `SERVING`
  when the server is up and `NOT_SERVING` during shutdown so I can verify deployment health
  in CI and staging.

## Technical Approach

### Crate addition

Add `tonic-health` to `Cargo.toml` under `[dependencies]`. This crate is maintained by the
tonic project and provides the `grpc.health.v1.Health` service implementation out of the box.
Pin to a version compatible with the existing `tonic = "0.13"` dependency (at time of writing,
`tonic-health = "0.13"`).

```toml
tonic-health = "0.13"
```

No changes to `build.rs` are needed; `tonic-health` ships pre-generated protobuf types and does
not require a `.proto` file in this crate.

### Changes to `src/main.rs`

`main.rs` is the only file that changes. The modification touches three locations in `main()`:

**1. Construction (after step 6, before step 8 — "Build the tonic Server").**

Call `tonic_health::server::health_reporter()` to obtain a `(HealthReporter, HealthService)` pair.
The `HealthService` is the gRPC handler; the `HealthReporter` is a cheap-to-clone handle for
setting status.

```rust
let (mut health_reporter, health_service) = tonic_health::server::health_reporter();
```

**2. Register service names before binding (`serve_with_incoming_shutdown`).**

Set `SERVING` for both the empty string (the conventional "server-wide" check) and the fully
qualified `"eventfold.EventStore"` service name. Both must be set; load balancers typically
query the empty service name, while targeted tooling queries the specific service name.

```rust
health_reporter
    .set_serving::<EventStoreServer<EventfoldService>>()
    .await;
health_reporter
    .set_service_status("", tonic_health::ServingStatus::Serving)
    .await;
```

`set_serving::<EventStoreServer<EventfoldService>>()` uses the `NamedService` trait
implementation on the tonic-generated server wrapper to derive the service name
`"eventfold.EventStore"` automatically, avoiding hardcoded string literals for the qualified
name.

Do this **after** the TCP listener is bound (step 9 in the existing numbered sequence) so the
`SERVING` status is set only once the socket is accepting connections.

**3. Add the health service to the tonic server builder.**

```rust
let server = tonic::transport::Server::builder()
    .add_service(health_service)
    .add_service(EventStoreServer::new(service));
```

**4. Transition to `NOT_SERVING` on shutdown signal.**

The existing `shutdown_signal()` function is a bare async function that resolves on
SIGINT/SIGTERM. Refactor the shutdown handling in `main()` to set `NOT_SERVING` before the
server begins draining:

```rust
// Wait for shutdown signal, then mark NOT_SERVING before the server stops.
shutdown_signal().await;
health_reporter
    .set_service_status("", tonic_health::ServingStatus::NotServing)
    .await;
health_reporter
    .set_not_serving::<EventStoreServer<EventfoldService>>()
    .await;
```

Because `serve_with_incoming_shutdown` takes a `Future` for its shutdown argument (not a
callback), the pattern changes slightly: drive the shutdown signal and the status transition
in a single async block passed to `serve_with_incoming_shutdown`:

```rust
server
    .serve_with_incoming_shutdown(incoming, async {
        shutdown_signal().await;
        health_reporter
            .set_service_status("", tonic_health::ServingStatus::NotServing)
            .await;
        health_reporter
            .set_not_serving::<EventStoreServer<EventfoldService>>()
            .await;
    })
    .await
    .unwrap_or_else(|e| { ... });
```

### File change table

| File | Change |
|------|--------|
| `Cargo.toml` | Add `tonic-health = "0.13"` to `[dependencies]` |
| `src/main.rs` | Construct `HealthReporter`/`HealthService`, register both service names, add health service to server, set `NOT_SERVING` inside shutdown closure |

No other files change. The library crate (`lib.rs`, `service.rs`, `store.rs`, etc.) is
untouched.

### Integration test

Add a test to `tests/` that spins up a real server (following the pattern of existing
integration tests) and uses `tonic-health`'s generated client to verify:

1. The health check returns `SERVING` for the empty service name while the server is running.
2. The health check returns `SERVING` for `"eventfold.EventStore"` while the server is running.

The `NOT_SERVING`-on-shutdown path is verified by the acceptance criteria below but does not
require a test that exercises real SIGTERM delivery; it is sufficient to verify the status
transition is wired correctly via direct `HealthReporter` manipulation in the shutdown closure.

## Acceptance Criteria

1. After adding `tonic-health = "0.13"` to `Cargo.toml`, `cargo build` completes with zero
   errors and zero warnings.
2. `cargo clippy --all-targets --all-features --locked -- -D warnings` passes with zero
   diagnostics after all changes.
3. A gRPC client calling `grpc.health.v1.Health/Check` with `service: ""` (empty string)
   against a running server receives a response with `status: SERVING`.
4. A gRPC client calling `grpc.health.v1.Health/Check` with `service: "eventfold.EventStore"`
   against a running server receives a response with `status: SERVING`.
5. A gRPC client calling `grpc.health.v1.Health/Check` with `service: "nonexistent.Service"`
   against a running server receives a gRPC `NOT_FOUND` status (this is the default behavior
   of `tonic-health` for unregistered service names and requires no additional code).
6. After the shutdown signal fires (inside the `serve_with_incoming_shutdown` shutdown future),
   the `HealthReporter` transitions both the empty-string service name and
   `"eventfold.EventStore"` to `NOT_SERVING` before the server stops accepting connections.
   This is verified by the integration test asserting `NOT_SERVING` is observable on a client
   that connects immediately after the status is set but before the server exits.
7. The existing five `EventStore` RPCs (`Append`, `ReadStream`, `ReadAll`, `SubscribeAll`,
   `SubscribeStream`) continue to pass all existing integration tests without modification.
8. `cargo test` passes with all tests green, including the new health check integration test.
9. The `SERVING` status is set **after** `tokio::net::TcpListener::bind` succeeds (i.e., not
   set prematurely during store open or writer spawn).

## Open Questions

- `tonic-health 0.13` is assumed to be compatible with `tonic 0.13`. If the published crate
  version does not match, the implementer should use whatever `tonic-health` version aligns
  with `tonic 0.13` per crates.io (both are maintained in the same `tonic` monorepo and use
  synchronized version numbers).
- The `NOT_SERVING` transition happens inside the shutdown future passed to
  `serve_with_incoming_shutdown`. The tonic server begins draining connections as soon as that
  future resolves. There is an inherent race: a health check in-flight at the exact moment of
  the status change could receive either `SERVING` or `NOT_SERVING`. This is acceptable for
  in-house workloads where the drain window is generous.

## Dependencies

- `tonic-health = "0.13"` (new dependency, crates.io)
- Existing `tonic = "0.13"` and `tokio` dependencies (no version changes required)
- `EventStoreServer` and `EventfoldService` from `src/service.rs` (unchanged)
- `shutdown_signal()` in `src/main.rs` (unchanged function; call site refactored into a closure)
