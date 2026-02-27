//! Application state and input handling for the TUI.
//!
//! Defines the core [`AppState`] struct that holds all mutable UI state, the
//! [`Tab`] enum for navigation, and the [`Action`] enum for user-triggered
//! actions. Key events are mapped to actions via [`handle_key_event`].

use std::collections::{HashMap, VecDeque};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Maximum number of events to retain in the live tail buffer.
const LIVE_TAIL_CAPACITY: usize = 10_000;

/// Which tab the user is currently viewing.
///
/// Tabs are ordered left-to-right in the tab bar. Each variant corresponds
/// to a distinct view with its own rendering and data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// Lists all streams with event counts and latest version.
    Streams,
    /// Shows events within a single selected stream.
    StreamDetail,
    /// Shows all events in global position order.
    GlobalLog,
    /// Auto-scrolling live subscription view.
    LiveTail,
}

/// All possible tabs in display order, used for Tab/Shift-Tab cycling.
const TAB_ORDER: [Tab; 4] = [
    Tab::Streams,
    Tab::StreamDetail,
    Tab::GlobalLog,
    Tab::LiveTail,
];

impl Tab {
    /// Returns the next tab in the cycle (wraps around).
    fn next(self) -> Tab {
        let idx = TAB_ORDER
            .iter()
            .position(|&t| t == self)
            .expect("tab in order");
        TAB_ORDER[(idx + 1) % TAB_ORDER.len()]
    }

    /// Returns the previous tab in the cycle (wraps around).
    fn prev(self) -> Tab {
        let idx = TAB_ORDER
            .iter()
            .position(|&t| t == self)
            .expect("tab in order");
        TAB_ORDER[(idx + TAB_ORDER.len() - 1) % TAB_ORDER.len()]
    }

    /// Returns the tab label for display in the tab bar.
    pub fn label(self) -> &'static str {
        match self {
            Tab::Streams => "Streams",
            Tab::StreamDetail => "Stream Events",
            Tab::GlobalLog => "Global Log",
            Tab::LiveTail => "Live Tail",
        }
    }
}

/// Summary info for a single stream, derived from a ReadAll scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamInfo {
    /// Stream UUID as a string.
    pub stream_id: String,
    /// Total number of events in this stream.
    pub event_count: u64,
    /// Latest stream version (zero-based).
    pub latest_version: u64,
}

/// A single recorded event as received from the gRPC API.
///
/// This is a lightweight representation used by the TUI -- not the full
/// domain `RecordedEvent` from `eventfold-db`, since we receive proto types
/// over the wire.
#[derive(Debug, Clone)]
pub struct EventRecord {
    /// Client-assigned event UUID string.
    pub event_id: String,
    /// Stream UUID string.
    pub stream_id: String,
    /// Zero-based version within the stream.
    pub stream_version: u64,
    /// Zero-based position in the global log.
    pub global_position: u64,
    /// Event type tag (e.g. "OrderPlaced").
    pub event_type: String,
    /// Raw metadata bytes.
    pub metadata: Vec<u8>,
    /// Raw payload bytes.
    pub payload: Vec<u8>,
}

/// Actions that the UI can trigger in response to input or async results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Quit the application.
    Quit,
    /// Switch to a specific tab.
    SwitchTab(Tab),
    /// Move cursor up in the current list.
    CursorUp,
    /// Move cursor down in the current list.
    CursorDown,
    /// Activate the selected item (e.g. Enter on Streams -> StreamDetail).
    Select,
    /// Refresh the current view's data.
    Refresh,
    /// Toggle JSON formatting for payload/metadata display.
    ToggleFormat,
    /// Pause/resume auto-scroll on Live Tail.
    TogglePause,
}

/// The full mutable state of the TUI application.
///
/// All rendering reads from this struct; all input handling mutates it.
/// The render loop and event loop share a single instance.
#[derive(Debug)]
pub struct AppState {
    /// Currently active tab.
    pub active_tab: Tab,
    /// Whether the application should exit.
    pub should_quit: bool,
    /// Server address for display in the status bar.
    pub server_addr: String,
    /// Whether to pretty-print JSON in detail panels.
    pub format_json: bool,

