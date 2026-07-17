//! Ratatui `TestBackend` snapshot coverage for responsive layouts,
//! focus states, overlays, tabs, scrolled logs, and plugin popups.

use mc_protocol::events::{PluginDetails, StatDataModel};
use mc_session::{SessionController, SessionError, SessionEvent};
use ratatui::backend::TestBackend;
use ratatui::style::Color;
use ratatui::Terminal;
use serde_json::json;

use mc_tui::app::{
    App, ConnectionState, EvaluateEntry, Focus, HelpPopup, LogKind, LogState, Mode,
    PluginSelection, Tab,
};
use mc_tui::ui;

/// Helper: construct a minimal App for snapshot testing.
fn make_app(ctrl: SessionController, rx: tokio::sync::mpsc::Receiver<SessionEvent>) -> App {
    App::new(ctrl, rx, "127.0.0.1".into(), 19144, None, None)
}

/// Helper: build a `StatDataModel` leaf/group for tests.
fn make_stat(
    name: &str,
    values: Vec<serde_json::Value>,
    children: Vec<StatDataModel>,
) -> StatDataModel {
    StatDataModel {
        name: name.into(),
        values,
        children,
        should_aggregate: false,
    }
}

/// Helper: populate `app.stats` with deterministic sample data spanning all
/// categories and multiple clients.
fn populate_stats(app: &mut App) {
    let stats = vec![
        make_stat(
            "server_tick_timings",
            vec![],
            vec![
                make_stat("tick", vec![json!(16.67)], vec![]),
                make_stat("entity", vec![json!(1500.0)], vec![]),
            ],
        ),
        make_stat(
            "app_memory",
            vec![],
            vec![
                make_stat("used", vec![json!(1_048_576.0)], vec![]),
                make_stat("total", vec![json!(4_194_304.0)], vec![]),
            ],
        ),
        make_stat(
            "dynamic_property_values",
            vec![],
            vec![
                make_stat("prop1", vec![json!(256.0)], vec![]),
                make_stat("prop2", vec![json!(512.0)], vec![]),
            ],
        ),
        make_stat(
            "handle_counts",
            vec![],
            vec![make_stat("callbacks", vec![json!(42.0)], vec![])],
        ),
        make_stat(
            "client_stats",
            vec![],
            vec![
                make_stat(
                    "client-a",
                    vec![],
                    vec![
                        make_stat("mem", vec![json!(524_288.0)], vec![]),
                        make_stat("cpu", vec![json!(12.0)], vec![]),
                    ],
                ),
                make_stat(
                    "client-b",
                    vec![],
                    vec![
                        make_stat("mem", vec![json!(1_048_576.0)], vec![]),
                        make_stat("cpu", vec![json!(20.0)], vec![]),
                    ],
                ),
            ],
        ),
    ];
    app.stats.accumulate(&stats, 1);
}

fn populate_addon_stats(app: &mut App) {
    app.stats.accumulate(
        &[
            make_stat(
                "handle_counts",
                vec![],
                vec![make_stat("callbacks", vec![json!(4)], vec![])],
            ),
            make_stat(
                "fine_grained_subscribers",
                vec![],
                vec![
                    make_stat(
                        "zeta",
                        vec![],
                        vec![make_stat("eventone", vec![json!(1)], vec![])],
                    ),
                    make_stat(
                        "alpha",
                        vec![],
                        vec![make_stat("eventtwo", vec![json!(2)], vec![])],
                    ),
                ],
            ),
        ],
        1,
    );
}

/// Helper: add many groups under the server-performance category so the
/// dashboard overflows and can be scrolled.
fn populate_scrollable_stats(app: &mut App) {
    // Create many distinct top-level groups in the uncategorized category so
    // card-level scrolling has multiple cards to move through.
    let stats: Vec<StatDataModel> = (0..16)
        .map(|i| make_stat(&format!("group_{i:02}"), vec![json!(i)], vec![]))
        .collect();
    app.stats.accumulate(&stats, 1);
    app.stats_selected_category = Some("uncategorized".into());
}

fn populate_oversized_property_table(app: &mut App) {
    let properties: Vec<StatDataModel> = (0..21)
        .map(|i| make_stat(&format!("prop{i:02}"), vec![json!(i)], vec![]))
        .collect();
    app.stats.accumulate(
        &[make_stat("dynamic_property_values", vec![], properties)],
        1,
    );
    app.stats_selected_category = Some("memory".into());
}

