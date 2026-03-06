use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use std::{io, time::Duration};

use crate::config::{Profile, ThemeSection};
use super::ui::Theme;

/// Run the interactive profile picker.
///
/// Returns the selected profile name, or `None` if the user quit.
pub fn run(profiles: &[(String, Profile)], theme_cfg: &ThemeSection) -> Result<Option<String>> {
    if profiles.is_empty() {
        return Ok(None);
    }

    let theme = Theme::from_config(theme_cfg);

    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        original_hook(info);
    }));

    setup_terminal()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    let mut list_state = ListState::default();
    list_state.select(Some(0));

    let result = picker_loop(&mut terminal, &mut list_state, profiles, &theme);

    restore_terminal()?;
    result
}

fn picker_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    list_state: &mut ListState,
    profiles: &[(String, Profile)],
    theme: &Theme,
) -> Result<Option<String>> {
    let tick = Duration::from_millis(16);

    loop {
        terminal.draw(|f| draw(f, list_state, profiles, theme))?;

        if event::poll(tick)? {
            match event::read()? {
                Event::Key(key) => match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(None),
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(None);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        let n = profiles.len();
                        let next = list_state.selected().map(|i| (i + 1) % n).unwrap_or(0);
                        list_state.select(Some(next));
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        let n = profiles.len();
                        let prev = list_state.selected()
                            .map(|i| if i == 0 { n - 1 } else { i - 1 })
                            .unwrap_or(0);
                        list_state.select(Some(prev));
                    }
                    KeyCode::Enter | KeyCode::Char(' ') => {
                        let name = list_state.selected()
                            .and_then(|i| profiles.get(i))
                            .map(|(name, _)| name.clone());
                        return Ok(name);
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }
}

// ── drawing ───────────────────────────────────────────────────────────────────

fn draw(frame: &mut Frame, list_state: &mut ListState, profiles: &[(String, Profile)], theme: &Theme) {
    let area = frame.area();

    // Outer chrome
    let outer_title = Line::from(vec![
        Span::styled(" ✦ wisp ", Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)),
        Span::styled("│", Style::default().fg(theme.muted)),
        Span::styled(
            format!(" {} profile{} ", profiles.len(), if profiles.len() == 1 { "" } else { "s" }),
            Style::default().fg(theme.muted),
        ),
    ]);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border))
            .title(outer_title),
        area,
    );

    let inner = area.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });

    // Vertical: body | footer
    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(inner);

    let body        = vert[0];
    let footer_area = vert[1];

    // Horizontal: list | details
    let horiz = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(body);

    draw_list(frame, horiz[0], list_state, profiles, theme);
    draw_details(frame, horiz[1], list_state.selected().and_then(|i| profiles.get(i)), theme);
    draw_footer(frame, footer_area, theme);
}

fn draw_list(
    frame: &mut Frame,
    area: Rect,
    list_state: &mut ListState,
    profiles: &[(String, Profile)],
    theme: &Theme,
) {
    let items: Vec<ListItem> = profiles.iter().map(|(name, p)| {
        let host = if p.address.is_empty() { "—".into() } else { p.address.clone() };
        ListItem::new(Line::from(vec![
            Span::styled(
                format!("  {:<18}", name),
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            ),
            Span::styled(host, Style::default().fg(theme.muted)),
        ]))
    }).collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.muted))
                .title(Span::styled(" profiles ", Style::default().fg(theme.muted))),
        )
        .highlight_style(
            Style::default()
                .fg(theme.selection_fg)
                .bg(theme.selection_bg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("❯ ");

    frame.render_stateful_widget(list, area, list_state);
}

fn draw_details(frame: &mut Frame, area: Rect, entry: Option<&(String, Profile)>, theme: &Theme) {
    let (title, content) = match entry {
        None => ("details".to_string(), vec![]),
        Some((name, p)) => {
            let transport = p.transport.as_deref().unwrap_or("tailscale");
            let transport_label = if transport.eq_ignore_ascii_case("ssh") {
                "SSH (standard)"
            } else {
                "Tailscale SSH"
            };

            let mut rows: Vec<(&'static str, String)> = vec![
                ("HOST",      p.address.clone()),
                ("USER",      p.user.clone().unwrap_or_else(|| "deploy".into())),
                ("PORT",      p.port.map(|v| v.to_string()).unwrap_or_else(|| "22".into())),
                ("TRANSPORT", transport_label.into()),
                ("INTERVAL",  format!("{}s", p.interval.unwrap_or(5))),
                ("WEB PORT",  format!(":{}", p.web_port.unwrap_or(8080))),
            ];

            if let Some(az) = &p.azure {
                rows.push(("AZURE DB",  az.db_server.clone()));
                rows.push(("AZ TYPE",   az.db_type.clone()));
            } else {
                rows.push(("AZURE DB",  "—".into()));
            }

            if let Some(al) = &p.alerts {
                rows.push(("CPU WARN",  format!("{:.0}%", al.cpu_warn)));
                rows.push(("CPU CRIT",  format!("{:.0}%", al.cpu_crit)));
                rows.push(("MEM WARN",  format!("{:.0}%", al.mem_warn)));
                rows.push(("MEM CRIT",  format!("{:.0}%", al.mem_crit)));
            }

            (format!(" {} ", name), rows)
        }
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.muted))
        .title(Span::styled(title, Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)));
    frame.render_widget(block, area);

    if content.is_empty() {
        return;
    }

    let inner = area.inner(ratatui::layout::Margin { horizontal: 2, vertical: 1 });

    let constraints: Vec<Constraint> =
        std::iter::repeat_n(Constraint::Length(2), content.len()).collect();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    for (i, (label, value)) in content.iter().enumerate() {
        if i >= chunks.len() { break; }
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{label:<11}"), Style::default().fg(theme.muted)),
                Span::styled(value.as_str(), Style::default().fg(theme.text).add_modifier(Modifier::BOLD)),
            ])),
            chunks[i],
        );
    }
}

