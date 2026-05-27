---
phase: 02-scene-pipeline
plan: 03
subsystem: render-kitty
tags: [kitty, rgba, z-buffer, fog, scene-bounds, world, supersample, glam]

# Dependency graph
requires:
  - phase: 02-scene-pipeline
    provides: "world::synthetic_scene() -> World{entities,bounds}, Entity{position,half_extents,status}, SceneBounds{center,radius}, Camera::frame_scene(&SceneBounds)"
  - phase: 01-render-core
    provides: "kitty real-pixel path (render_rgba/fill_tri z-buffer/2x SS AA/emit_kitty), theme::Palette::status_color, render3d::Projector/ViewParams, Camera orbit"
provides:
  - "render_rgba(entities: &[Entity], view: ViewParams, bounds: &SceneBounds, palette, w, h) -> Vec<u8> — rasterizes a whole World into one shared z-buffer with per-box status color"
  - "Scene-wide ABSOLUTE distance fog derived from SceneBounds (flicker-safe), replacing the single-cube CUBE_BOUND range"
  - "run_kitty + dump_rgba framing the whole rack via Camera::frame_scene(&world.bounds)"
affects: [02-04-human-verify, 05-theming (kitty path reconciliation / ROB-02)]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Per-pixel NDC-z depth test shared across ALL boxes => inter-box occlusion is free on the kitty path (no painter's sort, unlike braille)"
    - "Absolute scene-wide fog: near/far = |eye - bounds.center| ± bounds.radius — a function of static scene + camera only, never per-frame visible faces (flicker-safe property preserved from Phase 1)"
    - "Unit cube reused as a per-box geometry template, transformed per entity (translate position, scale half_extents*2) in world space"

key-files:
  created: []
  modified:
    - src/kitty.rs

key-decisions:
  - "render_rgba takes &SceneBounds (not loose (near,far)): one source of truth for where the scene is, and makes the flicker-safe absolute-fog property structurally obvious"
  - "Fog range generalized cube->scene: near = |eye-bounds.center|-bounds.radius, far = |eye-bounds.center|+bounds.radius; CUBE_BOUND constant removed, face_shade now takes near/far"
  - "Back-face cull is done in WORLD space against the eye using each entity's transformed face centroid (axis-aligned normals stay valid under the per-axis-positive scale)"
  - "Magic radius 3.2 removed from run_kitty/dump_rgba; both now call camera.frame_scene(&world.bounds); dump still does step(2.0) AFTER frame_scene for an informative 3/4 pose"
  - "yaw-invariance flicker test re-pinned under scene-wide fog by placing the probe face AT bounds.center (orbit-invariant distance), so only a true flicker bug could vary its shade"

metrics:
  duration: ~3 min
  completed: 2026-05-27
---

# Phase 2 Plan 03: Kitty Multi-Box Renderer Summary

Generalized the kitty real-pixel renderer (`src/kitty.rs`) from one hardcoded centered unit cube to a whole `World` of boxes, with scene-wide absolute fog and scene-framed orbit. Touched ONLY `src/kitty.rs` (wave-2 disjoint-files contract with 02-02 honored).

## What was built

- **`render_rgba` over a whole World.** New signature:
  `pub fn render_rgba(entities: &[Entity], view: ViewParams, bounds: &SceneBounds, palette: &Palette, w: usize, h: usize) -> Vec<u8>`.
  The unit cube is built once as a per-box template; each entity's faces are transformed to world space (translate by `position`, scale by `half_extents * 2`), back-face culled against the eye in world space, projected (square-pixel `cell_aspect` 1.0), and filled into the SHARED `color`/`depth` buffers. Because depth is shared across all boxes, the existing `fill_tri` NDC-z test resolves inter-box occlusion automatically (near hides far). Per-box base color via `palette.status_color(entity.status)` (CONT-01).
- **Scene-wide ABSOLUTE fog.** Chosen mechanism: the passed `&SceneBounds`.
  `near = |view.eye - bounds.center| - bounds.radius`, `far = |view.eye - bounds.center| + bounds.radius`, computed once per frame and passed into `face_shade`. The single-cube `CUBE_BOUND` constant is removed. The range is a function of the static scene + camera only — NOT a per-frame min/max of visible faces — so the flicker-safe property from Phase 1 is preserved (verified by the re-pinned yaw-invariance test).
- **Scene-framed camera.** `run_kitty` and `dump_rgba` now build `synthetic_scene()` and call `camera.frame_scene(&world.bounds)`; the magic `radius = 3.2` is gone. `dump_rgba` advances `step(2.0)` after framing for an informative 3/4 pose.
- **All prior machinery preserved:** 2× supersample buffers + box-downsample, NDC-z `fill_tri`, `emit_kitty`/`delete_all`/`base64`, zlib `o=z` payloads, status bar, raw-mode restore.

## Tests (all green; suite 56 -> 60)

- `z_buffer_occludes_far_box_behind_near_box` — two same-status boxes stacked on the view axis; the overlap pixel equals the A-only render exactly (near fully hides far, no blend).
- `distinct_status_boxes_yield_distinct_colors` — Running + Crashed side by side => ≥2 distinct non-background colors.
- `empty_scene_is_all_background` — empty entities + degenerate bounds => all-background, no panic.
- `top_face_shade_is_yaw_invariant` — re-pinned under scene-wide fog (probe face at `bounds.center`); shade is identical across a full yaw sweep.

## Verification

- `cargo build` clean, `cargo clippy --all-targets` clean, `cargo test` green (60 passed).
- Greps: `fn render_rgba` shows `&[Entity]` + `bounds: &SceneBounds`; `CUBE_BOUND` and `3.2` return nothing; `frame_scene`/`synthetic_scene` matched; `unit_cube` is only the per-box template.

### Kitty visual capture (the 02-04 verification artifact)

Command (MUST be `--release`):

```
cargo run --release -- --dump-rgba /tmp/dd3_scene.rgba
```

Output: `720 560 1612800` (720×560 RGBA = 720·560·4 bytes). Converted to `/tmp/dd3_scene.png` for inspection.

Inspection result: all functional criteria confirmed — five group clusters lay out in a diagonal rack, boxes carry distinct status colors (indigo running, red crashed, cyan restarting, amber paused), and depth fog + perspective shrink the far boxes, so per-box color, z-buffer occlusion, and scene framing all WORK.

## Deviations from Plan

None to the code — the plan was executed as written within `src/kitty.rs`. (Tasks 1 and 2 were committed together in one atomic commit because the Task 1 signature change makes the Task 2 call sites a hard compile dependency — the file does not build with only one half applied.)

## Finding for 02-04 (framing too small — NOT a kitty-path bug)

In the dumped frame the rack occupies only a small central region (non-background bbox ≈ 163×66 px within 720×560, well under 5% of the frame). The kitty rasterization is correct; the smallness comes from `Camera::frame_scene` (02-01, `src/camera/mod.rs`) being conservative — the near-corner tangent bound, the `FRAME_SAFETY_MARGIN` headroom, and the deep `GROUP_DEPTH 18.0` pull the camera far back. This was deliberately left untouched: it lives in 02-01's file and changing it risks the wave-2 disjoint-files contract, and STATE already flags "tighten framing if too sparse/deep" as a human-verify decision. Recommend 02-04 evaluate tightening `frame_scene` framing (smaller margin or scaling the framed radius down) so the rack reads larger.

## Notes / open items

- The kitty backend continues to front-run Phase 5 ROB-02; reconcile in Phase 5 planning (carried from 01-05).
- STATE.md position intentionally NOT updated here — orchestrator owns phase-level state.