/// Helper: render and return the backend buffer.
fn render_buffer(app: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            ui::render(f, app);
        })
        .unwrap();
    terminal.backend().buffer().clone()
}

/// Check that a slice of the buffer contains a given string pattern.
fn buffer_contains(buf: &ratatui::buffer::Buffer, substring: &str) -> bool {
    for y in 0..buf.area.height {
        let mut line = String::new();
        for x in 0..buf.area.width {
            if let Some(cell) = buf.cell((x, y)) {
                line.push(cell.symbol().chars().next().unwrap_or(' '));
            }
        }
        if line.contains(substring) {
            return true;
        }
    }
    false
}

fn text_positions(buf: &ratatui::buffer::Buffer, substring: &str) -> Vec<(u16, u16)> {
    let mut found = Vec::new();
    for y in 0..buf.area.height {
        let line: String = (0..buf.area.width)
            .filter_map(|x| {
                buf.cell((x, y))
                    .map(|cell| cell.symbol().chars().next().unwrap_or(' '))
            })
            .collect();
        let mut start = 0;
        while let Some(index) = line[start..].find(substring) {
            found.push(((start + index) as u16, y));
            start += index + substring.len();
        }
    }
    found
}

// ── Snapshot: Idle state at 80×24 ─────────────────────────────────────

#[test]
fn snapshot_idle_80x24() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    let buf = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&buf, "Minecraft Debugger"),
        "header should show app name"
    );
    assert!(buffer_contains(&buf, "idle"), "status should show idle");
    assert!(
        buffer_contains(&buf, "Event Log"),
        "log panel should be visible"
    );
    assert!(
        buffer_contains(&buf, "l/c"),
        "footer should advertise listen/connect shortcuts in idle context"
    );
    assert!(
        buffer_contains(&buf, "q Quit"),
        "footer should show quit key"
    );
    assert!(
        buffer_contains(&buf, "h Help"),
        "footer should show help key"
    );
}

#[test]
fn error_popup_is_compact_and_never_echoes_passcode() {
    let sentinel = "never-render-this-passcode";
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    app.sidebar.passcode.value = sentinel.into();
    app.handle_connection_result(Err(SessionError::ConnectionTaskDropped));

    let buf = render_buffer(&mut app, 80, 24);
    assert!(buffer_contains(&buf, "Connection failed"));
    assert!(buffer_contains(&buf, "Esc/Enter"));
    assert!(!buffer_contains(&buf, sentinel));
    assert!(!app
        .event_log
        .iter()
        .any(|entry| entry.message.contains(sentinel)));
}

#[test]
fn default_theme_keeps_selection_semantic_and_log_text_meaningful() {
    assert_eq!(ui::DEFAULT_THEME_NAME, "High Contrast");
    assert_ne!(LogKind::Stopped.symbol(), '?');
    assert!(!LogKind::Stopped.label().is_empty());

    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    app.add_log("visible event text");
    app.log_state.selected = None;
    let buf = render_buffer(&mut app, 80, 24);
    let log = text_positions(&buf, "[1] Log")[0];
    let stats = text_positions(&buf, "[2] Stats")[0];
    assert_ne!(
        buf.cell(log).unwrap().style(),
        buf.cell(stats).unwrap().style()
    );
    assert!(buffer_contains(&buf, "visible event text"));

    let canvas = Color::Rgb(10, 10, 14);
    assert_eq!(buf.cell((0, 1)).unwrap().style().bg, Some(canvas));
    let event = text_positions(&buf, "visible event text")[0];
    assert_eq!(buf.cell(event).unwrap().style().bg, Some(canvas));
    let idle = text_positions(&buf, "idle")[0];
    assert_eq!(
        buf.cell(idle).unwrap().style().bg,
        Some(Color::Rgb(42, 42, 50))
    );
    assert_ne!(
        buf.cell(idle).unwrap().style().fg,
        buf.cell(event).unwrap().style().fg
    );
}

#[test]
fn overlays_are_opaque_over_a_light_terminal_background() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    app.toggle_help();
    let buf = render_buffer(&mut app, 80, 24);

    assert!(
        (0..buf.area.height).all(|y| {
            (0..buf.area.width).all(|x| buf.cell((x, y)).unwrap().style().bg != Some(Color::Reset))
        }),
        "the root canvas and help overlay must not expose Reset backgrounds"
    );
}

