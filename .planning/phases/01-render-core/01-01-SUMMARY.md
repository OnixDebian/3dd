---
phase: 01-render-core
plan: 01
subsystem: infra
tags: [rust, ratatui, crossterm, tokio, glam, color-eyre, tui, async, panic-hook]

# Dependency graph
requires: []
provides:
  - "Runnable cargo binary (dd3) opening an alt-screen TUI"
  - "tokio::select! event task multiplexing input + render tick + logic tick into mpsc<Event>"
  - "Event enum { Key, Resize, Tick, Render } and Action enum { Quit, None }"
  - "Tui lifecycle (enter/exit) + standalone restore() reused by the panic hook"
  - "App state + event loop drawing only on Event::Render; sync read-only view()"
  - "Placeholder bordered 'scene' block + status bar (fps / size / quit hint)"
  - "Capped frame pacing (30 FPS render, 60Hz logic) — no spin loop"
affects: [02-3d-pipeline, 04-animation, 05-theming]

# Tech tracking
tech-stack:
  added: [ratatui 0.30, crossterm 0.29, tokio 1.52 (full), glam 0.33, color-eyre 0.6, futures 0.3]
  patterns:
    - "Two-channel single-consumer loop (async producer -> sync render) [ARCH Pattern 1]"
    - "Interval-driven frame pacing via tokio::select! (no busy loop) [Pitfall #3]"
    - "Standalone restore() shared by Tui::exit and the panic hook [Pitfall #11]"
    - "Action intent layer decoupling key bindings from state mutation"
    - "Layout from live frame.area(), no cached size in the draw path [Pitfall #14]"
    - "dt from real Instant deltas for framerate-independent animation [Gaffer]"

key-files:
  created:
    - Cargo.toml
    - src/main.rs
    - src/tui.rs
    - src/action.rs
    - src/app.rs
    - src/ui/mod.rs
    - src/ui/status_bar.rs
  modified: []

key-decisions:
  - "Binary crate named dd3 (Cargo package name; the project/repo is '3dd')"
  - "Added futures 0.3 for StreamExt::next on crossterm EventStream"
  - "tokio resolved to 1.52.x (latest 1.x satisfying ~1.47 semver)"
  - "Render 30 FPS / logic tick 60Hz as the default cadence"
  - "restore() guarded by is_raw_mode_enabled to stay idempotent + panic-safe"

patterns-established:
  - "Pattern 1: tokio::select! event task -> mpsc<Event> single-consumer main loop"
  - "Pattern 2: terminal restore extracted to a free fn reused by the panic hook"
  - "Pattern 3: pure synchronous view(frame, &App) laying out from frame.area()"

# Metrics
duration: 2min
completed: 2026-05-26
---

# Phase 1 Plan 01: App Skeleton Summary

**Panic-safe async ratatui+tokio TUI skeleton: a tokio::select! event task feeds an mpsc<Event> single-consumer loop that draws a placeholder scene + status bar at a capped 30 FPS, quits on q/Esc/Ctrl-C, and restores the terminal on both clean exit and panic.**

## Performance

- **Duration:** 2 min (active execution; excludes interactive terminal verification, which requires a real TTY)
- **Started:** 2026-05-26T21:35:25Z
- **Completed:** 2026-05-26T21:38:11Z
- **Tasks:** 3
- **Files created:** 7

