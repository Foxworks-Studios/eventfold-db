# Tickets for PRD 011: TLS and mTLS Support

**Source PRD:** prd/011-tls-mtls-support.md
**Created:** 2026-02-27
**Total Tickets:** 6
**Estimated Total Complexity:** 13 (S=1, M=2, L=3: 1+3+2+2+3+2=13)

---

### Ticket 1: Enable tonic TLS feature and add TlsConfig struct with env-var parsing

**Description:**
Enable the `tls` feature on the `tonic` dependency in both workspace `Cargo.toml` files and add
`rcgen` as a root dev-dependency. Add the `TlsConfig` struct to `src/main.rs` and extend
`Config` with a `tls: Option<TlsConfig>` field. Implement the validation logic in
`Config::from_env()` covering all three env vars (`EVENTFOLD_TLS_CERT`, `EVENTFOLD_TLS_KEY`,
`EVENTFOLD_TLS_CA`) including the partial-set error cases. No server startup changes yet.

**Scope:**
- Modify: `Cargo.toml` (root) -- add `features = ["tls"]` to `tonic`, add `rcgen = "0.13"` to
  `[dev-dependencies]`
- Modify: `eventfold-console/Cargo.toml` -- add `features = ["tls"]` to `tonic`
- Modify: `src/main.rs` -- add `TlsConfig` struct, `tls: Option<TlsConfig>` field on `Config`,
  extend `Config::from_env()` with TLS env-var parsing and validation, add unit tests

**Acceptance Criteria:**
- [ ] `TlsConfig` is a private struct with fields `cert_path: PathBuf`, `key_path: PathBuf`, and
  `ca_path: Option<PathBuf>`. It derives `Debug`, `Clone`, `PartialEq`.
- [ ] `Config` gains a `tls: Option<TlsConfig>` field.
- [ ] `Config::from_env()` returns `Ok` with `tls: None` when neither `EVENTFOLD_TLS_CERT` nor
  `EVENTFOLD_TLS_KEY` nor `EVENTFOLD_TLS_CA` is set. Existing tests continue to pass.
- [ ] Test: set `EVENTFOLD_DATA`, `EVENTFOLD_TLS_CERT=/tmp/c.crt`, `EVENTFOLD_TLS_KEY=/tmp/k.key`,
  unset `EVENTFOLD_TLS_CA` -> `Config::from_env()` returns `Ok` with `config.tls == Some(TlsConfig
  { cert_path: "/tmp/c.crt".into(), key_path: "/tmp/k.key".into(), ca_path: None })`.
- [ ] Test: set `EVENTFOLD_DATA`, `EVENTFOLD_TLS_CERT=/tmp/c.crt`, `EVENTFOLD_TLS_KEY=/tmp/k.key`,
  `EVENTFOLD_TLS_CA=/tmp/ca.crt` -> `Config::from_env()` returns `Ok` with `config.tls ==
  Some(TlsConfig { ..., ca_path: Some("/tmp/ca.crt".into()) })`.
- [ ] Test: set `EVENTFOLD_DATA` and `EVENTFOLD_TLS_CERT=/tmp/c.crt`, leave `EVENTFOLD_TLS_KEY`
  unset -> `Config::from_env()` returns `Err` whose message contains `"EVENTFOLD_TLS_KEY"`.
- [ ] Test: set `EVENTFOLD_DATA` and `EVENTFOLD_TLS_KEY=/tmp/k.key`, leave `EVENTFOLD_TLS_CERT`
  unset -> `Config::from_env()` returns `Err` whose message contains `"EVENTFOLD_TLS_CERT"`.
- [ ] Test: set `EVENTFOLD_DATA` and `EVENTFOLD_TLS_CA=/tmp/ca.crt`, leave both `CERT` and `KEY`
  unset -> `Config::from_env()` returns `Err` whose message contains both `"EVENTFOLD_TLS_CERT"`
  and `"EVENTFOLD_TLS_KEY"` (or equivalent phrasing indicating CA-without-cert+key is invalid).
- [ ] `cargo build` at workspace root produces zero warnings after this change.
- [ ] Quality gates pass (build, lint, fmt, tests).

**Dependencies:** None
**Complexity:** M
**Maps to PRD AC:** AC 1, AC 2, AC 3, AC 4

---

### Ticket 2: Wire ServerTlsConfig into server startup

**Description:**
Extend `main()` in `src/main.rs` to read the cert, key, and optionally the CA cert files from disk
(using `tokio::fs::read`) when `config.tls` is `Some`, build `tonic::transport::ServerTlsConfig`
with `Identity` (and optionally `client_ca_root` for mTLS), and attach it to the
`tonic::transport::Server::builder()` before binding. The plaintext path (when `config.tls` is
`None`) is unchanged.

