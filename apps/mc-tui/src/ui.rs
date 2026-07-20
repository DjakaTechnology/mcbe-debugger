use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Cell, List, ListItem, ListState, Paragraph, Row, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Sparkline, Table, Wrap,
};
use ratatui::Frame;

use mc_protocol::events::LogLevel;

use crate::app::{
    App, ConnectionState, ErrorPopup, FieldState, FilterPopup, Focus, HelpPopup, LogKind, Mode,
    PluginSelection, Tab, MAX_EVAL_HISTORY, SIDEBAR_WIDTH, WIDE_WIDTH,
};

// ── High-contrast default theme ───────────────────────────────────────

pub const DEFAULT_THEME_NAME: &str = "High Contrast";

struct Theme {
    bg: Color,
    surface: Color,
    border: Color,
    border_focused: Color,
    text: Color,
    text_dim: Color,
    text_invert: Color,
    accent: Color,
    accent_warn: Color,
    accent_err: Color,
    accent_ok: Color,
}

const THEME: Theme = Theme {
    // Do not inherit the terminal profile: High Contrast is deterministic on
    // both light and dark terminals.
    bg: Color::Rgb(10, 10, 14),
    surface: Color::Rgb(42, 42, 50),
    border: Color::Rgb(80, 80, 90),
    border_focused: Color::Cyan,
    text: Color::Rgb(220, 220, 230),
    text_dim: Color::Rgb(140, 140, 150),
    text_invert: Color::Rgb(28, 28, 32),
    accent: Color::Cyan,
    accent_warn: Color::Yellow,
    accent_err: Color::Red,
    accent_ok: Color::Green,
};

fn kind_color(kind: LogKind) -> Color {
    match kind {
        LogKind::System => Color::Rgb(200, 200, 210),
        LogKind::Protocol => Color::Rgb(100, 200, 255),
        LogKind::Stopped => Color::Rgb(255, 200, 80),
        LogKind::Thread => Color::Rgb(120, 160, 255),
        LogKind::Print => Color::Rgb(120, 220, 120),
        LogKind::Notification => Color::Rgb(220, 120, 220),
        LogKind::Stat => Color::Rgb(100, 200, 255),
        LogKind::ProfilerCapture => Color::Rgb(255, 120, 120),
        LogKind::Schema => Color::Rgb(120, 160, 255),
        LogKind::Terminated => Color::Rgb(255, 100, 100),
        LogKind::Unknown => Color::Rgb(140, 140, 150),
    }
}

fn status_symbol(state: &ConnectionState) -> &'static str {
    match state {
        ConnectionState::Idle => "●",
        ConnectionState::Pending => "◐",
        ConnectionState::Connected { .. } => "✓",
        ConnectionState::Disconnected => "✕",
    }
}

fn status_color(state: &ConnectionState) -> Color {
    match state {
        ConnectionState::Idle => THEME.accent,
        ConnectionState::Pending => THEME.accent_warn,
        ConnectionState::Connected { .. } => THEME.accent_ok,
        ConnectionState::Disconnected => THEME.accent_err,
    }
}

fn status_text(state: &ConnectionState, compact: bool) -> String {
    match state {
        ConnectionState::Idle => "idle".into(),
        ConnectionState::Pending => "pending".into(),
        ConnectionState::Connected { version, .. } => {
            if compact {
                format!("v{version}")
            } else {
                format!("connected v{version}")
            }
        }
        ConnectionState::Disconnected => {
            if compact {
                "off".into()
            } else {
                "disconnected".into()
            }
        }
    }
}

fn block(title: &str, focused: bool) -> Block<'_> {
    let mut b = Block::default()
        .borders(Borders::ALL)
        .border_style(if focused {
            Style::default().fg(THEME.border_focused)
        } else {
            Style::default().fg(THEME.border)
        })
        .style(Style::default().bg(THEME.bg))
        .title(Span::styled(
            title,
            Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
        ))
        .title_alignment(Alignment::Left);
    if focused {
        b = b.border_type(ratatui::widgets::BorderType::Thick);
    }
    b
}

fn compact_block(title: &str, focused: bool) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(if focused {
            Style::default().fg(THEME.border_focused)
        } else {
            Style::default().fg(THEME.border)
        })
        .style(Style::default().bg(THEME.bg))
        .title(Span::styled(
            title,
            Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
        ))
}

// ── Layout entry point ────────────────────────────────────────────────

/// Render the full TUI shell.
pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    // Establish an opaque root canvas before any layout or overlay is drawn.
    frame.render_widget(Block::default().style(Style::default().bg(THEME.bg)), area);
    app.set_last_known_size(area.width, area.height);

    let compact = app.compact();
    let sidebar_visible = app.is_sidebar_visible();

    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    render_header(frame, app, root[0], compact);

    let body = root[1];
    let wide = area.width >= WIDE_WIDTH;

    if wide {
        let h = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(SIDEBAR_WIDTH), Constraint::Min(0)])
            .split(body);
        render_sidebar(frame, app, h[0], compact);
        render_main(frame, app, h[1], compact);
    } else if sidebar_visible {
        // Non-wide sidebar is a true overlay: render main across the full body,
        // then clear and paint the sidebar on top of the left portion.
        render_main(frame, app, body, compact);
        let sidebar_w = if body.width < 50 {
            body.width.saturating_sub(4).max(20)
        } else {
            SIDEBAR_WIDTH
        };
        let sidebar_area = Rect {
            x: body.x,
            y: body.y,
            width: sidebar_w.min(body.width),
            height: body.height,
        };
        frame.render_widget(
            Block::default().style(Style::default().bg(THEME.bg)),
            sidebar_area,
        );
        render_sidebar(frame, app, sidebar_area, compact);
    } else {
        render_main(frame, app, body, compact);
    }

    render_footer(frame, app, root[2], compact);

    // Modals take priority over everything else. Errors are deliberately the
    // top layer so they can be dismissed back to the selector or editor.
    if let Some(ref error) = app.error_popup {
        render_error_popup(frame, area, error, compact);
    } else if let Some(ref sel) = app.plugin_selection {
        render_plugin_popup(frame, area, sel, compact);
    } else if app.help_popup == HelpPopup::Visible {
        render_help_popup(frame, area, compact);
    } else if app.search_input.open {
        render_search_popup(frame, app, area, compact);
    } else if app.filter_popup.open {
        render_filter_popup(frame, app, area, compact);
    } else if app.command_input.open {
        render_command_popup(frame, app, area, compact);
    } else if app.evaluate_input.open {
        render_evaluate_popup(frame, app, area, compact);
    }
}

// ── Header ────────────────────────────────────────────────────────────

fn render_header(frame: &mut Frame, app: &App, area: Rect, compact: bool) {
    let title = if compact || area.width < 70 {
        "MC Dbg"
    } else {
        "Minecraft Debugger"
    };

    let status = status_text(&app.state, compact);
    let symbol = status_symbol(&app.state);
    let status_fg = status_color(&app.state);

    let total = app.event_log.len();
    let visible = app.filtered_log_indices.len();
    let proto = match app.state {
        ConnectionState::Connected { version, .. } => format!("v{version}"),
        _ => "-".into(),
    };

    let left = Span::styled(
        format!(" {title} "),
        Style::default()
            .fg(THEME.text)
            .bg(THEME.surface)
            .add_modifier(Modifier::BOLD),
    );
    let status_span = Span::styled(
        format!(" {symbol} {status} "),
        Style::default().fg(status_fg).bg(THEME.surface),
    );
    let count_text = if visible == total || app.log_filter.is_identity() {
        format!(" {visible} ev ")
    } else {
        format!(" {visible}/{total} ev ")
    };
    let count_span = Span::styled(
        count_text,
        if app.is_filtered() {
            Style::default().fg(THEME.accent_warn).bg(THEME.surface)
        } else {
            Style::default().fg(THEME.text_dim).bg(THEME.surface)
        },
    );
    let proto_span = Span::styled(
        format!(" proto {proto} "),
        Style::default().fg(THEME.text_dim).bg(THEME.surface),
    );

    let filler = Span::styled(
        " ".repeat(area.width as usize),
        Style::default().bg(THEME.surface),
    );

    let line = Line::from(vec![left, status_span, count_span, proto_span, filler]);
    let paragraph = Paragraph::new(line).style(Style::default().bg(THEME.surface));
    frame.render_widget(paragraph, area);
}

// ── Footer ────────────────────────────────────────────────────────────

