//! App state + the main event loop.
//!
//! The loop is the single consumer of the unified `Event` channel. It mutates
//! `App` in place and calls the synchronous `terminal.draw(...)` only on
//! `Event::Render` (ARCHITECTURE Pattern 1). The draw closure reads `&self`
//! only and never `.await`s (ARCHITECTURE Anti-Pattern 2).
//!
//! ## Live data plumbing (03-04)
//!
//! The app also drains a non-blocking `mpsc::UnboundedReceiver<DockerMsg>` once
//! per outer loop iteration and feeds messages into a `LiveWorld` reconciler.
//! When the reconciler produces a rebuilt `World`, the app swaps it in. The
//! receiver is `try_recv`-only — no Docker calls happen on the render path,
//! and the channel drain runs at the same place that drains the event queue,
//! so a quiet daemon doesn't cost any extra wakeups (DOCK-04 cadence
//! decoupling, Pitfall 3 idle CPU).
//!
//! The camera is re-framed ONLY when the entity COUNT changes (containers
//! created/destroyed). Pure stats updates that just resize an existing box
//! never re-frame — that would jitter the view every second as samples land.

use std::time::Instant;

use color_eyre::Result;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::action::{apply_input_action, Action, Effect};
use crate::camera::{Camera, SPIN_RATE};
use crate::config::RenderConfig;
use crate::tui::{Event, Tui};
use crate::ui;
use crate::world::scene::SceneBounds;
use crate::world::selection::Selection;
use crate::world::{self, DockerMsg, LiveWorld, World};

/// All application state. Mutated only by the main loop.
pub struct App {
    /// Set true to break the event loop and quit.
    pub should_quit: bool,
    /// Live terminal size in cells (width, height).
    pub size: (u16, u16),
    /// Number of logic ticks elapsed — animation hook for later plans.
    pub tick_count: u64,
    /// Timestamp of the previous rendered frame (for fps / dt).
    last_render: Instant,
    /// Most recent measured frames-per-second.
    pub fps: f32,
    /// Timestamp of the previous logic tick (for dt).
    last_tick: Instant,
    /// Static framing camera holding a fixed 3/4 view of the rack (the orbit was
    /// disabled in the verify-tuning pass — see [`crate::camera`]).
    pub camera: Camera,
    /// Per-box self-spin angle (radians), advanced on the logic tick. The camera
    /// is static; this is the scene's motion — each box spins in place about its
    /// own +Y axis. Framerate-independent (advanced by real `dt`).
    pub spin: f32,
    /// Rendering knobs (cell_aspect, near/far). The camera owns the fov.
    pub render_config: RenderConfig,
    /// The live datacenter scene — the SINGLE source of truth for what the
    /// braille UI renders. With a live producer it STARTS EMPTY (the in-scene
    /// banner kicks in until the first `DockerMsg::Added` lands); with the
    /// test/dump constructor it is seeded from `synthetic_scene()` so legacy
    /// tests still pass.
    pub world: World,
    /// Live reconciler that turns DockerMsg into the World shape the renderer
    /// already consumes. Owns the stable per-id slot scheme (CONT-05).
    pub(crate) live: LiveWorld,
    /// Tab-cycle selection + brightness pulse + detail-panel toggle (04-03).
    /// Mutated by `update()` (via `apply_input_action`) and by
    /// `on_tick`/`drain_docker` (pulse advance + reconcile-on-remove).
    pub selection: Selection,
    /// Non-blocking receiver for typed DockerMsg events from the producer task
    /// in `docker::streams`. `None` for the test/dump constructor (`App::new`)
    /// so unit tests don't need a producer.
    docker_rx: Option<UnboundedReceiver<DockerMsg>>,
}

impl App {
    /// Construct fresh app state for tests / the offline dump path.
    ///
    /// Seeds the world from the deterministic `synthetic_scene()` so anything
    /// that reads `app.world` (the braille view, the `boxes:` count in the
    /// status bar) has something to display when no live producer is wired.
    /// Used by the test suite and `Default` only — the real binary always
    /// goes through `with_docker_rx`.
    pub fn new() -> Self {
        let now = Instant::now();
        // Build the synthetic scene once and frame the camera to its bounds so
        // tests / offline dumps see the full rack.
        let world = world::synthetic_scene();
        let mut camera = Camera::new();
        camera.frame_scene(&world);
        Self {
            should_quit: false,
            size: (0, 0),
            tick_count: 0,
            last_render: now,
            fps: 0.0,
            last_tick: now,
            camera,
            spin: 0.0,
            render_config: RenderConfig::default(),
            world,
            live: LiveWorld::new(),
            selection: Selection::new(),
            docker_rx: None,
        }
    }

