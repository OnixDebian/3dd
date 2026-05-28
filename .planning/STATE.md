# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-05-26)

**Core value:** A beautiful, legible 3D scene that lets you grasp the state of your Docker environment at a glance — what's alive, what's hot, what's connected to what.
**Current focus:** Phase 3 (Docker Data Layer) — Wave 1 in flight (03-01 done; 03-02 running in parallel)

## Current Position

Phase: 3 of 5 (Docker Data Layer) — IN PROGRESS
Plan: 1 of 4 complete (03-01 docker stats normalizer)
Status: 03-01 GREEN; normalizer pinned by 13 unit tests; 03-02 running in parallel (Wave 1)
Last activity: 2026-05-28 — Completed 03-01 (docker stats normalizer); pure, NaN-safe, CPU%-delta + memory guarded, feeds existing load_to_half_extent

## Phase 2 Notes (for Phase 3 planning)

- **Architecture:** App owns a single `World` (src/world/: entity/layout/scene/mod). `synthetic_scene()` → 30 boxes / 3 network-groups, deterministic. Both renderers consume `&[Entity]` + `SceneBounds`; `Camera::frame_scene(&world)` binary-searches distance to projected-AABB fill (FRAME_TARGET_FILL=0.92, binding = horizontal axis at cell_aspect 2). When real Docker data lands (Phase 3), it replaces synthetic_scene() output — keep the same World/Entity shape.
- **Occlusion:** braille = single cross-box painter's sort (face-centroid); kitty = per-pixel z-buffer. Boxes never physically intersect (SLOT_SPACING 2.6), so painter's sort is safe for convex boxes.
- **DEVIATION / RECONCILE (CAM-01):** roadmap criterion #4 was "autopilot ORBIT camera". Human overrode it live → shipped PER-BOX self-spin (each box rotates about its own Y at SPIN_RATE 0.525) + STATIC scene-framed camera (YAW_RATE=0). Motion is framerate-independent (on logic tick). REVISIT in Phase 4 when manual explore (CAM-02/03) lands — decide whether orbit returns or per-box-spin stays.
- **Roadmap deviation carried from Phase 1:** kitty backend front-runs part of Phase 5 ROB-02 (capability/degrade). Still open.

Progress: ███████░░░ ~65% (Phase 1: 5/5, Phase 2: 4/4, Phase 3: 1/4)

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

**Phase 3 (Docker Data):**
- 03-01: PURE normalizer — bollard-free. `src/docker/stats.rs` owns `RawCpu`/`RawMem` (plain input structs 03-03 maps `ContainerStatsResponse` onto) + `StatSample` (cpu_pct/mem_used/mem_limit/mem_fraction/load/warming_up) + `normalize()`. CPU%-delta formula per PITFALLS Pitfall 1 with EVERY guard pinned by 13 unit tests on synthetic before/after samples.
- 03-01: `load = max(cpu_norm, mem_fraction)` in `[0,1]` — box grows for whichever resource it is hot on; `cpu_pct` + `mem_fraction` also exposed for future HUD.
- 03-01: warming-up detection = `prev_cpu.is_none()` OR prev counters both zero (covers bollard's first-frame `precpu_stats` shape); load forced to 0.0.
- 03-01: counter regression (`cur < prev`, e.g. daemon/container restart) also yields 0.0 cpu_pct — added as an explicit guard beyond the plan's list (auto-fix Rule 1).
- 03-01: f64 for delta math (counters are large u64), f32 only on user-facing fields; final-scrub coerces any non-finite slip to 0.0.
- 03-01: `src/docker/mod.rs` declares `pub mod stats;` live + MARKED commented stubs for `domain` / `connect` / `streams` — 03-02/03-03 each uncomment exactly one line when their file lands (no mod.rs conflict). Re-exports `normalize, RawCpu, RawMem, StatSample` for `crate::docker::normalize` call sites.
- 03-01: 64 → 77 tests; `cargo clippy --tests -- -D warnings` clean; stats.rs has zero `use bollard`.

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

- Phase 3 next (Wave 1 continuation): 03-02 (`docker::domain` + `docker::connect`) — uncomments `pub mod domain;` and `pub mod connect;` in `src/docker/mod.rs` (the marked commented stubs); produces the bollard client + connection state machine + empty/permission/down domain states.
- Phase 3 Wave 2: 03-03 (`docker::streams`) — uncomments `pub mod streams;`; maps bollard's `ContainerStatsResponse` onto `RawCpu`/`RawMem` and calls `crate::docker::normalize` per container; picks cgroup v1 `cache` vs v2 `inactive_file` per daemon.
- Phase 3 Wave 3: 03-04 (renderer wire) — replaces `world::synthetic_scene()` output with live containers using the same `World/Entity` shape; `StatSample::load` feeds `world::entity::load_to_half_extent` directly.
- Pre-existing fmt drift on the pre-phase-3 files (`src/world/scene.rs`, `src/app.rs`, etc.) — NOT touched by this plan; pick up in a separate `chore(fmt)` whenever.

### Blockers/Concerns

- ~~Legibility (will it look good?)~~ RESOLVED — Phase 1 legibility spike APPROVED by human in a real terminal; cube reads as a solid 3D form
- ~~01-01 interactive verification UNVERIFIED~~ RESOLVED — q/Esc restore, resize-no-garbage, panic restore all confirmed working in a real terminal during the 01-05 verify
- ~~Layout algorithm (rack-grid vs network-floors) unresolved~~ RESOLVED in 02-01 — network-grouped rack grid (groups as Z-bands)
- Inter-box draw ordering: braille path uses painter's sort (fine for convex boxes, watch overlapping boxes across groups); kitty path's per-pixel z-buffer already handles arbitrary overlap
- Volume size not exposed by Docker API — decide proxy metric in Phase 4 planning

## Session Continuity

Last session: 2026-05-28
Stopped at: 03-01 COMPLETE (Wave 1 of Phase 3). docker stats normalizer (`src/docker/stats.rs`) is pure, NaN-safe, CPU%-delta + memory guarded, pinned by 13 unit tests. 03-02 was running in parallel against the same working tree.
Resume file: None
Resume note: 03-01 done. The CPU%-delta gotcha (PITFALLS Pitfall 1) is now CONTAINED in a single pure function (`crate::docker::normalize`) with every guard pinned by tests on synthetic before/after samples. The `load` field on `StatSample` is in `[0,1]` and feeds `world::entity::load_to_half_extent` UNCHANGED — once 03-03 maps bollard's `ContainerStatsResponse` onto `RawCpu`/`RawMem`, the renderer's box-size signal becomes guaranteed-finite for real containers. `src/docker/mod.rs` declares `pub mod stats;` LIVE and carries MARKED commented stubs for `domain` / `connect` / `streams` — 03-02/03-03 each uncomment exactly one line when their file lands (no mod.rs conflict). NOTE on parallel execution: while 03-01 was running its Task 2 verification, the parallel 03-02 agent had simultaneously uncommented `pub mod domain;` in mod.rs and dropped an untracked `src/docker/domain.rs` into the tree; this plan's commits deliberately do NOT include those changes so 03-02 can land its own atomic commit. Wave 1 continues with 03-02; Wave 2 = 03-03 (streams); Wave 3 = 03-04 (renderer wire). All Phase-1/Phase-2 invariants intact: 77/77 tests, clippy clean on `--tests -- -D warnings`, no bollard import inside stats.rs.