    // -- Streams tab --
    /// Cached stream list from the last ReadAll scan.
    pub streams: Vec<StreamInfo>,
    /// Selected index in the streams list.
    pub streams_cursor: usize,
    /// Whether streams data is currently loading.
    pub streams_loading: bool,

    // -- Stream Detail tab --
    /// Stream ID currently being viewed in the detail tab.
    pub detail_stream_id: Option<String>,
    /// Events loaded for the detail stream.
    pub detail_events: Vec<EventRecord>,
    /// Selected index in the detail events list.
    pub detail_cursor: usize,
    /// Whether detail data is currently loading.
    pub detail_loading: bool,

    // -- Global Log tab --
    /// Events loaded for the global log view.
    pub global_events: Vec<EventRecord>,
    /// Selected index in the global log list.
    pub global_cursor: usize,
    /// Whether global log data is currently loading.
    pub global_loading: bool,

    // -- Live Tail tab --
    /// Buffered events from the subscription (capped at [`LIVE_TAIL_CAPACITY`]).
    pub live_events: VecDeque<EventRecord>,
    /// Whether the live tail has caught up with historical events.
    pub live_caught_up: bool,
    /// Whether auto-scroll is paused.
    pub live_paused: bool,
    /// Selected index in the live tail list (when paused).
    pub live_cursor: usize,
}

impl AppState {
    /// Create a new `AppState` with default values.
    ///
    /// # Arguments
    ///
    /// * `server_addr` - The server address string for status bar display.
    pub fn new(server_addr: String) -> Self {
        Self {
            active_tab: Tab::Streams,
            should_quit: false,
            server_addr,
            format_json: true,
            streams: Vec::new(),
            streams_cursor: 0,
            streams_loading: false,
            detail_stream_id: None,
            detail_events: Vec::new(),
            detail_cursor: 0,
            detail_loading: false,
            global_events: Vec::new(),
            global_cursor: 0,
            global_loading: false,
            live_events: VecDeque::new(),
            live_caught_up: false,
            live_paused: false,
            live_cursor: 0,
        }
    }

    /// Push a live event into the tail buffer, evicting the oldest if at capacity.
    pub fn push_live_event(&mut self, event: EventRecord) {
        if self.live_events.len() >= LIVE_TAIL_CAPACITY {
            self.live_events.pop_front();
        }
        self.live_events.push_back(event);
    }

    /// Returns the number of items in the current tab's list for cursor bounds.
    fn current_list_len(&self) -> usize {
        match self.active_tab {
            Tab::Streams => self.streams.len(),
            Tab::StreamDetail => self.detail_events.len(),
            Tab::GlobalLog => self.global_events.len(),
            Tab::LiveTail => self.live_events.len(),
        }
    }

    /// Returns a mutable reference to the current tab's cursor.
    fn current_cursor_mut(&mut self) -> &mut usize {
        match self.active_tab {
            Tab::Streams => &mut self.streams_cursor,
            Tab::StreamDetail => &mut self.detail_cursor,
            Tab::GlobalLog => &mut self.global_cursor,
            Tab::LiveTail => &mut self.live_cursor,
        }
    }