fn render_footer(frame: &mut Frame, app: &App, area: Rect, compact: bool) {
    let text = if app.error_popup.is_some() {
        if compact {
            "Esc/Ent Dismiss | q Quit"
        } else {
            " Esc/Enter Dismiss | q Quit "
        }
    } else if app.plugin_selection.is_some() {
        if compact {
            "↑↓ Sel | Ent Ok | Esc Can | q Quit"
        } else {
            " ↑↓ Select | Enter Confirm | Esc Cancel | q Quit "
        }
    } else if app.help_popup == HelpPopup::Visible {
        if compact {
            "Esc Close | q Quit"
        } else {
            " Esc Close | q Quit "
        }
    } else if app.search_input.open {
        if compact {
            "Ent Search | Esc Close"
        } else {
            " Enter Search | Esc Close "
        }
    } else if app.filter_popup.open {
        if compact {
            "↑↓ Sel | Spc/Ent Tog | Esc Close | r Reset"
        } else {
            " ↑/↓ Select | Space/Enter Toggle | Esc Close | r Reset "
        }
    } else if app.command_input.open {
        if compact {
            "Ent Send | Esc Close | ↑↓ Hist"
        } else {
            " Enter Send | Esc Close | ↑/↓ History "
        }
    } else if app.evaluate_input.open {
        if compact {
            "Ent Eval | Esc Close"
        } else {
            " Enter Evaluate | Esc Close "
        }
    } else {
        let idle_or_disconnected = matches!(
            app.state,
            ConnectionState::Idle | ConnectionState::Disconnected
        );
        match app.focus {
            Focus::Main if idle_or_disconnected && app.tab == Tab::Log && !compact => {
                " Tab Focus | l/c | / Search | f Filter | ↑/k ↓/j Scroll | q Quit | h Help "
            }
            Focus::Main if idle_or_disconnected && app.tab == Tab::Stats && !compact => {
                if app.stats_selected_category.as_deref() == Some("scripting")
                    && !app.stats.subscriber_addon_ids().is_empty()
                {
                    " Tab | l/c | ←/→ Cat | [/] Addon | r Clear stats | 1 Log | 2 Stats | S Sidebar | q Quit | h Help "
                } else {
                    " Tab | l/c | ←/→ Cat | r Clear stats | 1 Log | 2 Stats | S Sidebar | q Quit | h Help "
                }
            }
            Focus::Main if idle_or_disconnected && compact && app.tab == Tab::Stats => {
                if app.stats_selected_category.as_deref() == Some("scripting")
                    && !app.stats.subscriber_addon_ids().is_empty()
                {
                    "Tab|l/c|←→Cat|[/]Add|rClr|1/2|S Side|h Help|q Quit"
                } else {
                    "Tab|l/c|←→Cat|rClr|1/2|S Side|h Help|q Quit"
                }
            }
            Focus::Main if idle_or_disconnected && compact => {
                "Tab | l/c | rClr | 1/2 | S Side | q Quit"
            }
            Focus::Main if app.tab == Tab::Log && !compact => {
                " Tab Focus | : Cmd | e Eval | c Clear | / Search | f Filter | r Reset | q Quit "
            }
            Focus::Main if app.tab == Tab::Log => "Tab Foc | :Cmd eEval | cClr | / f r q Quit",
            Focus::Main if app.tab == Tab::Stats && !compact => {
                if app.stats_selected_category.as_deref() == Some("client")
                    && app.stats.client_ids().len() > 1
                {
                    " Tab | ←/→ Cat | [/] Client | ↑/↓ Scroll | r Clear stats | q Quit | h Help "
                } else if app.stats_selected_category.as_deref() == Some("scripting")
                    && !app.stats.subscriber_addon_ids().is_empty()
                {
                    " Tab | ←/→ Cat | [/] Addon | ↑/↓ Scroll | r Clear stats | q Quit | h Help "
                } else {
                    " Tab | ←/→ Cat | ↑/↓ Scroll | r Clear stats | q Quit | h Help "
                }
            }
            Focus::Main if app.tab == Tab::Stats => {
                if app.stats_selected_category.as_deref() == Some("client")
                    && app.stats.client_ids().len() > 1
                {
                    "Tab|←→Cat|[/]Cl|↑↓Scr|rClr|h Help|q Quit"
                } else if app.stats_selected_category.as_deref() == Some("scripting")
                    && !app.stats.subscriber_addon_ids().is_empty()
                {
                    "Tab|←→Cat|[/]Add|↑↓Scr|rClr|h Help|q Quit"
                } else {
                    "Tab|←→Cat|↑↓Scr|rClr|h Help|q Quit"
                }
            }
            Focus::Main => "Tab Foc | 1Log 2Sts | S Side | q Quit",
            _ if !compact => {
                " Tab/Shift+Tab Focus | Enter Action | Esc Close Sidebar | q Quit | h Help "
            }
            _ => "Tab Foc | Ent Act | Esc Cls | q Quit",
        }
    };

    let paragraph = Paragraph::new(Span::styled(
        text,
        Style::default().fg(THEME.text_dim).bg(THEME.surface),
    ))
    .alignment(Alignment::Center);
    frame.render_widget(paragraph, area);
}

// ── Sidebar ───────────────────────────────────────────────────────────

fn render_sidebar(frame: &mut Frame, app: &App, area: Rect, compact: bool) {
    let focused = app.focus.is_sidebar();
    let title = if compact { "Conn" } else { " Connection " };
    let block = compact_block(title, focused);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width < 3 || inner.height < 3 {
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // mode selector
            // Host is hidden in Listen mode so it does not steal vertical space.
            if app.sidebar.mode == Mode::Connect {
                Constraint::Length(1)
            } else {
                Constraint::Length(0)
            }, // host (or empty in listen)
            Constraint::Length(1), // port
            Constraint::Length(1), // advanced toggle
            if app.sidebar.advanced_open {
                Constraint::Length(1)
            } else {
                Constraint::Length(0)
            }, // target uuid (advanced)
            if app.sidebar.advanced_open {
                Constraint::Length(1)
            } else {
                Constraint::Length(0)
            }, // passcode (advanced)
            Constraint::Length(1), // primary action
            Constraint::Length(1), // spacer
            Constraint::Min(0),    // summary
        ])
        .split(inner);

    render_mode_selector(frame, app, rows[0], compact);

    if app.sidebar.mode == Mode::Connect {
        render_field(
            frame,
            app,
            Focus::SidebarHost,
            "Host",
            &app.sidebar.host,
            rows[1],
            false,
            compact,
        );
    }

    render_field(
        frame,
        app,
        Focus::SidebarPort,
        "Port",
        &app.sidebar.port,
        rows[2],
        false,
        compact,
    );

    render_advanced_toggle(frame, app, rows[3], compact);

    if app.sidebar.advanced_open {
        render_field(
            frame,
            app,
            Focus::SidebarTargetUuid,
            "UUID",
            &app.sidebar.target_uuid,
            rows[4],
            false,
            compact,
        );
        render_field(
            frame,
            app,
            Focus::SidebarPasscode,
            "Pass",
            &app.sidebar.passcode,
            rows[5],
            true,
            compact,
        );
    }

    render_primary_action(frame, app, rows[6], compact);
    render_summary(frame, app, rows[8], compact);
}

fn render_mode_selector(frame: &mut Frame, app: &App, area: Rect, compact: bool) {
    let listen_label = if compact { "[L]" } else { "[Listen]" };
    let connect_label = if compact { "[C]" } else { "[Connect]" };

    let listen_fg = if app.sidebar.mode == Mode::Listen {
        THEME.accent
    } else {
        THEME.text_dim
    };
    let connect_fg = if app.sidebar.mode == Mode::Connect {
        THEME.accent
    } else {
        THEME.text_dim
    };

    let listen = Span::styled(
        listen_label,
        if app.focus == Focus::SidebarModeListen {
            Style::default()
                .fg(listen_fg)
                .bg(THEME.surface)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().fg(listen_fg).add_modifier(Modifier::BOLD)
        },
    );
    let connect = Span::styled(
        connect_label,
        if app.focus == Focus::SidebarModeConnect {
            Style::default()
                .fg(connect_fg)
                .bg(THEME.surface)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().fg(connect_fg).add_modifier(Modifier::BOLD)
        },
    );

    let line = Line::from(vec![listen, Span::raw(" "), connect]);
    frame.render_widget(Paragraph::new(line), area);
}

