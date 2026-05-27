---
phase: 02-scene-pipeline
plan: 01
subsystem: scene-data
tags: [world, entity, layout, scene-bounds, camera, glam, tdd, deterministic]

# Dependency graph
requires:
  - phase: 01-render-core
    provides: "theme::Status + Palette (status->color), Camera orbit + frustum-safe reasoning, render3d::Projector/ViewParams, RenderConfig"
provides:
  - "src/world module: Entity, network-grouped rack layout(), synthetic_scene() (50 boxes / 5 groups), SceneBounds"
  - "Clamped/compressive load_to_half_extent (CONT-02 sizing proxy)"
  - "Camera::frame_scene(&SceneBounds) — frames the whole rack, frustum-safe across the full yaw orbit (CAM-01 generalization)"
  - "RenderConfig.far raised 100->500 to enclose a multi-group rack"
affects: [02-02-braille-rasterizer, 02-03-kitty-rasterizer, 04-networks (group->floor-plane)]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Pure deterministic data layer: layout/sizing are closed-form functions of stable id/group (no clock, no rand, no global mutable state)"
    - "Forward-facing public API guarded by targeted #[allow(dead_code)] until the rasterizer plans consume it (mirrors theme/render3d)"
    - "Status flows through the existing theme::Status; world carries zero inline RGB (THEME-01 holds)"

key-files:
  created:
    - src/world/mod.rs
    - src/world/entity.rs
    - src/world/layout.rs
    - src/world/scene.rs
  modified:
    - src/camera/mod.rs
    - src/config/mod.rs
    - src/main.rs

key-decisions:
  - "Layout RESOLVED: network-grouped rack grid — groups are contiguous Z-bands (future Phase 4 floor-planes), boxes fill an X/Y rack grid within a group"
  - "Sizing constants: MIN_HALF 0.3, MAX_HALF 1.2; SLOT_SPACING 3.0 (>2*MAX_HALF); GROUP_DEPTH 18.0; GRID_COLS 4; scene = 5 groups x 10 = 50 entities"
  - "frame_scene radius via NEAR-CORNER tangent bound against the cell-aspect-narrowed HORIZONTAL half-FOV, not the simple sphere/vertical bound"
  - "RenderConfig.far 100->500 (Rule 3 blocking fix): the deep rack + pulled-back framing radius put far box corners beyond the old far plane"

patterns-established:
  - "TDD RED->GREEN per task: stub-fails commit (test) then real-impl commit (feat)"
  - "Deterministic synthetic data: Knuth multiplicative hash on id for load, id%10 for status variety — reproducible, no rand"

# Metrics
duration: ~12min
completed: 2026-05-27
---

# Phase 2 Plan 1: World Scene Data Layer Summary

**A pure, deterministic `world` module that turns one cube into a 50-box network-grouped rack — stable slots, clamped/compressive sizing, theme::Status per box, enclosing SceneBounds — plus a Camera::frame_scene that fits the whole rack in-frustum across the full orbit.**

## Performance

- **Duration:** ~12 min
- **Tasks:** 3
- **Files modified:** 7 (4 created, 3 modified)
- **Tests:** 36 -> 52 (+16: 6 entity, 3 layout, 6 scene, 1 camera)

## Accomplishments
- Domain `Entity { id, position, half_extents, status: theme::Status, group }` reusing the existing status enum (no parallel color/status type).
- `load_to_half_extent`: clamped, sqrt-compressive, monotonic, NaN/out-of-range-safe sizing proxy (CONT-02).
- Network-aware deterministic `layout(group, index)`: closed-form, collision-free, group-clustered slots (CONT-05 — the resolved rack-grid-vs-floors decision).
- `synthetic_scene() -> World`: 50 boxes across 5 groups with deterministic per-id load + varied status, plus `SceneBounds { min, max, center, radius }`.
- `Camera::frame_scene`: orbit-targets the scene center and solves a frustum-safe radius enclosing the whole rack, pinned across a full yaw sweep.

## Task Commits

1. **Task 1 (RED): entity tests** - `3b774f4` (test)
2. **Task 1 (GREEN): load_to_half_extent** - `5534fda` (feat)
3. **Task 2 (RED): layout + scene tests** - `7a6afd1` (test)
4. **Task 2 (GREEN): layout + synthetic scene** - `10f5a51` (feat)
5. **Task 3: Camera::frame_scene + far-plane fix** - `1161194` (feat)

_TDD tasks 1 & 2 each split test (RED) -> feat (GREEN); no refactor commit needed (impls were already minimal)._

