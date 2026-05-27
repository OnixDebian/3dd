# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-05-26)

**Core value:** A beautiful, legible 3D scene that lets you grasp the state of your Docker environment at a glance — what's alive, what's hot, what's connected to what.
**Current focus:** Phase 2 COMPLETE — next: Phase 3 (Docker Data Layer)

## Current Position

Phase: 2 of 5 (Scene Pipeline) — COMPLETE
Plan: 4 of 4 complete (02-04 app integration + scene verify)
Status: Phase 2 COMPLETE — multi-box scene verified by human in a real terminal across both backends; VERIFICATION passed 4/4
Last activity: 2026-05-27 — Completed 02-04 (human-verify APPROVED); per-box self-spin + projected-AABB framing accepted

## Phase 2 Notes (for Phase 3 planning)

- **Architecture:** App owns a single `World` (src/world/: entity/layout/scene/mod). `synthetic_scene()` → 30 boxes / 3 network-groups, deterministic. Both renderers consume `&[Entity]` + `SceneBounds`; `Camera::frame_scene(&world)` binary-searches distance to projected-AABB fill (FRAME_TARGET_FILL=0.92, binding = horizontal axis at cell_aspect 2). When real Docker data lands (Phase 3), it replaces synthetic_scene() output — keep the same World/Entity shape.
- **Occlusion:** braille = single cross-box painter's sort (face-centroid); kitty = per-pixel z-buffer. Boxes never physically intersect (SLOT_SPACING 2.6), so painter's sort is safe for convex boxes.
- **DEVIATION / RECONCILE (CAM-01):** roadmap criterion #4 was "autopilot ORBIT camera". Human overrode it live → shipped PER-BOX self-spin (each box rotates about its own Y at SPIN_RATE 0.525) + STATIC scene-framed camera (YAW_RATE=0). Motion is framerate-independent (on logic tick). REVISIT in Phase 4 when manual explore (CAM-02/03) lands — decide whether orbit returns or per-box-spin stays.
- **Roadmap deviation carried from Phase 1:** kitty backend front-runs part of Phase 5 ROB-02 (capability/degrade). Still open.

Progress: ██████░░░░ ~60% (Phase 1: 5/5, Phase 2: 1/4)

## Performance Metrics

**Velocity:**
- Total plans completed: 4
- Average duration: ~3 min
- Total execution time: ~0 hours

**By Phase:**

| Phase | Plans | Total | Avg/Plan |
|-------|-------|-------|----------|
| 1 (Render Core) | 5/5 ✅ | ~27 min | ~5 min |

**Recent Trend:**
- Last 5 plans: 01-01 (2 min), 01-04 (4 min), 01-05 (~15 min incl. human-verify + tuning)
- Trend: steady, with 01-05 longer due to the legibility human-verify loop

## Accumulated Context

### Decisions

Decisions are logged in PROJECT.md Key Decisions table.
Recent decisions affecting current work:

