---
phase: 02-scene-pipeline
plan: 04
subsystem: app-integration
tags: [app, world, ui-seam, status-bar, per-box-spin, static-camera, projected-aabb, frame_scene, rotate_y_about, kitty, braille]

# Dependency graph
requires:
  - phase: 02-scene-pipeline
    provides: "world::synthetic_scene() -> World{entities,bounds}; render_scene (braille) over a World; render_rgba (kitty) over a World; Camera::frame_scene"
  - phase: 01-render-core
    provides: "App/UI braille path (ratatui Canvas SceneShape), Camera, render3d Projector/ViewParams, theme::Palette::status_color, kitty real-pixel path"
provides:
  - "App owns a single World (pub world: World), built once in App::new and framed by Camera::frame_scene at construction — single source of truth for the scene"
  - "Braille UI seam from 02-02 closed: ui/scene.rs render_scene takes &World; the temporary synthetic_scene() in the UI layer is gone"
  - "render3d::rotate_y_about(point, center, angle) — per-box self-rotation primitive (vertices AND normals) used by both renderers"
  - "Per-box self-spin motion model in BOTH backends (raster.rs + kitty.rs) replacing whole-scene camera orbit"
  - "Static 3/4 camera (YAW_RATE=0; FRAME_YAW + PITCH_BIAS) — orbit disabled"
  - "Projector::ndc (raw NDC) + projected-AABB Camera::frame_scene that binary-searches the framing distance to FRAME_TARGET_FILL=0.92 on the binding (horizontal) axis"
  - "status bar shows boxes: {n} (scene scale visible at a glance)"
affects: [03-interaction (motion model is now per-box-spin, not camera-orbit), 05-theming (kitty path ROB-02 reconciliation), ROADMAP/REQUIREMENTS reconciliation (CAM-01 motion model changed)]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Single source of truth for the scene: the App owns one World; both the UI view layer and the camera framing read it (no UI-layer temporary scene)"
    - "Per-box self-spin: each box rotates about its OWN +Y axis (rotate_y_about with center = entity.position); normals are rotated too (about origin) so shading stays correct under spin"
    - "Static camera holding a fixed 3/4 framing pose (orbit disabled) — motion comes from the boxes, not the camera"
    - "Projected-AABB framing: binary-search the eye distance so the rack's actual screen-space projected AABB (swept across a full turn of per-box spin) reaches FRAME_TARGET_FILL of the binding NDC half-axis — replaces the under-filling circumscribing-sphere fit"

key-files:
  created: []
  modified:
    - src/app.rs
    - src/ui/mod.rs
    - src/ui/scene.rs
    - src/ui/status_bar.rs
    - src/render3d/mod.rs
    - src/render3d/raster.rs
    - src/render3d/project.rs
    - src/kitty.rs
    - src/camera/mod.rs

key-decisions:
  - "App owns the World (pub world: World), built once in App::new and framed via Camera::frame_scene at construction — closes the 02-02 braille UI seam (synthetic_scene() removed from the UI layer)"
  - "MOTION MODEL OVERRIDE (human, during verify): replaced whole-scene camera orbit with PER-BOX self-rotation about each box's own +Y axis + a STATIC 3/4 camera. Deviates from roadmap success criterion #4 / CAM-01 (autopilot orbit). Must be reconciled in Phase planning."
  - "rotate_y_about lives in render3d and is shared by both renderers; normals are rotated too (about origin) to keep face shading correct under spin"
  - "Camera orbit disabled (YAW_RATE=0); camera holds FRAME_YAW + PITCH_BIAS (fixed 3/4 pose)"
  - "Denser/shallower rack to fill more frame while staying frustum-safe: SLOT_SPACING 3.0->2.6, GROUP_DEPTH 18->10, GROUP_COUNT 5->3; smaller FRAME_SAFETY_MARGIN"
  - "frame_scene rewritten from a bounding-SPHERE fit to a PROJECTED-AABB fit: binary-search distance to FRAME_TARGET_FILL=0.92 on the binding (horizontal) axis, sweeping every entity AABB corner across a full turn of per-box spin against the canonical cell_aspect-2 braille frustum (keeps kitty/taller terminals safe). Added Projector::ndc; project() now layers on it."

# Metrics
duration: ~2h (across verify-tuning rounds)
completed: 2026-05-27
---

# Phase 2 Plan 04: App Integration + Scene Verify Summary

**App now owns a single World framed by a projected-AABB fit; the 02-02 braille UI seam is closed; and after human verify the motion model became per-box self-spin under a static 3/4 camera (orbit disabled) — human-approved on both backends, 64 tests green.**

