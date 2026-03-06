pub mod picker;
pub mod ui;

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers, MouseEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend, widgets::TableState};
use std::{
    collections::BTreeMap,
    io,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot, watch};

use crate::telemetry::Snapshot;
use crate::config::{AlertsSection, ThemeSection};
use crate::{LogStreamRequest, RemoteAction, RemoteActionRequest, RemoteActionResult};

/// How long without a fresh snapshot before we show "reconnecting…"
const STALE_THRESHOLD: Duration = Duration::from_secs(15);

// ── compose row model ─────────────────────────────────────────────────────────

/// Each entry in the visual table is either a group header or a container row.
#[derive(Debug, Clone)]
pub enum ComposeRow {
    /// Section header for a Docker Compose project.
    Header(String),
    /// Index into `Snapshot::containers`.
    Container(usize),
}

// ── streaming log popup ───────────────────────────────────────────────────────

pub struct LogStreamPopup {
    pub title:       String,
    pub body:        String,
    pub rx:          mpsc::Receiver<String>,
    pub scroll:      u16,
    pub auto_scroll: bool,
    pub ended:       bool,
}

// ── app state ─────────────────────────────────────────────────────────────────

pub struct App {
    pub snapshot:        Option<Snapshot>,
    pub last_updated:    Option<Instant>,
    pub table_state:     TableState,
    /// Maps each visual table row to a `ComposeRow` (header or container index).
    pub compose_rows:    Vec<ComposeRow>,
    pub pending_action:  Option<(String, Instant)>,
    pub popup:           Option<Popup>,
    pub pending_result:  Option<oneshot::Receiver<RemoteActionResult>>,
    pub pending_mode:    Option<PendingMode>,
    pub prune_guard_until: Option<Instant>,
    pub log_stream:      Option<LogStreamPopup>,
}

pub enum PendingMode {
    ToastSuccess(String),
    Popup,
}

pub struct Popup {
    pub title:    String,
    pub body:     String,
    pub is_error: bool,
    pub loading:  bool,
    pub scroll:   u16,
}

impl App {
    fn new() -> Self {
        Self {
            snapshot:          None,
            last_updated:      None,
            table_state:       TableState::default(),
            compose_rows:      Vec::new(),
            pending_action:    None,
            popup:             None,
            pending_result:    None,
            pending_mode:      None,
            prune_guard_until: None,
            log_stream:        None,
        }
    }

    fn ingest(&mut self, snap: Snapshot) {
        // Build compose-grouped row order ──────────────────────────────────────
        let mut grouped: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        let mut ungrouped: Vec<usize> = Vec::new();

        for (i, c) in snap.containers.iter().enumerate() {
            match c.compose_project() {
                Some(proj) => grouped.entry(proj.to_string()).or_default().push(i),
                None       => ungrouped.push(i),
            }
        }

        let mut rows: Vec<ComposeRow> = Vec::new();
        for (project, indices) in &grouped {
            rows.push(ComposeRow::Header(project.clone()));
            for &idx in indices {
                rows.push(ComposeRow::Container(idx));
            }
        }
        for idx in &ungrouped {
            rows.push(ComposeRow::Container(*idx));
        }

        // Clamp / repair selection ─────────────────────────────────────────────
        let sel = self.table_state.selected().unwrap_or(0);
        let first_container = rows.iter().position(|r| matches!(r, ComposeRow::Container(_)));
        let last_container  = rows.iter().rposition(|r| matches!(r, ComposeRow::Container(_)));

        let new_sel = if rows.is_empty() {
            None
        } else if matches!(rows.get(sel), Some(ComposeRow::Container(_))) {
            Some(sel) // existing selection is still valid
        } else if sel >= rows.len() {
            last_container // went past the end — pick last container
        } else {
            first_container // landed on a header — move to first container
        };
        self.table_state.select(new_sel);

        self.compose_rows = rows;
        self.snapshot     = Some(snap);
        self.last_updated = Some(Instant::now());
    }

    pub fn is_stale(&self) -> bool {
        self.last_updated
            .map(|t| t.elapsed() > STALE_THRESHOLD)
            .unwrap_or(false)
    }

    fn selected_name(&self) -> Option<String> {
        let snap = self.snapshot.as_ref()?;
        let row  = self.table_state.selected()?;
        match self.compose_rows.get(row) {
            Some(ComposeRow::Container(idx)) => snap.containers.get(*idx).map(|c| c.names.clone()),
            _ => None,
        }
    }

    fn select_next(&mut self) {
        let total = self.compose_rows.len();
        if total == 0 { return; }
        let start = self.table_state.selected().unwrap_or(0);
        let mut next = (start + 1) % total;
        for _ in 0..total {
            if matches!(self.compose_rows.get(next), Some(ComposeRow::Container(_))) { break; }
            next = (next + 1) % total;
        }
        self.table_state.select(Some(next));
    }

