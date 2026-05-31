//! Application configuration.
//!
//! Two structs live here:
//!
//! - [`RenderConfig`] — projection knobs (cell_aspect, fov, near, far). Stays
//!   code-defaulted in v1; NO TOML surface (its values are perceptually load-
//!   bearing and rarely user-tuned at runtime — PITFALLS.md Pitfall 2 on
//!   `cell_aspect`).
//! - [`AppConfig`] — the Phase-5 TOML schema: palette name, hud_visible,
//!   auto_degrade, force_mode, degraded_fps_cap. Loaded from
//!   `~/.config/3dd/config.toml` with graceful per-field defaults.
//!
//! v1 SCOPE NOTE: AppConfig owns ONLY the 5 Phase-5 fields below. AppConfig
//! does NOT compose RenderConfig in v1, and per-color TOML overrides
//! (e.g. `[palette.custom] running = "#aabbcc"`) are OUT OF SCOPE for v1
//! (deferred to v2). THEME-06 v1 = palettes picked BY NAME from TOML.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Rendering configuration: the projection knobs.
///
/// `cell_aspect` is the *only* place the terminal-cell squash is corrected. A
/// braille dot is not square on screen (character cells are ~1:2, twice as tall
/// as wide), so the same world-space unit length would otherwise map to a larger
/// vertical than horizontal on-screen extent — cubes render as bricks. We fold a
/// single named factor into the perspective `aspect` term to undo this.
///
/// # Braille vs kitty perspective parity (05-06 RV3 known limitation)
///
/// The braille tier with `cell_aspect=2.0` produces a projector aspect of
/// `(W*2)/(H*4)/2.0 = W/(4H)`, while the kitty tier (real square pixels,
/// `cell_aspect=1.0`) produces `(W*cw_px)/(H*ch_px) = W*cw/(H*ch)` where
/// `ch_px/cw_px ≈ 2.0` for typical monospace fonts — so kitty's projector
/// aspect is `≈ W/(2H)`, TWICE the braille aspect. A smaller aspect under
/// `perspective_rh` means a larger horizontal-FOV per vertical-FOV, which
/// makes a fixed-size world box project WIDER on screen. Net effect: the
/// braille tier renders boxes ~50% squatter (wider/flatter) than the kitty
/// tier renders the same scene.
///
/// User feedback recorded in 05-06 RV3 (translated): "braille is better
/// [than ASCII] but everything is flattened [compared to kitty]". This is
/// the same projection-aspect asymmetry described above, not a regression.
/// We deliberately keep `cell_aspect=2.0` because Phase 1 calibrated the
/// camera framing (FRAME_REF_CELL_ASPECT, FRAME_TARGET_FILL, the entire
/// `frame_scene` distance solver, and ~40 dependent tests) around it.
/// Lowering it ON THE PROJECTOR ONLY (leaving the framing alone) was tried
/// at 1.5 and 1.0 during RV3 diagnosis: both narrowed boxes horizontally
/// inside the SAME framed window without actually adding vertical extent
/// (the vertical FOV is fixed at `DEFAULT_FOV`), so visible "flatness"
/// changed shape but not magnitude. A correct fix would need to also
/// re-frame the camera against the new projector cell_aspect — a Phase-1
/// invariant-touching change deferred to v2.
///
/// **For now, this is documented as an intrinsic property of the braille
/// tier under the current camera-framing calibration.** The kitty tier is
/// the reference visual; the braille tier is a fallback and accepts a
/// modest aspect-ratio drift in exchange for working on every Unicode-
/// Braille-capable terminal without pixel-protocol support. ASCII tier
/// (RV2: `HalfBlock`) inherits the same braille framing path and therefore
/// the same drift — no additional concern.
///
/// **Future fix sketch (v2):** thread the actual terminal cell pixel size
/// (already available via `crossterm::terminal::window_size`) into a
/// dynamic `RenderConfig.cell_aspect = ch_px / cw_px * 0.5` for braille,
/// AND have the camera re-frame against this dynamic aspect on resize /
/// startup. Touches Phase 1's framing constants — out of scope for 05-06.
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
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
            // Far enough to enclose a whole multi-group rack: the Phase 2 scene
            // spans dozens of units deep (groups are separated Z-bands), and
            // `Camera::frame_scene` pulls the eye back several scene-radii to fit
            // the cone, so the farthest box corner can sit a few hundred units
            // out. The single-cube path is unaffected (its fog is absolute,
            // camera-distance based, not derived from `far`).
            far: 500.0,
        }
    }
}

