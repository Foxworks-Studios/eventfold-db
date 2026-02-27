//! Terminal initialization, restoration, and render loop.
//!
//! Provides functions to set up the crossterm backend for ratatui, restore the
//! terminal on exit, and the main render function that draws the TUI layout.

use std::io;

use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Tabs};

use crate::app::{AppState, Tab};
use crate::views;

/// Terminal type alias using the crossterm backend over stdout.
pub type Term = Terminal<CrosstermBackend<io::Stdout>>;

/// Initialize the terminal: enable raw mode, enter alternate screen,
/// and create the ratatui terminal.
///
/// # Returns
///
/// A configured `Terminal` ready for rendering.
///
/// # Errors
///
/// Returns `io::Error` if terminal setup fails.
pub fn init_terminal() -> io::Result<Term> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

/// Restore the terminal: disable raw mode and leave alternate screen.
///
/// This should be called on exit (including panic paths) to avoid leaving
/// the terminal in a broken state.
///
/// # Errors
///
/// Returns `io::Error` if terminal restoration fails.
pub fn restore_terminal() -> io::Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;
    Ok(())
}

/// All possible tab variants in display order.
const TAB_ORDER: [Tab; 4] = [
    Tab::Streams,
    Tab::StreamDetail,
    Tab::GlobalLog,
    Tab::LiveTail,
];

/// Render the full TUI layout to the terminal.
///
/// Layout:
/// ```text
/// +--[ Streams ]--[ Stream Events ]--[ Global Log ]--[ Live Tail ]--+
/// |                                                                   |
/// |                         Main content area                         |
/// |                                                                   |
/// +-------------------------------------------------------------------+
/// | Status bar                                                        |
/// +-------------------------------------------------------------------+
/// ```
///
/// # Arguments
///
/// * `terminal` - The ratatui terminal to draw to.
/// * `state` - The current application state.
///
/// # Errors
///
/// Returns `io::Error` if drawing fails.
pub fn render(terminal: &mut Term, state: &AppState) -> io::Result<()> {
    terminal.draw(|frame| {
        let size = frame.area();

        // Split into: tabs bar (3 lines), content area, status bar (1 line).
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(size);

        render_tabs(frame, chunks[0], state);
        render_content(frame, chunks[1], state);
        render_status_bar(frame, chunks[2], state);
    })?;
    Ok(())
}

/// Render the tab bar at the top of the screen.
fn render_tabs(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let titles: Vec<Line> = TAB_ORDER
        .iter()
        .map(|t| {
            let style = if *t == state.active_tab {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            Line::from(Span::styled(t.label(), style))
        })
        .collect();

    let selected = TAB_ORDER
        .iter()
        .position(|&t| t == state.active_tab)
        .unwrap_or(0);

    let tabs = Tabs::new(titles)
        .select(selected)
        .block(Block::default().borders(Borders::ALL))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );

    frame.render_widget(tabs, area);
}

/// Render the main content area based on the active tab.
fn render_content(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {
    match state.active_tab {
        Tab::Streams => views::streams::render(frame, area, state),
        Tab::StreamDetail => views::stream_detail::render(frame, area, state),
        Tab::GlobalLog => views::global_log::render(frame, area, state),
        Tab::LiveTail => views::live_tail::render(frame, area, state),
    }
}

/// Render the status bar at the bottom of the screen.
fn render_status_bar(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let stream_count = state.streams.len();
    let json_indicator = if state.format_json {
        "JSON:on"
    } else {
        "JSON:off"
    };
    let text = format!(
        " Connected: {} | {} streams | {} | 1-4:tab  j/k:scroll  Enter:select  \
         r:refresh  f:format  p:pause  q:quit",
        state.server_addr, stream_count, json_indicator,
    );

    let paragraph = Paragraph::new(Line::from(Span::styled(
        text,
        Style::default().fg(Color::White).bg(Color::DarkGray),
    )));
    frame.render_widget(paragraph, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    // Terminal init/restore are side-effectful (raw mode, alternate screen),
    // so we only test them as compile-time checks and verify the render
    // function works with a test backend.

    #[test]
    fn render_does_not_panic_with_test_backend() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let state = AppState::new("127.0.0.1:2113".into());
        // Render each tab to exercise all views.
        for tab in &TAB_ORDER {
            let mut s = AppState::new("test".into());
            s.active_tab = *tab;
            terminal
                .draw(|frame| {
                    let size = frame.area();
                    let chunks = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(3),
                            Constraint::Min(1),
                            Constraint::Length(1),
                        ])
                        .split(size);
                    render_tabs(frame, chunks[0], &s);
                    render_content(frame, chunks[1], &s);
                    render_status_bar(frame, chunks[2], &s);
                })
                .expect("draw should not fail");
        }
        // Also test the public render() function (it needs the real Term type,
        // so we just verify the inner functions work).
        let _ = state; // used above indirectly
    }

    #[test]
    fn render_with_populated_streams() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut state = AppState::new("test".into());
        state.streams = vec![
            crate::app::StreamInfo {
                stream_id: "abc-123".into(),
                event_count: 5,
                latest_version: 4,
            },
            crate::app::StreamInfo {
                stream_id: "def-456".into(),
                event_count: 10,
                latest_version: 9,
            },
        ];
        terminal
            .draw(|frame| {
                let size = frame.area();
                render_content(frame, size, &state);
            })
            .expect("draw should not fail");
    }

    #[test]
    fn render_with_populated_detail() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut state = AppState::new("test".into());
        state.active_tab = Tab::StreamDetail;
        state.detail_stream_id = Some("abc-123".into());
        state.detail_events = vec![crate::app::EventRecord {
            event_id: "eid-1".into(),
            stream_id: "abc-123".into(),
            stream_version: 0,
            global_position: 0,
            event_type: "OrderPlaced".into(),
            metadata: vec![],
            payload: br#"{"item":"widget"}"#.to_vec(),
        }];
        terminal
            .draw(|frame| {
                let size = frame.area();
                render_content(frame, size, &state);
            })
            .expect("draw should not fail");
    }

    #[test]
    fn render_live_tail_with_events() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut state = AppState::new("test".into());
        state.active_tab = Tab::LiveTail;
        for i in 0..5 {
            state.push_live_event(crate::app::EventRecord {
                event_id: format!("eid-{i}"),
                stream_id: "sid-1".into(),
                stream_version: i,
                global_position: i,
                event_type: "TestEvent".into(),
                metadata: vec![],
                payload: br#"{}"#.to_vec(),
            });
        }
        state.live_caught_up = true;
        terminal
            .draw(|frame| {
                let size = frame.area();
                render_content(frame, size, &state);
            })
            .expect("draw should not fail");
    }
}
