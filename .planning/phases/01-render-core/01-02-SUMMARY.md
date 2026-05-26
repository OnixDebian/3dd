---
phase: 01-render-core
plan: 02
subsystem: theming
tags: [rust, ratatui, color, palette, theme, fog, depth-cue, THEME-01, REND-03, CONT-01]

# Dependency graph
requires:
  - "01-01 (app-skeleton): ratatui 0.30, binary-only mod layout, ratatui::style::Color"
provides:
  - "theme module: the single home for renderer colors (no inline RGB at draw sites)"
  - "Status enum { Running, Paused, Stopped, Restarting, Crashed } (mirrors CONT-01)"
  - "Palette { background, edge, glow, running, paused, stopped, restarting, crashed }"
  - "Palette::default() — Notion-soft indigo #5B5BD6 default (only RGB literals in the crate)"
  - "Palette::status_color(Status) -> Color"
  - "dim(color, factor) depth/fog primitive + dim_toward + Palette::fog + lerp_color"
affects: [04-rasterizer, 05-scene, "Phase 5 theming (presets/TOML/quantization)"]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Palette abstraction: all draw colors resolved via Palette; RGB literals confined to theme/mod.rs (THEME-01)"
    - "dim()/lerp_color() RGB lerp depth-cue primitive; non-Rgb passthrough deferred to Phase 5 quantization"
    - "Module-level #![allow(dead_code)] on a forward-facing API consumed by later plans"

key-files:
  created:
    - src/theme/mod.rs
  modified:
    - src/main.rs

key-decisions:
  - "No src/lib.rs: kept 01-01's binary-only mod layout; added `mod theme;` to main.rs (avoids conflict with parallel 01-03)"
  - "dim() blends toward black; Palette::fog()/dim_toward() blend toward the palette background"
  - "Non-Rgb colors pass through dim/lerp unchanged — 256/16-color quantization is Phase 5"
  - "Default palette: indigo #5B5BD6 running+glow, amber paused, gray stopped, cyan-blue restarting, red crashed"
  - "Module-scoped #![allow(dead_code)] so the not-yet-consumed API stays clippy-clean without masking dead code elsewhere"

patterns-established:
  - "THEME-01: every renderer color goes through Palette; grep `Color::Rgb(` matches only src/theme/mod.rs"
  - "REND-03 depth cue: dim(color, factor) is the per-face brightness + distance-fog primitive for plan 04"

# Metrics
duration: 2min
completed: 2026-05-26
---

# Phase 1 Plan 02: Palette Abstraction Summary

**A `theme` module that confines every renderer color to one place: a `Palette` (background/edge/glow + a color per container `Status`), `status_color(Status) -> Color`, and a `dim(color, factor)`/`lerp_color` depth-fog primitive — so plans 04/05 never inline an RGB value (THEME-01) and have the distance-shading helper REND-03 needs.**

## Performance

- **Duration:** ~2 min
- **Started:** 2026-05-26T21:40:41Z
- **Completed:** 2026-05-26T21:42:11Z
- **Tasks:** 2
- **Files created:** 1 (`src/theme/mod.rs`); **modified:** 1 (`src/main.rs`, one line)

## Accomplishments
- `Status` enum (`Running/Paused/Stopped/Restarting/Crashed`) mirroring CONT-01, defined now so Phase 2/3 reuse it when real Docker state arrives.
- `Palette` struct with the exact field set plans 04/05 need: `background`, `edge`, `glow`, and one color per status.
- `Palette::default()` — a Notion-soft indigo (`#5B5BD6`) accent with a distinct, legible hue per status. **These are the only RGB literals in the whole renderer** — verified by `grep -rln "Color::Rgb(" src/` matching only `src/theme/mod.rs` (THEME-01 invariant holds).
- `Palette::status_color(Status) -> Color` — the one place a status becomes a color.
- `dim(color, factor)` depth/fog primitive plus `dim_toward(color, target, factor)`, `Palette::fog(color, factor)`, and `lerp_color(a, b, t)` — the per-face brightness + distance-fog primitive plan 04 (REND-03) consumes.
- 7 unit tests covering dim endpoints (1.0 keeps, 0.0 -> black), midpoint, factor clamping, non-Rgb passthrough, fog-to-background, and lerp endpoints. All pass.