**Scope:**
- Modify: `src/main.rs` -- update `main()` to conditionally build and attach `ServerTlsConfig`;
  add startup log lines for TLS mode and mTLS mode

**Acceptance Criteria:**
- [ ] When `config.tls` is `None`, `main()` constructs the server builder identically to the
  current code -- no regression.
- [ ] When `config.tls` is `Some` with no `ca_path`, `main()` calls
  `server_builder.tls_config(ServerTlsConfig::new().identity(Identity::from_pem(cert, key)))`
  before `add_service`.
- [ ] When `config.tls` is `Some` with a `ca_path`, `main()` additionally calls
  `.client_ca_root(Certificate::from_pem(ca))` on the `ServerTlsConfig` before attaching.
- [ ] If `tokio::fs::read` fails for any cert/key file, `tracing::error!` is called with the path
  and error, and the process exits non-zero (matches the existing startup error pattern).
- [ ] A `tracing::info!` line is emitted on startup indicating TLS mode (e.g.
  `"TLS enabled"`) or mTLS mode (e.g. `"mTLS enabled"`).
- [ ] Test: `start_test_server` helper in `tests/grpc_service.rs` continues to work without
  modification -- `cargo test --test grpc_service` passes (plaintext path unchanged).
- [ ] Note: full TLS round-trip verification comes in Ticket 5 (integration tests). This ticket
  only wires the startup path and verifies the plaintext regression.
- [ ] Quality gates pass (build, lint, fmt, tests).

**Dependencies:** Ticket 1
**Complexity:** M
**Maps to PRD AC:** AC 1, AC 2, AC 5, AC 6, AC 7, AC 8

---

### Ticket 3: Add TlsOptions struct and update Client::connect in eventfold-console

**Description:**
Add a `TlsOptions` struct to `eventfold-console/src/client.rs` and update `Client::connect` to
accept `TlsOptions` as a second parameter. When `tls.enabled` is true, build a
`tonic::transport::ClientTlsConfig`, optionally attach the custom CA cert and/or client identity,
and connect via `Channel::from_shared(...)?. tls_config(...)?.connect().await`. When
`tls.enabled` is false, use the existing `EventStoreClient::connect` path. Update all call
sites of `Client::connect` in `src/main.rs` of the console to pass `TlsOptions::default()`.

**Scope:**
- Modify: `eventfold-console/src/client.rs` -- add `TlsOptions` struct (public, derives
  `Debug`, `Clone`, `Default`), update `Client::connect` signature to
  `connect(addr: &str, tls: TlsOptions)`, implement TLS channel construction branch, update
  existing unit tests that reference `Client`
- Modify: `eventfold-console/src/main.rs` -- update `Client::connect(&cli.addr)` call to
  `Client::connect(&cli.addr, TlsOptions::default())`

**Acceptance Criteria:**
- [ ] `TlsOptions` has fields `enabled: bool`, `ca_pem: Option<Vec<u8>>`,
  `identity: Option<(Vec<u8>, Vec<u8>)>`. It derives `Debug`, `Clone`, `Default`.
- [ ] `Client::connect` signature is `pub async fn connect(addr: &str, tls: TlsOptions) ->
  Result<Self, ConsoleError>`.
- [ ] Test: `TlsOptions::default()` has `enabled = false`, `ca_pem = None`, `identity = None`.
- [ ] Test: calling `Client::connect("http://[::1]:0", TlsOptions::default()).await` returns
  `Err(ConsoleError::ConnectionFailed(_))` (no server listening) -- verifies the plaintext path
  still hits `ConnectionFailed` on failure, not a panic.
- [ ] Test: calling `Client::connect("http://[::1]:0", TlsOptions { enabled: true, ..Default::
  default() }).await` returns `Err(ConsoleError::ConnectionFailed(_))` -- TLS path also
  surfaces connection errors correctly.
- [ ] Existing `proto_to_event_record`, `client_is_debug`, and `client_is_clone` unit tests still
  pass without modification.
- [ ] `eventfold-console/src/main.rs` passes `TlsOptions::default()` so the console compiles and
  the default (no-TLS) behavior is unchanged.
- [ ] Quality gates pass (build, lint, fmt, tests).

**Dependencies:** Ticket 1
**Complexity:** M
**Maps to PRD AC:** AC 1, AC 9

---

### Ticket 4: Add TLS CLI flags to eventfold-console and validate before connect