// ── Snapshot: Idle state at 120×30 (wide, with sidebar) ───────────────

#[test]
fn snapshot_idle_120x30() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    app.set_mode(Mode::Connect);
    let buf = render_buffer(&mut app, 120, 30);
    assert!(buffer_contains(&buf, "Minecraft Debugger"));
    assert!(buffer_contains(&buf, "Connection"));
    assert!(buffer_contains(&buf, "State: Idle"));
    assert!(buffer_contains(&buf, "Host:"));
    assert!(buffer_contains(&buf, "Port:"));
}

// ── Snapshot: Connected state at 80×24 ────────────────────────────────

#[test]
fn snapshot_connected_80x24() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    app.state = ConnectionState::Connected {
        version: 9,
        plugin_count: 2,
    };
    app.add_log("Connected — protocol v9, 2 plugin(s) registered.");
    app.handle_session_event(SessionEvent::Debuggee(
        mc_protocol::events::DebuggeeEvent::Stopped {
            reason: "breakpoint".into(),
            thread: 0,
        },
    ));

    let buf = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&buf, "connected v9"),
        "header should show protocol version"
    );
    assert!(
        buffer_contains(&buf, "breakpoint"),
        "log should show stopped event"
    );
    assert!(
        buffer_contains(&buf, "STOPPED"),
        "log should show stopped kind"
    );
}

// ── Snapshot: Wide connected with sidebar ─────────────────────────────

#[test]
fn snapshot_connected_120x30() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    app.state = ConnectionState::Connected {
        version: 9,
        plugin_count: 2,
    };
    app.add_log("Connected — protocol v9.");
    app.add_log("[LOG] server started");

    let buf = render_buffer(&mut app, 120, 30);
    assert!(buffer_contains(&buf, "v9"));
    assert!(buffer_contains(&buf, "Connection"));
    assert!(buffer_contains(&buf, "Proto: v9"));
    assert!(buffer_contains(&buf, "Plugins:"));
}

// ── Snapshot: Disconnected state ──────────────────────────────────────

#[test]
fn snapshot_disconnected_80x24() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    app.state = ConnectionState::Disconnected;
    app.add_log("Connection lost (disconnected).");

    let buf = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&buf, "disconnected"),
        "header should show disconnected status"
    );
    assert!(
        buffer_contains(&buf, "Connection lost"),
        "log should show disconnect message"
    );
}

// ── Snapshot: Compact mode abbreviates but remains usable ─────────────

#[test]
fn snapshot_compact_50x15() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    app.state = ConnectionState::Connected {
        version: 7,
        plugin_count: 1,
    };
    app.add_log("Compact mode check.");

    let buf = render_buffer(&mut app, 50, 15);
    assert!(
        buffer_contains(&buf, "MC Dbg"),
        "compact header should abbreviate title"
    );
    assert!(
        buffer_contains(&buf, "v7"),
        "compact header should show version"
    );
    assert!(
        buffer_contains(&buf, "Log"),
        "compact footer should mention log"
    );
    assert!(
        buffer_contains(&buf, "Quit"),
        "compact footer should show quit"
    );
}

// ── Snapshot: Sidebar overlay at 80×24 ────────────────────────────────

#[test]
fn snapshot_sidebar_overlay_80x24() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    app.show_sidebar = true;
    app.focus = Focus::SidebarModeListen;

    let buf = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&buf, "Connection"),
        "overlay should render sidebar"
    );
    assert!(
        buffer_contains(&buf, "Listen"),
        "overlay should show mode selector"
    );
    assert!(
        buffer_contains(&buf, "Port:"),
        "overlay should show port field"
    );
    assert!(
        buffer_contains(&buf, "F11 step in"),
        "main pane should remain visible behind the overlay"
    );
}

// ── Phase 5: Stats dashboard snapshots ────────────────────────────────

