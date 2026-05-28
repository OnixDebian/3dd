---
phase: 03-docker-data
verified: 2026-05-28T00:00:00Z
status: passed
score: 5/5 must_haves verified
requirements_covered: [DOCK-01, DOCK-02, DOCK-03, DOCK-04, ROB-01]
---

# Phase 3: Docker Data Layer Verification Report

**Phase Goal:** Feed real Docker data into the proven renderer — list/inspect containers, stream correct live stats, and react to create/destroy events, with graceful failure states.

**Verified:** 2026-05-28
**Status:** passed
**Score:** 5/5 must-haves verified
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|---|---|---|
| 1 | Scene shows real local containers via bollard | passed | seed `list_containers(all=true)` at `src/docker/streams.rs:90-92` → `from_bollard_summary` → `DockerMsg::Added` → `LiveWorld::handle_added` (`src/world/live.rs:167-201`). UAT in 03-04-SUMMARY:134 confirms 14 boxes matched `docker ps -a`. |
| 2 | CPU%/memory correct with NaN/empty guards | passed | Formula `(cpu_delta/system_delta) * online_cpus * 100` at `src/docker/stats.rs:151`. Warming-up detection at `src/docker/stats.rs:128-131` (None prev OR both-zero counters). Guards: `system_delta <= 0.0`, `cpu_delta < 0.0`, `online_cpus == 0`, percpu_len fallback at `src/docker/stats.rs:119-122`, finite-scrub at `src/docker/stats.rs:175-181`, ceiling clamp at `src/docker/stats.rs:152-153`. Pinned by 11 stats tests including `never_nan_inf` adversarial sweep at `src/docker/stats.rs:463-555`. |
| 3 | Create/destroy adds/removes box without restart | passed | `events()` subscription at `src/docker/streams.rs:149`. Actions wired at `src/docker/streams.rs:182-244`: create/start → Added + spawn stats; die/destroy → abort stats + Removed. Reconciler maintains stable per-container slots (CONT-05) at `src/world/live.rs:167-214`: `handle_removed` only nulls the slot (`group_slots[g][index] = None`) without shifting neighbors. UAT in 03-04-SUMMARY confirmed 0→14→15→14 reconciliation via `docker run/rm` mid-session. |
| 4 | Stat rate decoupled from render rate | passed | Braille app drains via `try_recv` once per outer loop at `src/app.rs:165-178` (`drain_docker`). Kitty backend drains via `try_recv` once per 33ms frame at `src/kitty.rs:413-418`. Unbounded mpsc channel; producer cadence (~1Hz per container stats) and consumer cadence (33ms render) are fully independent. Pinned by `drain_docker_stat_only_returns_false_no_recount` at `src/app.rs:303-328`. |
| 5 | Graceful daemon-down/empty/permission states | passed | Pre-TUI probe at `src/main.rs:62-68` runs BEFORE `tui.enter()` (line 85) AND BEFORE `run_kitty` (line 82). On `ProbeError`, classified message → stderr → `exit(1)`, never touches raw mode. Empty-state banner at `src/ui/mod.rs:22,39-58,69-104` (braille) and `src/kitty.rs:453-471` (kitty); both backends share the `EMPTY_BANNER` const. UAT in 03-04-SUMMARY:128 simulated `DOCKER_HOST=unix:///tmp/no_such_sock.sock` → "Docker socket not found … is Docker installed and running?" on clean terminal, exit 1, no garble. |

**Score:** 5/5 truths verified

### Required Artifacts

| Artifact | Expected | Status | Details |
|---|---|---|---|
| `src/docker/stats.rs` | PURE normalizer, CPU%-delta formula + guards | passed | 557 lines, 11 tests including adversarial `never_nan_inf` sweep; formula at L151; warming_up at L128-131; finite-scrub at L175-181 |
| `src/docker/connect.rs` | `connect_and_probe` + `ProbeError` classification | passed | 441 lines, 14 tests; SocketMissing/PermissionDenied/DaemonDown/Other; `effective_socket_path` honors DOCKER_HOST |
| `src/docker/domain.rs` | bollard-free `ContainerSnapshot` + `map_status` | passed | 258 lines, 11 tests; dead/exited(nonzero)/OOM → Crashed; safe Stopped default; `from_bollard_summary` is the SOLE bollard-container importer outside `connect`/`streams` |
| `src/docker/streams.rs` | producer task: events + per-container stats → mpsc | passed | 555 lines, 5 tests; seed + events loop + per-id stats; `abort_stats_task` on die/destroy; `spawn_stats_for_id_if_absent` (idempotent on start/unpause); `Stopped` event-typed fallback never panics |
| `src/world/live.rs` | `LiveWorld` reconciler with stable slots (CONT-05) | passed | 589 lines, 9 tests; per-group slot vectors at L107-108; `removed_frees_slot_others_stable`, `id_keeps_slot_across_churn`, `reused_hole_keeps_rack_compact`, `warming_up_does_not_snap`, `empty_set_is_safe`, `unknown_id_messages_are_noop`, `distinct_groups_get_distinct_z_bands` |
| `src/main.rs` | probe BEFORE Tui::enter / enable_raw_mode | passed | L62 (probe) precedes L82 (run_kitty) and L85 (tui.enter) |
| `src/app.rs` | drains DockerMsg via try_recv each loop | passed | `drain_docker` at L165-178; re-frames camera only on count change (L216-218) |
| `src/kitty.rs::run_kitty` | drains DockerMsg via try_recv each 33ms frame | passed | L413-418 drain; L419-427 count-change re-frame; L453-471 empty-banner branch |
| `src/ui/mod.rs::EMPTY_BANNER` | shared empty-state banner constant | passed | L22 const + `render_empty_banner` at L69-104 (braille); kitty re-uses const at L464 |