**Description:**
Add four new optional CLI flags to the `Cli` struct in `eventfold-console/src/main.rs`:
`--tls` (bool flag), `--tls-ca` (`Option<PathBuf>`), `--tls-client-cert` (`Option<PathBuf>`),
and `--tls-client-key` (`Option<PathBuf>`). Before connecting, validate that
`--tls-client-cert` and `--tls-client-key` are either both supplied or both absent; if only one
is given, print an error to stderr naming both flags and exit non-zero. Read the cert/key/CA
files from disk using `tokio::fs::read`, construct `TlsOptions`, and pass it to
`Client::connect`.

**Scope:**
- Modify: `eventfold-console/src/main.rs` -- add 4 new fields to `Cli`, add pre-connect
  validation logic, read PEM files, build `TlsOptions`, pass to `Client::connect`

**Acceptance Criteria:**
- [ ] `Cli` has fields `tls: bool`, `tls_ca: Option<PathBuf>`, `tls_client_cert: Option<PathBuf>`,
  `tls_client_key: Option<PathBuf>`, each with appropriate `#[arg(...)]` attributes.
- [ ] When `--tls-client-cert` is provided without `--tls-client-key` (or vice versa), the process
  prints a message to stderr containing both `"--tls-client-cert"` and `"--tls-client-key"` and
  exits with a non-zero status -- no connection attempt is made.
- [ ] When no TLS flags are set, `TlsOptions::default()` is passed to `Client::connect` (existing
  behavior preserved).
- [ ] When `--tls` is set with a valid `--tls-ca /path`, the CA PEM bytes are read and placed in
  `TlsOptions::ca_pem`.
- [ ] When `--tls`, `--tls-client-cert /c.crt`, and `--tls-client-key /c.key` are all set, both
  PEM files are read and placed in `TlsOptions::identity`.
- [ ] Test: invoke the binary via `std::process::Command` with `--tls-client-cert /dev/null`
  (without `--tls-client-key`) -- assert exit status is non-zero and stderr contains
  `"--tls-client-key"`.
- [ ] Test: invoke the binary via `std::process::Command` with `--tls-client-key /dev/null`
  (without `--tls-client-cert`) -- assert exit status is non-zero and stderr contains
  `"--tls-client-cert"`.
- [ ] Quality gates pass (build, lint, fmt, tests).

**Dependencies:** Ticket 3
**Complexity:** M
**Maps to PRD AC:** AC 1, AC 9, AC 10

---

### Ticket 5: TLS and mTLS integration tests using rcgen

**Description:**
Add `tests/tls_integration.rs` with six test cases that exercise the full TLS and mTLS server
paths. Use `rcgen` to generate self-signed CA, server cert/key, and client cert/key in-process
(no static fixture files). Add helper functions `start_tls_test_server` and
`start_mtls_test_server` analogous to `start_test_server` in `tests/grpc_service.rs`. All tests
use ephemeral ports via `TcpListener::bind("[::1]:0")`.