#[test]
fn snapshot_stats_addon_selector_and_filtering() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    populate_addon_stats(&mut app);
    app.set_tab(Tab::Stats);
    app.stats_selected_category = Some("scripting".into());
    let all = render_buffer(&mut app, 80, 24);
    assert!(buffer_contains(&all, "Addon:"));
    assert!(buffer_contains(&all, "eventone"));
    assert!(buffer_contains(&all, "eventtwo"));
    assert!(!buffer_contains(&all, "zeta.eventone"));
    assert!(!buffer_contains(&all, "alpha.eventtwo"));
    assert!(buffer_contains(&all, "callbacks"));
    app.stats_selected_addon = Some("alpha".into());
    let selected = render_buffer(&mut app, 80, 24);
    assert!(buffer_contains(&selected, "eventtwo"));
    assert!(!buffer_contains(&selected, "eventone"));
    assert!(buffer_contains(&selected, "callbacks"));
    let compact = render_buffer(&mut app, 50, 15);
    assert!(buffer_contains(&compact, "A:"));
    assert!(buffer_contains(&compact, "Help"));
}

#[test]
fn snapshot_stats_tab_80x24() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    populate_stats(&mut app);
    app.set_tab(Tab::Stats);

    let buf = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&buf, "Server Performance"),
        "default category label should show"
    );
    assert!(
        buffer_contains(&buf, "server_tick_timings"),
        "group card should show"
    );
    assert!(buffer_contains(&buf, "tick"), "series label should show");
    assert!(
        buffer_contains(&buf, "16.7"),
        "formatted latest value should show"
    );
    assert!(
        buffer_contains(&buf, "Clear stats"),
        "footer should advertise clear stats"
    );
    assert_eq!(
        text_positions(&buf, "server_tick_timings")
            .iter()
            .map(|(x, _)| *x)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        1,
        "default layout remains a single card column"
    );
}

#[test]
fn snapshot_stats_wide_grid_120x30() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    populate_stats(&mut app);
    app.set_tab(Tab::Stats);
    app.stats_selected_category = Some("memory".into());

    let buf = render_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "Memory"),
        "memory category should be selected"
    );
    assert!(
        buffer_contains(&buf, "app_memory"),
        "memory group should show in wide grid"
    );
    assert!(
        buffer_contains(&buf, "dynamic_property_values"),
        "dynamic property group should show in wide grid"
    );
    assert!(
        buffer_contains(&buf, "1.00 MB"),
        "memory value should be scaled to MB"
    );
    let titles = text_positions(&buf, "app_memory");
    let dynamic_titles = text_positions(&buf, "dynamic_property_values");
    assert!(!titles.is_empty() && !dynamic_titles.is_empty());
    assert_ne!(
        titles[0].0, dynamic_titles[0].0,
        "wide cards must occupy distinct columns"
    );
}

#[test]
fn snapshot_stats_compact_values_only_50x15() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    populate_stats(&mut app);
    app.set_tab(Tab::Stats);

    let buf = render_buffer(&mut app, 50, 15);
    assert!(
        buffer_contains(&buf, "Perf"),
        "compact category label should show"
    );
    assert!(
        buffer_contains(&buf, "server_tick_timings"),
        "group card should show in compact mode"
    );
    assert!(
        buffer_contains(&buf, "16.7"),
        "compact latest value should show"
    );
    assert!(
        buffer_contains(&buf, "rClr"),
        "compact footer should advertise clear stats"
    );
    assert!(
        buffer_contains(&buf, "h Help"),
        "compact Stats footer must expose help"
    );
}

#[test]
fn snapshot_stats_dynamic_property_table_80x24() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    populate_stats(&mut app);
    app.set_tab(Tab::Stats);
    app.stats_selected_category = Some("memory".into());

    let buf = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&buf, "Memory"),
        "memory category should be selected"
    );
    assert!(
        buffer_contains(&buf, "Property"),
        "dynamic property table header should show"
    );
    assert!(
        buffer_contains(&buf, "Value"),
        "dynamic property table header should show"
    );
    assert!(
        buffer_contains(&buf, "prop1"),
        "dynamic property name should show"
    );
    assert!(
        buffer_contains(&buf, "256.0"),
        "dynamic property value should be formatted truthfully"
    );
}

#[test]
fn snapshot_stats_oversized_table_is_clipped_and_reachable_80x24() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    populate_oversized_property_table(&mut app);
    app.set_tab(Tab::Stats);

    let initial = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&initial, "prop00"),
        "first property renders immediately"
    );
    assert!(
        !buffer_contains(&initial, "prop20"),
        "overflow stays below the viewport"
    );

    app.scroll_stats_bottom();
    let bottom = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&bottom, "prop20"),
        "last property is reachable by scrolling"
    );
    assert!(
        buffer_contains(&bottom, "dynamic_property_values"),
        "card title remains truthful"
    );
    assert!(
        (0..bottom.area.height).any(|y| (0..bottom.area.width)
            .any(|x| { bottom.cell((x, y)).is_some_and(|cell| cell.symbol() != " ") })),
        "oversized dashboard must not render blank"
    );
}

