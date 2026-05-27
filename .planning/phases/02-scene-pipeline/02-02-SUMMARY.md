---
phase: 02-scene-pipeline
plan: 02
subsystem: render-braille
tags: [braille, rasterizer, render-scene, painters-sort, occlusion, fog, world, frame-scene]

# Dependency graph
requires:
  - phase: 02-scene-pipeline
    provides: "world::Entity/World/SceneBounds, synthetic_scene(), Camera::frame_scene(&SceneBounds)"
  - phase: 01-render-core
    provides: "render3d::Projector/ViewParams/Framebuffer, theme::Palette (status->color, dim, fog), single-cube render() shading/AA-resolve, RenderConfig"
provides:
  - "render3d::render_scene(&[Entity], view, viewport, palette, config) -> Framebuffer — multi-box braille rasterizer"
  - "RenderFace redefined to carry world-space verts + per-box base color; fill_face(&[Vec3;4]) takes world verts directly"
  - "ui::scene::render_scene(frame, area, camera, &[Entity], palette, config) — braille SceneShape blits the whole World, framed by Camera::frame_scene"
affects: [02-04-integration (replaces the temporary synthetic_scene() seam with app-owned World), 05-polish]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Single combined cross-box painter's pool: every visible face of every box sorted farthest-first as ONE list (inter-box occlusion, not per-box-then-concatenated)"
    - "Per-face world-space geometry on RenderFace (verts, not shared-cube indices) so the same fill path serves single-cube and multi-box"
    - "Shared shade_and_fill between render() and render_scene() — both paths shade identically (orientation Lambert x scene-wide fog, per-face base color)"

key-files:
  created: []
  modified:
    - src/render3d/raster.rs
    - src/render3d/mod.rs
    - src/ui/scene.rs
    - src/ui/mod.rs

key-decisions:
  - "RenderFace carries its OWN 4 world-space verts + base Color (not indices into a shared cube) — the data-model change the whole multi-box path hinges on"
  - "fill_face signature changed to fill_face(fb, projector, &[Vec3;4], color) — per-box world vertices reach the fill; no cube/indices params"
  - "Cross-box painter's sort over ONE combined pool (not sort-per-box-then-concat) is what makes a near box occlude a far one"
  - "Scene-wide fog range (near/far across ALL visible faces of ALL boxes) so far boxes dim and depth reads at scale"
  - "Camera::frame_scene called in ui::scene::render_scene before building ViewParams — orbit frames the whole rack, not the unit-cube radius"

patterns-established:
  - "Automated AT-SCALE ordering guard: a 3x3 crowded grid + known near box, asserting the center pixel is the near box's color — cross-box sort correctness is test-pinned, not only human-judged"

# Metrics
duration: ~8min
completed: 2026-05-27
---

# Phase 2 Plan 2: Braille Multi-Box Rasterizer Summary

**Generalized the braille rasterizer from one centered cube to a whole `World` of boxes: `render3d::render_scene` draws every Entity at its world slot, sized by its half-extents, colored by its theme::Status, with a SINGLE cross-box farthest-first painter's sort so near boxes occlude far ones — wired into the braille SceneShape and framed by Camera::frame_scene.**

## Performance

- **Duration:** ~8 min
- **Tasks:** 2 (both `auto`, autonomous)
- **Files modified:** 4 (0 created, 4 modified)
- **Tests:** 52 -> 60 (+8 render_scene tests: 2-box occlusion, at-scale 3x3 grid occlusion, per-box color, empty scene, off-center framing, plus the updated painters-sort unit test)

## Accomplishments
- **`render3d::render_scene(&[Entity], view, viewport, palette, config) -> Framebuffer`** — the multi-box braille rasterizer. Per box: transform the unit cube into world space once (scale by `2*half_extents`, translate to `position`), gather each visible face's 4 world verts, back-face cull, push to ONE combined face pool.
- **Cross-box painter's sort** — the single combined pool is sorted farthest-first as a whole, so a near box's faces are drawn last and overwrite far ones (the critical inter-box occlusion correctness step).
- **Scene-wide fog** — near/far distance range computed across ALL visible faces of ALL boxes, so far boxes dim and the rack reads at depth (no flat-noise collapse).
- **Per-box status color** — `palette.status_color(entity.status)` carried on each `RenderFace.base` (CONT-01, zero inline RGB in render3d).
- **Braille SceneShape** now blits the World via `render3d::render_scene` and frames the orbit via `Camera::frame_scene(&bounds)`; the lone `unit_cube()` call is gone from the braille UI path.
- Preserved everything from the single-cube path: supersample buffer, AA-resolve (dominant-color-per-dot, flat faces), back-face cull, orientation Lambert, identity y-mapping (projector owns the single flip). The legacy `render()` still passes all its tests.

## Task Commits

1. **Task 1: render_scene multi-box rasterizer + cross-box sort** - `3ed9df2` (feat)
2. **Task 2: wire World into braille SceneShape + frame camera** - `b9c2bec` (feat)

## Files Modified
- `src/render3d/raster.rs` - redefined `RenderFace { verts: [Vec3;4], normal, distance, base: Color }`; changed `fill_face(fb, projector, &[Vec3;4], color)`; added `render_scene` + shared `shade_and_fill`; updated `render()` to materialize world verts; removed now-unused `face_centroid`; +5 render_scene tests, updated the painters-sort unit test.
- `src/render3d/mod.rs` - re-export `render_scene` alongside `render`.
- `src/ui/scene.rs` - `SceneShape` carries `entities: Vec<Entity>`, renders via `render3d::render_scene`; `render_scene` takes `&[Entity]` and calls `Camera::frame_scene(&bounds)` before building ViewParams; dropped the `unit_cube`/single-cube `render` imports.
- `src/ui/mod.rs` - threads a temporary `world::synthetic_scene()` into `scene::render_scene` (the 02-04 seam).

