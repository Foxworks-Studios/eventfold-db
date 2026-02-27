# Code Review: Ticket 2 -- Wire ServerTlsConfig into server startup

**Ticket:** 2 -- Wire ServerTlsConfig into server startup
**Impl Report:** prd/011-tls-mtls-support-reports/ticket-02-impl.md
**Date:** 2026-02-27 17:15
**Verdict:** APPROVED

---

## AC Coverage

| AC # | Description | Status | Notes |
|------|-------------|--------|-------|
| 1 | When `config.tls` is `None`, server builder is constructed identically to prior code -- no regression | Met | `src/main.rs` lines 251-295: the `if let Some(ref tls) = config.tls` block is skipped entirely when `tls` is `None`; the builder flows straight to `add_service` on line 297. All 23 grpc_service integration tests pass unchanged (plaintext path). |
| 2 | When `config.tls` is `Some` with no `ca_path`, `tls_config(ServerTlsConfig::new().identity(...))` is called before `add_service` | Met | Lines 253-294: cert and key are read with `tokio::fs::read`, `Identity::from_pem(cert, key)` builds the identity, `ServerTlsConfig::new().identity(identity)` is constructed, and `builder.tls_config(tls_config)` is called. The CA branch at line 275 is skipped when `ca_path` is `None`. |
| 3 | When `config.tls` is `Some` with `ca_path`, `.client_ca_root(Certificate::from_pem(ca))` is additionally called | Met | Lines 275-285: `tokio::fs::read(ca_path)` reads the CA cert, `Certificate::from_pem(ca)` wraps it, `tls_config = tls_config.client_ca_root(ca_cert)` attaches it before the `tls_config()` call on the builder. |
| 4 | If `tokio::fs::read` fails, `tracing::error!` with path and error, then `process::exit(1)` | Met | Three `.unwrap_or_else` handlers: cert (lines 254-261), key (lines 263-270), CA (lines 276-283). Each logs path + error via `tracing::error!` and calls `std::process::exit(1)`. Additionally, `builder.tls_config()` failure is caught at line 291-294 with the same pattern. The binary test `binary_exits_nonzero_when_tls_cert_file_missing` (line 586) verifies non-zero exit with nonexistent cert/key paths. |
| 5 | `tracing::info!` emitted for TLS mode or mTLS mode | Met | Line 286: `tracing::info!("mTLS enabled")` when `ca_path` is present. Line 288: `tracing::info!("TLS enabled")` when `ca_path` is absent. |
| 6 | `start_test_server` helper in `tests/grpc_service.rs` continues to work without modification | Met | All 23 grpc_service tests pass. The `start_test_server` helper was not touched -- the plaintext path is structurally identical (builder -> add_service -> serve). |
| 7 | Full TLS round-trip verification deferred to Ticket 5 | Met | No TLS integration tests with real certs were added. Noted in impl report. Correctly deferred. |
| 8 | Quality gates pass | Met | Verified independently: `cargo build` (zero warnings), `cargo clippy --all-targets --all-features --locked -- -D warnings` (clean), `cargo fmt --check` (clean), `cargo test` (233 passed, 0 failed). |

## Issues Found

### Critical (must fix before merge)

None.

### Major (should fix, risk of downstream problems)

None.

### Minor (nice to fix, not blocking)

1. **`process::exit(1)` skips destructors.** The `unwrap_or_else(|e| { tracing::error!(...); std::process::exit(1) })` pattern at lines 254, 263, 276, and 291 terminates the process without running Rust destructors, meaning tracing subscriber flush is not guaranteed and any open file handles are abandoned. The impl report correctly notes this tradeoff and acknowledges that the binary test can only assert exit status, not stderr content. This matches the existing error pattern used for `Store::open` failure (line 224) and listener bind failure (line 302-308), so it is consistent with the codebase. Not a correctness issue -- the OS reclaims all resources on process exit -- but worth noting for future reference if structured logging to a file is added.

## Suggestions (non-blocking)

- The `builder.tls_config(tls_config).unwrap_or_else(...)` at line 291 handles the case where tonic rejects the TLS configuration (e.g., invalid PEM data). This is a good defensive addition that goes beyond the literal AC wording (which only mentions file read failures). Well done.

- The `binary_exits_nonzero_when_tls_cert_file_missing` test uses `cargo run --bin eventfold-db --quiet` which triggers a compilation check on each run. This is consistent with the existing `binary_exits_nonzero_without_eventfold_data` test pattern. In a large CI pipeline, pre-building the binary and invoking it directly would be faster, but this is not a concern for this project's test count.

## Scope Check

- Files within scope: YES -- Only `src/main.rs` was modified, which is the sole file listed in the ticket scope.
- Scope creep detected: NO -- The changes are limited to (a) TLS wiring in `main()`, (b) one new binary integration test, and (c) updating existing tests to clear TLS env vars (necessary to prevent test pollution from Ticket 1's new env vars).
- Unauthorized dependencies added: NO -- No new dependencies. The `tonic::transport::{Identity, ServerTlsConfig, Certificate}` types are available via the `tls-ring` feature enabled in Ticket 1.

## Risk Assessment

- Regression risk: LOW -- The plaintext path is structurally unchanged (just split into `let mut builder` + `let server = builder.add_service(...)` instead of a single chained expression). All 233 tests pass, including the 23 grpc_service integration tests that exercise the full plaintext gRPC round-trip.
- Security concerns: NONE -- The TLS implementation delegates entirely to tonic's `ServerTlsConfig`, which uses rustls under the hood. No custom cryptographic logic. File reads use standard `tokio::fs::read` with no path traversal concerns (paths come from operator-controlled environment variables).
- Performance concerns: NONE -- The TLS setup is a one-time startup cost (3 async file reads + `ServerTlsConfig` construction). No impact on per-request hot path.
