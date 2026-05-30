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

pub mod omarchy;

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

impl Status {
    /// Visual mode: `Running` renders as a solid (filled) box; every other status
    /// renders as a wireframe (only the 12 cube edges, transparent faces). The
    /// renderer asks here instead of pattern-matching the enum directly so the
    /// rule has a single home.
    pub fn is_solid(self) -> bool {
        matches!(self, Status::Running)
    }
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
    /// Volume cylinder color (ENT-03). A muted teal that reads as "data"
    /// without competing with status colors (running green / crashed red).
    pub volume: Color,
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
    /// Delegates to [`Palette::notion_soft`] — the single source of truth for
    /// these RGB literals (THEME-02). Every existing caller that depends on
    /// `Palette::default()` resolves to the same colors the renderer was tuned
    /// against in Phases 1–4.
    fn default() -> Self {
        Self::notion_soft()
    }
}

impl Palette {
    /// Notion-soft (the existing default — see PROJECT.md "design DNA").
    /// Indigo `#5B5BD6` accent against a muted near-black background. This is
    /// the palette the renderer was tuned against in Phases 1–4, so it stays
    /// the [`Default`] for full backward compatibility with snapshot tests.
    ///
    /// These RGB literals — together with the other two presets below — are
    /// the ONLY truecolor literals allowed in the whole renderer (THEME-01).
    pub fn notion_soft() -> Self {
        Self {
            background: Color::Rgb(0x1A, 0x1A, 0x22), // muted near-black with a faint indigo tint
            edge: Color::Rgb(0x9A, 0x9A, 0xA8),       // soft gray — wireframe color for non-Running
            glow: Color::Rgb(0x8A, 0x8A, 0xF0),       // bright indigo accent
            volume: Color::Rgb(0x50, 0x8C, 0x8C),     // muted teal — volume cylinders (ENT-03)
            running: Color::Rgb(0x7F, 0xE0, 0x8A),    // light green — alive (vivid faces)
            paused: Color::Rgb(0xE2, 0xB1, 0x4F),     // amber — held
            stopped: Color::Rgb(0x6B, 0x6B, 0x78),    // gray — dormant
            restarting: Color::Rgb(0x4F, 0xA6, 0xE2), // cyan-blue — in flux
            crashed: Color::Rgb(0xE2, 0x5B, 0x5B),    // red — failed
        }
    }

    /// Cyberpunk-neon: high-contrast magenta / cyan / lime against deep black.
    /// Reads as "screen of a hacker thriller". Glow is hot magenta; running is
    /// electric lime; crashed is hazard red-orange. Status colors are pulled
    /// apart in hue so even at small sizes the lifecycle reads.
    pub fn cyberpunk_neon() -> Self {
        Self {
            background: Color::Rgb(0x0A, 0x06, 0x12), // near-black with purple bias
            edge: Color::Rgb(0x4A, 0x36, 0x6E),       // muted indigo edge
            glow: Color::Rgb(0xFF, 0x2D, 0x95),       // hot magenta
            volume: Color::Rgb(0x00, 0xE5, 0xE5),     // electric cyan
            running: Color::Rgb(0xC0, 0xFF, 0x33),    // electric lime
            paused: Color::Rgb(0xFF, 0xC8, 0x00),     // saturated amber
            stopped: Color::Rgb(0x55, 0x4A, 0x68),    // muted indigo-gray
            restarting: Color::Rgb(0x00, 0xB4, 0xFF), // bright cyan
            crashed: Color::Rgb(0xFF, 0x3A, 0x14),    // hazard red-orange
        }
    }

    /// Terminal-green: monochrome phosphor green on black, the classic
    /// VT100 / IBM 3270 look. Status colors stay within the green / amber /
    /// gray family to preserve the monochrome feel; `crashed` breaks the rule
    /// (only red — without that break, a crashed container disappears into a
    /// stopped one).
    pub fn terminal_green() -> Self {
        Self {
            background: Color::Rgb(0x00, 0x0A, 0x00), // CRT black
            edge: Color::Rgb(0x40, 0x80, 0x40),       // dim green
            glow: Color::Rgb(0x80, 0xFF, 0xA0),       // bright phosphor
            volume: Color::Rgb(0x40, 0xA0, 0x40),     // muted green (same family)
            running: Color::Rgb(0x33, 0xFF, 0x33),    // CRT green
            paused: Color::Rgb(0xC8, 0xC8, 0x40),     // dim amber-green
            stopped: Color::Rgb(0x40, 0x60, 0x40),    // dim green-gray
            restarting: Color::Rgb(0x80, 0xC8, 0x80), // mid green
            crashed: Color::Rgb(0xFF, 0x40, 0x40),    // red (intentional break)
        }
    }

    /// Resolve a palette by name. Unknown names return `None` — the caller
    /// decides whether to log, error, or silently default. Accepts both the
    /// human-friendly kebab form (`"cyberpunk-neon"`) and the snake form
    /// (`"cyberpunk_neon"`) to be forgiving with TOML.
    pub fn by_name(name: &str) -> Option<Self> {
        match name.replace('_', "-").as_str() {
            "notion-soft" => Some(Self::notion_soft()),
            "cyberpunk-neon" => Some(Self::cyberpunk_neon()),
            "terminal-green" => Some(Self::terminal_green()),
            _ => None,
        }
    }