> Implementer note: Verify `rcgen` 0.13's API before starting -- specifically whether the
> certificate-generation entry point is `CertificateParams::new(...).self_signed(...)` or
> `Certificate::generate_self_signed(...)`. Also confirm the exact tonic TLS feature flag name
> (it may be `tls` or `tls-roots`; check `tonic` 0.13's `Cargo.toml` before writing imports).

**Scope:**
- Create: `tests/tls_integration.rs` -- full test file with 6 test cases and two helper
  functions (`start_tls_test_server`, `start_mtls_test_server`)

**Acceptance Criteria:**
- [ ] `start_tls_test_server` generates a self-signed CA + server cert/key via `rcgen`, starts
  an in-process tonic server with `ServerTlsConfig`, and returns the server address + CA PEM
  bytes (for client configuration) + `TempDir`.
- [ ] `start_mtls_test_server` does the same but additionally generates a client cert/key signed
  by the same CA and calls `.client_ca_root(...)` on the `ServerTlsConfig`; returns the CA PEM,
  client cert PEM, and client key PEM alongside the address.
- [ ] Test `tls_client_append_and_read_all`: start a TLS server, connect with
  `ClientTlsConfig` + the server's CA PEM, call `Append` with one event, then `ReadAll` from
  position 0 -- assert the response contains the appended event (verifies AC 5).
- [ ] Test `tls_plaintext_client_rejected`: start a TLS server, attempt to connect with a
  plaintext `EventStoreClient::connect("http://...")` and call any RPC -- assert the result is
  `Err(_)` with a transport-level error (not `Status::ok`; verifies AC 6).
- [ ] Test `mtls_client_no_cert_rejected`: start an mTLS server, connect with
  `ClientTlsConfig` (TLS enabled, correct CA, but no client identity), call any RPC -- assert
  the result is `Err(_)` (transport rejection before RPC dispatch; verifies AC 7).
- [ ] Test `mtls_client_valid_cert_accepted`: start an mTLS server, connect with
  `ClientTlsConfig` + the CA PEM + a valid client `Identity`, call `Append` then `ReadAll` --
  assert the appended event is returned (verifies AC 8).
- [ ] Test `plaintext_server_plaintext_client_still_works`: start a plaintext server using the
  existing `start_test_server` helper, connect with a plaintext client, append and read -- assert
  success (regression guard for AC 2).
- [ ] Test `tls_wrong_ca_rejected`: start a TLS server, connect with a `ClientTlsConfig` that
  uses a *different* self-signed CA (not the server's CA) -- assert the result is `Err(_)`
  (certificate verification failure).
- [ ] Quality gates pass (build, lint, fmt, tests).

**Dependencies:** Ticket 1, Ticket 2
**Complexity:** L
**Maps to PRD AC:** AC 2, AC 5, AC 6, AC 7, AC 8

---

### Ticket 6: Verification and integration check

**Description:**
Run the full PRD 011 acceptance criteria checklist end-to-end. Verify that all tickets integrate
correctly as a cohesive feature: server starts in plaintext and TLS modes, mTLS rejects and
accepts clients correctly, the console TLS CLI flags are wired, and all existing tests continue
to pass without modification.

**Scope:**
- No new files. This is a verification-only ticket.

**Acceptance Criteria:**
- [ ] `cargo build` at workspace root produces zero warnings.
- [ ] `cargo clippy --all-targets --all-features --locked -- -D warnings` exits clean.
- [ ] `cargo fmt --check` exits clean.
- [ ] `cargo test` runs all tests green including `tests/tls_integration.rs`,
  `tests/grpc_service.rs`, `tests/writer_integration.rs`, `tests/broker_integration.rs`,
  `tests/server_binary.rs`, and `tests/idempotent_appends.rs`.
- [ ] All PRD 011 acceptance criteria pass (checklist):
  - [ ] AC 1: zero build warnings, clippy, fmt clean.
  - [ ] AC 2: no TLS env vars set -> `config.tls` is `None`; all existing tests pass.
  - [ ] AC 3: only one of `CERT`/`KEY` set -> `Config::from_env()` returns `Err` naming the
    missing variable.
  - [ ] AC 4: only `CA` set (no `CERT`/`KEY`) -> `Config::from_env()` returns `Err`.
  - [ ] AC 5: TLS client connects to TLS server, `Append` + `ReadAll` return `Ok`.
  - [ ] AC 6: plaintext client to TLS server returns a transport-level `Err`.
  - [ ] AC 7: mTLS server rejects TLS client with no client cert.
  - [ ] AC 8: mTLS server accepts TLS client with valid client cert, `Append` + `ReadAll` return `Ok`.
  - [ ] AC 9: `eventfold-console --tls --addr https://host:2113` builds; `--tls-ca` controls
    CA verification; omitting `--tls-ca` for a custom-CA server causes `ConnectionFailed`.
  - [ ] AC 10: `--tls-client-cert` without `--tls-client-key` exits non-zero and names both flags.
- [ ] No regressions in any pre-existing tests.

**Dependencies:** All previous tickets
**Complexity:** M
**Maps to PRD AC:** AC 1-10

---

## AC Coverage Matrix

| PRD AC # | Description                                                                                 | Covered By Ticket(s)         | Status  |
|----------|---------------------------------------------------------------------------------------------|------------------------------|---------|
| 1        | Zero build warnings; clippy and fmt pass                                                    | Ticket 1, 2, 3, 4, 5, 6     | Covered |
| 2        | No TLS env vars -> `config.tls` is `None`; server starts plaintext; all existing tests pass | Ticket 1, 2, 5, 6            | Covered |
| 3        | Only one of `CERT`/`KEY` set -> `Err` naming the missing variable                          | Ticket 1, 6                  | Covered |
| 4        | Only `CA` set without `CERT`+`KEY` -> `Err`                                                | Ticket 1, 6                  | Covered |
| 5        | TLS client to TLS server: `Append` + `ReadAll` succeed                                     | Ticket 2, 5, 6               | Covered |
| 6        | Plaintext client to TLS server -> transport-level error                                     | Ticket 2, 5, 6               | Covered |
| 7        | mTLS server rejects TLS client with no client cert                                          | Ticket 2, 5, 6               | Covered |
| 8        | mTLS server accepts TLS client with valid client cert; RPCs succeed                         | Ticket 2, 5, 6               | Covered |
| 9        | `eventfold-console --tls --addr https://...` builds; `--tls-ca` controls CA verification    | Ticket 3, 4, 6               | Covered |
| 10       | `--tls-client-cert` without `--tls-client-key` -> non-zero exit naming both flags           | Ticket 4, 6                  | Covered |