## Performance

- **Duration:** ~2h (multiple verify-tuning rounds before human sign-off)
- **Completed:** 2026-05-27
- **Tasks:** 2 auto + 1 human-verify checkpoint (APPROVED)
- **Files modified:** 9

## Accomplishments

- **App owns the World (single source of truth).** `App` gains `pub world: World`, built once in `App::new` and framed via `Camera::frame_scene(&world)` at construction (CAM-01 default-on, static scene). `ui::view` forwards `app.world` to `scene::render_scene`.
- **02-02 braille UI seam closed.** `ui/scene.rs::render_scene` now takes `&World`; the temporary `synthetic_scene()` the UI layer built as a compile-seam in 02-02 is gone, and the per-frame `SceneBounds` re-derive + `frame_scene` churn is dropped. `grep "synthetic_scene" src/ui/` is empty.
- **Status bar shows `boxes: {n}`** so the scene scale is visible at a glance during verify (fps/size/mode preserved).
- **Per-box self-spin in BOTH renderers.** New `render3d::rotate_y_about(point, center, angle)`. Each box rotates about its OWN `+Y` axis (`center = entity.position`); in `render3d/raster.rs` (braille) and `kitty.rs` the per-box vertices AND normals are rotated (normals about origin) so shading stays correct. The braille spin angle is advanced on the framerate-independent logic tick and threaded `app -> ui -> render_scene`; the kitty path advances its own spin by real `dt`.
- **Static 3/4 camera.** Orbit disabled (`YAW_RATE = 0`); the camera holds a fixed `FRAME_YAW + PITCH_BIAS` 3/4 framing. Motion comes from the boxes, not the camera.
- **Projected-AABB framing (~2x fuller frame).** `Camera::frame_scene` rewritten from a circumscribing-SPHERE fit (which under-filled the diagonal-ribbon rack to ~40-50%) to a PROJECTED-AABB fit: it binary-searches the eye distance so the rack's actual screen-space projected bounding box reaches `FRAME_TARGET_FILL = 0.92` of the binding (horizontal) NDC half-axis, sweeping every entity AABB corner across a full turn of per-box spin against the canonical `cell_aspect`-2 braille frustum (so kitty / taller terminals stay safe too). Added `Projector::ndc` (raw NDC); `project()` now layers on it.

## Task Commits

1. **Task 1: App owns the World; close braille UI seam; status bar box count** — `862788e` (feat)
2. **Verify-tuning: per-box self-spin + static framing + zoomed-in rack** — `2831e77` (feat)
3. **Verify-tuning: projected-AABB framing fills frame ~2x larger** — `c6810a4` (feat)

**Plan metadata:** this SUMMARY commit (docs: complete app integration + scene verify plan)

_Task 2 was a CAPTURE/no-op-code task (kitty `--dump-rgba` already existed from 02-03) — no functional source commit; its artifacts feed the verify checkpoint below._

## Files Created/Modified

- `src/app.rs` — App owns `pub world: World`; built + framed once in `App::new`; owns a spin angle advanced on the logic tick.
- `src/ui/mod.rs` — `view()` forwards `app.world` (+ spin) to `scene::render_scene`.
- `src/ui/scene.rs` — `render_scene` takes `&World`; 02-02 `synthetic_scene()` temporary removed; per-frame re-frame churn dropped.
- `src/ui/status_bar.rs` — HUD shows `boxes: {n}`.
- `src/render3d/mod.rs` — new `rotate_y_about(point, center, angle)` + tests.
- `src/render3d/raster.rs` — braille path applies per-box vertex + normal spin.
- `src/render3d/project.rs` — `Projector::ndc` (raw NDC); `project()` layers on it.
- `src/kitty.rs` — kitty path applies per-box vertex + normal spin; advances spin by real `dt`.
- `src/camera/mod.rs` — orbit disabled (`YAW_RATE=0`, fixed 3/4 pose); `frame_scene` rewritten to the projected-AABB fit (`FRAME_TARGET_FILL=0.92`); denser/shallower rack constants; frustum/fill tests updated.

## Decisions Made

See frontmatter `key-decisions`. Headline: the App is the single owner of the World, and the scene's MOTION MODEL was changed during human verify from camera-orbit to per-box self-spin under a static camera.

## Deviations from Plan

**1. [Human override during verify — NOT an auto-fix rule] Motion model changed: camera-orbit -> per-box self-spin + static camera**