## Task Commits

1. **Task 1: Palette, Status, and color resolution** — `4f1ca6a` (feat)
2. **Task 2: Depth/fog dimming helper** — `7291aea` (feat)

**Plan metadata:** `docs(01-02): complete palette-abstraction plan` (final commit)

## API surface (for plans 04 / 05)

```rust
// src/theme/mod.rs
pub enum Status { Running, Paused, Stopped, Restarting, Crashed }

pub struct Palette {
    pub background: Color,
    pub edge: Color,
    pub glow: Color,
    pub running: Color,
    pub paused: Color,
    pub stopped: Color,
    pub restarting: Color,
    pub crashed: Color,
}

impl Default for Palette { /* Notion-soft indigo default */ }
impl Palette {
    pub fn status_color(&self, status: Status) -> Color;
    pub fn fog(&self, color: Color, factor: f32) -> Color; // blend toward background
}

pub fn dim(color: Color, factor: f32) -> Color;            // blend toward black, factor 1.0=keep, 0.0=black
pub fn dim_toward(color: Color, target: Color, factor: f32) -> Color;
pub fn lerp_color(a: Color, b: Color, t: f32) -> Color;    // t clamped [0,1]
```

**Default palette values:**

| Field | RGB | Hex | Intent |
|-------|-----|-----|--------|
| background | (0x1A,0x1A,0x22) | #1A1A22 | muted near-black, faint indigo tint / fog target |
| edge | (0x6B,0x6B,0x78) | #6B6B78 | soft gray wireframe |
| glow | (0x5B,0x5B,0xD6) | #5B5BD6 | indigo accent |
| running | (0x5B,0x5B,0xD6) | #5B5BD6 | indigo — alive |
| paused | (0xE2,0xB1,0x4F) | #E2B14F | amber — held |
| stopped | (0x6B,0x6B,0x78) | #6B6B78 | gray — dormant |
| restarting | (0x4F,0xA6,0xE2) | #4FA6E2 | cyan-blue — in flux |
| crashed | (0xE2,0x5B,0x5B) | #E25B5B | red — failed |

**How plans 04/05 consume it:**
- Rasterizer (04): for each face, `palette.status_color(status)` then `dim(color, brightness)` for shading and `palette.fog(color, depth_factor)` (or `dim`) for distance fog. Edge/wireframe uses `palette.edge`; glow uses `palette.glow`; clear the canvas to `palette.background`.
- Scene (05): hold a `Palette` (default for now) and pass it down; never name a `Color::Rgb` at any draw site.
- Phase 5: swap the active `Palette` (presets/TOML) and add 256/16-color quantization in `dim`/`lerp_color`'s currently-passthrough non-Rgb branch — no draw-site changes required.

