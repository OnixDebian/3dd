---
phase: 03-docker-data
plan: 03
subsystem: docker
tags: [tokio, bollard, mpsc, streams, reconciliation, live-data]

# Dependency graph
requires:
  - phase: 03-docker-data
    provides: 03-01 StatSample + normalize (CPU%/mem delta + warming-up); 03-02 ContainerSnapshot/map_status/from_bollard_summary + connect_and_probe
  - phase: 02-scene-pipeline
    provides: World/Entity/SceneBounds + layout() + load_to_half_extent() — reused verbatim
provides:
  - DockerMsg enum (Added/Removed/StatusChanged/Stat) — bollard-free message bus
  - LiveWorld reconciliation engine (per-id stable slots, anti-teleport across churn)
  - spawn_docker_tasks(docker, tx) — events + per-container stats producers
  - sample_from_response — bollard ContainerStatsResponse -> normalized StatSample (Phase-4 reusable)
affects: [03-04-renderer-wire, Phase 4 networks/floor-planes]

# Tech tracking
tech-stack:
  added: []  # no new crates; uses existing tokio + futures + bollard 0.21
  patterns:
    - "Producer/consumer split: tokio tasks produce DockerMsg; renderer side drains via try_recv (decoupled cadence DOCK-04)"
    - "Per-group slot vectors (Vec<Vec<Option<String>>>) — anti-teleport invariant by construction"
    - "Stable Entity.id encoding: (group_id << 16 | index_in_group) fits u32, unique while alive"
    - "Stream-error == clean task end (no panic, no unwrap on stream items; events::die/destroy handles entity removal)"

key-files:
  created:
    - src/world/live.rs
    - src/docker/streams.rs
  modified:
    - src/world/mod.rs
    - src/docker/mod.rs

key-decisions:
  - "DockerMsg shape — Added/Removed/StatusChanged/Stat (small, bollard-free, Phase-4 extendable)"
  - "Per-group slot vectors over a single global slot space — different-group containers cannot consume same-group holes (no cross-group teleport)"
  - "Entity.id = (group_id << 16 | index_in_group) — stable & unique by construction, no hashing"
  - "JoinHandle::abort over CancellationToken — tokio-util not in deps; abort is sufficient for our fire-and-forget stats tasks"
  - "Single orchestrator task owns the per-id stats-task HashMap — no Mutex needed (map never escapes)"
  - "cgroup v2 `inactive_file` -> cgroup v1 `cache` -> 0 picker, per-sample (host type unknown at startup)"
  - "On any stream end/error: end task cleanly. Removed flows from events::die/destroy, not from stats death"

patterns-established:
  - "DockerMsg as the producer/consumer contract — Phase 4 extends this enum, never the channel type"
  - "LiveWorld is the SOLE place that calls layout() with a live group_id — synthetic_scene unchanged, both renderers untouched"
  - "spawn_*_task helpers idempotent: a duplicate start/unpause event doesn't pile up extra streams"

# Metrics
duration: 21 min
completed: 2026-05-28
---

# Phase 3 Plan 3: Docker Live Streams Summary

**Async tokio producers stream bollard events + per-container stats into typed DockerMsgs; LiveWorld reconciles them into the existing World/Entity shape with stable per-container slots.**

## Performance

- **Duration:** 21 min
- **Started:** 2026-05-28T11:02:42Z
- **Completed:** 2026-05-28T11:24:04Z
- **Tasks:** 2
- **Files modified:** 4 (2 created + 2 mod.rs edits)

## Accomplishments

