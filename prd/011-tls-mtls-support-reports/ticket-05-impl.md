# Implementation Report: Ticket 5 -- TLS and mTLS integration tests using rcgen

**Ticket:** 5 - TLS and mTLS integration tests using rcgen
**Date:** 2026-02-27 12:00
**Status:** COMPLETE

---

## Files Changed

### Created
- `tests/tls_integration.rs` - Integration test file with 6 test cases exercising TLS and mTLS server paths, plus helper functions for certificate generation and server startup.

### Modified
- None

## Implementation Notes
- Used `rcgen` 0.13 API: `KeyPair::generate()`, `CertificateParams::new(subjects)`, `params.self_signed(&key)`, `params.signed_by(&key, &ca_cert, &ca_key)` -- all work correctly as described in the prior work summary.
- Server certificates include `SanType::IpAddress` for both `[::1]` (IPv6 loopback) and `127.0.0.1` (IPv4 loopback), plus `SanType::DnsName("localhost")` to ensure hostname verification passes regardless of how tonic resolves the address.
- `generate_tls_certs()` and `generate_mtls_certs()` are separate functions rather than composing one from the other, because the CA key is needed to sign both server and client certificates in the mTLS case, and `generate_tls_certs` does not expose the CA key.
- Negative tests (rejection cases) use a two-level match pattern: first check if `Channel::connect` fails (which is acceptable -- TLS handshake rejection), then if it succeeds, attempt an RPC and assert the error code is a transport-level error (`Unavailable`, `Internal`, or `Unknown`). This handles the timing variability in how tonic/rustls surface TLS failures.
- Followed existing patterns from `tests/grpc_service.rs` for server setup (ephemeral port via `[::1]:0`, `TcpListenerStream`, `serve_with_incoming`, 50ms sleep for startup).
- `TlsCerts` and `MtlsCerts` structs are private to the test file and hold PEM bytes as `Vec<u8>`.

## Acceptance Criteria
- [x] AC 1: `start_tls_test_server` generates a self-signed CA + server cert/key via `rcgen`, starts an in-process tonic server with `ServerTlsConfig`, and returns the server address + CA PEM bytes + `TempDir`. (lines 141-179)
- [x] AC 2: `start_mtls_test_server` does the same but additionally generates a client cert/key signed by the same CA and calls `.client_ca_root(...)` on the `ServerTlsConfig`; returns the CA PEM, client cert PEM, and client key PEM alongside the address. (lines 181-228)
- [x] AC 3: Test `tls_client_append_and_read_all` -- starts TLS server, connects with `ClientTlsConfig` + CA PEM, calls Append then ReadAll, asserts the appended event is returned. (lines 232-272)
- [x] AC 4: Test `tls_plaintext_client_rejected` -- starts TLS server, connects with plaintext `http://`, asserts the result is `Err(_)`. (lines 276-308)
- [x] AC 5: Test `mtls_client_no_cert_rejected` -- starts mTLS server, connects with TLS (correct CA, no client identity), asserts the result is `Err(_)`. (lines 312-352)
- [x] AC 6: Test `mtls_client_valid_cert_accepted` -- starts mTLS server, connects with TLS + client `Identity`, calls Append then ReadAll, asserts the appended event is returned. (lines 356-398)
- [x] AC 7: Test `plaintext_server_plaintext_client_still_works` -- starts plaintext server, connects with plaintext client, appends and reads, asserts success. (lines 402-454)
- [x] AC 8: Test `tls_wrong_ca_rejected` -- starts TLS server, connects with a different self-signed CA, asserts the result is `Err(_)`. (lines 458-508)
- [x] AC 9: Quality gates pass (build, lint, fmt, tests).

## Test Results
- Lint: PASS (`cargo clippy --all-targets --all-features --locked -- -D warnings` -- zero warnings)
- Tests: PASS (239 total: 180 unit + 23 grpc_service + 2 broker + 6 idempotent + 6 server_binary + 6 tls_integration + 1 writer + 15 main bin)
- Build: PASS (`cargo build` -- zero warnings)
- Fmt: PASS (`cargo fmt --check` -- no diffs)
- New tests added: 6 tests in `tests/tls_integration.rs`:
  - `tls_client_append_and_read_all`
  - `tls_plaintext_client_rejected`
  - `mtls_client_no_cert_rejected`
  - `mtls_client_valid_cert_accepted`
  - `plaintext_server_plaintext_client_still_works`
  - `tls_wrong_ca_rejected`

## Concerns / Blockers
- None
