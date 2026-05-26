//! Palette / theme abstraction (THEME-01).
//!
//! Every drawable color in the renderer is resolved through a [`Palette`]. No
//! draw site (rasterizer in plan 04, scene in plan 05) names an RGB value
//! directly — they ask the palette for a status color, the background, the
//! edge/glow color, or run [`dim`] for distance fog. This is the single place
//! where RGB literals are allowed to live, which is what prevents the
//! "inline color scattered everywhere" debt that PITFALLS.md flags as
//! MEDIUM-cost to retrofit.
//!
//! The full theming system (presets, TOML config, runtime switching,
//! 256/16-color quantization) lands in Phase 5; this module is intentionally
//! small but complete for what plans 04/05 need.
//!
//! NOTE: this is a forward-facing API — plans 04 (rasterizer) and 05 (scene)
//! are the first consumers, so until then the items below are reported as dead
//! code. The module-level `allow` keeps the abstraction in place without
//! masking dead code elsewhere in the crate.
#![allow(dead_code)]

use ratatui::style::Color;

/// Container lifecycle status (mirrors CONT-01). Defined here so the
/// status -> color mapping has a home now; Phase 2/3 reuse the same enum when
/// real Docker state arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    Running,
    Paused,
    Stopped,
    Restarting,
    Crashed,
}

/// The set of colors the renderer is allowed to draw with.
///
/// Prefer truecolor [`Color::Rgb`] values here; Phase 5 adds quantization for
/// 256/16-color terminals. The fields cover exactly what plans 04/05 need:
/// face/edge color, glow accent, background, and one color per [`Status`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// Scene background / fog target.
    pub background: Color,
    /// Cube/face edge (wireframe) color.
    pub edge: Color,
    /// Accent used for glow / highlights / "alive" emphasis.
    pub glow: Color,
    /// Status: running.
    pub running: Color,
    /// Status: paused.
    pub paused: Color,
    /// Status: stopped.
    pub stopped: Color,
    /// Status: restarting.
    pub restarting: Color,
    /// Status: crashed.
    pub crashed: Color,
}

impl Default for Palette {
    /// A Notion-soft default: indigo `#5B5BD6` accent for running/glow against a
    /// muted near-black background, with a distinct, legible hue per status.
    ///
    /// These RGB literals are the ONLY ones in the whole renderer.
    fn default() -> Self {
        Self {
            background: Color::Rgb(0x1A, 0x1A, 0x22), // muted near-black with a faint indigo tint
            edge: Color::Rgb(0x6B, 0x6B, 0x78),       // soft gray wireframe
            glow: Color::Rgb(0x5B, 0x5B, 0xD6),       // indigo accent (#5B5BD6)
            running: Color::Rgb(0x5B, 0x5B, 0xD6),    // indigo — alive
            paused: Color::Rgb(0xE2, 0xB1, 0x4F),     // amber — held
            stopped: Color::Rgb(0x6B, 0x6B, 0x78),    // gray — dormant
            restarting: Color::Rgb(0x4F, 0xA6, 0xE2), // cyan-blue — in flux
            crashed: Color::Rgb(0xE2, 0x5B, 0x5B),    // red — failed
        }
    }
}

impl Palette {
    /// Resolve a [`Status`] to its palette color. The renderer calls this
    /// instead of naming a color inline.
    pub fn status_color(&self, status: Status) -> Color {
        match status {
            Status::Running => self.running,
            Status::Paused => self.paused,
            Status::Stopped => self.stopped,
            Status::Restarting => self.restarting,
            Status::Crashed => self.crashed,
        }
    }
}
