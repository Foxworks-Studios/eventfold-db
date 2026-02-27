//! Global log view: all events in global position order.
//!
//! Shows a split layout similar to stream detail: the top half is a paginated
//! events table, the bottom half shows detail for the selected event.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Text;
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table, Wrap};

use crate::app::AppState;
use crate::views::{format_bytes, truncate};

/// Render the global log view into the given area.
///
/// Top half: global events table. Bottom half: detail panel for selected event.
///
/// # Arguments
///
/// * `frame` - The ratatui frame to draw into.
/// * `area` - The rectangular area to render within.
/// * `state` - The application state with global log data.
pub fn render(frame: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    render_events_table(frame, chunks[0], state);
    render_detail_panel(frame, chunks[1], state);
}

/// Render the global events table.
fn render_events_table(frame: &mut Frame, area: Rect, state: &AppState) {
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

    let rows: Vec<Row> = if state.global_loading {
        vec![Row::new(vec!["Loading...", "", "", "", ""])]
    } else if state.global_events.is_empty() {
        vec![Row::new(vec!["No events", "", "", "", ""])]
    } else {
        state
            .global_events
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let style = if i == state.global_cursor {
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
        .block(Block::default().borders(Borders::ALL).title(" Global Log "));

    frame.render_widget(table, area);
}

/// Render the detail panel for the selected event in the global log.
fn render_detail_panel(frame: &mut Frame, area: Rect, state: &AppState) {
    let event = state.global_events.get(state.global_cursor);

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
        None => "Select an event to view details".to_string(),
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