fn draw_footer(frame: &mut Frame, area: Rect, theme: &Theme) {
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("[jk/↑↓]", Style::default().fg(theme.warning)),
            Span::styled(" navigate  ", Style::default().fg(theme.muted)),
            Span::styled("[enter]", Style::default().fg(theme.warning)),
            Span::styled(" connect  ", Style::default().fg(theme.muted)),
            Span::styled("[q/esc]", Style::default().fg(theme.warning)),
            Span::styled(" quit", Style::default().fg(theme.muted)),
        ])),
        area,
    );
}

// ── welcome / no-config screen ────────────────────────────────────────────────

/// Show the onboarding screen when no configuration or host was provided.
/// Blocks until the user presses any key.
pub fn run_welcome(theme_cfg: &ThemeSection) -> Result<()> {
    let theme = Theme::from_config(theme_cfg);

    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        original_hook(info);
    }));

    setup_terminal()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let tick = Duration::from_millis(16);

    loop {
        terminal.draw(|f| draw_welcome(f, &theme))?;
        if event::poll(tick)? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                    _ => {}
                }
            }
        }
    }

    restore_terminal()?;
    Ok(())
}

fn draw_welcome(frame: &mut Frame, theme: &Theme) {
    let area = frame.area();

    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border))
            .title(Line::from(vec![
                Span::styled(" ✦ wisp ", Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)),
                Span::styled("│", Style::default().fg(theme.muted)),
                Span::styled(" getting started ", Style::default().fg(theme.muted)),
            ])),
        area,
    );

    let inner = area.inner(ratatui::layout::Margin { horizontal: 2, vertical: 1 });

    // Vertical: header | cards | footer
    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),  // headline
            Constraint::Length(1),  // spacer
            Constraint::Length(4),  // card 1
            Constraint::Length(1),  // spacer
            Constraint::Length(4),  // card 2
            Constraint::Length(1),  // spacer
            Constraint::Length(4),  // card 3
            Constraint::Min(0),     // padding
            Constraint::Length(1),  // footer
        ])
        .split(inner);

    // Headline
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "No configuration found",
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                "Choose one of the options below to get started.",
                Style::default().fg(theme.muted),
            )),
        ]),
        vert[0],
    );

    // Card builder
    let card = |title: &'static str, command: &'static str, desc: &'static str, t: &Theme| {
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(command, Style::default().fg(t.accent).add_modifier(Modifier::BOLD)),
            ]),
            Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(desc, Style::default().fg(t.muted)),
            ]),
        ])
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(t.muted))
                .title(Span::styled(format!(" {title} "), Style::default().fg(t.border))),
        )
        .wrap(Wrap { trim: false })
    };

    frame.render_widget(
        card(
            "connect directly",
            "wisp -H <tailscale-ip> -u <user>",
            "One-shot connection — no config file needed",
            theme,
        ),
        vert[2],
    );

    frame.render_widget(
        card(
            "interactive setup",
            "wisp --setup",
            "Guided wizard — discovers Azure resources and writes ~/.config/wisp/config.toml",
            theme,
        ),
        vert[4],
    );

    frame.render_widget(
        card(
            "config file",
            "~/.config/wisp/config.toml  or  ./wisp.toml",
            "[profiles.prod]  address = \"100.64.0.10\"  user = \"deploy\"",
            theme,
        ),
        vert[6],
    );

    // Footer
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("[q/esc]", Style::default().fg(theme.warning)),
            Span::styled(" quit", Style::default().fg(theme.muted)),
        ])),
        vert[8],
    );
}

// ── terminal helpers ──────────────────────────────────────────────────────────

fn setup_terminal() -> Result<()> {
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    Ok(())
}

fn restore_terminal() -> Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture)?;
    Ok(())
}
