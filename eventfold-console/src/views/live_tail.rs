//! Live tail view: auto-scrolling subscription with pause support.
//!
//! Displays events streaming from a `SubscribeAll` subscription in real time.
//! Shows a "Catching up..." indicator until the `CaughtUp` marker is received,
//! then switches to "Live". The `p` key pauses auto-scroll for inspection.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Text;
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table, Wrap};

use crate::app::AppState;
use crate::views::{format_bytes, truncate};

/// Render the live tail view into the given area.
///
/// Top section: scrolling events table. Bottom section: detail of the
/// event at the cursor (visible when paused). A status indicator shows
/// "Catching up...", "Live", or "Paused".
///
/// # Arguments
///
/// * `frame` - The ratatui frame to draw into.
/// * `area` - The rectangular area to render within.
/// * `state` - The application state with live tail data.
pub fn render(frame: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);

    render_events_table(frame, chunks[0], state);
    render_detail_panel(frame, chunks[1], state);
}

/// Render the live events table.
fn render_events_table(frame: &mut Frame, area: Rect, state: &AppState) {
    let status = if state.live_paused {
        "Paused"
    } else if state.live_caught_up {
        "Live"
    } else {
        "Catching up..."
    };

    let title = format!(
        " Live Tail [{status}] ({} events) ",
        state.live_events.len()
    );

    let header = Row::new(vec![
        "Global Pos",
        "Stream ID",
        "Version",
        "Type",
        "Preview",
    ])
    .style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row> = if state.live_events.is_empty() {
        vec![Row::new(vec!["Waiting for events...", "", "", "", ""])]
    } else {
        // Show events from the VecDeque. When not paused, auto-scroll to bottom.
        // When paused, render around the cursor position.
        let events: Vec<_> = state.live_events.iter().collect();
        let visible_height = area.height.saturating_sub(4) as usize; // account for borders+header
        let display_cursor = if state.live_paused {
            state.live_cursor
        } else {
            events.len().saturating_sub(1)
        };

        // Determine the window of events to show.
        let start = display_cursor.saturating_sub(visible_height.saturating_sub(1));
        let end = events.len().min(start + visible_height);

        events[start..end]
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let actual_idx = start + i;
                let style = if actual_idx == display_cursor {
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                let preview = truncate(&format_bytes(&e.payload, false), 30);
                Row::new(vec![
                    e.global_position.to_string(),
                    truncate(&e.stream_id, 12),
                    e.stream_version.to_string(),
                    e.event_type.clone(),
                    preview,
                ])
                .style(style)
            })
            .collect()
    };

    let widths = [
        Constraint::Length(12),
        Constraint::Length(14),
        Constraint::Length(10),
        Constraint::Percentage(25),
        Constraint::Percentage(30),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(title));

    frame.render_widget(table, area);
}

/// Render the detail panel for the selected live event.
fn render_detail_panel(frame: &mut Frame, area: Rect, state: &AppState) {
    let cursor = if state.live_paused {
        state.live_cursor
    } else {
        state.live_events.len().saturating_sub(1)
    };
    let event = state.live_events.get(cursor);

    let text = match event {
        Some(e) => {
            let payload = format_bytes(&e.payload, state.format_json);
            let metadata = format_bytes(&e.metadata, state.format_json);
            format!(
                "Event ID:  {}\nStream:    {}\nVersion:   {}\n\
                 Global:    {}\nType:      {}\n\n\
                 --- Payload ---\n{}\n\n--- Metadata ---\n{}",
                e.event_id,
                e.stream_id,
                e.stream_version,
                e.global_position,
                e.event_type,
                payload,
                metadata,
            )
        }
        None => "Waiting for events...".to_string(),
    };

    let paragraph = Paragraph::new(Text::raw(text))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Event Detail "),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, area);
}