// ============================================================================
// AppConfig — Phase-5 TOML schema (THEME-06 v1)
// ============================================================================

/// User-facing runtime config — the TOML schema.
///
/// File location:
///   - `$XDG_CONFIG_HOME/3dd/config.toml`, or
///   - `~/.config/3dd/config.toml` (the standard XDG fallback `dirs` resolves).
///
/// Missing file = `Default::default()` (no error). Partial file = missing
/// fields fall back to defaults per-field (`#[serde(default)]`). Unknown
/// fields = parse error (`deny_unknown_fields`) so typos like `pallete = "x"`
/// don't silently produce surprising defaults.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct AppConfig {
    /// Active palette name. One of:
    /// `"notion-soft"` | `"cyberpunk-neon"` | `"terminal-green"` | `"omarchy"`.
    /// Default: `"notion-soft"` (matches `theme::Palette::default()`).
    pub palette: String,

    /// Whether the legend HUD is visible at startup (THEME-05). Default `true`.
    pub hud_visible: bool,

    /// Auto-detect-and-downshift on weak terminals / SSH (ROB-02). Default `true`.
    pub auto_degrade: bool,

    /// Force a specific render mode regardless of terminal capability.
    /// One of: `"auto"` | `"kitty"` | `"truecolor"` | `"ascii"`. Default `"auto"`.
    pub force_mode: String,

    /// FPS cap when degraded (ROB-02). Default `15`.
    pub degraded_fps_cap: u32,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            palette: "notion-soft".to_string(),
            hud_visible: true,
            auto_degrade: true,
            force_mode: "auto".to_string(),
            degraded_fps_cap: 15,
        }
    }
}

// ----------------------------------------------------------------------------
// ConfigError
// ----------------------------------------------------------------------------

/// Errors produced by the config loader. NEVER raised when the file is simply
/// missing — that path returns `Ok(Default::default())`. Raised only when:
/// - the file exists but cannot be read (`Io`), or
/// - the file is readable but does not parse as TOML / has unknown fields /
///   has the wrong type for a known field (`Parse`).
#[derive(Debug)]
pub enum ConfigError {
    /// File exists but cannot be read (e.g. permissions, transient I/O).
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// File read but failed to parse / validate as TOML.
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io { path, source } => write!(
                f,
                "could not read config file {}: {} — check file permissions",
                path.display(),
                source
            ),
            ConfigError::Parse { path, source } => write!(
                f,
                "malformed config file {}: {} — fix the TOML and re-run",
                path.display(),
                source
            ),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Io { source, .. } => Some(source),
            ConfigError::Parse { source, .. } => Some(source),
        }
    }
}

// ----------------------------------------------------------------------------
// Path resolution + loader
// ----------------------------------------------------------------------------

/// Resolve the default config path: `$XDG_CONFIG_HOME/3dd/config.toml` (or
/// `~/.config/3dd/config.toml` on standard XDG-fallback platforms). Returns
/// `None` if neither resolves (e.g. exotic environments where `dirs` can't
/// find a config root).
pub fn default_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("3dd").join("config.toml"))
}