#[allow(clippy::too_many_arguments)]
fn render_field(
    frame: &mut Frame,
    app: &App,
    focus: Focus,
    label: &str,
    field: &FieldState,
    area: Rect,
    masked: bool,
    _compact: bool,
) {
    let focused = app.focus == focus;
    let display = if masked {
        "•".repeat(field.value.chars().count())
    } else {
        field.value.clone()
    };

    let label_span = Span::styled(
        format!("{label}: "),
        Style::default().fg(if focused {
            THEME.accent
        } else {
            THEME.text_dim
        }),
    );
    let value_span = Span::styled(
        display,
        if focused {
            Style::default().fg(THEME.text).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(THEME.text)
        },
    );

    let mut line = Line::from(vec![label_span, value_span]);
    if focused {
        // Show a trailing cursor indicator when focused.
        line.spans
            .push(Span::styled("█", Style::default().fg(THEME.border_focused)));
    }

    let block = Block::default()
        .borders(Borders::NONE)
        .style(Style::default().bg(if focused { THEME.surface } else { THEME.bg }));
    frame.render_widget(Paragraph::new(line).block(block), area);
}

fn render_advanced_toggle(frame: &mut Frame, app: &App, area: Rect, compact: bool) {
    let focused = app.focus == Focus::SidebarAdvanced;
    let marker = if app.sidebar.advanced_open {
        "▾"
    } else {
        "▸"
    };
    let text = if compact {
        format!("{marker} Adv")
    } else {
        format!("{marker} Advanced")
    };
    let span = Span::styled(
        text,
        if focused {
            Style::default()
                .fg(THEME.accent)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().fg(THEME.text_dim)
        },
    );
    frame.render_widget(Paragraph::new(Line::from(span)), area);
}

fn render_primary_action(frame: &mut Frame, app: &App, area: Rect, compact: bool) {
    let focused = app.focus == Focus::SidebarPrimary;
    let (label, fg) = match app.state {
        ConnectionState::Idle => match app.sidebar.mode {
            Mode::Listen => ("Listen", THEME.accent_ok),
            Mode::Connect => ("Connect", THEME.accent_ok),
        },
        ConnectionState::Pending => ("Cancel", THEME.accent_warn),
        ConnectionState::Connected { .. } => ("Disconnect", THEME.accent_err),
        ConnectionState::Disconnected if app.auto_relisten_pending => {
            ("Cancel retry", THEME.accent_warn)
        }
        ConnectionState::Disconnected => match app.sidebar.mode {
            Mode::Listen => ("Listen", THEME.accent_ok),
            Mode::Connect => ("Connect", THEME.accent_ok),
        },
    };

    let text = if compact {
        format!("[{label}]")
    } else {
        format!("  [{label}]  ")
    };

    let span = Span::styled(
        text,
        if focused {
            Style::default()
                .fg(fg)
                .bg(THEME.surface)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(fg).add_modifier(Modifier::BOLD)
        },
    );
    frame.render_widget(
        Paragraph::new(Line::from(span)).alignment(Alignment::Center),
        area,
    );
}

fn render_summary(frame: &mut Frame, app: &App, area: Rect, _compact: bool) {
    let mut lines = vec![];

    let state_label = match app.state {
        ConnectionState::Idle => "Idle",
        ConnectionState::Pending => "Pending...",
        ConnectionState::Connected { .. } => "Connected",
        ConnectionState::Disconnected => "Disconnected",
    };
    lines.push(Line::from(vec![
        Span::styled("State: ", Style::default().fg(THEME.text_dim)),
        Span::styled(state_label, Style::default().fg(THEME.text)),
    ]));

    if let ConnectionState::Connected {
        version,
        plugin_count,
    } = app.state
    {
        lines.push(Line::from(vec![
            Span::styled("Proto: ", Style::default().fg(THEME.text_dim)),
            Span::styled(format!("v{version}"), Style::default().fg(THEME.text)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Plugins: ", Style::default().fg(THEME.text_dim)),
            Span::styled(plugin_count.to_string(), Style::default().fg(THEME.text)),
        ]));
    }

    if app.handshake.is_some() {
        let host = app.sidebar.host.value.clone();
        lines.push(Line::from(vec![
            Span::styled("Host: ", Style::default().fg(THEME.text_dim)),
            Span::styled(host, Style::default().fg(THEME.text)),
        ]));
        match app.sidebar.port_u16() {
            Some(port) => lines.push(Line::from(vec![
                Span::styled("Port: ", Style::default().fg(THEME.text_dim)),
                Span::styled(port.to_string(), Style::default().fg(THEME.text)),
            ])),
            None => lines.push(Line::from(vec![
                Span::styled("Port: ", Style::default().fg(THEME.text_dim)),
                Span::styled("invalid", Style::default().fg(THEME.accent_err)),
            ])),
        }
    }

    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::default().fg(THEME.text)),
        area,
    );
}

// ── Main pane ─────────────────────────────────────────────────────────

fn render_main(frame: &mut Frame, app: &mut App, area: Rect, compact: bool) {
    let focused = app.focus == Focus::Main;
    let main_block = block("", focused);
    let inner = main_block.inner(area);
    frame.render_widget(main_block, area);

    if inner.width < 4 || inner.height < 4 {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // debug controls
            Constraint::Length(1), // tabs
            Constraint::Min(0),    // content
        ])
        .split(inner);

    render_debug_controls(frame, app, chunks[0], compact);
    render_tabs(frame, app, chunks[1], compact);

    match app.tab {
        Tab::Log => render_event_log(frame, app, chunks[2], compact),
        Tab::Stats => render_stats_dashboard(frame, app, chunks[2], compact),
    }
}

fn render_debug_controls(frame: &mut Frame, app: &App, area: Rect, compact: bool) {
    let connected = matches!(app.state, ConnectionState::Connected { .. });
    let paused = connected && app.stopped;
    let running = connected && !app.stopped;
    let busy = app.is_debug_busy();

    let can_pause = app.can_pause();
    let can_continue = app.can_continue();
    let can_step = app.can_step();

    let status_text = if busy {
        if compact {
            "busy"
        } else {
            " busy "
        }
    } else if paused {
        if compact {
            "paused"
        } else {
            " paused "
        }
    } else if running {
        if compact {
            "running"
        } else {
            " running "
        }
    } else {
        ""
    };
    let status_fg = if busy || paused {
        THEME.accent_warn
    } else if running {
        THEME.accent_ok
    } else {
        THEME.text_dim
    };

    let status = Span::styled(
        status_text,
        Style::default().fg(status_fg).add_modifier(Modifier::BOLD),
    );

    let style_for = |enabled: bool| {
        if enabled {
            Style::default().fg(THEME.text)
        } else {
            Style::default()
                .fg(THEME.text_dim)
                .add_modifier(Modifier::DIM)
        }
    };

    let cont = Span::styled(
        if compact { "F5▶" } else { "F5 continue" },
        style_for(can_continue),
    );
    let pause = Span::styled(
        if compact { "F6⏸" } else { "F6 pause" },
        style_for(can_pause),
    );
    let next = Span::styled(
        if compact { "F10⤼" } else { "F10 next" },
        style_for(can_step),
    );
    let step_in = Span::styled(
        if compact { "F11↓" } else { "F11 step in" },
        style_for(can_step),
    );
    let step_out = Span::styled(
        if compact {
            "SF11↑"
        } else {
            "Shift+F11 step out"
        },
        style_for(can_step),
    );

    let spans = vec![
        status,
        Span::raw(" "),
        cont,
        Span::raw("  "),
        pause,
        Span::raw("  "),
        next,
        Span::raw("  "),
        step_in,
        Span::raw("  "),
        step_out,
    ];

    frame.render_widget(
        Paragraph::new(Line::from(spans)).alignment(Alignment::Center),
        area,
    );
}

fn render_tabs(frame: &mut Frame, app: &App, area: Rect, compact: bool) {
    let log_label = if compact { "[1 Log]" } else { " [1] Log " };
    let stats_label = if compact { "[2 Stats]" } else { " [2] Stats " };

    let log_style = if app.tab == Tab::Log {
        Style::default()
            .fg(THEME.text_invert)
            .bg(THEME.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(THEME.text_dim)
    };
    let stats_style = if app.tab == Tab::Stats {
        Style::default()
            .fg(THEME.text_invert)
            .bg(THEME.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(THEME.text_dim)
    };

    let line = Line::from(vec![
        Span::styled(log_label, log_style),
        Span::raw(" "),
        Span::styled(stats_label, stats_style),
    ]);
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Left), area);
}

