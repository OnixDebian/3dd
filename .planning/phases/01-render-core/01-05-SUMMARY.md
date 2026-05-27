---
phase: 01-render-core
plan: 05
subsystem: ui
tags: [ratatui, braille, canvas, orbit-camera, autopilot, glam, 3d, legibility-spike]

# Dependency graph
requires:
  - phase: 01-01
    provides: App/Tui async loop (render tick + logic tick), terminal restore on quit/panic
  - phase: 01-02
    provides: Palette/Theme abstraction (status_color, dim, fog) — single source of color
  - phase: 01-04
    provides: render3d::render snapshot renderer + ViewParams{eye,target,up,fov} as sole camera input; Framebuffer at braille sub-pixel resolution; lit_pixels() blit feed
provides:
  - Autopilot orbit Camera (yaw/pitch/radius/target) that builds a render3d::ViewParams each frame
  - SceneShape: framebuffer -> braille Painter blit, the single adapter owning the coordinate mapping
  - End-to-end render path wired into the app loop (cube orbits on the steady tick)
  - HUMAN-VERIFIED legibility: a terminal-3D cube reads as a solid 3D form (Phase 1 #1 risk de-risked)
affects: [phase-02-layout, scene-rendering, camera-controls, many-boxes]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Producer/consumer decoupling: Camera (05) -> ViewParams (04), render3d never imports Camera (no cycle)"
    - "Single coordinate-mapping adapter (SceneShape) — y-flip applied exactly once in projection, never re-applied in the blit"
    - "Framerate-independent autopilot: camera.step(dt) on the logic tick using real dt (Gaffer decoupling)"

key-files:
  created:
    - src/camera/mod.rs
    - src/ui/scene.rs
  modified:
    - src/ui/mod.rs
    - src/app.rs

key-decisions:
  - "Orbit defaults frustum-safe: DEFAULT_RADIUS=6.0, DEFAULT_FOV=60deg keep all 8 unit-cube vertices in-frustum across the entire yaw x pitch orbit (asin(0.866/6.0)~8.3deg vs 30deg half-FOV => ~21.7deg margin) so the skip-on-clip face-drop never fires mid-rotation"
  - "Autopilot yaw rate ~30deg/s (YAW_RATE=0.525 rad/s, full revolution ~12s) after human re-verify wanted slightly faster rotation; bumped 1.5x from the original ~20deg/s"
  - "Pitch bob: steady downward bias ~15deg (PITCH_BIAS=PI/8*1.2) + ~33deg amplitude eased sine (PITCH_AMPLITUDE=PI/8*1.5) to reveal both the top and bottom faces; hard-clamped clear of the gimbal poles (PITCH_LIMIT=PI/2-0.15)"
  - "Brightened palette after human-verify: running/glow indigo #5B5BD6 -> #8A8AF0, edge soft-gray brightened"
  - "Raised Lambert floor MIN_LAMBERT 0.35 -> 0.62 and fog floor FOG_MIN 0.45 -> 0.7 so far/oblique faces stay legible instead of going muddy"
  - "cell_aspect=2.0 (unchanged from 01-03 default) — cube reads cubic end-to-end; human confirmed not squashed"
  - "y-flip lives once in NDC->screen projection (01-03), so the blit maps framebuffer-top-left -> Painter with no second inversion; human confirmed right-side-up"

patterns-established:
  - "Camera->ViewParams producer side closes the 05->04 dependency with no cycle; render3d stays a pure snapshot renderer"
  - "SceneShape is the SINGLE place braille/canvas coordinate math lives"

# Metrics
duration: ~15min
completed: 2026-05-27
---

# Phase 1 Plan 05: Scene Orbit + Legibility Verify Summary

**An autopilot orbit camera drives an aspect-correct, painter's-sorted, depth-shaded cube into the terminal's braille Canvas — and a real-terminal human verify confirmed it reads as a solid 3D cube, de-risking the project's #1 risk (terminal-3D legibility).**

## Performance

- **Duration:** ~15 min (across human-verify checkpoint + tuning rounds)
- **Tasks:** 2 auto + 1 human-verify checkpoint (APPROVED)
- **Files modified:** 4

## Accomplishments

- Orbit `Camera` (yaw/pitch/radius/target) with a framerate-independent autopilot `step(dt)` that builds a `render3d::ViewParams` each frame — the producer side of the 05->04 decoupling (render3d never imports Camera).
- `SceneShape`: the single framebuffer->braille `Painter::paint` adapter, resolving the braille coordinate / y-flip spike (flip applied once in projection, not re-applied in the blit).
- End-to-end render path wired into the app loop; the cube orbits smoothly on the steady tick at low CPU.
- **Human verify APPROVED in a real terminal (via captured screenshots):** brightness, 3D legibility, and no clipping all good.
- Frustum-safe orbit defaults pinned by `orbit_keeps_all_vertices_in_frustum` (full yaw turn x worst-case pitch extremes, 0.12 NDC margin) so no face is dropped mid-rotation.

## Task Commits

1. **Task 1: Orbit camera + autopilot** - `9ec6369` (feat)
2. **Task 2: SceneShape braille blit wired into ui::view** - `823bce1` (feat)

**Tuning round 1 (from human-verify feedback):**
3. **Brighten scene — vivid palette, higher Lambert floor, weaker fog** - `22dd223` (fix)
4. **Reveal top/bottom faces + pull camera back to kill clipping** - `88e69bc` (fix)
5. **Pre-checkpoint progress note** - `f1d0165` (docs)

**Final tuning (after human APPROVED, requested faster orbit):**
6. **Increase autopilot orbit speed (YAW_RATE 0.35 -> 0.525)** - `fecf62e` (fix)

**Plan metadata:** see the docs commit completing this plan.

## Files Created/Modified

- `src/camera/mod.rs` - Orbit Camera + autopilot; builds render3d::ViewParams; frustum-safe defaults documented in module docs.
- `src/ui/scene.rs` - SceneShape: framebuffer->braille Painter blit (the single coordinate-mapping adapter).
- `src/ui/mod.rs` - Replaced the 01-01 placeholder scene block with the SceneShape canvas; threads Camera/palette/config through view().
- `src/app.rs` - Added `camera: Camera`; advances orbit on the logic tick via `camera.step(dt)`; stores scene-area pixel viewport for resize.

## Final Tuned Values (post human-verify)

| Knob | Value | Note |
|------|-------|------|
| Orbit radius | 6.0 | pulled back from 4.0 to clear the wide pitch sweep |
| FOV | 60deg | ~21.7deg vertical frustum margin at radius 6.0 |
| Yaw rate | 0.525 rad/s (~30deg/s) | full revolution ~12s; bumped 1.5x from ~20deg/s |
| Pitch bias | PI/8*1.2 (~15deg) | steady downward tilt onto the top face |
| Pitch amplitude | PI/8*1.5 (~33deg) | wide bob to reveal top + bottom |
| Pitch limit | PI/2 - 0.15 | hard gimbal clamp |
| MIN_LAMBERT | 0.62 | raised from 0.35 for legible oblique faces |
| FOG_MIN | 0.7 | raised from 0.45 so far faces stay readable |
| running/glow | #8A8AF0 | brightened from #5B5BD6 |
| cell_aspect | 2.0 | cube reads cubic; unchanged from 01-03 |

## Decisions Made

See `key-decisions` in frontmatter. Headline: frustum-safe orbit (radius 6.0 / fov 60deg) + a wide eased pitch bob to reveal top/bottom faces + a brighter palette and higher shading floors, all confirmed by the human verify.

## Human-Verify Outcome

**APPROVED** after tuning. The human reviewed the rendered cube in a real terminal (captured screenshots): brightness good, reads clearly as 3D, no clipping/face-drop across the orbit. They requested one final change — a slightly faster orbit — applied in `fecf62e` (yaw ~20deg/s -> ~30deg/s).

### Known / accepted behavior (signed off by the human)

- **Head-on poses briefly read flat:** when the camera is square to one face the cube momentarily looks 2D. This is geometrically correct (a cube face IS flat) and is accepted, not a bug.
- **Bottom face shows less than the top:** the steady ~15deg downward pitch bias favors looking onto the top face, so the bottom is revealed less. Intentional and accepted.
- **View-fixed headlight Lambert:** cull + shading share one frame's ViewParams, so shading tracks the view exactly — no stale-frame "lag" bug (diagnosed in tuning round 1, left as designed).

### Deferred-from-01-01 terminal checks — CONFIRMED

The interactive behaviors that the no-TTY exec sandbox could not verify in 01-01 were confirmed working in the real terminal during this verify:

- **q / Esc restore:** terminal restored (cursor + echo) on quit.
- **Resize:** scene re-sizes/re-centers with no garbage.
- **Panic restore:** terminal restored on panic.

This clears the 01-01 "User Verification Required" pending todo.

## Deviations from Plan

None — plan executed as written. The post-checkpoint tuning (brightness, pitch, radius, shading floors, orbit speed) is the expected human-verify feedback loop the plan's checkpoint resume-signal explicitly calls for, not unplanned scope.

## Issues Encountered

- **Clipping on the wide pitch sweep:** widening pitch to reveal top/bottom swung vertices toward the frustum edge. Resolved by pulling the orbit radius 4.0 -> 6.0 and rewriting `orbit_keeps_all_vertices_in_frustum` to sweep the worst-case pitch extremes with a 0.12 NDC margin.
- **No-TTY sandbox:** `cargo run` panics at terminal init in the exec sandbox (expected) — the runtime/legibility checks were the human's job in a real terminal.

## Verification

- `cargo build` / `cargo clippy --all-targets` / `cargo test` all clean — 34 tests pass.
- `grep Color::Rgb src/` matches only `src/theme/mod.rs` (THEME-01 holds).
- `grep Camera src/render3d/` returns nothing (decoupling holds).

## User Setup Required

None.
