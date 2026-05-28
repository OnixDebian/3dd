---
phase: 03-docker-data
plan: 02
subsystem: docker
tags: [bollard, docker, daemon, probe, error-handling, domain-isolation]

# Dependency graph
requires:
  - phase: 02-scene-pipeline
    provides: theme::Status (reused by ContainerSnapshot, no parallel status enum)
provides:
  - bollard 0.21 dependency wired (resolves with tokio 1.47, no conflict)
  - src/docker/domain.rs — ContainerSnapshot { id, name, status, group_key } + map_status() + from_bollard_summary() (Anti-Pattern 4 isolation boundary)
  - src/docker/connect.rs — connect_and_probe() + ProbeError (SocketMissing/PermissionDenied/DaemonDown/Other) with actionable plain-text messages (ROB-01)
  - confirmed bollard 0.21 module paths for 03-03 (Docker, connect_with_local_defaults, system::version/ping, ContainerStatsResponse, StatsOptions/Builder)
affects: [03-03-streams (consumes connect_and_probe + maps stats into StatSample/ContainerSnapshot), 03-04-app-integration (wires probe before Tui::enter, prints + exits on ProbeError BEFORE raw mode)]

# Tech tracking
tech-stack:
  added: [bollard 0.21]
  patterns:
    - "Bollard isolation boundary: bollard::* imports confined to src/docker/connect.rs + the from_bollard_summary fn in src/docker/domain.rs (Anti-Pattern 4)"
    - "Pre-TUI probe pattern: connect_and_probe is pure logic (no terminal I/O); caller in main.rs prints + exits BEFORE raw mode entry (ROB-01 / Pitfall 9)"
    - "ProbeError classification by source-chain io::Error walk: handles bollard's hyper/hyper-util/io error wrapping"

key-files:
  created:
    - src/docker/domain.rs (ContainerSnapshot + map_status + from_bollard_summary, 11 tests)
    - src/docker/connect.rs (ProbeError + connect_and_probe + classifier, 14 tests)
  modified:
    - Cargo.toml (bollard 0.21)
    - src/docker/mod.rs (uncommented pub mod domain; pub mod connect; + re-exports)

key-decisions:
  - "ContainerSnapshot speaks theme::Status (CONT-01 reuse, no parallel enum); group_key = deterministic primary network name (sorted keys, 'none' default)"
  - "Docker state -> theme::Status rule: dead/oom/exited(!=0) -> Crashed; exited(0)/created/removing/stopping/unknown -> Stopped (visible-but-dim safe default); running/paused/restarting -> direct"
  - "ProbeError variants: SocketMissing (path), PermissionDenied (path), DaemonDown (path, detail), Other (raw text). Display gives the EXACT plain-text caller prints to stderr"
  - "Classifier walks bollard::errors::Error -> io::Error via source() chain (handles hyper/hyper-util/legacy wrapping); unknown io kinds and unclassified bollard errors fall back to DaemonDown with raw detail, never panic"
  - "Probe uses Docker::version() (single request, full handshake/parse) not ping(); no streams opened here (03-03 territory)"
  - "connect_and_probe is pure logic — NO terminal I/O. main.rs (03-04) prints + exits on Err BEFORE Tui::enter, satisfying ROB-01"

patterns-established:
  - "Domain isolation: world/render/ui never `use bollard::...`. The ONLY two files importing bollard container types are src/docker/connect.rs (Docker handle + errors) and the single from_bollard_summary fn in src/docker/domain.rs"
  - "Actionable error messages: every ProbeError variant's Display is self-contained, names the socket path, and includes a concrete remediation hint (install/run, docker group, systemctl start docker)"

# Metrics
duration: 9 min
completed: 2026-05-28
---

# Phase 3 Plan 02: Docker connect + domain isolation Summary

**bollard 0.21 wired with a classified pre-TUI daemon probe (ProbeError: SocketMissing/PermissionDenied/DaemonDown/Other) and a bollard-free ContainerSnapshot + map_status domain boundary keeping bollard types out of world/render/ui.**

