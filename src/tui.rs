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
use std::sync::atomic::{AtomicBool, Ordering};

use color_eyre::Result;
use crossterm::cursor;
use crossterm::event::{
    Event as CrosstermEvent, EventStream, KeyEvent, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement,
    EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Global flag set when the kitty keyboard protocol (KKP) was successfully
/// pushed at terminal-enter time. The free-standing [`restore`] function
/// consults this so the panic-hook restore path pops KKP back off — without
/// it, a crash inside the render loop would leave the host terminal in
/// "KKP-on" mode after process exit, affecting subsequent shell input
/// rendering (e.g. arrow keys printing as escape sequences in some
/// configurations).
///
/// `AtomicBool` rather than a `Mutex<bool>` because the panic-hook path
/// must be allocation-free and lock-free — `Ordering::SeqCst` is fine here;
/// the flag is touched once at startup and once at shutdown.
///
/// Pub(crate) read access via [`mark_kkp_active`] so the kitty backend
/// (which uses its OWN raw-mode setup, not `Tui::enter`) can also mark
/// the flag and benefit from the same panic-hook pop on crash.
static KKP_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Crate-visible setter for `KKP_ACTIVE`. Used by the kitty backend
/// (`kitty::run_kitty`) which has its own raw-mode lifecycle and pushes
/// KKP outside `Tui::enter`. Calling with `true` ensures the panic-hook
/// `restore()` path pops KKP for both backends. Calling with `false`
/// (after the cleanup-pop completes inside `run_kitty`) keeps `restore`
/// from issuing a duplicate pop.
pub(crate) fn mark_kkp_active(active: bool) {
    KKP_ACTIVE.store(active, Ordering::SeqCst);
}

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
            // If the consumer falls behind (a heavy frame), DROP the missed ticks
            // instead of bursting catch-up ticks. Without this, a slow render lets
            // Render/Tick events flood the unbounded channel and key events (incl.
            // quit) get stuck behind a growing backlog — the app stops responding
            // to q/Esc/Ctrl-C.
            render_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            tick_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

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
                                // 05-05-RV7: forward ALL KeyEventKinds —
                                // Press, Repeat, AND Release. The held-set
                                // tracker in `App::run` flips inserts/removes
                                // on Press/Release; `Action::from_key`
                                // filters Repeat for discrete keys
                                // (so holding `P` cycles palette once) while
                                // keeping Repeat for continuous Nudge axes
                                // (the OS-repeat fallback on terminals
                                // without KKP). Forwarding Release here is
                                // the load-bearing change vs pre-RV7, which
                                // silently dropped Release at this layer
                                // and made held-set tracking impossible.
                                if tx.send(Event::Key(key)).is_err() {
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

    /// Non-blocking poll for an already-queued event. The main loop drains the
    /// channel with this so a backlog of Render/Tick events is collapsed in one
    /// pass (and a pending quit key is reached immediately) instead of rendering
    /// every queued frame.
    pub fn try_next(&mut self) -> Option<Event> {
        self.event_rx.try_recv().ok()
    }

    /// Enter raw mode + alternate screen and hide the cursor.
    ///
    /// 05-05-RV7: also attempts to push the kitty keyboard protocol (KKP)
    /// enhancement flags. KKP delivers `KeyEventKind::Press`,
    /// `KeyEventKind::Repeat`, AND `KeyEventKind::Release` (pre-KKP
    /// terminals deliver only Press for most keys), which is the
    /// load-bearing capability for held-set-driven continuous nudges:
    /// without Release we can't tell when the user lifts the key, so the
    /// held set would accumulate stuck entries.
    ///
    /// Behavior on terminals WITHOUT KKP (alacritty, xterm, most ssh
    /// sessions): the push is a noop (the terminal ignores the
    /// `CSI > 1 u` push) and `supports_keyboard_enhancement()` returns
    /// false — we leave `KKP_ACTIVE` as false and the render loop falls
    /// back to the OS-repeat path (Press + Repeat both route to
    /// `Action::Nudge*`, coalesced once per frame). Worst case: same
    /// behavior as pre-RV7 (the user's original lag complaint stays,
    /// but ONLY on terminals without KKP).
    ///
    /// Behavior on terminals WITH KKP (kitty, ghostty, WezTerm):
    /// `KKP_ACTIVE` flips true; the render loop uses
    /// `HeldAction::from_key_code` on every Press/Release to drive the
    /// held set; the OS-repeat initial delay is bypassed entirely.
    ///
    /// The flag combination is `DISAMBIGUATE_ESCAPE_CODES |
    /// REPORT_EVENT_TYPES`. `REPORT_EVENT_TYPES` is the one that
    /// surfaces Release events — that's the only flag we strictly
    /// need. `DISAMBIGUATE_ESCAPE_CODES` is added because it improves
    /// the handling of Esc / Shift+Tab / function keys in many
    /// terminals (no behavior regression in our key map) and is widely
    /// supported alongside event-types.
    pub fn enter(&mut self) -> Result<()> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        stdout.execute(EnterAlternateScreen)?;
        stdout.execute(cursor::Hide)?;
        // KKP: best-effort push. Detect first so we don't push to a
        // terminal that won't honor pop on shutdown — the
        // supports_keyboard_enhancement() roundtrip is the canonical
        // detection (sends `CSI ? u`, waits for `CSI ? <flags> u`
        // response). Failure to detect (e.g. dumb pipe, ssh without
        // tty) silently leaves KKP off.
        if supports_keyboard_enhancement().unwrap_or(false) {
            let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_EVENT_TYPES;
            if stdout.execute(PushKeyboardEnhancementFlags(flags)).is_ok() {
                KKP_ACTIVE.store(true, Ordering::SeqCst);
            }
        }
        self.terminal.clear()?;
        Ok(())
    }

    /// Returns true iff `enter()` successfully pushed the kitty keyboard
    /// protocol (and the host terminal honored it). Consumed by the
    /// render loop to decide whether to drive nudges from the held-set
    /// (KKP path) or to fall back to the OS-repeat → Action::Nudge*
    /// dispatch path.
    pub fn kkp_active(&self) -> bool {
        KKP_ACTIVE.load(Ordering::SeqCst)
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
///
/// 05-05-RV7: also pops kitty keyboard protocol (KKP) flags if they were
/// pushed at `Tui::enter`. Pairing is REQUIRED — leaving KKP active after
/// process exit makes the host shell render arrow keys as escape sequences
/// in some terminal configurations. The `KKP_ACTIVE` global is the single
/// source of truth for "did we push, do we need to pop"; flipping it
/// false here makes restore idempotent (subsequent calls noop the pop).
pub fn restore() -> io::Result<()> {
    // Only act if we are actually in raw mode to avoid spurious escape writes.
    if crossterm::terminal::is_raw_mode_enabled().unwrap_or(false) {
        // Pop KKP FIRST so the pop escape is consumed under raw mode
        // (where the terminal can parse it cleanly) rather than ending
        // up echoed on the cooked-mode shell prompt after `disable_raw`.
        if KKP_ACTIVE.swap(false, Ordering::SeqCst) {
            let mut stdout = io::stdout();
            // PopKeyboardEnhancementFlags is the only safe way to undo a
            // matched push; sending raw `CSI < u` would skip crossterm's
            // internal state-tracking.
            let _ = stdout.execute(PopKeyboardEnhancementFlags);
        }
        disable_raw_mode()?;
        let mut stdout = io::stdout();
        stdout.execute(LeaveAlternateScreen)?;
        stdout.execute(cursor::Show)?;
    } else {
        // Even if raw mode is somehow already disabled (panic mid-shutdown),
        // we must still clear the KKP flag and pop if necessary — the host
        // terminal otherwise retains the enhanced kbd state across our exit.
        if KKP_ACTIVE.swap(false, Ordering::SeqCst) {
            let mut stdout = io::stdout();
            let _ = stdout.execute(PopKeyboardEnhancementFlags);
        }
    }
    Ok(())
}
