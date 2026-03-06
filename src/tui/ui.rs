use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Wrap},
};

use super::{App, ComposeRow, LogStreamPopup, Popup};
use crate::config::AlertsSection;
use crate::telemetry::{azure::DbMetrics, docker::{ContainerInfo, ContainerStats}};

// ── theme ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Theme {
    pub accent:       Color,
    pub border:       Color,
    pub muted:        Color,
    pub text:         Color,
    pub success:      Color,
    pub warning:      Color,
    pub danger:       Color,
    pub panel:        Color,
    pub selection_fg: Color,
    pub selection_bg: Color,
}

impl Theme {
    pub fn from_config(cfg: &crate::config::ThemeSection) -> Self {
        Self {
            accent:       parse_color(&cfg.accent,       Color::Cyan),
            border:       parse_color(&cfg.border,       Color::Blue),
            muted:        parse_color(&cfg.muted,        Color::DarkGray),
            text:         parse_color(&cfg.text,         Color::White),
            success:      parse_color(&cfg.success,      Color::Green),
            warning:      parse_color(&cfg.warning,      Color::Yellow),
            danger:       parse_color(&cfg.danger,       Color::Red),
            panel:        parse_color(&cfg.panel,        Color::Black),
            selection_fg: parse_color(&cfg.selection_fg, Color::Black),
            selection_bg: parse_color(&cfg.selection_bg, Color::Cyan),
        }
    }
}

fn parse_color(value: &str, fallback: Color) -> Color {
    let raw = value.trim();
    if let Some(hex) = raw.strip_prefix('#')
        && hex.len() == 6
        && let Ok(n) = u32::from_str_radix(hex, 16)
    {
        return Color::Rgb(((n >> 16) & 0xff) as u8, ((n >> 8) & 0xff) as u8, (n & 0xff) as u8);
    }

    match raw.to_ascii_lowercase().as_str() {
        "black"        => Color::Black,
        "red"          => Color::Red,
        "green"        => Color::Green,
        "yellow"       => Color::Yellow,
        "blue"         => Color::Blue,
        "magenta"      => Color::Magenta,
        "cyan"         => Color::Cyan,
        "gray" | "grey"           => Color::Gray,
        "darkgray" | "darkgrey"   => Color::DarkGray,
        "lightred"     => Color::LightRed,
        "lightgreen"   => Color::LightGreen,
        "lightyellow"  => Color::LightYellow,
        "lightblue"    => Color::LightBlue,
        "lightmagenta" => Color::LightMagenta,
        "lightcyan"    => Color::LightCyan,
        "white"        => Color::White,
        _              => fallback,
    }
}

// ── draw ──────────────────────────────────────────────────────────────────────

pub fn draw(
    frame: &mut Frame,
    app: &mut App,
    host: &str,
    theme: &Theme,
    alerts: &AlertsSection,
    web_port: u16,
) {
    let area = frame.area();
    let has_azure = app.snapshot.as_ref()
        .map(|s| s.azure_db.is_some() || s.azure_db_name.is_some())
        .unwrap_or(false);

    // ── outer border ──────────────────────────────────────────────────────────
    let title = Line::from(vec![
        Span::styled(" ✦ wisp ", Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)),
        Span::styled("│", Style::default().fg(theme.muted)),
        Span::styled(format!(" host {} ", host), Style::default().fg(theme.muted)),
    ]);
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .title(title);
    frame.render_widget(outer, area);

    let inner = area.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });

    // ── vertical layout ───────────────────────────────────────────────────────
    let stale     = app.is_stale();
    let alert_msg = alert_banner_text(app, alerts);
    let has_alert = alert_msg.is_some();

    let mut v_constraints: Vec<Constraint> = Vec::new();
    if stale     { v_constraints.push(Constraint::Length(1)); }
    if has_alert { v_constraints.push(Constraint::Length(1)); }
    v_constraints.push(Constraint::Min(0));
    v_constraints.push(Constraint::Length(1));

    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints(v_constraints)
        .split(inner);

    let mut vi = 0usize;

    if stale {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    " ⚠  reconnecting… ",
                    Style::default().fg(theme.selection_fg).bg(theme.warning).add_modifier(Modifier::BOLD),
                ),
                Span::styled("  host hasn't responded for 15s", Style::default().fg(theme.muted)),
            ])),
            vert[vi],
        );
        vi += 1;
    }

    if let Some(msg) = alert_msg {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    " ⚑  alert ",
                    Style::default().fg(Color::Black).bg(theme.danger).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("  {msg}"), Style::default().fg(theme.danger)),
            ])),
            vert[vi],
        );
        vi += 1;
    }

    let body_area   = vert[vi];     vi += 1;
    let footer_area = vert[vi];

    // ── body: table [| azure sidebar] ────────────────────────────────────────
    if has_azure {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(0), Constraint::Length(30)])
            .split(body_area);
        draw_table(frame, app, cols[0], theme, alerts);
        draw_azure_sidebar(frame, app, cols[1], theme);
    } else {
        draw_table(frame, app, body_area, theme, alerts);
    }

    draw_footer(frame, app, footer_area, theme, web_port);

    // Overlays — streaming popup takes priority over regular popup.
    if let Some(stream) = &app.log_stream {
        draw_stream_popup(frame, stream, theme);
    } else if let Some(popup) = &app.popup {
        draw_popup(frame, popup, theme);
    }
}