/// Load config from `path` (or the default path when `path` is `None`).
///
/// Semantics (pinned by unit tests):
/// - File MISSING → `Ok(Default::default())`. No error — first-run UX should
///   not require a config file.
/// - File PRESENT + readable + valid TOML → `Ok(parsed)`.
/// - File PRESENT + malformed → `Err(ConfigError::Parse)` with path + the
///   `toml::de::Error` line/col so the user can fix it.
/// - File PRESENT + I/O error (permissions, etc.) → `Err(ConfigError::Io)`.
pub fn load_or_default(path: Option<&Path>) -> Result<AppConfig, ConfigError> {
    // Resolve effective path: explicit override, else the default lookup.
    // If neither yields a path (dirs::config_dir() returned None and no
    // explicit path), we just return the default config silently — there's
    // nowhere to look.
    let resolved: PathBuf = match path {
        Some(p) => p.to_path_buf(),
        None => match default_config_path() {
            Some(p) => p,
            None => return Ok(AppConfig::default()),
        },
    };

    // File missing -> default. NotFound is the ONLY io::ErrorKind we treat as
    // "fall back to defaults"; permission denied / other I/O surfaces as error
    // so the user sees that their config exists but couldn't be read.
    let text = match std::fs::read_to_string(&resolved) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(AppConfig::default());
        }
        Err(e) => {
            return Err(ConfigError::Io {
                path: resolved,
                source: e,
            });
        }
    };

    toml::from_str::<AppConfig>(&text).map_err(|e| ConfigError::Parse {
        path: resolved,
        source: e,
    })
}

/// Load config with CLI-flag override.
///
/// Scans `args` for `--config <PATH>` (two adjacent tokens, mirroring the
/// existing `--dump-rgba <PATH>` pattern in `main.rs`):
/// - PRESENT → load that path. If the file is MISSING, this is an error
///   (explicit override should not silently fall back to defaults — the user
///   asked for a specific file).
/// - ABSENT → fall back to the default path with `load_or_default(None)`.
pub fn load_or_default_with_cli(args: &[String]) -> Result<AppConfig, ConfigError> {
    if let Some(pos) = args.iter().position(|a| a == "--config") {
        if let Some(p) = args.get(pos + 1) {
            let path = PathBuf::from(p);
            // Explicit override: missing file is an Io error (NotFound), not
            // a silent default. The user pointed us at a specific file; if
            // it's not there we should say so loudly.
            let text = std::fs::read_to_string(&path).map_err(|source| ConfigError::Io {
                path: path.clone(),
                source,
            })?;
            return toml::from_str::<AppConfig>(&text).map_err(|source| ConfigError::Parse {
                path,
                source,
            });
        }
        // `--config` with no following token: treat as no override (fall
        // through to the default lookup). Could also be an error but the
        // permissive behavior matches the existing --dump-rgba contract.
    }
    load_or_default(None)
}

