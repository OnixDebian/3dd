---
phase: 01-render-core
verified: 2026-05-27T00:00:00Z
status: passed
score: 5/5 must-haves verified
re_verification:
  previous_status: none
  note: initial verification
gaps: []
human_verification:
  - test: "Cube reads as a solid 3D form across the orbit"
    expected: "Faces occlude, depth-shaded, no upside-down/squashed frames"
    why_human: "Visual legibility cannot be confirmed in a non-TTY sandbox"
    status: human-confirmed (per phase note: 3/4 poses clear, brightness tuned, no frustum clipping)
  - test: "q/Esc and panic restore the terminal; resize re-layouts"
    expected: "Clean terminal on every exit path; scene re-centers on resize"
    why_human: "Requires a live TTY; human + tmux frame capture confirmed"
    status: human-confirmed
---

# Phase 1: Render Core & Legibility Spike — Verification Report

**Phase Goal:** Prove terminal-3D reads as 3D — a single aspect-correct, depth-shaded cube orbits smoothly without pegging a CPU core, on a panic-safe app skeleton.
**Verified:** 2026-05-27
**Status:** passed
**Re-verification:** No — initial verification

## Build / Test Gate

| Check | Result | Evidence |
|-------|--------|----------|
| `cargo build` | PASS | Finished, no errors |
| `cargo clippy --all-targets -- -D warnings` | PASS (zero warnings) | Forced recompile (touch src/main.rs) then Finished clean |
| `cargo test` | PASS | 34 passed; 0 failed; 0 ignored |

## Goal Achievement — Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Cube reads as solid 3D (occlude, depth-shade, fog) | VERIFIED | src/render3d/raster.rs back-face cull + painter's sort + Lambert + fog |
| 2 | Cube is cubic, single config-exposed aspect factor | VERIFIED | src/config/mod.rs:23 + applied once at src/render3d/project.rs:53 |
| 3 | Orbits smoothly, no CPU peg (FPS cap) | VERIFIED | src/tui.rs tokio::select! at 30 FPS / 60 Hz tick |
| 4 | Quit/panic restores terminal; resize re-layouts | VERIFIED | src/main.rs panic hook before raw mode; src/ui live-area layout |
| 5 | All color via palette; no hardcoded color | VERIFIED | grep "Color::Rgb(" → only src/theme/mod.rs |

**Score: 5/5 truths verified**

## Per-Criterion Detail

### Criterion 1 — Solid 3D form (REND-01/02/03) — PASS
- Back-face cull: `raster.rs:76` drops faces whose normal points away from the eye (`face.normal.dot(to_eye) <= 0.0`).
- Painter's sort (farthest-first): `raster.rs:93-97` `sort_unstable_by` on `b.distance.partial_cmp(&a.distance)`; nearer faces drawn last and overwrite. Pinned by test `painters_sort_orders_farthest_first` and `occlusion_center_pixel_is_near_face`.
- Orientation (Lambert-ish) shading: `raster.rs:113-114` `lambert = normal.dot(to_eye_dir)`, floored by `MIN_LAMBERT = 0.62` (raster.rs:35). Test `shading_varies_color_across_faces`.
- Distance fog toward background: `raster.rs:115-118` + `fog_factor` (raster.rs:152-163) maps near→1.0, far→FOG_MIN 0.7; blended via `palette.fog` (theme/mod.rs:96-98 → `dim_toward` background). Test `fog_blends_toward_background`.

### Criterion 2 — Cubic, single aspect knob (REND-04) — PASS
- Single named, config-exposed value: `RenderConfig::cell_aspect` (config/mod.rs:23, default 2.0 at :35).
- Applied in exactly one place: `project.rs:53` `let aspect = (px_w / px_h) / config.cell_aspect;`. No other application site (grep confirms all other hits are tests/docs).
- Pinned by tests `unit_cube_footprint_is_cubic`, `cell_aspect_is_load_bearing`, and rasterizer `footprint_reads_cubic_at_subpixel_level`.

