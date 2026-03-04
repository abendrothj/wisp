use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
};

use super::App;
use crate::telemetry::{azure::DbMetrics, docker::{ContainerInfo, ContainerStats}};

pub fn draw(frame: &mut Frame, app: &mut App, host: &str) {
    let area = frame.area();
    let has_azure = app.snapshot.as_ref().and_then(|s| s.azure_db.as_ref()).is_some();

    // ── outer border ──────────────────────────────────────────────────────────
    let title = Line::from(vec![
        Span::styled(" wisp ", Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)),
        Span::styled(format!("── {} ", host), Style::default().fg(Color::DarkGray)),
    ]);
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
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
}

// ── container table ───────────────────────────────────────────────────────────

fn draw_table(frame: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    let header = Row::new(["CONTAINER", "STATE", "CPU%", "MEM", "STATUS"].map(|h| {
        Cell::from(h).style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD))
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
                .map(|c| build_row(c, stats_map.get(c.names.as_str()).copied()))
                .collect()
        }
    };

    let table = Table::new(
        rows,
        [Constraint::Length(26), Constraint::Length(10), Constraint::Length(8),
         Constraint::Length(18), Constraint::Min(0)],
    )
    .header(header)
    .row_highlight_style(
        Style::default().fg(Color::Black).bg(Color::Blue).add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("▶ ");

    frame.render_stateful_widget(table, area, &mut app.table_state);
}

fn build_row<'a>(c: &'a ContainerInfo, stats: Option<&'a ContainerStats>) -> Row<'a> {
    let state_style = if c.state == "running" {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::Red)
    };

    let (cpu_str, cpu_style, mem_str) = match stats {
        None => ("–".to_string(), Style::default().fg(Color::DarkGray), "–".to_string()),
        Some(s) => {
            let style = cpu_color(parse_pct(&s.cpu_perc));
            (s.cpu_perc.clone(), style, s.mem_usage.clone())
        }
    };

    let status_style = if c.status.contains("unhealthy") {
        Style::default().fg(Color::Red)
    } else if c.status.contains("healthy") {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::Yellow)
    };

    Row::new(vec![
        Cell::from(c.names.as_str()),
        Cell::from(c.state.as_str()).style(state_style),
        Cell::from(cpu_str).style(cpu_style),
        Cell::from(mem_str),
        Cell::from(c.status.as_str()).style(status_style),
    ])
}

// ── azure sidebar ─────────────────────────────────────────────────────────────

fn draw_azure_sidebar(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(" azure db ", Style::default().fg(Color::DarkGray)));
    frame.render_widget(block, area);

    let inner = area.inner(ratatui::layout::Margin { horizontal: 2, vertical: 1 });
    let db = match app.snapshot.as_ref().and_then(|s| s.azure_db.as_ref()) {
        Some(d) => d,
        None => return,
    };

    let rows = azure_metric_rows(db);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(std::iter::repeat_n(Constraint::Length(2), rows.len()).collect::<Vec<_>>())
        .split(inner);

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
        ("CONNS".into(), format!("{:.0}",  db.connections),     Style::default().fg(Color::White)),
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
        Line::from(vec![
            Span::styled("[q]", Style::default().fg(Color::Yellow)),
            Span::styled(" quit  ", Style::default().fg(Color::DarkGray)),
            Span::styled("[jk/↑↓]", Style::default().fg(Color::Yellow)),
            Span::styled(" select  ", Style::default().fg(Color::DarkGray)),
            Span::styled("[r]", Style::default().fg(Color::Yellow)),
            Span::styled(" restart  ", Style::default().fg(Color::DarkGray)),
            Span::styled("[:8080]", Style::default().fg(Color::Blue)),
            Span::styled(" web  ", Style::default().fg(Color::DarkGray)),
            Span::styled(refresh, Style::default().fg(Color::DarkGray)),
        ])
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