    /// Apply an [`Action`] to mutate the application state.
    pub fn apply_action(&mut self, action: Action) {
        match action {
            Action::Quit => self.should_quit = true,
            Action::SwitchTab(tab) => {
                self.active_tab = tab;
                // Auto-load data when switching to a tab that has no data yet.
                match tab {
                    Tab::Streams if self.streams.is_empty() && !self.streams_loading => {
                        self.streams_loading = true;
                    }
                    Tab::GlobalLog if self.global_events.is_empty() && !self.global_loading => {
                        self.global_loading = true;
                    }
                    _ => {}
                }
            }
            Action::CursorUp => {
                let cursor = self.current_cursor_mut();
                *cursor = cursor.saturating_sub(1);
            }
            Action::CursorDown => {
                let len = self.current_list_len();
                let cursor = self.current_cursor_mut();
                if len > 0 {
                    *cursor = (*cursor + 1).min(len - 1);
                }
            }
            Action::Select => {
                // On Streams tab, Enter switches to StreamDetail for the selected stream
                // and triggers a data load.
                if self.active_tab == Tab::Streams
                    && let Some(stream) = self.streams.get(self.streams_cursor)
                {
                    self.detail_stream_id = Some(stream.stream_id.clone());
                    self.detail_events.clear();
                    self.detail_cursor = 0;
                    self.detail_loading = true;
                    self.active_tab = Tab::StreamDetail;
                }
            }
            Action::Refresh => {
                // Mark current tab as loading; the event loop will trigger
                // the actual data fetch.
                match self.active_tab {
                    Tab::Streams => self.streams_loading = true,
                    Tab::StreamDetail => self.detail_loading = true,
                    Tab::GlobalLog => self.global_loading = true,
                    Tab::LiveTail => {} // live tail refreshes automatically
                }
            }
            Action::ToggleFormat => self.format_json = !self.format_json,
            Action::TogglePause => {
                if self.active_tab == Tab::LiveTail {
                    self.live_paused = !self.live_paused;
                }
            }
        }
    }

    /// Collect unique streams from a batch of events (e.g. from a ReadAll scan).
    ///
    /// Builds `StreamInfo` entries by tracking max stream_version and count
    /// per stream_id. Results are sorted by stream_id for stable display order.
    pub fn collect_streams(events: &[EventRecord]) -> Vec<StreamInfo> {
        let mut map: HashMap<&str, (u64, u64)> = HashMap::new();
        for event in events {
            let entry = map.entry(&event.stream_id).or_insert((0, 0));
            entry.0 += 1; // event_count
            if event.stream_version > entry.1 {
                entry.1 = event.stream_version; // latest_version
            }
        }
        let mut streams: Vec<StreamInfo> = map
            .into_iter()
            .map(|(id, (count, version))| StreamInfo {
                stream_id: id.to_string(),
                event_count: count,
                latest_version: version,
            })
            .collect();
        streams.sort_by(|a, b| a.stream_id.cmp(&b.stream_id));
        streams
    }
}

