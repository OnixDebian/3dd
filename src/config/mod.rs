//! Application configuration.
//!
//! For now everything is defaulted in code; Phase 5 wires TOML loading. The one
//! perceptually load-bearing value here is [`RenderConfig::cell_aspect`] — the
//! single, named terminal cell-aspect correction factor (PITFALLS.md Pitfall 2).

// Consumed by render3d and wired to TOML in Phase 5; defaulted for now.
#![allow(dead_code)]

/// Rendering configuration: the projection knobs.
///
/// `cell_aspect` is the *only* place the terminal-cell squash is corrected. A
/// braille dot is not square on screen (character cells are ~1:2, twice as tall
/// as wide), so the same world-space unit length would otherwise map to a larger
/// vertical than horizontal on-screen extent — cubes render as bricks. We fold a
/// single named factor into the perspective `aspect` term to undo this.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderConfig {
    /// Terminal cell aspect correction factor (on-screen height/width of a dot).
    ///
    /// Default ~2.0 (empirical font/terminal constant; shipped as a tunable knob
    /// because the exact value depends on font and terminal).
    pub cell_aspect: f32,
    /// Vertical field of view, in radians.
    pub fov: f32,
    /// Near clip plane distance (view space).
    pub near: f32,
    /// Far clip plane distance (view space).
    pub far: f32,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            cell_aspect: 2.0,
            fov: std::f32::consts::FRAC_PI_3, // 60 degrees
            near: 0.1,
            far: 100.0,
        }
    }
}
