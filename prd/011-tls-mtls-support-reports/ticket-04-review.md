# Code Review: Ticket 4 -- Add TLS CLI flags to eventfold-console and validate before connect

**Ticket:** 4 -- Add TLS CLI flags to eventfold-console and validate before connect
**Impl Report:** prd/011-tls-mtls-support-reports/ticket-04-impl.md
**Date:** 2026-02-27 14:30
**Verdict:** APPROVED

---

## AC Coverage

| AC # | Description | Status | Notes |
|------|-------------|--------|-------|
| 1 | `Cli` has `tls: bool`, `tls_ca: Option<PathBuf>`, `tls_client_cert: Option<PathBuf>`, `tls_client_key: Option<PathBuf>` with `#[arg(...)]` attributes | Met | Lines 35-49 of main.rs. `tls` uses `#[arg(long, default_value_t = false)]`, the three `Option<PathBuf>` fields use `#[arg(long)]`. All correct. |
| 2 | cert-without-key (or vice versa) prints stderr with both flag names and exits non-zero | Met | `validate_tls_flags()` at lines 64-79. The `eprintln!` on line 73-76 contains both `"--tls-client-cert"` and `"--tls-client-key"` literally, plus names the `{missing}` one. `std::process::exit(1)` ensures non-zero exit. Called at line 133 before any connection. |
| 3 | No TLS flags set -> `TlsOptions::default()` passed | Met | `build_tls_options()` line 91: when `!cli.tls && cli.tls_ca.is_none() && cli.tls_client_cert.is_none()`, returns `TlsOptions::default()`. Since `validate_tls_flags` guarantees cert/key pairing, `tls_client_key` alone cannot be Some here. Correct. |
| 4 | `--tls` + `--tls-ca /path` -> CA PEM read into `TlsOptions::ca_pem` | Met | Lines 101-106: `tokio::fs::read(ca_path)` with `.with_context()`, assigned to `opts.ca_pem = Some(ca_pem)`. |
| 5 | `--tls` + cert + key -> both PEM files read into `TlsOptions::identity` | Met | Lines 108-122: reads cert, `.expect()` for key path (safe after validation), reads key, assigns `opts.identity = Some((cert_pem, key_pem))`. |
| 6 | Test: binary with `--tls-client-cert /dev/null` (no key) -> non-zero exit, stderr contains `"--tls-client-key"` | Met | Test `cert_without_key_exits_nonzero_and_mentions_key_flag` at lines 283-300. Uses `std::process::Command`, asserts `!output.status.success()` and `stderr.contains("--tls-client-key")`. Verified passing. |
| 7 | Test: binary with `--tls-client-key /dev/null` (no cert) -> non-zero exit, stderr contains `"--tls-client-cert"` | Met | Test `key_without_cert_exits_nonzero_and_mentions_cert_flag` at lines 303-320. Same pattern, asserts stderr contains `"--tls-client-cert"`. Verified passing. |
| 8 | Quality gates pass for eventfold-console crate | Met | Verified: `cargo build -p eventfold-console` (zero warnings), `cargo clippy -p eventfold-console --all-targets --locked -- -D warnings` (clean), `cargo fmt -p eventfold-console -- --check` (clean), `cargo test -p eventfold-console` (65 passed, 0 failed). Workspace-wide gates also pass clean. |

## Issues Found

### Critical (must fix before merge)
- None

### Major (should fix, risk of downstream problems)
- None

### Minor (nice to fix, not blocking)
- **`#[arg(long, default_value_t = false)]` is redundant for `bool` fields** (line 36): In clap derive, a `bool` field with `#[arg(long)]` already defaults to `false` and acts as a flag. The `default_value_t = false` is unnecessary. This is purely cosmetic and does not affect behavior.
- **Impl report incorrectly claims workspace-wide clippy fails** (impl report line 43): The report states `tests/tls_integration.rs` causes workspace-wide clippy failures, but `cargo clippy --all-targets --all-features --locked -- -D warnings` passes clean at workspace level. This is a misleading claim in the report but has no code impact.

## Suggestions (non-blocking)
- The `binary_path()` test helper manually constructs the path via `CARGO_MANIFEST_DIR`. An alternative is to use `assert_cmd` or `escargot` crates for more robust binary discovery, but the current approach is fine for this crate and consistent with avoiding unnecessary dependencies.
- The implicit TLS enablement when `--tls-ca` or `--tls-client-cert`/`--tls-client-key` are provided without `--tls` is a reasonable UX choice (providing any TLS option implies TLS). Consider documenting this in the CLI help text for the `--tls` flag (e.g., "Implied when any --tls-* option is provided").

## Scope Check
- Files within scope: YES -- only `eventfold-console/src/main.rs` was modified, matching the ticket scope.
- Scope creep detected: NO
- Unauthorized dependencies added: NO

## Risk Assessment
- Regression risk: LOW -- Existing behavior is unchanged when no TLS flags are passed. The `validate_tls_flags` call is additive (no-op when both cert and key are absent or both present). The `build_tls_options` returns `TlsOptions::default()` when no TLS flags are set, preserving the pre-existing plaintext path.
- Security concerns: NONE -- PEM files are read from user-specified paths with proper error handling. No secrets stored in code.
- Performance concerns: NONE -- File reads happen once at startup.
