# PRD 009: Interactive Console TUI for EventfoldDB

**Status:** DRAFT
**Created:** 2026-02-27

---

## Problem Statement

EventfoldDB is running but there is no way to inspect what's in it. Developers need a tool to browse streams, read events, view the global log, and watch live updates during local development. This tool should connect to a running EventfoldDB instance over gRPC and provide an interactive terminal interface.

## Goals

- Provide a standalone binary (`eventfold-console`) that connects to an EventfoldDB server and lets users interactively browse all stored data.
- Convert the project to a Cargo workspace so the console is a separate crate that depends on the main crate's generated proto types.
- Support four core features: list streams, read stream events, browse the global log, and live tail (real-time subscription).

## Non-Goals

- Writing/appending events from the console. This is a read-only inspection tool.
- A web-based UI or REST API. Terminal only.
- Adding new RPCs to the EventfoldDB server (e.g., `ListStreams`). The console works with the existing 5 RPCs.
- Production monitoring, alerting, or metrics dashboards.

## User Stories

- As a developer, I want to see all streams in my local EventfoldDB instance so I can understand what data exists.
- As a developer, I want to read the events in a specific stream so I can debug my event-sourced application.
- As a developer, I want to browse the global event log in position order so I can see the full sequence of events across all streams.
- As a developer, I want to watch events arrive in real-time so I can verify my application is producing the correct events.

## Technical Approach

### Workspace Conversion

The project is currently a single crate. Add a `[workspace]` section to the existing root `Cargo.toml`:

```toml
[workspace]
members = [".", "eventfold-console"]
```

No files move. The existing crate stays at root as a member. `Cargo.lock` stays at root. The new crate lives at `eventfold-console/`.

### New Crate Structure

```
eventfold-console/
  Cargo.toml
  src/
    main.rs              -- clap arg parsing (--addr), tokio runtime, bootstrap
    error.rs             -- ConsoleError enum (anyhow at top level)
    client.rs            -- gRPC client wrapper: connect, read_all, read_stream,
                            subscribe_all, subscribe_stream, list_streams
    app.rs               -- AppState, Tab enum, input handling, action dispatch
    tui.rs               -- Terminal init/restore (crossterm), render + event loop
    views/
      mod.rs             -- Re-exports, format_bytes helper
      streams.rs         -- Streams list table
      stream_detail.rs   -- Events table + detail panel for selected stream
      global_log.rs      -- All events in global position order, paginated
      live_tail.rs       -- Auto-scrolling subscription view
```

### Dependencies

```toml
[dependencies]
eventfold-db = { path = ".." }
anyhow = "1"
clap = { version = "4", features = ["derive"] }
crossterm = "0.28"
ratatui = "0.29"
tokio = { version = "1", features = ["full"] }
tonic = "0.13"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
serde_json = "1"
unicode-width = "0.2"
```

No `build.rs` needed — proto types accessed via `eventfold_db::proto::*`.

### TUI Layout

```
+--[ Streams ]--[ Stream Events ]--[ Global Log ]--[ Live Tail ]--+
|                                                                   |
|                         Main content area                         |
|                         (table + detail panel)                    |
|                                                                   |
+-------------------------------------------------------------------+
| Status: Connected to 127.0.0.1:2113 | 5 streams | Tab/q/r/?     |
+-------------------------------------------------------------------+
```

Four tabs, navigable with `1`/`2`/`3`/`4` or `Tab`/`Shift-Tab`. `j`/`k` or `Up`/`Down` for scrolling. `Enter` to drill into a stream. `r` to refresh. `f` to toggle JSON formatting. `p` to pause live tail. `q` to quit.

### Feature: List Streams

No `ListStreams` RPC exists. Derive by paginated `ReadAll` scan (page size 1000), collecting unique `stream_id` entries with event counts and latest versions. Cache results, `r` to refresh. Show loading progress in status bar.

### Feature: Read Stream Events

Call `ReadStream` for the selected stream. Split view: top half is a scrollable event table (global position, stream version, event type, payload preview), bottom half is a detail panel showing the full event when selected. `format_bytes()` tries JSON pretty-print, falls back to UTF-8, falls back to hex dump.

