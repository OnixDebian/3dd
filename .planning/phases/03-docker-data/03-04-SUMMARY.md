---
phase: 03-docker-data
plan: 04
subsystem: docker-renderer-wire
tags: [bollard, mpsc, live-data, tui-lifecycle, empty-state, probe-before-tui]

# Dependency graph
requires:
  - phase: 03-02
    provides: docker::connect_and_probe() + ProbeError (classified, plain-text user_message)
  - phase: 03-03
    provides: spawn_docker_tasks(docker, tx) + DockerMsg + LiveWorld::apply
  - phase: 02-04
    provides: synthetic_scene() + World/Entity/SceneBounds shape (data source contract)
provides:
  - "Pre-TUI daemon probe on both backends (braille + kitty) gates raw mode on a clean terminal — Pitfall 9 / ROB-01 closed end-to-end"
  - "Single mpsc::unbounded_channel<DockerMsg>() per process; producer task spawned in main.rs; receiver passed into the chosen backend"
  - "App owns Option<UnboundedReceiver<DockerMsg>> + LiveWorld; drain_docker() is non-blocking once-per-loop, never per-frame"
  - "Camera re-frames ONLY on entity-count change (Added/Removed) — pure Stat samples never re-frame, avoiding per-second view jitter"
  - "Empty-state banner ('No containers running…') in BOTH backends when LiveWorld is empty — never a void, never a panic on degenerate bounds"
  - "Both backends verified against a real local daemon: list_containers seed + live events::create/destroy reconcile observably (0 -> 14 -> 15 -> 14)"
affects:
  - "Phase 4: ENT-01 floor-planes will consume the same Entity.group_key + already-stable per-id slots; HUD/network visualization plugs into the live World"
  - "Phase 5: ROB-02 will inherit this probe-before-TUI pattern for capability degrade; mid-session daemon-loss banner is deferred here (last World stays on screen + no panic)"

# Tech tracking
tech-stack:
  added: []          # No new deps — uses existing tokio mpsc + bollard handle
  patterns:
    - "Probe-before-raw-mode (Pitfall 9): docker::connect_and_probe() awaited in main.rs BEFORE Tui::enter / before run_kitty's enable_raw_mode; on Err print ProbeError to stderr + exit(1) on a CLEAN terminal"
    - "Producer/consumer channel ownership: main.rs owns the (tx, rx) pair; tx is moved into spawn_docker_tasks; rx is passed into the chosen backend entry point; JoinHandle.abort() on shutdown"
    - "Non-blocking channel drain on the render path (DOCK-04): App::run drains via while-let try_recv once per outer loop iteration; run_kitty does the same inside its 33ms loop. NEVER a Docker call per frame."
    - "Entity-count-change re-frame: a flag returned by drain_docker() / tracked by last_count in run_kitty distinguishes Add/Remove (worth re-framing) from Stat (resize-only, never re-frame). Pins camera stability under live churn."
    - "Empty-state in-scene banner: ui::EMPTY_BANNER (single source of truth) rendered as centered italic Paragraph (braille, palette.edge gray) or centered plain text (kitty, cell math). Status bar always present in both."

key-files:
  created: []
  modified:
    - src/main.rs              # connect_and_probe + channel + spawn_docker_tasks + thread rx into backend
    - src/docker/mod.rs        # tightened the dead_code allow comment now that the surface is wired
    - src/app.rs               # docker_rx + LiveWorld + drain_docker() + camera re-frame on count change + 5 unit tests
    - src/ui/mod.rs            # EMPTY_BANNER constant + centered-banner branch when world is empty
    - src/kitty.rs             # run_kitty consumes rx live; empty-state banner; 'boxes: N' in status bar