fn render_event_log(frame: &mut Frame, app: &App, area: Rect, compact: bool) {
    let focused = app.focus == Focus::Main;
    let filtered_total = app.filtered_log_indices.len();
    let raw_total = app.event_log.len();
    let visible_height = area.height.saturating_sub(2).max(1) as usize;
    let max_offset = filtered_total.saturating_sub(visible_height);

    // Effective viewport offset is computed locally from the filtered view,
    // selection, and follow state so the visible window always tracks intent.
    let selected = app
        .log_state
        .selected
        .filter(|&s| filtered_total > 0 && s < filtered_total);
    let offset = if filtered_total == 0 {
        0
    } else if app.log_state.follow || selected.is_none() {
        max_offset
    } else if let Some(sel) = selected {
        let mut off = app.log_state.offset.min(max_offset);
        if sel < off {
            off = sel;
        } else if sel >= off.saturating_add(visible_height) {
            off = sel.saturating_sub(visible_height).saturating_add(1);
        }
        off.min(max_offset)
    } else {
        0
    };

    let title = if filtered_total == 0 {
        if compact {
            "Log 0/0".into()
        } else {
            " Event Log 0/0 ".into()
        }
    } else {
        let pos = selected.map(|s| s + 1).unwrap_or(0);
        if compact {
            let filtered = if app.is_filtered() { "*" } else { "" };
            format!("Log {pos}/{filtered_total}{filtered}")
        } else {
            let filtered = if app.is_filtered() { " *" } else { "" };
            let follow = if app.log_state.follow { " ↓" } else { "" };
            format!(" Event Log {pos}/{filtered_total}{filtered}{follow} ")
        }
    };

    let block = block(&title, focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width < 3 || inner.height < 1 {
        return;
    }

    let rows: Vec<Row> = app
        .filtered_log_indices
        .iter()
        .skip(offset)
        .take(visible_height)
        .enumerate()
        .map(|(idx, &absolute)| {
            let filtered_idx = offset + idx;
            let selected = app.log_state.selected == Some(filtered_idx);
            let style = if selected {
                Style::default()
                    .bg(THEME.surface)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let entry = &app.event_log[absolute];
            let seq = format!("{:>3}", filtered_idx + 1);
            let time = entry.timestamp.clone();
            let sym = entry.kind.symbol().to_string();
            let kind = if compact { "" } else { entry.kind.label() };

            let mut cells = vec![
                Cell::from(Span::styled(seq, Style::default().fg(THEME.text_dim))),
                Cell::from(Span::styled(time, Style::default().fg(THEME.text_dim))),
                Cell::from(Span::styled(
                    sym,
                    Style::default().fg(kind_color(entry.kind)),
                )),
            ];
            if !compact {
                cells.push(Cell::from(Span::styled(
                    kind,
                    Style::default().fg(kind_color(entry.kind)),
                )));
            }
            cells.push(Cell::from(Span::styled(
                entry.message.clone(),
                Style::default().fg(THEME.text),
            )));

            Row::new(cells).style(style)
        })
        .collect();

    let constraints = if compact {
        vec![
            Constraint::Length(4), // seq
            Constraint::Length(8), // time
            Constraint::Length(2), // symbol
            Constraint::Min(0),    // message
        ]
    } else {
        vec![
            Constraint::Length(4),  // seq
            Constraint::Length(8),  // time
            Constraint::Length(2),  // symbol
            Constraint::Length(10), // kind
            Constraint::Min(0),     // message
        ]
    };

    let table = Table::new(rows, constraints)
        .column_spacing(1)
        .style(Style::default().fg(THEME.text));
    frame.render_widget(table, inner);

    // Vertical scrollbar when the filtered view overflows.
    if filtered_total > visible_height {
        let mut state = ScrollbarState::new(filtered_total.saturating_sub(visible_height))
            .position(offset.min(filtered_total.saturating_sub(visible_height)));
        frame.render_stateful_widget(
            Scrollbar::default()
                .orientation(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("↑"))
                .end_symbol(Some("↓")),
            inner.inner(Margin {
                horizontal: 0,
                vertical: 0,
            }),
            &mut state,
        );
    }

    // If everything is filtered out, show a grounded empty-state hint.
    if raw_total > 0 && filtered_total == 0 {
        let hint = if compact {
            "all entries hidden"
        } else {
            "All entries hidden by filter"
        };
        let paragraph = Paragraph::new(hint)
            .alignment(Alignment::Center)
            .style(Style::default().fg(THEME.text_dim));
        frame.render_widget(paragraph, inner);
    }
}

fn render_stats_dashboard(frame: &mut Frame, app: &mut App, area: Rect, compact: bool) {
    app.reconcile_stats_selection();

    let focused = app.focus == Focus::Main;
    let main_block = block("", focused);
    let inner = main_block.inner(area);
    frame.render_widget(main_block, area);

    if inner.width < 4 || inner.height < 4 {
        render_stats_empty(frame, area);
        return;
    }

    let categories = app.stats.categories();
    if categories.is_empty() {
        render_stats_empty(frame, area);
        return;
    }

    let selected_key = app.stats_selected_category.as_deref();
    let clients = app.stats.client_ids();
    let addons = app.stats.subscriber_addon_ids();
    let show_client = selected_key == Some("client") && clients.len() > 1;
    let show_addon = selected_key == Some("scripting") && !addons.is_empty();

    // Header: category selector and optional client selector.
    let header_constraints = if show_client {
        let client_width = client_selector_width(&clients, compact);
        vec![Constraint::Min(0), Constraint::Length(client_width)]
    } else if show_addon {
        let addon_width = addon_selector_width(&addons, compact);
        vec![Constraint::Min(0), Constraint::Length(addon_width)]
    } else {
        vec![Constraint::Percentage(100)]
    };
    let header = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(header_constraints)
        .split(Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        });

    render_category_selector(frame, app, &categories, header[0], compact);
    if show_client {
        render_client_selector(frame, app, &clients, header[1], compact);
    } else if show_addon {
        render_addon_selector(frame, app, &addons, header[1], compact);
    }

    let content = Rect {
        x: inner.x,
        y: inner.y + 1,
        width: inner.width,
        height: inner.height - 1,
    };
    if content.height < 3 {
        return;
    }

    let Some(selected_cat) = selected_key.and_then(|key| categories.iter().find(|c| c.key == key))
    else {
        render_stats_empty(frame, area);
        return;
    };

    let table_mode = selected_cat.key == "memory";
    // Keep the accumulated model immutable, but give the client selector real
    // filtering semantics.  The selector is intentionally a view filter, not
    // a second stats collection, so changing clients never loses history.
    let filtered_groups: Vec<crate::stats::StatGroup> = selected_cat
        .groups
        .iter()
        .map(|group| {
            let group = filter_client_group(group, app.stats_selected_client.as_deref());
            filter_addon_group(&group, app.stats_selected_addon.as_deref())
        })
        .collect();
    let groups: Vec<&crate::stats::StatGroup> = filtered_groups.iter().collect();
    if groups.is_empty() {
        let title = format!(" {} ", selected_cat.label);
        let block = compact_block(&title, false);
        let inside = block.inner(content);
        frame.render_widget(block, content);
        frame.render_widget(
            Paragraph::new("No groups in this category.")
                .alignment(Alignment::Center)
                .style(Style::default().fg(THEME.text_dim)),
            inside,
        );
        return;
    }

    // WIDE_WIDTH describes the terminal's layout intent, not the width left
    // after the persistent sidebar.  At 120 columns the main pane is only
    // about 90 columns wide, but it still has room for two useful cards.
    const MIN_STAT_CARD_WIDTH: u16 = 32;
    let wide =
        app.last_known_width() >= WIDE_WIDTH && inner.width > MIN_STAT_CARD_WIDTH.saturating_mul(2);
    let cards: Vec<StatCard> = groups
        .iter()
        .map(|g| StatCard::new(g, compact, table_mode))
        .collect();

    let columns: u16 = if wide { 2 } else { 1 };
    render_card_grid(frame, app, &cards, content, columns, compact, table_mode);
}

fn filter_client_group(
    group: &crate::stats::StatGroup,
    selected_client: Option<&str>,
) -> crate::stats::StatGroup {
    if group.name != "client_stats" {
        return group.clone();
    }

    let series = selected_client
        .map(|client| {
            group
                .series
                .iter()
                .filter(|series| {
                    crate::stats::get_client_id(&series.path).as_deref() == Some(client)
                })
                .cloned()
                .collect()
        })
        .unwrap_or_else(|| group.series.clone());

    crate::stats::StatGroup {
        name: group.name.clone(),
        series,
    }
}

fn filter_addon_group(
    group: &crate::stats::StatGroup,
    selected_addon: Option<&str>,
) -> crate::stats::StatGroup {
    if group.name != "fine_grained_subscribers" {
        return group.clone();
    }
    let series = group
        .series
        .iter()
        .filter(|series| {
            selected_addon
                .map(|addon| {
                    crate::stats::get_subscriber_parts(&series.path)
                        .map(|(id, _)| id == addon)
                        .unwrap_or(false)
                })
                .unwrap_or(true)
        })
        .map(|series| {
            let mut series = series.clone();
            if let Some((_, event)) = crate::stats::get_subscriber_parts(&series.path) {
                series.name = event.clone();
                // `render_series_row` uses `short_name`, so retain a
                // synthetic group prefix while preserving the full event.
                series.path = format!("subscriber.{event}");
            }
            series
        })
        .collect();
    crate::stats::StatGroup {
        name: group.name.clone(),
        series,
    }
}

/// Minimal description of a group card used for virtual scrolling/layout.
struct StatCard<'a> {
    group: &'a crate::stats::StatGroup,
    height: u16,
}