    /// Construct fresh app state with a live Docker receiver.
    ///
    /// The world starts EMPTY — the renderer shows the in-scene "no containers"
    /// banner until the first reconciled message lands. We do NOT pre-frame
    /// the camera here because there are no bounds to frame to yet; the camera
    /// is framed lazily by the first non-empty reconcile (and again whenever
    /// the entity count changes — see `run`).
    pub fn with_docker_rx(rx: UnboundedReceiver<DockerMsg>) -> Self {
        let now = Instant::now();
        Self {
            should_quit: false,
            size: (0, 0),
            tick_count: 0,
            last_render: now,
            fps: 0.0,
            last_tick: now,
            camera: Camera::new(),
            spin: 0.0,
            render_config: RenderConfig::default(),
            world: World {
                entities: Vec::new(),
                bounds: SceneBounds::from_entities(&[]),
            },
            live: LiveWorld::new(),
            selection: Selection::new(),
            docker_rx: Some(rx),
        }
    }

    /// Dispatch a high-level intent to a state mutation.
    ///
    /// Routes through [`apply_input_action`] — the SINGLE dispatch surface
    /// both backends use (04-03). The returned [`Effect`] tells `update` what
    /// the dispatch surface couldn't do itself: Quit sets `should_quit`,
    /// SpawnInspect would fire the off-thread inspect (04-06 plugs that in).
    pub fn update(&mut self, action: Action) {
        let effect = apply_input_action(
            action,
            &mut self.camera,
            &mut self.selection,
            &self.world,
            &self.live,
        );
        match effect {
            Effect::Quit => self.should_quit = true,
            Effect::SpawnInspect(_id) => {
                // 04-06 plugs the docker::inspect::fetch_detail spawn here.
                // `selection.detail_open` is already set by apply_input_action.
            }
            Effect::None => {}
        }
    }

    /// Handle a terminal resize. Later plans recompute the 3D viewport here.
    pub fn on_resize(&mut self, w: u16, h: u16) {
        self.size = (w, h);
    }

    /// Advance animation by real elapsed time. `dt` is seconds since the last
    /// tick so the orbit stays framerate-independent (Gaffer decoupling): the
    /// camera advances by REAL elapsed time, not per-frame.
    pub fn on_tick(&mut self, dt: f32) {
        self.tick_count = self.tick_count.wrapping_add(1);
        self.camera.step(dt); // static framing now (no orbit)
        // Advance the per-box self-spin by REAL elapsed time, wrapped to [0, TAU).
        let dt = if dt.is_finite() && dt > 0.0 { dt } else { 0.0 };
        self.spin = (self.spin + SPIN_RATE * dt).rem_euclid(std::f32::consts::TAU);
        // Per-frame size easing (CONT-03 / 04-01). Mutates `half_extents` in
        // place on the live world's entity slice — no World reallocation per
        // tick (RESEARCH Pitfall A). dt is REAL elapsed time; `on_tick` is
        // the canonical place for framerate-independent motion. Empty world
        // is a no-op (zero-length slice).
        self.live.dress(dt, &mut self.world.entities);
        // Brightness-pulse phase advance for the selected box (04-03 CAM-04).
        // Same dt source as dress() so the pulse is framerate-independent.
        self.selection.tick(dt);
    }

    /// Drain everything currently queued on the Docker channel (non-blocking)
    /// and reconcile through the LiveWorld. Returns `true` if the entity COUNT
    /// changed — the caller uses that to re-frame the camera. The COUNT trigger
    /// is intentional: pure stat updates that resize an existing box never
    /// re-frame, which would jitter the view every second as samples arrive.
    ///
    /// No Docker call ever happens here — `try_recv` reads from an in-process
    /// mpsc queue, so this is bounded by the message rate (~1/sec per running
    /// container for stats) and cheap to run every loop pass.
    pub(crate) fn drain_docker(&mut self) -> bool {
        let Some(rx) = self.docker_rx.as_mut() else {
            return false;
        };
        let before = self.world.entities.len();
        let mut rebuilt = false;
        while let Ok(msg) = rx.try_recv() {
            if let Some(w) = self.live.apply(msg) {
                self.world = w;
                rebuilt = true;
            }
        }
        let count_changed = rebuilt && self.world.entities.len() != before;
        if count_changed {
            // A container disappeared — if it was the selected one, fall back
            // to the first remaining (or None when empty). Other state on
            // Selection is unchanged.
            self.selection.reconcile(&self.world);
        }
        count_changed
    }

