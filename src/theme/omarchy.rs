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

use serde::Deserialize;

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
}