    /// Like [`Palette::by_name`] but never `None` — unknown names fall back to
    /// [`Palette::notion_soft`]. Used by `main.rs` when honoring
    /// `AppConfig.palette`: a typo'd palette name must never crash the app.
    pub fn by_name_or_default(name: &str) -> Self {
        Self::by_name(name).unwrap_or_else(Self::notion_soft)
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

    // -- THEME-02: named palette presets ----------------------------------

    /// Pins backward compatibility for every existing snapshot / visual test:
    /// `Palette::notion_soft()` MUST return the exact RGB triplets that
    /// `Default::default()` used to inline before THEME-02 landed.
    #[test]
    fn notion_soft_matches_legacy_default() {
        let p = Palette::notion_soft();
        assert_eq!(p.background, Color::Rgb(0x1A, 0x1A, 0x22));
        assert_eq!(p.edge, Color::Rgb(0x9A, 0x9A, 0xA8));
        assert_eq!(p.glow, Color::Rgb(0x8A, 0x8A, 0xF0));
        assert_eq!(p.volume, Color::Rgb(0x50, 0x8C, 0x8C));
        assert_eq!(p.running, Color::Rgb(0x7F, 0xE0, 0x8A));
        assert_eq!(p.paused, Color::Rgb(0xE2, 0xB1, 0x4F));
        assert_eq!(p.stopped, Color::Rgb(0x6B, 0x6B, 0x78));
        assert_eq!(p.restarting, Color::Rgb(0x4F, 0xA6, 0xE2));
        assert_eq!(p.crashed, Color::Rgb(0xE2, 0x5B, 0x5B));
    }

    /// `Default::default()` must keep delegating to `notion_soft` — every
    /// existing call site relies on this for visual continuity.
    #[test]
    fn default_equals_notion_soft() {
        assert_eq!(Palette::default(), Palette::notion_soft());
    }

    /// Visual distinguishability: each preset's `running` color must differ
    /// from the other two — running boxes are the dominant signal, and the
    /// whole point of a preset is for it to read differently.
    #[test]
    fn three_presets_have_distinct_running_colors() {
        let n = Palette::notion_soft().running;
        let c = Palette::cyberpunk_neon().running;
        let t = Palette::terminal_green().running;
        assert_ne!(n, c, "notion_soft and cyberpunk_neon share running color");
        assert_ne!(n, t, "notion_soft and terminal_green share running color");
        assert_ne!(c, t, "cyberpunk_neon and terminal_green share running color");
    }

    /// Visual distinguishability: each preset's `glow` (selection / accent)
    /// must differ from the other two so the selected-container pulse reads
    /// as a different color cue per palette.
    #[test]
    fn three_presets_have_distinct_glow_colors() {
        let n = Palette::notion_soft().glow;
        let c = Palette::cyberpunk_neon().glow;
        let t = Palette::terminal_green().glow;
        assert_ne!(n, c, "notion_soft and cyberpunk_neon share glow color");
        assert_ne!(n, t, "notion_soft and terminal_green share glow color");
        assert_ne!(c, t, "cyberpunk_neon and terminal_green share glow color");
    }

    /// Dispatcher contract: every known name resolves to its preset; an
    /// unknown name returns `None` (caller decides default policy).
    #[test]
    fn by_name_matches_each_preset() {
        assert_eq!(Palette::by_name("notion-soft"), Some(Palette::notion_soft()));
        assert_eq!(
            Palette::by_name("cyberpunk-neon"),
            Some(Palette::cyberpunk_neon())
        );
        assert_eq!(
            Palette::by_name("terminal-green"),
            Some(Palette::terminal_green())
        );
        assert_eq!(Palette::by_name("no-such-palette"), None);
    }

    /// TOML often uses snake_case; CLI users often type kebab-case. Both
    /// must resolve to the same preset.
    #[test]
    fn by_name_accepts_snake_and_kebab_case() {
        assert_eq!(
            Palette::by_name("cyberpunk_neon"),
            Palette::by_name("cyberpunk-neon")
        );
        assert_eq!(
            Palette::by_name("terminal_green"),
            Palette::by_name("terminal-green")
        );
        assert_eq!(
            Palette::by_name("notion_soft"),
            Palette::by_name("notion-soft")
        );
    }

    /// `by_name_or_default` never returns `None`: unknown names fall back to
    /// `notion_soft`. Used by main.rs so a typo'd config value can't crash.
    #[test]
    fn by_name_or_default_falls_back_to_notion_soft() {
        assert_eq!(
            Palette::by_name_or_default("nope-not-real"),
            Palette::notion_soft()
        );
        assert_eq!(
            Palette::by_name_or_default("cyberpunk-neon"),
            Palette::cyberpunk_neon()
        );
    }

    /// Truecolor invariant: every preset field must be `Color::Rgb(_,_,_)`.
    /// The renderer's fog / dim math passes non-RGB colors through unchanged,
    /// which is wrong — a preset slipping in `Color::Reset` or a 16-color
    /// named variant would silently break depth shading.
    #[test]
    fn every_preset_has_all_fields_as_rgb() {
        for p in [
            Palette::notion_soft(),
            Palette::cyberpunk_neon(),
            Palette::terminal_green(),
        ] {
            for c in [
                p.background,
                p.edge,
                p.glow,
                p.volume,
                p.running,
                p.paused,
                p.stopped,
                p.restarting,
                p.crashed,
            ] {
                assert!(
                    matches!(c, Color::Rgb(_, _, _)),
                    "preset field is not Color::Rgb: {c:?}"
                );
            }
        }
    }
}