## Decisions Made
- **RenderFace world-verts redefinition (the load-bearing data-model change):** with many boxes there is no single shared cube to index, so `RenderFace` now carries its own 4 world-space vertices and the per-box base color. The legacy `render()` materializes its face's world verts from `cube.vertices[indices]` so BOTH paths share one face type and one `fill_face`.
- **New `fill_face` signature:** `fn fill_face(fb: &mut Framebuffer, projector: &Projector, verts: &[Vec3; 4], color: Color)` — projects `verts[k]` directly; no `cube`/`indices` params. This is what lets per-box world vertices reach the fill.
- **`render_scene` signature:** `pub fn render_scene(entities: &[Entity], view: ViewParams, viewport: (usize, usize), palette: &Palette, config: &RenderConfig) -> Framebuffer`.
- **SceneShape consumes entities + bounds:** `ui::scene::render_scene` takes `&[Entity]`, computes `SceneBounds::from_entities(entities)`, frames a *copy* of the camera with `frame_scene(&bounds)`, and stores `entities.to_vec()` on the shape (the shape is drawn behind a `&self` `Shape::draw`, so it owns its snapshot).

## Cross-box-sort approach & cyclic-overlap caveat
- The approach is a SINGLE combined farthest-first painter's sort over every visible face of every box (`Vec<RenderFace>` spanning all entities, sorted once by `distance` = camera-to-face-centroid). This is sufficient for the convex, **non-overlapping axis-aligned boxes** the layout guarantees (`no_two_aabbs_overlap`: min center distance >= 2*MAX_HALF, so AABBs never intersect).
- **Residual caveat (for the 02-04 human-verify):** painter's sort orders by face *centroid* distance. For non-intersecting convex boxes this is correct in the common case, but the classic painter's-algorithm cyclic-overlap failure (mutually-overlapping silhouettes where no single back-to-front order exists per-pixel) is theoretically possible when two boxes' screen footprints overlap and their centroid-distance order disagrees with the per-pixel depth order. The kitty backend's per-pixel z-buffer is immune; the braille path relies on centroid ordering. Because the boxes never physically intersect and are reasonably separated (SLOT_SPACING 3.0, GROUP_DEPTH 18.0), this should be rare — but it is the thing to watch when eyeballing the rack at scale in 02-04. If artifacts appear, options are: split faces / sort sub-fragments, or (heavier) add a per-pixel depth test to the braille fill.

## Deviations from Plan
None — plan executed as written. The only adjustment was tightening a too-strict threshold in my own new sanity test (`render_scene_off_center_box_lands_in_framebuffer`): the first eye distance (z=20) put the small box at only 24 lit px; pulled the eye to z=6 so the off-center box rasterizes a solid footprint. No production-code change.

## Verification
- `cargo test`: 60 passed (52 prior + 8 new), 0 failed.
- `cargo clippy --all-targets`: clean.
- `cargo build`: clean.
- `grep Color::Rgb src/render3d/`: none (THEME-01 holds — color flows through Palette only).
- `grep unit_cube src/ui/scene.rs`: none (braille UI no longer renders a lone cube).
- `grep "fn fill_face" src/render3d/raster.rs`: takes `verts: &[Vec3; 4]`, no `cube`/`indices`.
- Inter-box occlusion pinned by BOTH `render_scene_two_box_occlusion_near_hides_far` and the at-scale `render_scene_at_scale_grid_near_hides_far` (3x3 crowded grid + known near box; center pixel must be the near box's color).

## How to capture a braille frame for the 02-04 human-verify
- The braille path renders straight to the terminal (no offline RGBA dump — `--dump-rgba` is the KITTY path only, see `src/main.rs:45`). To verify the braille rack at scale, run the binary forcing the braille backend in a real terminal:
  - `cargo run -- --braille` (or any non-kitty terminal: Alacritty/SSH auto-selects braille).
  - `--braille` / `--kitty` override the auto-detected backend (`src/main.rs:53-54`).
- What to look at (per the screenshot/verify rule, MEMORY): dozens of boxes read as distinct solid forms; near boxes cleanly occlude far ones (no far-through-near bleed); status colors are distinguishable; far boxes are fogged dimmer than near ones (depth reads); the whole rack is framed (nothing clipped at the edges as the orbit spins). If overlap artifacts appear, see the cyclic-overlap caveat above.

## Next Phase Readiness (02-04 integration seam)
- **The seam:** `ui::mod::view` currently builds a fresh `world::synthetic_scene()` every frame and threads `world.entities` into `scene::render_scene`. 02-04 should give the `App` an owned `World` (built once) and pass `&app.world.entities` instead, deleting the per-frame `synthetic_scene()` call. The `ui::scene::render_scene` signature (`&[Entity]`) and SceneShape are already final; only the caller changes.
- **GROUP_DEPTH knob (carried from 02-01):** still open — if the rack reads too sparse/deep on the braille screen during the 02-04 verify, tighten `world::layout` GROUP_DEPTH 18 and re-confirm the clustering test margin.
- The kitty rasterizer (02-03, parallel Wave 2) consumes the same `World` + `frame_scene` independently; this plan touched none of its files.
