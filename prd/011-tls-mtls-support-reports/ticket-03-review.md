# Code Review: Ticket 3 -- Add TlsOptions struct and update Client::connect in eventfold-console

**Ticket:** 3 -- Add TlsOptions struct and update Client::connect in eventfold-console
**Impl Report:** prd/011-tls-mtls-support-reports/ticket-03-impl.md
**Date:** 2026-02-27 14:30
**Verdict:** APPROVED

---

## AC Coverage

| AC # | Description | Status | Notes |
|------|-------------|--------|-------|
| 1 | `TlsOptions` has fields `enabled: bool`, `ca_pem: Option<Vec<u8>>`, `identity: Option<(Vec<u8>, Vec<u8>)>`, derives `Debug`, `Clone`, `Default` | Met | Lines 28-36 of `client.rs`. All three fields present, all three derives present. |
| 2 | `Client::connect` signature is `pub async fn connect(addr: &str, tls: TlsOptions) -> Result<Self, ConsoleError>` | Met | Line 65 of `client.rs`. Exact signature match. |
| 3 | Test: `TlsOptions::default()` has `enabled = false`, `ca_pem = None`, `identity = None` | Met | Test `tls_options_default_has_tls_disabled` at line 366. Asserts all three default values. |
| 4 | Test: `Client::connect("http://[::1]:0", TlsOptions::default()).await` returns `Err(ConsoleError::ConnectionFailed(_))` | Met | Test `connect_plaintext_no_server_returns_connection_failed` at line 374. Uses `matches!` macro to verify variant. |
| 5 | Test: `Client::connect("http://[::1]:0", TlsOptions { enabled: true, ..Default::default() }).await` returns `Err(ConsoleError::ConnectionFailed(_))` | Met | Test `connect_tls_no_server_returns_connection_failed` at line 385. Constructs TLS-enabled options and verifies `ConnectionFailed`. |
| 6 | Existing `proto_to_event_record`, `client_is_debug`, `client_is_clone` unit tests pass without modification | Met | Verified via `cargo test -p eventfold-console` -- all 63 tests pass. These three tests are unchanged in the diff. |
| 7 | `eventfold-console/src/main.rs` passes `TlsOptions::default()` | Met | Line 55 of `main.rs`. Import added at line 23, call site updated. |
| 8 | Quality gates pass (build, lint, fmt, tests) | Met | Verified: `cargo clippy -p eventfold-console --all-targets --all-features --locked -- -D warnings` (clean), `cargo test -p eventfold-console` (63 pass, 0 fail), `cargo fmt -p eventfold-console --check` (clean). |

## Issues Found

### Critical (must fix before merge)
- None.

### Major (should fix, risk of downstream problems)
- None.

### Minor (nice to fix, not blocking)
- **Test uses `.unwrap_err()` in test code** (`client.rs` lines 377, 392): The connection tests call `result.unwrap_err()` after an `assert!(result.is_err())`. This is fine in tests, and the assertion guards against a panic, but a single `let Err(err) = result else { panic!("expected error") }` would be marginally cleaner. Not blocking.

## Suggestions (non-blocking)
- The plaintext branch in `connect()` (lines 83-89) is a subset of the TLS branch (lines 66-82) -- both use `Channel::from_shared` + `.connect()`. The only difference is the `.tls_config()` call. A small refactor could build the `Endpoint` first and conditionally apply `tls_config`, reducing duplication. Not required for this ticket; consider if the function grows.
- The `spawn_subscription` `or_else` -> `inspect_err` fix (line 252) is correct and behavior-preserving. Good catch on the pre-existing lint.

## Scope Check
- Files within scope: YES -- `eventfold-console/src/client.rs` and `eventfold-console/src/main.rs` are in scope.
- Scope creep detected: YES (minor, justified) -- Two additional files were modified:
  - `eventfold-console/src/app.rs`: collapsible `if` -> `let`-chain (lines 260-271)
  - `eventfold-console/src/views/mod.rs`: collapsible `if` -> `let`-chain (lines 28-32)
  - Both are trivial, behavior-preserving transformations required to pass `cargo clippy -D warnings` on the console crate. These pre-existing lints would block the quality gate for this ticket. The changes are mechanical and carry no behavioral risk.
- Additionally, `spawn_subscription` in `client.rs` had a pre-existing `bind_instead_of_map`/`manual_inspect` lint fixed (`or_else` -> `inspect_err`). This is within the in-scope file and is semantically equivalent.
- Unauthorized dependencies added: NO

## Risk Assessment
- Regression risk: LOW -- The plaintext path was changed from `EventStoreClient::connect(addr)` to `Channel::from_shared(addr).connect().await` + `EventStoreClient::new(channel)`. This is functionally equivalent (the former is a convenience wrapper around the latter in tonic), and both tests verify the error path. The TLS path is purely additive.
- Security concerns: NONE -- TLS configuration is straightforward delegation to tonic's `ClientTlsConfig`. No custom crypto, no secrets in code.
- Performance concerns: NONE -- `addr.to_string()` allocation is a one-time cost at connection time. No hot paths affected.
