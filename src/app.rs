//! App state + the main event loop.
//!
//! The loop is the single consumer of the unified `Event` channel. It mutates
//! `App` in place and calls the synchronous `terminal.draw(...)` only on
//! `Event::Render` (ARCHITECTURE Pattern 1). The draw closure reads `&self`
//! only and never `.await`s (ARCHITECTURE Anti-Pattern 2).

use std::time::Instant;

use color_eyre::Result;

use crate::action::Action;
use crate::camera::Camera;
use crate::config::RenderConfig;
use crate::tui::{Event, Tui};
use crate::ui;

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
    /// Autopilot orbit camera driving the 3D scene.
    pub camera: Camera,
    /// Rendering knobs (cell_aspect, near/far). The camera owns the fov.
    pub render_config: RenderConfig,
}

impl App {
    /// Construct fresh app state.
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            should_quit: false,
            size: (0, 0),
            tick_count: 0,
            last_render: now,
            fps: 0.0,
            last_tick: now,
            camera: Camera::new(),
            render_config: RenderConfig::default(),
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
        self.camera.step(dt);
    }

    /// Run the main event loop until `should_quit`.
    pub async fn run(&mut self, tui: &mut Tui) -> Result<()> {
        // Seed the live size from the terminal so the first frame is correct.
        let area = tui.terminal.size()?;
        self.size = (area.width, area.height);

        while let Some(event) = tui.next().await {
            match event {
                Event::Key(key) => self.update(Action::from_key(key)),
                Event::Resize(w, h) => self.on_resize(w, h),
                Event::Tick => {
                    let now = Instant::now();
                    let dt = now.duration_since(self.last_tick).as_secs_f32();
                    self.last_tick = now;
                    self.on_tick(dt);
                }
                Event::Render => {
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

            if self.should_quit {
                break;
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