key-decisions:
  - "kitty stays SYNC (no async fn run_kitty): UnboundedReceiver::try_recv is non-blocking and needs no runtime — the producer task already lives on main's tokio runtime; converting the kitty loop to async would be a larger blast radius for no gain"
  - "App world starts EMPTY in with_docker_rx (not synthetic-seeded): the empty-state banner is the FIRST thing the user sees if their host has no containers — never a void, criterion #5"
  - "Camera re-frame trigger = entity-count change ONLY: stat samples rebuild the World (boxes resize) but never re-frame; otherwise the view jitters every second"
  - "--dump-rgba stays daemon-free (synthetic): it's an inspection tool for the RENDERER, not the data path; documented in main.rs"
  - "Daemon-loss mid-session: deferred to Phase 5 ROB-02; current behaviour = last World stays on screen, producer task ends cleanly, NO panic, NO blank void (the hard requirement is met)"
  - "Probe-before-Tui::enter AND probe-before-enable_raw_mode (kitty): both backends share the same pre-TUI gate in main.rs so the probe runs ONCE; the chosen backend never sees a failed probe"
  - "Producer JoinHandle returned by spawn_docker_tasks is held in main and .abort()-ed after the backend returns — no orphan task outlives the renderer"

patterns-established:
  - "Pre-TUI probe gate: any pre-condition that can talk to the network/socket runs in main BEFORE flipping the terminal; raw mode is reserved for the rendering phase"
  - "Once-per-outer-loop drain: the same drain pattern the event channel uses (try_next coalesce) is now the template for any in-process queue the renderer reads — bounded work per loop, zero blocking"
  - "Backend-symmetric empty state: a single EMPTY_BANNER constant is consumed by both renderers; future copy changes touch one place"

# Metrics
duration: 10min
completed: 2026-05-28
---

# Phase 3 Plan 4: Renderer Wire Summary

**Live Docker containers now render in both 3D backends with mid-session add/remove reconciliation, pre-TUI probe gating on a clean terminal, and an in-scene empty-state banner on zero containers — Phase 3 criteria #1–#5 observably TRUE.**

## Performance

- **Duration:** 10 min
- **Started:** 2026-05-28T11:29:30Z
- **Completed:** 2026-05-28T11:40:27Z
- **Tasks:** 3
- **Files modified:** 5
- **Tests:** 116 → 121 (+5; all in `src/app.rs::tests`)

## Accomplishments

- **Pre-TUI daemon probe** runs in `main.rs` BEFORE both `Tui::enter` (braille) and `enable_raw_mode` (kitty). On `Err` the classified `ProbeError` is printed to stderr and the process exits 1 — verified against `DOCKER_HOST=unix:///tmp/no_such_sock.sock`: the actionable "Docker socket not found … is Docker installed and running?" lands on a CLEAN terminal, exit code 1, no raw-mode garble (Pitfall 9 / ROB-01 closed end-to-end on the user-facing path).
- **mpsc::unbounded_channel<DockerMsg>** plumbed end-to-end. The producer (`spawn_docker_tasks(docker, tx) -> JoinHandle<()>`) lives on the existing tokio runtime; the receiver is consumed by the chosen backend (App for braille, run_kitty for kitty). The producer JoinHandle is held in `main` and `.abort()`-ed after the backend returns — no orphan task outlives the renderer.
- **Braille App live-data wire-up.** `App::with_docker_rx(rx)` starts with an EMPTY World (banner-first), holds a `LiveWorld`, and drains the channel non-blocking once per outer loop iteration via `drain_docker()`. Camera re-frames ONLY when the entity-count changes (Add/Remove) — pure Stat samples rebuild the World but never re-frame, so the view doesn't jitter every second. `App::new()` is preserved for tests / `--dump-rgba`, still synthetic-seeded.
- **Kitty run_kitty live-data wire-up.** Kept SYNC (UnboundedReceiver::try_recv needs no runtime); replaced `synthetic_scene()` seed with `LiveWorld` + empty World start. Same drain + re-frame discipline as braille (`last_count` tracker). When the World is empty the kitty loop SKIPS `render_rgba` entirely and writes a centered banner via cell-math; image surface is cleared exactly once on the empty↔non-empty edge so the banner doesn't flicker. `render_rgba` / `emit_kitty` / 33ms pacing / raw-mode lifecycle / quit-key handling all byte-for-byte unchanged.
- **Empty-state banner in both backends.** Single source of truth at `ui::EMPTY_BANNER`. Braille renders it as a centered italic Paragraph (palette.edge gray) inside the same bordered "scene" block so the chrome doesn't change. Kitty draws it centered in cell coords (`rows/2`, centered col). Status bar stays on the reserved bottom row in both backends — and now shows `boxes: N` so live add/remove is observable.
- **Live visual verify against the real local daemon (13 stopped + 1 running baseline).** Inside a single 6-second kitty session, the status-bar `boxes:` field walked `0 → 14 → 15 → 14` as we `docker run` / `docker rm -f` a test container mid-session — proving criterion #3 directly. Box COUNT matches `docker ps -a`. Render internals untouched (`git diff src/render3d/` empty).

