//! EventfoldDB interactive TUI console.
//!
//! Connects to a running EventfoldDB server over gRPC and provides an
//! interactive terminal interface for browsing streams, events, and
//! watching live updates.
//!
//! # Usage
//!
//! ```text
//! eventfold-console [--addr <ADDRESS>]
//! ```
//!
//! The `--addr` flag defaults to `http://[::1]:2113`.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{self, Event, poll};
use tokio::sync::mpsc;

use eventfold_console::app::{self, AppState};
use eventfold_console::client::{self, Client, SubscriptionMsg, TlsOptions};
use eventfold_console::tui;

/// Interactive TUI console for browsing an EventfoldDB server.
#[derive(Parser, Debug)]
#[command(name = "eventfold-console", version, about)]
struct Cli {
    /// Server address to connect to.
    #[arg(long, default_value = "http://[::1]:2113")]
    addr: String,

    /// Enable TLS for the connection.
    #[arg(long, default_value_t = false)]
    tls: bool,

    /// Path to a PEM-encoded CA certificate for server verification.
    #[arg(long)]
    tls_ca: Option<PathBuf>,

    /// Path to a PEM-encoded client certificate for mTLS.
    #[arg(long)]
    tls_client_cert: Option<PathBuf>,

    /// Path to a PEM-encoded client private key for mTLS.
    #[arg(long)]
    tls_client_key: Option<PathBuf>,
}

/// Tick interval for the event loop (approximately 30 fps).
const TICK_INTERVAL: Duration = Duration::from_millis(33);

/// Page size for reading events in the detail and global log views.
const READ_PAGE_SIZE: u64 = 1000;

/// Validate that `--tls-client-cert` and `--tls-client-key` are either both
/// supplied or both absent. If only one is given, prints an error to stderr
/// and exits with a non-zero status code.
fn validate_tls_flags(cli: &Cli) {
    let has_cert = cli.tls_client_cert.is_some();
    let has_key = cli.tls_client_key.is_some();
    if has_cert != has_key {
        let missing = if has_cert {
            "--tls-client-key"
        } else {
            "--tls-client-cert"
        };
        eprintln!(
            "error: --tls-client-cert and --tls-client-key must be provided \
             together; missing {missing}"
        );
        std::process::exit(1);
    }
}

/// Build [`TlsOptions`] from the parsed CLI flags by reading PEM files from
/// disk.
///
/// When no TLS flags are set, returns `TlsOptions::default()` (plaintext).
///
/// # Errors
///
/// Returns an error if any specified PEM file cannot be read from disk.
async fn build_tls_options(cli: &Cli) -> Result<TlsOptions> {
    // No TLS flags set -- plaintext mode.
    if !cli.tls && cli.tls_ca.is_none() && cli.tls_client_cert.is_none() {
        return Ok(TlsOptions::default());
    }

    let mut opts = TlsOptions {
        enabled: true,
        ..Default::default()
    };

    // Read CA certificate if provided.
    if let Some(ref ca_path) = cli.tls_ca {
        let ca_pem = tokio::fs::read(ca_path)
            .await
            .with_context(|| format!("failed to read CA certificate: {}", ca_path.display()))?;
        opts.ca_pem = Some(ca_pem);
    }

    // Read client identity if provided (already validated as a pair).
    if let Some(ref cert_path) = cli.tls_client_cert {
        let cert_pem = tokio::fs::read(cert_path).await.with_context(|| {
            format!("failed to read client certificate: {}", cert_path.display())
        })?;
        // Safe to unwrap: validate_tls_flags ensures key is present when cert is.
        let key_path = cli
            .tls_client_key
            .as_ref()
            .expect("invariant: tls_client_key must be Some when tls_client_cert is Some");
        let key_pem = tokio::fs::read(key_path)
            .await
            .with_context(|| format!("failed to read client key: {}", key_path.display()))?;
        opts.identity = Some((cert_pem, key_pem));
    }

    Ok(opts)
}

