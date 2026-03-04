pub mod ui;

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend, widgets::TableState};
use std::{
    io,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, watch};

use crate::telemetry::Snapshot;

/// How long without a fresh snapshot before we show "reconnecting…"
const STALE_THRESHOLD: Duration = Duration::from_secs(15);

pub struct App {
    pub snapshot: Option<Snapshot>,
    pub last_updated: Option<Instant>,
    pub table_state: TableState,
    row_count: usize,
    /// Brief status message after an action (e.g. "restarting finlingo-admin…")
    pub pending_action: Option<(String, Instant)>,
}

impl App {
    fn new() -> Self {
        Self {
            snapshot: None,
            last_updated: None,
            table_state: TableState::default(),
            row_count: 0,
            pending_action: None,
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
    restart_tx: mpsc::Sender<String>,
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

    let result = run_loop(&mut terminal, &mut app, host, &mut rx, &restart_tx);

    restore_terminal()?;
    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    host: &str,
    rx: &mut watch::Receiver<Option<Snapshot>>,
    restart_tx: &mpsc::Sender<String>,
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

        terminal.draw(|frame| ui::draw(frame, app, host))?;

        if event::poll(tick)? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Char('c')
                        if key.modifiers.contains(KeyModifiers::CONTROL) => break,

                    KeyCode::Down | KeyCode::Char('j') => app.select_next(),
                    KeyCode::Up   | KeyCode::Char('k') => app.select_prev(),

                    KeyCode::Char('r') => {
                        if let Some(name) = app.selected_name() {
                            // blocking_send is valid inside block_in_place.
                            let _ = restart_tx.blocking_send(name.clone());
                            app.pending_action = Some((
                                format!("restarting {}…", name),
                                Instant::now(),
                            ));
                        }
                    }

                    _ => {}
                }
            }
        }
    }
    Ok(())
}

fn setup_terminal() -> Result<()> {
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    Ok(())
}

fn restore_terminal() -> Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;
    Ok(())
}