- LiveWorld reconciles 4-variant DockerMsg into the existing `World { entities, bounds }` shape — both renderers consume it unchanged (03-04 just swaps the source).
- Stable per-container slot guarantee (CONT-05) pinned by 9 unit tests: anti-teleport across single removal, long churn, hole reuse, multi-group, and status-only changes.
- Decoupled cadence (DOCK-04) — tokio producer tasks own bollard events + per-container stats streams, push typed DockerMsg over `tokio::sync::mpsc::UnboundedSender`; render side will drain non-blocking in 03-04.
- Stats samples normalized via 03-01's `normalize()` BEFORE leaving the data layer — `RawCpu`/`RawMem` mapping factored out as `sample_from_response()` for unit testing (5 tests, no daemon needed).
- Cancellation invariant (PITFALLS Pitfall 7): per-id `HashMap<String, JoinHandle<()>>` tracks stats tasks; `die`/`destroy` events `.abort()` the right one. Orchestrator-task-owned, no Mutex.
- Zero `.unwrap()` / `.expect()` on Docker/stream results (verified via grep). Stream errors end the affected task cleanly; events::die/destroy is the canonical entity-removal path.
- 116/116 tests pass (102 baseline + 14 new); `cargo clippy --all-targets -- -D warnings` clean.

## Task Commits

Each task was committed atomically:

1. **Task 1: LiveWorld — DockerMsg + reconciliation into the existing World/Entity shape with stable slots** — `a2d8122` (feat)
2. **Task 2: spawn_docker_tasks — bollard events + per-container stats streams -> normalized DockerMsg** — `650575e` (feat)

## Files Created/Modified

- `src/world/live.rs` (NEW) — DockerMsg enum + LiveWorld. Per-group `Vec<Vec<Option<String>>>` slots, insertion-ordered group registry, idempotent `handle_added`, anti-teleport `handle_removed`, status/load in-place updates. 9 tests.
- `src/world/mod.rs` (MODIFIED) — `pub mod live;` + `pub use live::{DockerMsg, LiveWorld};`.
- `src/docker/streams.rs` (NEW) — `spawn_docker_tasks(docker, tx)` orchestrator: list_containers seed -> events loop reconcile -> per-container stats streams; `sample_from_response` pure helper. 5 tests on the response-mapping helper.
- `src/docker/mod.rs` (MODIFIED) — uncommented `pub mod streams;` (the marked line 03-01 left); `pub use streams::spawn_docker_tasks;`.

## Decisions Made

### DockerMsg shape (pinned)

Four variants, all bollard-free, deliberately small so Phase 4 can extend without breaking the consumer side:

- `Added(ContainerSnapshot)` — seed/create/start.
- `Removed(String)` — destroy/die-of-removable.
- `StatusChanged(String, theme::Status)` — pause/unpause/restart/die-transition/oom.
- `Stat(String, StatSample)` — normalized resource sample.

`ContainerSnapshot` (the 03-02 boundary) carries the `group_key` (primary network name) — Phase 4's network floor-planes (ENT-01) drop in via the same field.

### Stable-slot scheme (CONT-05 — anti-teleport)

The plan offered TWO options for the slot space. We chose **per-group slot vectors**:

- `groups: HashMap<String, u16>` — insertion-ordered group registry (first network seen -> group 0).
- `group_slots: Vec<Vec<Option<String>>>` — one slot vector per group.
- `id_to_addr: HashMap<String, (u16, usize)>` — reverse index.
- On Added: ensure_group, then push the container into its group's lowest-free slot (`Vec::position(is_none)`); fresh slot when no hole.
- On Removed: null out THAT group's slot. Other groups' slot vectors are untouched.

Rejected the global-slot-space alternative because computing `index_within_group` from a global vector forced same-group neighbors to teleport whenever a same-group middle peer left (count-of-occupied-same-group-slots would shift). The first pass of the plan implemented that approach; the `removed_frees_slot_others_stable` test caught the bug and the per-group structure replaced it.

### Entity.id encoding

`Entity.id = (group_id as u32) * (1 << 16) + index_in_group`. Rationale:

- Unique while the container is alive (group + slot is a unique tuple).
- Stable per container id (neither group nor slot index ever changes once assigned).
- Fits `u32` for any plausible container count (max 65535 groups * 65535 slots).
- No hashing — string-id-to-u32 would need a stable hash and risk collisions.

Documented in the module header and exercised by the `eid(group, idx)` test helper.

### Cancellation: JoinHandle::abort, not CancellationToken

