# PRD 011: TLS and mTLS Support

**Status:** TICKETS READY
**Created:** 2026-02-26
**Author:** PRD Writer Agent

---

## Problem Statement

EventfoldDB is deployed on a remote Oracle Cloud ARM instance where projection services and command APIs connect over a network. All gRPC traffic is currently plaintext, exposing event data and connection metadata to any observer on the path. Adding TLS encrypts the transport and, with mutual TLS, lets the server reject connections from unauthorized clients without requiring a separate authentication layer.

## Goals

- Encrypt gRPC traffic between the server and all clients when cert/key env vars are provided.
- Support optional mutual TLS (mTLS): when a CA cert is also provided, the server requires and verifies client certificates.
- Keep plaintext mode fully intact when no TLS env vars are set (zero breaking change for existing deployments).
- Enable `eventfold-console` to connect to TLS-enabled servers via CLI flags.
- Cover both modes (plaintext and TLS) in integration tests.

## Non-Goals

- Certificate issuance, rotation, or an internal PKI. Operators supply certificates from any source (Let's Encrypt, step-ca, OpenSSL self-signed, etc.).
- ALPN negotiation beyond what tonic's `ServerTlsConfig` / `ClientTlsConfig` provide by default.
- HTTP/1.1 plaintext-and-TLS on the same port (SNI-based switching). TLS and plaintext are mutually exclusive per port.
- Token-based or password-based authentication. mTLS is the only auth mechanism in scope.
- TLS for the `eventfold-console` live-tail reconnect loop. If the subscription drops, the existing reconnect behavior is unchanged; TLS applies only to the initial channel setup.
- Any changes to the on-disk format, event schema, or gRPC proto definitions.
- Hot certificate reload. A server restart is required to pick up new certificates.

## User Stories

- As an operator, I want the EventfoldDB server to serve TLS when I set `EVENTFOLD_TLS_CERT` and `EVENTFOLD_TLS_KEY`, so that all gRPC traffic is encrypted in transit.
- As an operator, I want to additionally set `EVENTFOLD_TLS_CA` to require client certificates, so that only services I have issued certs to can connect.
- As an operator, I want the server to start in plaintext mode when I set none of the TLS env vars, so that my existing deployment is unaffected.
- As a developer, I want to run `eventfold-console --tls --addr https://host:2113` to browse a TLS-protected server, so that I can inspect production or staging data.
- As a developer, I want to pass `--tls-ca /path/to/ca.crt` to `eventfold-console` to verify the server certificate against a custom CA, so that I can connect to servers using self-signed or private PKI certificates.
- As a developer, I want `eventfold-console` to accept `--tls-client-cert` and `--tls-client-key` flags so that I can authenticate against a server running in mTLS mode.

## Technical Approach

### Dependency Changes

Enable tonic's TLS feature in both crates. Tonic bundles `rustls` via the `tls` feature flag; no additional TLS crate is needed.

**Root `Cargo.toml`** — change:
```toml
tonic = "0.13"
```
to:
```toml
tonic = { version = "0.13", features = ["tls"] }
```

**`eventfold-console/Cargo.toml`** — same change:
```toml
tonic = { version = "0.13", features = ["tls"] }
```

### Server: Config Changes (`src/main.rs`)

Add three new optional fields to `Config` and parse them in `Config::from_env()`:

```rust
/// Optional TLS configuration parsed from environment variables.
///
/// Present only when `EVENTFOLD_TLS_CERT` and `EVENTFOLD_TLS_KEY` are both set.
#[derive(Debug, Clone, PartialEq)]
struct TlsConfig {
    /// Path to the PEM-encoded server certificate file.
    cert_path: PathBuf,
    /// Path to the PEM-encoded server private key file.
    key_path: PathBuf,
    /// Path to the PEM-encoded CA certificate for client verification (mTLS).
    /// When `Some`, the server requires and verifies client certificates.
    ca_path: Option<PathBuf>,
}
```

`Config` gains a field `tls: Option<TlsConfig>`. Parsing rules:

| Env var              | Required | Behavior when absent                                      |
|----------------------|----------|-----------------------------------------------------------|
| `EVENTFOLD_TLS_CERT` | No       | TLS disabled; `tls` is `None`                             |
| `EVENTFOLD_TLS_KEY`  | No       | TLS disabled; `tls` is `None`                             |
| `EVENTFOLD_TLS_CA`   | No       | mTLS disabled; `ca_path` is `None` within `TlsConfig`    |

Validation rule: if exactly one of `EVENTFOLD_TLS_CERT` / `EVENTFOLD_TLS_KEY` is set (but not both), `Config::from_env()` returns `Err` with a message identifying the missing variable. Setting only `EVENTFOLD_TLS_CA` without cert+key also returns `Err`.

### Server: Startup Changes (`src/main.rs`)

In `main()`, after building the `tonic::transport::Server`, conditionally attach `ServerTlsConfig` before binding. Reads certificate and key files using `tokio::fs::read` (async) during startup. On any IO error, logs with `tracing::error!` and exits non-zero.

```rust
// Pseudocode for the TLS branch in main():
let server_builder = tonic::transport::Server::builder();
let server_builder = if let Some(ref tls) = config.tls {
    let cert = tokio::fs::read(&tls.cert_path).await?;
    let key  = tokio::fs::read(&tls.key_path).await?;
    let identity = tonic::transport::Identity::from_pem(cert, key);
    let mut tls_config = tonic::transport::ServerTlsConfig::new().identity(identity);
    if let Some(ref ca_path) = tls.ca_path {
        let ca = tokio::fs::read(ca_path).await?;
        let ca_cert = tonic::transport::Certificate::from_pem(ca);
        tls_config = tls_config.client_ca_root(ca_cert);
    }
    server_builder.tls_config(tls_config)?
} else {
    server_builder
};
```

The rest of startup (bind, serve_with_incoming_shutdown) is unchanged.

### Console: CLI Changes (`eventfold-console/src/main.rs`)

Add three new optional flags to the `Cli` struct:

| Flag              | Type                | Description                                              |
|-------------------|---------------------|----------------------------------------------------------|
| `--tls`           | `bool` (flag)       | Enable TLS on the client channel                         |
| `--tls-ca`        | `Option<PathBuf>`   | Path to CA cert PEM (for self-signed / private PKI)      |
| `--tls-client-cert` | `Option<PathBuf>` | Path to client cert PEM (for mTLS)                       |
| `--tls-client-key`  | `Option<PathBuf>` | Path to client key PEM (for mTLS)                        |

`--tls-client-cert` and `--tls-client-key` must both be provided together; if only one is supplied, the console prints an error and exits non-zero before connecting.

Update the `--addr` default to `http://[::1]:2113`. When `--tls` is set, the address scheme must be `https://`; if the user provides an `http://` address with `--tls`, emit a warning but proceed (tonic ignores the scheme and uses the TLS config).

### Console: Client Changes (`eventfold-console/src/client.rs`)

Replace the current `Client::connect(addr: &str)` signature with:

```rust
/// TLS options for connecting to EventfoldDB.
#[derive(Debug, Clone, Default)]
pub struct TlsOptions {
    /// Enable TLS. When false, all other fields are ignored.
    pub enabled: bool,
    /// PEM-encoded CA certificate for server verification.
    pub ca_pem: Option<Vec<u8>>,
    /// PEM-encoded client certificate + key for mTLS.
    pub identity: Option<(Vec<u8>, Vec<u8>)>,
}

impl Client {
    pub async fn connect(addr: &str, tls: TlsOptions) -> Result<Self, ConsoleError> { ... }
}
```

Inside `connect`, when `tls.enabled` is true, build a `tonic::transport::ClientTlsConfig`, optionally add the CA cert and client identity, then call `Channel::from_shared(addr)?.tls_config(tls_config)?.connect().await`. When `tls.enabled` is false, use the existing `EventStoreClient::connect(addr)` path.

`spawn_subscription` and all call sites of `Client::connect` in `main.rs` are updated to pass `TlsOptions`.

### Integration Tests (`tests/`)

Add a new test file `tests/tls_integration.rs` that:

1. Generates a self-signed CA, server cert/key, and client cert/key using the `rcgen` crate (added as a dev-dependency).
2. Starts a TLS-only test server helper (`start_tls_test_server`) analogous to `start_test_server` in `tests/grpc_service.rs`, using `ServerTlsConfig` with the generated cert/key.
3. Starts an mTLS test server helper (`start_mtls_test_server`) that additionally sets `client_ca_root`.
4. Verifies that a TLS client can append and read events through the TLS server.
5. Verifies that an mTLS client (with a valid client cert) can connect to the mTLS server.
6. Verifies that a plaintext client connection to the TLS server fails (tonic returns a transport error, not a gRPC status).
7. Verifies that an mTLS client with no client cert is rejected by the mTLS server.

### File Change Table

| File                                   | Change                                                              |
|----------------------------------------|---------------------------------------------------------------------|
| `Cargo.toml`                           | Add `features = ["tls"]` to `tonic` dependency                     |
| `src/main.rs`                          | Add `TlsConfig` struct, parse 3 new env vars, conditional TLS setup |
| `eventfold-console/Cargo.toml`         | Add `features = ["tls"]` to `tonic` dependency; add `rcgen` to dev-deps is not needed here (rcgen is only in root dev-deps) |
| `eventfold-console/src/main.rs`        | Add 4 new CLI flags, read cert/key files, pass `TlsOptions`        |
| `eventfold-console/src/client.rs`      | New `TlsOptions` struct, update `Client::connect` signature        |
| `tests/tls_integration.rs`             | New integration test file (6 test cases)                            |
| `Cargo.toml` (dev-dependencies)        | Add `rcgen = "0.13"` for in-test certificate generation            |

## Acceptance Criteria

1. `cargo build` at the workspace root compiles with zero warnings. `cargo clippy --all-targets --all-features --locked -- -D warnings` passes. `cargo fmt --check` passes.

2. When `EVENTFOLD_TLS_CERT` and `EVENTFOLD_TLS_KEY` are both unset, `Config::from_env()` succeeds and `config.tls` is `None`. The server starts and accepts plaintext gRPC connections on port 2113. All existing integration tests in `tests/grpc_service.rs`, `tests/writer_integration.rs`, `tests/broker_integration.rs`, and `tests/server_binary.rs` pass without modification.

3. When `EVENTFOLD_TLS_CERT` is set but `EVENTFOLD_TLS_KEY` is absent (or vice versa), `Config::from_env()` returns `Err` with an error message that names the missing variable. The server exits non-zero.

4. When `EVENTFOLD_TLS_CERT`, `EVENTFOLD_TLS_KEY`, and `EVENTFOLD_TLS_CA` are all absent and only `EVENTFOLD_TLS_CA` is set, `Config::from_env()` returns `Err` with an error message stating that CA without cert+key is invalid.

5. A tonic client configured with `ClientTlsConfig` pointing to the correct CA cert can connect to a server started with `ServerTlsConfig` and successfully execute `Append` and `ReadAll` RPCs, receiving `Ok` responses.

6. A plaintext tonic client (`EventStoreClient::connect("http://...")`) that attempts to connect to a server running in TLS mode receives a transport-level connection error (not a gRPC `Status::ok`).

7. A server started with `EVENTFOLD_TLS_CA` set (mTLS mode) rejects a TLS client that presents no client certificate with a transport-level connection error before any RPC is dispatched.

8. A server started with `EVENTFOLD_TLS_CA` set (mTLS mode) accepts a TLS client that presents a valid client certificate signed by the configured CA and successfully serves `Append` and `ReadAll` RPCs.

9. `eventfold-console --tls --addr https://host:2113` builds and launches without panicking. When connecting to a server using a CA not trusted by the system root store, `--tls-ca /path/to/ca.crt` causes the connection to succeed; omitting `--tls-ca` for such a server causes `ConsoleError::ConnectionFailed`.

10. `eventfold-console --tls-client-cert /path/to/client.crt` (without `--tls-client-key`) prints an error message to stderr containing both flag names and exits with a non-zero status code before attempting any connection.

## Open Questions

- **rcgen API surface**: `rcgen` 0.13 is the latest stable release as of the knowledge cutoff. Confirm the crate's certificate-generation API (specifically `Certificate::generate_self_signed` or equivalent) is available at that version before implementation begins.
- **tonic TLS feature flag name**: Tonic 0.13 may expose TLS via `features = ["tls"]` (rustls) or separate `tls-roots` / `tls-native-roots` features. Verify the exact feature name in `tonic` 0.13's `Cargo.toml` before adding the dependency.
- **System root store vs. custom CA**: When `--tls-ca` is omitted and `--tls` is set, should `eventfold-console` fall back to the system root store? The PRD assumes yes (standard tonic/rustls behavior). If the server uses a self-signed cert without `--tls-ca`, the connection will fail with a certificate verification error, which is the correct and expected behavior.

## Dependencies

- **Depends on**: PRDs 001-009 (complete EventfoldDB with workspace, gRPC server, and console TUI).
- **External crates added**: `tonic` `tls` feature (both crates); `rcgen = "0.13"` as a root dev-dependency for integration tests.
- **No proto changes**: the `.proto` file and generated code are unmodified. TLS is purely a transport-layer concern.