struct CardRenderOptions {
    content_offset: usize,
    compact: bool,
    table_mode: bool,
}

impl<'a> StatCard<'a> {
    fn new(group: &'a crate::stats::StatGroup, compact: bool, table_mode: bool) -> Self {
        let height = card_height(group, compact, table_mode);
        Self { group, height }
    }
}

fn card_height(group: &crate::stats::StatGroup, compact: bool, table_mode: bool) -> u16 {
    let _ = compact; // row density is the same; sparklines are hidden, not collapsed
    if table_mode && group.name == "dynamic_property_values" {
        // Table header + one row per property, plus borders.
        (3 + group.series.len().max(1) as u16).max(4)
    } else {
        // Borders + title padding + one row per series.
        (3 + group.series.len().max(1) as u16).max(3)
    }
}

fn render_stats_empty(frame: &mut Frame, area: Rect) {
    let block = block(" Stats ", false);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let text = "Waiting for Stat2 data...\nConnect to a debuggee and emit stats to see them here.";
    frame.render_widget(
        Paragraph::new(text)
            .alignment(Alignment::Center)
            .style(Style::default().fg(THEME.text_dim)),
        inner,
    );
}

fn render_category_selector(
    frame: &mut Frame,
    app: &App,
    categories: &[crate::stats::StatCategory],
    area: Rect,
    compact: bool,
) {
    if area.width < 3 || categories.is_empty() {
        return;
    }

    let use_compact_labels = compact || area.width < 40;
    let mut spans = vec![];
    for cat in categories {
        let selected = app.stats_selected_category.as_deref() == Some(cat.key);
        let label = if use_compact_labels {
            format!("{}{}", cat.icon, compact_category_label(cat.key))
        } else {
            format!(" {} {} ", cat.icon, cat.label)
        };
        let style = if selected {
            Style::default()
                .fg(THEME.text_invert)
                .bg(THEME.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(THEME.text_dim)
        };
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)).alignment(Alignment::Left),
        area,
    );
}

fn compact_category_label(key: &str) -> &'static str {
    match key {
        "server-performance" => "Perf",
        "memory" => "Mem",
        "scripting" => "Scr",
        "client" => "Cli",
        _ => "Misc",
    }
}

fn client_selector_width(clients: &[String], compact: bool) -> u16 {
    if compact {
        (2 + clients.len() * 4 + 1).clamp(8, 30) as u16
    } else {
        let ids_width: usize = clients.iter().map(|id| id.len() + 2).sum();
        (8 + ids_width + 2).clamp(12, 50) as u16
    }
}

fn render_client_selector(
    frame: &mut Frame,
    app: &App,
    clients: &[String],
    area: Rect,
    compact: bool,
) {
    if area.width < 3 || clients.is_empty() {
        return;
    }

    let prefix = if compact { "C:" } else { "Client: " };
    let mut spans = vec![Span::styled(prefix, Style::default().fg(THEME.text_dim))];

    for (idx, id) in clients.iter().enumerate() {
        let selected = app.stats_selected_client.as_deref() == Some(id.as_str());
        let style = if selected {
            Style::default()
                .fg(THEME.text_invert)
                .bg(THEME.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(THEME.text_dim)
        };
        let label = if compact {
            format!("[{idx}]")
        } else {
            format!(" {id} ")
        };
        spans.push(Span::styled(label, style));
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)).alignment(Alignment::Right),
        area,
    );
}

fn addon_selector_width(addons: &[String], compact: bool) -> u16 {
    if compact {
        (4 + addons.len() * 4).clamp(8, 34) as u16
    } else {
        (8 + addons.iter().map(|id| id.len() + 2).sum::<usize>()).clamp(14, 56) as u16
    }
}

fn render_addon_selector(
    frame: &mut Frame,
    app: &App,
    addons: &[String],
    area: Rect,
    compact: bool,
) {
    if area.width < 3 {
        return;
    }
    let prefix = if compact { "A:" } else { "Addon: " };
    let mut spans = vec![Span::styled(prefix, Style::default().fg(THEME.text_dim))];
    let mut entries = vec![("All".to_string(), app.stats_selected_addon.is_none())];
    entries.extend(
        addons
            .iter()
            .map(|id| (id.clone(), app.stats_selected_addon.as_deref() == Some(id))),
    );
    for (idx, (id, selected)) in entries.iter().enumerate() {
        let label = if compact {
            format!(
                "[{}]",
                if idx == 0 {
                    "*".into()
                } else {
                    truncate(id, 2)
                }
            )
        } else {
            format!(" {} ", truncate(id, 18))
        };
        let style = if *selected {
            Style::default()
                .fg(THEME.text_invert)
                .bg(THEME.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(THEME.text_dim)
        };
        spans.push(Span::styled(label, style));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).alignment(Alignment::Right),
        area,
    );
}