// ── alert banner helper ───────────────────────────────────────────────────────

fn alert_banner_text(app: &App, alerts: &AlertsSection) -> Option<String> {
    let snap = app.snapshot.as_ref()?;
    let stats_map: std::collections::HashMap<&str, &ContainerStats> =
        snap.stats.iter().map(|s| (s.name.as_str(), s)).collect();

    let mut cpu_over: Vec<&str> = Vec::new();
    let mut mem_over: Vec<&str> = Vec::new();

    for c in &snap.containers {
        if let Some(st) = stats_map.get(c.names.as_str()) {
            if parse_pct(&st.cpu_perc).map(|p| p >= alerts.cpu_crit).unwrap_or(false) {
                cpu_over.push(&c.names);
            }
            if parse_pct(&st.mem_perc).map(|p| p >= alerts.mem_crit).unwrap_or(false) {
                mem_over.push(&c.names);
            }
        }
    }

    if cpu_over.is_empty() && mem_over.is_empty() {
        return None;
    }

    let mut parts: Vec<String> = Vec::new();
    if !cpu_over.is_empty() {
        parts.push(format!("CPU >{:.0}%: {}", alerts.cpu_crit, cpu_over.join(", ")));
    }
    if !mem_over.is_empty() {
        parts.push(format!("MEM >{:.0}%: {}", alerts.mem_crit, mem_over.join(", ")));
    }
    Some(parts.join("  •  "))
}

// ── container table ───────────────────────────────────────────────────────────

fn draw_table(
    frame: &mut Frame,
    app: &mut App,
    area: ratatui::layout::Rect,
    theme: &Theme,
    alerts: &AlertsSection,
) {
    let table_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.muted))
        .title(Span::styled(" containers ", Style::default().fg(theme.muted)));
    frame.render_widget(table_block, area);
    let table_area = area.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });

    let header = Row::new(
        ["CONTAINER", "STATE", "HEALTH", "CPU%", "MEM", "NET I/O", "STATUS"].map(|h| {
            Cell::from(h).style(
                Style::default()
                    .fg(theme.text)
                    .bg(theme.muted)
                    .add_modifier(Modifier::BOLD),
            )
        }),
    )
    .bottom_margin(1);

    let rows: Vec<Row> = match &app.snapshot {
        None => vec![Row::new(vec![Cell::from(
            Span::styled("  waiting for data…", Style::default().fg(theme.muted)),
        )])],
        Some(snap) => {
            let stats_map: std::collections::HashMap<&str, &ContainerStats> =
                snap.stats.iter().map(|s| (s.name.as_str(), s)).collect();

            let mut result: Vec<Row> = Vec::new();
            let mut zebra = false;

            for row in &app.compose_rows {
                match row {
                    ComposeRow::Header(project) => {
                        result.push(
                            Row::new(vec![
                                Cell::from(Line::from(vec![
                                    Span::styled(
                                        format!(" ◆ {project}"),
                                        Style::default()
                                            .fg(theme.accent)
                                            .add_modifier(Modifier::BOLD),
                                    ),
                                ])),
                                Cell::from(""), Cell::from(""), Cell::from(""),
                                Cell::from(""), Cell::from(""), Cell::from(""),
                            ])
                            .style(Style::default().bg(theme.panel)),
                        );
                        zebra = false;
                    }
                    ComposeRow::Container(idx) => {
                        if let Some(c) = snap.containers.get(*idx) {
                            let st = stats_map.get(c.names.as_str()).copied();
                            result.push(build_row(c, st, zebra, theme, alerts));
                            zebra = !zebra;
                        }
                    }
                }
            }

            if result.is_empty() {
                vec![Row::new(vec![Cell::from(
                    Span::styled("  no containers", Style::default().fg(theme.muted)),
                )])]
            } else {
                result
            }
        }
    };

    let table = Table::new(
        rows,
        [
            Constraint::Length(26),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(18),
            Constraint::Length(18),
            Constraint::Min(0),
        ],
    )
    .header(header)
    .row_highlight_style(
        Style::default()
            .fg(theme.selection_fg)
            .bg(theme.selection_bg)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("❯ ");

    frame.render_stateful_widget(table, table_area, &mut app.table_state);
}

