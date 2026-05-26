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
            edge: Color::Rgb(0x9A, 0x9A, 0xA8),       // soft gray wireframe (brightened)
            glow: Color::Rgb(0x8A, 0x8A, 0xF0),       // bright indigo accent
            running: Color::Rgb(0x8A, 0x8A, 0xF0),    // bright indigo — alive (vivid faces)
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

    /// Dim `color` toward this palette's background by `factor`, where
    /// `factor == 1.0` keeps the color and `factor == 0.0` collapses it to the
    /// background. Convenience wrapper around [`dim_toward`] using the palette
    /// background as the fog target.
    pub fn fog(&self, color: Color, factor: f32) -> Color {
        dim_toward(color, self.background, factor)
    }
}

/// Linearly interpolate between two RGB colors. `t` is clamped to `[0, 1]`:
/// `t == 0.0` returns `a`, `t == 1.0` returns `b`.
///
/// Only [`Color::Rgb`] inputs are interpolated; if either color is not RGB the
/// nearer endpoint is returned (`a` for `t < 0.5`, else `b`). Phase 5 handles
/// 256/16-color quantization, so non-truecolor cases are passed through here.
pub fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    match (a, b) {
        (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) => Color::Rgb(
            lerp_u8(ar, br, t),
            lerp_u8(ag, bg, t),
            lerp_u8(ab, bb, t),
        ),
        _ => {
            if t < 0.5 {
                a
            } else {
                b
            }
        }
    }
}

/// Depth/fog dimming primitive (supports REND-03).
///
/// `factor` in `0.0..=1.0` controls brightness: `1.0` is the original color,
/// `0.0` is fully dimmed (black). Distant geometry passes a small factor so it
/// reads as fogged. `factor` is clamped to `[0, 1]`.
///
/// Only [`Color::Rgb`] is scaled; non-RGB colors pass through unchanged
/// (Phase 5 adds 256/16-color quantization).
pub fn dim(color: Color, factor: f32) -> Color {
    dim_toward(color, Color::Rgb(0, 0, 0), factor)
}

/// Like [`dim`], but blends toward an explicit `target` (e.g. a palette
/// background) instead of black. `factor == 1.0` keeps `color`; `factor == 0.0`
/// returns `target`. `factor` is clamped to `[0, 1]`. Non-RGB `color` passes
/// through unchanged.
pub fn dim_toward(color: Color, target: Color, factor: f32) -> Color {
    let factor = factor.clamp(0.0, 1.0);
    match color {
        // lerp_color(target, color, factor): factor 1.0 -> color, 0.0 -> target.
        Color::Rgb(..) => lerp_color(target, color, factor),
        other => other,
    }
}

/// Linear interpolation between two channel values, rounded to the nearest u8.
fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    let a = a as f32;
    let b = b as f32;
    (a + (b - a) * t).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: Color = Color::Rgb(255, 255, 255);

    #[test]
    fn dim_full_factor_keeps_color() {
        // factor 1.0 -> original color unchanged.
        assert_eq!(dim(WHITE, 1.0), WHITE);
    }

    #[test]
    fn dim_zero_factor_goes_black() {
        // factor 0.0 -> fully dimmed (black).
        assert_eq!(dim(WHITE, 0.0), Color::Rgb(0, 0, 0));
    }

    #[test]
    fn dim_midpoint_is_between() {
        // factor 0.5 -> midway between black and white (~128 per channel).
        let Color::Rgb(r, g, b) = dim(WHITE, 0.5) else {
            panic!("expected Rgb");
        };
        assert!((120..=136).contains(&r), "r={r}");
        assert_eq!(r, g);
        assert_eq!(g, b);
    }

    #[test]
    fn dim_clamps_out_of_range_factor() {
        // factors outside [0,1] clamp instead of overshooting.
        assert_eq!(dim(WHITE, 5.0), WHITE);
        assert_eq!(dim(WHITE, -2.0), Color::Rgb(0, 0, 0));
    }

    #[test]
    fn dim_passes_through_non_rgb() {
        // Non-truecolor colors are untouched until Phase 5 quantization.
        assert_eq!(dim(Color::Red, 0.3), Color::Red);
    }

    #[test]
    fn fog_blends_toward_background() {
        let pal = Palette::default();
        // factor 1.0 keeps the color; 0.0 collapses to background.
        assert_eq!(pal.fog(WHITE, 1.0), WHITE);
        assert_eq!(pal.fog(WHITE, 0.0), pal.background);
    }

    #[test]
    fn lerp_color_endpoints() {
        let a = Color::Rgb(0, 0, 0);
        let b = Color::Rgb(100, 200, 50);
        assert_eq!(lerp_color(a, b, 0.0), a);
        assert_eq!(lerp_color(a, b, 1.0), b);
    }
}