## Accomplishments
- Crate scaffolded with the full project dependency baseline pinned (ratatui 0.30, crossterm 0.29, tokio 1.47+, glam 0.33, color-eyre 0.6)
- Terminal-restoring panic hook installed before raw mode (Pitfall #11): a deliberate panic restores the terminal before the report prints
- Canonical ratatui-async loop: a single `tokio::select!` task multiplexes crossterm `EventStream` + a 30 FPS render interval + a 60Hz logic interval into one `mpsc<Event>`, so idle CPU never spins (Pitfall #3)
- App state + event loop draws only on `Event::Render` via a sync, read-only `view()`; layout derives from live `frame.area()` so resize never garbles (Pitfall #14)
- Quit mapping (q / Esc / Ctrl-C -> `Action::Quit`) and a status bar showing live fps + terminal size

## Task Commits

Each task was committed atomically:

1. **Task 1: Scaffold crate and pin dependencies** - `830024b` (feat)
2. **Task 2: Terminal lifecycle + tokio::select! event task** - `599b05a` (feat)
3. **Task 3: App state, event loop, resize handling, placeholder UI** - `aec827b` (feat)

**Plan metadata:** `docs(01-01): complete app-skeleton plan` (this commit)

## Files Created
- `Cargo.toml` - Crate manifest; binary `dd3`; pinned core deps + `futures`
- `src/main.rs` - tokio entry point; installs color-eyre + terminal-restoring panic hook; builds Tui, runs App, restores on every exit path
- `src/tui.rs` - `Tui` (owns ratatui `Terminal`), `enter()`/`exit()`, standalone idempotent `restore()`, `Event` enum, and the `tokio::select!` event task (`RENDER_FPS=30`, `TICK_HZ=60`)
- `src/action.rs` - `Action { Quit, None }` + `from_key` (q/Esc/Ctrl-C -> Quit)
- `src/app.rs` - `App` state, `update(action)` dispatcher, `on_resize`, `on_tick(dt)`, and `run()` main loop (draw only on Render; dt from real `Instant` deltas)
- `src/ui/mod.rs` - pure `view(frame, &app)`; vertical split into bordered "scene" placeholder + 1-row status bar
- `src/ui/status_bar.rs` - `render()` Paragraph: "3dd | fps | size | q to quit"

## Key API surface (for plans 02/04/05)
- **Wire the Canvas:** replace the inner content of the bordered block in `ui::view` (the `scene_area` chunk in `src/ui/mod.rs`).
- **Wire the orbit/animation:** advance camera + world in `App::on_tick(&mut self, dt: f32)` (`src/app.rs`); `dt` is real elapsed seconds.
- **Add input intents:** extend `Action` + `Action::from_key` (`src/action.rs`); dispatch in `App::update`.
- **Add data channel:** the loop is a single consumer; add a second `mpsc` receiver and `select!` over both per ARCH Pattern 1 (Docker layer, Phase 3).
- **Rates:** `tui::RENDER_FPS = 30`, `tui::TICK_HZ = 60` (public constants).
- **Restore API:** `tui::restore()` is a free, idempotent fn (used by `Tui::exit`, `Drop`, and the panic hook).

## Decisions Made
- **Cargo package name `dd3`** — `3dd` is not a valid Rust crate name (cannot start with a digit). The project/repo name stays "3dd"; the binary is `dd3`.
- **Added `futures` 0.3** (not in the plan's dep list) — required for `StreamExt::next()` on crossterm's `EventStream`. Critical to the select! integration.
- **tokio resolved to 1.52.x** — `~1.47` semver means `>=1.47, <2.0`; cargo picked the latest 1.x patch. Satisfies the plan's intent (pinned to a 1.47+ baseline).
- **Render 30 FPS / logic 60Hz** as defaults (ARCH Frame Timing table; 30 FPS is smooth and CPU-cheap in a terminal).
- **`restore()` guards on `is_raw_mode_enabled()`** so it is safe to call repeatedly (exit, Drop, panic hook) without emitting spurious escape sequences.
- **Key-release events ignored** (`KeyEventKind::Press` only) to avoid double-fires on terminals that report releases.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Added the `futures` crate for EventStream consumption**
- **Found during:** Task 2 (event task)
- **Issue:** crossterm `EventStream` yields a `Stream`; `.next().await` requires a `StreamExt` trait in scope, which the plan's dependency list did not provide.
- **Fix:** Added `futures = "0.3"` to Cargo.toml and imported `futures::StreamExt`.
- **Files modified:** Cargo.toml, src/tui.rs
- **Verification:** `cargo build` clean.
- **Committed in:** `830024b` (Cargo.toml) / `599b05a` (tui.rs)

**2. [Rule 3 - Blocking] Crate named `dd3` instead of `3dd`**
- **Found during:** Task 1 (scaffold)
- **Issue:** Rust crate names cannot begin with a digit, so `3dd` is rejected by cargo.
- **Fix:** Named the package/binary `dd3`; project identity "3dd" preserved in description + repo.
- **Files modified:** Cargo.toml
- **Verification:** `cargo build` produces `target/debug/dd3`.
- **Committed in:** `830024b`

**3. [Rule 1 - Bug/Lint] Collapsed a nested `if` flagged by clippy**
- **Found during:** verification (`cargo clippy`)
- **Issue:** `clippy::collapsible_if` warning in the event task; the success criteria require clippy clean.
- **Fix:** Merged `if key.kind == Press { if tx.send(..).is_err() { break } }` into a single `&&` condition.
- **Files modified:** src/tui.rs
- **Verification:** `cargo clippy` reports no warnings.
- **Committed in:** `599b05a`

**4. [Rule 2 - Missing Critical] `EventStream` requires the crossterm `event-stream` feature**
- **Found during:** Task 1/2
- **Issue:** `crossterm::event::EventStream` is gated behind the `event-stream` cargo feature; without it the async input source does not exist.
- **Fix:** Enabled `features = ["event-stream"]` on the crossterm dependency.
- **Files modified:** Cargo.toml
- **Verification:** `cargo build` clean; `EventStream::new()` resolves.
- **Committed in:** `830024b`

---

**Total deviations:** 4 auto-fixed (3 blocking, 1 lint). No architectural changes; no scope creep.
**Impact on plan:** All four were necessary to make the planned design compile and meet the "clippy clean" criterion. The architecture matches the plan exactly.

## Issues Encountered
- **No interactive verification possible in this environment.** The execution sandbox has no controlling TTY (`test -t 0`/`test -t 1` both false), so the interactive verifications in the plan — alt-screen render, live resize, quit-restores-terminal, idle CPU soak, deliberate-panic-restores-terminal — could not be exercised here. A `cargo run` attempt failed precisely because raw mode / `EventStream` need a real terminal ("No such device or address" at `enable_raw_mode`). Notably, in that no-TTY failure the **panic hook still fired and the color-eyre report printed cleanly**, which is partial evidence the restore-before-report path works. The code follows the canonical ratatui-async pattern (ARCH Pattern 1) verbatim. **The host must run these checks in a real terminal** (see below).

## User Verification Required (real terminal)
Run from a real terminal (`cargo run`) and confirm:
- [ ] Alt-screen TUI shows a bordered "scene" block + a status bar with non-zero fps and live size.
- [ ] `q` / `Esc` / `Ctrl-C` quit; afterwards `echo test` echoes and the cursor is visible (terminal fully restored).
- [ ] Temporarily add `panic!("test")` in `App::on_tick` after a few ticks -> terminal restored, panic report legible -> then REMOVE it.
- [ ] Resize the window repeatedly -> no garbage, no panic.
- [ ] Idle ~30s -> `top`/`htop` shows single-digit % of one core (no pegged core).

## Next Phase Readiness
- The async loop + render cadence + panic/terminal safety are in place — the foundation every later plan builds on.
- Plan 02 (3D pipeline) can wire a braille `Canvas` into the `scene_area` and step the camera in `App::on_tick`.
- **Blocker/Concern:** interactive behaviors (CPU idle, resize, panic-restore) are coded to the canonical pattern but UNVERIFIED in this sandbox; they require a one-time manual check in a real terminal before relying on them in Phase 2.

---
*Phase: 01-render-core*
*Completed: 2026-05-26*
