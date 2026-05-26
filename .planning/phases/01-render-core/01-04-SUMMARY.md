---
phase: 01-render-core
plan: 04
subsystem: render
tags: [rasterizer, framebuffer, painters-algorithm, braille, glam, depth-shading, fog]

# Dependency graph
requires:
  - phase: 01-02
    provides: "theme::Palette (status_color), theme::dim/fog depth-shading helpers, Status enum"
  - phase: 01-03
    provides: "render3d::project::Projector + RenderConfig (cell_aspect/fov/near/far), pinned Y-flip"
provides:
  - "render3d::framebuffer::Framebuffer — braille sub-pixel color buffer with safe set/get/clear + lit_pixels() blit iterator"
  - "render3d::cube::unit_cube() — 8 verts, 6 CCW quad faces, outward normals, FaceId"
  - "render3d::ViewParams { eye, target, up, fov } — sole camera input (no Camera-type dependency)"
  - "render3d::render() — painter's-sorted, back-face-culled, depth-shaded, fogged solid-cube rasterizer"
affects: [01-05-scene-blit, camera-orbit, docker-node-rendering]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Painter's algorithm (farthest-first sort) for occlusion — no per-pixel z-buffer"
    - "Barycentric two-triangle quad fill with bounds-safe, NaN-guarded pixel writes"
    - "ViewParams DTO decouples render3d from the (later) Camera type, preserving 05->04 dep direction"
    - "All draw-site color resolved via Palette + theme::dim/fog (zero inline RGB in render3d)"

key-files:
  created:
    - src/render3d/framebuffer.rs
    - src/render3d/cube.rs
    - src/render3d/raster.rs
  modified:
    - src/render3d/mod.rs

key-decisions:
  - "ViewParams.fov overrides RenderConfig.fov (camera owns its lens; config keeps near/far/cell_aspect)"
  - "Faces are convex quads (CCW-from-outside) triangulated at fill time; FaceId kept for plan-05 per-face tinting"
  - "Two-tier shading: orientation via theme::dim toward black, distance fog via palette.fog toward background"
  - "MIN_LAMBERT floor (0.35) + FOG_MIN floor (0.45) keep edge-on/far faces from collapsing into background"

patterns-established:
  - "Framebuffer (w,h) is braille SUB-PIXEL resolution (2*cells_w, 4*cells_h); lit_pixels() is the plan-05 blit feed"
  - "Painter's sort + back-face cull is sufficient occlusion for convex boxes (PITFALLS #4)"

# Metrics
duration: 4min
completed: 2026-05-26
---

# Phase 1 Plan 04: Framebuffer Rasterizer Summary

**Painter's-sorted, back-face-culled, depth-shaded + fogged solid-cube rasterizer producing a braille-resolution Framebuffer from raw ViewParams — built on the existing Projector and Palette, zero inline RGB, zero Camera-type dependency.**

## Performance

- **Duration:** ~4 min
- **Started:** 2026-05-26T21:46:55Z
- **Completed:** 2026-05-26T21:50:36Z
- **Tasks:** 2
- **Files modified:** 4 (3 created, 1 modified)

## Accomplishments

