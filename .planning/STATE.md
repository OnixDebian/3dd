# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-05-26)

**Core value:** A beautiful, legible 3D scene that lets you grasp the state of your Docker environment at a glance — what's alive, what's hot, what's connected to what.
**Current focus:** Phase 3 (Docker Data Layer) — Wave 2 COMPLETE (03-03 streams + LiveWorld); next: Wave 3 (03-04 renderer wire)

## Current Position

Phase: 3 of 5 (Docker Data Layer) — IN PROGRESS
Plan: 3 of 4 complete (03-01 + 03-02 + 03-03)
Status: Wave 2 COMPLETE; producer/consumer split (DockerMsg + spawn_docker_tasks + LiveWorld) in place; 116/116 tests pass, clippy clean
Last activity: 2026-05-28 — Completed 03-03 (DockerMsg + LiveWorld reconciliation + spawn_docker_tasks events/stats producers); decoupled cadence (DOCK-04) + stable per-id slots (CONT-05) pinned

## Phase 2 Notes (for Phase 3 planning)

- **Architecture:** App owns a single `World` (src/world/: entity/layout/scene/mod). `synthetic_scene()` → 30 boxes / 3 network-groups, deterministic. Both renderers consume `&[Entity]` + `SceneBounds`; `Camera::frame_scene(&world)` binary-searches distance to projected-AABB fill (FRAME_TARGET_FILL=0.92, binding = horizontal axis at cell_aspect 2). When real Docker data lands (Phase 3), it replaces synthetic_scene() output — keep the same World/Entity shape.
- **Occlusion:** braille = single cross-box painter's sort (face-centroid); kitty = per-pixel z-buffer. Boxes never physically intersect (SLOT_SPACING 2.6), so painter's sort is safe for convex boxes.
- **DEVIATION / RECONCILE (CAM-01):** roadmap criterion #4 was "autopilot ORBIT camera". Human overrode it live → shipped PER-BOX self-spin (each box rotates about its own Y at SPIN_RATE 0.525) + STATIC scene-framed camera (YAW_RATE=0). Motion is framerate-independent (on logic tick). REVISIT in Phase 4 when manual explore (CAM-02/03) lands — decide whether orbit returns or per-box-spin stays.
- **Roadmap deviation carried from Phase 1:** kitty backend front-runs part of Phase 5 ROB-02 (capability/degrade). Still open.

