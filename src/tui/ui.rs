use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
};
use super::App;
use crate::telemetry::docker::{ContainerInfo, ContainerStats};

pub fn draw(frame: &mut Frame, app: &mut App, host: &str) {
    let area = frame.area();

    // ── outer chrome ──────────────────────────────────────────────────────────
    let title = Line::from(vec![
        Span::styled(" lightcontain ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::styled(format!("── {} ", host), Style::default().fg(Color::DarkGray)),
    ]);
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(title);

    frame.render_widget(outer, area);

    // ── inner layout: table + footer ──────────────────────────────────────────
    let inner = area.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(inner);

    draw_table(frame, app, chunks[0]);
    draw_footer(frame, app, chunks[1]);
}

fn draw_table(frame: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    let header = Row::new(vec![
        Cell::from("CONTAINER").style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        Cell::from("STATE").style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        Cell::from("CPU%").style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        Cell::from("MEM USAGE").style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        Cell::from("STATUS").style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
    ])
    .bottom_margin(1);

    let rows: Vec<Row> = match &app.snapshot {
        None => vec![Row::new(vec![Cell::from(
            Span::styled("  waiting for data…", Style::default().fg(Color::DarkGray)),
        )])],
        Some(snap) => {
            // Build a quick lookup: container name → stats
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
        [
            Constraint::Length(26), // CONTAINER
            Constraint::Length(10), // STATE
            Constraint::Length(8),  // CPU%
            Constraint::Length(20), // MEM USAGE
            Constraint::Min(0),     // STATUS
        ],
    )
    .header(header)
    .row_highlight_style(
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
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
            let cpu_pct = parse_pct(&s.cpu_perc);
            let style = cpu_color(cpu_pct);
            (s.cpu_perc.clone(), style, s.mem_usage.clone())
        }
    };

    // Healthy/unhealthy glyph embedded in status
    let status_style = if c.status.contains("healthy") && !c.status.contains("unhealthy") {
        Style::default().fg(Color::Green)
    } else if c.status.contains("unhealthy") {
        Style::default().fg(Color::Red)
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

fn draw_footer(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let refresh_text = match app.last_updated {
        None => "  waiting…".to_string(),
        Some(t) => {
            let secs = t.elapsed().as_secs();
            format!("  refreshed {}s ago", secs)
        }
    };

    let line = Line::from(vec![
        Span::styled("[q]", Style::default().fg(Color::Yellow)),
        Span::styled(" quit  ", Style::default().fg(Color::DarkGray)),
        Span::styled("[↑↓/jk]", Style::default().fg(Color::Yellow)),
        Span::styled(" select  ", Style::default().fg(Color::DarkGray)),
        Span::styled(refresh_text, Style::default().fg(Color::DarkGray)),
    ]);

    frame.render_widget(Paragraph::new(line), area);
}

/// Parse "4.85%" → Some(4.85), "N/A" or malformed → None.
fn parse_pct(s: &str) -> Option<f64> {
    s.trim_end_matches('%').parse().ok()
}

fn cpu_color(pct: Option<f64>) -> Style {
    match pct {
        None => Style::default().fg(Color::DarkGray),
        Some(p) if p >= 20.0 => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        Some(p) if p >= 5.0 => Style::default().fg(Color::Yellow),
        Some(_) => Style::default().fg(Color::Green),
    }
}