fn build_row<'a>(
    c: &'a ContainerInfo,
    stats: Option<&'a ContainerStats>,
    zebra: bool,
    theme: &Theme,
    alerts: &AlertsSection,
) -> Row<'a> {
    let state_style = if c.state == "running" {
        Style::default().fg(theme.success)
    } else {
        Style::default().fg(theme.danger)
    };

    let (cpu_str, cpu_style, mem_str, mem_style, net_str) = match stats {
        None => (
            "–".to_string(), Style::default().fg(theme.muted),
            "–".to_string(), Style::default().fg(theme.muted),
            "–".to_string(),
        ),
        Some(s) => {
            let cpu_style = threshold_style(parse_pct(&s.cpu_perc), alerts.cpu_warn, alerts.cpu_crit, theme);
            let mem_style = threshold_style(parse_pct(&s.mem_perc), alerts.mem_warn, alerts.mem_crit, theme);
            (s.cpu_perc.clone(), cpu_style, s.mem_usage.clone(), mem_style, s.net_io.clone())
        }
    };

    let status_style = if c.status.contains("unhealthy") {
        Style::default().fg(theme.danger)
    } else if c.status.contains("healthy") {
        Style::default().fg(theme.success)
    } else {
        Style::default().fg(theme.warning)
    };

    let health = health_label(&c.status);
    let health_style = match health {
        "healthy"   => Style::default().fg(theme.success),
        "unhealthy" => Style::default().fg(theme.danger).add_modifier(Modifier::BOLD),
        "starting"  => Style::default().fg(theme.warning),
        _           => Style::default().fg(theme.muted),
    };

    let state_chip = if c.state == "running" { "● running" } else { "● stopped" };

    Row::new(vec![
        Cell::from(c.names.as_str()),
        Cell::from(state_chip).style(state_style),
        Cell::from(health).style(health_style),
        Cell::from(cpu_str).style(cpu_style),
        Cell::from(mem_str).style(mem_style),
        Cell::from(net_str),
        Cell::from(c.status.as_str()).style(status_style),
    ])
    .style(if zebra {
        Style::default().bg(Color::Reset)
    } else {
        Style::default().bg(theme.panel)
    })
}

fn health_label(status: &str) -> &'static str {
    let s = status.to_ascii_lowercase();
    if s.contains("unhealthy") {
        "unhealthy"
    } else if s.contains("healthy") {
        "healthy"
    } else if s.contains("health: starting") || s.contains("starting") {
        "starting"
    } else {
        "none"
    }
}

// ── azure sidebar ─────────────────────────────────────────────────────────────

