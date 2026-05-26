---
phase: 01-render-core
plan: 03
subsystem: render
tags: [glam, projection, perspective, aspect-correction, tdd, math]
type: tdd
requires: ["01-01"]
provides:
  - "Projector: world Vec3 -> screen pixel (x, y, depth) via glam look_at_rh/perspective_rh/project_point3"
  - "RenderConfig: single cell_aspect correction factor + fov/near/far defaults"
  - "Pinned NDC->screen Y-flip convention (+up world -> smaller screen y, top-left grid)"
affects:
  - "01-04 (rasterizer draws into the screen pixel space this defines)"
  - "01-05 (ratatui Canvas adapter must NOT double-flip Y)"
  - "05 (TOML config will load RenderConfig.cell_aspect/fov/near/far)"
tech-stack:
  added: []
  patterns:
    - "render3d is a pure, unit-testable module (no I/O, no terminal, no globals)"
    - "cell_aspect correction single-sourced into the perspective aspect term"
    - "NDC->screen Y-flip happens in exactly one helper (ndc_to_screen)"
key-files:
  created:
    - src/render3d/mod.rs
    - src/render3d/project.rs
    - src/config/mod.rs
  modified:
    - src/main.rs
key-decisions:
  - "Cubic = bbox_w ~= cell_aspect * bbox_h (not bbox_w == bbox_h): a cube reads cubic on a ~1:2 dot grid only when its dot footprint is cell_aspect times wider than tall"
  - "depth returned for sorting is NDC z in [-1,1] (smaller = nearer)"
  - "clip in view space (z > -near => None) BEFORE the perspective divide, plus an NDC frustum bounds check"
duration: ~12 min
completed: 2026-05-26
---

# Phase 01 Plan 03: Aspect-Correct 3D Projection Pipeline Summary

Pure glam-based world→screen projection (`look_at_rh` × `perspective_rh` →
`project_point3` → screen) with the terminal cell-aspect squash corrected by a
single named `cell_aspect` factor and the NDC→screen Y-flip pinned by test.

## What Was Built

**`src/render3d/project.rs` — `Projector`**

API:
- `Projector::new(eye: Vec3, target: Vec3, up: Vec3, viewport_px: (u32, u32), config: &RenderConfig) -> Projector`
  - builds `view = Mat4::look_at_rh(eye, target, up)` and
    `proj = Mat4::perspective_rh(config.fov, aspect, config.near, config.far)`,
    storing `view`, `view_proj = proj * view`, the pixel viewport, and `near`.
- `project(&self, world: Vec3) -> Option<(f32, f32, f32)>`
  - returns `(screen_x, screen_y, depth)` or `None` if clipped.
  - **depth = NDC z in `[-1, 1]`** (smaller = nearer) — the value plan 04 sorts
    on for the painter's algorithm.

Pipeline inside `project()`:
1. Transform world → view (`view.transform_point3`). Clip: if `view_pos.z > -near`
   the point is at/behind the near plane (RH view looks down −Z) → `None`. This
   happens **before** the perspective divide so behind-camera points never wrap
   to bogus on-screen coordinates.
2. Perspective divide via `view_proj.project_point3(world)` → NDC.
3. Frustum bounds check `in_frustum(ndc)` (each of x/y/z within `[-1, 1]`; glam's
   `perspective_rh` uses the OpenGL-style `[-1,1]` z range) → `None` if outside.
4. `ndc_to_screen(ndc, px_w, px_h)` → screen pixels.

**`src/config/mod.rs` — `RenderConfig`** (`Debug, Clone, Copy, PartialEq, Default`)
- `cell_aspect: f32` (default **2.0**), `fov: f32` (default `FRAC_PI_3` = 60°),
  `near: f32` (0.1), `far: f32` (100.0). Defaulted in code; Phase 5 wires TOML.

**`src/main.rs`** — added `mod config;` and `mod render3d;` (minimal additive
edits; coexisted with the parallel 01-02 `mod theme;`).

## The aspect term (how cell_aspect is composed)

Applied in exactly ONE production line (`project.rs:53`):

```rust
let aspect = (px_w / px_h) / config.cell_aspect;
```

`perspective_rh`'s aspect is width/height. Dividing the pixel aspect by
`cell_aspect` increases the NDC-x scale, so the same world unit length maps to a
proportionally wider dot footprint — undoing the fact that a braille dot is
~1:2 (twice as tall as wide). `grep -rn cell_aspect src/` confirms it is defined
once in config and read once in projection; no inline aspect magic numbers.

## The Y-flip convention (PINNED — plan 05 must NOT double-flip)

The NDC→screen mapping lives in exactly one helper (`project.rs:105`):

```rust
let x = (ndc.x * 0.5 + 0.5) * px_w;
let y = (1.0 - (ndc.y * 0.5 + 0.5)) * px_h;   // single Y-flip
```

NDC is math-coords (+y up); the framebuffer is a top-left grid (+y down). The
`1.0 -` flips it. Locked by `up_world_point_maps_to_upper_half`: a world point
offset along the camera's +up direction projects to a **strictly smaller screen
y** than the target. **Convention for downstream plans: the flip is already
applied here. Plan 04 rasterizes into this top-left pixel space; plan 05's
ratatui Canvas adapter must avoid a second flip.**

## TDD Cycle

