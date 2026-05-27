# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-05-26)

**Core value:** A beautiful, legible 3D scene that lets you grasp the state of your Docker environment at a glance — what's alive, what's hot, what's connected to what.
**Current focus:** Phase 1 — Render Core & Legibility Spike

## Current Position

Phase: 1 of 5 (Render Core & Legibility Spike)
Plan: 5 of 5 complete (01-05 scene-orbit-verify)
Status: Phase 1 COMPLETE — legibility spike APPROVED by human in a real terminal
Last activity: 2026-05-27 — Completed 01-05-scene-orbit-verify-PLAN.md (human-verify APPROVED)

Progress: ██████████ 100%

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

### Pending Todos

- (none open from Phase 1) — next: Phase 2 planning (layout algorithm: rack-grid vs network-floors)

### Blockers/Concerns

- ~~Legibility (will it look good?)~~ RESOLVED — Phase 1 legibility spike APPROVED by human in a real terminal; cube reads as a solid 3D form
- ~~01-01 interactive verification UNVERIFIED~~ RESOLVED — q/Esc restore, resize-no-garbage, panic restore all confirmed working in a real terminal during the 01-05 verify
- Layout algorithm (rack-grid vs network-floors) unresolved — decide in Phase 2 planning
- Volume size not exposed by Docker API — decide proxy metric in Phase 4 planning

## Session Continuity

Last session: 2026-05-27
Stopped at: 01-05 COMPLETE — human APPROVED the cube in a real terminal; final orbit-speed bump applied (fecf62e); SUMMARY + STATE finalized. Phase 1 done (5/5).
Resume file: None
Resume note: Phase 1 (Render Core & Legibility Spike) is COMPLETE and APPROVED. The terminal-3D legibility risk is de-risked — cube reads as a solid, smooth, cubic 3D form at low CPU; q/Esc/resize/panic-restore confirmed in a real terminal. Final knobs: radius 6.0, fov 60°, yaw ~30°/s (0.525), pitch bias ~15° + amplitude ~33°, MIN_LAMBERT 0.62, FOG_MIN 0.7, cell_aspect 2.0, running/glow #8A8AF0. build/clippy/test clean (34 tests). NEXT: Phase 2 planning — generalize one cube into many boxes against this proven render path; resolve the layout algorithm (rack-grid vs network-floors), designed network-aware up front.