## Performance

- **Duration:** 9 min
- **Started:** 2026-05-28T10:46:02Z
- **Completed:** 2026-05-28T10:56:00Z
- **Tasks:** 3
- **Files modified:** 4 (Cargo.toml, Cargo.lock, src/docker/mod.rs, plus 2 new files src/docker/domain.rs + src/docker/connect.rs)

## Accomplishments

- bollard 0.21 added, resolves with existing tokio 1.47 (no version conflict), full project builds clean
- `ContainerSnapshot { id, name, status, group_key }` — bollard-free domain shape downstream consumes; reuses `theme::Status` so we have ONE status enum across the codebase (CONT-01)
- `map_status(state, oom, exit_code)` — total mapping from Docker state strings to `theme::Status` with the chosen Crashed-vs-Stopped policy documented and tested across every state value
- `from_bollard_summary(&bollard::models::ContainerSummary) -> ContainerSnapshot` — THE single isolation point where bollard container types meet domain types (Anti-Pattern 4 boundary)
- `connect_and_probe() -> Result<Docker, ProbeError>` — pre-TUI daemon probe via `Docker::connect_with_local_defaults` + `version()` round-trip; failures classified into actionable plain-text messages ready for stderr BEFORE raw mode (ROB-01 / Pitfall 9)
- Classifier walks `bollard::errors::Error` and the `std::error::Error::source()` chain to find an underlying `io::Error` — handles hyper/hyper-util legacy wrapping that bollard 0.21 surfaces — and never panics, always yields a non-empty actionable message
- 25 new `#[cfg(test)]` tests added (11 domain + 14 connect); total project tests 77 → 102, all pass; `cargo clippy --all-targets -- -D warnings` clean

## Task Commits

Each task was committed atomically (per user CLAUDE.md: OnixDebian author, no Claude mentions, conventional commits, `{type}(03-02): ...`):

1. **Task 1: Add bollard 0.21 + confirm 0.21 API paths** — `855e597` (chore)
2. **Task 2: ContainerSnapshot + map_status + from_bollard_summary (domain isolation)** — `78e561c` (feat)
3. **Task 3: connect_and_probe + ProbeError (daemon probe with classified errors)** — `24c2b7a` (feat)

**Plan metadata:** _(this commit, docs)_

## Files Created/Modified

- `Cargo.toml` — added `bollard = "0.21"`
- `Cargo.lock` — bollard transitive deps (bollard-stubs, hyperlocal, hyper-util, etc.)
- `src/docker/mod.rs` — uncommented `pub mod domain;` and `pub mod connect;` (the lines 03-01 left marked for this plan) + re-exported `ContainerSnapshot`, `map_status`, `from_bollard_summary`, `connect_and_probe`, `ProbeError`
- `src/docker/domain.rs` (NEW, 257 lines) — `ContainerSnapshot`, `map_status`, `from_bollard_summary`, 11 tests
- `src/docker/connect.rs` (NEW, 379 lines) — `ProbeError`, `connect_and_probe`, classifier (io kind + source-chain walk), `effective_socket_path`, 14 tests

## bollard 0.21 API paths (confirmed for 03-03)

Recorded here so 03-03 doesn't re-discover them (PLAN.md explicitly asked for this):

