# Implementation Report: Ticket 4 -- Add TLS CLI flags to eventfold-console and validate before connect

**Ticket:** 4 - Add TLS CLI flags to eventfold-console and validate before connect
**Date:** 2026-02-27 12:00
**Status:** COMPLETE

---

## Files Changed

### Created
- None

### Modified
- `eventfold-console/src/main.rs` - Added 4 TLS CLI fields to `Cli` struct, `validate_tls_flags()` function, `build_tls_options()` async function, wired validation and TLS options into `main()`, added 2 integration tests via `std::process::Command`

## Implementation Notes
- The `validate_tls_flags()` function checks that `--tls-client-cert` and `--tls-client-key` are either both present or both absent. On mismatch, it prints an error to stderr naming both flags and the missing one, then calls `std::process::exit(1)`.
- The `build_tls_options()` function returns `TlsOptions::default()` (plaintext) when no TLS flags are set. When TLS flags are present, it reads PEM files using `tokio::fs::read` and constructs `TlsOptions` with `enabled: true`.
- Validation runs before any connection attempt, ensuring the error message is clear and the process exits before trying to connect.
- The `expect()` call in `build_tls_options` for `tls_client_key` is safe because `validate_tls_flags` has already guaranteed the pairing invariant.
- Tests use `std::process::Command` to invoke the compiled binary, matching the ticket's requirement for subprocess-based testing. The binary path is derived from `CARGO_MANIFEST_DIR`.
- Used `#[arg(long, default_value_t = false)]` for the `--tls` boolean flag and `#[arg(long)]` for the `Option<PathBuf>` fields, following clap derive conventions.

## Acceptance Criteria
- [x] AC 1: `Cli` has `tls: bool`, `tls_ca: Option<PathBuf>`, `tls_client_cert: Option<PathBuf>`, `tls_client_key: Option<PathBuf>` with `#[arg(...)]` attributes -- lines 35-49 of main.rs
- [x] AC 2: When `--tls-client-cert` without `--tls-client-key` (or vice versa), prints error to stderr naming both flags and exits non-zero -- `validate_tls_flags()` at lines 61-79
- [x] AC 3: When no TLS flags set, `TlsOptions::default()` is passed -- `build_tls_options()` returns default on line 92
- [x] AC 4: When `--tls` + `--tls-ca /path`, CA PEM bytes read into `TlsOptions::ca_pem` -- lines 101-106
- [x] AC 5: When `--tls` + cert + key, both PEM files read into `TlsOptions::identity` -- lines 108-122
- [x] AC 6: Test: binary with `--tls-client-cert /dev/null` (no key) -> non-zero exit, stderr contains `"--tls-client-key"` -- test `cert_without_key_exits_nonzero_and_mentions_key_flag`
- [x] AC 7: Test: binary with `--tls-client-key /dev/null` (no cert) -> non-zero exit, stderr contains `"--tls-client-cert"` -- test `key_without_cert_exits_nonzero_and_mentions_cert_flag`
- [x] AC 8: Quality gates pass for eventfold-console crate

## Test Results
- Build: PASS (`cargo build -p eventfold-console` -- zero warnings)
- Clippy: PASS (`cargo clippy -p eventfold-console --all-targets --locked -- -D warnings` -- zero warnings)
- Fmt: PASS (`cargo fmt -p eventfold-console -- --check`)
- Tests: PASS (`cargo test -p eventfold-console` -- 65 tests, 0 failures)
- New tests added: 2 tests in `eventfold-console/src/main.rs` (`cert_without_key_exits_nonzero_and_mentions_key_flag`, `key_without_cert_exits_nonzero_and_mentions_cert_flag`)

## Concerns / Blockers
- Workspace-wide `cargo clippy --all-targets --all-features --locked -- -D warnings` fails due to pre-existing issues in `tests/tls_integration.rs` (unused variables, dead code from another ticket's incomplete work). This is outside the scope of this ticket. The eventfold-console crate itself passes all quality gates cleanly.
- The `tests/tls_integration.rs` file also has formatting issues (`cargo fmt --check` reports diffs). Again, outside this ticket's scope.