    fn select_prev(&mut self) {
        let total = self.compose_rows.len();
        if total == 0 { return; }
        let start = self.table_state.selected().unwrap_or(0);
        let mut prev = if start == 0 { total - 1 } else { start - 1 };
        for _ in 0..total {
            if matches!(self.compose_rows.get(prev), Some(ComposeRow::Container(_))) { break; }
            prev = if prev == 0 { total - 1 } else { prev - 1 };
        }
        self.table_state.select(Some(prev));
    }
}

// ── public entry point ────────────────────────────────────────────────────────

pub fn run(
    host: &str,
    mut rx: watch::Receiver<Option<Snapshot>>,
    action_tx: mpsc::Sender<RemoteActionRequest>,
    stream_tx: mpsc::Sender<LogStreamRequest>,
    theme_cfg: ThemeSection,
    alerts: AlertsSection,
    web_port: u16,
) -> Result<()> {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        original_hook(info);
    }));

    setup_terminal()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut app = App::new();
    let theme = ui::Theme::from_config(&theme_cfg);

    let result = run_loop(
        &mut terminal,
        &mut app,
        host,
        &theme,
        &alerts,
        web_port,
        &mut rx,
        &action_tx,
        &stream_tx,
    );

    restore_terminal()?;
    result
}

#[allow(clippy::too_many_arguments)]
fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    host: &str,
    theme: &ui::Theme,
    alerts: &AlertsSection,
    web_port: u16,
    rx: &mut watch::Receiver<Option<Snapshot>>,
    action_tx: &mpsc::Sender<RemoteActionRequest>,
    stream_tx: &mpsc::Sender<LogStreamRequest>,
) -> Result<()> {
    let tick = Duration::from_millis(16);

    loop {
        // Consume latest snapshot.
        if rx.has_changed().unwrap_or(false)
            && let Some(snap) = rx.borrow_and_update().clone() {
            app.ingest(snap);
        }

        // Clear expired pending action messages (show for 8 s).
        if app.pending_action.as_ref()
            .map(|(_, t)| t.elapsed() > Duration::from_secs(8))
            .unwrap_or(false) {
            app.pending_action = None;
        }

        if app.prune_guard_until.map(|t| t <= Instant::now()).unwrap_or(false) {
            app.prune_guard_until = None;
        }

        // Drain streaming log lines.
        if let Some(stream) = app.log_stream.as_mut() {
            if !stream.ended {
                loop {
                    match stream.rx.try_recv() {
                        Ok(line) => {
                            stream.body.push_str(&line);
                            stream.body.push('\n');
                            if stream.auto_scroll {
                                stream.scroll = u16::MAX;
                            }
                        }
                        Err(mpsc::error::TryRecvError::Empty) => break,
                        Err(mpsc::error::TryRecvError::Disconnected) => {
                            stream.body.push_str("\n[stream ended]");
                            stream.ended = true;
                            stream.auto_scroll = false;
                            break;
                        }
                    }
                }
            }
        }

        // Resolve completed one-shot action.
        if let Some(rx) = app.pending_result.as_mut() {
            match rx.try_recv() {
                Ok(result) => {
                    app.pending_action = None;
                    match app.pending_mode.take() {
                        Some(PendingMode::Popup) => {
                            app.popup = Some(Popup {
                                title: result.title,
                                body: result.output,
                                is_error: result.is_error,
                                loading: false,
                                scroll: 0,
                            });
                        }
                        Some(PendingMode::ToastSuccess(message)) => {
                            if result.is_error {
                                app.popup = Some(Popup {
                                    title: result.title,
                                    body: result.output,
                                    is_error: true,
                                    loading: false,
                                    scroll: 0,
                                });
                            } else {
                                app.pending_action = Some((message, Instant::now()));
                            }
                        }
                        None => {
                            if result.is_error {
                                app.popup = Some(Popup {
                                    title: result.title,
                                    body: result.output,
                                    is_error: true,
                                    loading: false,
                                    scroll: 0,
                                });
                            }
                        }
                    }
                    app.pending_result = None;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    app.pending_result = None;
                    app.pending_mode   = None;
                    app.popup = Some(Popup {
                        title:    "Action failed".to_string(),
                        body:     "action result channel closed unexpectedly".to_string(),
                        is_error: true,
                        loading:  false,
                        scroll:   0,
                    });
                }
            }
        }

        terminal.draw(|frame| ui::draw(frame, app, host, theme, alerts, web_port))?;

        if event::poll(tick)? {
            match event::read()? {
                // ── streaming log popup input ──────────────────────────────
                Event::Key(key) if app.log_stream.is_some() => {
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('q') => {
                            app.log_stream = None; // drop rx → kills child process
                        }
                        KeyCode::Down | KeyCode::Char('j') => stream_scroll_down(app, 1),
                        KeyCode::Up   | KeyCode::Char('k') => stream_scroll_up(app, 1),
                        KeyCode::PageDown => stream_scroll_down(app, 10),
                        KeyCode::PageUp   => stream_scroll_up(app, 10),
                        KeyCode::Home => {
                            if let Some(s) = app.log_stream.as_mut() {
                                s.scroll = 0;
                                s.auto_scroll = false;
                            }
                        }
                        KeyCode::End => {
                            if let Some(s) = app.log_stream.as_mut() {
                                s.scroll = u16::MAX;
                                s.auto_scroll = true;
                            }
                        }
                        _ => {}
                    }
                }

                // ── regular popup input ────────────────────────────────────
                Event::Mouse(mouse) if app.popup.is_some() => {
                    match mouse.kind {
                        MouseEventKind::ScrollDown => popup_scroll_down(app, 3),
                        MouseEventKind::ScrollUp   => popup_scroll_up(app, 3),
                        _ => {}
                    }
                }

                Event::Key(key) if app.popup.is_some() => {
                    match key.code {
                        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                            app.popup = None;
                        }
                        KeyCode::Down | KeyCode::Char('j') => popup_scroll_down(app, 1),
                        KeyCode::Up   | KeyCode::Char('k') => popup_scroll_up(app, 1),
                        KeyCode::PageDown => popup_scroll_down(app, 10),
                        KeyCode::PageUp   => popup_scroll_up(app, 10),
                        KeyCode::Home => {
                            if let Some(p) = app.popup.as_mut() { p.scroll = 0; }
                        }
                        KeyCode::End => {
                            if let Some(p) = app.popup.as_mut() { p.scroll = u16::MAX; }
                        }
                        _ => {}
                    }
                }

                // ── mouse on streaming popup ───────────────────────────────
                Event::Mouse(mouse) if app.log_stream.is_some() => {
                    match mouse.kind {
                        MouseEventKind::ScrollDown => stream_scroll_down(app, 3),
                        MouseEventKind::ScrollUp   => stream_scroll_up(app, 3),
                        _ => {}
                    }
                }

                // ── main table input ───────────────────────────────────────
                Event::Mouse(mouse) => {
                    if app.popup.is_some() {
                        match mouse.kind {
                            MouseEventKind::ScrollDown => popup_scroll_down(app, 3),
                            MouseEventKind::ScrollUp   => popup_scroll_up(app, 3),
                            _ => {}
                        }
                    }
                }

                Event::Key(key) => {
                    match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Char('c')
                            if key.modifiers.contains(KeyModifiers::CONTROL) => break,

                        KeyCode::Down | KeyCode::Char('j') => app.select_next(),
                        KeyCode::Up   | KeyCode::Char('k') => app.select_prev(),

                        KeyCode::Char('r') => {
                            if let Some(name) = app.selected_name() {
                                let (tx, rx) = oneshot::channel();
                                if action_tx.blocking_send(RemoteActionRequest {
                                    action:     RemoteAction::Restart { name: name.clone() },
                                    respond_to: tx,
                                }).is_ok() {
                                    app.pending_result = Some(rx);
                                    app.pending_mode   = Some(PendingMode::ToastSuccess(format!("{name} restarted")));
                                    app.pending_action = Some((format!("restarting {name}…"), Instant::now()));
                                }
                            }
                        }

                        KeyCode::Char('a') => {
                            if let Some(name) = app.selected_name() {
                                let (tx, rx) = oneshot::channel();
                                if action_tx.blocking_send(RemoteActionRequest {
                                    action:     RemoteAction::Start { name: name.clone() },
                                    respond_to: tx,
                                }).is_ok() {
                                    app.pending_result = Some(rx);
                                    app.pending_mode   = Some(PendingMode::ToastSuccess(format!("{name} started")));
                                    app.pending_action = Some((format!("starting {name}…"), Instant::now()));
                                }
                            }
                        }

                        KeyCode::Char('x') => {
                            if let Some(name) = app.selected_name() {
                                let (tx, rx) = oneshot::channel();
                                if action_tx.blocking_send(RemoteActionRequest {
                                    action:     RemoteAction::Stop { name: name.clone() },
                                    respond_to: tx,
                                }).is_ok() {
                                    app.pending_result = Some(rx);
                                    app.pending_mode   = Some(PendingMode::ToastSuccess(format!("{name} stopped")));
                                    app.pending_action = Some((format!("stopping {name}…"), Instant::now()));
                                }
                            }
                        }

                        KeyCode::Char('l') => {
                            if let Some(name) = app.selected_name() {
                                let (tx, rx) = oneshot::channel();
                                if action_tx.blocking_send(RemoteActionRequest {
                                    action:     RemoteAction::Logs { name: name.clone() },
                                    respond_to: tx,
                                }).is_ok() {
                                    app.pending_result = Some(rx);
                                    app.pending_mode   = Some(PendingMode::Popup);
                                    app.popup = Some(Popup {
                                        title:    format!("Logs: {name}"),
                                        body:     "fetching last 50 lines…".to_string(),
                                        is_error: false,
                                        loading:  true,
                                        scroll:   0,
                                    });
                                }
                            }
                        }

                        KeyCode::Char('f') => {
                            if let Some(name) = app.selected_name() {
                                let (resp_tx, resp_rx) = oneshot::channel();
                                if stream_tx.blocking_send(LogStreamRequest {
                                    name: name.clone(),
                                    response_tx: resp_tx,
                                }).is_ok() {
                                    if let Ok(chunk_rx) = resp_rx.blocking_recv() {
                                        app.popup = None;
                                        app.log_stream = Some(LogStreamPopup {
                                            title:       format!("Logs: {name} (streaming)"),
                                            body:        String::new(),
                                            rx:          chunk_rx,
                                            scroll:      u16::MAX,
                                            auto_scroll: true,
                                            ended:       false,
                                        });
                                    }
                                }
                            }
                        }

                        KeyCode::Enter => {
                            if let Some(name) = app.selected_name() {
                                let (tx, rx) = oneshot::channel();
                                if action_tx.blocking_send(RemoteActionRequest {
                                    action:     RemoteAction::Inspect { name: name.clone() },
                                    respond_to: tx,
                                }).is_ok() {
                                    app.pending_result = Some(rx);
                                    app.pending_mode   = Some(PendingMode::Popup);
                                    app.popup = Some(Popup {
                                        title:    format!("Inspect: {name}"),
                                        body:     "running docker inspect…".to_string(),
                                        is_error: false,
                                        loading:  true,
                                        scroll:   0,
                                    });
                                }
                            }
                        }

                        KeyCode::Char('d') => {
                            let (tx, rx) = oneshot::channel();
                            if action_tx.blocking_send(RemoteActionRequest {
                                action:     RemoteAction::SystemDf,
                                respond_to: tx,
                            }).is_ok() {
                                app.pending_result = Some(rx);
                                app.pending_mode   = Some(PendingMode::Popup);
                                app.popup = Some(Popup {
                                    title:    "Docker Disk Usage".to_string(),
                                    body:     "running docker system df…".to_string(),
                                    is_error: false,
                                    loading:  true,
                                    scroll:   0,
                                });
                            }
                        }

                        KeyCode::Char('p') => {
                            let now = Instant::now();
                            if app.prune_guard_until.map(|until| until > now).unwrap_or(false) {
                                app.prune_guard_until = None;
                                let (tx, rx) = oneshot::channel();
                                if action_tx.blocking_send(RemoteActionRequest {
                                    action:     RemoteAction::Prune,
                                    respond_to: tx,
                                }).is_ok() {
                                    app.pending_result = Some(rx);
                                    app.pending_mode   = Some(PendingMode::Popup);
                                    app.popup = Some(Popup {
                                        title:    "Prune: stopped containers".to_string(),
                                        body:     "running docker container prune -f…".to_string(),
                                        is_error: false,
                                        loading:  true,
                                        scroll:   0,
                                    });
                                }
                            } else {
                                app.prune_guard_until = Some(now + Duration::from_secs(5));
                                app.pending_action = Some((
                                    "guarded prune: press [p] again within 5s".to_string(),
                                    Instant::now(),
                                ));
                            }
                        }

                        _ => {}
                    }
                }

                _ => {}
            }
        }
    }
    Ok(())
}

// ── terminal setup / teardown ─────────────────────────────────────────────────

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

// ── scroll helpers ────────────────────────────────────────────────────────────

fn popup_scroll_down(app: &mut App, amount: u16) {
    if let Some(p) = app.popup.as_mut() {
        p.scroll = p.scroll.saturating_add(amount);
    }
}

fn popup_scroll_up(app: &mut App, amount: u16) {
    if let Some(p) = app.popup.as_mut() {
        p.scroll = p.scroll.saturating_sub(amount);
    }
}

fn stream_scroll_down(app: &mut App, amount: u16) {
    if let Some(s) = app.log_stream.as_mut() {
        s.scroll = s.scroll.saturating_add(amount);
        s.auto_scroll = false;
    }
}

fn stream_scroll_up(app: &mut App, amount: u16) {
    if let Some(s) = app.log_stream.as_mut() {
        s.scroll = s.scroll.saturating_sub(amount);
        s.auto_scroll = false;
    }
}
