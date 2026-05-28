---
phase: 03-docker-data
plan: 01
subsystem: docker-data
tags: [docker, stats, cpu-delta, memory, normalizer, tdd, rust]

# Dependency graph
requires:
  - phase: 01-render-core
    provides: theme::Status (CONT-01) — reused by Entity via world layer
  - phase: 02-scene-pipeline
    provides: world::entity::load_to_half_extent + MIN_HALF/MAX_HALF (CONT-02) — the [0,1]->half-extent map our `load` field feeds
provides:
  - "src/docker/ module root with stats live + commented slots for domain/connect/streams (03-02 / 03-03 file-add only)"
  - "stats::RawCpu / stats::RawMem — bollard-free input structs 03-03 will map ContainerStatsResponse onto"
  - "stats::StatSample — the domain stat type (cpu_pct, mem_used, mem_limit, mem_fraction, load, warming_up)"
  - "stats::normalize() — pure, NaN-safe CPU%-delta + memory normalizer, pinned by 13 unit tests on synthetic before/after samples"
  - "load: [0,1] field that feeds world::entity::load_to_half_extent unchanged — box size now driven by a guarded number"
affects: [03-02-docker-domain, 03-03-stats-stream, 03-04-renderer-wire, phase-4-detail-hud]

# Tech tracking
tech-stack:
  added: []        # NO new deps — bollard came in via 03-02's prior commit, stats.rs imports zero of it
  patterns:
    - "Pure normalizer over plain input structs (bollard-free) — 03-03 maps daemon types onto Raw* before calling normalize, isolating churn"
    - "TDD red→green: every guard from PITFALLS Pitfall 1 pinned by a failing test before any implementation existed"
    - "f64 for u64-counter delta math; f32 only on the final user-visible field"
    - "Final-scrub coerces any non-finite slip to 0.0 — defence-in-depth against future code paths producing NaN/Inf"

key-files:
  created:
    - "src/docker/mod.rs — phase-3 module root; pub mod stats; live + marked commented slots for domain/connect/streams"
    - "src/docker/stats.rs — RawCpu/RawMem/StatSample + normalize() + 13 unit tests"
  modified:
    - "src/main.rs — added `mod docker;` (alphabetical, after `mod config;`)"

key-decisions:
  - "load = max(cpu_norm, mem_fraction) — a box grows for whichever resource it is hot on"
  - "Warming-up = prev_cpu.is_none() OR prev counters are both zero (covers bollard's first-frame precpu_stats shape)"
  - "Counter regression (cpu_delta < 0) treated as idle (0.0), not as overflow"
  - "mod.rs declared live `pub mod stats;` plus two MARKED commented stubs for sibling modules — 03-02/03-03 each uncomment exactly one line (no file conflict)"

patterns-established:
  - "Pure-function normalizer + bollard-free input structs: the bollard mapping lives in 03-03 (one file, one place to update on API churn)"
  - "Test-first for any guard-heavy numeric pipeline that drives renderer geometry — a NaN here = a NaN half-extent = renderer garbage"

# Metrics
duration: 6min
completed: 2026-05-28
---

# Phase 3 Plan 01: Docker Stats Normalizer Summary

**Pure CPU%-delta + memory normalizer with first-sample, zero-divisor, and NaN/Inf guards, pinned by 13 unit tests on synthetic before/after samples — the renderer's size signal is now provably finite.**

## Performance

- **Duration:** 6 min
- **Started:** 2026-05-28T10:45:52Z
- **Completed:** 2026-05-28T10:51:26Z
- **Tasks:** 2 (RED + GREEN)
- **Files modified:** 3 (mod.rs + stats.rs created, main.rs touched)

## Accomplishments