| Item | Path in bollard 0.21 |
|------|----------------------|
| Docker handle type | `bollard::Docker` (re-exported from `crate::docker::Docker`) |
| Local-defaults constructor | `Docker::connect_with_local_defaults() -> Result<Docker, bollard::errors::Error>` (sync; bollard does a `Path::exists()` check and surfaces `Error::SocketNotFoundError(String)` here) |
| Default socket path constant | `bollard::docker::DEFAULT_SOCKET = "unix:///var/run/docker.sock"` (unix); honors `DOCKER_HOST` env var via the variant constructors |
| Probe — version | `Docker::version(&self) -> Result<SystemVersion, Error>` (async; in `bollard::system`) |
| Probe — ping | `Docker::ping(&self) -> Result<String, Error>` (async; in `bollard::system`) |
| Stats stream response type | `bollard::models::ContainerStatsResponse` (re-export of `bollard_stubs::models::ContainerStatsResponse`) |
| Stats options + builder | `bollard::query_parameters::StatsOptions` + `bollard::query_parameters::StatsOptionsBuilder` (re-exports of `bollard_stubs::query_parameters::*`) |
| Stats call | `Docker::stats(&self, container_name: &str, options: Option<StatsOptions>) -> impl Stream<Item = Result<ContainerStatsResponse, Error>>` (in `bollard::container`) |
| Container summary (list_containers) | `bollard::models::ContainerSummary` — fields used: `id: Option<String>`, `names: Option<Vec<String>>` (Docker prefixes with `/`), `state: Option<ContainerSummaryStateEnum>` (has `Display` impl yielding lowercase wire strings), `network_settings.networks: Option<HashMap<String, EndpointSettings>>` |
| Container state (inspect) | `bollard::models::ContainerState` — fields needed by `map_status`: `status: Option<ContainerStateStatusEnum>`, `oom_killed: Option<bool>`, `exit_code: Option<i64>` |
| Errors type | `bollard::errors::Error` — relevant variants: `SocketNotFoundError(String)`, `IOError { err: std::io::Error }`, `HyperResponseError { err: hyper::Error }`, `HyperLegacyError { err: hyper_util::client::legacy::Error }`, `DockerResponseServerError { status_code, message }` |

**Re-export note:** `bollard::models` is `pub use bollard_stubs::models` and `bollard::query_parameters` is `pub use bollard_stubs::query_parameters`. Use the `bollard::` paths in app code; never depend on `bollard_stubs` directly.

## ProbeError variants + messages (verbatim Display output)

The exact plain-text strings the caller (03-04) will print to stderr:

| Variant | Display |
|---------|---------|
| `SocketMissing { path }` | `Docker socket not found at {path} — is Docker installed and running?` |
| `PermissionDenied { path }` | `Permission denied on {path} — add your user to the `docker` group or use the rootless socket.` |
| `DaemonDown { path, detail }` | `Docker daemon not reachable at {path} — is it running? (try: systemctl start docker) [{detail}]` |
| `Other(detail)` | `Could not talk to the Docker daemon: {detail}` |

Path defaults to `/var/run/docker.sock` (unix) / `\\.\pipe\docker_engine` (windows); honors `DOCKER_HOST` env var.

## ContainerSnapshot shape

```rust
pub struct ContainerSnapshot {
    pub id: String,         // Full Docker container id (64-char hex). Stable slot key for layout().
    pub name: String,       // First name with Docker's leading '/' stripped; falls back to id[..12].
    pub status: theme::Status, // CONT-01 reuse — no parallel enum.
    pub group_key: String,  // Deterministic primary network name (sorted keys); "none" if no networks.
                            // Phase 4 ENT-01 layout grouping axis.
}
```

## Docker state -> theme::Status mapping rule (the chosen Crashed-vs-Stopped policy)

```
running                            -> Running
paused                             -> Paused
restarting                         -> Restarting
dead                               -> Crashed   (always; overrides exit_code=0)
exited & oom_killed==true          -> Crashed
exited & exit_code != 0            -> Crashed
exited & exit_code == 0            -> Stopped
exited & no oom/exit info          -> Stopped   (safe default — list_containers gives no oom/exit)
created / removing / stopping      -> Stopped
""  (empty) / anything unknown     -> Stopped   (visible-but-dim, never drops the container)
```

Decision rationale: `Stopped` (visible-but-dim) is the safe default so unknown states never silently vanish from the scene; the `dead` and "exited with failure signal" cases get `Crashed` (red) so the user sees the problem clearly.

