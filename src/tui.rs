//! Terminal lifecycle + the async event task.
//!
//! `Tui` owns the ratatui [`Terminal`] and spawns a single `tokio::select!`
//! task that multiplexes three sources into one `mpsc<Event>`:
//!   1. crossterm input (`EventStream`)            -> `Event::Key` / `Event::Resize`
//!   2. a render `interval` capped at `RENDER_FPS` -> `Event::Render`
//!   3. a logic `interval` at `TICK_HZ`            -> `Event::Tick`
//!
//! Because the select! sleeps between ticks the idle CPU cost stays in single
//! digits (Pitfall #3). The render path never blocks on I/O.

use std::io::{self, Stdout};

use color_eyre::Result;
use crossterm::cursor;
use crossterm::event::{
    Event as CrosstermEvent, EventStream, KeyEvent, KeyEventKind,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Render cadence cap. 30 FPS is smooth in a terminal and CPU-cheap, honoring
/// the "must not peg a core" constraint (Pitfall #3).
pub const RENDER_FPS: u64 = 30;
/// Logic/animation tick rate. Animation advances by real `dt` so the visuals
/// stay framerate-independent.
pub const TICK_HZ: u64 = 60;

/// A unified event delivered to the main loop.
#[derive(Debug, Clone, Copy)]
pub enum Event {
    /// A key press.
    Key(KeyEvent),
    /// Terminal resized to (width, height) in cells.
    Resize(u16, u16),
    /// A logic/animation tick.
    Tick,
    /// Time to draw a frame.
    Render,
}

/// Backend terminal type used throughout the app.
pub type Backend = CrosstermBackend<Stdout>;

/// Owns the ratatui terminal and the spawned event task.
pub struct Tui {
    /// The ratatui terminal over a crossterm backend on stdout.
    pub terminal: Terminal<Backend>,
    /// Receiver end of the unified event channel.
    event_rx: mpsc::UnboundedReceiver<Event>,
    /// Sender kept so the task can be (re)spawned; held for lifetime symmetry.
    event_tx: mpsc::UnboundedSender<Event>,
    /// Handle to the spawned select! task.
    task: Option<JoinHandle<()>>,
}

impl Tui {
    /// Build the terminal and start the event task. Does NOT enter raw mode;
    /// call [`Tui::enter`] for that.
    pub fn new() -> Result<Self> {
        let backend = CrosstermBackend::new(io::stdout());
        let terminal = Terminal::new(backend)?;
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        let mut tui = Self {
            terminal,
            event_rx,
            event_tx,
            task: None,
        };
        tui.spawn_event_task();
        Ok(tui)
    }

    /// Spawn the multiplexing event task.
    fn spawn_event_task(&mut self) {
        let tx = self.event_tx.clone();
        let render_period = std::time::Duration::from_millis(1000 / RENDER_FPS);
        let tick_period = std::time::Duration::from_millis(1000 / TICK_HZ);

        let handle = tokio::spawn(async move {
            let mut reader = EventStream::new();
            let mut render_interval = tokio::time::interval(render_period);
            let mut tick_interval = tokio::time::interval(tick_period);

            loop {
                let crossterm_event = reader.next();

                tokio::select! {
                    _ = render_interval.tick() => {
                        if tx.send(Event::Render).is_err() {
                            break;
                        }
                    }
                    _ = tick_interval.tick() => {
                        if tx.send(Event::Tick).is_err() {
                            break;
                        }
                    }
                    maybe_event = crossterm_event => {
                        match maybe_event {
                            Some(Ok(CrosstermEvent::Key(key))) => {
                                // Ignore key-release events (Windows emits them).
                                if key.kind == KeyEventKind::Press
                                    && tx.send(Event::Key(key)).is_err()
                                {
                                    break;
                                }
                            }
                            Some(Ok(CrosstermEvent::Resize(w, h))) => {
                                if tx.send(Event::Resize(w, h)).is_err() {
                                    break;
                                }
                            }
                            Some(Ok(_)) => {}
                            // Stream error or end: stop the task.
                            Some(Err(_)) | None => break,
                        }
                    }
                }
            }
        });

        self.task = Some(handle);
    }

    /// Await the next event from the channel.
    pub async fn next(&mut self) -> Option<Event> {
        self.event_rx.recv().await
    }

    /// Enter raw mode + alternate screen and hide the cursor.
    pub fn enter(&mut self) -> Result<()> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        stdout.execute(EnterAlternateScreen)?;
        stdout.execute(cursor::Hide)?;
        self.terminal.clear()?;
        Ok(())
    }

    /// Leave the alternate screen, disable raw mode and restore the cursor.
    /// Idempotent — safe to call multiple times and from the panic hook.
    pub fn exit(&mut self) -> Result<()> {
        restore()?;
        if let Some(task) = self.task.take() {
            task.abort();
        }
        Ok(())
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        // Backstop: ensure the terminal is restored even if exit() was missed.
        let _ = restore();
    }
}

/// Free-standing terminal restore used by BOTH `Tui::exit` and the panic hook.
/// Must be idempotent and must never panic.
pub fn restore() -> io::Result<()> {
    // Only act if we are actually in raw mode to avoid spurious escape writes.
    if crossterm::terminal::is_raw_mode_enabled().unwrap_or(false) {
        disable_raw_mode()?;
        let mut stdout = io::stdout();
        stdout.execute(LeaveAlternateScreen)?;
        stdout.execute(cursor::Show)?;
    }
    Ok(())
}
