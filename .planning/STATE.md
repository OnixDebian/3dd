# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-05-26)

**Core value:** A beautiful, legible 3D scene that lets you grasp the state of your Docker environment at a glance — what's alive, what's hot, what's connected to what.
**Current focus:** Phase 3 (Docker Data Layer) — Wave 1 COMPLETE (03-01 + 03-02); next: Wave 2 (03-03 streams)

## Current Position

Phase: 3 of 5 (Docker Data Layer) — IN PROGRESS
Plan: 2 of 4 complete (03-01 stats normalizer + 03-02 connect/domain)
Status: Wave 1 COMPLETE; bollard 0.21 wired, pre-TUI daemon probe + ContainerSnapshot domain boundary in place; 102/102 tests pass, clippy clean
Last activity: 2026-05-28 — Completed 03-02 (bollard + connect_and_probe + ContainerSnapshot); ROB-01 / Pitfall 9 foundation in place ready for 03-04 to wire BEFORE Tui::enter

## Phase 2 Notes (for Phase 3 planning)

- **Architecture:** App owns a single `World` (src/world/: entity/layout/scene/mod). `synthetic_scene()` → 30 boxes / 3 network-groups, deterministic. Both renderers consume `&[Entity]` + `SceneBounds`; `Camera::frame_scene(&world)` binary-searches distance to projected-AABB fill (FRAME_TARGET_FILL=0.92, binding = horizontal axis at cell_aspect 2). When real Docker data lands (Phase 3), it replaces synthetic_scene() output — keep the same World/Entity shape.
- **Occlusion:** braille = single cross-box painter's sort (face-centroid); kitty = per-pixel z-buffer. Boxes never physically intersect (SLOT_SPACING 2.6), so painter's sort is safe for convex boxes.
- **DEVIATION / RECONCILE (CAM-01):** roadmap criterion #4 was "autopilot ORBIT camera". Human overrode it live → shipped PER-BOX self-spin (each box rotates about its own Y at SPIN_RATE 0.525) + STATIC scene-framed camera (YAW_RATE=0). Motion is framerate-independent (on logic tick). REVISIT in Phase 4 when manual explore (CAM-02/03) lands — decide whether orbit returns or per-box-spin stays.
- **Roadmap deviation carried from Phase 1:** kitty backend front-runs part of Phase 5 ROB-02 (capability/degrade). Still open.