`tokio-util::sync::CancellationToken` is not in our deps. Adding it for this single feature was rejected — `JoinHandle::abort()` is sufficient because:

- The orchestrator task is the sole owner of `HashMap<String, JoinHandle<()>>`.
- Stats tasks do nothing but await stream items and forward DockerMsgs; abort at any await point is safe.
- The orchestrator's final cleanup (events loop ended) drains the map and aborts everything — no orphans.

### Stream-error policy

Every stream consumer (events, per-container stats) ends its task on `Err(_)` with NO panic, NO retry, NO unwrap. The "container went away" signal is the canonical `events::die`/`destroy`, which fires the right `Removed` + abort the right stats task. A naked stats-stream error is treated as a transient signal the events loop will reconcile.

### Memory cache picker

Per-sample picker: `stats["inactive_file"]` (cgroup v2) first, fall back to `stats["cache"]` (cgroup v1), then 0. Per-sample because the daemon type is stable per host but unknown a priori, and a global flag would just be the same logic at startup. The picker is fully encapsulated in `raw_mem_from`; pinned by `raw_mem_picker_falls_back_to_cgroup_v1_cache`.

### Idempotent stream-task spawning

`spawn_stats_for_id_if_absent` checks `JoinHandle::is_finished()` before spawning. A duplicate `start`/`unpause` (which can happen on certain daemon restart shapes) does NOT pile up duplicate streams. Same for the orchestrator's idempotent `handle_added`: re-emitted seed events refresh the snapshot but keep the slot.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Per-group slot vectors replaced the global slot space**
- **Found during:** Task 1 (LiveWorld tests)
- **Issue:** The plan offered a "global slot space" as one option for the stable-slot scheme. The first implementation chose it and the `removed_frees_slot_others_stable` test caught a real teleport bug: when slot 1 (group 0) was freed, slot 2 (group 0)'s `index_within_group` shifted from 2 to 1, moving the box.
- **Fix:** Switched to per-group slot vectors. A same-group middle peer leaving leaves the rest of the group's `Vec` untouched — `index_in_group` stays put by construction. Different-group containers can no longer consume same-group holes (which would have caused cross-group teleports).
- **Files modified:** src/world/live.rs (architecture rewrite, same public API)
- **Verification:** All 9 LiveWorld tests pass, including the explicit anti-teleport `removed_frees_slot_others_stable` and the long-churn `id_keeps_slot_across_churn`.
- **Committed in:** a2d8122 (Task 1 commit — caught and fixed before commit)

**2. [Rule 1 - Bug] Borrow-checker / idempotent re-add path**
- **Found during:** Task 1 (first compile)
- **Issue:** The idempotent re-add path tried to mutably borrow `self.entries` while ALSO calling `self.ensure_group` (which mutably borrows `self.groups` + `self.group_slots`). The compiler rejected the overlapping borrows.
- **Fix:** Call `ensure_group` BEFORE taking the mutable borrow of `entries`. Removed an awkward intermediate helper method on `ContainerSnapshot` that the no-longer-needed re-shape had inherited.
- **Files modified:** src/world/live.rs
- **Verification:** `cargo build` clean.
- **Committed in:** a2d8122 (Task 1 commit)

**3. [Rule 1 - Bug] Removed a fragile sentinel test**
- **Found during:** Task 2 (cargo test)
- **Issue:** A sentinel test that scanned this very source file for the string `".unwrap()"` and asserted it was absent. The test failed on itself: its own assertion message contained the substring.
- **Fix:** Dropped the in-source sentinel. The plan's `<verify>` step already calls for an external `grep -n "unwrap" src/docker/streams.rs` — that grep runs to completion with only doc-comment hits (lines 24, 83) and the safe `.unwrap_or(...)` helpers; no naked `.unwrap()` or `.expect()` on Docker/stream results.
- **Files modified:** src/docker/streams.rs
- **Verification:** Manual grep + clippy clean confirm the invariant.
- **Committed in:** 650575e (Task 2 commit)

