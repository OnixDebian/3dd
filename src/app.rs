//! App state + the main event loop.
//!
//! The loop is the single consumer of the unified `Event` channel. It mutates
//! `App` in place and calls the synchronous `terminal.draw(...)` only on
//! `Event::Render` (ARCHITECTURE Pattern 1). The draw closure reads `&self`
//! only and never `.await`s (ARCHITECTURE Anti-Pattern 2).

use std::time::Instant;

use color_eyre::Result;

use crate::action::Action;
use crate::camera::{Camera, SPIN_RATE};
use crate::config::RenderConfig;
use crate::tui::{Event, Tui};
use crate::ui;
use crate::world::{self, World};

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
    /// The synthetic datacenter scene — the SINGLE source of truth for what the
    /// braille UI renders. Built once at construction (static this phase); the
    /// camera is framed to its bounds so the whole rack fills the frame.
    pub world: World,
}

impl App {
    /// Construct fresh app state.
    pub fn new() -> Self {
        let now = Instant::now();
        // Build the synthetic scene once (static this phase) and frame the orbit
        // to its bounds so the autopilot shows the whole rack from frame one
        // (CAM-01 default-on). `step(dt)` only advances yaw/pitch, never radius/
        // target, so this single framing stays correct as the scene is static.
        let world = world::synthetic_scene();
        let mut camera = Camera::new();
        camera.frame_scene(&world.bounds);
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
        }
    }

    /// Dispatch a high-level intent to a state mutation.
    pub fn update(&mut self, action: Action) {
        match action {
            Action::Quit => self.should_quit = true,
            Action::None => {}
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
