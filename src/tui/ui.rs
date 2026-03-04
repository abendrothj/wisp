use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Wrap},
};

use super::{App, Popup};
use crate::telemetry::{azure::DbMetrics, docker::{ContainerInfo, ContainerStats}};

pub fn draw(frame: &mut Frame, app: &mut App, host: &str) {
    let area = frame.area();
    let has_azure = app
        .snapshot
        .as_ref()
        .map(|s| s.azure_db.is_some() || s.azure_db_name.is_some())
        .unwrap_or(false);

    // ── outer border ──────────────────────────────────────────────────────────
    let title = Line::from(vec![
        Span::styled(" ✦ wisp ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::styled("│", Style::default().fg(Color::DarkGray)),
        Span::styled(format!(" host {} ", host), Style::default().fg(Color::DarkGray)),
    ]);
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Blue))
        .title(title);
    frame.render_widget(outer, area);

    let inner = area.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });

    // ── vertical layout: [reconnecting banner?] | body | footer ──────────────
    let stale = app.is_stale();
    let mut v_constraints: Vec<Constraint> = vec![];
    if stale { v_constraints.push(Constraint::Length(1)); }
    v_constraints.push(Constraint::Min(0));
    v_constraints.push(Constraint::Length(1));

    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints(v_constraints)
        .split(inner);

    let (banner_area, body_area, footer_area) = if stale {
        (Some(vert[0]), vert[1], vert[2])
    } else {
        (None, vert[0], vert[1])
    };

    // ── reconnecting banner ───────────────────────────────────────────────────
    if let Some(ba) = banner_area {
        let banner = Paragraph::new(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                " ⚠  reconnecting… ",
                Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  host hasn't responded for 15s", Style::default().fg(Color::DarkGray)),
        ]));
        frame.render_widget(banner, ba);
    }

    // ── body: table [| azure sidebar] ────────────────────────────────────────
    if has_azure {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(0), Constraint::Length(30)])
            .split(body_area);
        draw_table(frame, app, cols[0]);
        draw_azure_sidebar(frame, app, cols[1]);
    } else {
        draw_table(frame, app, body_area);
    }

    draw_footer(frame, app, footer_area);

    if let Some(popup) = &app.popup {
        draw_popup(frame, popup);
    }
}

// ── container table ───────────────────────────────────────────────────────────

fn draw_table(frame: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    let table_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(" containers ", Style::default().fg(Color::DarkGray)));
    frame.render_widget(table_block, area);
    let table_area = area.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });

    let header = Row::new(["CONTAINER", "STATE", "CPU%", "MEM", "NET I/O", "STATUS"].map(|h| {
        Cell::from(h).style(
            Style::default()
                .fg(Color::White)
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
    }))
    .bottom_margin(1);

    let rows: Vec<Row> = match &app.snapshot {
        None => vec![Row::new(vec![Cell::from(
            Span::styled("  waiting for data…", Style::default().fg(Color::DarkGray)),
        )])],
        Some(snap) => {
            let stats_map: std::collections::HashMap<&str, &ContainerStats> =
                snap.stats.iter().map(|s| (s.name.as_str(), s)).collect();
            snap.containers
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let zebra = i % 2 == 0;
                    build_row(c, stats_map.get(c.names.as_str()).copied(), zebra)
                })
                .collect()
        }
    };

    let table = Table::new(
        rows,
        [
            Constraint::Length(26),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(18),
            Constraint::Length(18),
            Constraint::Min(0),
        ],
    )
    .header(header)
    .row_highlight_style(
        Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("❯ ");

    frame.render_stateful_widget(table, table_area, &mut app.table_state);
}

