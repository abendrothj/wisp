pub mod ui;

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers, MouseEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend, widgets::TableState};
use std::{
    io,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot, watch};

use crate::ssh::Transport;
use crate::telemetry::Snapshot;
use crate::{RemoteAction, RemoteActionRequest, RemoteActionResult};

/// How long without a fresh snapshot before we show "reconnecting…"
const STALE_THRESHOLD: Duration = Duration::from_secs(15);

pub struct App {
    pub snapshot: Option<Snapshot>,
    pub last_updated: Option<Instant>,
    pub table_state: TableState,
    row_count: usize,
    /// Brief status message after an action (e.g. "restarting finlingo-admin…")
    pub pending_action: Option<(String, Instant)>,
    pub popup: Option<Popup>,
    pub pending_result: Option<oneshot::Receiver<RemoteActionResult>>,
    pub pending_mode: Option<PendingMode>,
    pub prune_guard_until: Option<Instant>,
}

pub enum PendingMode {
    ToastSuccess(String),
    Popup,
}

pub struct Popup {
    pub title: String,
    pub body: String,
    pub is_error: bool,
    pub loading: bool,
    pub scroll: u16,
}

#[derive(Clone)]
pub struct ShellTarget {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub transport: Transport,
}

impl App {
    fn new() -> Self {
        Self {
            snapshot: None,
            last_updated: None,
            table_state: TableState::default(),
            row_count: 0,
            pending_action: None,
            popup: None,
            pending_result: None,
            pending_mode: None,
            prune_guard_until: None,
        }
    }

    fn ingest(&mut self, snap: Snapshot) {
        self.row_count = snap.containers.len();
        if let Some(i) = self.table_state.selected() {
            if i >= self.row_count && self.row_count > 0 {
                self.table_state.select(Some(self.row_count - 1));
            }
        }
        self.snapshot = Some(snap);
        self.last_updated = Some(Instant::now());
    }

    pub fn is_stale(&self) -> bool {
        self.last_updated
            .map(|t| t.elapsed() > STALE_THRESHOLD)
            .unwrap_or(false)
    }

    /// Return the name of the currently selected container, if any.
    fn selected_name(&self) -> Option<String> {
        let snap = self.snapshot.as_ref()?;
        let i    = self.table_state.selected()?;
        snap.containers.get(i).map(|c| c.names.clone())
    }

    fn select_next(&mut self) {
        if self.row_count == 0 { return; }
        let next = self.table_state.selected()
            .map(|i| (i + 1) % self.row_count)
            .unwrap_or(0);
        self.table_state.select(Some(next));
    }

    fn select_prev(&mut self) {
        if self.row_count == 0 { return; }
        let prev = self.table_state.selected()
            .map(|i| if i == 0 { self.row_count - 1 } else { i - 1 })
            .unwrap_or(0);
        self.table_state.select(Some(prev));
    }
}