**4. [Rule 3 - Blocking] Clippy collapsible-if churn**
- **Found during:** Task 2 verification (`cargo clippy --all-targets -- -D warnings`)
- **Issue:** The events loop had multiple `match` arms each with an `if tx.send(...).is_err() { break; }` shape that clippy flagged with `collapsible_match` / `collapsible_if`.
- **Fix:** Refactored every arm to compute a `closed: bool` and a single `if stop { break; }` at the end of the match. Cleaner control flow, clippy clean, semantics unchanged.
- **Files modified:** src/docker/streams.rs
- **Verification:** `cargo clippy --all-targets -- -D warnings` clean; tests still pass.
- **Committed in:** 650575e (Task 2 commit)

---

**Total deviations:** 4 auto-fixed (3 bugs, 1 blocking — all caught DURING execution before commit)
**Impact on plan:** All deviations were execution-quality fixes (correctness, compile, clippy) — none expanded scope. The slot-space rewrite is the most consequential and STRENGTHENS the CONT-05 invariant rather than weakening it.

## Issues Encountered

None — every problem encountered was a deviation handled inline (see above) and verified by tests.

## User Setup Required

None — no external services or env vars introduced by this plan. (03-04 will surface the `connect_and_probe` errors, but only when a daemon is unreachable — and 03-02 already shipped the messages.)

## Next Phase Readiness

**Ready for 03-04 (renderer wire):**

- The producer/consumer contract is the `tokio::sync::mpsc::UnboundedSender<DockerMsg>` channel. The render side creates the channel via `tokio::sync::mpsc::unbounded_channel::<DockerMsg>()`, passes the sender into `spawn_docker_tasks(docker, tx)`, and drains the receiver with `rx.try_recv()` (non-blocking) on every render tick.
- The consumer's per-tick loop:
  ```rust
  while let Ok(msg) = rx.try_recv() {
      if let Some(world) = live_world.apply(msg) {
          app.world = world; // hand the rebuilt World to the renderer
      }
  }
  ```
- `app.world` starts empty (`LiveWorld::new()` -> empty `World`). 03-04 replaces `world::synthetic_scene()` output with this live World.
- `connect_and_probe()` (03-02) runs BEFORE `Tui::enter` in main.rs — its `Docker` handle feeds straight into `spawn_docker_tasks`. The Docker handle is `Clone` (it's a Hyper client wrapper), so cloning into each stats task is cheap.
- For 03-04 consideration: `ContainerSnapshot` from `list_containers` doesn't carry OOM/exit-code (see 03-02 doc on `from_bollard_summary`), so `exited` containers seed as `Stopped`. The events loop's `oom` handler upgrades to `Crashed` correctly when relevant; no extra inspect_container call needed.

**Concerns / open questions:**

- The `events()` stream subscribes with no filters. On a busy Docker host this can be chatty — Phase 4 can add `EventsOptionsBuilder::filters({"type": ["container"]})` (a server-side filter) to reduce wire traffic. Today we filter client-side via `evt.typ == Some(EventMessageTypeEnum::CONTAINER)`.
- The `create` event seeds with `group_key: "none"` because the bollard `EventMessage.actor.attributes` does NOT carry the container's network attachments. A subsequent `start` event correctly transitions status; the network attachment is picked up on the NEXT list_containers call (which we don't currently re-issue mid-stream). For Phase 4's network visualization this gap is acceptable — the container appears in the "none" floor-plane until the next reconnect; Phase 4 can add a targeted `inspect_container` call on `start` to backfill the group_key.
- The producer task itself returns a `JoinHandle<()>` — on shutdown 03-04 should `.abort()` it so the events stream and any in-flight stats tasks don't outlive the TUI.
- Phase 4 grouping revisit: the per-group slot scheme will need a "move container to new group" path when a container's network attachments change mid-life. Today's `handle_added` idempotent re-emit refreshes the snapshot but does NOT migrate the slot — that's a Phase-4 concern when networks become a first-class visual.

---
*Phase: 03-docker-data*
*Completed: 2026-05-28*