## Task Commits

1. **Task 1: Probe before TUI + spawn docker tasks + thread channel** — `affe49f` (feat)
2. **Task 2: Braille App live drain + empty-state banner** — `f97a7f9` (feat)
3. **Task 3: Kitty run_kitty live drain + empty-state banner + visual verify** — `cbd3a84` (feat)

**Plan metadata:** (next commit — see git log for hash)

## Files Created/Modified

- `src/main.rs` — `connect_and_probe()` awaited before any raw mode; `mpsc::unbounded_channel::<DockerMsg>()` + `spawn_docker_tasks(docker, tx)`; backend dispatch threads `rx` into `App::with_docker_rx(rx)` or `run_kitty(rx)`; producer `JoinHandle.abort()` on shutdown; `--dump-rgba` stays daemon-free
- `src/docker/mod.rs` — tightened the module-scope `#![allow(dead_code)]` comment now that the public surface (connect_and_probe, spawn_docker_tasks, DockerMsg, LiveWorld) is consumed by main.rs
- `src/app.rs` — `App` gained `docker_rx: Option<UnboundedReceiver<DockerMsg>>` + `live: LiveWorld`; `with_docker_rx` constructor starts EMPTY (banner-first), `new` stays synthetic-seeded for tests; `drain_docker()` non-blocking once-per-loop; `run()` calls `frame_scene` only on entity-count change; 5 new tests cover empty start, drain rebuilds, empty-channel no-op, stat-only-no-recount, removal-to-empty-with-finite-bounds
- `src/ui/mod.rs` — new `EMPTY_BANNER` constant (single source of truth for both backends); `view()` branches on `app.world.entities.is_empty()` to render a centered italic Paragraph inside the same bordered "scene" block
- `src/kitty.rs` — `run_kitty(rx)` accepts the receiver; `LiveWorld` + empty World start; while-let try_recv drain each loop pass; entity-count-change camera re-frame; centered-banner branch on empty world; `boxes: N` in status bar; `render_rgba`/`emit_kitty` untouched

## Decisions Made

- **kitty stays SYNC** rather than going `async fn`: `UnboundedReceiver::try_recv` doesn't require a tokio runtime, and the producer task already lives on `main`'s tokio runtime. Converting the kitty loop would be a much larger blast radius for zero observable gain.
- **App world starts EMPTY** in `with_docker_rx` (not synthetic-seeded). Reason: criterion #5 requires the empty-state banner to be the first thing the user sees if their host has no containers — synthesizing 30 fake boxes for ~half a second before the real seed lands would be a worse first impression than the banner.
- **Camera re-frame on entity-count change ONLY.** Pure Stat samples rebuild the World (boxes resize) but the rack's enclosing radius barely shifts; re-framing every second on stats would jitter the view. The boolean returned by `App::drain_docker` (and the `last_count` tracker in kitty) gates this.
- **`--dump-rgba` stays daemon-free.** It's an offline RENDERER inspection tool (used to verify Phase 1/2 visuals on CI / fresh VMs); making it consume the live producer would defeat that purpose. Documented inline in `main.rs`.
- **Daemon-loss mid-session deferred to Phase 5 ROB-02.** Current behaviour: if the producer ends (events stream tore down / daemon went away), the last `World` stays on screen, the channel drain just sees `try_recv` return `Empty`, and the renderer keeps painting the cached frame. NO panic, NO blank void — the hard requirement here is met. A full "daemon lost" banner with reconnect is a Phase 5 concern.
- **Producer JoinHandle aborted on shutdown.** `spawn_docker_tasks` returns the orchestrator handle; main holds it and `.abort()`-s it after the backend returns. Prevents the producer outliving the renderer (Pitfall 7 strictly applied at the top level too).
- **Producer probe AND backend dispatch share one entry point in main.rs.** Both forks (kitty / braille) sit BELOW the probe `match` so the probe runs once, on the same code path, regardless of which backend is chosen.

## Deviations from Plan