fn render_card_grid(
    frame: &mut Frame,
    app: &mut App,
    cards: &[StatCard<'_>],
    area: Rect,
    columns: u16,
    compact: bool,
    table_mode: bool,
) {
    if cards.is_empty() || area.height < 3 {
        return;
    }

    let col_count = columns as usize;
    // Pair cards into rows; each row is as tall as its tallest card.  The
    // viewport is a visual scroll, rather than an all-or-nothing row: this is
    // important for property tables that are taller than the terminal.
    let row_heights: Vec<u16> = cards
        .chunks(col_count)
        .map(|chunk| chunk.iter().map(|c| c.height).max().unwrap_or(0))
        .collect();

    let total_height: u16 = row_heights.iter().sum();
    let max_offset = usize::from(total_height.saturating_sub(area.height));
    let offset = app.stats_scroll_offset.min(max_offset);
    app.stats_scroll_offset = offset;

    let mut row_y = area.y;
    let end_y = area.y + area.height;
    for (row_idx, row_height) in row_heights.iter().enumerate() {
        let row_top = row_y;
        let row_bottom = row_y + *row_height;
        row_y = row_bottom;
        let shifted_top = area.y as i32 + (row_top - area.y) as i32 - offset as i32;
        let visible_top = shifted_top.max(area.y as i32) as u16;
        let visible_bottom =
            (row_bottom as i32 - offset as i32).clamp(area.y as i32, end_y as i32) as u16;
        if visible_top >= visible_bottom {
            continue;
        }
        let row_area = Rect {
            x: area.x,
            y: visible_top,
            width: area.width,
            height: visible_bottom - visible_top,
        };
        let content_offset = usize::try_from(visible_top as i32 - shifted_top).unwrap_or(0);
        render_card_row(
            frame,
            cards,
            row_idx,
            col_count,
            row_area,
            CardRenderOptions {
                content_offset,
                compact,
                table_mode,
            },
        );
    }
}

fn render_card_row(
    frame: &mut Frame,
    cards: &[StatCard<'_>],
    row_idx: usize,
    col_count: usize,
    area: Rect,
    options: CardRenderOptions,
) {
    if col_count == 1 {
        let card = &cards[row_idx];
        render_group_card(
            frame,
            card.group,
            area,
            options.content_offset,
            options.compact,
            options.table_mode,
        );
        return;
    }

    let constraints: Vec<Constraint> = (0..col_count)
        .map(|_| Constraint::Ratio(1, col_count as u32))
        .collect();
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(area);

    for col_idx in 0..col_count {
        let card_idx = row_idx * col_count + col_idx;
        if let Some(card) = cards.get(card_idx) {
            // A paired row is as tall as its largest card.  Do not let a
            // shorter card paint into the taller card's clipped slice.
            let visible_height = area.height.min(
                card.height
                    .saturating_sub(u16::try_from(options.content_offset).unwrap_or(u16::MAX)),
            );
            if visible_height == 0 {
                continue;
            }
            let card_area = Rect {
                height: visible_height,
                ..cols[col_idx]
            };
            render_group_card(
                frame,
                card.group,
                card_area,
                options.content_offset,
                options.compact,
                options.table_mode,
            );
        }
    }
}

fn render_group_card(
    frame: &mut Frame,
    group: &crate::stats::StatGroup,
    area: Rect,
    content_offset: usize,
    compact: bool,
    table_mode: bool,
) {
    if area.width < 4 || area.height < 3 {
        return;
    }

    if table_mode && group.name == "dynamic_property_values" {
        let title = format!(" {} ", group.name);
        let block = compact_block(&title, false);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.width < 2 || inner.height < 1 {
            return;
        }
        render_dynamic_property_table(frame, group, inner, content_offset);
        return;
    }

    // The grid clips a tall card by passing only its visible slice here.  A
    // normal card has one border row before its series, so the visual offset
    // is one row ahead of the series offset once its top has scrolled away.
    // Keep the top/bottom borders honest instead of redrawing a false title at
    // the viewport edge.
    let title = format!(" {} ", group.name);
    let card_height = card_height(group, compact, false);
    let full_top = area.y as i32 - content_offset as i32;
    let full_bottom = full_top + i32::from(card_height);
    let viewport_bottom = i32::from(area.y) + i32::from(area.height);
    let show_top = content_offset == 0;
    let show_bottom = full_bottom <= viewport_bottom;
    let mut borders = Borders::LEFT | Borders::RIGHT;
    if show_top {
        borders.insert(Borders::TOP);
    }
    if show_bottom {
        borders.insert(Borders::BOTTOM);
    }
    let block = compact_block(&title, false).borders(borders);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width < 2 || inner.height < 1 {
        return;
    }

    let series = &group.series;
    let series_offset = content_offset.saturating_sub(1);
    let visible = inner
        .height
        .min(series.len().saturating_sub(series_offset) as u16) as usize;
    for (i, s) in series.iter().skip(series_offset).take(visible).enumerate() {
        let row_area = Rect {
            x: inner.x,
            y: inner.y + i as u16,
            width: inner.width,
            height: 1,
        };
        render_series_row(frame, s, &group.name, row_area, compact);
    }
}

fn render_dynamic_property_table(
    frame: &mut Frame,
    group: &crate::stats::StatGroup,
    area: Rect,
    content_offset: usize,
) {
    if area.width < 6 || area.height < 3 {
        return;
    }

    let header = Row::new(vec!["Property", "Value"]).style(
        Style::default()
            .fg(THEME.accent)
            .add_modifier(Modifier::BOLD),
    );
    let rows: Vec<Row> = group
        .series
        .iter()
        // The header is pinned inside the card viewport; the scroll offset
        // therefore advances property rows directly.
        .skip(content_offset)
        .take(area.height.saturating_sub(1) as usize)
        .map(|s| {
            let prop = crate::stats::short_name(&s.path);
            let value = crate::stats::format_group_value(&group.name, s.values.last().copied());
            let half = (area.width / 2).max(8);
            Row::new(vec![
                Cell::from(Span::styled(
                    truncate(&prop, half as usize),
                    Style::default().fg(THEME.text),
                )),
                Cell::from(Span::styled(value, Style::default().fg(THEME.accent))),
            ])
        })
        .collect();

    let widths = [Constraint::Percentage(60), Constraint::Percentage(40)];
    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::NONE))
        .column_spacing(1);
    frame.render_widget(table, area);
}

