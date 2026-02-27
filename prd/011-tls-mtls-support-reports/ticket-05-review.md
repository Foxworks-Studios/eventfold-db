# Code Review: Ticket 5 -- TLS and mTLS integration tests using rcgen

**Ticket:** 5 -- TLS and mTLS integration tests using rcgen
**Impl Report:** prd/011-tls-mtls-support-reports/ticket-05-impl.md
**Date:** 2026-02-27 14:30
**Verdict:** APPROVED

---

## AC Coverage

| AC # | Description | Status | Notes |
|------|-------------|--------|-------|
| 1 | `start_tls_test_server` generates CA + server cert via rcgen, starts tonic server with `ServerTlsConfig`, returns address + CA PEM + TempDir | Met | Lines 67-95 (`generate_tls_certs`), lines 145-179 (`start_tls_test_server`). CA is self-signed with `IsCa::Ca(Unconstrained)`, server cert signed by CA with IPv4/IPv6 loopback SANs + DNS "localhost". Server uses `ServerTlsConfig::new().identity(identity)`. Returns `(SocketAddr, Vec<u8>, TempDir)`. |
| 2 | `start_mtls_test_server` generates CA + server + client certs, calls `.client_ca_root(...)`, returns CA PEM + client cert/key PEM + address | Met | Lines 101-139 (`generate_mtls_certs`), lines 185-228 (`start_mtls_test_server`). Uses `ServerTlsConfig::new().identity(identity).client_ca_root(ca_cert)`. Returns `(SocketAddr, Vec<u8>, Vec<u8>, Vec<u8>, TempDir)`. |
| 3 | Test `tls_client_append_and_read_all`: TLS client connects, Append + ReadAll succeed | Met | Lines 232-272. Connects with `ClientTlsConfig` + correct CA + `domain_name("localhost")`. Appends one event, reads back, asserts event type and stream ID match. |
| 4 | Test `tls_plaintext_client_rejected`: plaintext client to TLS server gets `Err(_)` | Met | Lines 276-308. Uses `http://` scheme. Two-level match: if connect succeeds, RPC must fail with transport-level error code; if connect fails, that is also acceptable. |
| 5 | Test `mtls_client_no_cert_rejected`: TLS client without client identity rejected by mTLS server | Met | Lines 312-352. Connects with correct CA but no `.identity()`. Same two-level match pattern as AC 4. |
| 6 | Test `mtls_client_valid_cert_accepted`: mTLS client with valid cert, Append + ReadAll succeed | Met | Lines 356-398. Connects with CA + client `Identity::from_pem(...)`. Appends, reads, asserts correctness. |
| 7 | Test `plaintext_server_plaintext_client_still_works`: regression guard | Met | Lines 402-454. Starts server without TLS, connects with `http://`, appends and reads successfully. |
| 8 | Test `tls_wrong_ca_rejected`: client with wrong CA is rejected | Met | Lines 458-508. Generates a separate CA, attempts connection -- same two-level match pattern. |
| 9 | Quality gates pass (build, lint, fmt, tests) | Met | Independently verified: `cargo fmt --check` clean, `cargo clippy --all-targets --all-features --locked -- -D warnings` clean, `cargo test` all 239 tests pass (180 unit + 23 grpc_service + 2 broker + 6 idempotent + 6 server_binary + 6 tls_integration + 1 writer + 15 main bin). |

## Issues Found

### Critical (must fix before merge)
- None.

### Major (should fix, risk of downstream problems)
- None.

### Minor (nice to fix, not blocking)
- **Duplicated CA/server cert generation logic between `generate_tls_certs` and `generate_mtls_certs`** (lines 67-95 vs 101-122). The CA generation and server cert generation are identical in both functions. A shared helper could reduce the ~30 lines of duplication. However, the impl report explicitly justifies this: the CA key is needed to sign both server and client certs in the mTLS case, and `generate_tls_certs` does not expose it. The duplication is localized to test code, so this is acceptable as-is.

## Suggestions (non-blocking)

- The `plaintext_server_plaintext_client_still_works` test (lines 402-454) duplicates the server setup from `grpc_service.rs::start_test_server`. The ticket AC explicitly requires this test in `tls_integration.rs` and it does not call the existing `start_test_server` helper (which is private to `grpc_service.rs`). If test helpers are ever extracted into a shared module, this duplication could be resolved. Not a concern for this ticket.
- The `make_proposed` and `no_stream` helpers are duplicated from `grpc_service.rs`. Same reasoning applies -- integration test files are independent compilation units and cannot share private helpers without a shared test utilities module. Acceptable for now.

## Scope Check
- Files within scope: YES -- only `tests/tls_integration.rs` was created. No other files were modified by this ticket.
- Scope creep detected: NO
- Unauthorized dependencies added: NO -- `rcgen = "0.13"` was added by Ticket 1, not this ticket.

## Risk Assessment
- Regression risk: LOW -- This ticket only adds a new integration test file. No production code was modified. All 233 pre-existing tests continue to pass.
- Security concerns: NONE -- Test certificates are ephemeral and generated in-process using rcgen. No static fixture files or hardcoded secrets.
- Performance concerns: NONE -- Certificate generation and TLS handshakes are per-test overhead (~50ms sleep per test for server startup), which is standard for this codebase's integration test pattern.

## Additional Notes

The implementation is clean and well-structured:

1. **rcgen API usage is correct** for version 0.13: `KeyPair::generate()`, `CertificateParams::new(subjects)`, `params.self_signed(&key)`, `params.signed_by(&key, &ca_cert, &ca_key)`. The CA has `is_ca = IsCa::Ca(BasicConstraints::Unconstrained)` set correctly.

2. **SAN configuration is thorough**: server certs include IPv6 loopback (`::1`), IPv4 loopback (`127.0.0.1`), and DNS `localhost`. This ensures hostname verification passes regardless of address resolution.

3. **Negative test pattern is robust**: the two-level match (connection failure OR RPC failure) correctly handles the timing variability in how tonic/rustls surface TLS failures. The accepted error codes (`Unavailable`, `Internal`, `Unknown`) cover all plausible transport-level error mappings.

4. **Test isolation is correct**: each test uses `tempfile::tempdir()` for storage and ephemeral port binding via `[::1]:0`.

5. **Follows established patterns**: the test file mirrors `tests/grpc_service.rs` in structure (helper functions, `TcpListenerStream`, `serve_with_incoming`, 50ms sleep).

Running test count: 239 total (180 unit + 59 integration).