- **Found during:** Task 3 (human-verify checkpoint)
- **Plan/roadmap intent:** Roadmap success criterion #4 / CAM-01 specified an "autopilot ORBIT camera with motion parallax" framing the whole scene — i.e. the CAMERA moves around a static rack.
- **What changed:** Per explicit human feedback during verify (the whole scene spinning as a group filled only ~10% of the frame and read poorly), the camera orbit was DISABLED (`YAW_RATE=0`, fixed 3/4 pose) and motion was moved INTO the scene: each box now self-rotates about its own `+Y` axis (`rotate_y_about`). Framing was then re-derived via the new projected-AABB fit to fill ~2x more of the frame.
- **Files modified:** `src/camera/mod.rs`, `src/render3d/mod.rs`, `src/render3d/raster.rs`, `src/render3d/project.rs`, `src/kitty.rs`, `src/app.rs`, `src/ui/*`
- **Verification:** human-approved on both backends in a real terminal ("да, двигаемся дальше").
- **Committed in:** `2831e77` (spin + static camera) and `c6810a4` (projected-AABB framing).

> **PHASE-RECONCILIATION FLAG (important):** CAM-01's intent has partially CHANGED. The roadmap/REQUIREMENTS describe an autopilot **camera orbit** with motion parallax; the implemented (and human-approved) model is **per-box self-spin + a static 3/4 camera**. This is a deliberate human override, not a bug. Phase planning MUST reconcile ROADMAP / REQUIREMENTS so CAM-01 reflects the per-box-spin motion model (and decide whether camera orbit is dropped, deferred, or layered on later).

---

**Total deviations:** 1 (human-directed motion-model override, applied across the verify-tuning rounds).
**Impact on plan:** Tasks 1 and 2 executed as written; the human-verify checkpoint redirected criterion #4's motion model. No unrelated scope creep — all changes serve legibility (frame fill + readable motion) and were human-approved.

## Issues Encountered

- **Whole-scene group spin filled ~10% of frame and read poorly.** Resolved by (a) per-box self-spin instead of group spin, (b) a denser/shallower rack (`SLOT_SPACING` 3.0->2.6, `GROUP_DEPTH` 18->10, `GROUP_COUNT` 5->3), and (c) the projected-AABB framing fit (`FRAME_TARGET_FILL=0.92`) replacing the under-filling sphere fit — net ~2x fuller frame. (Carries forward 02-03's "framing too small" finding, which pointed at the sphere fit in `frame_scene`.)

## Verification

- **Human verify: APPROVED on BOTH backends** in a real terminal (kitty preferred path + braille fallback). Human confirmed dozens of boxes read clearly at scale, status color + load size are visible, near occludes far, slots are stable (no jitter), and the per-box spin under the static 3/4 camera reads well. Sign-off: "да, двигаемся дальше."
- `cargo test` — **64 passed** (suite grew from 60; added `rotate_y_about` tests, projected-AABB framing/fill pins, and static-camera frustum sweep).
- `cargo build` clean, `cargo clippy --all-targets` clean.
- `grep "synthetic_scene" src/ui/` — empty (seam closed); `grep "world" src/app.rs` — matches (App owns the World).
- Kitty capture artifact (from 02-03's `--dump-rgba`, re-inspected): `cargo run --release -- --dump-rgba /tmp/dd3_scene.rgba` -> `/tmp/dd3_scene.png`. Braille captured live by the human via `cargo run -- --braille`.

## Contract status

- **CONT-01 (status color):** satisfied — per-box `palette.status_color` on both paths.
- **CONT-02 (load size):** satisfied — clamped load proxy sizing, preserved at scale.
- **CONT-05 (stable slots):** satisfied — boxes hold their layout slot frame-to-frame (per-box spin does not move slots); human confirmed no jitter/teleport.
- **CAM-01 (autopilot framing):** PARTIALLY CHANGED — camera frames the whole scene on the binding axis by default, but the **orbit** motion was replaced by **per-box self-spin** under a static camera (see deviation + reconciliation flag).

## Next Phase Readiness

- Phase 2 scene pipeline is app-integrated and human-verified on both backends; ready to close.
- **Blocker for phase reconciliation (not for code):** ROADMAP / REQUIREMENTS must be updated so CAM-01's motion model reflects per-box self-spin (not camera orbit). Flagged here for the orchestrator's phase-level handling.
- Kitty backend continues to front-run Phase 5 ROB-02 (carried from 01-05 / 02-03); reconcile in Phase 5 planning.

---
*Phase: 02-scene-pipeline*
*Completed: 2026-05-27*