Progress: ████████░░ ~80% (Phase 1: 5/5, Phase 2: 4/4, Phase 3: 3/4; Phase 4+5 plans not yet drafted)

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
- 03-03: PRODUCER/CONSUMER split via `DockerMsg` enum (Added / Removed / StatusChanged(id, theme::Status) / Stat(id, StatSample)) — bollard-free message bus over `tokio::sync::mpsc::UnboundedSender`. `src/world/live.rs` (NEW) `LiveWorld::apply(msg) -> Option<World>` reconciles into the EXISTING `World { entities, bounds }` shape; reuses `layout()` + `load_to_half_extent()` + `SceneBounds::from_entities` verbatim (no signature changes to entity/layout/scene).
- 03-03: STABLE PER-CONTAINER SLOT (CONT-05 anti-teleport) via PER-GROUP slot vectors. `groups: HashMap<String, u16>` insertion-ordered; `group_slots: Vec<Vec<Option<String>>>` indexed by group id; `id_to_addr: HashMap<String, (u16, usize)>` reverse index. On `Removed` only the SAME-group slot is nulled — different-group containers cannot consume same-group holes, so same-group neighbours never shift. The first pass tried a global slot space and the anti-teleport test caught the bug; per-group structure replaced it (auto-fix Rule 1).
- 03-03: `Entity.id = (group_id as u32) * (1<<16) + index_in_group` — stable per id, unique while alive, fits u32, no hashing. Documented in `src/world/live.rs` module header; tests use the `eid(group, idx)` helper.
- 03-03: WARMING-UP samples are no-ops in LiveWorld (don't overwrite a real previous load with the zero from a fresh warming-up frame); load==same is also a no-op (no World rebuild churn). Unknown-id messages (race-window between events and stats) are silent no-ops, never panics.
- 03-03: `src/docker/streams.rs` (NEW) `spawn_docker_tasks(docker, tx) -> JoinHandle<()>` orchestrator: (1) `list_containers(all=true)` seed via `from_bollard_summary` -> `Added` + spawn stats task per Running; (2) `events()` loop reconciles via container actions (create/start/pause/unpause/restart/die/oom/destroy); (3) per-container `stats(id, stream:true)` task feeds bollard `ContainerStatsResponse` -> `sample_from_response()` -> 03-01's `normalize()` -> `Stat`. NEVER `.unwrap()`/`.expect()` a stream item; stream end/error ends task cleanly (events::die/destroy handles entity removal).
- 03-03: CANCELLATION via `HashMap<String, JoinHandle<()>>` owned by the orchestrator task (no Mutex — handle never escapes). die/destroy `.abort()`s the right stats task; final cleanup drains the map. `tokio-util::CancellationToken` REJECTED for this — JoinHandle::abort is sufficient and saves a dep. Idempotent `spawn_stats_for_id_if_absent` checks `JoinHandle::is_finished()` so duplicate start/unpause events don't pile up duplicate streams.
- 03-03: `sample_from_response(&ContainerStatsResponse) -> StatSample` factored as a pure helper (no daemon needed for unit tests). cgroup v2 `inactive_file` -> cgroup v1 `cache` -> 0 picker per-sample. `online_cpus: Option<u32>` widened to u64 to match `RawCpu`.
- 03-03: `src/docker/mod.rs` uncommented `pub mod streams;` (the marked line 03-01 left) + `pub use streams::spawn_docker_tasks;`. `src/world/mod.rs` added `pub mod live;` + `pub use live::{DockerMsg, LiveWorld};`.
- 03-03: 14 new tests (9 live + 5 streams); 102 → 116 tests. `cargo clippy --all-targets -- -D warnings` clean. `bollard::*` imports STILL CONFINED to `connect.rs` + `domain.rs` (single fn) + new `streams.rs` (the mapping helpers `raw_cpu_from` / `raw_mem_from` + the bollard event/stats types in the orchestrator).
- For 03-03 (KNOWN, for 03-04 to consider): `create` event seeds `group_key: "none"` because bollard `EventMessage.actor.attributes` doesn't carry network attachments — Phase 4 may add a targeted `inspect_container` on `start` to backfill. The `events()` stream subscribes with NO filter (client-side filtering on `EventMessageTypeEnum::CONTAINER`); Phase 4 can switch to a server-side `filters({"type": ["container"]})` if wire chatter becomes a problem. `spawn_docker_tasks` returns a `JoinHandle<()>` 03-04 should `.abort()` on shutdown.

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

- Phase 3 Wave 3 (next): 03-04 (renderer wire) — call `connect_and_probe()` in `main.rs::main` BEFORE `Tui::enter` (and before `run_kitty`); on `Err(e)` print `e.user_message()` to stderr + `std::process::exit(1)`. Create `tokio::sync::mpsc::unbounded_channel::<DockerMsg>()`; pass tx to `spawn_docker_tasks(docker, tx)`. Each render tick: drain `rx.try_recv()` -> `live_world.apply(msg) -> Option<World>`; on `Some(w)` swap `app.world = w`. Replace `synthetic_scene()` call with `LiveWorld::new()` -> empty `World` starting state. Abort the orchestrator `JoinHandle` on shutdown.
- For 03-04 consideration (carried over from 03-02): `from_bollard_summary` doesn't carry OOM/exit-code, so list_containers `exited` surfaces as `Stopped`. 03-03's events::oom upgrades to Crashed correctly for new transitions; the only gap is containers that were already exited-with-OOM when the app started.
- For 03-04 consideration (NEW from 03-03): `create` events seed `group_key: "none"` (bollard EventMessage actor attributes don't carry network attachments). A targeted `inspect_container` on `start` could backfill the group_key — defer to Phase 4 when network visualization needs it.
- For Phase 4 planning: `ContainerSnapshot.group_key` is the canonical layout group axis (used by LiveWorld now). The per-group slot scheme will need a "move container to new group" path when a container's network attachment changes mid-life — handle_added currently refreshes the snapshot but does NOT migrate the slot.
- For Phase 4 planning: `events()` subscribes without filters; client-side filtering on `EventMessageTypeEnum::CONTAINER`. Switch to server-side `filters({"type": ["container"]})` if wire chatter becomes a problem.
- Pre-existing fmt drift on the pre-phase-3 files (`src/world/scene.rs`, `src/app.rs`, etc.) — NOT touched by this plan; pick up in a separate `chore(fmt)` whenever.

### Blockers/Concerns

- ~~Legibility (will it look good?)~~ RESOLVED — Phase 1 legibility spike APPROVED by human in a real terminal; cube reads as a solid 3D form
- ~~01-01 interactive verification UNVERIFIED~~ RESOLVED — q/Esc restore, resize-no-garbage, panic restore all confirmed working in a real terminal during the 01-05 verify
- ~~Layout algorithm (rack-grid vs network-floors) unresolved~~ RESOLVED in 02-01 — network-grouped rack grid (groups as Z-bands)
- Inter-box draw ordering: braille path uses painter's sort (fine for convex boxes, watch overlapping boxes across groups); kitty path's per-pixel z-buffer already handles arbitrary overlap
- Volume size not exposed by Docker API — decide proxy metric in Phase 4 planning

## Session Continuity

Last session: 2026-05-28
Stopped at: 03-03 COMPLETE (Wave 2 of Phase 3 DONE — DockerMsg + LiveWorld reconciliation + spawn_docker_tasks events/stats producers). cargo build/test/clippy all clean; 116/116 tests pass.
Resume file: None
Resume note: Wave 2 of Phase 3 COMPLETE. The producer/consumer contract is fully in place: (1) `crate::world::DockerMsg` is the bollard-free message bus (Added / Removed / StatusChanged / Stat); (2) `crate::world::LiveWorld::apply(msg) -> Option<World>` reconciles into the EXISTING `World { entities, bounds }` shape with stable per-id slots (CONT-05 pinned by 9 unit tests); (3) `crate::docker::spawn_docker_tasks(docker, tx) -> JoinHandle<()>` owns the list_containers seed + events() reconcile loop + per-container stats(stream:true) streams, normalizing each sample via 03-01's `normalize()` before emitting `Stat`. Stats samples are mapped via `sample_from_response` (factored for unit tests). Per-id `HashMap<String, JoinHandle<()>>` aborts the right stats task on die/destroy (Pitfall 7). ZERO `.unwrap()`/`.expect()` on Docker/stream results (only `.unwrap_or` safe defaults). NEXT (Wave 3): 03-04 (renderer wire) — `tokio::sync::mpsc::unbounded_channel::<DockerMsg>()` in main.rs; call `connect_and_probe` BEFORE `Tui::enter` (Pitfall 9 — exit cleanly on Err with `e.user_message()` to stderr); pass tx to `spawn_docker_tasks`; on each render tick drain `rx.try_recv()` -> `live_world.apply(msg)` and swap `app.world` when reconciliation produces a new World. Replace `synthetic_scene()` with `LiveWorld::new()` empty state. Anti-teleport invariant (CONT-05) is now structurally guaranteed by per-group slot vectors — different-group containers cannot consume same-group holes, so same-group neighbours never shift on removal. All Phase-1/Phase-2 invariants intact: 116/116 tests, clippy clean on `--all-targets -- -D warnings`, no bollard import outside `src/docker/`.