## Files Created/Modified
- `src/world/entity.rs` - Entity struct + clamped compressive `load_to_half_extent` (MIN_HALF 0.3, MAX_HALF 1.2).
- `src/world/layout.rs` - `layout(group, index)` network-grouped rack grid (SLOT_SPACING 3.0, GROUP_DEPTH 18.0, GRID_COLS 4).
- `src/world/scene.rs` - `synthetic_scene()` (50 boxes / 5 groups, deterministic load+status) + `SceneBounds::from_entities`.
- `src/world/mod.rs` - `World { entities, bounds }`, module wiring, public re-exports.
- `src/camera/mod.rs` - `Camera::frame_scene(&SceneBounds)` + `FRAME_HALF_FOV`/`FRAME_SAFETY_MARGIN` consts + `frame_scene_keeps_whole_scene_in_frustum` test.
- `src/config/mod.rs` - `RenderConfig.far` 100 -> 500.
- `src/main.rs` - `mod world;`.

## Decisions Made
- **Layout (resolved phase research):** network-grouped rack grid. Each group is a contiguous Z-band (Phase 4 ENT-01 drops its network floor-plane in as the band's plane); within a group, boxes fill columns along X and shelves up Y. Network grouping is the first-class placement axis.
- **Constants:** MIN_HALF=0.3, MAX_HALF=1.2, SLOT_SPACING=3.0 (>2*MAX_HALF, the no-overlap guarantee), GROUP_DEPTH=18.0 (> max intra-group spread, the clustering guarantee), GRID_COLS=4. Scene = GROUP_COUNT 5 * PER_GROUP 10 = 50 entities.
- **frame_scene radius formula:** `theta = FRAME_HALF_FOV(0.367) - FRAME_SAFETY_MARGIN(0.15)`, `radius = scene_radius * (1 + 1/tan(theta))`. Uses the near-corner tangent bound (the box corner nearest the eye subtends a larger angle than the sphere bound predicts) and the cell-aspect-narrowed horizontal half-angle (the binding axis, not the 30 deg vertical).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Raised RenderConfig.far 100 -> 500**
- **Found during:** Task 3 (frame_scene frustum pin failed — far box corners clipped on NDC z).
- **Issue:** The multi-group rack spans ~72 units deep; frame_scene pulls the eye back ~5.5 scene-radii, putting the farthest corner ~240 units out — well beyond the old far=100 plane. Box corners clipped mid-orbit.
- **Fix:** far 100 -> 500. The single-cube path is unaffected (its fog is camera-distance absolute, not derived from far).
- **Files modified:** src/config/mod.rs
- **Commit:** `1161194`

**2. [Rule 1 - Bug in initial derivation] frame_scene used the wrong frustum axis/bound**
- **Found during:** Task 3 (first two formula attempts clipped).
- **Issue:** The simple `scene_r / sin(vertical_half_fov)` sphere bound is wrong here for two reasons: (a) cell_aspect=2.0 narrows the HORIZONTAL FOV (~21 deg) below the vertical (30 deg), so horizontal is the binding axis; (b) the box corner nearest the eye subtends a larger angle than the sphere bound predicts.
- **Fix:** Solve the near-corner tangent bound against the horizontal half-angle: `radius = scene_r*(1 + 1/tan(theta))`.
- **Files modified:** src/camera/mod.rs
- **Commit:** `1161194`

## Public API surface (for 02-02 braille & 02-03 kitty)
- `crate::world::World { entities: Vec<Entity>, bounds: SceneBounds }`
- `crate::world::Entity { id, position, half_extents, status: theme::Status, group }`
- `crate::world::scene::SceneBounds { min, max, center, radius }`
- `crate::world::synthetic_scene() -> World`
- `crate::world::layout::layout(group, index_in_group) -> Vec3`
- `crate::world::load_to_half_extent(load: f32) -> f32`
- `Camera::frame_scene(&SceneBounds)`

## Verification
- `cargo test`: 52 passed (36 prior + 16 new), 0 failed.
- `cargo clippy --all-targets`: clean.
- `cargo build`: clean.
- `grep theme::Status src/world/`: status flows through the existing enum; `grep Color::Rgb src/world/`: none (THEME-01 holds).
- Purity: no Instant/rand/static-mut in src/world/.

## Next Phase Readiness
- 02-02 (braille) and 02-03 (kitty) can both consume `synthetic_scene()` + `Camera::frame_scene` without touching each other's files. The kitty per-pixel z-buffer already handles arbitrary box overlap; the braille painter's sort must order boxes back-to-front (watch inter-box ordering).
- Phase 4 networks: `Entity.group` is the floor-plane key; layout already places groups as contiguous Z-bands.
- Knob to revisit during a render verify: GROUP_DEPTH 18 makes a deep scene (far raised to 500); if the rack reads too sparse/deep on screen, tighten GROUP_DEPTH and re-confirm the clustering test margin.