- `src/docker/` module rooted with `stats` live and clearly-marked commented slots for sibling submodules — 03-02 (`domain`, `connect`) and 03-03 (`streams`) each uncomment exactly one line when their file lands.
- `stats::normalize()` implements PITFALLS Pitfall 1 exactly: `cpu_pct = (cpu_delta/system_delta) * online_cpus * 100`, with `online_cpus` falling back to `percpu_len` when missing/zero, `system_delta <= 0` short-circuiting before any division, the final value clamped to `[0, online_cpus*100]`, and a closing scrub coercing any non-finite slip to `0.0`.
- Warming-up detection covers both the no-prior-sample case AND bollard's first-frame "precpu_stats with all-zero counters" shape — the box is not sized from this sample (load forced to 0.0).
- Memory subtracts cache (cgroup-v2 inactive_file / v1 cache, picked by 03-03), clamps used to `[0, limit]`, emits `mem_fraction` in `[0,1]` (or 0.0 when limit == 0).
- A single `load` field — the MAX of CPU-normalized and `mem_fraction`, clamped to `[0,1]` — feeds the EXISTING `world::entity::load_to_half_extent` unchanged. Box size signal is now driven by a guaranteed-finite number.
- Test suite: 64 → 77 (13 new), all clean. `cargo clippy --tests -- -D warnings` clean. No bollard import anywhere in stats.rs.

## Task Commits

1. **Task 1 (RED): docker module skeleton + failing normalizer tests** — `3150e94` (test)
2. **Task 2 (GREEN): implement the guarded normalizer** — `24050f3` (feat)

**Plan metadata:** (this commit)

## Files Created/Modified

- `src/docker/mod.rs` — phase-3 module root, declares `pub mod stats;` live, carries marked commented stubs for `domain` / `connect` / `streams` (the lines 03-02 and 03-03 will uncomment), re-exports `normalize`, `RawCpu`, `RawMem`, `StatSample`.
- `src/docker/stats.rs` — `RawCpu` / `RawMem` (bollard-free input structs that 03-03 will map `ContainerStatsResponse` onto), `StatSample` (domain output), `normalize()` (pure, guarded, NaN-safe), 13 `#[cfg(test)]` cases on synthetic before/after samples.
- `src/main.rs` — added `mod docker;` (alphabetical, after `mod config;`).

## Decisions Made