### Feature: Global Log

Call `ReadAll` with paginated fetching (page size 500). Same split view as stream events but with stream_id column visible. Load next page on scroll-to-boundary.

### Feature: Live Tail

Spawn `SubscribeAll` on a tokio task. Events flow through `tokio::sync::mpsc` channel to the render loop (non-blocking `try_recv` on each tick). `VecDeque` buffer capped at 10,000 events (oldest evicted). Shows "Catching up..." until `CaughtUp` marker, then "Live". `p` pauses auto-scroll.

### Architecture

```
main() -> tokio runtime
  |
  +-- init_terminal(crossterm)
  +-- connect gRPC client
  +-- event loop (~30 FPS, 33ms tick):
  |     1. poll crossterm key events
  |     2. drain mpsc channel (async gRPC results)
  |     3. update AppState
  |     4. render(terminal, &app_state)
  +-- restore_terminal()
```

Async gRPC calls spawned as tokio tasks, results sent back via `mpsc`. The render loop never blocks on async operations.

## Acceptance Criteria

### AC-1: Workspace builds

`cargo build` at the workspace root compiles both `eventfold-db` and `eventfold-console` with zero warnings. All existing 210 tests pass. `cargo clippy --all-targets --all-features --locked -- -D warnings` passes.

### AC-2: Console connects to server

`eventfold-console --addr 127.0.0.1:2113` connects to a running EventfoldDB server. Connection failure shows a clear error message. Default address is `http://127.0.0.1:2113`.

### AC-3: TUI launches and tabs work

The console shows 4 tabs (Streams, Stream Events, Global Log, Live Tail). Tab switching via `1`/`2`/`3`/`4` and `Tab`/`Shift-Tab` works. `q` quits cleanly and restores the terminal. The status bar shows the connected address.

### AC-4: List Streams

The Streams tab shows a table of all streams with columns: Stream ID, Event Count, Latest Version. Data is loaded via paginated `ReadAll` scan. A loading indicator is shown during the scan. `r` refreshes. An empty store shows "No streams found".

### AC-5: Read Stream Events

Selecting a stream on the Streams tab and pressing `Enter` navigates to the Stream Events tab showing that stream's events. The table shows: Global Position, Stream Version, Event Type, Payload Preview. Selecting an event shows its full detail (event_id, payload, metadata) in a detail panel below. `f` toggles JSON formatting of payload/metadata.

### AC-6: Global Log

The Global Log tab shows all events in global position order with columns: Global Position, Stream ID, Stream Version, Event Type, Payload Preview. Events are loaded in pages. Scrolling loads additional pages as needed. The detail panel works the same as Stream Events.

### AC-7: Live Tail

The Live Tail tab subscribes to all events via `SubscribeAll`. Events appear in a scrolling list that auto-scrolls to the bottom. The status bar shows "Catching up..." during catch-up and "Live" after. `p` pauses/resumes auto-scroll. New events continue buffering while paused. Maximum 10,000 events in the buffer (oldest evicted).

### AC-8: Payload display

Byte payloads and metadata are displayed as: pretty-printed JSON if valid JSON, UTF-8 string if valid UTF-8, hex dump otherwise. `f` toggles between formatted and raw display.

### AC-9: Build and lint

`cargo build` zero warnings. `cargo clippy --all-targets --all-features --locked -- -D warnings` passes. `cargo fmt --check` passes. `cargo test` all green (existing tests unaffected).

## Dependencies

- **Depends on**: PRDs 001-008 (complete EventfoldDB with gRPC server)
- **Depended on by**: Nothing
- **External crates**: clap 4, crossterm 0.28, ratatui 0.29, anyhow 1, serde_json 1, unicode-width 0.2

## Cargo.toml Additions

Root `Cargo.toml` — add workspace section:

```toml
[workspace]
members = [".", "eventfold-console"]
```

New `eventfold-console/Cargo.toml` — see Dependencies section above.