## Decisions Made
- **No `src/lib.rs`.** The plan frontmatter listed `src/lib.rs`, but 01-01 established a binary-only layout with `mod` declarations in `main.rs`; the plan's Task 1 says to "match whatever layout plan 01-01 established." Added `mod theme;` to `main.rs` instead. This also avoids a write conflict with the parallel plan 01-03 (which independently added `mod config;` / `mod render3d;` to the same file — both coexisted via minimal additive edits).
- **`dim()` blends toward black; `fog()`/`dim_toward()` blend toward background.** Gives plan 04 both options: simple multiply-to-black brightness and palette-aware fog without a Palette reference at every call site.
- **Non-Rgb passthrough.** `dim`/`lerp_color` only interpolate `Color::Rgb`; named/indexed colors pass through unchanged, deferring quantization to Phase 5 as the plan specifies.
- **Module-scoped `#![allow(dead_code)]`.** The whole module is a forward-facing API not consumed until plans 04/05; a module-level allow keeps the build/clippy clean (per 01-01's clippy-clean bar) without globally masking dead code.

## Deviations from Plan

### Auto-fixed / layout adaptations

**1. [Rule 3 - Blocking / layout] Used `mod theme;` in main.rs instead of creating `src/lib.rs`**
- **Found during:** Task 1.
- **Issue:** Plan frontmatter named `src/lib.rs`, but the crate is binary-only (01-01 used `mod` decls in `main.rs`). Creating a lib.rs would diverge from the established layout and risk clobbering the parallel 01-03 plan editing the same shared files.
- **Fix:** Added a single `mod theme;` line to `main.rs`; re-read the file immediately before editing. The sibling plan's `mod config;`/`mod render3d;` additions coexist cleanly.
- **Files modified:** `src/main.rs`.
- **Verification:** `cargo build` clean; `theme::` tests run.
- **Committed in:** `4f1ca6a`.

**2. [Rule 2 - Missing critical] Module-level `#![allow(dead_code)]` on the theme API**
- **Found during:** Task 1 verification (build emitted dead-code warnings for the not-yet-consumed `Status`/`Palette`/`status_color`).
- **Issue:** The abstraction exists ahead of its plan-04/05 consumers (the whole point of THEME-01), so it is legitimately unused now; 01-01 set a clippy-clean bar.
- **Fix:** Added a scoped `#![allow(dead_code)]` with a comment, so the forward-facing API stays warning-free without hiding dead code elsewhere.
- **Files modified:** `src/theme/mod.rs`.
- **Verification:** `cargo build`/`cargo clippy` produce zero warnings from `theme` (remaining 4 build warnings all originate in the parallel 01-03 files `render3d/`, `config/`, not touched here).
- **Committed in:** `4f1ca6a`.

**Total deviations:** 2 (1 layout adaptation, 1 missing-critical lint suppression). No architectural changes; no scope creep. The Palette/Status/dim API matches the plan exactly.

## Issues Encountered
- **None blocking.** Note for the orchestrator: the parallel plan 01-03 also edits `src/main.rs` and adds `render3d/`+`config/` modules; those produce 4 pre-existing dead-code/unused-import warnings unrelated to this plan. The `mod theme;` line and 01-03's module lines merged without conflict. If a later full-tree `cargo clippy -D warnings` gate is added, 01-03's warnings (not theme's) will need addressing.

## State-worthy notes (orchestrator: fold into STATE.md after the wave)
- **Decision (01-02):** No `src/lib.rs` — `mod theme;` added to `main.rs`, matching 01-01's binary-only layout; coexists with 01-03's additive module decls.
- **Decision (01-02):** RGB literals confined to `src/theme/mod.rs` (THEME-01 invariant; verifiable via `grep -rln "Color::Rgb(" src/`).
- **Decision (01-02):** Default palette = indigo `#5B5BD6` accent (running/glow), amber paused, gray stopped, cyan-blue restarting, red crashed.
- **Decision (01-02):** `dim()` -> black, `fog()`/`dim_toward()` -> background; non-Rgb passthrough until Phase 5 quantization.
- **Concern (carry forward):** parallel 01-03 leaves 4 dead-code/unused warnings in `render3d/`+`config/`; revisit if a `-D warnings` gate lands.

## Next Phase Readiness
- THEME-01 is satisfied: a single Palette resolves every status/edge/glow/background color, and `grep` confirms no RGB literal escapes `theme/mod.rs`.
- REND-03's depth-cue primitive (`dim`/`fog`) is in place and tested for plan 04's per-face brightness + distance fog.
- **Next:** Plan 04 (rasterizer) and Plan 05 (scene) consume `Palette`/`Status`/`dim` instead of inlining colors. Plan 03 (parallel) continues independently.

---
*Phase: 01-render-core*
*Completed: 2026-05-26*
