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

use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{self, Event, poll};
use tokio::sync::mpsc;

use eventfold_console::app::{self, AppState};
use eventfold_console::client::{self, Client, SubscriptionMsg};
use eventfold_console::tui;

/// Interactive TUI console for browsing an EventfoldDB server.
#[derive(Parser, Debug)]
#[command(name = "eventfold-console", version, about)]
struct Cli {
    /// Server address to connect to.
    #[arg(long, default_value = "http://[::1]:2113")]
    addr: String,
}

/// Tick interval for the event loop (approximately 30 fps).
const TICK_INTERVAL: Duration = Duration::from_millis(33);

/// Page size for listing streams via ReadAll scan.
const LIST_STREAMS_PAGE_SIZE: u64 = 1000;

/// Page size for reading events in the detail and global log views.
const READ_PAGE_SIZE: u64 = 1000;

#[tokio::main]
async fn main() -> Result<()> {
    // Parse CLI arguments.
    let cli = Cli::parse();

    // Initialize tracing (respects RUST_LOG env var).
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();

    // Connect to the EventfoldDB server.
    let client = Client::connect(&cli.addr)
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
        if poll(TICK_INTERVAL)? {
            if let Event::Key(key) = event::read()? {
                if let Some(action) = app::handle_key_event(key, state.active_tab) {
                    state.apply_action(action);
                }
            }
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
        match client.list_streams(LIST_STREAMS_PAGE_SIZE).await {
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