#[test]
fn snapshot_stats_compact_help_keeps_stats_instructions_50x15() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    populate_stats(&mut app);
    app.set_tab(Tab::Stats);
    app.help_popup = HelpPopup::Visible;

    let buf = render_buffer(&mut app, 50, 15);
    assert!(buffer_contains(&buf, "Stats:"));
    assert!(buffer_contains(&buf, "category"));
    assert!(buffer_contains(&buf, "cards"));
}

#[test]
fn snapshot_stats_multiple_clients_120x30() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    populate_stats(&mut app);
    app.set_tab(Tab::Stats);
    app.stats_selected_category = Some("client".into());

    let buf = render_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "Client:"),
        "client selector should show when >1 client IDs"
    );
    assert!(
        buffer_contains(&buf, "client-a"),
        "first client id should appear in selector"
    );
    assert!(
        buffer_contains(&buf, "client-b"),
        "second client id should appear in selector"
    );
    // Default selected client is the first one; only its rows should render.
    assert!(
        buffer_contains(&buf, "client-a.mem"),
        "selected client series should show"
    );
    assert!(
        buffer_contains(&buf, "12.0"),
        "selected client value should show"
    );
    assert!(
        !buffer_contains(&buf, "20.0"),
        "unselected client values should be filtered out"
    );
}

#[test]
fn snapshot_stats_category_navigation_80x24() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    populate_stats(&mut app);
    app.set_tab(Tab::Stats);
    app.reconcile_stats_selection();

    app.select_next_category();
    let buf = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&buf, "Memory"),
        "next category should be memory"
    );
    assert!(
        buffer_contains(&buf, "app_memory"),
        "memory group card should render"
    );
    assert!(
        !buffer_contains(&buf, "server_tick_timings"),
        "previous category cards should not render"
    );

    app.select_prev_category();
    let buf = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&buf, "Server Performance"),
        "previous category should restore server-performance"
    );
    assert!(
        buffer_contains(&buf, "server_tick_timings"),
        "server performance group should render again"
    );
}

#[test]
fn snapshot_stats_scroll_80x24() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    populate_scrollable_stats(&mut app);
    app.set_tab(Tab::Stats);

    let buf = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&buf, "group_00"),
        "first group should be visible before scrolling"
    );
    assert!(
        buffer_contains(&buf, "group_01"),
        "early groups should be visible before scrolling"
    );

    app.scroll_stats_bottom();
    let buf = render_buffer(&mut app, 80, 24);
    assert!(
        !buffer_contains(&buf, "group_00"),
        "first group should scroll out of view"
    );
    assert!(
        buffer_contains(&buf, "group_15"),
        "last group should be reachable by scrolling"
    );
}

#[test]
fn snapshot_stats_reset_80x24() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    populate_stats(&mut app);
    app.set_tab(Tab::Stats);

    app.clear_stats();
    let buf = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&buf, "Waiting for Stat2 data"),
        "clearing stats should show the empty state"
    );
    assert!(
        !buffer_contains(&buf, "server_tick_timings"),
        "cleared stats should not show group cards"
    );
}

#[test]
fn snapshot_stats_empty_state_80x24() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    app.set_tab(Tab::Stats);

    let buf = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&buf, "Waiting for Stat2 data"),
        "empty stats state should explain the wait"
    );
    assert!(
        !buffer_contains(&buf, "Phase 5"),
        "placeholder text should be replaced"
    );
}

// ── Snapshot: Focused field styling ───────────────────────────────────

#[test]
fn snapshot_focused_field_80x24() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    app.show_sidebar = true;
    app.set_mode(Mode::Connect);
    app.focus = Focus::SidebarHost;
    app.sidebar.host.value = "test-host".into();

    let buf = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&buf, "test-host"),
        "focused field value should render"
    );
    assert!(buffer_contains(&buf, "Host:"), "host label should render");
}

// ── Snapshot: Scrolled event log ──────────────────────────────────────

