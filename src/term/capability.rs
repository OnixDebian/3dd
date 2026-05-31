//! Terminal capability detection (ROB-02).
//!
//! Classifies the current terminal environment into one of three tiers:
//! [`TerminalCapability::Kitty`] (full graphics protocol available),
//! [`TerminalCapability::Truecolor`] (full RGB cell output, e.g. Alacritty
//! / Wezterm without graphics), or [`TerminalCapability::Ascii`] (degraded
//! — SSH, dumb terminals, missing COLORTERM).
//!
//! Inputs are env vars only — we do NOT issue a terminal capability query
//! (DA / kitty graphics probe) because the user has consciously launched
//! 3dd; an extra escape-sequence handshake before raw mode is brittle on
//! slow SSH links and complicates the Pitfall-9 probe-before-raw-mode
//! contract.
//!
//! Detection precedence (HIGHEST wins):
//! 1. KITTY_WINDOW_ID / GHOSTTY_RESOURCES_DIR / WEZTERM_PANE present
//!    → Kitty
//! 2. TERM contains "kitty" or "ghostty" → Kitty
//! 3. SSH_CONNECTION or SSH_CLIENT present AND COLORTERM is empty
//!    → Ascii (slow link, assume conservative)
//! 4. TERM in {"dumb", "vt100", "linux"} → Ascii
//! 5. COLORTERM == "truecolor" or "24bit" → Truecolor
//! 6. TERM starts with "xterm-256color" or "alacritty" → Truecolor
//!    (these typically support truecolor even when COLORTERM unset)
//! 7. Default → Truecolor (most modern terminals support it)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalCapability {
    /// Kitty graphics protocol available — best path, real pixels.
    Kitty,
    /// Truecolor cell rendering — braille + 24-bit RGB SGR works.
    Truecolor,
    /// Degraded — coarse markers + capped FPS over SSH or dumb terminals.
    Ascii,
}

impl TerminalCapability {
    /// Stable lowercase tag for the status bar / logs.
    #[allow(dead_code)] // Wired by main.rs + status_bar in Task 2 of 05-06.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Kitty => "kitty",
            Self::Truecolor => "truecolor",
            Self::Ascii => "ascii",
        }
    }

    /// Parse a `force_mode` string from the TOML config or CLI flag.
    /// Returns `None` on unrecognized input so the caller falls through to
    /// auto-detect (typo'd config still produces a usable app).
    ///
    /// `"braille"` maps to [`Self::Truecolor`] because the braille cell
    /// renderer IS the truecolor tier on the 3dd side — the user thinks
    /// in terms of the renderer, the resolver thinks in terms of the
    /// capability class.
    #[allow(dead_code)] // Wired by main.rs CLI flag mapper in Task 2 of 05-06.
    pub fn from_force_mode(s: &str) -> Option<Self> {
        match s {
            "kitty" => Some(Self::Kitty),
            "braille" => Some(Self::Truecolor),
            "ascii" => Some(Self::Ascii),
            _ => None,
        }
    }
}

/// Detect via env vars. Pure — same inputs always yield same output.
/// Public for unit testing via `env::set_var` in tests; production calls
/// it from main.rs before raw mode.
pub fn detect() -> TerminalCapability {
    // Tier 1: explicit kitty-family env vars.
    if std::env::var_os("KITTY_WINDOW_ID").is_some()
        || std::env::var_os("GHOSTTY_RESOURCES_DIR").is_some()
        || std::env::var_os("WEZTERM_PANE").is_some()
    {
        return TerminalCapability::Kitty;
    }
    let term = std::env::var("TERM").unwrap_or_default();
    if term.contains("kitty") || term.contains("ghostty") {
        return TerminalCapability::Kitty;
    }

    // Tier 2: SSH + no COLORTERM = conservative degrade.
    let in_ssh = std::env::var_os("SSH_CONNECTION").is_some()
        || std::env::var_os("SSH_CLIENT").is_some();
    let colorterm = std::env::var("COLORTERM").unwrap_or_default();
    if in_ssh && colorterm.is_empty() {
        return TerminalCapability::Ascii;
    }

    // Tier 3: dumb / vt100 / linux console.
    if matches!(term.as_str(), "dumb" | "vt100" | "linux") {
        return TerminalCapability::Ascii;
    }

    // Tier 4: COLORTERM says truecolor.
    if colorterm == "truecolor" || colorterm == "24bit" {
        return TerminalCapability::Truecolor;
    }

    // Tier 5: well-known truecolor TERM values.
    if term.starts_with("xterm-256color") || term.starts_with("alacritty") {
        return TerminalCapability::Truecolor;
    }

    // Default: most modern terminals support truecolor.
    TerminalCapability::Truecolor
}

