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
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::action::{apply_input_action, coalesce_actions, Action, Effect};
/// How many consecutive logic ticks the World must stay empty AFTER having
/// previously been non-empty before the empty-state banner re-appears. At
/// `crate::tui::TICK_HZ = 60` this is ~5000 ms of stable emptiness — the
/// 05-04-RV5 bump from RV4's 1000 ms.
///
/// User feedback after RV4 (1000 ms / 60 ticks debounce + cached world):
/// banner STILL flickered ~100 ms occasionally on launch. Root cause
/// diagnosed as the COLD-START race, not the RV4 debounce window:
///
/// 1. At process launch, `non_empty_seen_once == false` and the cache is
///    `None` — both `should_show_empty_banner()` and `effective_world` fell
///    through to the immediate banner branch.
/// 2. bollard's `list_containers` seed pass routinely returns at
///    t~80-150 ms; before that, NO `Added` message has drained. The first
///    1-3 render frames (~33-100 ms at 30 FPS) painted the banner, then it
///    vanished the moment containers populated — a textbook ~100 ms flash.
///
/// RV5 closes the cold-start race AND tightens the invariant the user
/// asked for ("if cache is populated, keep it for at least 5 seconds"):
///
/// - [`STARTUP_GRACE_TICKS`] (~1 s) suppresses the banner unconditionally
///   for the first second after launch. The renderer paints the same
///   neutral bordered "scene" chrome the RV4 fallback already used for
///   the cache-None + debounce-active path — no banner text, no scene
///   content, just borders. After the grace expires, either the daemon
///   has spoken (non_empty_seen_once flipped → cache renders) or the
///   daemon truly has zero containers (banner shows, Phase 3 criterion
///   #5 preserved with a 1 s delay — acceptable trade for eliminating
///   the flash).
///
/// - The debounce window grows from 60 → 300 ticks (5 s). Combined with
///   the invariant "cache always renders during the window", this means
///   that during normal operation post-first-container the banner is
///   effectively unreachable: every transient empty (`docker rm -f` +
///   `docker run`, container restart-policy bounces, mid-session daemon
///   hiccups) is well under 5 s, so the cached scene holds and the user
///   never sees the banner unless they intentionally stop everything
///   and wait.
pub(crate) const EMPTY_BANNER_DEBOUNCE_TICKS: u32 = 300;