    /// Run the main event loop until `should_quit`.
    pub async fn run(&mut self, tui: &mut Tui) -> Result<()> {
        // Seed the live size from the terminal so the first frame is correct.
        let area = tui.terminal.size()?;
        self.size = (area.width, area.height);

        while let Some(event) = tui.next().await {
            // Drain everything already queued in one pass. Input/resize/tick are
            // applied immediately; Render is COALESCED to a single draw at the end.
            // This keeps the loop responsive to quit keys even if a heavy frame let
            // a backlog build up — we never render the backlog frame-by-frame.
            let mut render_requested = false;
            let mut next = Some(event);
            while let Some(ev) = next {
                match ev {
                    Event::Key(key) => self.update(Action::from_key(key)),
                    Event::Resize(w, h) => self.on_resize(w, h),
                    Event::Tick => {
                        let now = Instant::now();
                        let dt = now.duration_since(self.last_tick).as_secs_f32();
                        self.last_tick = now;
                        self.on_tick(dt);
                    }
                    Event::Render => render_requested = true,
                }
                if self.should_quit {
                    return Ok(());
                }
                next = tui.try_next();
            }

            // Reconcile any pending Docker messages BEFORE rendering. This runs
            // once per outer loop iteration, never on a tight inner loop. If
            // the entity COUNT changed, re-frame the camera so the new rack
            // (post-add/remove) is fully in view; pure stat updates do not
            // re-frame (avoids per-sample camera jitter).
            if self.drain_docker()
                && !self.world.entities.is_empty()
                && self.camera.autopilot_active
            {
                // Manual-mode driver must NOT be yanked back by every container
                // add/remove — only auto-reframe while the user hasn't taken
                // over yet (04-03 CAM-03).
                self.camera.frame_scene(&self.world);
            }

            if render_requested {
                let now = Instant::now();
                let dt = now.duration_since(self.last_render).as_secs_f32();
                self.last_render = now;
                if dt > 0.0 {
                    // Smooth the fps reading a little to avoid jitter.
                    let instant_fps = 1.0 / dt;
                    self.fps = if self.fps == 0.0 {
                        instant_fps
                    } else {
                        self.fps * 0.9 + instant_fps * 0.1
                    };
                }
                tui.terminal.draw(|frame| ui::view(frame, self))?;
            }
        }

        Ok(())
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docker::ContainerSnapshot;
    use crate::theme::Status;

    fn snap(id: &str) -> ContainerSnapshot {
        ContainerSnapshot {
            id: id.to_string(),
            name: id.to_string(),
            status: Status::Running,
            group_key: "net0".to_string(),
            ..ContainerSnapshot::default()
        }
    }

    /// `App::with_docker_rx` starts EMPTY so the empty-state banner can fire
    /// before the first Added message lands. The synthetic scene is reserved
    /// for `App::new()` (tests / dump path).
    #[test]
    fn with_docker_rx_starts_empty() {
        let (_tx, rx) = tokio::sync::mpsc::unbounded_channel::<DockerMsg>();
        let app = App::with_docker_rx(rx);
        assert!(
            app.world.entities.is_empty(),
            "live app must start empty so the empty-state banner is reached"
        );
    }

    /// drain_docker applies channel messages through LiveWorld and rebuilds the
    /// World — the consumer-side wiring that makes the live scene reflect real
    /// containers without per-frame Docker calls.
    #[test]
    fn drain_docker_applies_messages_and_rebuilds_world() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<DockerMsg>();
        let mut app = App::with_docker_rx(rx);
        assert!(app.world.entities.is_empty());

        tx.send(DockerMsg::Added(snap("a"))).unwrap();
        tx.send(DockerMsg::Added(snap("b"))).unwrap();
        let count_changed = app.drain_docker();
        assert!(count_changed, "two Added messages must change the count");
        assert_eq!(app.world.entities.len(), 2);
    }

    /// drain_docker returns false when no messages are pending — so the run
    /// loop never re-frames the camera unnecessarily on a quiet channel.
    #[test]
    fn drain_docker_returns_false_on_empty_channel() {
        let (_tx, rx) = tokio::sync::mpsc::unbounded_channel::<DockerMsg>();
        let mut app = App::with_docker_rx(rx);
        assert!(!app.drain_docker());
    }

