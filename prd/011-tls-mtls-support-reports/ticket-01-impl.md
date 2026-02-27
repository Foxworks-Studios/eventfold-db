# Implementation Report: Ticket 1 -- Enable tonic TLS feature and add TlsConfig struct with env-var parsing

**Ticket:** 1 - Enable tonic TLS feature and add TlsConfig struct with env-var parsing
**Date:** 2026-02-27 16:00
**Status:** COMPLETE

---

## Files Changed

### Created
- None

### Modified
- `Cargo.toml` - Changed `tonic = "0.13"` to `tonic = { version = "0.13", features = ["tls-ring"] }`, added `rcgen = "0.13"` to `[dev-dependencies]`
- `eventfold-console/Cargo.toml` - Changed `tonic = "0.13"` to `tonic = { version = "0.13", features = ["tls-ring"] }`
- `src/main.rs` - Added `TlsConfig` struct, `tls: Option<TlsConfig>` field on `Config`, TLS env-var parsing in `Config::from_env()`, cleared TLS env vars in all existing tests, added 6 new unit tests

## Implementation Notes
- **tonic TLS feature name**: The ticket specified `features = ["tls"]` but tonic 0.13 does NOT have a `tls` feature. The available TLS features are `tls-ring`, `tls-aws-lc`, `tls-native-roots`, and `tls-webpki-roots`. Used `tls-ring` as it is the standard portable crypto backend that enables `ServerTlsConfig`, `ClientTlsConfig`, `Identity`, and `Certificate` (all gated behind the internal `_tls-any` feature). This deviation from the ticket is necessary -- using `features = ["tls"]` would cause a cargo build failure.
- **Match-based validation**: The TLS env-var parsing uses an exhaustive `match` on the `(cert, key, ca)` tuple, covering all valid and invalid combinations cleanly. No fallthrough patterns needed.
- **Existing test isolation**: All 8 existing serial tests now call `clear_tls_env()` to remove the 3 TLS env vars, preventing cross-test pollution. The `binary_exits_nonzero_without_eventfold_data` test uses `.env_remove()` on the Command builder for the same purpose.
- **Doc comments updated**: Both `Config` struct and `from_env()` method doc comments now document the 3 TLS environment variables and their error conditions.

## Acceptance Criteria
- [x] AC 1: `TlsConfig` is a private struct with `cert_path: PathBuf`, `key_path: PathBuf`, `ca_path: Option<PathBuf>`. Derives `Debug`, `Clone`, `PartialEq`.
- [x] AC 2: `Config` gains `tls: Option<TlsConfig>` field.
- [x] AC 3: `Config::from_env()` returns `Ok` with `tls: None` when no TLS vars set. Existing tests pass. -- Test: `from_env_tls_none_when_no_tls_vars`
- [x] AC 4: cert+key set, CA unset -> `Ok` with `TlsConfig { cert_path, key_path, ca_path: None }` -- Test: `from_env_tls_cert_and_key_without_ca`
- [x] AC 5: cert+key+CA all set -> `Ok` with `TlsConfig { ..., ca_path: Some(...) }` -- Test: `from_env_tls_cert_key_and_ca`
- [x] AC 6: cert set, key missing -> `Err` containing "EVENTFOLD_TLS_KEY" -- Test: `from_env_tls_cert_without_key_returns_err`
- [x] AC 7: key set, cert missing -> `Err` containing "EVENTFOLD_TLS_CERT" -- Test: `from_env_tls_key_without_cert_returns_err`
- [x] AC 8: CA set, both cert+key missing -> `Err` containing both "EVENTFOLD_TLS_CERT" and "EVENTFOLD_TLS_KEY" -- Test: `from_env_tls_ca_only_returns_err`
- [x] AC 9: `cargo build` at workspace root produces zero warnings.
- [x] AC 10: Quality gates pass (build, lint, fmt, tests).

## Test Results
- Lint: PASS (`cargo clippy --all-targets --all-features --locked -- -D warnings` -- zero warnings)
- Tests: PASS (232 total: 180 + 14 + 2 + 23 + 6 + 6 + 1 = 232, all green)
- Build: PASS (zero warnings)
- Fmt: PASS
- New tests added:
  - `src/main.rs::tests::from_env_tls_none_when_no_tls_vars`
  - `src/main.rs::tests::from_env_tls_cert_and_key_without_ca`
  - `src/main.rs::tests::from_env_tls_cert_key_and_ca`
  - `src/main.rs::tests::from_env_tls_cert_without_key_returns_err`
  - `src/main.rs::tests::from_env_tls_key_without_cert_returns_err`
  - `src/main.rs::tests::from_env_tls_ca_only_returns_err`

## Concerns / Blockers
- **tonic feature name mismatch**: The ticket and PRD both reference `features = ["tls"]` which does not exist in tonic 0.13. Used `tls-ring` instead, which is the correct feature that enables TLS support via the `ring` crypto backend. Downstream tickets that reference the `tls` feature should also use `tls-ring`. This is a factual correction, not a scope change.
- None otherwise.