- **Load combination = `max(cpu_norm, mem_fraction)`.** A box grows for whichever resource it is hot on. Alternative weighted averages were rejected: max preserves the "hot box reads big" signal without a quiet container's heavy memory hiding behind a CPU spike (or vice versa). `cpu_pct` and `mem_fraction` are also exposed on `StatSample` so a future HUD (Phase 4) can surface both numbers without re-doing the math.
- **Warming-up detection includes the all-zero precpu_stats shape**, not just `prev_cpu.is_none()`. bollard's stream emits a first frame where `precpu_stats` is present but zero, which would yield a huge `system_delta` against `cur.system_usage` and a meaningless tiny cpu_pct — flagging it as warming-up forces `load = 0.0` and the box stays at floor size for that one frame.
- **Counter regression (cur < prev) treated as idle, not as overflow.** Docker counters do not wrap in any practical timeframe, but daemon restarts or container restarts can reset them. Returning 0.0 is harmless; pretending nothing happened by wrapping would inflate a fake spike.
- **f64 for delta math, f32 only on output.** Counters are u64 and large (nanoseconds since container start can easily exceed `f32`'s 24-bit mantissa); f64 keeps the delta accurate. The user-facing fields are f32 because they feed the f32 renderer / size map.
- **mod.rs uses marked commented stubs, not a file-existence probe.** Simpler, language-level, no build script. 03-02 and 03-03 do a one-line uncomment with the comment as their landing-zone marker.
- **`#![allow(dead_code)]` + `#![allow(unused_imports)]` on mod.rs.** Forward-facing API (consumed by 03-04 and Phase 4) — public before its first call site, mirrors the established pattern from `world/entity.rs` and others.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Negative `cpu_delta` (counter regression) was not explicitly listed in the plan's guards, but is a real edge — added it to the zero-cpu branch.**

- **Found during:** Task 2 (GREEN implementation) — designing the `never_nan_inf` adversarial sweep.
- **Issue:** The plan's guards covered first sample, `system_delta <= 0`, `online_cpus == 0`, and non-finite. It did NOT explicitly cover the case where `cur.total_usage < prev.total_usage` (a daemon/container restart). Without a guard, the formula would yield a NEGATIVE cpu_pct, which the clamp to `[0, online*100]` would catch — but emitting 0.0 explicitly is clearer and survives a future guard reorder.
- **Fix:** Combined the existing `system_delta <= 0.0` short-circuit with `|| cpu_delta < 0.0` and added a `counter regression` test case to the `never_nan_inf` sweep.
- **Files modified:** `src/docker/stats.rs` (normalize() + tests::never_nan_inf cases vector)
- **Verification:** `never_nan_inf` passes with `cur.total_usage=100, prev.total_usage=1_000` in the cases vector; `load` is finite, `cpu_pct == 0.0`.
- **Committed in:** `24050f3` (GREEN task commit)

**2. [Rule 3 - Blocking] `pub use` of forward-facing items triggers `unused_imports` warning at crate root.**

- **Found during:** Task 1 (RED) — first build after writing mod.rs.
- **Issue:** `pub use stats::{normalize, RawCpu, RawMem, StatSample};` is a forward-facing API consumed by 03-04 / Phase 4 — it has no caller yet, so the compiler warns `unused_imports`. The plan says clippy must be clean with `-D warnings`.
- **Fix:** Added `#![allow(unused_imports)]` alongside the existing `#![allow(dead_code)]` on mod.rs, with a comment explaining why ("forward-facing API, wired in by 03-04 / Phase 4").
- **Files modified:** `src/docker/mod.rs`
- **Verification:** `cargo clippy --tests --bin dd3 -- -D warnings` clean.
- **Committed in:** `3150e94` (RED task commit)

---

**Total deviations:** 2 auto-fixed (1 missing-guard bug, 1 blocking lint).
**Impact on plan:** Both auto-fixes essential. The counter-regression guard hardens the normalizer against daemon restarts (PITFALLS-class concern that the plan's high-level guards covered implicitly but didn't name). The `allow(unused_imports)` is a presentation-layer fix — the re-exports themselves are deliberate and called out in the plan.

## Issues Encountered

- **Parallel-execution file race on `src/docker/mod.rs` and `src/world/scene.rs`.** Wave 1 of phase 3 runs 03-01 (this plan) and 03-02 (the bollard / domain plan) in parallel against the same working tree. During Task 2 verification, a `cargo fmt --all` reformatted the entire crate (which had pre-existing fmt drift in pre-phase-3 files such as `src/world/scene.rs`, `src/app.rs`, etc.), AND the parallel 03-02 agent had simultaneously written `pub mod domain;` into `mod.rs` plus an untracked `src/docker/domain.rs`. Resolved by `git checkout`-ing every file outside this plan's scope (all pre-existing files + `src/docker/mod.rs`) and leaving 03-02's untracked `src/docker/domain.rs` alone for that plan's executor to commit. My final commits touch only `src/docker/mod.rs`, `src/docker/stats.rs`, `src/main.rs` (the last is part of the RED commit only).
- **Note for the orchestrator:** because of the above, the `cargo fmt --check` verification was satisfied only for THIS plan's files (`rustfmt --check src/docker/stats.rs src/docker/mod.rs`), not the whole crate. Pre-existing fmt drift in `src/world/scene.rs` etc. predates phase 3 and is not this plan's to fix.

## Next Phase Readiness

Ready for 03-02 (`docker::domain` / `docker::connect`) and 03-03 (`docker::streams`):

- **03-02 task:** uncomment `// pub mod domain;` and `// pub mod connect;` in `src/docker/mod.rs` (lines 33 and 34 in this plan's commit). The `connect` line is still a comment; if 03-02 is already in flight, it has presumably done the equivalent edit on its own branch/working state — let its commit land normally.
- **03-03 task:** uncomment `// pub mod streams;` (line 35). It will MAP bollard's `ContainerStatsResponse` onto `RawCpu` / `RawMem` per the field comments on those structs (the cgroup v1 vs v2 cache-key pick is the responsibility of 03-03, not stats.rs).
- **03-04 task:** call `crate::docker::normalize(...)` and feed `StatSample::load` into `world::entity::load_to_half_extent(...)` — no normalizer-side changes needed.

No blockers, no open questions on the normalizer itself.

---
*Phase: 03-docker-data*
*Completed: 2026-05-28*