### Key Link Verification

| From | To | Via | Status | Details |
|---|---|---|---|---|
| `main.rs` | `connect_and_probe` | `match` exit on Err | wired | `main.rs:62-68` |
| `main.rs` | `spawn_docker_tasks` | `tokio::spawn` with mpsc tx | wired | `main.rs:73-74` |
| `main.rs` (kitty) | `run_kitty(rx)` | mpsc rx | wired | `main.rs:82` |
| `main.rs` (braille) | `App::with_docker_rx(rx)` | mpsc rx | wired | `main.rs:86` |
| `streams::run_events_loop` | `LiveWorld` | `DockerMsg` enum (bollard-free) | wired | enum at `world/live.rs:62-74` |
| `streams::spawn_stats_task` | `normalize` | `sample_from_response` mapping | wired | `streams.rs:356-367` |
| `app::drain_docker` | `LiveWorld::apply` | `try_recv` loop | wired | `app.rs:171-176` |
| `kitty::run_kitty` | `LiveWorld::apply` | `try_recv` loop | wired | `kitty.rs:413-418` |
| `LiveWorld::build_world` | renderer | shares `World{entities, bounds}` shape; reuses `layout()` + `load_to_half_extent()` verbatim | wired | `live.rs:274-306` |
| `ui::view` | `EMPTY_BANNER` | empty-entities branch | wired | `ui/mod.rs:39-58` |
| `kitty::run_kitty` | `EMPTY_BANNER` | empty-entities branch | wired | `kitty.rs:453-471` |

### Requirements Coverage

| Requirement | Status | Evidence |
|---|---|---|
| DOCK-01 (real container list) | satisfied | `list_containers(all=true)` seed + events reconciliation |
| DOCK-02 (correct CPU%/mem) | satisfied | guarded delta formula in `stats.rs` |
| DOCK-03 (event-driven add/remove) | satisfied | `events()` loop + per-id stats abort |
| DOCK-04 (rate decoupling) | satisfied | unbounded mpsc + `try_recv` drain in both backends |
| ROB-01 (graceful failure) | satisfied | pre-TUI probe + classified ProbeError + empty banner + safe Stopped default |
| CONT-05 (anti-teleport, strengthened) | reinforced | per-group slot vectors at `live.rs:107-108`, three dedicated regression tests |

### Anti-Patterns Found

None blocking. Bollard isolation holds:
- `src/docker/connect.rs:56,186` — expected (the bollard boundary)
- `src/docker/streams.rs:56,57,60,372,399,420` — expected (the bollard mapping layer)
- `src/docker/domain.rs:120,140-152` — expected (the SOLE container-type mapper)
- ZERO `use bollard` outside `src/docker/` (Anti-Pattern 4 isolation confirmed)

`#![allow(dead_code)]` / `#![allow(unused_imports)]` at module level in `docker/mod.rs` and submodules is intentional (documented in `docker/mod.rs:25-29`).

### Build & Test

- `cargo test --quiet`: **121 passed; 0 failed; 0 ignored** (matches expected count from plan brief)
- `cargo build --release`: clean, no warnings
- `cargo clippy --all-targets -- -D warnings`: clean, no warnings

### Renderer Isolation

`git log --since="2026-05-27" -- src/render3d/` returned NO commits from Phase 3 — only pre-Phase-3 commits `c6810a4` and `2831e77` (both 02-04). The proven renderer was preserved byte-for-byte; only its data source flipped from `synthetic_scene()` to the live `LiveWorld`. Anti-Pattern 1 (re-litigating the renderer) avoided.

### Human Verification Required

None. 03-04 ran live UAT against a real daemon (14 boxes verified, 0→14→15→14 reconciliation observed via `docker run`/`docker rm` mid-session, `DOCKER_HOST=unix:///tmp/no_such_sock.sock` exercised the failure banner). The remaining criteria (#2 formula correctness, #4 drain pattern) are pinned by automated tests. The code wiring matches the UAT-reported behavior (probe before raw-mode, try_recv drain, stable slots, count-change-only re-framing).

### Notable Deviations (verified, not regressions)

- 03-03 per-group slot vectors (instead of a flat shared slot vector) STRENGTHEN CONT-05: a container in network A can never displace a container in network B when freed. Confirmed at `src/world/live.rs:107-108` and pinned by `distinct_groups_get_distinct_z_bands` at `live.rs:572-587`.

### Gaps Summary

None. All five success criteria from the ROADMAP are observable in the code, exercised by 121 passing tests, and corroborated by the 03-04 UAT against a real Docker daemon.

---

*Verified: 2026-05-28*
*Verifier: Claude (gsd-verifier)*