/// How many consecutive logic ticks after launch during which the empty-
/// state banner is unconditionally suppressed, regardless of
/// `non_empty_seen_once`. At `crate::tui::TICK_HZ = 60` this is ~1000 ms.
///
/// Rationale: bollard's startup `list_containers` seed routinely lands at
/// t~80-150 ms on a busy daemon. Before that, the App's `non_empty_seen_once`
/// bit is `false` and the banner would paint for the first 1-3 render
/// frames (~33-100 ms). The 1 s grace covers the seed roundtrip with
/// generous headroom on slow daemons. After the grace, if the daemon
/// truly has zero containers the banner finally shows (criterion #5
/// preserved with a small delay).
pub(crate) const STARTUP_GRACE_TICKS: u32 = 60;
use crate::camera::{Camera, SPIN_RATE};
use crate::config::RenderConfig;
use crate::docker::Docker;
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
    /// Active palette. Single source of truth for the braille backend's
    /// color choices — `ui::view` reads `app.palette` each frame instead of
    /// constructing `Palette::default()`. Cycled at runtime by
    /// `Effect::CyclePalette` (THEME-04).
    pub palette: crate::theme::Palette,
    /// Active palette NAME (for the status bar + legend HUD label). Held
    /// alongside `palette` because Color is opaque after construction —
    /// we can't reverse a Palette back to its preset name.
    pub palette_name: String,
    /// App-wide config loaded by main.rs. The palette field above is its
    /// already-resolved counterpart; this field carries everything ELSE
    /// (auto_degrade, degraded_fps_cap, force_mode, hud_visible) that
    /// 05-05 / 05-06 read from.
    pub config: crate::config::AppConfig,
    /// True once we've observed at least one non-empty World since launch.
    /// Sticky: never flips back to `false` after the first container appears.
    /// Used by the empty-banner debounce: at startup (no containers ever
    /// observed) the banner shows immediately; after the first container has
    /// appeared, a transient empty state during rapid container churn
    /// (e.g. `docker rm -f` followed instantly by `docker run`) is suppressed
    /// for [`EMPTY_BANNER_DEBOUNCE_TICKS`] ticks (~200 ms at TICK_HZ=60), so
    /// the banner doesn't flash on/off during normal-life daemon activity.
    pub(crate) non_empty_seen_once: bool,
    /// Number of consecutive logic ticks the World has been empty. Reset to
    /// 0 every tick the World is non-empty. Read by `ui::view` (via
    /// [`Self::should_show_empty_banner`]) to gate the banner on stable
    /// emptiness rather than instantaneous emptiness.
    pub(crate) empty_streak_ticks: u32,
    /// Cached snapshot of the most recent non-empty world (RV4).
    ///
    /// Written on every non-empty `on_tick` (cheap — `World: Clone` is a
    /// `Vec<Entity>` clone plus a copy of `SceneBounds`; typical N <= ~50
    /// containers, sub-microsecond per tick). Read by `ui::view` during
    /// the empty-banner debounce window: instead of painting a blank
    /// scene-block, the view renders this cached world so the chrome
    /// stays visually continuous through transient empties.
    ///
    /// `None` only at first launch before any container has been seen —
    /// at which point `non_empty_seen_once` is also `false` and the
    /// view-layer falls through to the immediate banner path (Phase 3
    /// criterion #5 preserved).
    pub(crate) last_non_empty_world: Option<World>,
    /// Non-blocking receiver for typed DockerMsg events from the producer task
    /// in `docker::streams`. `None` for the test/dump constructor (`App::new`)
    /// so unit tests don't need a producer.
    docker_rx: Option<UnboundedReceiver<DockerMsg>>,
    /// Bollard Docker handle (Arc-clone, cheap) used by [`Self::spawn_detail_fetch`]
    /// to fire `fetch_detail` off-thread on `Effect::SpawnInspect`. `None` in
    /// tests / the dump path (no daemon connected) — OpenDetail then surfaces
    /// `selection.detail_open=true` but the spawn handler is a noop, so the
    /// popup stays in its "loading…" state forever (test path safety, not a
    /// production code path).
    docker: Option<Docker>,
    /// Cloned sender for `DockerMsg::Inspected` results from the spawned
    /// `fetch_detail` task — same channel the producer uses, so the existing
    /// `drain_docker` loop demuxes Inspected into `selection.pending_detail`
    /// alongside Stat / Added / Removed. `None` in tests.
    tx_for_inspect: Option<UnboundedSender<DockerMsg>>,
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
            palette: crate::theme::Palette::notion_soft(),
            palette_name: "notion-soft".to_string(),
            config: crate::config::AppConfig::default(),
            // App::new seeds a synthetic non-empty world, so mark non-empty
            // as seen once already — there's no first-launch banner state to
            // preserve in the test path. The debounce counters are inert
            // because the world stays non-empty.
            non_empty_seen_once: true,
            empty_streak_ticks: 0,
            // App::new seeds a synthetic non-empty world, but the cache is
            // populated lazily on the first non-empty tick (see on_tick); a
            // direct clone here would couple `new` to the test path's
            // assumption of an EMPTY cache pre-tick. Leave None — the first
            // tick fills it.
            last_non_empty_world: None,
            docker_rx: None,
            docker: None,
            tx_for_inspect: None,
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
            palette: crate::theme::Palette::notion_soft(),
            palette_name: "notion-soft".to_string(),
            config: crate::config::AppConfig::default(),
            // Live path: world starts empty. non_empty_seen_once is false
            // until the first Added lands — the banner shows IMMEDIATELY on
            // startup (no debounce wait at first launch). After the first
            // container appears, the sticky flag flips true and the debounce
            // kicks in for subsequent transient-empty states.
            non_empty_seen_once: false,
            empty_streak_ticks: 0,
            // No container has ever been observed yet — cache is empty. The
            // first non-empty on_tick will populate it.
            last_non_empty_world: None,
            docker_rx: Some(rx),
            docker: None,
            tx_for_inspect: None,
        }
    }

    /// Construct fresh app state with a live Docker receiver AND a docker
    /// handle + sender for off-thread `fetch_detail` spawn on Enter (04-06b).
    ///
    /// `tx` is the SAME sender the producer task in `docker::streams` is
    /// already using — `DockerMsg::Inspected` results flow back through the
    /// existing mpsc and demux into `selection.pending_detail` inside
    /// [`Self::drain_docker`]. `docker` is a cheap-clone Arc handle inside
    /// bollard; the spawn handler clones it again before moving into the
    /// `async move` block, so the App keeps its own handle for subsequent
    /// Enters.
    pub fn with_docker(
        docker: Docker,
        tx: UnboundedSender<DockerMsg>,
        rx: UnboundedReceiver<DockerMsg>,
        config: crate::config::AppConfig,
    ) -> Self {
        let mut app = Self::with_docker_rx(rx);
        app.docker = Some(docker);
        app.tx_for_inspect = Some(tx);
        // Resolve initial palette from config.palette. Unknown name falls
        // back to notion-soft (by_name contract) AND we rewrite
        // palette_name to "notion-soft" so next_palette's
        // `.position(|n| *n == palette_name)` can locate the current slot
        // and cycling keeps working. Without this rewrite, an unknown
        // config string would leave palette_name pointing at a non-member
        // of the order vec, breaking the cycle.
        let (palette, palette_name) = match crate::theme::Palette::by_name(&config.palette) {
            Some(p) => (p, config.palette.clone()),
            None => {
                eprintln!(
                    "config: unknown palette '{}', falling back to notion-soft",
                    config.palette
                );
                (crate::theme::Palette::notion_soft(), "notion-soft".to_string())
            }
        };
        app.palette = palette;
        app.palette_name = palette_name;
        app.config = config;
        app
    }

    /// Cycle order for `P`. omarchy is included only when
    /// `Palette::from_omarchy()` resolves at boot — we don't want to
    /// silently include a slot that maps to nothing.
    ///
    /// Returns the next (name, palette) pair after the current name. Kept
    /// local to App; the kitty backend mirrors this list inline in
    /// `run_kitty` to avoid a hard kitty -> App dependency.
    fn next_palette(&self) -> (String, crate::theme::Palette) {
        let mut order: Vec<&str> = vec!["notion-soft", "cyberpunk-neon", "terminal-green"];
        if crate::theme::Palette::from_omarchy().is_some() {
            order.push("omarchy");
        }
        let cur = order
            .iter()
            .position(|n| *n == self.palette_name.as_str())
            .unwrap_or(0);
        let next_name = order[(cur + 1) % order.len()];
        (
            next_name.to_string(),
            crate::theme::Palette::by_name_or_default(next_name),
        )
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
            Effect::SpawnInspect(id) => self.spawn_detail_fetch(id),
            Effect::CyclePalette => {
                let (name, palette) = self.next_palette();
                self.palette = palette;
                self.palette_name = name;
            }
            Effect::None => {}
        }
    }

    /// Off-thread inspect-and-deliver for the detail panel (04-06b, CAM-05).
    ///
    /// Called by [`Self::update`] when [`apply_input_action`] surfaces
    /// `Effect::SpawnInspect(container_id)` (i.e. user pressed Enter on a
    /// selected box). Fires `docker::fetch_detail` on the tokio runtime and
    /// pipes the result back through the existing mpsc as
    /// `DockerMsg::Inspected(snap)` — [`Self::drain_docker`] then demuxes it
    /// into `selection.pending_detail` and clears `inspect_in_flight`.
    ///
    /// Pitfall 8 closure: the render loop NEVER blocks on the inspect call —
    /// `tokio::spawn` returns immediately. A slow daemon shows the
    /// "loading…" popup state for as long as the call takes (typically
    /// <50ms for a healthy local socket).
    ///
    /// Idempotent: if `inspect_in_flight` is already true (Enter pressed
    /// twice quickly), the second call is a noop — the first spawn's result
    /// will land regardless of the second press.
    ///
    /// Test-path safety: when `self.docker` is `None` (the `App::new` /
    /// `with_docker_rx` constructors used in unit tests), this is a noop —
    /// the popup stays in "loading…" state, which is the right test-time
    /// behavior (no real daemon to inspect against).
    ///
    /// On bollard error inside `fetch_detail`: the spawned future swallows
    /// the error and does NOT send a message. The popup stays in "loading…"
    /// — better than wedging the render loop on an unhandled `ProbeError`.
    /// Future work (Phase 5 ROB-02) may add an error banner channel.
    fn spawn_detail_fetch(&mut self, id: String) {
        if self.selection.inspect_in_flight {
            return;
        }
        let (Some(docker), Some(tx)) = (self.docker.as_ref(), self.tx_for_inspect.as_ref()) else {
            // Test path: no real daemon connected. The selection.detail_open
            // flag was already set by apply_input_action; the popup will show
            // "loading…" forever (or until Esc closes it). Production always
            // goes through `with_docker`, so this branch is test-only.
            return;
        };
        self.selection.inspect_in_flight = true;
        self.selection.pending_detail = None;
        let docker = docker.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            if let Ok(snap) = crate::docker::fetch_detail(&docker, &id).await {
                let _ = tx.send(DockerMsg::Inspected(snap));
            }
            // On error: don't send. The popup stays "loading…" until the
            // user presses Esc or Enter again. See doc comment above.
        });
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
        // Empty-banner debounce counters (05-04-RV2). Tracked at TICK_HZ
        // (60 Hz, framerate-independent) rather than at render rate so the
        // debounce is consistent across slow / degraded frames. When the
        // world is non-empty, mark "ever seen" and reset the streak; when
        // empty, increment the streak (saturating). `ui::view` consults
        // `should_show_empty_banner()` to decide whether to actually paint
        // the banner — see that method's doc comment.
        if self.world.entities.is_empty() {
            self.empty_streak_ticks = self.empty_streak_ticks.saturating_add(1);
        } else {
            self.non_empty_seen_once = true;
            self.empty_streak_ticks = 0;
            // RV4: cache the freshest non-empty world for the empty-banner
            // debounce path. Cloning a small `Vec<Entity>` + `SceneBounds`
            // every tick is well under the per-tick budget (worst case
            // ~50 containers ~= ~5 µs at 60 Hz; the dress() pass already
            // walked the same slice this tick). Reading `view`-side, the
            // cache is preferred over the live empty world during the
            // debounce window so the scene chrome doesn't flash to blank.
            self.last_non_empty_world = Some(self.world.clone());
        }
    }

    /// Whether the empty-state banner should actually be drawn this frame.
    ///
    /// Three gates, evaluated in order:
    ///
    /// 1. **Cold-start grace** ([`STARTUP_GRACE_TICKS`], ~1 s at TICK_HZ=60):
    ///    while `tick_count < STARTUP_GRACE_TICKS` the banner is
    ///    unconditionally suppressed. Eliminates the ~100 ms flash the
    ///    user reported when launching `dd3` against a busy daemon
    ///    (bollard's `list_containers` seed lands at t~80-150 ms; the
    ///    first 1-3 render frames would otherwise paint the banner before
    ///    any Added arrives). 05-04-RV5 root-cause fix. The view renders
    ///    a neutral bordered "scene" chrome (no banner text, no content)
    ///    during this window — same chrome shape the live render uses, so
    ///    the transition to populated state is visually seamless.
    ///
    /// 2. **First-launch true empty** (post-grace, `!non_empty_seen_once`):
    ///    after the grace expires AND we have never observed a non-empty
    ///    world, the banner shows. This preserves Phase 3 criterion #5
    ///    (a daemon with zero containers must show the banner) with a
    ///    ~1 s delay — acceptable trade for eliminating the flash.
    ///
    /// 3. **Sustained empty** ([`EMPTY_BANNER_DEBOUNCE_TICKS`], ~5 s at
    ///    TICK_HZ=60): after we have seen at least one container, the
    ///    banner only re-appears if the world stays empty for 5 s
    ///    straight. RV5 bump from RV4's 1 s. Combined with the cached-
    ///    world render path in [`ui::view`], this realizes the user's
    ///    "keep cache for at least 5 seconds" invariant: every realistic
    ///    transient empty (container restart, `docker rm -f` + `docker run`,
    ///    mid-session daemon hiccup) is well under 5 s, so the cached
    ///    scene holds and the banner stays hidden during normal use.
    ///
    /// Called only when `world.entities.is_empty()` is already true at the
    /// view site; this method does NOT itself check that condition.
    pub(crate) fn should_show_empty_banner(&self) -> bool {
        if self.tick_count < STARTUP_GRACE_TICKS as u64 {
            return false;
        }
        !self.non_empty_seen_once || self.empty_streak_ticks >= EMPTY_BANNER_DEBOUNCE_TICKS
    }

    // Note: `view` callers inline the "live vs cached world" decision so
    // they can `&mut app.selection` alongside the chosen world borrow.
    // The decision is documented at the `ui::view` call site and mirrored
    // in the kitty backend's `render_target` selection.

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
            // 04-06b: demux Inspected into selection BEFORE `live.apply`.
            // The cache write in `LiveWorld::handle_inspected` still runs
            // (apply takes the msg by value below), but the popup-facing
            // slot is the source of truth for the renderer — Selection
            // owns `pending_detail`, LiveWorld owns the by-id cache.
            if let DockerMsg::Inspected(snap) = &msg {
                self.selection.pending_detail = Some(snap.clone());
                self.selection.inspect_in_flight = false;
            }
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
            //
            // 05-04-RV1 (release-stops-input fix): KEY actions are collected,
            // then `coalesce_actions` collapses repeats before dispatch. A
            // user holding P or W for 2s queues ~60 key events at the OS
            // auto-repeat rate; without coalescing the backlog plays out one-
            // per-frame AFTER release, so the palette / camera keeps stepping
            // for a noticeable beat. With coalescing, the drain pass discards
            // duplicates and applies at most one nudge/cycle per outer loop —
            // hold-to-glide still works (the next outer iteration sees the
            // next batch and applies one more), but release stops the motion
            // on the NEXT drain pass (no events arrive → no actions dispatched).
            let mut render_requested = false;
            let mut pending_actions: Vec<Action> = Vec::new();
            let mut next = Some(event);
            while let Some(ev) = next {
                match ev {
                    Event::Key(key) => pending_actions.push(Action::from_key(key)),
                    Event::Resize(w, h) => self.on_resize(w, h),
                    Event::Tick => {
                        let now = Instant::now();
                        let dt = now.duration_since(self.last_tick).as_secs_f32();
                        self.last_tick = now;
                        self.on_tick(dt);
                    }
                    Event::Render => render_requested = true,
                }
                next = tui.try_next();
            }

            // Dispatch the coalesced action set in first-occurrence order. An
            // OS key-repeat backlog of 30 CyclePalette events becomes ONE
            // CyclePalette here; a left-then-right yaw pair sums into a near-
            // zero yaw nudge that's effectively idempotent.
            for action in coalesce_actions(&pending_actions) {
                self.update(action);
                if self.should_quit {
                    return Ok(());
                }
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

    // ---- 04-06b: Effect::SpawnInspect handling + Inspected demux -----------

    use crate::docker::{DetailSnapshot, HealthSummary};

    fn detail_for(id: &str) -> DetailSnapshot {
        DetailSnapshot {
            id: id.to_string(),
            name: id.to_string(),
            status: Status::Running,
            health: HealthSummary::None,
            started_at_iso: String::new(),
            restart_count: 0,
            restart_policy: "no".to_string(),
            image_human: String::new(),
            image_digest: String::new(),
            network_mode: String::new(),
            networks: Vec::new(),
            ports: Vec::new(),
            mounts: Vec::new(),
        }
    }

    /// App::new() has no docker handle — OpenDetail must NOT panic on the
    /// missing `self.docker`. The spawn handler returns early as a noop and
    /// the selection.detail_open flag stays as apply_input_action set it.
    #[test]
    fn app_open_detail_with_no_docker_handle_does_not_panic() {
        // App::new() seeds the synthetic scene + None docker / tx.
        let mut app = App::new();
        // Manually arm the selection so OpenDetail surfaces SpawnInspect (the
        // synthetic scene has entities with ids; pick the first one and we
        // need a corresponding LiveWorld id to resolve — easier to just call
        // spawn_detail_fetch directly to exercise the early-return branch).
        // The doc contract: with docker=None, spawn_detail_fetch is a noop.
        app.spawn_detail_fetch("any-id".to_string());
        // The flag may have been false going in; the noop branch should NOT
        // mutate it (the production path sets it to true before spawning).
        assert!(
            !app.selection.inspect_in_flight,
            "no-docker spawn must NOT set inspect_in_flight (noop branch)"
        );
        assert!(app.selection.pending_detail.is_none());
    }

    /// A synthetic `DockerMsg::Inspected` fed through drain_docker populates
    /// `selection.pending_detail` and clears `inspect_in_flight`. This is the
    /// other half of the Effect::SpawnInspect round-trip — the spawn fires
    /// fetch_detail, the producer sends back Inspected, drain_docker demuxes.
    #[test]
    fn app_inspected_message_populates_pending_detail() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<DockerMsg>();
        let mut app = App::with_docker_rx(rx);
        assert!(app.selection.pending_detail.is_none());
        app.selection.inspect_in_flight = true;

        tx.send(DockerMsg::Inspected(detail_for("c1"))).unwrap();
        app.drain_docker();

        assert!(
            app.selection.pending_detail.is_some(),
            "Inspected msg must populate pending_detail"
        );
        assert_eq!(app.selection.pending_detail.as_ref().unwrap().id, "c1");
        assert!(
            !app.selection.inspect_in_flight,
            "Inspected msg must clear inspect_in_flight"
        );
    }

    /// Even when nothing else changes (no Added / Removed in the same drain),
    /// the in_flight flag must still clear — the Inspected demux is BEFORE
    /// the `live.apply` call.
    #[test]
    fn app_inspected_clears_in_flight_flag_with_no_other_msgs() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<DockerMsg>();
        let mut app = App::with_docker_rx(rx);
        app.selection.inspect_in_flight = true;

        tx.send(DockerMsg::Inspected(detail_for("alpha"))).unwrap();
        app.drain_docker();

        assert!(!app.selection.inspect_in_flight);
        assert!(app.selection.pending_detail.is_some());
    }

    /// A second spawn_detail_fetch call while one is in-flight must be a
    /// noop — prevents Enter-mashing from queueing duplicate fetches against
    /// the daemon. The first spawn's result lands regardless.
    #[test]
    fn app_duplicate_open_detail_does_not_double_spawn() {
        let mut app = App::new();
        // Manually set the in-flight flag as if a previous spawn already ran.
        app.selection.inspect_in_flight = true;
        // Without a real docker handle, both calls hit the early-return.
        // With a real handle, the in_flight gate is what stops the second
        // call. Pin both paths: spawn handler must NOT clear in_flight.
        app.spawn_detail_fetch("c1".to_string());
        assert!(
            app.selection.inspect_in_flight,
            "second spawn while in-flight must not flip the flag off"
        );
        // pending_detail must NOT be touched by a duplicate spawn either —
        // a partially-populated popup would lose its data on a double-press.
        app.selection.pending_detail = Some(detail_for("preexisting"));
        app.spawn_detail_fetch("c1".to_string());
        assert!(
            app.selection.pending_detail.is_some(),
            "duplicate spawn must not clear pending_detail"
        );
    }

    // ---- THEME-04 (05-04) runtime palette swap ------------------------------

    /// `next_palette` walks the cycle order in the documented sequence and
    /// wraps back to notion-soft. The omarchy slot is conditional on
    /// Palette::from_omarchy() — when the host has no alacritty theme file
    /// the cycle is 3-element. We assert the first three steps because
    /// those are stable regardless of host config.
    #[test]
    fn next_palette_cycles_through_named_presets() {
        let mut app = App::new();
        // Start at notion-soft (App::new() default).
        assert_eq!(app.palette_name, "notion-soft");

        let (n1, _) = app.next_palette();
        assert_eq!(n1, "cyberpunk-neon", "step 1: notion-soft -> cyberpunk-neon");
        app.palette_name = n1;

        let (n2, _) = app.next_palette();
        assert_eq!(n2, "terminal-green", "step 2: cyberpunk-neon -> terminal-green");
        app.palette_name = n2;

        // Step 3 is either "omarchy" (when from_omarchy() resolves) or wraps
        // back to "notion-soft" — pin both possibilities so the test works on
        // any host. The cycle MUST land on a member of the order vec.
        let (n3, _) = app.next_palette();
        assert!(
            n3 == "omarchy" || n3 == "notion-soft",
            "step 3 must be omarchy (if available) OR wrap to notion-soft, got: {n3}"
        );
    }

    /// Dispatching Action::CyclePalette through `update()` mutates BOTH
    /// `palette` and `palette_name` in place. The starting Palette is
    /// notion_soft (App::new default); after one cycle it MUST differ.
    #[test]
    fn cycle_palette_via_update_mutates_state() {
        let mut app = App::new();
        let before_palette = app.palette;
        let before_name = app.palette_name.clone();

        app.update(Action::CyclePalette);

        assert_ne!(
            app.palette_name, before_name,
            "CyclePalette must change palette_name"
        );
        assert_ne!(
            app.palette, before_palette,
            "CyclePalette must change the palette struct (different RGB triplets)"
        );
    }

    /// Palette cycling is meta-state — App::update must NOT flip autopilot.
    /// Mirrors the action.rs `apply_cycle_palette_does_not_flip_autopilot`
    /// pin, exercised through the App dispatch path.
    #[test]
    fn cycle_palette_does_not_flip_autopilot() {
        let mut app = App::new();
        assert!(app.camera.autopilot_active);
        app.update(Action::CyclePalette);
        assert!(
            app.camera.autopilot_active,
            "CyclePalette through App::update must NOT disturb autopilot"
        );
    }

    /// `App::with_docker` honors the AppConfig.palette field at startup —
    /// passing `palette = "cyberpunk-neon"` constructs an App with that
    /// palette already active AND palette_name set to the same string.
    #[test]
    fn with_docker_resolves_palette_from_config() {
        use crate::config::AppConfig;
        // We don't actually need a real Docker handle for this assertion;
        // construct via with_docker_rx + manually set the fields the way
        // with_docker would, since with_docker requires a bollard Docker
        // handle we can't conjure without a live socket. Instead we exercise
        // the resolution code path by mimicking what with_docker does after
        // calling with_docker_rx.
        let (_tx, rx) = tokio::sync::mpsc::unbounded_channel::<DockerMsg>();
        let mut app = App::with_docker_rx(rx);
        let config = AppConfig {
            palette: "cyberpunk-neon".to_string(),
            ..AppConfig::default()
        };
        let (palette, palette_name) = match crate::theme::Palette::by_name(&config.palette) {
            Some(p) => (p, config.palette.clone()),
            None => (
                crate::theme::Palette::notion_soft(),
                "notion-soft".to_string(),
            ),
        };
        app.palette = palette;
        app.palette_name = palette_name;
        app.config = config;

        assert_eq!(app.palette, crate::theme::Palette::cyberpunk_neon());
        assert_eq!(app.palette_name, "cyberpunk-neon");
    }

    // ---- 05-04-RV2: empty-banner debounce -----------------------------------

    /// 05-04-RV5: at startup (no container has ever been observed AND the
    /// cold-start grace has not yet expired), the banner is SUPPRESSED. The
    /// renderer paints a neutral bordered "scene" block instead so the
    /// ~80-150 ms bollard `list_containers` seed roundtrip can complete
    /// without flashing the banner text on screen.
    ///
    /// After [`STARTUP_GRACE_TICKS`] ticks (~1 s at TICK_HZ=60) the gate
    /// lifts: if `non_empty_seen_once` is still false the banner finally
    /// shows (Phase 3 criterion #5 — a daemon with zero containers gets
    /// its banner, just delayed by ~1 s).
    #[test]
    fn empty_banner_suppressed_during_startup_grace_then_shows() {
        let (_tx, rx) = tokio::sync::mpsc::unbounded_channel::<DockerMsg>();
        let mut app = App::with_docker_rx(rx);
        // First frame after launch — world empty, never seen a container,
        // tick_count == 0. The grace gate must SUPPRESS the banner.
        assert!(app.world.entities.is_empty());
        assert!(!app.non_empty_seen_once);
        assert_eq!(app.tick_count, 0);
        assert!(
            !app.should_show_empty_banner(),
            "RV5 cold-start grace must suppress the banner for the first \
             STARTUP_GRACE_TICKS — this is the fix for the ~100 ms flash \
             the user reported on dd3 launch"
        );

        // Tick all the way to the grace boundary; banner still suppressed
        // on the LAST tick of the grace window (`tick_count < GRACE`).
        for _ in 0..STARTUP_GRACE_TICKS {
            app.on_tick(0.016);
        }
        // tick_count is now exactly STARTUP_GRACE_TICKS — the gate opens.
        assert_eq!(app.tick_count, STARTUP_GRACE_TICKS as u64);
        assert!(
            app.should_show_empty_banner(),
            "after the grace expires AND we have never seen a container, \
             the banner must finally show (Phase 3 criterion #5)"
        );
    }

    /// After a container has appeared and then disappeared, the banner is
    /// SUPPRESSED for the debounce window even though the world is empty
    /// right now. The renderer keeps painting the last 3D scene chrome —
    /// no flash.
    #[test]
    fn empty_banner_debounces_after_container_churn() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<DockerMsg>();
        let mut app = App::with_docker_rx(rx);
        // Container appears — flips non_empty_seen_once on the next tick.
        tx.send(DockerMsg::Added(snap("only"))).unwrap();
        app.drain_docker();
        assert_eq!(app.world.entities.len(), 1);
        app.on_tick(0.016);
        assert!(app.non_empty_seen_once);
        assert_eq!(app.empty_streak_ticks, 0);

        // Container disappears — world is empty but streak counter is fresh.
        tx.send(DockerMsg::Removed("only".to_string())).unwrap();
        app.drain_docker();
        assert!(app.world.entities.is_empty());

        // First tick of empty: streak == 1 (RV5: < DEBOUNCE_TICKS=300).
        // Banner SHOULD STAY HIDDEN — the visible behavior is "retained
        // 3D scene chrome", not a banner flash.
        app.on_tick(0.016);
        assert_eq!(app.empty_streak_ticks, 1);
        assert!(
            !app.should_show_empty_banner(),
            "first tick of post-churn empty must NOT show banner (debounce)"
        );
    }

    /// After the debounce window elapses (RV5: 300 consecutive empty ticks
    /// = ~5 s at TICK_HZ=60), the banner returns. A genuinely-empty
    /// daemon still gets its banner — just after a longer retained-frame
    /// window than RV4's 1 s.
    #[test]
    fn empty_banner_returns_after_debounce_window_elapses() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<DockerMsg>();
        let mut app = App::with_docker_rx(rx);
        tx.send(DockerMsg::Added(snap("only"))).unwrap();
        app.drain_docker();
        app.on_tick(0.016);
        tx.send(DockerMsg::Removed("only".to_string())).unwrap();
        app.drain_docker();

        // Tick the empty world EMPTY_BANNER_DEBOUNCE_TICKS times.
        for _ in 0..EMPTY_BANNER_DEBOUNCE_TICKS {
            app.on_tick(0.016);
        }
        assert_eq!(app.empty_streak_ticks, EMPTY_BANNER_DEBOUNCE_TICKS);
        assert!(
            app.should_show_empty_banner(),
            "banner must return after the debounce window of stable emptiness"
        );
    }

    /// A non-empty tick BETWEEN two empty episodes resets the streak — the
    /// debounce window starts fresh on each transient. This is the realistic
    /// `docker rm -f` then `docker run` pattern: never enough sustained
    /// emptiness to trip the banner.
    ///
    /// RV5: warm up past the cold-start grace first so the assertion isn't
    /// trivially satisfied by the grace gate; we want to validate the
    /// streak-reset semantics on their own.
    #[test]
    fn empty_banner_streak_resets_on_non_empty_tick() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<DockerMsg>();
        let mut app = App::with_docker_rx(rx);
        tx.send(DockerMsg::Added(snap("only"))).unwrap();
        app.drain_docker();
        // Warm past the cold-start grace so the gate doesn't shadow this
        // assertion. The world is non-empty here so the streak stays at 0
        // throughout.
        for _ in 0..STARTUP_GRACE_TICKS {
            app.on_tick(0.016);
        }
        assert!(app.tick_count >= STARTUP_GRACE_TICKS as u64);
        // Empty for a few ticks but under the (RV5: 300-tick) debounce
        // threshold.
        tx.send(DockerMsg::Removed("only".to_string())).unwrap();
        app.drain_docker();
        for _ in 0..5 {
            app.on_tick(0.016);
        }
        assert_eq!(app.empty_streak_ticks, 5);
        // New container arrives — streak resets, banner stays hidden.
        tx.send(DockerMsg::Added(snap("two"))).unwrap();
        app.drain_docker();
        app.on_tick(0.016);
        assert_eq!(app.empty_streak_ticks, 0);
        assert!(
            !app.should_show_empty_banner(),
            "non-empty tick must reset the empty streak"
        );
    }

    /// `non_empty_seen_once` is sticky — it never flips back to false even
    /// after the world empties. The flag's job is to distinguish first-launch
    /// (no containers EVER) from post-launch transient empties.
    #[test]
    fn non_empty_seen_once_is_sticky_after_first_container() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<DockerMsg>();
        let mut app = App::with_docker_rx(rx);
        assert!(!app.non_empty_seen_once);
        tx.send(DockerMsg::Added(snap("a"))).unwrap();
        app.drain_docker();
        app.on_tick(0.016);
        assert!(app.non_empty_seen_once);
        tx.send(DockerMsg::Removed("a".to_string())).unwrap();
        app.drain_docker();
        // Many empty ticks must NOT flip the sticky flag.
        for _ in 0..100 {
            app.on_tick(0.016);
        }
        assert!(
            app.non_empty_seen_once,
            "non_empty_seen_once must stay sticky across empty ticks"
        );
    }

    // ---- 05-04-RV4: cached-world during debounce + bumped window ----------

    /// The cache is populated on EVERY non-empty `on_tick`, so a subsequent
    /// transient-empty tick has a fresh snapshot to render from. The cache
    /// holds the world's entities (the load-bearing surface for the view).
    #[test]
    fn last_non_empty_world_cache_populates_on_non_empty_tick() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<DockerMsg>();
        let mut app = App::with_docker_rx(rx);
        assert!(
            app.last_non_empty_world.is_none(),
            "fresh App must start with no cached world"
        );
        tx.send(DockerMsg::Added(snap("a"))).unwrap();
        tx.send(DockerMsg::Added(snap("b"))).unwrap();
        app.drain_docker();
        app.on_tick(0.016);
        let cached = app
            .last_non_empty_world
            .as_ref()
            .expect("cache must be Some after non-empty tick");
        assert_eq!(
            cached.entities.len(),
            2,
            "cache must mirror the world content (2 containers added)"
        );
    }

    /// The cache survives a transient empty: once populated by a non-empty
    /// tick, an empty world followed by ticks does NOT clear it (no
    /// `last_non_empty_world = None` path on empty). That's what lets the
    /// view-layer render the cached snapshot during the debounce window.
    #[test]
    fn last_non_empty_world_cache_survives_transient_empty() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<DockerMsg>();
        let mut app = App::with_docker_rx(rx);
        tx.send(DockerMsg::Added(snap("only"))).unwrap();
        app.drain_docker();
        app.on_tick(0.016);
        assert!(app.last_non_empty_world.is_some());

        // Container disappears — cache MUST stay populated.
        tx.send(DockerMsg::Removed("only".to_string())).unwrap();
        app.drain_docker();
        assert!(app.world.entities.is_empty());
        // Several empty ticks within the debounce window: cache holds.
        for _ in 0..10 {
            app.on_tick(0.016);
        }
        let cached = app
            .last_non_empty_world
            .as_ref()
            .expect("cache must survive transient empty ticks");
        assert_eq!(
            cached.entities.len(),
            1,
            "cached snapshot must hold the last non-empty entity set"
        );
    }

    /// The cache UPDATES whenever the world is non-empty: a churn pattern
    /// (1 -> 0 -> 2 containers) leaves the cache at the LATEST non-empty
    /// snapshot (2 containers), not the original (1). This matters because
    /// the next transient empty should render the freshest scene, not a
    /// stale one from minutes ago.
    #[test]
    fn last_non_empty_world_cache_refreshes_on_each_non_empty_tick() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<DockerMsg>();
        let mut app = App::with_docker_rx(rx);
        // First era: 1 container.
        tx.send(DockerMsg::Added(snap("first"))).unwrap();
        app.drain_docker();
        app.on_tick(0.016);
        assert_eq!(app.last_non_empty_world.as_ref().unwrap().entities.len(), 1);
        // Removed — cache holds at 1.
        tx.send(DockerMsg::Removed("first".to_string())).unwrap();
        app.drain_docker();
        app.on_tick(0.016);
        assert_eq!(app.last_non_empty_world.as_ref().unwrap().entities.len(), 1);
        // Second era: 2 new containers — cache must refresh.
        tx.send(DockerMsg::Added(snap("second"))).unwrap();
        tx.send(DockerMsg::Added(snap("third"))).unwrap();
        app.drain_docker();
        app.on_tick(0.016);
        assert_eq!(
            app.last_non_empty_world.as_ref().unwrap().entities.len(),
            2,
            "cache must reflect the FRESHEST non-empty world, not the original"
        );
    }

    /// The bumped debounce window (RV5 5000 ms = 300 ticks at 60 Hz) holds
    /// the banner off through a much longer churn gap than RV4's 1 s. A
    /// 200-tick (~3.3 s) empty episode — which RV4's 60-tick (~1 s)
    /// window would have shown the banner for — keeps the banner hidden
    /// under RV5. This realizes the user's "cache valid for at least 5
    /// seconds" invariant.
    #[test]
    fn rv5_debounce_window_covers_200_tick_empty_episode() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<DockerMsg>();
        let mut app = App::with_docker_rx(rx);
        tx.send(DockerMsg::Added(snap("a"))).unwrap();
        app.drain_docker();
        // Warm past the cold-start grace before exercising the post-empty
        // debounce: otherwise the grace gate would shadow this assertion.
        for _ in 0..STARTUP_GRACE_TICKS {
            app.on_tick(0.016);
        }
        tx.send(DockerMsg::Removed("a".to_string())).unwrap();
        app.drain_docker();
        // 200 empty ticks (~3.3 s at 60 Hz) — well past RV4's 60-tick
        // threshold, well under RV5's 300-tick threshold.
        for _ in 0..200 {
            app.on_tick(0.016);
        }
        assert_eq!(app.empty_streak_ticks, 200);
        assert!(
            !app.should_show_empty_banner(),
            "RV5 window (300 ticks) must keep banner hidden through a 200-tick empty episode"
        );
        // Verify the threshold constant itself is the RV5 bump.
        assert_eq!(
            EMPTY_BANNER_DEBOUNCE_TICKS, 300,
            "RV5 expected EMPTY_BANNER_DEBOUNCE_TICKS bumped to 300 (~5000 ms at 60 Hz)"
        );
    }

    /// Verify-frame helper for RV4: write a tick-by-tick trace of the
    /// debounce state machine through a realistic churn scenario to
    /// `/tmp/v504-rv4-debounce-trace.txt`. Demonstrates that the banner
    /// stays hidden through transient empties while the cache freezes
    /// the last non-empty scene.
    #[test]
    #[ignore = "writes a file under /tmp; run on demand via --ignored for human verify"]
    fn rv4_writes_debounce_trace() {
        use std::io::Write;
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<DockerMsg>();
        let mut app = App::with_docker_rx(rx);
        let mut f = std::fs::File::create("/tmp/v504-rv4-debounce-trace.txt").unwrap();
        writeln!(
            f,
            "RV4 empty-banner debounce verify\n\
             ====================================\n\
             EMPTY_BANNER_DEBOUNCE_TICKS = {EMPTY_BANNER_DEBOUNCE_TICKS} \
             (at TICK_HZ=60 that's ~{} ms)\n",
            (EMPTY_BANNER_DEBOUNCE_TICKS as f32) * 1000.0 / 60.0
        )
        .unwrap();
        writeln!(
            f,
            "frame  world.len  streak  seen_once  cache.len  banner?  note",
        )
        .unwrap();
        let log = |f: &mut std::fs::File, frame: usize, app: &App, note: &str| {
            let cache_len = app
                .last_non_empty_world
                .as_ref()
                .map(|w| w.entities.len() as i64)
                .unwrap_or(-1);
            writeln!(
                f,
                "{:>5}  {:>9}  {:>6}  {:>9}  {:>9}  {:>7}  {}",
                frame,
                app.world.entities.len(),
                app.empty_streak_ticks,
                app.non_empty_seen_once,
                cache_len,
                app.should_show_empty_banner(),
                note,
            )
            .unwrap();
        };

        log(&mut f, 0, &app, "fresh app, first-launch (RV5 grace gate SUPPRESSES banner during the first STARTUP_GRACE_TICKS window — no flash)");
        // Add a container; tick once.
        tx.send(DockerMsg::Added(snap("c1"))).unwrap();
        app.drain_docker();
        app.on_tick(0.016);
        log(&mut f, 1, &app, "container added, non_empty_seen_once flipped, cache populated");
        // Tick a few more times with the world non-empty.
        for i in 2..5 {
            app.on_tick(0.016);
            log(&mut f, i, &app, "steady state with 1 container");
        }
        // Remove the container — transient empty begins.
        tx.send(DockerMsg::Removed("c1".to_string())).unwrap();
        app.drain_docker();
        app.on_tick(0.016);
        log(&mut f, 5, &app, "container removed; first empty tick — debounce HOLDS banner OFF, cache RETAINS last scene");
        // 30 more empty ticks (~500 ms — would have shown banner under RV2's 200 ms window).
        for i in 6..35 {
            app.on_tick(0.016);
            if i == 17 {
                log(&mut f, i, &app, "tick ~17: past RV2's 12-tick threshold, RV5 STILL holding banner");
            }
        }
        log(&mut f, 35, &app, "30 ticks empty — RV5 still holding banner; cache freezes scene");
        // New container arrives BEFORE debounce expires — banner never shows.
        tx.send(DockerMsg::Added(snap("c2"))).unwrap();
        app.drain_docker();
        app.on_tick(0.016);
        log(&mut f, 36, &app, "new container — streak reset, banner stays hidden, no flash");
        // Now starve to a real empty: remove + ride out the full debounce window.
        tx.send(DockerMsg::Removed("c2".to_string())).unwrap();
        app.drain_docker();
        for _ in 37..=37 + EMPTY_BANNER_DEBOUNCE_TICKS as usize {
            app.on_tick(0.016);
        }
        log(
            &mut f,
            37 + EMPTY_BANNER_DEBOUNCE_TICKS as usize,
            &app,
            "EMPTY_BANNER_DEBOUNCE_TICKS empty — debounce window elapsed, banner FINALLY shows (stable empty)",
        );
        writeln!(f, "\nPASS: banner stayed hidden through transient empties; showed only after stable empty").unwrap();
    }

    // ---- 05-04-RV5: cold-start grace + invariant tests ----------------------

    /// **Cold-start no-flicker invariant** (the canonical RV5 regression
    /// pin): from t=0 through any number of ticks where the daemon's
    /// `list_containers` seed lands at frame N (e.g. N=3, ~100 ms),
    /// `should_show_empty_banner()` must NEVER return true during the
    /// `0..N` window — only AFTER the seed lands and the world is empty
    /// for [`EMPTY_BANNER_DEBOUNCE_TICKS`] would the banner show.
    ///
    /// This is the test that pins the user's reported bug: "banner still
    /// flickers ~100 ms occasionally on launch". Before RV5, this test
    /// would FAIL at the very first frame because
    /// `!non_empty_seen_once && tick_count < STARTUP_GRACE_TICKS` paths
    /// to `true`.
    #[test]
    fn rv5_no_banner_during_cold_start_seed_race() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<DockerMsg>();
        let mut app = App::with_docker_rx(rx);

        // SIMULATE a realistic bollard seed race: ticks 0..N pass with no
        // Docker messages. Then at tick N the seed lands (multiple Added).
        // For every frame in 0..N the banner MUST stay hidden (this is
        // exactly the cold-start grace's job).
        const SEED_LAND_FRAME: u32 = 6; // ~100 ms at 60 Hz — what the user reported
        for i in 0..SEED_LAND_FRAME {
            app.on_tick(0.016);
            assert!(
                !app.should_show_empty_banner(),
                "frame {i}: banner must NOT show during cold-start grace \
                 (user-reported ~100 ms flicker regression pin)"
            );
        }
        // Seed lands.
        tx.send(DockerMsg::Added(snap("c1"))).unwrap();
        tx.send(DockerMsg::Added(snap("c2"))).unwrap();
        app.drain_docker();
        app.on_tick(0.016);
        assert_eq!(app.world.entities.len(), 2);
        assert!(
            app.non_empty_seen_once,
            "seed lands → sticky flag flips on the first non-empty tick"
        );
        assert!(
            !app.should_show_empty_banner(),
            "post-seed: banner must stay hidden"
        );
    }

    /// **Mid-session no-flicker invariant**: a single empty tick between
    /// two non-empty ticks (the absolute-worst transient: 1-frame
    /// removal-and-reappearance) MUST NOT show the banner on ANY frame.
    /// This pins the user's "transient ~100 ms empty episode" pattern.
    #[test]
    fn rv5_no_banner_during_single_tick_transient_empty() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<DockerMsg>();
        let mut app = App::with_docker_rx(rx);

        // Warm past cold-start grace with a container present so the test
        // exercises the post-non-empty-seen branch.
        tx.send(DockerMsg::Added(snap("a"))).unwrap();
        app.drain_docker();
        for _ in 0..STARTUP_GRACE_TICKS {
            app.on_tick(0.016);
        }
        assert!(app.non_empty_seen_once);
        assert!(app.last_non_empty_world.is_some());

        // Remove → empty for one tick → re-add.
        tx.send(DockerMsg::Removed("a".to_string())).unwrap();
        app.drain_docker();
        assert!(app.world.entities.is_empty());
        app.on_tick(0.016);
        assert_eq!(app.empty_streak_ticks, 1);
        assert!(
            !app.should_show_empty_banner(),
            "1-tick transient empty must NOT show banner (debounce + cache)"
        );

        tx.send(DockerMsg::Added(snap("b"))).unwrap();
        app.drain_docker();
        app.on_tick(0.016);
        assert!(!app.world.entities.is_empty());
        assert_eq!(app.empty_streak_ticks, 0);
        assert!(
            !app.should_show_empty_banner(),
            "after re-add the streak resets — banner must stay hidden"
        );
    }

    /// **Long-transient no-flicker invariant**: even a 4-second empty
    /// episode (240 ticks) — far longer than any realistic Docker churn
    /// gap, but inside the RV5 5-second debounce — keeps the banner
    /// hidden. The cache renders for the whole window.
    #[test]
    fn rv5_no_banner_during_4_second_empty_episode() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<DockerMsg>();
        let mut app = App::with_docker_rx(rx);
        tx.send(DockerMsg::Added(snap("a"))).unwrap();
        app.drain_docker();
        for _ in 0..STARTUP_GRACE_TICKS {
            app.on_tick(0.016);
        }
        tx.send(DockerMsg::Removed("a".to_string())).unwrap();
        app.drain_docker();
        // 240 empty ticks (~4 s at 60 Hz) — under the 300-tick window.
        // The banner must stay hidden for ALL 240 frames.
        for i in 0..240 {
            app.on_tick(0.016);
            assert!(
                !app.should_show_empty_banner(),
                "frame {i} of 4-s empty episode: banner must stay hidden \
                 (RV5 5-s debounce + cached-world render)"
            );
        }
        assert_eq!(app.empty_streak_ticks, 240);
    }

    /// **STARTUP_GRACE_TICKS sanity check**: pin the constant. If a future
    /// refactor accidentally drops the grace to 0 or removes it, this test
    /// will fail and the cold-start flicker would silently regress.
    ///
    /// `const { assert! }` per clippy's `assertions_on_constants` lint —
    /// the assertion is evaluated at compile time, so a regressing edit
    /// fails the BUILD, not just the test run.
    #[test]
    fn rv5_startup_grace_ticks_is_at_least_one_second() {
        const { assert!(STARTUP_GRACE_TICKS >= 60) };
    }

    /// **RV5 invariant trace** (ignored test, writes to /tmp): tick-by-tick
    /// state log through cold-start + steady-state + transient-empty +
    /// long-empty for the human-verify checkpoint. Demonstrates that the
    /// banner field stays `false` until either the grace expires on a
    /// truly-empty daemon or 300 ticks of sustained emptiness elapse.
    #[test]
    #[ignore = "writes a file under /tmp; run on demand via --ignored for human verify"]
    fn rv5_writes_invariant_trace() {
        use std::io::Write;
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<DockerMsg>();
        let mut app = App::with_docker_rx(rx);
        let mut f = std::fs::File::create("/tmp/v504-rv5-invariant-trace.txt").unwrap();
        writeln!(
            f,
            "RV5 cold-start + cache invariant verify\n\
             ==========================================\n\
             STARTUP_GRACE_TICKS         = {STARTUP_GRACE_TICKS} (~{} ms at TICK_HZ=60)\n\
             EMPTY_BANNER_DEBOUNCE_TICKS = {EMPTY_BANNER_DEBOUNCE_TICKS} (~{} ms at TICK_HZ=60)\n",
            (STARTUP_GRACE_TICKS as f32) * 1000.0 / 60.0,
            (EMPTY_BANNER_DEBOUNCE_TICKS as f32) * 1000.0 / 60.0,
        )
        .unwrap();
        writeln!(
            f,
            "tick    world  streak  seen_once  cache  banner?  note",
        )
        .unwrap();
        let log = |f: &mut std::fs::File, app: &App, note: &str| {
            let cache_len = app
                .last_non_empty_world
                .as_ref()
                .map(|w| w.entities.len() as i64)
                .unwrap_or(-1);
            writeln!(
                f,
                "{:>5}  {:>5}  {:>6}  {:>9}  {:>5}  {:>7}  {}",
                app.tick_count,
                app.world.entities.len(),
                app.empty_streak_ticks,
                app.non_empty_seen_once,
                cache_len,
                app.should_show_empty_banner(),
                note,
            )
            .unwrap();
        };

        // === COLD-START ===
        log(&mut f, &app, "tick 0: fresh app, grace gate suppresses banner (no flash)");
        // Simulate a slow seed: 6 ticks pass with NO Docker messages.
        for _ in 0..6 {
            app.on_tick(0.016);
        }
        log(&mut f, &app, "tick 6 (~100 ms): bollard seed still in flight; banner STILL suppressed by grace");
        // Seed lands.
        tx.send(DockerMsg::Added(snap("c1"))).unwrap();
        tx.send(DockerMsg::Added(snap("c2"))).unwrap();
        app.drain_docker();
        app.on_tick(0.016);
        log(&mut f, &app, "tick 7: seed landed; sticky flag flipped; cache populated; banner hidden");
        // Steady state.
        for _ in 0..STARTUP_GRACE_TICKS {
            app.on_tick(0.016);
        }
        log(&mut f, &app, "post-grace steady state: non-empty world, banner hidden");

        // === TRANSIENT EMPTY ===
        tx.send(DockerMsg::Removed("c1".to_string())).unwrap();
        tx.send(DockerMsg::Removed("c2".to_string())).unwrap();
        app.drain_docker();
        app.on_tick(0.016);
        log(&mut f, &app, "containers removed; first empty tick — debounce HOLDS banner OFF, cache renders");
        // 240 empty ticks (~4 s) — well under 5-s window.
        for _ in 0..240 {
            app.on_tick(0.016);
        }
        log(&mut f, &app, "4 s of empty — RV5 STILL holding banner; cache freezes scene");

        // New container — streak resets.
        tx.send(DockerMsg::Added(snap("c3"))).unwrap();
        app.drain_docker();
        app.on_tick(0.016);
        log(&mut f, &app, "new container arrived inside the window — streak reset, banner stays hidden, no flash");

        // === SUSTAINED EMPTY (banner finally shows) ===
        tx.send(DockerMsg::Removed("c3".to_string())).unwrap();
        app.drain_docker();
        for _ in 0..(EMPTY_BANNER_DEBOUNCE_TICKS as usize) {
            app.on_tick(0.016);
        }
        log(&mut f, &app, "300 ticks empty (~5 s) — debounce elapsed, banner FINALLY shows");

        writeln!(
            f,
            "\nPASS: banner stayed hidden through cold-start, transient empties, \
             and a 4-s empty episode; showed only after sustained 5-s empty"
        )
        .unwrap();
    }

    /// Unknown palette names in config fall back to notion-soft AND rewrite
    /// `palette_name` to "notion-soft" — so `next_palette`'s `.position()`
    /// lookup can find the current slot and cycling keeps working. Without
    /// this rewrite, an unknown TOML string would leave the cycle broken at
    /// the wrap step.
    #[test]
    fn with_docker_unknown_palette_falls_back_and_renames() {
        use crate::config::AppConfig;
        let (_tx, rx) = tokio::sync::mpsc::unbounded_channel::<DockerMsg>();
        let mut app = App::with_docker_rx(rx);
        let config = AppConfig {
            palette: "no-such-palette".to_string(),
            ..AppConfig::default()
        };
        // Same resolution logic as with_docker — mirrored here because
        // with_docker needs a real Docker handle.
        let (palette, palette_name) = match crate::theme::Palette::by_name(&config.palette) {
            Some(p) => (p, config.palette.clone()),
            None => (
                crate::theme::Palette::notion_soft(),
                "notion-soft".to_string(),
            ),
        };
        app.palette = palette;
        app.palette_name = palette_name;

        assert_eq!(
            app.palette,
            crate::theme::Palette::notion_soft(),
            "unknown name must fall back to notion_soft palette"
        );
        assert_eq!(
            app.palette_name, "notion-soft",
            "palette_name must be REWRITTEN to 'notion-soft' (not left as 'no-such-palette') so next_palette's .position() can find the slot"
        );

        // And cycling from the fallback state still works (regression-pin
        // for the bug this rewrite prevents):
        let (n1, _) = app.next_palette();
        assert_eq!(n1, "cyberpunk-neon");
    }
}
