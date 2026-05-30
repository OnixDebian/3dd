//! THEME-03: derive a Palette from the current Omarchy theme bundle.
//!
//! Omarchy installs a per-theme directory at `~/.config/omarchy/current/theme/`
//! containing (among other tool configs) an `alacritty.toml` with the 16 ANSI
//! slots and a primary background/foreground pair. That single file is the
//! best machine-readable source of "the user's current colors": it is already
//! TOML (no extra format support needed), ships in every omarchy theme bundle,
//! and uses standard ANSI color names (green/red/yellow/blue/cyan/magenta)
//! that map cleanly onto our [`crate::theme::Status`] semantics.
//!
//! This module is purely additive. It parses the file into a small intermediate
//! shape, maps the ANSI slots to a [`crate::theme::Palette`], and exposes a
//! best-effort loader. Missing or malformed files return `None` — the caller
//! (05-04 runtime wiring) falls back to the default preset rather than panic.
//!
//! No visible UI changes in 05-03; this module is the substrate 05-04 binds.
#![allow(dead_code)]

use ratatui::style::Color;
use serde::Deserialize;

use crate::theme::Palette;

// ---------------------------------------------------------------------------
// Intermediate parse shape
// ---------------------------------------------------------------------------

/// Top-level shape of the slice of alacritty.toml we care about. Alacritty's
/// real schema has many more tables (cursor, search, hints, ...); we ignore
/// them silently (NO `deny_unknown_fields`) — the omarchy file is human-edited
/// and may legitimately carry tables we don't care about.
#[derive(Debug, Deserialize)]
pub(crate) struct AlacrittyFile {
    pub(crate) colors: Colors,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Colors {
    #[serde(default)]
    pub(crate) primary: Option<Primary>,
    #[serde(default)]
    pub(crate) normal: Option<AnsiBlock>,
    #[serde(default)]
    pub(crate) bright: Option<AnsiBlock>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Primary {
    #[serde(default)]
    pub(crate) background: Option<String>,
    #[serde(default)]
    pub(crate) foreground: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct AnsiBlock {
    #[serde(default)]
    pub(crate) black: Option<String>,
    #[serde(default)]
    pub(crate) red: Option<String>,
    #[serde(default)]
    pub(crate) green: Option<String>,
    #[serde(default)]
    pub(crate) yellow: Option<String>,
    #[serde(default)]
    pub(crate) blue: Option<String>,
    #[serde(default)]
    pub(crate) magenta: Option<String>,
    #[serde(default)]
    pub(crate) cyan: Option<String>,
    #[serde(default)]
    pub(crate) white: Option<String>,
}

// ---------------------------------------------------------------------------
// Hex parsing
// ---------------------------------------------------------------------------

/// Parse `"#rrggbb"`, `"0xrrggbb"`, or bare `"rrggbb"` into `(r, g, b)` bytes.
/// Returns `None` on malformed input — the caller decides whether to fall back
/// or error out. Whitespace around the input is trimmed.
pub(crate) fn parse_hex_rgb(s: &str) -> Option<(u8, u8, u8)> {
    let s = s.trim();
    let s = s
        .strip_prefix('#')
        .or_else(|| s.strip_prefix("0x"))
        .unwrap_or(s);
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some((r, g, b))
}

// ---------------------------------------------------------------------------
// TOML parsing
// ---------------------------------------------------------------------------

/// Parse an alacritty.toml string into our intermediate shape. Extra tables
/// are ignored; missing color fields default to `None`. Returns `None` on
/// syntax errors — callers fall back to the default preset.
pub(crate) fn parse_alacritty_toml(content: &str) -> Option<AlacrittyFile> {
    toml::from_str::<AlacrittyFile>(content).ok()
}

// ---------------------------------------------------------------------------
// Mapping: AlacrittyFile -> Palette
// ---------------------------------------------------------------------------

/// Push a `Color::Rgb(r,g,b)` slightly toward gray to read as "dormant".
///
/// ANSI `white` is too bright for the Stopped status — a stopped wireframe
/// box drawn in pure ANSI white looks alive next to a live Running box. This
/// helper halves each channel and adds a 30-unit floor, pulling the slot
/// toward a muted gray that reads as off-but-still-visible.
///
/// Math: `c' = c/2 + 30`. At c=255 -> 157; at c=0 -> 30; at c=128 -> 94.
/// The +30 floor keeps the box from being lost against a near-black
/// background.
pub(crate) fn dim_for_stopped(c: Color) -> Color {
    match c {
        Color::Rgb(r, g, b) => {
            Color::Rgb(r / 2 + 30, g / 2 + 30, b / 2 + 30)
        }
        other => other,
    }
}

/// Map a parsed `AlacrittyFile` to a [`Palette`]. None inputs fall back to the
/// notion-soft default for that slot — a half-defined theme still produces a
/// usable palette.
///
/// Mapping rules (semantic ANSI -> status):
///
/// | Palette field | First choice           | Fallback chain                             |
/// |---------------|------------------------|--------------------------------------------|
/// | running       | bright.green           | normal.green -> notion_soft.running         |
/// | crashed       | bright.red             | normal.red -> notion_soft.crashed           |
/// | paused        | normal.yellow          | bright.yellow -> notion_soft.paused         |
/// | restarting    | normal.blue            | bright.blue -> notion_soft.restarting       |
/// | stopped       | dim(normal.white)      | dim(bright.white) -> notion_soft.stopped    |
/// | edge          | normal.white           | bright.white -> notion_soft.edge            |
/// | glow          | bright.magenta         | normal.magenta -> bright.green -> notion_soft.glow |
/// | volume        | normal.cyan            | bright.cyan -> notion_soft.volume           |
/// | background    | primary.background     | notion_soft.background                     |
///
/// The "bright then normal" preference for `running` / `crashed` / `glow`
/// reflects that those are the dominant visual signals — we want them vivid;
/// the "normal then bright" preference for `paused` / `restarting` / `edge` /
/// `volume` keeps secondary cues from washing the scene out.
pub(crate) fn map_to_palette(file: &AlacrittyFile) -> Palette {
    let normal = file.colors.normal.as_ref();
    let bright = file.colors.bright.as_ref();
    let primary = file.colors.primary.as_ref();

    // Helper: resolve `normal.X` first, then `bright.X`, then None.
    let pick = |get: &dyn Fn(&AnsiBlock) -> &Option<String>| -> Option<Color> {
        normal
            .and_then(|b| get(b).as_ref())
            .or_else(|| bright.and_then(|b| get(b).as_ref()))
            .and_then(|hex| parse_hex_rgb(hex))
            .map(|(r, g, b)| Color::Rgb(r, g, b))
    };

    // Helper: resolve `bright.X` only (no fallback to normal).
    let bright_only =
        |get: &dyn Fn(&AnsiBlock) -> &Option<String>| -> Option<Color> {
            bright
                .and_then(|b| get(b).as_ref())
                .and_then(|hex| parse_hex_rgb(hex))
                .map(|(r, g, b)| Color::Rgb(r, g, b))
        };

    let primary_bg = primary
        .and_then(|p| p.background.as_ref())
        .and_then(|hex| parse_hex_rgb(hex))
        .map(|(r, g, b)| Color::Rgb(r, g, b));

    let fallback = Palette::notion_soft();
    Palette {
        background: primary_bg.unwrap_or(fallback.background),
        edge: pick(&|b| &b.white).unwrap_or(fallback.edge),
        glow: bright_only(&|b| &b.magenta)
            .or_else(|| pick(&|b| &b.magenta))
            .or_else(|| bright_only(&|b| &b.green))
            .unwrap_or(fallback.glow),
        volume: pick(&|b| &b.cyan).unwrap_or(fallback.volume),
        running: bright_only(&|b| &b.green)
            .or_else(|| pick(&|b| &b.green))
            .unwrap_or(fallback.running),
        paused: pick(&|b| &b.yellow).unwrap_or(fallback.paused),
        stopped: pick(&|b| &b.white)
            .map(dim_for_stopped)
            .unwrap_or(fallback.stopped),
        restarting: pick(&|b| &b.blue).unwrap_or(fallback.restarting),
        crashed: bright_only(&|b| &b.red)
            .or_else(|| pick(&|b| &b.red))
            .unwrap_or(fallback.crashed),
    }
}

// ---------------------------------------------------------------------------
// Filesystem entry points
// ---------------------------------------------------------------------------

/// Default path: `$XDG_CONFIG_HOME/omarchy/current/theme/alacritty.toml`.
/// Returns `None` if no config-dir can be resolved (no `$HOME`, no
/// `$XDG_CONFIG_HOME` — vanishingly rare on a real desktop).
pub(crate) fn default_omarchy_alacritty_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|d| {
        d.join("omarchy")
            .join("current")
            .join("theme")
            .join("alacritty.toml")
    })
}

/// Best-effort load of the current omarchy theme's alacritty.toml.
///
/// - `path = None` -> use [`default_omarchy_alacritty_path`].
/// - `path = Some(p)` -> read `p` directly (used by tests).
///
/// Returns `None` if the file is missing, unreadable, or unparseable. The
/// caller (the 05-04 runtime wiring) falls back to the default preset.
pub fn load_omarchy_palette(
    path: Option<&std::path::Path>,
) -> Option<Palette> {
    let path: std::path::PathBuf = match path {
        Some(p) => p.to_path_buf(),
        None => default_omarchy_alacritty_path()?,
    };
    let content = std::fs::read_to_string(&path).ok()?;
    let file = parse_alacritty_toml(&content)?;
    Some(map_to_palette(&file))
}

// ---------------------------------------------------------------------------
// Tests (Task 1: parse layer)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- parse_hex_rgb ----------------------------------------------------