#[tokio::main]
async fn main() -> Result<()> {
    // Parse CLI arguments.
    let cli = Cli::parse();

    // Validate that --tls-client-cert and --tls-client-key are paired.
    validate_tls_flags(&cli);

    // Initialize tracing (respects RUST_LOG env var).
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();

    // Build TLS options from CLI flags.
    let tls_options = build_tls_options(&cli)
        .await
        .context("Failed to build TLS options")?;

    // Connect to the EventfoldDB server.
    let client = Client::connect(&cli.addr, tls_options)
        .await
        .context("Failed to connect to EventfoldDB server")?;

    // Initialize the terminal.
    let mut terminal = tui::init_terminal().context("Failed to initialize terminal")?;

    // Set up a panic hook that restores the terminal before printing the panic.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = tui::restore_terminal();
        original_hook(info);
    }));

    // Initialize application state.
    let mut state = AppState::new(cli.addr.clone());

    // Set up the subscription channel for live tail.
    let (sub_tx, mut sub_rx) = mpsc::channel::<SubscriptionMsg>(1024);

    // Spawn the live tail subscription task.
    let sub_client = client.clone();
    tokio::spawn(async move {
        client::spawn_subscription(sub_client, 0, sub_tx).await;
    });

    // Clone the client for async data fetching.
    let mut fetch_client = client;

    // Trigger initial streams load.
    state.streams_loading = true;

    // Main event loop.
    let result = run_event_loop(&mut terminal, &mut state, &mut fetch_client, &mut sub_rx).await;

    // Restore the terminal regardless of success or failure.
    tui::restore_terminal().context("Failed to restore terminal")?;

    result
}

/// Main event loop: polls for key events, drains async channels, updates
/// state, and renders.
///
/// Runs until `state.should_quit` is set to `true`.
async fn run_event_loop(
    terminal: &mut tui::Term,
    state: &mut AppState,
    client: &mut Client,
    sub_rx: &mut mpsc::Receiver<SubscriptionMsg>,
) -> Result<()> {
    loop {
        // 1. Render the current state.
        tui::render(terminal, state)?;

        // 2. Handle any pending data fetches.
        handle_data_fetches(state, client).await?;

        // 3. Drain subscription messages (non-blocking).
        while let Ok(msg) = sub_rx.try_recv() {
            match msg {
                SubscriptionMsg::Event(event) => state.push_live_event(event),
                SubscriptionMsg::CaughtUp => state.live_caught_up = true,
                SubscriptionMsg::Error(e) => {
                    tracing::error!(error = %e, "Subscription error");
                }
            }
        }

        // 4. Poll for key events (non-blocking with tick timeout).
        if poll(TICK_INTERVAL)?
            && let Event::Key(key) = event::read()?
            && let Some(action) = app::handle_key_event(key, state.active_tab)
        {
            state.apply_action(action);
        }

        // 5. Check if we should quit.
        if state.should_quit {
            return Ok(());
        }
    }
}

/// Handle pending data fetches based on loading flags in the state.
///
/// When a tab's `loading` flag is true, fetches data from the server and
/// updates the state, then clears the flag.
async fn handle_data_fetches(state: &mut AppState, client: &mut Client) -> Result<()> {
    if state.streams_loading {
        match client.list_streams().await {
            Ok(streams) => state.streams = streams,
            Err(e) => tracing::error!(error = %e, "Failed to list streams"),
        }
        state.streams_loading = false;
    }

    if state.detail_loading {
        if let Some(ref stream_id) = state.detail_stream_id {
            let sid = stream_id.clone();
            match client.read_stream(&sid, 0, READ_PAGE_SIZE).await {
                Ok(events) => state.detail_events = events,
                Err(e) => tracing::error!(error = %e, "Failed to read stream"),
            }
        }
        state.detail_loading = false;
    }

    if state.global_loading {
        match client.read_all(0, READ_PAGE_SIZE).await {
            Ok(events) => state.global_events = events,
            Err(e) => tracing::error!(error = %e, "Failed to read global log"),
        }
        state.global_loading = false;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    /// Find the eventfold-console binary path in target/debug.
    fn binary_path() -> std::path::PathBuf {
        // `cargo test` sets CARGO_BIN_EXE_eventfold-console` or we can use
        // `env!("CARGO_BIN_EXE_eventfold-console")` but that only works in
        // integration tests. For unit tests in main.rs, build path manually.
        let mut path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.pop(); // go from eventfold-console/ to workspace root
        path.push("target");
        path.push("debug");
        path.push("eventfold-console");
        path
    }

    #[test]
    fn cert_without_key_exits_nonzero_and_mentions_key_flag() {
        let output = Command::new(binary_path())
            .arg("--tls-client-cert")
            .arg("/dev/null")
            .output()
            .expect("failed to execute binary");

        assert!(
            !output.status.success(),
            "expected non-zero exit, got: {:?}",
            output.status
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("--tls-client-key"),
            "stderr should mention --tls-client-key, got: {stderr}"
        );
    }

    #[test]
    fn key_without_cert_exits_nonzero_and_mentions_cert_flag() {
        let output = Command::new(binary_path())
            .arg("--tls-client-key")
            .arg("/dev/null")
            .output()
            .expect("failed to execute binary");

        assert!(
            !output.status.success(),
            "expected non-zero exit, got: {:?}",
            output.status
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("--tls-client-cert"),
            "stderr should mention --tls-client-cert, got: {stderr}"
        );
    }
}
