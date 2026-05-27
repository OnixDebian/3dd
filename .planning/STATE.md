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

- (none open from Phase 1) — next: Phase 2 planning (layout algorithm: rack-grid vs network-floors)

### Blockers/Concerns

- ~~Legibility (will it look good?)~~ RESOLVED — Phase 1 legibility spike APPROVED by human in a real terminal; cube reads as a solid 3D form
- ~~01-01 interactive verification UNVERIFIED~~ RESOLVED — q/Esc restore, resize-no-garbage, panic restore all confirmed working in a real terminal during the 01-05 verify
- Layout algorithm (rack-grid vs network-floors) unresolved — decide in Phase 2 planning
- Volume size not exposed by Docker API — decide proxy metric in Phase 4 planning

## Session Continuity

Last session: 2026-05-27
Stopped at: 01-05 COMPLETE + post-verify rendering evolution. Dual render backend shipped (kitty real-pixels + braille fallback, auto-detected), quit-responsiveness bug fixed, flat-shading/fog/flicker fixes. All committed; build/clippy/test clean (36 tests, release builds offline).
Resume file: None
Resume note: Phase 1 COMPLETE & APPROVED. Terminal-3D legibility de-risked. KEY ARCHITECTURE: dual render backend auto-selected by terminal — kitty graphics protocol (real RGB pixels, smooth, `src/kitty.rs`) where supported, braille fallback (`src/render3d` + `src/ui/scene.rs`) elsewhere; Alacritty/SSH get braille (no graphics protocol). Run with `--release` for the kitty path. NEXT: Phase 2 — generalize one cube into MANY boxes. The kitty per-pixel z-buffer already handles arbitrary box overlap (braille path uses painter's sort, fine for convex boxes but watch inter-box ordering). Resolve the layout algorithm (rack-grid vs network-floors), designed network-aware up front. NOTE: roadmap deviation — the kitty backend front-runs part of Phase 5's ROB-02 capability/degrade path; reconcile during Phase 5 planning.