## Decisions Made

- **Status mapping policy:** `dead`, exited-with-OOM, and exited-with-nonzero-exit-code → `Crashed`. Plain `exited(0)` and unknown states → `Stopped`. Documented in the `map_status` doc table and pinned by 11 unit tests covering every case.
- **`group_key` is the primary network name, sorted-keys-first for determinism:** A `HashMap` iteration order would otherwise jitter the group axis frame-to-frame; sorting keys before taking the first gives a stable choice across `list_containers` responses. `"none"` is the explicit "no networks" group key (also used by the synthetic-scene path until Phase 4 lands real grouping).
- **Probe uses `version()` not `ping()`:** Both exist on bollard 0.21; `version()` exercises the full handshake + JSON parse path and gives back `SystemVersion`, which is a strictly better signal than `ping()`'s `"PONG"` string for "is this daemon real?".
- **Classifier never returns `Other` from a connection failure:** Unclassified connection-shaped errors fall back to `DaemonDown` with the raw text appended so the user always sees a remediation hint. `Other` is reserved for genuinely non-connection failures (`JsonDataError`, `APIVersionParseError`, etc.).
- **`connect_and_probe` performs zero terminal I/O:** Keeps it testable, reusable by both backends, and safe to call BEFORE `Tui::enter`. The caller (03-04) prints + exits on `Err` before raw mode.
- **Walked `Error::source()` chain to find inner `io::Error`:** bollard 0.21 wraps lots of low-level errors via `hyper::Error` and `hyper_util::client::legacy::Error`. A direct match on `B::IOError` misses those; the source-chain walk extracts the `io::Error::kind()` so `PermissionDenied` / `ConnectionRefused` / `NotFound` reach the right `ProbeError` variant even through wrapping.
- **`mod docker;` is NOT added to main.rs by this plan:** 03-01 owns that wiring (was already in main.rs by the time this plan committed Task 2). The orchestrator's parallelism note required us to leave that line alone; we only uncommented the two marked `pub mod` lines inside `src/docker/mod.rs`.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] `cargo clippy -D warnings` rejected `if/else-if` with identical bodies in `map_status`**

- **Found during:** Task 2 verification (`cargo clippy --all-targets -- -D warnings`)
- **Issue:** Clippy `if_same_then_else` lint fired on the OOM and non-zero-exit branches of the `exited` arm; both produce `Status::Crashed`.
- **Fix:** Collapsed into a single boolean expression `if oom || nonzero_exit { Crashed } else { Stopped }` (semantically identical, clippy clean).
- **Files modified:** `src/docker/domain.rs`
- **Verification:** All 11 domain tests still pass; clippy clean.
- **Committed in:** `78e561c` (Task 2 commit).

**2. [Rule 3 - Blocking] `cargo clippy -D warnings` rejected `Option::map(...).flatten()` in `from_bollard_summary`**

- **Found during:** Task 2 verification (`cargo clippy --all-targets -- -D warnings`)
- **Issue:** Clippy `map_flatten` lint fired on the group_key extraction; idiomatic Rust uses `and_then`.
- **Fix:** Replaced `.map(|map| {...}).flatten()` with `.and_then(|map| {...})`.
- **Files modified:** `src/docker/domain.rs`
- **Verification:** Mapping logic unchanged (tested implicitly via the type signatures); clippy clean.
- **Committed in:** `78e561c` (Task 2 commit).

**3. [Rule 3 - Blocking] `cargo clippy -D warnings` rejected `io::Error::new(io::ErrorKind::Other, ...)` in test code**

- **Found during:** Task 3 verification (`cargo clippy --all-targets -- -D warnings`)
- **Issue:** Clippy `io_other_error` lint requires `io::Error::other(msg)` shorthand instead.
- **Fix:** Replaced `io::Error::new(io::ErrorKind::Other, "weird underlying io failure")` with `io::Error::other("weird underlying io failure")`.
- **Files modified:** `src/docker/connect.rs` (test only)
- **Verification:** All 14 connect tests still pass; clippy clean.
- **Committed in:** `24c2b7a` (Task 3 commit).