- **RED** (`6c9c484`): five failing tests — center, cubic, Y-flip orientation,
  clipping, aspect-knob — against `RenderConfig` + a `Projector` stub
  (`project` returned `None`). 4/5 failed; the clipping test was a structural
  false-pass off the stub's blanket `None`, then turned into a true pass in GREEN.
- **GREEN** (`6a046e8`): implemented the full pipeline; all 5 pass (12/12 with the
  parallel 01-02 theme tests). Included a corrected cubic assertion (see deviation).
- **REFACTOR**: skipped — no commit. `ndc_to_screen` and `in_frustum` were already
  extracted in GREEN and `cell_aspect`/Y-flip are each single-sourced; no
  behavior-preserving cleanup was warranted.

## Test Results

`cargo test` → **12 passed, 0 failed** (5 projection + 7 inherited theme).
`cargo clippy --bin dd3` → clean for `render3d`/`config`. Purity verified: no
`crossterm`/`ratatui::Terminal`/`std::io`/`print*` in `src/render3d/` or `src/config/`.

The five projection assertions:
- `target_projects_to_viewport_center` — look-at target → `(px_w/2, px_h/2)` ±1px.
- `unit_cube_footprint_is_cubic` — `bbox_w ≈ cell_aspect * bbox_h` (rel err < 0.1).
- `up_world_point_maps_to_upper_half` — +up point's screen y strictly < target's.
- `behind_camera_point_is_clipped` — point at +50 Z (behind camera) → `None`.
- `cell_aspect_is_load_bearing` — `cell_aspect` 1.0 vs 3.0 widens the footprint
  ratio in the expected direction (proves the factor is wired and single-sourced).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Corrected the "cubic" assertion to be physically right**
- **Found during:** GREEN phase. With default `cell_aspect=2.0` on a square
  pixel viewport, a unit cube projected to `bbox_w = 2 * bbox_h`. My RED test
  asserted `bbox_w ≈ bbox_h`, which is physically *wrong*: on a real terminal a
  dot is ~1:2 (twice as tall as wide), so a dot footprint with equal width/height
  renders as a brick, not a cube. The plan's prose ("WIDTH ≈ HEIGHT") was a
  simplification; its load-bearing `must_haves` truth is "cubic **under the
  aspect-correction factor**".
- **Fix:** Assert `bbox_w ≈ cell_aspect * bbox_h` (collapses to width==height
  only when `cell_aspect == 1.0`, i.e. perfectly square dots). Kept the
  research-recommended default `cell_aspect = 2.0`.
- **Files modified:** src/render3d/project.rs (test only).
- **Verification:** test now passes at default config; aspect-knob test still
  independently proves the factor changes the ratio.
- **Commit:** 6a046e8 (folded into GREEN).

**2. [Rule 3 - Blocking] Suppressed dead-code/unused-import warnings on the new modules**
- **Found during:** GREEN build. `Projector`/`RenderConfig` are exercised by
  tests but not yet consumed by the binary (plans 04/05 will), producing
  `dead_code`/`unused_imports` warnings that dirtied an otherwise-clean build.
- **Fix:** module-scoped `#![allow(dead_code)]` in `project.rs` and `config/mod.rs`
  (with a comment noting future consumers), and `#[allow(unused_imports)]` on the
  `pub use project::Projector;` convenience re-export.
- **Files modified:** src/render3d/project.rs, src/render3d/mod.rs, src/config/mod.rs.
- **Verification:** `cargo build`/`cargo clippy --bin dd3` clean.
- **Commit:** 6a046e8 (folded into GREEN).

**Total deviations:** 2 auto-fixed (1 bug in a test assertion, 1 blocking build
hygiene). No scope creep; no architectural changes.

## State Notes (for orchestrator → STATE.md)

**Decisions to record:**
- 01-03: "cubic" defined as `bbox_w ≈ cell_aspect * bbox_h` (a square dot
  footprint looks like a brick on ~1:2 terminal dots); `cell_aspect` default 2.0.
- 01-03: Projection `depth` returned for sorting is NDC z `[-1, 1]` (smaller = nearer).
- 01-03: NDC→screen Y-flip is applied once here (top-left grid); plan 05 adapter
  must NOT double-flip.
- 01-03: Clip in view space (`z > -near` → None) before the perspective divide.

**Concerns/coordination:**
- `src/main.rs` and `src/config/mod.rs` are shared with the parallel 01-02 plan.
  This plan made minimal additive edits (`mod config; mod render3d;`) and created
  `config/mod.rs`. If 01-02 also created `config/mod.rs`, the two definitions must
  be merged at wave reconciliation (this plan owns `RenderConfig`; 01-02 may add
  sibling config types/fields).
- The aspect/Y-flip math is unit-correct but its *perceptual* payoff (does a cube
  actually read as 3D?) is still gated on plan 05's human terminal check.

## Files Created/Modified

- **Created:** `src/render3d/mod.rs`, `src/render3d/project.rs` (258 lines incl.
  tests), `src/config/mod.rs` (41 lines)
- **Modified:** `src/main.rs` (+2 module declarations)

## Commits

- `6c9c484` test(01-03): failing projection + aspect + y-flip tests
- `6a046e8` feat(01-03): aspect-correct 3D projection pipeline

## Next Step

Ready for 01-04 (rasterizer) — it draws into the top-left screen pixel space and
sorts by the NDC-z depth this `Projector` returns.