/// Run the TUI.
///
/// `restart_tx` — send a container name here to trigger `docker restart <name>` on the host.
pub fn run(
    host: &str,
    mut rx: watch::Receiver<Option<Snapshot>>,
    action_tx: mpsc::Sender<RemoteActionRequest>,
    shell_target: ShellTarget,
) -> Result<()> {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        original_hook(info);
    }));

    setup_terminal()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut app = App::new();
    app.table_state.select(Some(0));

    let result = run_loop(
        &mut terminal,
        &mut app,
        host,
        &mut rx,
        &action_tx,
        &shell_target,
    );

    restore_terminal()?;
    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    host: &str,
    rx: &mut watch::Receiver<Option<Snapshot>>,
    action_tx: &mpsc::Sender<RemoteActionRequest>,
    shell_target: &ShellTarget,
) -> Result<()> {
    let tick = Duration::from_millis(16);

    loop {
        // Consume latest snapshot.
        if rx.has_changed().unwrap_or(false) {
            if let Some(snap) = rx.borrow_and_update().clone() {
                app.ingest(snap);
            }
        }

        // Clear expired pending action messages (show for 8 s).
        if app.pending_action.as_ref().map(|(_, t)| t.elapsed() > Duration::from_secs(8)).unwrap_or(false) {
            app.pending_action = None;
        }

        if app.prune_guard_until.map(|t| t <= Instant::now()).unwrap_or(false) {
            app.prune_guard_until = None;
        }

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
                    app.pending_mode = None;
                    app.popup = Some(Popup {
                        title: "Action failed".to_string(),
                        body: "action result channel closed unexpectedly".to_string(),
                        is_error: true,
                        loading: false,
                        scroll: 0,
                    });
                }
            }
        }

        terminal.draw(|frame| ui::draw(frame, app, host))?;

        if event::poll(tick)? {
            match event::read()? {
                Event::Mouse(mouse) => {
                    if app.popup.is_some() {
                        match mouse.kind {
                            MouseEventKind::ScrollDown => popup_scroll_down(app, 3),
                            MouseEventKind::ScrollUp => popup_scroll_up(app, 3),
                            _ => {}
                        }
                    }
                }

                Event::Key(key) => {
                    if app.popup.is_some() {
                        match key.code {
                            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                                app.popup = None;
                            }
                            KeyCode::Down | KeyCode::Char('j') => popup_scroll_down(app, 1),
                            KeyCode::Up | KeyCode::Char('k') => popup_scroll_up(app, 1),
                            KeyCode::PageDown => popup_scroll_down(app, 10),
                            KeyCode::PageUp => popup_scroll_up(app, 10),
                            KeyCode::Home => {
                                if let Some(p) = app.popup.as_mut() {
                                    p.scroll = 0;
                                }
                            }
                            KeyCode::End => {
                                if let Some(p) = app.popup.as_mut() {
                                    p.scroll = u16::MAX;
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }

                    match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Char('c')
                            if key.modifiers.contains(KeyModifiers::CONTROL) => break,

                        KeyCode::Down | KeyCode::Char('j') => app.select_next(),
                        KeyCode::Up   | KeyCode::Char('k') => app.select_prev(),

                        KeyCode::Char('r') => {
                            if let Some(name) = app.selected_name() {
                                let (tx, rx) = oneshot::channel();
                                let req = RemoteActionRequest {
                                    action: RemoteAction::Restart { name: name.clone() },
                                    respond_to: tx,
                                };
                                if action_tx.blocking_send(req).is_ok() {
                                    app.pending_result = Some(rx);
                                    app.pending_mode = Some(PendingMode::ToastSuccess(format!("{} restarted", name)));
                                    app.pending_action = Some((
                                        format!("restarting {}…", name),
                                        Instant::now(),
                                    ));
                                }
                            }
                        }

                        KeyCode::Char('a') => {
                            if let Some(name) = app.selected_name() {
                                let (tx, rx) = oneshot::channel();
                                let req = RemoteActionRequest {
                                    action: RemoteAction::Start { name: name.clone() },
                                    respond_to: tx,
                                };
                                if action_tx.blocking_send(req).is_ok() {
                                    app.pending_result = Some(rx);
                                    app.pending_mode = Some(PendingMode::ToastSuccess(format!("{} started", name)));
                                    app.pending_action = Some((
                                        format!("starting {}…", name),
                                        Instant::now(),
                                    ));
                                }
                            }
                        }

                        KeyCode::Char('x') => {
                            if let Some(name) = app.selected_name() {
                                let (tx, rx) = oneshot::channel();
                                let req = RemoteActionRequest {
                                    action: RemoteAction::Stop { name: name.clone() },
                                    respond_to: tx,
                                };
                                if action_tx.blocking_send(req).is_ok() {
                                    app.pending_result = Some(rx);
                                    app.pending_mode = Some(PendingMode::ToastSuccess(format!("{} stopped", name)));
                                    app.pending_action = Some((
                                        format!("stopping {}…", name),
                                        Instant::now(),
                                    ));
                                }
                            }
                        }

                        KeyCode::Char('l') => {
                            if let Some(name) = app.selected_name() {
                                let (tx, rx) = oneshot::channel();
                                let req = RemoteActionRequest {
                                    action: RemoteAction::Logs { name: name.clone() },
                                    respond_to: tx,
                                };
                                if action_tx.blocking_send(req).is_ok() {
                                    app.pending_result = Some(rx);
                                    app.pending_mode = Some(PendingMode::Popup);
                                    app.popup = Some(Popup {
                                        title: format!("Logs: {name}"),
                                        body: "fetching last 50 lines…".to_string(),
                                        is_error: false,
                                        loading: true,
                                        scroll: 0,
                                    });
                                }
                            }
                        }

                        KeyCode::Enter => {
                            if let Some(name) = app.selected_name() {
                                let (tx, rx) = oneshot::channel();
                                let req = RemoteActionRequest {
                                    action: RemoteAction::Inspect { name: name.clone() },
                                    respond_to: tx,
                                };
                                if action_tx.blocking_send(req).is_ok() {
                                    app.pending_result = Some(rx);
                                    app.pending_mode = Some(PendingMode::Popup);
                                    app.popup = Some(Popup {
                                        title: format!("Inspect: {name}"),
                                        body: "running docker inspect…".to_string(),
                                        is_error: false,
                                        loading: true,
                                        scroll: 0,
                                    });
                                }
                            }
                        }

                        KeyCode::Char('d') => {
                            let (tx, rx) = oneshot::channel();
                            let req = RemoteActionRequest {
                                action: RemoteAction::SystemDf,
                                respond_to: tx,
                            };
                            if action_tx.blocking_send(req).is_ok() {
                                app.pending_result = Some(rx);
                                app.pending_mode = Some(PendingMode::Popup);
                                app.popup = Some(Popup {
                                    title: "Docker Disk Usage".to_string(),
                                    body: "running docker system df…".to_string(),
                                    is_error: false,
                                    loading: true,
                                    scroll: 0,
                                });
                            }
                        }

                        KeyCode::Char('p') => {
                            let now = Instant::now();
                            if app.prune_guard_until.map(|until| until > now).unwrap_or(false) {
                                app.prune_guard_until = None;
                                let (tx, rx) = oneshot::channel();
                                let req = RemoteActionRequest {
                                    action: RemoteAction::Prune,
                                    respond_to: tx,
                                };
                                if action_tx.blocking_send(req).is_ok() {
                                    app.pending_result = Some(rx);
                                    app.pending_mode = Some(PendingMode::Popup);
                                    app.popup = Some(Popup {
                                        title: "Prune: stopped containers".to_string(),
                                        body: "running docker container prune -f…".to_string(),
                                        is_error: false,
                                        loading: true,
                                        scroll: 0,
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

                        KeyCode::Char('s') => {
                            if let Some(name) = app.selected_name() {
                                app.pending_action = Some((
                                    format!("opening shell in {}…", name),
                                    Instant::now(),
                                ));
                                match open_container_shell(terminal, shell_target, &name) {
                                    Ok(()) => {
                                        app.pending_action = Some((
                                            format!("shell closed for {}", name),
                                            Instant::now(),
                                        ));
                                    }
                                    Err(e) => {
                                        app.popup = Some(Popup {
                                            title: format!("Shell: {name}"),
                                            body: format!("{e:#}"),
                                            is_error: true,
                                            loading: false,
                                            scroll: 0,
                                        });
                                    }
                                }
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

fn popup_scroll_down(app: &mut App, amount: u16) {
    if let Some(popup) = app.popup.as_mut() {
        popup.scroll = popup.scroll.saturating_add(amount);
    }
}

fn popup_scroll_up(app: &mut App, amount: u16) {
    if let Some(popup) = app.popup.as_mut() {
        popup.scroll = popup.scroll.saturating_sub(amount);
    }
}

fn open_container_shell(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    target: &ShellTarget,
    name: &str,
) -> Result<()> {
    restore_terminal()?;

    let command = format!("docker exec -it {} /bin/sh", shell_quote(name));
    let mut process = match target.transport {
        Transport::Tailscale => {
            let mut cmd = Command::new("tailscale");
            cmd.args([
                "ssh",
                &format!("{}@{}", target.user, target.host),
                "--",
                &command,
            ]);
            cmd
        }
        Transport::Ssh => {
            let mut cmd = Command::new("ssh");
            cmd.args([
                "-p",
                &target.port.to_string(),
                &format!("{}@{}", target.user, target.host),
                &command,
            ]);
            cmd
        }
    };

    let status = process
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();

    setup_terminal()?;
    terminal.clear()?;

    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => anyhow::bail!("shell command exited with status {s}"),
        Err(e) => Err(e.into()),
    }
}

fn shell_quote(value: &str) -> String {
    let escaped = value.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}