fn build_row<'a>(c: &'a ContainerInfo, stats: Option<&'a ContainerStats>, zebra: bool) -> Row<'a> {
    let state_style = if c.state == "running" {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::Red)
    };

    let (cpu_str, cpu_style, mem_str, net_str) = match stats {
        None => (
            "–".to_string(),
            Style::default().fg(Color::DarkGray),
            "–".to_string(),
            "–".to_string(),
        ),
        Some(s) => {
            let style = cpu_color(parse_pct(&s.cpu_perc));
            (s.cpu_perc.clone(), style, s.mem_usage.clone(), s.net_io.clone())
        }
    };

    let status_style = if c.status.contains("unhealthy") {
        Style::default().fg(Color::Red)
    } else if c.status.contains("healthy") {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::Yellow)
    };

    let state_chip = if c.state == "running" { "● running" } else { "● stopped" };

    Row::new(vec![
        Cell::from(c.names.as_str()),
        Cell::from(state_chip).style(state_style),
        Cell::from(cpu_str).style(cpu_style),
        Cell::from(mem_str),
        Cell::from(net_str),
        Cell::from(c.status.as_str()).style(status_style),
    ])
    .style(if zebra {
        Style::default().bg(Color::Reset)
    } else {
        Style::default().bg(Color::Black)
    })
}

// ── azure sidebar ─────────────────────────────────────────────────────────────

fn draw_azure_sidebar(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(" azure db ", Style::default().fg(Color::Cyan)));
    frame.render_widget(block, area);

    let inner = area.inner(ratatui::layout::Margin { horizontal: 2, vertical: 1 });
    let db = app.snapshot.as_ref().and_then(|s| s.azure_db.as_ref());

    let db_name = app
        .snapshot
        .as_ref()
        .and_then(|s| s.azure_db_name.as_deref())
        .unwrap_or("(loading)");
    let db_type = app
        .snapshot
        .as_ref()
        .and_then(|s| s.azure_db_type.as_deref())
        .unwrap_or("(unknown)");

    let name_line = Line::from(vec![
        Span::styled("DB       ", Style::default().fg(Color::DarkGray)),
        Span::styled(db_name, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
    ]);
    frame.render_widget(Paragraph::new(name_line), inner);

    let type_line = Line::from(vec![
        Span::styled("TYPE     ", Style::default().fg(Color::DarkGray)),
        Span::styled(db_type, Style::default().fg(Color::White)),
    ]);
    let type_area = ratatui::layout::Rect {
        x: inner.x,
        y: inner.y.saturating_add(1),
        width: inner.width,
        height: 1,
    };
    frame.render_widget(Paragraph::new(type_line), type_area);

    if db.is_none() {
        let note = Paragraph::new(Line::from(vec![
            Span::styled("azure warming… ", Style::default().fg(Color::DarkGray)),
            Span::styled("az CLI token fetch can be slow", Style::default().fg(Color::DarkGray)),
        ]))
        .wrap(Wrap { trim: false });
        let note_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y.saturating_add(3),
            width: inner.width,
            height: inner.height.saturating_sub(3),
        };
        frame.render_widget(note, note_area);
        return;
    }

    let db = db.expect("checked is_some above");

    let metrics_area = ratatui::layout::Rect {
        x: inner.x,
        y: inner.y.saturating_add(3),
        width: inner.width,
        height: inner.height.saturating_sub(3),
    };

    let rows = azure_metric_rows(db);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(std::iter::repeat_n(Constraint::Length(2), rows.len()).collect::<Vec<_>>())
        .split(metrics_area);

    for (i, (label, value, style)) in rows.iter().enumerate() {
        if i >= chunks.len() { break; }
        let line = Line::from(vec![
            Span::styled(format!("{label:<8} "), Style::default().fg(Color::DarkGray)),
            Span::styled(value.clone(), *style),
        ]);
        frame.render_widget(Paragraph::new(line), chunks[i]);
    }
}

fn azure_metric_rows(db: &DbMetrics) -> Vec<(String, String, Style)> {
    vec![
        ("CPU".into(),   format!("{:.1}%", db.cpu_percent),     pct_style(db.cpu_percent)),
        ("MEM".into(),   format!("{:.1}%", db.memory_percent),  pct_style(db.memory_percent)),
        ("STOR".into(),  format!("{:.1}%", db.storage_percent), pct_style(db.storage_percent)),
        ("ACT CONN".into(), format!("{:.0}",  db.connections),  Style::default().fg(Color::White)),
    ]
}

