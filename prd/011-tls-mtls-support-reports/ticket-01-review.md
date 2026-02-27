# Code Review: Ticket 1 -- Enable tonic TLS feature and add TlsConfig struct with env-var parsing

**Ticket:** 1 -- Enable tonic TLS feature and add TlsConfig struct with env-var parsing
**Impl Report:** prd/011-tls-mtls-support-reports/ticket-01-impl.md
**Date:** 2026-02-27 16:30
**Verdict:** APPROVED

---

## AC Coverage

| AC # | Description | Status | Notes |
|------|-------------|--------|-------|
| 1 | `TlsConfig` is a private struct with `cert_path: PathBuf`, `key_path: PathBuf`, `ca_path: Option<PathBuf>`, derives `Debug`, `Clone`, `PartialEq` | Met | `src/main.rs` lines 12-21: struct is private (no `pub`), all three fields present with correct types, `#[derive(Debug, Clone, PartialEq)]` on line 12. |
| 2 | `Config` gains `tls: Option<TlsConfig>` field | Met | `src/main.rs` line 47: `tls: Option<TlsConfig>` added to `Config` struct. |
| 3 | `from_env()` returns `Ok` with `tls: None` when no TLS vars set; existing tests pass | Met | Test `from_env_tls_none_when_no_tls_vars` (line 411) explicitly verifies `config.tls == None`. All 8 pre-existing tests updated with `clear_tls_env()` call; all 232 tests pass. |
| 4 | cert+key set, CA unset -> `Ok` with `TlsConfig { cert_path, key_path, ca_path: None }` | Met | Test `from_env_tls_cert_and_key_without_ca` (line 518) sets CERT+KEY, unsets CA, asserts exact `TlsConfig` with `ca_path: None`. Match arm at line 123 handles this case. |
| 5 | cert+key+CA all set -> `Ok` with `TlsConfig { ..., ca_path: Some(...) }` | Met | Test `from_env_tls_cert_key_and_ca` (line 495) sets all three, asserts exact `TlsConfig` with `ca_path: Some(...)`. Match arm at line 129 handles this case. |
| 6 | cert set, key missing -> `Err` containing `"EVENTFOLD_TLS_KEY"` | Met | Test `from_env_tls_cert_without_key_returns_err` (line 474) asserts `is_err()` and message contains `"EVENTFOLD_TLS_KEY"`. Match arm at line 135 returns error with this text. |
| 7 | key set, cert missing -> `Err` containing `"EVENTFOLD_TLS_CERT"` | Met | Test `from_env_tls_key_without_cert_returns_err` (line 453) asserts `is_err()` and message contains `"EVENTFOLD_TLS_CERT"`. Match arm at line 141 returns error with this text. |
| 8 | CA set, both cert+key missing -> `Err` containing both `"EVENTFOLD_TLS_CERT"` and `"EVENTFOLD_TLS_KEY"` | Met | Test `from_env_tls_ca_only_returns_err` (line 425) asserts both strings in error. Match arm at line 147 returns message containing both. |
| 9 | `cargo build` at workspace root produces zero warnings | Met | Verified: `cargo build` completes with zero warnings. |
| 10 | Quality gates pass (build, lint, fmt, tests) | Met | Verified independently: `cargo build` (zero warnings), `cargo clippy --all-targets --all-features --locked -- -D warnings` (zero warnings), `cargo fmt --check` (clean), `cargo test` (232 passed, 0 failed). |

## Issues Found

### Critical (must fix before merge)

None.

### Major (should fix, risk of downstream problems)

None.

### Minor (nice to fix, not blocking)

None.

## Suggestions (non-blocking)

- The `tls-ring` deviation from the ticket's `tls` feature name is correct and well-documented in the impl report. Tonic 0.13 does not have a `tls` feature; `tls-ring` is the right choice. Future tickets should use `tls-ring` consistently. This is a factual correction, not a scope violation.

- The match-based tuple approach for TLS env-var validation is clean, exhaustive, and easy to read. The wildcard `_` on the CA position in the `(Some(_), None, _)` and `(None, Some(_), _)` arms correctly collapses two sub-cases each (CA present or absent) into a single error path, which is the right behavior since the primary error is the missing cert/key, not the CA.

## Scope Check

- Files within scope: YES -- `Cargo.toml`, `eventfold-console/Cargo.toml`, `src/main.rs` are all listed in the ticket scope.
- `Cargo.lock` also changed as an automatic transitive effect of adding `tls-ring` features. This is expected and not a scope violation.
- `prd/011-tls-mtls-support.md` appears in the diff as a new untracked file (the PRD itself). This is a workflow artifact, not a code change.
- Scope creep detected: NO
- Unauthorized dependencies added: NO -- `rcgen` as a dev-dependency was explicitly called for in the ticket scope. `tls-ring` feature on `tonic` was the ticket's primary objective (with the correct feature name correction).

## Risk Assessment

- Regression risk: LOW -- All 232 existing tests pass. The only production code change is adding a new `tls` field to `Config` and parsing logic in `from_env()`. The `main()` function is completely unchanged; TLS wiring into server startup is deferred to Ticket 2. Existing tests were updated with `clear_tls_env()` to prevent env-var leakage.
- Security concerns: NONE -- No TLS handshake logic introduced yet; this ticket only parses file paths from environment variables. File reading and certificate loading happen in Ticket 2.
- Performance concerns: NONE -- Three additional `std::env::var()` calls at startup is negligible.
