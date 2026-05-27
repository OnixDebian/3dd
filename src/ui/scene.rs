//! Framebuffer → braille bridge: the SINGLE adapter that owns the
//! coordinate-system mapping (ARCH Pattern 3).
//!
//! [`SceneShape`] is a ratatui [`Shape`] painted inside a `Marker::Braille`
//! [`Canvas`]. Each frame it:
//!   1. renders the cube to a [`Framebuffer`] sized to the canvas braille
//!      resolution (`2*cols × 4*rows`) via [`render3d::render`], and
//!   2. blits every lit sub-pixel with [`Painter::paint`].
//!
//! ## The braille coordinate convention (the resolved research spike)
//!
//! A ratatui `Canvas` uses MATH coordinates (bottom-left origin, configurable
//! x/y bounds) for its high-level `draw` helpers — but [`Painter::paint`] takes
//! GRID DOT indices directly, with a TOP-LEFT origin (`(0,0)` is the upper-left
//! dot, y increasing downward). The framebuffer is ALSO a top-left grid, and the
//! projector (plan 01-03) ALREADY applied the single NDC→screen Y-flip (pinned by
//! `up_world_point_maps_to_upper_half`). So the mapping here is the IDENTITY:
//!
//!   framebuffer (x, y)  →  painter.paint(x, y, color)
//!
//! No second inversion. Flipping again would put the cube upside-down; the human
//! gate just sanity-checks it isn't. The framebuffer is sized to exactly match
//! the canvas grid resolution so `(x, y)` indices line up 1:1 with braille dots.
//!
//! All color comes from the framebuffer (already palette-derived in `render`);
//! no RGB is named here.

use ratatui::symbols::Marker;
use ratatui::widgets::canvas::{Canvas, Painter, Shape};
use ratatui::widgets::{Block, Borders};
use ratatui::Frame;

use crate::camera::{Camera, DEFAULT_FOV};
use crate::config::RenderConfig;
use crate::render3d::render_scene as raster_render_scene;
use crate::theme::Palette;
use crate::world::Entity;

/// A ratatui [`Shape`] that blits a rendered multi-box framebuffer into a
/// braille canvas. Owns a snapshot of the camera/config/entities it renders with.
struct SceneShape {
    /// Braille sub-pixel viewport `(w, h) = (2*cols, 4*rows)` — must match the
    /// canvas grid resolution so dot indices line up 1:1.
    viewport: (usize, usize),
    /// The World's boxes to draw, framed by [`Camera::frame_scene`].
    entities: Vec<Entity>,
    /// Already framed by [`Camera::frame_scene`] before draw (target/radius set).
    camera: Camera,
    palette: Palette,
    config: RenderConfig,
}

impl Shape for SceneShape {
    fn draw(&self, painter: &mut Painter) {
        // Degenerate areas: nothing to paint.
        if self.viewport.0 == 0 || self.viewport.1 == 0 {
            return;
        }

        // Camera owns the lens: feed render_scene a ViewParams built from the
        // (already scene-framed) orbit.
        let view = self.camera.view_params(DEFAULT_FOV);
        let fb = raster_render_scene(
            &self.entities,
            view,
            self.viewport,
            &self.palette,
            &self.config,
        );

        // Blit: framebuffer top-left (x, y) maps to braille dot (x, y) with NO
        // second flip — the projector already flipped Y once (see module docs).
        for (x, y, color) in fb.lit_pixels() {
            painter.paint(x, y, color);
        }
    }
}

/// Render the orbiting World of boxes into the scene `area`.
///
/// Builds a `Marker::Braille` canvas inside a bordered "scene" block and paints
/// a [`SceneShape`] sized to the canvas's braille grid resolution. The viewport
/// is derived from the LIVE inner area every frame, so a resize re-sizes the
/// framebuffer with no stale cache (PITFALLS #14).
///
/// The camera is framed to the WHOLE scene via [`Camera::frame_scene`] (target =
/// scene center, radius solved to fit the bounding sphere), not the unit-cube
/// radius — so the orbit shows the whole rack.
///
/// NOTE (02-04 seam): `entities` is passed in by the caller. Until the app owns a
/// `World`, `ui::mod::view` threads a temporary `world::synthetic_scene()`
/// through; plan 02-04 replaces that temporary with the app-owned World.
pub fn render_scene(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    camera: &Camera,
    entities: &[Entity],
    palette: &Palette,
    config: &RenderConfig,
) {
    let block = Block::default().title("scene").borders(Borders::ALL);

    // Inner area = canvas grid in CELLS (block borders removed). Braille grid
    // resolution is 2 dots wide × 4 dots tall per cell.
    let inner = block.inner(area);
    let viewport = (inner.width as usize * 2, inner.height as usize * 4);

    // Frame the whole rack: target the scene center and solve the orbit radius
    // for the scene bounding sphere (CAM-01), not the unit-cube radius.
    let bounds = crate::world::SceneBounds::from_entities(entities);
    let mut framed = *camera;
    framed.frame_scene(&bounds);

    let shape = SceneShape {
        viewport,
        entities: entities.to_vec(),
        camera: framed,
        palette: *palette,
        config: *config,
    };

    let canvas = Canvas::default()
        .block(block)
        .marker(Marker::Braille)
        .background_color(palette.background)
        // Bounds match the braille dot grid (top-left math via Painter::paint,
        // not these high-level bounds — kept 1:1 with the grid for clarity).
        .x_bounds([0.0, viewport.0.max(1) as f64 - 1.0])
        .y_bounds([0.0, viewport.1.max(1) as f64 - 1.0])
        .paint(|ctx| ctx.draw(&shape));

    frame.render_widget(canvas, area);
}