fn pct_style(pct: f64) -> Style {
    if pct >= 80.0 { Style::default().fg(Color::Red).add_modifier(Modifier::BOLD) }
    else if pct >= 50.0 { Style::default().fg(Color::Yellow) }
    else { Style::default().fg(Color::Green) }
}

// ── footer ────────────────────────────────────────────────────────────────────

fn draw_footer(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let refresh = match app.last_updated {
        None    => "waiting…".to_string(),
        Some(t) => format!("{}s ago", t.elapsed().as_secs()),
    };

    // Pending restart message supersedes normal hints for 8 s.
    let right = if let Some((msg, _)) = &app.pending_action {
        Line::from(vec![
            Span::styled(format!(" ⟳ {}", msg), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        ])
    } else {
        let azure_loading = app
            .snapshot
            .as_ref()
            .map(|s| s.azure_db_name.is_some() && s.azure_db.is_none())
            .unwrap_or(false);

        let mut spans = vec![
            Span::styled("[q]", Style::default().fg(Color::Yellow)),
            Span::styled(" quit  ", Style::default().fg(Color::DarkGray)),
            Span::styled("• ", Style::default().fg(Color::DarkGray)),
            Span::styled("[jk/↑↓]", Style::default().fg(Color::Yellow)),
            Span::styled(" select  ", Style::default().fg(Color::DarkGray)),
            Span::styled("• ", Style::default().fg(Color::DarkGray)),
            Span::styled("[r]", Style::default().fg(Color::Yellow)),
            Span::styled(" restart  ", Style::default().fg(Color::DarkGray)),
            Span::styled("[l]", Style::default().fg(Color::Yellow)),
            Span::styled(" logs  ", Style::default().fg(Color::DarkGray)),
            Span::styled("[d]", Style::default().fg(Color::Yellow)),
            Span::styled(" disk  ", Style::default().fg(Color::DarkGray)),
            Span::styled("• ", Style::default().fg(Color::DarkGray)),
            Span::styled("[:8080]", Style::default().fg(Color::Blue)),
            Span::styled(" web  ", Style::default().fg(Color::DarkGray)),
            Span::styled(refresh, Style::default().fg(Color::DarkGray)),
        ];

        if azure_loading {
            spans.push(Span::styled("  •  ", Style::default().fg(Color::DarkGray)));
            spans.push(Span::styled("azure pending (az cli slow)", Style::default().fg(Color::DarkGray)));
        }

        Line::from(spans)
    };

    frame.render_widget(Paragraph::new(right), area);
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn parse_pct(s: &str) -> Option<f64> {
    s.trim_end_matches('%').parse().ok()
}

fn cpu_color(pct: Option<f64>) -> Style {
    match pct {
        None => Style::default().fg(Color::DarkGray),
        Some(p) if p >= 20.0 => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        Some(p) if p >= 5.0  => Style::default().fg(Color::Yellow),
        Some(_) => Style::default().fg(Color::Green),
    }
}

fn draw_popup(frame: &mut Frame, popup: &Popup) {
    let area = frame.area();

    if area.width < 24 || area.height < 8 {
        frame.render_widget(Clear, area);
        let tiny = Paragraph::new("terminal too small\nresize to view logs")
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::DarkGray))
                    .title(Span::styled(" popup ", Style::default().fg(Color::Yellow))),
            )
            .wrap(Wrap { trim: false });
        frame.render_widget(tiny, area);
        return;
    }

    let popup_width = area.width.saturating_sub((area.width / 10).max(2) * 2);
    let popup_height = area.height.saturating_sub((area.height / 8).max(1) * 2);
    let popup_area = ratatui::layout::Rect {
        x: area.x + (area.width.saturating_sub(popup_width)) / 2,
        y: area.y + (area.height.saturating_sub(popup_height)) / 2,
        width: popup_width.max(20),
        height: popup_height.max(6),
    };

    let title_style = if popup.is_error {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(format!(" {} ", popup.title), title_style));

    frame.render_widget(Clear, popup_area);
    frame.render_widget(block, popup_area);
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
        Paragraph::new(Span::styled(hint, Style::default().fg(Color::DarkGray))),
        chunks[1],
    );
}