    #[test]
    fn parse_hex_rgb_handles_hash_prefix() {
        assert_eq!(parse_hex_rgb("#1a1b26"), Some((0x1a, 0x1b, 0x26)));
    }

    #[test]
    fn parse_hex_rgb_handles_0x_prefix() {
        assert_eq!(parse_hex_rgb("0x1a1b26"), Some((0x1a, 0x1b, 0x26)));
    }

    #[test]
    fn parse_hex_rgb_handles_no_prefix() {
        assert_eq!(parse_hex_rgb("1a1b26"), Some((0x1a, 0x1b, 0x26)));
    }

    #[test]
    fn parse_hex_rgb_trims_whitespace() {
        assert_eq!(parse_hex_rgb("  #1a1b26  "), Some((0x1a, 0x1b, 0x26)));
    }

    #[test]
    fn parse_hex_rgb_rejects_malformed() {
        // Non-hex chars.
        assert_eq!(parse_hex_rgb("#xyz123"), None);
        // 5 chars after stripping prefix.
        assert_eq!(parse_hex_rgb("#1a1b2"), None);
        // 7 chars after stripping prefix.
        assert_eq!(parse_hex_rgb("#1a1b260"), None);
        // Empty.
        assert_eq!(parse_hex_rgb(""), None);
        // Only the prefix.
        assert_eq!(parse_hex_rgb("#"), None);
        // 3-char shorthand (`#fff`) — not supported (alacritty doesn't use it).
        assert_eq!(parse_hex_rgb("#fff"), None);
    }

