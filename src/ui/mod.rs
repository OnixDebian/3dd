//! The view layer: a pure `view(frame, &app)` that lays out the screen.
//!
//! Layout is always derived from the live `frame.area()` so a resize never
//! reads a stale cached size (Pitfall #14). The scene area hosts the braille
//! Canvas that renders the orbiting cube.

pub mod scene;
pub mod status_bar;

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::Frame;

use crate::app::App;

/// Render the whole UI for one frame. Synchronous, read-only on `App`.
pub fn view(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Vertical split: scene fills the screen above a 1-row status bar.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    let scene_area = chunks[0];
    let status_area = chunks[1];

    // The orbiting World of boxes renders into the braille Canvas in the scene
    // area. Palette is the single color source; the camera (already framed to
    // the scene at app construction) feeds render_scene a ViewParams. The app
    // owns the World — the single source of truth — so we forward it directly
    // (the 02-02 UI-layer synthetic_scene() temporary is gone).
    let palette = crate::theme::Palette::default();
    scene::render_scene(
        frame,
        scene_area,
        &app.camera,
        &app.world,
        &palette,
        &app.render_config,
    );

    status_bar::render(frame, status_area, app);
}
