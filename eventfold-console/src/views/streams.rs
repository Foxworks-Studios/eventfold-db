//! Streams list view: table of all streams with event counts.
//!
//! Renders a table showing every known stream with its UUID, event count,
//! and latest version. Supports cursor navigation and Enter-to-select.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Row, Table};

use crate::app::AppState;

/// Render the streams list table into the given area.
///
/// Shows a table with columns: Stream ID, Event Count, Latest Version.
/// The currently selected row is highlighted. If no streams are loaded,
/// shows a "No streams" message. If loading, shows "Loading...".
///
/// # Arguments
///
/// * `frame` - The ratatui frame to draw into.
/// * `area` - The rectangular area to render within.
/// * `state` - The application state with streams data.
pub fn render(frame: &mut Frame, area: Rect, state: &AppState) {
    let header = Row::new(vec!["Stream ID", "Events", "Latest Version"]).style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row> = if state.streams_loading {
        vec![Row::new(vec!["Loading...", "", ""])]
    } else if state.streams.is_empty() {
        vec![Row::new(vec!["No streams found", "", ""])]
    } else {
        state
            .streams
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let style = if i == state.streams_cursor {
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                Row::new(vec![
                    s.stream_id.clone(),
                    s.event_count.to_string(),
                    s.latest_version.to_string(),
                ])
                .style(style)
            })
            .collect()
    };

    let widths = [
        ratatui::layout::Constraint::Percentage(60),
        ratatui::layout::Constraint::Percentage(20),
        ratatui::layout::Constraint::Percentage(20),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(" Streams "));

    frame.render_widget(table, area);
}