// ----------------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a unique temp path (no `tempfile` dep). Caller is responsible for
    /// removing the file at the end of the test.
    fn temp_path(label: &str) -> PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "dd3-config-test-{}-{}-{}.toml",
            std::process::id(),
            label,
            nanos
        ))
    }

    /// Drop-guard that removes a tempfile when the test exits (panic-safe).
    struct CleanupPath(PathBuf);
    impl Drop for CleanupPath {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn default_config_is_notion_soft_palette_with_hud_on() {
        let c = AppConfig::default();
        assert_eq!(c.palette, "notion-soft");
        assert!(c.hud_visible);
        assert!(c.auto_degrade);
        assert_eq!(c.force_mode, "auto");
        assert_eq!(c.degraded_fps_cap, 15);
    }

    #[test]
    fn partial_toml_fills_defaults_per_field() {
        let path = temp_path("partial");
        let _guard = CleanupPath(path.clone());
        std::fs::write(&path, "palette = \"cyberpunk-neon\"\n").unwrap();

        let c = load_or_default(Some(&path)).expect("partial parse ok");
        assert_eq!(c.palette, "cyberpunk-neon");
        // Every other field fell back to its default value:
        assert!(c.hud_visible);
        assert!(c.auto_degrade);
        assert_eq!(c.force_mode, "auto");
        assert_eq!(c.degraded_fps_cap, 15);
    }

    #[test]
    fn unknown_field_is_an_error() {
        let path = temp_path("unknown");
        let _guard = CleanupPath(path.clone());
        // Typo: `pallete` instead of `palette`. deny_unknown_fields catches this.
        std::fs::write(&path, "pallete = \"omarchy\"\n").unwrap();

        let err = load_or_default(Some(&path)).expect_err("unknown field rejected");
        match err {
            ConfigError::Parse { path: p, source } => {
                assert_eq!(p, path);
                // The toml::de::Error message names the offending key.
                let msg = source.to_string();
                assert!(
                    msg.contains("pallete") || msg.contains("unknown field"),
                    "expected toml error to mention pallete or 'unknown field', got: {msg}"
                );
            }
            other => panic!("expected ConfigError::Parse, got: {other:?}"),
        }
    }

    #[test]
    fn malformed_toml_returns_parse_error() {
        let path = temp_path("malformed");
        let _guard = CleanupPath(path.clone());
        // Missing value after `=` — toml parse failure.
        std::fs::write(&path, "palette = \n").unwrap();

        let err = load_or_default(Some(&path)).expect_err("malformed rejected");
        let display = format!("{err}");
        // Display includes the path (so the user can fix it):
        assert!(
            display.contains(path.to_string_lossy().as_ref()),
            "Display should include path, got: {display}"
        );
        assert!(matches!(err, ConfigError::Parse { .. }));
    }

    #[test]
    fn missing_file_returns_default() {
        let path = temp_path("missing-on-purpose");
        // NOTE: we deliberately do NOT create the file.
        assert!(!path.exists());

        let c = load_or_default(Some(&path)).expect("missing file should be ok");
        assert_eq!(c, AppConfig::default());
    }

    #[test]
    fn force_mode_strings_parse() {
        let path = temp_path("force-mode");
        let _guard = CleanupPath(path.clone());
        std::fs::write(&path, "force_mode = \"kitty\"\n").unwrap();

        let c = load_or_default(Some(&path)).expect("force_mode parse ok");
        assert_eq!(c.force_mode, "kitty");
        // Default when omitted: confirmed in default_config_is_notion_soft_*.
        // Sanity-check the other variants also round-trip the parser.
        for s in &["auto", "kitty", "truecolor", "ascii"] {
            let p = temp_path(&format!("force-mode-{s}"));
            let _g = CleanupPath(p.clone());
            std::fs::write(&p, format!("force_mode = \"{s}\"\n")).unwrap();
            let c = load_or_default(Some(&p)).expect("variant parse ok");
            assert_eq!(c.force_mode, *s);
        }
    }

    #[test]
    fn cli_override_uses_path() {
        let path = temp_path("cli-override");
        let _guard = CleanupPath(path.clone());
        std::fs::write(&path, "palette = \"omarchy\"\n").unwrap();

        let args = vec![
            "dd3".to_string(),
            "--config".to_string(),
            path.to_string_lossy().to_string(),
        ];
        let c = load_or_default_with_cli(&args).expect("cli override ok");
        assert_eq!(c.palette, "omarchy");
    }

    #[test]
    fn cli_override_missing_file_is_error() {
        let path = temp_path("cli-override-missing");
        // Do NOT create the file. Explicit override must NOT silently default.
        assert!(!path.exists());

        let args = vec![
            "dd3".to_string(),
            "--config".to_string(),
            path.to_string_lossy().to_string(),
        ];
        let err = load_or_default_with_cli(&args).expect_err("explicit missing override = error");
        assert!(matches!(err, ConfigError::Io { .. }));
    }

    #[test]
    fn default_config_path_resolves_under_3dd() {
        // On every platform `dirs::config_dir()` should resolve (HOME is set);
        // we don't pin the exact prefix because it varies by OS (XDG vs macOS
        // vs Windows), but the suffix is invariant.
        let p = default_config_path().expect("config_dir resolves");
        let s = p.to_string_lossy();
        assert!(s.ends_with("3dd/config.toml") || s.ends_with("3dd\\config.toml"));
    }
}