    /// drain_docker returns false when only Stat messages flow (count is the
    /// same) — re-framing the camera every second on stat samples would jitter
    /// the view. Pin the cadence-decoupling invariant.
    #[test]
    fn drain_docker_stat_only_returns_false_no_recount() {
        use crate::docker::stats::StatSample;
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<DockerMsg>();
        let mut app = App::with_docker_rx(rx);

        // Seed one container so a Stat can land.
        tx.send(DockerMsg::Added(snap("a"))).unwrap();
        assert!(app.drain_docker(), "first Added changes count");

        // Now push a non-warming-up stat: world rebuilds but entity count is
        // identical, so the count-change detector must return false.
        let sample = StatSample {
            cpu_pct: 10.0,
            mem_used: 0,
            mem_limit: 0,
            mem_fraction: 0.1,
            load: 0.5,
            warming_up: false,
            blkio_r_bytes: 0,
            blkio_w_bytes: 0,
        };
        tx.send(DockerMsg::Stat("a".to_string(), sample)).unwrap();
        assert!(
            !app.drain_docker(),
            "Stat-only drain must not flag a count change"
        );
    }

    /// Removed message brings the world back to 0 entities — the empty-state
    /// banner re-engages without panicking on degenerate bounds.
    #[test]
    fn drain_docker_removal_empties_world() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<DockerMsg>();
        let mut app = App::with_docker_rx(rx);
        tx.send(DockerMsg::Added(snap("only"))).unwrap();
        app.drain_docker();
        tx.send(DockerMsg::Removed("only".to_string())).unwrap();
        let changed = app.drain_docker();
        assert!(changed, "Removed of the last container must change count");
        assert!(app.world.entities.is_empty());
        // Bounds must be finite even when empty — the renderer relies on this.
        assert!(app.world.bounds.center.x.is_finite());
        assert!(app.world.bounds.radius.is_finite());
    }

    /// App::update dispatches Action::SelectNext through apply_input_action and
    /// the Selection moves to the lowest-id entity in the world.
    #[test]
    fn app_update_dispatches_select_next_changes_selection() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<DockerMsg>();
        let mut app = App::with_docker_rx(rx);
        tx.send(DockerMsg::Added(snap("a"))).unwrap();
        tx.send(DockerMsg::Added(snap("b"))).unwrap();
        app.drain_docker();
        assert!(app.selection.selected_id.is_none());

        app.update(Action::SelectNext);
        // First slot in group net0 has entity id 0 (0 << 16 | 0).
        assert_eq!(app.selection.selected_id, Some(0));
    }

    /// First camera-key dispatch flips autopilot to manual.
    #[test]
    fn app_update_nudge_yaw_flips_autopilot_off() {
        let app_init = App::new();
        assert!(app_init.camera.autopilot_active);
        let mut app = app_init;
        app.update(Action::NudgeYaw(0.1));
        assert!(
            !app.camera.autopilot_active,
            "first camera-key Action must flip autopilot off"
        );
    }

    /// Removing the selected container moves the selection to the next live
    /// entity via Selection::reconcile (called inside drain_docker on count
    /// change).
    #[test]
    fn app_drain_docker_runs_reconcile_on_count_change() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<DockerMsg>();
        let mut app = App::with_docker_rx(rx);
        tx.send(DockerMsg::Added(snap("a"))).unwrap();
        tx.send(DockerMsg::Added(snap("b"))).unwrap();
        app.drain_docker();
        // Select the FIRST container (entity id 0).
        app.update(Action::SelectNext);
        assert_eq!(app.selection.selected_id, Some(0));

        // Removing "a" empties slot 0 — entity id 0 disappears; reconcile
        // moves the selection to whatever entity is first in the rebuilt
        // World (which is now b at slot 1, entity id 1).
        tx.send(DockerMsg::Removed("a".to_string())).unwrap();
        app.drain_docker();
        assert_eq!(
            app.selection.selected_id,
            Some(1),
            "selection must reconcile to the surviving entity (b at slot 1)"
        );
    }

    /// Effect::Quit path is observable through update().
    #[test]
    fn app_update_quit_sets_should_quit() {
        let mut app = App::new();
        assert!(!app.should_quit);
        app.update(Action::Quit);
        assert!(app.should_quit, "Action::Quit must set should_quit via Effect");
    }

    /// Esc with no panel open quits via the Effect::Quit branch.
    #[test]
    fn app_update_esc_with_no_panel_quits() {
        let mut app = App::new();
        assert!(!app.selection.detail_open);
        app.update(Action::CloseDetail);
        assert!(
            app.should_quit,
            "Esc with no panel must surface Effect::Quit through update()"
        );
    }
}