    // -- parse_alacritty_toml --------------------------------------------

    #[test]
    fn parse_alacritty_toml_ignores_extra_tables() {
        let content = r##"
[colors.primary]
background = "#1a1b26"
foreground = "#a9b1d6"

[colors.normal]
green = "#9ece6a"
red = "#f7768e"

[colors.search.matches]
foreground = "#1a1b26"
background = "#e0af68"
"##;
        let file = parse_alacritty_toml(content).expect("should parse");
        let primary = file.colors.primary.expect("primary present");
        assert_eq!(primary.background.as_deref(), Some("#1a1b26"));
        assert_eq!(primary.foreground.as_deref(), Some("#a9b1d6"));
        let normal = file.colors.normal.expect("normal present");
        assert_eq!(normal.green.as_deref(), Some("#9ece6a"));
        assert_eq!(normal.red.as_deref(), Some("#f7768e"));
        // Untouched fields stay None.
        assert!(normal.blue.is_none());
    }

    #[test]
    fn parse_alacritty_toml_partial_normal_block() {
        // Only green is set inside [colors.normal]. Every other slot is None.
        let content = r##"
[colors.normal]
green = "#33ff33"
"##;
        let file = parse_alacritty_toml(content).expect("should parse");
        let normal = file.colors.normal.expect("normal present");
        assert_eq!(normal.green.as_deref(), Some("#33ff33"));
        assert!(normal.red.is_none());
        assert!(normal.blue.is_none());
        assert!(normal.yellow.is_none());
        assert!(normal.cyan.is_none());
        assert!(normal.magenta.is_none());
        assert!(normal.white.is_none());
        assert!(normal.black.is_none());
        // No [colors.primary] / [colors.bright] -> both top-level options None.
        assert!(file.colors.primary.is_none());
        assert!(file.colors.bright.is_none());
    }