fn render_series_row(
    frame: &mut Frame,
    series: &crate::stats::StatSeries,
    group_name: &str,
    area: Rect,
    compact: bool,
) {
    let label = crate::stats::short_name(&series.path);
    let value = crate::stats::format_group_value(group_name, series.values.last().copied());

    if compact {
        let value_len = display_width(&value).min(10) as u16;
        let label_width = area.width.saturating_sub(value_len + 1);
        let line = Line::from(vec![
            Span::styled(
                truncate(&label, label_width as usize),
                Style::default().fg(THEME.text),
            ),
            Span::raw(" "),
            Span::styled(
                value,
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

    let value_len = display_width(&value).min(10) as u16;
    let min_label = 6u16;
    let min_spark = 4u16;
    let available = area.width.saturating_sub(value_len + 1);
    if available < min_label + min_spark {
        render_series_row(frame, series, group_name, area, true);
        return;
    }

    let label_width = (available / 2).clamp(min_label, available - min_spark);
    let spark_width = available - label_width;

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(label_width),
            Constraint::Length(value_len),
            Constraint::Length(spark_width),
        ])
        .split(area);

    frame.render_widget(
        Paragraph::new(Span::styled(
            truncate(&label, label_width as usize),
            Style::default().fg(THEME.text),
        )),
        chunks[0],
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            value,
            Style::default()
                .fg(THEME.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .alignment(Alignment::Right),
        chunks[1],
    );

    let data = sparkline_data(series, group_name);
    let sparkline = Sparkline::default()
        .data(&data)
        .style(Style::default().fg(THEME.accent));
    frame.render_widget(sparkline, chunks[2]);
}

fn sparkline_data(series: &crate::stats::StatSeries, group_name: &str) -> Vec<u64> {
    let scaled = crate::stats::scale_series_for_display(series, group_name);
    scaled
        .values
        .iter()
        .map(|v| {
            let scaled = *v * 1000.0;
            if scaled < 0.0 {
                0
            } else if scaled > u64::MAX as f64 {
                u64::MAX
            } else {
                scaled as u64
            }
        })
        .collect()
}

fn truncate(s: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if s.chars().count() <= max_width {
        s.to_string()
    } else {
        s.chars().take(max_width).collect()
    }
}

fn display_width(s: &str) -> usize {
    s.chars().count()
}

// ── Plugin popup ──────────────────────────────────────────────────────

fn render_error_popup(frame: &mut Frame, area: Rect, error: &ErrorPopup, compact: bool) {
    let popup_area = if compact || area.width < 60 {
        Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        }
    } else {
        let width = 58u16.min(area.width.saturating_sub(4));
        let height = 7u16.min(area.height.saturating_sub(4));
        Rect {
            x: area.x + area.width.saturating_sub(width) / 2,
            y: area.y + area.height.saturating_sub(height) / 2,
            width,
            height,
        }
    };

    frame.render_widget(
        Block::default().style(Style::default().bg(THEME.bg)),
        popup_area,
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Thick)
        .border_style(Style::default().fg(THEME.accent_err))
        .title(Span::styled(
            format!(" ✕ {} ", error.title),
            Style::default()
                .fg(THEME.accent_err)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);
    if inner.width < 4 || inner.height < 2 {
        return;
    }
    let hint = if compact {
        "Esc/Enter dismiss  q quit"
    } else {
        "Press Esc or Enter to dismiss  ·  q quit"
    };
    let text = Text::from(vec![
        Line::from(Span::styled(
            &error.message,
            Style::default().fg(THEME.text),
        )),
        Line::from(""),
        Line::from(Span::styled(hint, Style::default().fg(THEME.text_dim))),
    ]);
    frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), inner);
}

fn render_plugin_popup(frame: &mut Frame, area: Rect, sel: &PluginSelection, compact: bool) {
    let popup_area = plugin_popup_area(area, sel.plugins.len(), compact);

    frame.render_widget(
        Block::default().style(Style::default().bg(THEME.bg)),
        popup_area,
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Thick)
        .border_style(Style::default().fg(THEME.border_focused))
        .title(Span::styled(
            " Select Target Plugin ",
            Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let visible_height = inner.height as usize;
    let total = sel.plugins.len();
    let max_offset = total.saturating_sub(visible_height);

    // Keep the selected item inside the visible window.
    let mut scroll_offset = sel.scroll_offset.min(max_offset);
    if sel.selected < scroll_offset {
        scroll_offset = sel.selected;
    } else if sel.selected >= scroll_offset.saturating_add(visible_height) {
        scroll_offset = sel
            .selected
            .saturating_sub(visible_height)
            .saturating_add(1);
    }
    let scroll_offset = scroll_offset.min(max_offset);

    let items: Vec<ListItem> = sel
        .plugins
        .iter()
        .skip(scroll_offset)
        .take(visible_height)
        .enumerate()
        .map(|(idx, plugin)| {
            let absolute = scroll_offset + idx;
            let selected = absolute == sel.selected;
            let name_style = if selected {
                Style::default()
                    .fg(THEME.text_invert)
                    .bg(THEME.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(THEME.text)
            };
            let uuid_style = if selected {
                Style::default().fg(THEME.text_invert).bg(THEME.accent)
            } else {
                Style::default().fg(THEME.text_dim)
            };

            let name = Span::styled(plugin.name.clone(), name_style);
            let uuid = Span::styled(format!("  {}", plugin.module_uuid), uuid_style);
            ListItem::new(Line::from(vec![name, uuid]))
        })
        .collect();

    let list = List::new(items)
        .highlight_style(
            Style::default()
                .fg(THEME.accent)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    let mut state = ListState::default();
    state.select(Some(sel.selected.saturating_sub(scroll_offset)));
    frame.render_stateful_widget(list, inner, &mut state);

    if total > visible_height {
        let scrollbar_area = inner.inner(Margin {
            horizontal: 0,
            vertical: 0,
        });
        let mut state =
            ScrollbarState::new(total.saturating_sub(visible_height)).position(scroll_offset);
        frame.render_stateful_widget(
            Scrollbar::default()
                .orientation(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("↑"))
                .end_symbol(Some("↓")),
            scrollbar_area,
            &mut state,
        );
    }
}

fn plugin_popup_area(area: Rect, plugin_count: usize, compact: bool) -> Rect {
    if compact || area.width < 60 {
        let margin = 1u16;
        return Rect {
            x: area.x + margin,
            y: area.y + margin,
            width: area.width.saturating_sub(margin * 2),
            height: area.height.saturating_sub(margin * 2),
        };
    }

    let width = area.width.min(70);
    let height = ((plugin_count as u16) + 4).clamp(8, area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;

    Rect {
        x: area.x + x,
        y: area.y + y,
        width,
        height,
    }
}

// ── Help popup ────────────────────────────────────────────────────────

fn render_help_popup(frame: &mut Frame, area: Rect, compact: bool) {
    let popup_area = if compact || area.width < 60 {
        let margin = 1u16;
        Rect {
            x: area.x + margin,
            y: area.y + margin,
            width: area.width.saturating_sub(margin * 2),
            height: area.height.saturating_sub(margin * 2),
        }
    } else {
        let width = 54u16.min(area.width.saturating_sub(4));
        let height = 19u16.min(area.height.saturating_sub(4));
        let x = (area.width.saturating_sub(width)) / 2;
        let y = (area.height.saturating_sub(height)) / 2;
        Rect {
            x: area.x + x,
            y: area.y + y,
            width,
            height,
        }
    };

    frame.render_widget(
        Block::default().style(Style::default().bg(THEME.bg)),
        popup_area,
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Thick)
        .border_style(Style::default().fg(THEME.border_focused))
        .title(Span::styled(
            " Help ",
            Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
        ));

    if compact {
        let inner = block.inner(popup_area);
        frame.render_widget(block, popup_area);
        let text = "q quit  h help\nTab focus  S sidebar\n1 log  2 stats\nl listen  c connect\n↑/↓ scroll  PgUp/PgDn page\nStats: ←→ category  ↑↓ cards\nr clear stats  Esc close";
        frame.render_widget(
            Paragraph::new(text)
                .style(Style::default().fg(THEME.text))
                .wrap(Wrap { trim: true }),
            inner,
        );
        return;
    }

    let text = Text::from(vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "q",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" quit  "),
            Span::styled(
                "h",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" help"),
        ]),
        Line::from(vec![
            Span::styled(
                "Tab",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" cycle focus  "),
            Span::styled(
                "S",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" sidebar"),
        ]),
        Line::from(vec![
            Span::styled(
                "1",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" log tab  "),
            Span::styled(
                "2",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" stats tab"),
        ]),
        Line::from(vec![
            Span::styled(
                "l",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" listen  "),
            Span::styled(
                "c",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" connect"),
        ]),
        Line::from(vec![
            Span::styled(
                "x",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" cancel/disconnect"),
        ]),
        Line::from(vec![
            Span::styled(
                "↑/k",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" up  "),
            Span::styled(
                "↓/j",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" down"),
        ]),
        Line::from(vec![
            Span::styled(
                "PgUp/PgDn",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" page scroll"),
        ]),
        Line::from(vec![
            Span::styled(
                "g",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" top  "),
            Span::styled(
                "G",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" bottom"),
        ]),
        Line::from(vec![
            Span::styled(
                "Enter",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" confirm  "),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" cancel/close"),
        ]),
        Line::from(vec![
            Span::styled(
                "/",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" search  "),
            Span::styled(
                "f",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" filter  "),
            Span::styled(
                "r",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" reset"),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Stats tab",
            Style::default()
                .fg(THEME.accent_warn)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![
            Span::styled(
                "←/→",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" category  "),
            Span::styled(
                "[/]",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" client (when multiple are available)"),
        ]),
        Line::from(vec![
            Span::styled(
                "↑/↓",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" scroll cards  "),
            Span::styled(
                "r",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" clear stats"),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Debug keys",
            Style::default()
                .fg(THEME.accent_warn)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![
            Span::styled(
                "Space",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" pause/continue  "),
            Span::styled(
                "F5",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" continue"),
        ]),
        Line::from(vec![
            Span::styled(
                "F6",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" pause  "),
            Span::styled(
                "F10",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" next"),
        ]),
        Line::from(vec![
            Span::styled(
                "F11",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" step in  "),
            Span::styled(
                "SF11",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" step out"),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                ":",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" Minecraft command  "),
            Span::styled(
                "e",
                Style::default()
                    .fg(THEME.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" evaluate (stopped)"),
        ]),
    ]);

    let paragraph = Paragraph::new(text).block(block);
    frame.render_widget(paragraph, popup_area);
}

// ── Command/evaluate popup helpers ─────────────────────────────────────

fn input_popup_area(area: Rect, width: u16, height: u16, compact: bool) -> Rect {
    if compact || area.width < width + 4 {
        let margin = 1u16;
        Rect {
            x: area.x + margin,
            y: area.y + margin,
            width: area.width.saturating_sub(margin * 2),
            height: area.height.saturating_sub(margin * 2).max(height),
        }
    } else {
        let w = width.min(area.width.saturating_sub(4));
        let h = height.min(area.height.saturating_sub(4));
        let x = (area.width.saturating_sub(w)) / 2;
        let y = (area.height.saturating_sub(h)) / 2;
        Rect {
            x: area.x + x,
            y: area.y + y,
            width: w,
            height: h,
        }
    }
}

fn render_field_with_cursor<'a>(field: &'a FieldState, label: &'a str) -> Text<'a> {
    let before = &field.value[..field.cursor];
    let after = &field.value[field.cursor..];
    let line = Line::from(vec![
        Span::styled(format!("{label}: "), Style::default().fg(THEME.text_dim)),
        Span::styled(before, Style::default().fg(THEME.text)),
        Span::styled(
            if after.is_empty() {
                "█".to_string()
            } else {
                // Highlight the character under the cursor.
                after
                    .chars()
                    .next()
                    .map_or_else(|| "█".to_string(), |c| c.to_string())
            },
            Style::default().fg(THEME.border_focused),
        ),
        Span::styled(
            after.chars().skip(1).collect::<String>(),
            Style::default().fg(THEME.text),
        ),
    ]);
    Text::from(vec![line])
}

fn render_command_popup(frame: &mut Frame, app: &App, area: Rect, compact: bool) {
    let popup_area = input_popup_area(area, 60, 5, compact);
    frame.render_widget(
        Block::default().style(Style::default().bg(THEME.bg)),
        popup_area,
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Thick)
        .border_style(Style::default().fg(THEME.border_focused))
        .title(Span::styled(
            " Minecraft Command ",
            Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if inner.width < 3 || inner.height < 2 {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    let input_text = render_field_with_cursor(&app.command_input.field, "/");
    frame.render_widget(
        Paragraph::new(input_text).wrap(Wrap { trim: false }),
        chunks[0],
    );

    if let Some(ref err) = app.command_input.error {
        let error_line = Line::from(Span::styled(
            err.clone(),
            Style::default().fg(THEME.accent_err),
        ));
        frame.render_widget(
            Paragraph::new(Text::from(vec![error_line])).wrap(Wrap { trim: false }),
            chunks[1],
        );
    }
}

fn render_evaluate_popup(frame: &mut Frame, app: &App, area: Rect, compact: bool) {
    let popup_area = input_popup_area(area, 70, 14, compact);
    frame.render_widget(
        Block::default().style(Style::default().bg(THEME.bg)),
        popup_area,
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Thick)
        .border_style(Style::default().fg(THEME.border_focused))
        .title(Span::styled(
            " Evaluate ",
            Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if inner.width < 3 || inner.height < 4 {
        return;
    }

    let mut lines = vec![];

    // Busy / selecting indicator.
    if app.evaluate_input.busy {
        lines.push(Line::from(Span::styled(
            "Busy — waiting for result...",
            Style::default().fg(THEME.accent_warn),
        )));
    }

    if let Some(ref err) = app.evaluate_input.error {
        lines.push(Line::from(Span::styled(
            err.clone(),
            Style::default().fg(THEME.accent_err),
        )));
    }

    let input_text = render_field_with_cursor(&app.evaluate_input.field, "expr");
    lines.push(input_text.lines.first().cloned().unwrap_or_default());

    // History.
    if !app.evaluate_input.history.is_empty() {
        lines.push(Line::from(Span::styled(
            "— history —",
            Style::default().fg(THEME.text_dim),
        )));
        for entry in app.evaluate_input.history.iter().take(MAX_EVAL_HISTORY) {
            let marker = if entry.success { "✓" } else { "✕" };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{marker} "),
                    Style::default().fg(if entry.success {
                        THEME.accent_ok
                    } else {
                        THEME.accent_err
                    }),
                ),
                Span::styled(
                    format!("{}: ", entry.expression),
                    Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
                ),
            ]));
            for l in entry.detail.lines() {
                lines.push(Line::from(vec![Span::styled(
                    format!("    {l}"),
                    Style::default().fg(THEME.text_dim),
                )]));
            }
        }
    }

    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .scroll((0, 0)),
        inner,
    );
}

// ── Search popup ───────────────────────────────────────────────────────

fn render_search_popup(frame: &mut Frame, app: &App, area: Rect, compact: bool) {
    let popup_area = input_popup_area(area, 60, 5, compact);
    frame.render_widget(
        Block::default().style(Style::default().bg(THEME.bg)),
        popup_area,
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Thick)
        .border_style(Style::default().fg(THEME.border_focused))
        .title(Span::styled(
            " Search Event Log ",
            Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if inner.width < 3 || inner.height < 2 {
        return;
    }

    let input_text = render_field_with_cursor(&app.search_input.field, "search");
    frame.render_widget(Paragraph::new(input_text).wrap(Wrap { trim: false }), inner);
}

// ── Filter popup ───────────────────────────────────────────────────────

fn render_filter_popup(frame: &mut Frame, app: &App, area: Rect, compact: bool) {
    let popup_area = filter_popup_area(area, compact);
    frame.render_widget(
        Block::default().style(Style::default().bg(THEME.bg)),
        popup_area,
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Thick)
        .border_style(Style::default().fg(THEME.border_focused))
        .title(Span::styled(
            " Filters ",
            Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if inner.width < 3 || inner.height < 1 {
        return;
    }

    let mut lines = vec![];
    lines.push(Line::from(vec![Span::styled(
        "Event kinds",
        Style::default()
            .fg(THEME.accent)
            .add_modifier(Modifier::BOLD),
    )]));

    for (idx, kind) in LogKind::ALL.iter().enumerate() {
        let selected = app.filter_popup.selected == idx;
        let enabled = app.log_filter.kinds.get(*kind);
        let marker = if enabled { "[x]" } else { "[ ]" };
        let prefix = if selected { "▸ " } else { "  " };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{prefix}{marker} "),
                if selected {
                    Style::default().fg(THEME.accent)
                } else {
                    Style::default().fg(THEME.text_dim)
                },
            ),
            Span::styled(
                kind.label(),
                Style::default()
                    .fg(kind_color(*kind))
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    }

    lines.push(Line::from(vec![Span::styled(
        "Log levels",
        Style::default()
            .fg(THEME.accent)
            .add_modifier(Modifier::BOLD),
    )]));

    let level_rows = [
        (LogLevel::Log, "Log", 0),
        (LogLevel::Warn, "Warn", 1),
        (LogLevel::Error, "Error", 2),
    ];
    for (level, label, idx) in level_rows {
        let row = LogKind::ALL.len() + idx;
        let selected = app.filter_popup.selected == row;
        let enabled = app.log_filter.levels.get(level);
        let marker = if enabled { "[x]" } else { "[ ]" };
        let prefix = if selected { "▸ " } else { "  " };
        let color = match level {
            LogLevel::Verbose | LogLevel::Log => THEME.text,
            LogLevel::Warn => THEME.accent_warn,
            LogLevel::Error | LogLevel::Stop => THEME.accent_err,
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{prefix}{marker} "),
                if selected {
                    Style::default().fg(THEME.accent)
                } else {
                    Style::default().fg(THEME.text_dim)
                },
            ),
            Span::styled(
                label,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
        ]));
    }

    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .scroll((0, 0)),
        inner,
    );
}

fn filter_popup_area(area: Rect, compact: bool) -> Rect {
    let count = FilterPopup::row_count();
    let height = (count + 5).clamp(8, area.height.saturating_sub(4) as usize) as u16;
    if compact || area.width < 50 {
        let margin = 1u16;
        Rect {
            x: area.x + margin,
            y: area.y + margin,
            width: area.width.saturating_sub(margin * 2),
            height: area.height.saturating_sub(margin * 2).max(height),
        }
    } else {
        let width = 34u16.min(area.width.saturating_sub(4));
        let h = height.min(area.height.saturating_sub(4));
        let x = (area.width.saturating_sub(width)) / 2;
        let y = (area.height.saturating_sub(h)) / 2;
        Rect {
            x: area.x + x,
            y: area.y + y,
            width,
            height: h,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn oversized_group() -> crate::stats::StatGroup {
        crate::stats::StatGroup {
            name: "oversized".to_string(),
            series: (0..20)
                .map(|index| crate::stats::StatSeries {
                    name: format!("series-{index:02}"),
                    path: format!("oversized.series-{index:02}"),
                    ticks: vec![0],
                    values: vec![index as f64],
                })
                .collect(),
        }
    }

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .flat_map(|y| (0..buffer.area.width).map(move |x| buffer[(x, y)].symbol()))
            .collect()
    }

    #[test]
    fn oversized_ordinary_card_reaches_both_ends_when_scrolled() {
        let group = oversized_group();
        let area = Rect::new(0, 0, 80, 10);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("test terminal");

        terminal
            .draw(|frame| render_group_card(frame, &group, area, 0, false, false))
            .expect("initial render");
        let initial = buffer_text(&terminal);
        assert!(initial.contains("series-00"));
        assert!(!initial.contains("series-19"));
        assert!(initial.contains("oversized"));

        // card_height(20 series) is 23, so 13 is the grid's bottom offset for
        // this ten-row viewport.
        terminal
            .draw(|frame| render_group_card(frame, &group, area, 13, false, false))
            .expect("bottom render");
        let bottom = buffer_text(&terminal);
        assert!(!bottom.contains("series-00"));
        assert!(bottom.contains("series-19"));
        assert!(bottom.contains("─"));
    }

    #[test]
    fn subscriber_labels_hide_addon_and_preserve_full_event() {
        let group = crate::stats::StatGroup {
            name: "fine_grained_subscribers".into(),
            series: vec![crate::stats::StatSeries {
                name: "fine_grained_subscribers.demo.world.after.load".into(),
                path: "fine_grained_subscribers.demo.world.after.load".into(),
                ticks: vec![1],
                values: vec![1.0],
            }],
        };

        let filtered = filter_addon_group(&group, Some("demo"));
        assert_eq!(filtered.series.len(), 1);
        assert_eq!(
            crate::stats::short_name(&filtered.series[0].path),
            "world.after.load"
        );

        let all = filter_addon_group(&group, None);
        assert_eq!(
            crate::stats::short_name(&all.series[0].path),
            "world.after.load"
        );
    }
}

// ── Plugin popup ──────────────────────────────────────────────────────