#[test]
fn snapshot_scrolled_log_80x24() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    for i in 0..30 {
        app.add_log(format!("line {i}"));
    }
    app.log_state = LogState::default();
    app.log_state.top();

    let buf = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&buf, "line 0"),
        "top of log should be visible after scroll top"
    );
    assert!(
        buffer_contains(&buf, "Event Log 1/30"),
        "position indicator should show 1/30"
    );
}

// ── Snapshot: Plugin selection popup with a few options ───────────────

#[test]
fn snapshot_plugin_popup_80x24() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    app.state = ConnectionState::Pending;
    app.plugin_selection = Some(PluginSelection::new(vec![
        PluginDetails {
            name: "Minecraft".into(),
            module_uuid: "uuid-mc".into(),
        },
        PluginDetails {
            name: "Scripting".into(),
            module_uuid: "uuid-script".into(),
        },
        PluginDetails {
            name: "Client".into(),
            module_uuid: "uuid-client".into(),
        },
    ]));

    let buf = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&buf, "Select Target Plugin"),
        "popup title should show"
    );
    assert!(
        buffer_contains(&buf, "▸"),
        "popup should display selection marker on the chosen row"
    );
    assert!(
        buffer_contains(&buf, "Minecraft"),
        "first plugin should be listed"
    );
    assert!(
        buffer_contains(&buf, "Scripting"),
        "second plugin should be listed"
    );
    assert!(
        buffer_contains(&buf, "Client"),
        "third plugin should be listed"
    );
    assert!(
        buffer_contains(&buf, "uuid-mc"),
        "plugin uuid should be listed"
    );
    assert!(
        buffer_contains(&buf, "Enter"),
        "footer should show enter key"
    );
    assert!(
        buffer_contains(&buf, "Esc"),
        "footer should show escape key"
    );
}

// ── Snapshot: Plugin popup with many options (scrolling) ──────────────

#[test]
fn snapshot_plugin_popup_many_80x24() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    app.state = ConnectionState::Pending;
    let plugins: Vec<_> = (0..25)
        .map(|i| PluginDetails {
            name: format!("Plugin-{i}"),
            module_uuid: format!("00000000-0000-0000-0000-0000000000{i:02}"),
        })
        .collect();
    let mut sel = PluginSelection::new(plugins);
    sel.selected = 20;
    sel.scroll_offset = 15;
    app.plugin_selection = Some(sel);

    let buf = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&buf, "Select Target Plugin"),
        "popup title should show"
    );
    assert!(
        buffer_contains(&buf, "Plugin-20"),
        "selected plugin should be visible"
    );
    assert!(
        buffer_contains(&buf, "Plugin-15"),
        "scrolled viewport should show earlier plugins"
    );
}

// ── Snapshot: Plugin popup with a very long plugin name/uuid ──────────

#[test]
fn snapshot_plugin_popup_long_80x24() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    app.state = ConnectionState::Pending;
    app.plugin_selection = Some(PluginSelection::new(vec![PluginDetails {
        name: "VeryLongPluginNameThatExceedsThePopupWidthByQuiteALot".into(),
        module_uuid: "12345678-1234-1234-1234-123456789abc".into(),
    }]));

    let buf = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&buf, "VeryLongPluginName"),
        "long plugin name should be visible"
    );
    assert!(
        buffer_contains(&buf, "12345678"),
        "uuid prefix should be visible"
    );
}

// ── Snapshot: Event log with various entry types ──────────────────────

#[test]
fn snapshot_various_events_80x24() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    app.state = ConnectionState::Connected {
        version: 9,
        plugin_count: 1,
    };

    app.add_log("[LOG] This is a log message");
    app.add_log("[WARN] This is a warning");
    app.add_log("[ERROR] This is an error");
    app.add_log("Stopped: breakpoint (thread 0)");
    app.add_log("Terminated: game over");

    let buf = render_buffer(&mut app, 80, 24);
    assert!(buffer_contains(&buf, "LOG"), "log level tag should show");
    assert!(buffer_contains(&buf, "WARN"), "warn level tag should show");
    assert!(
        buffer_contains(&buf, "ERROR"),
        "error level tag should show"
    );
    assert!(
        buffer_contains(&buf, "breakpoint"),
        "stopped event should show"
    );
    assert!(
        buffer_contains(&buf, "game over"),
        "terminated reason should show"
    );
}

// ── Phase 2 visual fix assertions ─────────────────────────────────────