    #[test]
    fn parse_alacritty_toml_malformed_returns_none() {
        // Broken syntax: unfinished assignment.
        let content = "colors.primary = \n";
        assert!(parse_alacritty_toml(content).is_none());
        // Stray garbage.
        assert!(parse_alacritty_toml("not toml at all !@#$").is_none());
    }

    #[test]
    fn parse_alacritty_toml_empty_returns_default_shape() {
        // Empty file is valid TOML but has no [colors] table -> deserialize fails
        // because `colors` field is required. We accept the None result so
        // callers fall back to the default preset.
        assert!(parse_alacritty_toml("").is_none());
    }

    // -- map_to_palette ---------------------------------------------------

    /// Real-world tokyonight fixture as verified on the host at
    /// `~/.config/omarchy/current/theme/alacritty.toml`. Used by several tests
    /// below to keep the mapping rules pinned against a representative input.
    const TOKYONIGHT_FIXTURE: &str = r##"
[colors.primary]
background = "#1a1b26"
foreground = "#a9b1d6"

[colors.normal]
black   = "#32344a"
red     = "#f7768e"
green   = "#9ece6a"
yellow  = "#e0af68"
blue    = "#7aa2f7"
magenta = "#ad8ee6"
cyan    = "#449dab"
white   = "#787c99"

[colors.bright]
black   = "#444b6a"
red     = "#ff7a93"
green   = "#b9f27c"
yellow  = "#ff9e64"
blue    = "#7da6ff"
magenta = "#bb9af7"
cyan    = "#0db9d7"
white   = "#acb0d0"
"##;

    #[test]
    fn map_to_palette_uses_bright_green_for_running() {
        // bright.green wins over normal.green for the running slot — running
        // is the dominant signal and should be vivid.
        let file = parse_alacritty_toml(TOKYONIGHT_FIXTURE).expect("parses");
        let p = map_to_palette(&file);
        assert_eq!(p.running, Color::Rgb(0xb9, 0xf2, 0x7c));
    }

    #[test]
    fn map_to_palette_falls_back_to_normal_green_when_no_bright() {
        // No [colors.bright] block at all -> running falls back to normal.green.
        let content = r##"
[colors.primary]
background = "#000000"
[colors.normal]
green = "#33aa33"
"##;
        let file = parse_alacritty_toml(content).expect("parses");
        let p = map_to_palette(&file);
        assert_eq!(p.running, Color::Rgb(0x33, 0xaa, 0x33));
    }

    #[test]
    fn map_to_palette_uses_primary_background() {
        let file = parse_alacritty_toml(TOKYONIGHT_FIXTURE).expect("parses");
        let p = map_to_palette(&file);
        assert_eq!(p.background, Color::Rgb(0x1a, 0x1b, 0x26));
    }

    #[test]
    fn map_to_palette_partial_file_fills_with_fallback() {
        // Only primary.background is set; every other field should fall back
        // to notion_soft.
        let content = r##"
[colors.primary]
background = "#0a0a0a"
"##;
        let file = parse_alacritty_toml(content).expect("parses");
        let p = map_to_palette(&file);
        let fb = Palette::notion_soft();
        // Honored field:
        assert_eq!(p.background, Color::Rgb(0x0a, 0x0a, 0x0a));
        // Every other field falls back:
        assert_eq!(p.edge, fb.edge);
        assert_eq!(p.glow, fb.glow);
        assert_eq!(p.volume, fb.volume);
        assert_eq!(p.running, fb.running);
        assert_eq!(p.paused, fb.paused);
        assert_eq!(p.stopped, fb.stopped);
        assert_eq!(p.restarting, fb.restarting);
        assert_eq!(p.crashed, fb.crashed);
    }

    #[test]
    fn map_to_palette_malformed_hex_falls_back() {
        // A hex value present but malformed -> that slot falls back to notion_soft.
        let content = r##"
[colors.primary]
background = "not-a-hex"
[colors.normal]
green = "#xxxxxx"
"##;
        let file = parse_alacritty_toml(content).expect("parses");
        let p = map_to_palette(&file);
        let fb = Palette::notion_soft();
        assert_eq!(p.background, fb.background);
        assert_eq!(p.running, fb.running);
    }

