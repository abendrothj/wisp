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
    sync::mpsc,
    time::{Duration, Instant},
};

use crate::telemetry::Snapshot;

pub struct App {
    pub snapshot: Option<Snapshot>,
    pub last_updated: Option<Instant>,
    /// Drives the ratatui Table selection highlight.
    pub table_state: TableState,
    /// Number of rows in the current snapshot — used to clamp selection.
    row_count: usize,
}

impl App {
    fn new() -> Self {
        Self {
            snapshot: None,
            last_updated: None,
            table_state: TableState::default(),
            row_count: 0,
        }
    }

    fn ingest(&mut self, snap: Snapshot) {
        self.row_count = snap.containers.len();
        // Keep selection in bounds when container count changes.
        if let Some(i) = self.table_state.selected() {
            if i >= self.row_count && self.row_count > 0 {
                self.table_state.select(Some(self.row_count - 1));
            }
        }
        self.snapshot = Some(snap);
        self.last_updated = Some(Instant::now());
    }

    fn select_next(&mut self) {
        if self.row_count == 0 {
            return;
        }
        let next = self
            .table_state
            .selected()
            .map(|i| (i + 1) % self.row_count)
            .unwrap_or(0);
        self.table_state.select(Some(next));
    }

    fn select_prev(&mut self) {
        if self.row_count == 0 {
            return;
        }
        let prev = self
            .table_state
            .selected()
            .map(|i| if i == 0 { self.row_count - 1 } else { i - 1 })
            .unwrap_or(0);
        self.table_state.select(Some(prev));
    }
}

/// Run the TUI, consuming snapshots from `rx`.
///
/// Intended to be called inside `tokio::task::block_in_place` so the async
/// SSH polling task can keep running on the tokio thread pool.
pub fn run(host: &str, rx: mpsc::Receiver<Snapshot>) -> Result<()> {
    // Install a panic hook that restores the terminal before printing the panic.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        original_hook(info);
    }));

    setup_terminal()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut app = App::new();
    // Select first row so the UI doesn't look blank while waiting for data.
    app.table_state.select(Some(0));

    let tick = Duration::from_millis(16); // ~60 fps

    let result = run_loop(&mut terminal, &mut app, host, rx, tick);

    restore_terminal()?;
    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    host: &str,
    rx: mpsc::Receiver<Snapshot>,
    tick: Duration,
) -> Result<()> {
    loop {
        // Drain all pending snapshots — always display the freshest one.
        while let Ok(snap) = rx.try_recv() {
            app.ingest(snap);
        }

        terminal.draw(|frame| ui::draw(frame, app, host))?;

        // Poll for keyboard events with a short timeout so we render at ~60 fps
        // and check the channel each frame.
        if event::poll(tick)? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Char('c')
                        if key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        break
                    }
                    KeyCode::Down | KeyCode::Char('j') => app.select_next(),
                    KeyCode::Up | KeyCode::Char('k') => app.select_prev(),
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