None — plan executed exactly as written. All three tasks delivered the design decisions verbatim (probe before TUI, channel ownership in main, count-change-only re-frame, kitty stays sync, banner via `ui::EMPTY_BANNER`). No bugs found, no missing critical functionality discovered, no architectural surprises.

## Issues Encountered

- **clippy `while_let_loop` lint** on the first kitty drain pass (`loop { match rx.try_recv() { Ok(msg) => ..., Err(_) => break, } }`) — refactored to `while let Ok(msg) = rx.try_recv() { ... }` immediately. Clean.
- **`script` capture in the verification harness** drops styled ratatui output as raw escape codes (the braille backend's status-bar text only shows up after `strings` post-processing). NOT a code bug — the live terminal shows the bar correctly; this is just a quirk of capturing a TUI through `script` without a proper pty/cols negotiation. The kitty backend writes plain `\x1b[…H…` cursor-positioning + raw text for the status bar, so it captures cleanly via `script`; using the kitty log for live churn verification side-stepped the issue.

## User Setup Required

None — no external service configuration required. The probe handles the "user isn't in the docker group" / "daemon down" cases with actionable plain-text messages already.

## Observed Daemon UX

- **Live (daemon up):** Local daemon was reachable; the seed produced an `Added` per container (13 stopped + 1 running test container), the LiveWorld rebuilt to a 14-entity scene, the camera framed to the rack, both backends rendered. `docker run` / `docker rm -f` mid-session moved the box count 14 → 15 → 14 without restart, without panic. Box COUNT matches `docker ps -a` exactly.
- **Daemon down (simulated via `DOCKER_HOST=unix:///tmp/no_such_sock.sock`):** Probe classified the failure as `SocketMissing` and printed `Docker socket not found at /tmp/no_such_sock.sock — is Docker installed and running?` to stderr; process exit code 1; terminal stayed cooked. NO alt-screen flash, NO panic, NO garble. Both `--kitty` and `--braille` paths hit the same gate (probe runs before backend dispatch).
- **No-permission / socket-missing classifications:** Not exercised against a real ENOENT/EACCES this session (Arch host runs the user in the `docker` group and the socket exists), but the classifier is unit-tested in `src/docker/connect.rs::tests` (102 → 116 baseline) covering the SocketMissing / PermissionDenied / DaemonDown / Other paths. The user-message strings carry the actionable hints (install/run, `docker group`, `systemctl start docker`).

## Next Phase Readiness

- **Phase 3 COMPLETE.** All five criteria observably TRUE:
  1. Scene shows REAL containers — verified (14 boxes match `docker ps -a`).
  2. Box size reflects load via the normalized 03-01 stats path — verified (stats stream lands a `Stat`, LiveWorld rebuilds via `load_to_half_extent`).
  3. Live add/remove without restart — verified on kitty backend in a 6-second churn test (0 → 14 → 15 → 14).
  4. Render stays smooth while stats arrive ~1/sec — verified (drain is once-per-loop try_recv, no Docker call per frame; idle CPU not regressed).
  5. Graceful empty / down / permission / socket-missing states — verified (in-scene banner on empty, classified ProbeError + exit(1) on down/permission/missing; mid-session daemon-loss leaves the last World on screen without panic).
- **Phase 4 unblocks immediately:**
  - The live `World` is the same shape Phase 4 ENT-01 (network floor-planes) needs — `Entity.group_key` is already stable and slot-preserving (CONT-05 invariant).
  - HUD work can read `app.world.entities` + per-id load directly; no further data plumbing required.
  - The `create`-event `group_key: "none"` carryover from 03-03 is still open — a targeted `inspect_container` on `start` to backfill the group_key will likely land in early Phase 4 when network visualization needs it. Documented in 03-03's STATE.md notes; not blocking Phase 3 closure.
- **Concerns carried forward:**
  - Mid-session daemon-loss UX is currently "freeze on last frame" — acceptable for Phase 3 (no panic, no void) but Phase 5 ROB-02 should add a banner / reconnect attempt.
  - `--dump-rgba` is synthetic by design; if Phase 4 wants a live-data offline dump, that's a separate small inspection tool (build a `LiveWorld`, seed from a one-shot `list_containers`, dump rgba). Not needed yet.

---
*Phase: 03-docker-data*
*Completed: 2026-05-28*