#[test]
fn renderer_log_follow_bottom_row_visible() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    for i in 0..40 {
        app.add_log(format!("tail entry {i}"));
    }
    assert!(app.log_state.follow, "follow should be enabled by default");

    let buf = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&buf, "tail entry 39"),
        "bottom log row must be visible in follow mode"
    );
}

#[test]
fn renderer_log_scrolled_selection_visible() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    for i in 0..40 {
        app.add_log(format!("scrolled entry {i}"));
    }
    app.log_state.scroll(-12, app.event_log.len());
    assert!(
        !app.log_state.follow,
        "scrolling up should leave follow mode"
    );

    let buf = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&buf, "scrolled entry 27"),
        "scrolled selection must stay inside the viewport"
    );
}

#[test]
fn renderer_disconnected_listen_label() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    app.set_last_known_size(120, 30);
    app.set_mode(Mode::Listen);
    app.state = ConnectionState::Disconnected;
    app.show_sidebar = true;

    let buf = render_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "  [Listen]  "),
        "disconnected primary action should say Listen in Listen mode"
    );
    assert!(
        buffer_contains(&buf, "Disconnected"),
        "sidebar summary should show disconnected state"
    );
}

#[test]
fn renderer_disconnected_with_pending_retry_label() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    app.set_last_known_size(120, 30);
    app.set_mode(Mode::Listen);
    app.state = ConnectionState::Disconnected;
    app.show_sidebar = true;
    app.auto_relisten_pending = true;

    let buf = render_buffer(&mut app, 120, 30);
    assert!(
        buffer_contains(&buf, "Cancel retry"),
        "disconnected with pending retry should show Cancel retry"
    );
}

#[test]
fn renderer_empty_log_title() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);

    let buf = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&buf, "0/0"),
        "empty log title should show grounded 0/0"
    );
    assert!(
        !buffer_contains(&buf, "1/0"),
        "empty log title must not show 1/0"
    );
}

#[test]
fn renderer_connected_log_footer_clear() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    app.state = ConnectionState::Connected {
        version: 9,
        plugin_count: 1,
    };
    app.add_log("Connected.");

    let buf = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&buf, "c Clear"),
        "connected log footer should advertise c Clear"
    );
}

// ── Phase 3 visual assertions ─────────────────────────────────────────

#[test]
fn snapshot_running_controls_80x24() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    app.state = ConnectionState::Connected {
        version: 9,
        plugin_count: 1,
    };

    let buf = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&buf, "running"),
        "debug strip should show running state"
    );
    assert!(
        buffer_contains(&buf, "F5 continue"),
        "debug strip should show continue label"
    );
    assert!(
        buffer_contains(&buf, "F6 pause"),
        "debug strip should show pause label"
    );
}

#[test]
fn snapshot_paused_controls_80x24() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    app.state = ConnectionState::Connected {
        version: 9,
        plugin_count: 1,
    };
    app.stopped = true;
    app.stopped_thread_id = Some(0);

    let buf = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&buf, "paused"),
        "debug strip should show paused state"
    );
    assert!(
        buffer_contains(&buf, "F10 next"),
        "debug strip should show step labels when paused"
    );
}

#[test]
fn snapshot_command_input_80x24() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    app.state = ConnectionState::Connected {
        version: 9,
        plugin_count: 1,
    };
    app.command_input.open();
    app.command_input.field.value = "say hello".into();

    let buf = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&buf, "Minecraft Command"),
        "command popup title should show"
    );
    assert!(
        buffer_contains(&buf, "say hello"),
        "command input value should show"
    );
    assert!(
        buffer_contains(&buf, "Enter Send"),
        "footer should advertise enter to send"
    );
}

#[test]
fn snapshot_evaluate_busy_80x24() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    app.state = ConnectionState::Connected {
        version: 9,
        plugin_count: 1,
    };
    app.stopped = true;
    app.stopped_thread_id = Some(0);
    app.evaluate_input.open();
    app.evaluate_input.busy = true;
    app.evaluate_input.pending_expression = "1+1".into();

    let buf = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&buf, "Evaluate"),
        "evaluate popup title should show"
    );
    assert!(
        buffer_contains(&buf, "Busy"),
        "evaluate popup should show busy state"
    );
}