fn draw_azure_sidebar(frame: &mut Frame, app: &App, area: ratatui::layout::Rect, theme: &Theme) {
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(theme.muted))
        .title(Span::styled(" azure db ", Style::default().fg(theme.accent)));
    frame.render_widget(block, area);

    let inner = area.inner(ratatui::layout::Margin { horizontal: 2, vertical: 1 });
    let db = app.snapshot.as_ref().and_then(|s| s.azure_db.as_ref());

    let db_name = app.snapshot.as_ref()
        .and_then(|s| s.azure_db_name.as_deref())
        .unwrap_or("(loading)");
    let db_type = app.snapshot.as_ref()
        .and_then(|s| s.azure_db_type.as_deref())
        .unwrap_or("(unknown)");

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("DB       ", Style::default().fg(theme.muted)),
            Span::styled(db_name, Style::default().fg(theme.text).add_modifier(Modifier::BOLD)),
        ])),
        inner,
    );

    let type_area = ratatui::layout::Rect {
        x: inner.x, y: inner.y.saturating_add(1), width: inner.width, height: 1,
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("TYPE     ", Style::default().fg(theme.muted)),
            Span::styled(db_type, Style::default().fg(theme.text)),
        ])),
        type_area,
    );

    if db.is_none() {
        let note_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y.saturating_add(3),
            width: inner.width,
            height: inner.height.saturating_sub(3),
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("azure warming… ", Style::default().fg(theme.muted)),
                Span::styled("az CLI token fetch can be slow", Style::default().fg(theme.muted)),
            ]))
            .wrap(Wrap { trim: false }),
            note_area,
        );
        return;
    }

    let db = db.expect("checked is_some above");
    let metrics_area = ratatui::layout::Rect {
        x: inner.x,
        y: inner.y.saturating_add(3),
        width: inner.width,
        height: inner.height.saturating_sub(3),
    };

    let rows = azure_metric_rows(db, theme);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(std::iter::repeat_n(Constraint::Length(2), rows.len()).collect::<Vec<_>>())
        .split(metrics_area);

    for (i, (label, value, style)) in rows.iter().enumerate() {
        if i >= chunks.len() { break; }
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{label:<8} "), Style::default().fg(theme.muted)),
                Span::styled(value.clone(), *style),
            ])),
            chunks[i],
        );
    }
}

fn azure_metric_rows(db: &DbMetrics, theme: &Theme) -> Vec<(String, String, Style)> {
    vec![
        ("CPU".into(),      format!("{:.1}%", db.cpu_percent),     pct_style(db.cpu_percent, theme)),
        ("MEM".into(),      format!("{:.1}%", db.memory_percent),  pct_style(db.memory_percent, theme)),
        ("STOR".into(),     format!("{:.1}%", db.storage_percent), pct_style(db.storage_percent, theme)),
        ("ACT CONN".into(), format!("{:.0}",  db.connections),     Style::default().fg(theme.text)),
    ]
}

fn pct_style(pct: f64, theme: &Theme) -> Style {
    if pct >= 80.0      { Style::default().fg(theme.danger).add_modifier(Modifier::BOLD) }
    else if pct >= 50.0 { Style::default().fg(theme.warning) }
    else                { Style::default().fg(theme.success) }
}

// ── footer ────────────────────────────────────────────────────────────────────

fn draw_footer(frame: &mut Frame, app: &App, area: ratatui::layout::Rect, theme: &Theme, web_port: u16) {
    let refresh = match app.last_updated {
        None    => "waiting…".to_string(),
        Some(t) => format!("{}s ago", t.elapsed().as_secs()),
    };

    let right = if let Some((msg, _)) = &app.pending_action {
        Line::from(vec![
            Span::styled(format!(" ⟳ {msg}"), Style::default().fg(theme.warning).add_modifier(Modifier::BOLD)),
        ])
    } else {
        let azure_loading = app.snapshot.as_ref()
            .map(|s| s.azure_db_name.is_some() && s.azure_db.is_none())
            .unwrap_or(false);

        let mut spans = vec![
            Span::styled("[q]", Style::default().fg(theme.warning)),
            Span::styled(" quit  ", Style::default().fg(theme.muted)),
            Span::styled("• ", Style::default().fg(theme.muted)),
            Span::styled("[jk/↑↓]", Style::default().fg(theme.warning)),
            Span::styled(" select  ", Style::default().fg(theme.muted)),
            Span::styled("• ", Style::default().fg(theme.muted)),
            Span::styled("[r]", Style::default().fg(theme.warning)),
            Span::styled(" restart  ", Style::default().fg(theme.muted)),
            Span::styled("[a]", Style::default().fg(theme.warning)),
            Span::styled(" start  ", Style::default().fg(theme.muted)),
            Span::styled("[x]", Style::default().fg(theme.warning)),
            Span::styled(" stop  ", Style::default().fg(theme.muted)),
            Span::styled("[enter]", Style::default().fg(theme.warning)),
            Span::styled(" inspect  ", Style::default().fg(theme.muted)),
            Span::styled("[l]", Style::default().fg(theme.warning)),
            Span::styled(" logs  ", Style::default().fg(theme.muted)),
            Span::styled("[f]", Style::default().fg(theme.warning)),
            Span::styled(" stream  ", Style::default().fg(theme.muted)),
            Span::styled("[d]", Style::default().fg(theme.warning)),
            Span::styled(" disk  ", Style::default().fg(theme.muted)),
            Span::styled("[p][p]", Style::default().fg(theme.warning)),
            Span::styled(" prune  ", Style::default().fg(theme.muted)),
            Span::styled("• ", Style::default().fg(theme.muted)),
            Span::styled(format!("[:{web_port}]"), Style::default().fg(theme.border)),
            Span::styled(" web  ", Style::default().fg(theme.muted)),
            Span::styled(refresh, Style::default().fg(theme.muted)),
        ];

        if azure_loading {
            spans.push(Span::styled("  •  ", Style::default().fg(theme.muted)));
            spans.push(Span::styled("azure pending (az cli slow)", Style::default().fg(theme.muted)));
        }

        Line::from(spans)
    };

    frame.render_widget(Paragraph::new(right), area);
}