- Roadmap: render-first build order — validate terminal-3D legibility (Phase 1) before any Docker work
- Roadmap: THEME-01 palette abstraction pulled into Phase 1 (avoid hardcoded-color rework)
- Roadmap: layout algorithm in Phase 2 designed network-aware up front, though networks visualize in Phase 4
- 01-01: Cargo package/binary named `dd3` (Rust crate names can't start with a digit); repo/project stays "3dd"
- 01-01: Added `futures` 0.3 for `StreamExt::next` on crossterm `EventStream`; crossterm uses `event-stream` feature
- 01-01: Frame cadence defaults — render 30 FPS (`tui::RENDER_FPS`), logic tick 60Hz (`tui::TICK_HZ`)
- 01-01: Terminal restore extracted to free `tui::restore()` reused by `Tui::exit`, `Drop`, and the panic hook
- 01-04: `render3d::ViewParams { eye, target, up, fov }` is the SOLE camera input — render3d has no `Camera` type dependency (plan 05's Camera builds a ViewParams and feeds it in)
- 01-04: `ViewParams.fov` overrides `RenderConfig.fov` (camera owns the lens; config keeps near/far/cell_aspect)
- 01-04: Occlusion via painter's sort (farthest-first) + back-face cull — NO per-pixel z-buffer (sufficient for convex boxes, PITFALLS #4)
- 01-04: Framebuffer `(w,h)` is braille SUB-PIXEL resolution (2*cells_w, 4*cells_h); `lit_pixels()` is the plan-05 blit feed
- 01-04: Shading = Lambert orientation (theme::dim, floor 0.35) × distance fog (palette.fog toward background, floor 0.45); base color = palette.status_color(Running), zero inline RGB in render3d
- 01-05: Camera→ViewParams producer side closes the 05→04 decoupling (render3d never imports Camera, no cycle); autopilot step(dt) on the logic tick (framerate-independent)
- 01-05: Frustum-safe orbit — DEFAULT_RADIUS 6.0, DEFAULT_FOV 60° keep all 8 cube vertices in-frustum across the full yaw×pitch orbit (~21.7° margin); pinned by orbit_keeps_all_vertices_in_frustum with 0.12 NDC margin
- 01-05 (human-verify tuning, APPROVED): brightened running/glow indigo #5B5BD6→#8A8AF0; raised MIN_LAMBERT 0.35→0.62 and FOG_MIN 0.45→0.7; PITCH_BIAS ~15° + PITCH_AMPLITUDE ~33° to reveal top/bottom faces; cell_aspect 2.0 (cube reads cubic)
- 01-05: Final yaw rate ~30°/s (YAW_RATE 0.525, ~12s/revolution) — bumped 1.5x from ~20°/s on human request
- 01-05: y-flip lives once in NDC→screen projection (01-03); SceneShape blit does NOT re-invert — cube confirmed right-side-up

**Phase 2 (Scene Pipeline):**
- 02-01: LAYOUT RESOLVED (rack-grid vs network-floors) → network-grouped rack grid. Groups = contiguous Z-bands (Phase 4 ENT-01 floor-planes drop in as the band's plane); within a group boxes fill columns/X and shelves/Y. Network grouping is the first-class placement axis.
- 02-01: `src/world` is pure deterministic data — layout()/sizing are closed-form functions of stable id/group (NO clock, NO rand, NO global mutable state). synthetic_scene() = 50 boxes / 5 groups, deterministic per-id load (Knuth hash) + status variety (id%10).
- 02-01: Sizing constants — MIN_HALF 0.3, MAX_HALF 1.2; load_to_half_extent is sqrt-compressive, clamped, NaN-safe (CONT-02). Layout consts — SLOT_SPACING 3.0 (>2*MAX_HALF, no-overlap), GROUP_DEPTH 18.0 (>intra-group spread, clustering), GRID_COLS 4.
- 02-01: Entity.status REUSES theme::Status (CONT-01); world carries ZERO inline RGB (THEME-01 holds) — color resolved downstream by Palette.
- 02-01: Camera::frame_scene targets bounds.center, radius = scene_r*(1 + 1/tan(theta)) — NEAR-CORNER tangent bound against the cell-aspect-narrowed HORIZONTAL half-FOV (FRAME_HALF_FOV 0.367, FRAME_SAFETY_MARGIN 0.15), NOT the vertical sphere bound. Pinned frustum-safe across full yaw orbit by frame_scene_keeps_whole_scene_in_frustum.
- 02-01: RenderConfig.far 100->500 (blocking fix) — the deep rack + pulled-back framing radius put far corners beyond the old far plane; single-cube fog is absolute so unaffected.

**Post-verification rendering evolution (2026-05-27, human-driven, all under 01-05):**
- ARCHITECTURE PIVOT → DUAL RENDER BACKEND, auto-selected by terminal capability (`src/kitty.rs::supports_kitty_graphics`, dispatched in `main.rs`):
  - **kitty graphics protocol (real RGB pixels)** in kitty/ghostty/wezterm — smooth, anti-aliased, no braille staircase ("ratty-quality"). PREFERRED high-quality path.
  - **braille** fallback everywhere else (Alacritty, SSH, dumb terminals — Alacritty has NO graphics protocol at all, unfixable there).
  - Override flags `--kitty` / `--braille`; `--dump-rgba <path>` writes one RGBA frame for offline inspection (how the kitty render is verified without a capturable display).
- kitty renderer (`src/kitty.rs`): per-pixel z-buffer (exact occlusion — well-suited to Phase 2's many overlapping boxes), 2× supersampled AA, square pixels (cell_aspect 1.0), flat per-face shading, zlib-compressed frames (`o=z`, ~148× smaller payload via `miniz_oxide`), status bar on reserved bottom row, radius 3.2. MUST build `--release` (debug raster ~79ms/frame ≈12fps; release ~11ms).
- braille shading refined: faces stay FLAT — resolve picks the DOMINANT face color per dot (mode), never a cross-face blend (blend darkened wall-tops); majority-coverage threshold drops faint AA "dribble" specks. Edge-outline experiment was tried and REVERTED (faces looked uneven / flickered).
- fog made ABSOLUTE (camera-distance ± cube bound), not per-frame visible-face min/max — fixed top-face brightness flicker.
- INPUT RESPONSIVENESS FIX (real bug): unbounded event channel + heavy render → Render/Tick backlog buried quit keys → app sometimes wouldn't quit on q/Esc/Ctrl-C. Fixed via `MissedTickBehavior::Skip` + main loop drains the queue and COALESCES Render to one draw (`tui::try_next`, `app::run`).
- Tests now 36 (added kitty `top_face_shade_is_yaw_invariant`).

### Pending Todos

- Phase 2 next: 02-02 (braille rasterizer) + 02-03 (kitty rasterizer) — both consume world::synthetic_scene() + Camera::frame_scene
- Render verify when a rasterizer lands: confirm the rack reads legibly; GROUP_DEPTH 18 makes a deep scene (far raised to 500) — tighten if too sparse/deep

### Blockers/Concerns

- ~~Legibility (will it look good?)~~ RESOLVED — Phase 1 legibility spike APPROVED by human in a real terminal; cube reads as a solid 3D form
- ~~01-01 interactive verification UNVERIFIED~~ RESOLVED — q/Esc restore, resize-no-garbage, panic restore all confirmed working in a real terminal during the 01-05 verify
- ~~Layout algorithm (rack-grid vs network-floors) unresolved~~ RESOLVED in 02-01 — network-grouped rack grid (groups as Z-bands)
- Inter-box draw ordering: braille path uses painter's sort (fine for convex boxes, watch overlapping boxes across groups); kitty path's per-pixel z-buffer already handles arbitrary overlap
- Volume size not exposed by Docker API — decide proxy metric in Phase 4 planning

## Session Continuity

Last session: 2026-05-27
Stopped at: Phase 2 COMPLETE (all 4 plans + human-verify APPROVED + VERIFICATION passed 4/4). Multi-box scene shipped on both backends; per-box self-spin + projected-AABB framing accepted by human. build/clippy/test clean (64 tests).
Resume file: None
Resume note: Phase 2 DONE. The synthetic multi-box scene reads legibly at scale on both backends and is human-approved. ARCHITECTURE: App owns one `World` (`src/world/`: entity/layout/scene/mod) — `synthetic_scene()` builds 30 boxes / 3 network-groups, deterministic from id (stable slots). Both renderers take `&[Entity]` + `SceneBounds`: braille `render3d::render_scene` (cross-box painter's sort) and kitty `render_rgba` (per-pixel z-buffer); `Camera::frame_scene(&world)` binary-searches camera distance to a projected-AABB fill (FRAME_TARGET_FILL=0.92, binding=horizontal at cell_aspect 2). Run `--release` for the kitty path; `--dump-rgba <path>` writes one RGBA frame for offline inspect (MEMORY: screenshot & verify after render changes). MOTION MODEL CHANGED (human override, RECONCILE in Phase 4): orbit disabled (YAW_RATE=0), each box self-spins about its own Y (SPIN_RATE 0.525), camera static & scene-framed — CAM-01 marked deviation. NEXT: Phase 3 — feed REAL Docker data (bollard) into this proven renderer, replacing synthetic_scene() output with live containers (same World/Entity shape); correct CPU%/mem (cumulative-delta), create/destroy events, graceful daemon-down/empty/permission states. Open roadmap deviation: kitty backend front-runs Phase 5 ROB-02; reconcile in Phase 5.
