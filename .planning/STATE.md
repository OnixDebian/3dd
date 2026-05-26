# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-05-26)

**Core value:** A beautiful, legible 3D scene that lets you grasp the state of your Docker environment at a glance — what's alive, what's hot, what's connected to what.
**Current focus:** Phase 1 — Render Core & Legibility Spike

## Current Position

Phase: 1 of 5 (Render Core & Legibility Spike)
Plan: 4 of 5 complete (01-04 framebuffer-rasterizer)
Status: In progress
Last activity: 2026-05-26 — Completed 01-04-framebuffer-rasterizer-PLAN.md

Progress: ████████░░ 80%

## Performance Metrics

**Velocity:**
- Total plans completed: 4
- Average duration: ~3 min
- Total execution time: ~0 hours

**By Phase:**

| Phase | Plans | Total | Avg/Plan |
|-------|-------|-------|----------|
| 1 (Render Core) | 4/5 | ~12 min | ~3 min |

**Recent Trend:**
- Last 5 plans: 01-01 (2 min), 01-04 (4 min)
- Trend: steady ~3 min/plan

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

### Pending Todos

- Verify 01-01 interactive behaviors in a REAL terminal (no TTY in exec sandbox): alt-screen render, q/Esc/Ctrl-C restore, deliberate-panic restore, resize-no-garbage, idle single-digit CPU. See 01-01-SUMMARY "User Verification Required".

### Blockers/Concerns

- Legibility (will it look good?) is MEDIUM confidence — Phase 1 is a deliberate spike to de-risk it
- 01-01 interactive verification UNVERIFIED (sandbox lacks a TTY); code follows canonical ratatui-async pattern but needs a one-time manual terminal check before Phase 2 relies on it
- Layout algorithm (rack-grid vs network-floors) unresolved — decide in Phase 2 planning
- Volume size not exposed by Docker API — decide proxy metric in Phase 4 planning

## Session Continuity

Last session: 2026-05-27
Stopped at: 01-05 human-verify tuning round 1 applied (brightness 22dd223, pitch+radius+test 88e69bc); PAUSED at a fresh human-verify checkpoint
Resume file: None
Resume note: Tuning round 1 from human feedback — (A) brighter: running/glow indigo #5B5BD6->#8A8AF0, MIN_LAMBERT 0.35->0.62, FOG_MIN 0.45->0.7; (B) top/bottom faces: added PITCH_BIAS ~15deg + widened bob to ~33deg amplitude; (C) clipping: radius 4.0->6.0 and rewrote orbit_keeps_all_vertices_in_frustum to sweep worst-case pitch with a 0.12 NDC margin; (D) shading "lag" diagnosed as designed view-fixed headlight Lambert (cull+shading share one frame's ViewParams — no stale-frame bug), left as-is. build/clippy/test all clean (34 tests). Awaiting human re-verify. After "approved", finish 01-05 — create 01-05-SUMMARY.md, update Current Position/Progress to 5/5 + Phase 1 complete, mark ROADMAP phase 1 done, metadata commit. No-TTY in sandbox so cargo run / restore checks are the human's job.