#[test]
fn snapshot_evaluate_history_80x24() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    app.state = ConnectionState::Connected {
        version: 9,
        plugin_count: 1,
    };
    app.stopped = true;
    app.stopped_thread_id = Some(0);
    app.evaluate_input.open();
    app.evaluate_input.history.push_back(EvaluateEntry {
        expression: "pos.x".into(),
        success: true,
        detail: "1.5".into(),
    });
    app.evaluate_input.history.push_back(EvaluateEntry {
        expression: "player".into(),
        success: false,
        detail: "not found".into(),
    });

    let buf = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&buf, "pos.x"),
        "successful evaluate expression should show"
    );
    assert!(
        buffer_contains(&buf, "player"),
        "failed evaluate expression should show"
    );
    assert!(
        buffer_contains(&buf, "not found"),
        "evaluate error message should show"
    );
}

#[test]
fn snapshot_compact_controls_50x15() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    app.state = ConnectionState::Connected {
        version: 9,
        plugin_count: 1,
    };
    app.stopped = true;
    app.stopped_thread_id = Some(0);

    let buf = render_buffer(&mut app, 50, 15);
    assert!(
        buffer_contains(&buf, "paused"),
        "compact debug strip should show paused state"
    );
    assert!(
        buffer_contains(&buf, "F5▶"),
        "compact debug strip should use compact continue symbol"
    );
    assert!(
        buffer_contains(&buf, "SF11↑"),
        "compact debug strip should use compact step-out symbol"
    );
}

// ── Phase 4 snapshots: search / filter / reset ─────────────────────────

#[test]
fn snapshot_search_active_80x24() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    app.state = ConnectionState::Connected {
        version: 9,
        plugin_count: 1,
    };
    app.search_input.open = true;
    app.search_input.field.value = "spawn".into();

    let buf = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&buf, "Search Event Log"),
        "search popup title should show"
    );
    assert!(
        buffer_contains(&buf, "spawn"),
        "search input value should render"
    );
    assert!(
        buffer_contains(&buf, "Enter Search"),
        "footer should advertise enter to search"
    );
}

#[test]
fn snapshot_filter_active_80x24() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    app.state = ConnectionState::Connected {
        version: 9,
        plugin_count: 1,
    };
    app.filter_popup.open = true;
    app.filter_popup.selected = 2; // Stopped

    let buf = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&buf, "Filters"),
        "filter popup title should show"
    );
    assert!(
        buffer_contains(&buf, "Event kinds"),
        "filter popup should list event kinds"
    );
    assert!(
        buffer_contains(&buf, "Log levels"),
        "filter popup should list log levels"
    );
    assert!(
        buffer_contains(&buf, "STOPPED"),
        "selected kind label should render"
    );
    assert!(
        buffer_contains(&buf, "Enter Toggle"),
        "footer should advertise toggle action"
    );
}

#[test]
fn snapshot_filtered_log_80x24() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    app.state = ConnectionState::Connected {
        version: 9,
        plugin_count: 1,
    };
    app.add_log("system alpha");
    app.add_log("system beta");
    app.add_log("system gamma");

    app.log_filter.search = "beta".into();
    app.rebuild_filtered_log_indices();

    let buf = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&buf, "beta"),
        "filtered log should show the matching row"
    );
    assert!(
        !buffer_contains(&buf, "alpha"),
        "filtered log should hide non-matching rows"
    );
    assert!(
        !buffer_contains(&buf, "gamma"),
        "filtered log should hide non-matching rows"
    );
    assert!(
        buffer_contains(&buf, "1/3"),
        "header or title should show visible over total count"
    );
}

#[test]
fn snapshot_reset_restores_full_view_80x24() {
    let (ctrl, rx) = SessionController::new();
    let mut app = make_app(ctrl, rx);
    app.state = ConnectionState::Connected {
        version: 9,
        plugin_count: 1,
    };
    app.add_log("alpha");
    app.add_log("beta");

    app.log_filter.search = "beta".into();
    app.log_filter.kinds.system = false;
    app.rebuild_filtered_log_indices();
    assert_eq!(app.filtered_log_indices.len(), 0);

    app.reset_filters();
    let buf = render_buffer(&mut app, 80, 24);
    assert!(
        buffer_contains(&buf, "alpha"),
        "reset should restore alpha row visibility"
    );
    assert!(
        buffer_contains(&buf, "beta"),
        "reset should restore beta row visibility"
    );
    assert!(
        buffer_contains(&buf, "2 ev"),
        "header should show the restored visible count"
    );
}