---

**Total deviations:** 3 auto-fixed (all Rule 3 — blocking clippy lints in our own new code on the `-D warnings` gate). **Impact on plan:** None — purely lint-policy idiom fixes, no behavior change. The clippy gate was a plan verification criterion (`cargo clippy --all-targets -- -D warnings clean`) so these had to be cleared.

## Authentication Gates

None encountered — `connect_and_probe` was implemented and unit-tested WITHOUT actually hitting a live daemon (per plan: probe is wired BEFORE the TUI in 03-04). Tests use synthetic `bollard::errors::Error` and `std::io::Error` instances to exercise the classifier deterministically. No `docker login` / `vercel login` / API-key flow involved at this layer — the docker daemon is local and the app is read-only.

## Issues Encountered

- **Parallel-write race on `src/docker/mod.rs` with 03-01 (Wave 1):** While I had edited `mod.rs` to uncomment `pub mod domain;`, 03-01 (running in parallel) wrote a newer version of `mod.rs` that reverted my edit. **Resolution:** Re-applied the `pub mod domain;` + re-export line after 03-01 committed (the merge was clean — single non-conflicting line per the plan's `<note_on_mod_rs>` contract). For Task 3's `pub mod connect;` edit, 03-01 had already settled, so the edit landed first time. The plan's contract held — the race-window edits are trivial one-liners and never structural.

## User Setup Required

None - no external service configuration required by this plan. (The user does need a local Docker daemon to run the app, but that's a runtime requirement that lands in 03-04 — and the whole point of this plan is that we'll surface that requirement as a clean plain-text message via `ProbeError` instead of crashing.)

## Next Phase Readiness

- **Ready for 03-03 (streams):** `connect_and_probe()` returns a live `Docker` handle that the streams layer can reuse directly (no reconnect). The bollard 0.21 API paths are pinned above so 03-03 doesn't re-discover them — note especially: `Docker::stats(name, Option<StatsOptions>) -> impl Stream<Item = Result<ContainerStatsResponse, Error>>` and `StatsOptionsBuilder::default().stream(true).build()`. 03-03 should map `ContainerStatsResponse` via the existing `docker::stats::normalize` (03-01 territory) and `ContainerSummary` via `docker::from_bollard_summary` (this plan).
- **Ready for 03-04 (app integration):** Call `connect_and_probe()` in `main.rs::main` BEFORE `Tui::enter` (and before `run_kitty` in the kitty path); on `Err(probe_err)` print `probe_err.user_message()` (or just `{probe_err}`) to stderr and `std::process::exit(1)`. This satisfies ROB-01 / Pitfall 9 — the failure UX is now a clean stderr line, not a raw-mode panic.
- **Blockers / concerns:**
  - None for 03-03/03-04 — the data-layer foundation is in place.
  - For Phase 4 (`world::layout` grouping by network): `ContainerSnapshot.group_key` is now the canonical group axis. The synthetic scene path will need a parallel decision (does `synthetic_scene` continue to invent group keys, or do we feed it through the same `Vec<ContainerSnapshot>` shape?). Not blocking — flag for Phase 4 planning.
  - **`from_bollard_summary` doesn't carry OOM/exit-code:** `ContainerSummary` (from `list_containers`) lacks those fields — so an `exited` container surfaces as `Stopped`, not `Crashed`, until an inspect-or-event-based path adds the OOM/exit signal. 03-03 should consider whether to (a) inspect each exited container for the precise OOM/exit signal, or (b) live with `Stopped` and let stream-fed crash events from `events()` upgrade the status. Documented in `from_bollard_summary`'s doc comment; not a blocker.

---
*Phase: 03-docker-data*
*Completed: 2026-05-28*