- **Framebuffer** (`framebuffer.rs`): `{ w, h, color: Vec<Option<Color>> }` at braille sub-pixel resolution. `new/clear/set/get` with bounds-checked `set` (silently ignores out-of-range — never panics on resize/clip, PITFALLS #14) and a `lit_pixels() -> (x, y, Color)` iterator that is the blit feed for plan 05.
- **Unit cube** (`cube.rs`): `unit_cube()` yields 8 corner vertices and 6 convex quad faces wound CCW-from-outside, each with an outward unit normal and a stable `FaceId`. Winding is unit-tested to agree with the declared normal so the cull is provably correct.
- **`ViewParams { eye, target, up, fov }`** (`mod.rs`): the sole camera input. `render3d` has NO `Camera` type dependency — plan 05's `Camera` will build a `ViewParams` and feed it in.
- **`render()`** (`raster.rs`): builds a `Projector` from the view params, back-face-culls (outward-normal · to-eye), painter's-sorts faces farthest-first (`sort_unstable_by`), and barycentric-fills each face's two triangles. Shading = Lambert orientation (`theme::dim`) × distance fog (`palette.fog` toward background); base color from `palette.status_color(Status::Running)`. Pure module — returns a Framebuffer, no terminal/IO.

## Task Commits

1. **Task 1: Framebuffer + cube geometry + ViewParams** - `cdfdf25` (feat)
2. **Task 2: Painter's-sorted, depth-shaded, fogged face fill** - `0767f78` (feat)

**Plan metadata:** (this commit) `docs(01-04): complete framebuffer-rasterizer plan`

## Files Created/Modified

- `src/render3d/framebuffer.rs` (created) - braille sub-pixel color buffer; safe set/get/clear; lit_pixels iterator
- `src/render3d/cube.rs` (created) - unit cube: 8 verts, 6 quad faces, outward normals, FaceId
- `src/render3d/raster.rs` (created) - render(): cull + painter's sort + barycentric fill + shading/fog
- `src/render3d/mod.rs` (modified) - declare framebuffer/cube/raster modules; define & export ViewParams + render

## Decisions Made

- **ViewParams owns the lens:** `view.fov` overrides `RenderConfig.fov` inside `render()`; `near/far/cell_aspect` still come from config. This lets the camera (plan 05) drive FOV while the config holds the perceptual aspect knob.
- **Quads, not pre-split triangles:** faces stay convex quads (CCW-from-outside) and are triangulated at fill time. `FaceId` is retained so plan 05 can tint per-face later.
- **Two-tier shading:** orientation (Lambert) dims toward black via `theme::dim`; distance fog blends toward the palette background via `palette.fog`. Floors (`MIN_LAMBERT=0.35`, `FOG_MIN=0.45`) keep edge-on/far faces from disappearing.
- **No z-buffer:** painter's sort + back-face cull is sufficient occlusion for a convex box (PITFALLS #4); avoids per-pixel depth cost.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] "Roughly square" raster footprint assertion used the wrong aspect invariant**

- **Found during:** Task 2 (headless raster test `footprint_is_roughly_square`)
- **Issue:** The test asserted the lit-pixel bounding box was ~1:1. But at braille SUB-PIXEL resolution a cube that READS as cubic occupies `cell_aspect` (=2.0) times as many columns as rows — exactly the invariant plan 03's `unit_cube_footprint_is_cubic` pins. The cube was rendering correctly (bbox 28×14, ratio 2.0); the test's expectation was wrong.
- **Fix:** Renamed to `footprint_reads_cubic_at_subpixel_level` and asserted `bbox_w ≈ cell_aspect * bbox_h` (rel_err < 0.2), matching the plan-03 convention.
- **Files modified:** src/render3d/raster.rs
- **Verification:** Test passes; bbox 28×14 against expected 28.
- **Committed in:** `0767f78` (Task 2 commit)

**2. [Rule 1 - Bug] Test-fixture RGB literals tripped the "no inline RGB" grep**

- **Found during:** Task 2 verification (`grep -rn "Color::Rgb(" src/render3d/`)
- **Issue:** Framebuffer unit-test fixtures used `Color::Rgb(...)` constants, which — while only test data, never a production draw-site color — matched the plan's no-inline-RGB guard grep.
- **Fix:** Switched fixtures to named `Color::Red`/`Color::Blue`. The grep is now empty; all production color still flows through the Palette.
- **Files modified:** src/render3d/framebuffer.rs
- **Verification:** `grep -rn "Color::Rgb(" src/render3d/` returns nothing; framebuffer tests still pass.
- **Committed in:** `0767f78` (Task 2 commit)

---

**Total deviations:** 2 auto-fixed (2 bugs, both in/around test verification)
**Impact on plan:** Both fixes were in the verification harness, not the rasterizer logic — the cube rendered correctly from the start. No scope creep.

## Issues Encountered

None — both deviations above were test-harness corrections; production rasterizer logic passed on first build.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

Ready for **01-05** (scene blit + orbiting camera):

- `render(cube, ViewParams{eye,target,up,fov}, viewport, &Palette, &RenderConfig) -> Framebuffer` is the entry point. Plan 05's `Camera` builds a `ViewParams` (orbit → eye, fixed target/up, fov) and calls this. `render3d` deliberately has no `Camera` type — keep that dependency direction.
- **Framebuffer contract for the blit:** `(w, h)` is braille SUB-PIXEL size — set it to `(2*cells_w, 4*cells_h)`. Walk `fb.lit_pixels()` (yields `(x, y, Color)`), group into 2×4 sub-pixel cells → braille glyph + cell color.
- **Shading model:** orientation (Lambert, floor 0.35) × distance fog (toward palette background, floor 0.45). Front/near faces bright, side/far dimmer; occlusion proven (head-on center pixel resolves to near-face base color).
- All 30 tests green; build + clippy clean.

**Verification still inherited from 01-01:** interactive terminal behaviors remain UNVERIFIED in the sandbox (no TTY). Plan 05 is the first plan that actually blits to a real terminal — that is the moment to do the one-time manual legibility check.

---
*Phase: 01-render-core*
*Completed: 2026-05-26*