/// Resolve the EFFECTIVE capability from config + auto-detect.
///
/// - If `force_mode` is set and parseable → that wins.
/// - Else if `auto_degrade == false` → auto-detect but NEVER return Ascii;
///   if detection said Ascii, override to Truecolor (the user opted out
///   of automatic degrade).
/// - Else → auto-detect.
///
/// An unknown `force_mode` string (typo, future variant) falls through to
/// auto-detect rather than panicking — the typo still produces a usable
/// app, and we eprintln on the way in (see main.rs caller) so the user
/// notices.
#[allow(dead_code)] // Wired by main.rs backend-pick in Task 2 of 05-06.
pub fn resolve(force_mode: Option<&str>, auto_degrade: bool) -> TerminalCapability {
    if let Some(s) = force_mode {
        if let Some(forced) = TerminalCapability::from_force_mode(s) {
            return forced;
        }
        // Unknown force_mode string: fall through to auto-detect (don't
        // crash — typo'd config should still produce a usable app).
    }
    let detected = detect();
    if !auto_degrade && detected == TerminalCapability::Ascii {
        return TerminalCapability::Truecolor;
    }
    detected
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // These tests touch process-global env vars. cargo runs tests in
    // parallel by default — serialize them through a Mutex so we don't
    // race on KITTY_WINDOW_ID / TERM / SSH_CONNECTION between threads.
    // `static MUTEX: Mutex<()> = Mutex::new(());` is stable since 1.63.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    /// Save+restore an env-var slice across a test body so we never leak
    /// state into other tests (or other test runs).
    struct EnvSnapshot {
        vars: Vec<(String, Option<String>)>,
    }
    impl EnvSnapshot {
        fn capture(names: &[&str]) -> Self {
            Self {
                vars: names
                    .iter()
                    .map(|n| (n.to_string(), std::env::var(n).ok()))
                    .collect(),
            }
        }
    }
    impl Drop for EnvSnapshot {
        fn drop(&mut self) {
            for (n, v) in &self.vars {
                match v {
                    Some(s) => std::env::set_var(n, s),
                    None => std::env::remove_var(n),
                }
            }
        }
    }

    const ENV_NAMES: &[&str] = &[
        "KITTY_WINDOW_ID",
        "GHOSTTY_RESOURCES_DIR",
        "WEZTERM_PANE",
        "TERM",
        "COLORTERM",
        "SSH_CONNECTION",
        "SSH_CLIENT",
    ];

    fn clear_env() {
        for n in ENV_NAMES {
            std::env::remove_var(n);
        }
    }

    #[test]
    fn kitty_window_id_picks_kitty() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvSnapshot::capture(ENV_NAMES);
        clear_env();
        std::env::set_var("KITTY_WINDOW_ID", "1");
        assert_eq!(detect(), TerminalCapability::Kitty);
    }

    #[test]
    fn ghostty_resources_dir_picks_kitty() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvSnapshot::capture(ENV_NAMES);
        clear_env();
        std::env::set_var("GHOSTTY_RESOURCES_DIR", "/usr/share/ghostty");
        assert_eq!(detect(), TerminalCapability::Kitty);
    }

    #[test]
    fn wezterm_pane_picks_kitty() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvSnapshot::capture(ENV_NAMES);
        clear_env();
        std::env::set_var("WEZTERM_PANE", "5");
        assert_eq!(detect(), TerminalCapability::Kitty);
    }

    #[test]
    fn term_kitty_substring_picks_kitty() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvSnapshot::capture(ENV_NAMES);
        clear_env();
        std::env::set_var("TERM", "xterm-kitty");
        assert_eq!(detect(), TerminalCapability::Kitty);
    }

    #[test]
    fn ssh_without_colorterm_picks_ascii() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvSnapshot::capture(ENV_NAMES);
        clear_env();
        std::env::set_var("SSH_CONNECTION", "1.2.3.4 22 5.6.7.8 22");
        std::env::set_var("TERM", "xterm");
        assert_eq!(detect(), TerminalCapability::Ascii);
    }

    #[test]
    fn ssh_with_truecolor_stays_truecolor() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvSnapshot::capture(ENV_NAMES);
        clear_env();
        std::env::set_var("SSH_CONNECTION", "1.2.3.4 22 5.6.7.8 22");
        std::env::set_var("COLORTERM", "truecolor");
        std::env::set_var("TERM", "xterm-256color");
        assert_eq!(detect(), TerminalCapability::Truecolor);
    }

    #[test]
    fn dumb_term_picks_ascii() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvSnapshot::capture(ENV_NAMES);
        clear_env();
        std::env::set_var("TERM", "dumb");
        assert_eq!(detect(), TerminalCapability::Ascii);
    }

    #[test]
    fn vt100_term_picks_ascii() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvSnapshot::capture(ENV_NAMES);
        clear_env();
        std::env::set_var("TERM", "vt100");
        assert_eq!(detect(), TerminalCapability::Ascii);
    }

    #[test]
    fn alacritty_term_picks_truecolor() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvSnapshot::capture(ENV_NAMES);
        clear_env();
        std::env::set_var("TERM", "alacritty");
        assert_eq!(detect(), TerminalCapability::Truecolor);
    }

    #[test]
    fn xterm_256color_picks_truecolor() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvSnapshot::capture(ENV_NAMES);
        clear_env();
        std::env::set_var("TERM", "xterm-256color");
        assert_eq!(detect(), TerminalCapability::Truecolor);
    }

    #[test]
    fn colorterm_24bit_picks_truecolor() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvSnapshot::capture(ENV_NAMES);
        clear_env();
        std::env::set_var("COLORTERM", "24bit");
        std::env::set_var("TERM", "screen");
        assert_eq!(detect(), TerminalCapability::Truecolor);
    }

    #[test]
    fn empty_env_defaults_to_truecolor() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvSnapshot::capture(ENV_NAMES);
        clear_env();
        assert_eq!(detect(), TerminalCapability::Truecolor);
    }

    #[test]
    fn force_mode_kitty_wins_over_detection() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvSnapshot::capture(ENV_NAMES);
        clear_env();
        std::env::set_var("TERM", "dumb");
        assert_eq!(resolve(Some("kitty"), true), TerminalCapability::Kitty);
    }

    #[test]
    fn force_mode_braille_maps_to_truecolor() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvSnapshot::capture(ENV_NAMES);
        clear_env();
        std::env::set_var("KITTY_WINDOW_ID", "1");
        assert_eq!(resolve(Some("braille"), true), TerminalCapability::Truecolor);
    }

    #[test]
    fn force_mode_ascii_wins_over_truecolor_detection() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvSnapshot::capture(ENV_NAMES);
        clear_env();
        std::env::set_var("TERM", "xterm-256color");
        std::env::set_var("COLORTERM", "truecolor");
        assert_eq!(resolve(Some("ascii"), true), TerminalCapability::Ascii);
    }

    #[test]
    fn auto_degrade_false_blocks_ascii_downshift() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvSnapshot::capture(ENV_NAMES);
        clear_env();
        std::env::set_var("TERM", "dumb"); // would normally give Ascii
        assert_eq!(resolve(None, false), TerminalCapability::Truecolor);
    }

    #[test]
    fn auto_degrade_false_preserves_kitty() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvSnapshot::capture(ENV_NAMES);
        clear_env();
        std::env::set_var("KITTY_WINDOW_ID", "1");
        // auto_degrade=false only overrides the Ascii downshift; Kitty
        // detection still wins.
        assert_eq!(resolve(None, false), TerminalCapability::Kitty);
    }

    #[test]
    fn unknown_force_mode_falls_through_to_detection() {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvSnapshot::capture(ENV_NAMES);
        clear_env();
        std::env::set_var("KITTY_WINDOW_ID", "1");
        assert_eq!(
            resolve(Some("no-such-mode"), true),
            TerminalCapability::Kitty,
        );
    }

    #[test]
    fn as_str_round_trip() {
        assert_eq!(TerminalCapability::Kitty.as_str(), "kitty");
        assert_eq!(TerminalCapability::Truecolor.as_str(), "truecolor");
        assert_eq!(TerminalCapability::Ascii.as_str(), "ascii");
    }

    #[test]
    fn from_force_mode_unknown_returns_none() {
        assert_eq!(TerminalCapability::from_force_mode("auto"), None);
        assert_eq!(TerminalCapability::from_force_mode(""), None);
        assert_eq!(TerminalCapability::from_force_mode("BRAILLE"), None);
    }
}