Progress: ███████░░░ ~70% (Phase 1: 5/5, Phase 2: 4/4, Phase 3: 2/4; Phase 4+5 plans not yet drafted)

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
- 03-02: bollard 0.21 wired (`Cargo.toml`), resolves with tokio 1.47, no conflict. bollard 0.21 type/module paths PINNED in 03-02-SUMMARY for 03-03: `bollard::Docker`, `Docker::connect_with_local_defaults` (sync), `Docker::version`/`ping` (async, in `bollard::system`), `bollard::models::ContainerStatsResponse` + `ContainerSummary` + `ContainerState` (re-exports of `bollard_stubs::models`), `bollard::query_parameters::StatsOptions[Builder]` (re-export of stubs), `Docker::stats(name, Option<StatsOptions>) -> impl Stream<Item = Result<ContainerStatsResponse, Error>>`.
- 03-02: `src/docker/domain.rs` (NEW) is the bollard isolation boundary (ARCHITECTURE Anti-Pattern 4). `ContainerSnapshot { id, name, status: theme::Status, group_key }` — bollard-free downstream shape. `map_status(state: &str, oom: Option<bool>, exit_code: Option<i64>) -> theme::Status` total mapping (Docker `dead`/exited-OOM/exited-nonzero → Crashed; exited(0)/created/removing/stopping/unknown → Stopped safe default; running/paused/restarting → direct). `from_bollard_summary(&bollard::models::ContainerSummary) -> ContainerSnapshot` is the SOLE function importing bollard container types (strips Docker's leading '/' on names; primary network = sorted-keys-first, "none" default).
- 03-02: `src/docker/connect.rs` (NEW) — `connect_and_probe()` async, returns `Result<Docker, ProbeError>`. Builds via `connect_with_local_defaults`, probes with `version()` (full handshake/parse, single request). `ProbeError` 4 variants: SocketMissing (path + install hint) / PermissionDenied (path + "docker group" hint) / DaemonDown (path + "systemctl start docker" hint + raw detail) / Other (raw detail). Classifier matches bollard `SocketNotFoundError`/`IOError` directly + walks `Error::source()` chain looking for inner `io::Error` (handles bollard 0.21's hyper/hyper-util legacy wrapping); unknown io kinds and unclassified bollard errors fall back to DaemonDown with raw detail — NEVER panics, ALWAYS yields a non-empty actionable message (ROB-01 invariant pinned by test). `connect_and_probe` performs ZERO terminal I/O — caller (03-04) prints + exits on `Err` BEFORE `Tui::enter` (Pitfall 9).
- 03-02: `mod.rs` uncomments `pub mod domain;` + `pub mod connect;` (the marked lines 03-01 left); re-exports `ContainerSnapshot/map_status/from_bollard_summary/connect_and_probe/ProbeError` on `crate::docker`. `mod docker;` is in main.rs (03-01's wiring); 03-02 did not touch main.rs.
- 03-02: 25 new tests (11 domain + 14 connect); 77 → 102 tests. `cargo clippy --all-targets -- -D warnings` clean. `bollard::*` imports CONFINED to `src/docker/connect.rs` (`use bollard::Docker;` + `use bollard::errors::Error as B;` in classifier) and the fully-qualified `bollard::models::ContainerSummary` in the single `from_bollard_summary` fn in `src/docker/domain.rs`.

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

- Phase 3 Wave 2 (next): 03-03 (`docker::streams`) — uncomments `pub mod streams;`; subscribes `Docker::events()` for create/destroy/die and spawns per-running-container `Docker::stats(id, Some(StatsOptions { stream: true }))` streams; maps bollard's `ContainerStatsResponse` onto `RawCpu`/`RawMem` and calls `crate::docker::normalize` per container; uses `crate::docker::from_bollard_summary` to convert `list_containers` output into `ContainerSnapshot`; picks cgroup v1 `cache` vs v2 `inactive_file` per daemon. Reuses the `Docker` handle returned by `connect_and_probe()` (no reconnect).
- Phase 3 Wave 3: 03-04 (renderer wire) — call `connect_and_probe()` in `main.rs::main` BEFORE `Tui::enter` (and before `run_kitty`); on `Err(e)` print `e.user_message()` (or `{e}`) to stderr and `std::process::exit(1)`. Replace `world::synthetic_scene()` output with live `Vec<ContainerSnapshot>` -> `Vec<Entity>` using the same `World/Entity` shape; `StatSample::load` feeds `world::entity::load_to_half_extent` directly; `ContainerSnapshot.group_key` becomes the layout's group axis (replacing the synthetic `id / GROUP_SIZE` derivation).
- For 03-03 consideration: `from_bollard_summary` doesn't carry OOM/exit-code (ContainerSummary doesn't expose them), so `exited` from `list_containers` surfaces as `Stopped` not `Crashed`. Either (a) inspect each exited container for the precise OOM/exit signal, or (b) live with `Stopped` and let stream-fed crash events from `events()` upgrade the status. Documented in `from_bollard_summary` doc comment.
- For Phase 4 planning: `ContainerSnapshot.group_key` is the canonical layout group axis. Decide whether `synthetic_scene` continues to invent group keys or feeds through the same `Vec<ContainerSnapshot>` shape.
- Pre-existing fmt drift on the pre-phase-3 files (`src/world/scene.rs`, `src/app.rs`, etc.) — NOT touched by this plan; pick up in a separate `chore(fmt)` whenever.

### Blockers/Concerns

- ~~Legibility (will it look good?)~~ RESOLVED — Phase 1 legibility spike APPROVED by human in a real terminal; cube reads as a solid 3D form
- ~~01-01 interactive verification UNVERIFIED~~ RESOLVED — q/Esc restore, resize-no-garbage, panic restore all confirmed working in a real terminal during the 01-05 verify
- ~~Layout algorithm (rack-grid vs network-floors) unresolved~~ RESOLVED in 02-01 — network-grouped rack grid (groups as Z-bands)
- Inter-box draw ordering: braille path uses painter's sort (fine for convex boxes, watch overlapping boxes across groups); kitty path's per-pixel z-buffer already handles arbitrary overlap
- Volume size not exposed by Docker API — decide proxy metric in Phase 4 planning

## Session Continuity

Last session: 2026-05-28
Stopped at: 03-02 COMPLETE (Wave 1 of Phase 3 DONE — both 03-01 and 03-02 landed). bollard 0.21 wired; daemon connect + classified probe + bollard-free ContainerSnapshot domain boundary in place. cargo build/test/clippy all clean; 102/102 tests pass.
Resume file: None
Resume note: Wave 1 of Phase 3 COMPLETE. The Phase-3 data-layer foundation is in place: (1) the CPU%-delta gotcha (Pitfall 1) is CONTAINED in `crate::docker::normalize` (03-01); (2) bollard is connected via `crate::docker::connect_and_probe()` returning `Result<Docker, ProbeError>` (03-02) — the `Docker` handle is reused throughout, no reconnect; (3) the bollard isolation boundary (Anti-Pattern 4) is enforced — bollard imports are CONFINED to `src/docker/connect.rs` and the single `from_bollard_summary` fn in `src/docker/domain.rs`. ROB-01 / Pitfall 9 is satisfied: `ProbeError` has 4 actionable variants (SocketMissing / PermissionDenied / DaemonDown / Other), each Display message names the socket path and includes a remediation hint (install/run, `docker` group, `systemctl start docker`), and `connect_and_probe` performs ZERO terminal I/O so it's safe to call BEFORE `Tui::enter`. NEXT (Wave 2): 03-03 (streams) — subscribe `events()` + spawn per-running-container `stats()` streams, map `ContainerStatsResponse` -> `RawCpu`/`RawMem` and call `normalize`, use `from_bollard_summary` to map `list_containers` -> `ContainerSnapshot`. Reuses the `Docker` handle from `connect_and_probe`. THEN Wave 3: 03-04 (renderer wire) — call `connect_and_probe` in `main.rs` BEFORE `Tui::enter`, exit on Err with `e.user_message()` to stderr; replace `synthetic_scene()` with live `Vec<ContainerSnapshot>` and feed `StatSample::load` into `world::entity::load_to_half_extent`. bollard 0.21 type paths are PINNED in 03-02-SUMMARY.md so 03-03 doesn't re-discover them. Race-window note on Wave 1 parallelism: `src/docker/mod.rs` saw two non-conflicting one-line edits (one from each plan, both inside the marked commented-stub region) — the contract held. All Phase-1/Phase-2 invariants intact: 102/102 tests, clippy clean on `--all-targets -- -D warnings`, no bollard import outside `src/docker/`.