/// Map a crossterm [`KeyEvent`] to an [`Action`], if applicable.
///
/// Returns `None` for keys that have no mapped action (e.g. typing random chars).
///
/// # Key Bindings
///
/// | Key             | Action                          |
/// |-----------------|---------------------------------|
/// | `q` / `Esc`     | Quit                           |
/// | `1`-`4`         | Switch to tab 1-4              |
/// | `Tab`           | Next tab                       |
/// | `Shift+Tab`     | Previous tab                   |
/// | `j` / `Down`    | Cursor down                    |
/// | `k` / `Up`      | Cursor up                      |
/// | `Enter`         | Select                         |
/// | `r`             | Refresh                        |
/// | `f`             | Toggle JSON formatting         |
/// | `p`             | Toggle pause (Live Tail)       |
pub fn handle_key_event(key: KeyEvent, current_tab: Tab) -> Option<Action> {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => Some(Action::Quit),
        KeyCode::Char('1') => Some(Action::SwitchTab(Tab::Streams)),
        KeyCode::Char('2') => Some(Action::SwitchTab(Tab::StreamDetail)),
        KeyCode::Char('3') => Some(Action::SwitchTab(Tab::GlobalLog)),
        KeyCode::Char('4') => Some(Action::SwitchTab(Tab::LiveTail)),
        KeyCode::Tab => Some(Action::SwitchTab(current_tab.next())),
        KeyCode::BackTab => Some(Action::SwitchTab(current_tab.prev())),
        KeyCode::Char('j') | KeyCode::Down => Some(Action::CursorDown),
        KeyCode::Char('k') | KeyCode::Up => Some(Action::CursorUp),
        KeyCode::Enter => Some(Action::Select),
        KeyCode::Char('r') => Some(Action::Refresh),
        KeyCode::Char('f') => Some(Action::ToggleFormat),
        KeyCode::Char('p') => Some(Action::TogglePause),
        // Ctrl+C also quits.
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Some(Action::Quit),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Tab tests --

    #[test]
    fn tab_next_cycles_forward() {
        assert_eq!(Tab::Streams.next(), Tab::StreamDetail);
        assert_eq!(Tab::StreamDetail.next(), Tab::GlobalLog);
        assert_eq!(Tab::GlobalLog.next(), Tab::LiveTail);
        assert_eq!(Tab::LiveTail.next(), Tab::Streams); // wraps
    }

    #[test]
    fn tab_prev_cycles_backward() {
        assert_eq!(Tab::Streams.prev(), Tab::LiveTail); // wraps
        assert_eq!(Tab::StreamDetail.prev(), Tab::Streams);
        assert_eq!(Tab::GlobalLog.prev(), Tab::StreamDetail);
        assert_eq!(Tab::LiveTail.prev(), Tab::GlobalLog);
    }

    #[test]
    fn tab_labels_are_non_empty() {
        for tab in &TAB_ORDER {
            assert!(!tab.label().is_empty());
        }
    }

    // -- AppState construction --

    #[test]
    fn new_app_state_defaults() {
        let state = AppState::new("127.0.0.1:2113".into());
        assert_eq!(state.active_tab, Tab::Streams);
        assert!(!state.should_quit);
        assert_eq!(state.server_addr, "127.0.0.1:2113");
        assert!(state.format_json);
        assert!(state.streams.is_empty());
        assert_eq!(state.streams_cursor, 0);
        assert!(!state.streams_loading);
        assert!(state.detail_stream_id.is_none());
        assert!(state.live_events.is_empty());
        assert!(!state.live_caught_up);
        assert!(!state.live_paused);
    }

    // -- Action::Quit --

    #[test]
    fn action_quit_sets_should_quit() {
        let mut state = AppState::new("test".into());
        state.apply_action(Action::Quit);
        assert!(state.should_quit);
    }

    // -- Action::SwitchTab --

    #[test]
    fn action_switch_tab_changes_active_tab() {
        let mut state = AppState::new("test".into());
        state.apply_action(Action::SwitchTab(Tab::GlobalLog));
        assert_eq!(state.active_tab, Tab::GlobalLog);
        state.apply_action(Action::SwitchTab(Tab::LiveTail));
        assert_eq!(state.active_tab, Tab::LiveTail);
    }

    // -- Cursor movement --

    #[test]
    fn cursor_down_advances_within_bounds() {
        let mut state = AppState::new("test".into());
        state.streams = vec![
            StreamInfo {
                stream_id: "a".into(),
                event_count: 1,
                latest_version: 0,
            },
            StreamInfo {
                stream_id: "b".into(),
                event_count: 2,
                latest_version: 1,
            },
        ];
        state.apply_action(Action::CursorDown);
        assert_eq!(state.streams_cursor, 1);
        // Should not exceed last index.
        state.apply_action(Action::CursorDown);
        assert_eq!(state.streams_cursor, 1);
    }

    #[test]
    fn cursor_up_does_not_go_below_zero() {
        let mut state = AppState::new("test".into());
        assert_eq!(state.streams_cursor, 0);
        state.apply_action(Action::CursorUp);
        assert_eq!(state.streams_cursor, 0);
    }

    #[test]
    fn cursor_down_on_empty_list_stays_at_zero() {
        let mut state = AppState::new("test".into());
        assert!(state.streams.is_empty());
        state.apply_action(Action::CursorDown);
        assert_eq!(state.streams_cursor, 0);
    }

    // -- Action::Select on Streams --

    #[test]
    fn select_on_streams_switches_to_detail() {
        let mut state = AppState::new("test".into());
        state.streams = vec![StreamInfo {
            stream_id: "abc-123".into(),
            event_count: 5,
            latest_version: 4,
        }];
        state.apply_action(Action::Select);
        assert_eq!(state.active_tab, Tab::StreamDetail);
        assert_eq!(state.detail_stream_id.as_deref(), Some("abc-123"));
        assert!(state.detail_loading);
    }

    #[test]
    fn select_on_empty_streams_does_nothing() {
        let mut state = AppState::new("test".into());
        state.apply_action(Action::Select);
        assert_eq!(state.active_tab, Tab::Streams); // unchanged
        assert!(state.detail_stream_id.is_none());
    }

    // -- Action::Refresh --

    #[test]
    fn refresh_sets_loading_on_streams_tab() {
        let mut state = AppState::new("test".into());
        state.apply_action(Action::Refresh);
        assert!(state.streams_loading);
    }

    #[test]
    fn refresh_sets_loading_on_global_log_tab() {
        let mut state = AppState::new("test".into());
        state.active_tab = Tab::GlobalLog;
        state.apply_action(Action::Refresh);
        assert!(state.global_loading);
    }

    // -- SwitchTab auto-load --

    #[test]
    fn switch_to_global_log_auto_loads_when_empty() {
        let mut state = AppState::new("test".into());
        assert!(state.global_events.is_empty());
        state.apply_action(Action::SwitchTab(Tab::GlobalLog));
        assert_eq!(state.active_tab, Tab::GlobalLog);
        assert!(state.global_loading);
    }

    #[test]
    fn switch_to_global_log_skips_load_when_populated() {
        let mut state = AppState::new("test".into());
        state.global_events.push(EventRecord {
            event_id: "eid".into(),
            stream_id: "sid".into(),
            stream_version: 0,
            global_position: 0,
            event_type: "Test".into(),
            metadata: vec![],
            payload: vec![],
        });
        state.apply_action(Action::SwitchTab(Tab::GlobalLog));
        assert!(!state.global_loading);
    }

    #[test]
    fn switch_to_streams_auto_loads_when_empty() {
        let mut state = AppState::new("test".into());
        state.active_tab = Tab::GlobalLog; // start elsewhere
        state.apply_action(Action::SwitchTab(Tab::Streams));
        assert!(state.streams_loading);
    }

    // -- Action::ToggleFormat --

    #[test]
    fn toggle_format_flips_flag() {
        let mut state = AppState::new("test".into());
        assert!(state.format_json); // default true
        state.apply_action(Action::ToggleFormat);
        assert!(!state.format_json);
        state.apply_action(Action::ToggleFormat);
        assert!(state.format_json);
    }

    // -- Action::TogglePause --

    #[test]
    fn toggle_pause_only_affects_live_tail() {
        let mut state = AppState::new("test".into());
        // On Streams tab, pause does nothing.
        state.apply_action(Action::TogglePause);
        assert!(!state.live_paused);

        // On LiveTail tab, pause toggles.
        state.active_tab = Tab::LiveTail;
        state.apply_action(Action::TogglePause);
        assert!(state.live_paused);
        state.apply_action(Action::TogglePause);
        assert!(!state.live_paused);
    }

    // -- Live tail buffer --

    #[test]
    fn push_live_event_appends_to_buffer() {
        let mut state = AppState::new("test".into());
        state.push_live_event(make_event(0));
        assert_eq!(state.live_events.len(), 1);
    }

    #[test]
    fn push_live_event_evicts_oldest_at_capacity() {
        let mut state = AppState::new("test".into());
        for i in 0..LIVE_TAIL_CAPACITY + 5 {
            state.push_live_event(make_event(i as u64));
        }
        assert_eq!(state.live_events.len(), LIVE_TAIL_CAPACITY);
        // Oldest should be event 5 (first 5 were evicted).
        assert_eq!(state.live_events.front().unwrap().global_position, 5);
    }

    // -- collect_streams --

    #[test]
    fn collect_streams_groups_by_stream_id() {
        let events = vec![
            make_event_with_stream("s1", 0, 0),
            make_event_with_stream("s1", 1, 1),
            make_event_with_stream("s2", 0, 2),
        ];
        let streams = AppState::collect_streams(&events);
        assert_eq!(streams.len(), 2);
        // Sorted by stream_id.
        assert_eq!(streams[0].stream_id, "s1");
        assert_eq!(streams[0].event_count, 2);
        assert_eq!(streams[0].latest_version, 1);
        assert_eq!(streams[1].stream_id, "s2");
        assert_eq!(streams[1].event_count, 1);
        assert_eq!(streams[1].latest_version, 0);
    }

    #[test]
    fn collect_streams_empty_input_returns_empty() {
        let streams = AppState::collect_streams(&[]);
        assert!(streams.is_empty());
    }

    // -- Key event mapping --

    #[test]
    fn key_q_maps_to_quit() {
        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert_eq!(handle_key_event(key, Tab::Streams), Some(Action::Quit));
    }

    #[test]
    fn key_esc_maps_to_quit() {
        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(handle_key_event(key, Tab::Streams), Some(Action::Quit));
    }

    #[test]
    fn key_ctrl_c_maps_to_quit() {
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(handle_key_event(key, Tab::Streams), Some(Action::Quit));
    }

    #[test]
    fn number_keys_switch_tabs() {
        let key1 = KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE);
        assert_eq!(
            handle_key_event(key1, Tab::GlobalLog),
            Some(Action::SwitchTab(Tab::Streams))
        );
        let key4 = KeyEvent::new(KeyCode::Char('4'), KeyModifiers::NONE);
        assert_eq!(
            handle_key_event(key4, Tab::Streams),
            Some(Action::SwitchTab(Tab::LiveTail))
        );
    }

    #[test]
    fn tab_key_advances_to_next_tab() {
        let key = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(
            handle_key_event(key, Tab::Streams),
            Some(Action::SwitchTab(Tab::StreamDetail))
        );
    }

    #[test]
    fn backtab_key_goes_to_previous_tab() {
        let key = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(
            handle_key_event(key, Tab::Streams),
            Some(Action::SwitchTab(Tab::LiveTail))
        );
    }

    #[test]
    fn j_and_down_map_to_cursor_down() {
        let j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(handle_key_event(j, Tab::Streams), Some(Action::CursorDown));
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(
            handle_key_event(down, Tab::Streams),
            Some(Action::CursorDown)
        );
    }

    #[test]
    fn k_and_up_map_to_cursor_up() {
        let k = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE);
        assert_eq!(handle_key_event(k, Tab::Streams), Some(Action::CursorUp));
        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(handle_key_event(up, Tab::Streams), Some(Action::CursorUp));
    }

    #[test]
    fn enter_maps_to_select() {
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(handle_key_event(key, Tab::Streams), Some(Action::Select));
    }

    #[test]
    fn r_maps_to_refresh() {
        let key = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE);
        assert_eq!(handle_key_event(key, Tab::Streams), Some(Action::Refresh));
    }

    #[test]
    fn f_maps_to_toggle_format() {
        let key = KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE);
        assert_eq!(
            handle_key_event(key, Tab::Streams),
            Some(Action::ToggleFormat)
        );
    }

    #[test]
    fn p_maps_to_toggle_pause() {
        let key = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE);
        assert_eq!(
            handle_key_event(key, Tab::Streams),
            Some(Action::TogglePause)
        );
    }

    #[test]
    fn unknown_key_returns_none() {
        let key = KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE);
        assert_eq!(handle_key_event(key, Tab::Streams), None);
    }

    // -- Test helpers --

    fn make_event(global_position: u64) -> EventRecord {
        EventRecord {
            event_id: format!("eid-{global_position}"),
            stream_id: "stream-1".into(),
            stream_version: global_position,
            global_position,
            event_type: "TestEvent".into(),
            metadata: vec![],
            payload: vec![],
        }
    }

    fn make_event_with_stream(
        stream_id: &str,
        stream_version: u64,
        global_position: u64,
    ) -> EventRecord {
        EventRecord {
            event_id: format!("eid-{global_position}"),
            stream_id: stream_id.into(),
            stream_version,
            global_position,
            event_type: "TestEvent".into(),
            metadata: vec![],
            payload: vec![],
        }
    }
}