    #[test]
    fn map_to_palette_full_omarchy_tokyonight_fixture() {
        // The regression net for the mapping rules — every field pinned to
        // the expected derived value from the real-world host's file.
        let file = parse_alacritty_toml(TOKYONIGHT_FIXTURE).expect("parses");
        let p = map_to_palette(&file);

        // primary.background.
        assert_eq!(p.background, Color::Rgb(0x1a, 0x1b, 0x26));
        // normal.white (preferred over bright.white).
        assert_eq!(p.edge, Color::Rgb(0x78, 0x7c, 0x99));
        // bright.magenta wins (cyberpunk-ish accent).
        assert_eq!(p.glow, Color::Rgb(0xbb, 0x9a, 0xf7));
        // normal.cyan (preferred over bright.cyan).
        assert_eq!(p.volume, Color::Rgb(0x44, 0x9d, 0xab));
        // bright.green wins for running.
        assert_eq!(p.running, Color::Rgb(0xb9, 0xf2, 0x7c));
        // normal.yellow (preferred over bright.yellow).
        assert_eq!(p.paused, Color::Rgb(0xe0, 0xaf, 0x68));
        // dim_for_stopped(normal.white) — computed via the fn so the test
        // doesn't duplicate the dim formula.
        assert_eq!(
            p.stopped,
            dim_for_stopped(Color::Rgb(0x78, 0x7c, 0x99))
        );
        // normal.blue (preferred over bright.blue).
        assert_eq!(p.restarting, Color::Rgb(0x7a, 0xa2, 0xf7));
        // bright.red wins for crashed.
        assert_eq!(p.crashed, Color::Rgb(0xff, 0x7a, 0x93));
    }

    #[test]
    fn dim_for_stopped_pulls_white_toward_gray() {
        // ANSI white -> moderately dim. 255/2 + 30 = 157.
        assert_eq!(
            dim_for_stopped(Color::Rgb(255, 255, 255)),
            Color::Rgb(157, 157, 157)
        );
        // 0 -> 30 (the floor keeps the box visible).
        assert_eq!(
            dim_for_stopped(Color::Rgb(0, 0, 0)),
            Color::Rgb(30, 30, 30)
        );
        // Non-RGB passes through.
        assert_eq!(dim_for_stopped(Color::Red), Color::Red);
    }

    // -- load_omarchy_palette ---------------------------------------------

    #[test]
    fn load_omarchy_palette_missing_file_returns_none() {
        // Explicit path to a guaranteed-nonexistent location.
        let p = std::path::PathBuf::from("/nonexistent/path/no_omarchy_here.toml");
        assert!(load_omarchy_palette(Some(&p)).is_none());
    }

    #[test]
    fn load_omarchy_palette_malformed_file_returns_none() {
        // Write deliberately-broken TOML to a tempfile and confirm None.
        let tmp = std::env::temp_dir().join("dd3_omarchy_malformed_5_03.toml");
        std::fs::write(&tmp, "not toml at all !@#$\n").expect("tempfile write");
        let result = load_omarchy_palette(Some(&tmp));
        // Clean up first so a failed assertion doesn't leave the tempfile.
        let _ = std::fs::remove_file(&tmp);
        assert!(result.is_none(), "malformed file should return None");
    }

    #[test]
    fn load_omarchy_palette_reads_real_fixture_from_disk() {
        // Round-trip: write the tokyonight fixture, load via the file path,
        // confirm the resulting Palette matches map_to_palette on the same
        // content. Verifies the file -> parse -> map pipeline end-to-end.
        let tmp = std::env::temp_dir().join("dd3_omarchy_fixture_5_03.toml");
        std::fs::write(&tmp, TOKYONIGHT_FIXTURE).expect("tempfile write");
        let result = load_omarchy_palette(Some(&tmp));
        let _ = std::fs::remove_file(&tmp);

        let expected =
            map_to_palette(&parse_alacritty_toml(TOKYONIGHT_FIXTURE).unwrap());
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn default_omarchy_alacritty_path_points_at_current_theme() {
        // Whatever dirs::config_dir resolves to on this host, the suffix must
        // be /omarchy/current/theme/alacritty.toml — that's the omarchy
        // contract this whole module rests on.
        let p = default_omarchy_alacritty_path()
            .expect("config dir resolvable on a real host");
        let s = p.to_string_lossy();
        assert!(
            s.ends_with("/omarchy/current/theme/alacritty.toml"),
            "expected omarchy path suffix, got: {s}"
        );
    }
}