// ── regular popup ─────────────────────────────────────────────────────────────

fn draw_popup(frame: &mut Frame, popup: &Popup, theme: &Theme) {
    let area = frame.area();

    if area.width < 24 || area.height < 8 {
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new("terminal too small\nresize to view popup")
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(theme.muted))
                        .title(Span::styled(" popup ", Style::default().fg(theme.warning))),
                )
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }

    let popup_area = centered_popup(area);

    let title_style = if popup.is_error {
        Style::default().fg(theme.danger).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.border).add_modifier(Modifier::BOLD)
    };

    frame.render_widget(Clear, popup_area);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.muted))
            .title(Span::styled(format!(" {} ", popup.title), title_style)),
        popup_area,
    );

    let inner = popup_area.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    frame.render_widget(
        Paragraph::new(popup.body.as_str())
            .scroll((popup.scroll, 0))
            .wrap(Wrap { trim: false }),
        chunks[0],
    );

    let hint = if popup.loading {
        "[mouse wheel/jk/↑↓/pgup/pgdn] scroll  [esc/enter/q] close"
    } else {
        "[mouse wheel/jk/↑↓/pgup/pgdn] scroll  [home/end] jump  [esc/enter/q] close"
    };
    frame.render_widget(
        Paragraph::new(Span::styled(hint, Style::default().fg(theme.muted))),
        chunks[1],
    );
}

// ── streaming log popup ───────────────────────────────────────────────────────

fn draw_stream_popup(frame: &mut Frame, stream: &LogStreamPopup, theme: &Theme) {
    let area = frame.area();
    if area.width < 24 || area.height < 8 { return; }

    let popup_area = centered_popup(area);

    let indicator = if stream.ended { " [ended]" } else { " [live]" };
    let indicator_style = if stream.ended {
        Style::default().fg(theme.muted)
    } else {
        Style::default().fg(theme.success)
    };

    let title_line = Line::from(vec![
        Span::styled(
            format!(" {} ", stream.title),
            Style::default().fg(theme.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(indicator, indicator_style),
    ]);

    frame.render_widget(Clear, popup_area);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.muted))
            .title(title_line),
        popup_area,
    );

    let inner = popup_area.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    frame.render_widget(
        Paragraph::new(stream.body.as_str())
            .scroll((stream.scroll, 0))
            .wrap(Wrap { trim: false }),
        chunks[0],
    );

    frame.render_widget(
        Paragraph::new(Span::styled(
            "[mouse wheel/jk/↑↓/pgup/pgdn] scroll  [home] top  [end] resume auto-scroll  [esc/q] close",
            Style::default().fg(theme.muted),
        )),
        chunks[1],
    );
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn centered_popup(area: ratatui::layout::Rect) -> ratatui::layout::Rect {
    let popup_width  = area.width.saturating_sub((area.width / 10).max(2) * 2);
    let popup_height = area.height.saturating_sub((area.height / 8).max(1) * 2);
    ratatui::layout::Rect {
        x:      area.x + (area.width.saturating_sub(popup_width)) / 2,
        y:      area.y + (area.height.saturating_sub(popup_height)) / 2,
        width:  popup_width.max(20),
        height: popup_height.max(6),
    }
}

fn parse_pct(s: &str) -> Option<f64> {
    s.trim_end_matches('%').parse().ok()
}

/// Colour a metric value based on configurable warn/crit thresholds.
fn threshold_style(pct: Option<f64>, warn: f64, crit: f64, theme: &Theme) -> Style {
    match pct {
        None                 => Style::default().fg(theme.muted),
        Some(p) if p >= crit => Style::default().fg(theme.danger).add_modifier(Modifier::BOLD),
        Some(p) if p >= warn => Style::default().fg(theme.warning),
        Some(_)              => Style::default().fg(theme.success),
    }
}