### Criterion 3 — Smooth orbit, no CPU peg (REND-05) — PASS
- FPS-capped async loop, not a spin loop: `tui.rs:96-127` `tokio::select!` over `render_interval` (30 FPS, :31/:85), `tick_interval` (60 Hz, :34/:86), and crossterm `EventStream`. select sleeps between ticks → low idle CPU (documented Pitfall #3).
- Render only on `Event::Render` (app.rs:91-105); logic advances by real `dt` (app.rs:85-90, camera.step). Framerate-independent orbit confirmed by `yaw_advances_with_dt`, `eye_stays_on_orbit_sphere`.

### Criterion 4 — Clean restore + resize (REND-06) — PASS
- Panic hook installed BEFORE raw mode: `main.rs:38` `install_hooks()` runs before `tui.enter()` at `main.rs:41`. Hook calls `tui::restore()` first then color-eyre report (main.rs:27-31).
- Idempotent restore on every exit path: `tui.rs:169-178 restore()` guarded by `is_raw_mode_enabled`; called from `Tui::exit` (:151-157), `Drop` backstop (:160-165), and the panic hook. Clean-exit path always calls `tui.exit()` (main.rs:47 regardless of run result).
- Resize-safe layout from live frame: `ui/mod.rs:17` `frame.area()` each frame; `ui/scene.rs:85-87` derives viewport from live `block.inner(area)` every frame (no cached size). Documented PITFALLS #14.

### Criterion 5 — Palette-only color (THEME-01) — PASS
- `grep -rln "Color::Rgb(" src/` returns ONLY `src/theme/mod.rs`. The default palette literals (theme/mod.rs:67-74) are the sole RGB site.
- Draw sites resolve color via palette: raster.rs uses `palette.status_color(Status::Running)` (:105), `theme::dim` (:117), `palette.fog` (:118); scene.rs blits framebuffer color (already palette-derived) and names no RGB.

## Architecture / Decoupling Check
- render3d does NOT depend on a `Camera` type: grep of `src/render3d/` shows only doc-comment mentions; `render()` consumes `ViewParams` (raster.rs:46-51). Camera (camera/mod.rs:30) imports `ViewParams`, so dependency points 05→04, no cycle.
- Single Y-flip lives in `project.rs:97+` (`ndc_to_screen`), pinned by `up_world_point_maps_to_upper_half`; scene blit is identity (scene.rs:61-65), no double flip.

## Anti-Patterns Found
None blocking. `#![allow(dead_code)]` appears in forward-facing modules (theme, project, raster, config) where APIs are consumed by later plans — documented and intentional, not stubs. No TODO/FIXME, no placeholder renders, no empty handlers, no console-log-only logic.

## Requirements Coverage
| Requirement | Status | Backed by |
|-------------|--------|-----------|
| REND-01 (occlusion) | SATISFIED | painter's sort, raster.rs |
| REND-02 (painter sort) | SATISFIED | raster.rs:93-97 |
| REND-03 (depth shade + fog) | SATISFIED | raster.rs:113-118, theme dim/fog |
| REND-04 (aspect correct) | SATISFIED | config cell_aspect, project.rs:53 |
| REND-05 (FPS cap / smooth) | SATISFIED | tui.rs select! loop |
| REND-06 (clean restore + resize) | SATISFIED | main.rs hook, tui.rs restore, ui live area |
| REND-07 (braille blit) | SATISFIED | ui/scene.rs SceneShape + Painter::paint |
| THEME-01 (palette abstraction) | SATISFIED | theme/mod.rs, grep gate clean |

## Human Verification (already confirmed per phase note)
The "reads as 3D / orbits smoothly / clean restore / resize" outcomes require a live TTY and were confirmed by the human + tmux frame capture (3/4 poses clear face contrast, brightness tuned brighter, no frustum clipping, q/Esc restore confirmed, ~30 fps capped, low CPU). The CODE backing every claim exists and is wired, and the build/clippy/test gate is green.

## Gaps Summary
No gaps. All 5 success criteria are satisfied by substantive, wired code and a green build/clippy/test gate. Plan 01-05 is the last checkbox in the roadmap; its deliverables (orbit camera, braille blit, legibility) are present in camera/mod.rs and ui/scene.rs and verified above.

---
_Verified: 2026-05-27_
_Verifier: Claude (gsd-verifier)_
