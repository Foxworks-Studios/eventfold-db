# Implementation Report: Ticket 2 -- Wire ServerTlsConfig into server startup

**Ticket:** 2 - Wire ServerTlsConfig into server startup
**Date:** 2026-02-27 14:30
**Status:** COMPLETE

---

## Files Changed

### Created
- None

### Modified
- `src/main.rs` - Added TLS wiring in `main()`: conditionally reads cert/key/CA files from disk, builds `ServerTlsConfig` with `Identity` and optional `client_ca_root`, attaches to server builder before `add_service`. Added `tracing::info!` for TLS/mTLS mode. Added one new test `binary_exits_nonzero_when_tls_cert_file_missing`.

## Implementation Notes
- The TLS wiring is inserted between the `Server::builder()` call and `add_service()` call (step 8 in the startup sequence). When `config.tls` is `None`, the builder passes through untouched -- zero behavioral change for plaintext mode.
- File reads use `tokio::fs::read` (async). On failure, `tracing::error!` logs the path and error, then `std::process::exit(1)` terminates the process. This matches the existing error pattern used for Store open failure and bind failure.
- The `tls_config()` call on the builder can also fail (e.g., invalid PEM data). This is handled with the same `unwrap_or_else` + `tracing::error!` + `process::exit(1)` pattern.
- The new binary integration test (`binary_exits_nonzero_when_tls_cert_file_missing`) only asserts on non-zero exit status, not stderr content, because `process::exit(1)` does not flush tracing subscriber buffers (Rust destructors are not run). This is consistent with how the existing `binary_exits_nonzero_without_eventfold_data` test works (that test checks stderr because its error path uses `eprintln!`, not `tracing::error!`).
- TLS types used: `tonic::transport::Identity`, `tonic::transport::ServerTlsConfig`, `tonic::transport::Certificate` -- all available via the `tls-ring` feature already enabled in Cargo.toml by Ticket 1.

## Acceptance Criteria
- [x] AC 1: When `config.tls` is `None`, server builder is constructed identically to prior code - the `if let Some(ref tls) = config.tls` block is skipped entirely; builder flows straight to `add_service`. All 23 grpc_service integration tests pass unchanged.
- [x] AC 2: When `config.tls` is `Some` with no `ca_path`, `tls_config(ServerTlsConfig::new().identity(Identity::from_pem(cert, key)))` is called before `add_service` - implemented at lines 253-297.
- [x] AC 3: When `config.tls` is `Some` with `ca_path`, `.client_ca_root(Certificate::from_pem(ca))` is additionally called - implemented at lines 275-285.
- [x] AC 4: If `tokio::fs::read` fails, `tracing::error!` with path and error, then `process::exit(1)` - implemented for cert (254-261), key (263-270), and CA (276-283). Verified by `binary_exits_nonzero_when_tls_cert_file_missing` test.
- [x] AC 5: `tracing::info!` emitted for TLS mode ("TLS enabled") or mTLS mode ("mTLS enabled") - lines 286 and 288.
- [x] AC 6: `start_test_server` helper in `tests/grpc_service.rs` works without modification - all 23 tests pass.
- [x] AC 7: Full TLS round-trip verification deferred to Ticket 5 - noted, no action taken.
- [x] AC 8: Quality gates pass - build, clippy, fmt, all 233 tests pass.

## Test Results
- Lint: PASS (`cargo clippy --all-targets --all-features --locked -- -D warnings`)
- Tests: PASS (233 total: 180 lib + 15 binary unit + 2 broker + 23 grpc_service + 6 idempotent + 6 server_binary + 1 writer)
- Build: PASS (zero warnings)
- Fmt: PASS (`cargo fmt --check`)
- New tests added: `src/main.rs::tests::binary_exits_nonzero_when_tls_cert_file_missing` - verifies the binary exits non-zero when TLS cert/key paths point to nonexistent files.

## Concerns / Blockers
- None
