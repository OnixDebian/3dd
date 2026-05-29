//! Live Docker event + per-container stats streams (the producer side).
//!
//! [`spawn_docker_tasks`] owns the three async producers and feeds typed,
//! bollard-free [`DockerMsg`] values into an mpsc channel that the renderer
//! drains via `try_recv` (the consumer wiring lands in 03-04). The producers
//! NEVER do CPU work or blocking I/O (PITFALLS Pitfall 8) — they are pure
//! async I/O -> channel.
//!
//! Architecture:
//!
//! - SEED. One `list_containers(all=true)` call on startup, mapped through
//!   [`crate::docker::from_bollard_summary`] -> `Added`. A per-running
//!   container `stats(id, stream:true)` task is spawned for each Running
//!   entry.
//! - EVENTS. A long-lived `docker.events()` subscription drives lifecycle
//!   reconciliation: create/start -> `Added` + spawn stats task; die/destroy
//!   -> `Removed` + cancel that container's stats task; pause/unpause/
//!   restart -> `StatusChanged`. This is the canonical Pitfall 7 path: we
//!   reconcile the live set from events rather than re-listing every tick.
//! - STATS (per running container). A `stats(id, StatsOptions{ stream:true })`
//!   stream feeds [`map_stats_response`] -> [`crate::docker::normalize`] ->
//!   `Stat`. On stream end/error (the container died or the daemon
//!   disconnected), the task ends cleanly — the events loop is responsible
//!   for the corresponding entity removal. We NEVER `.unwrap()` a stream
//!   item.
//!
//! Cancellation: per-container stats tasks are tracked by their
//! [`tokio::task::JoinHandle`] in a `HashMap<String, JoinHandle<()>>`. On
//! die/destroy we `abort()` the handle and drop it from the map (no orphan
//! tasks — Pitfall 7). The orchestrator task owns the map; it never escapes
//! to other tasks, so a `Mutex` isn't needed.
//!
//! Reused 03-01/03-02 surface:
//!
//! - [`crate::docker::from_bollard_summary`] — bollard `ContainerSummary` ->
//!   `ContainerSnapshot`.
//! - [`crate::docker::map_status`] — Docker state string + OOM/exit -> theme
//!   `Status`.
//! - [`crate::docker::normalize`] — raw counters -> `StatSample`.
//!
//! Bollard 0.21 surface used here (paths pinned in 03-02-SUMMARY):
//!
//! - `bollard::Docker` — the connection handle (from 03-02's
//!   `connect_and_probe`, reused not reconnected).
//! - `bollard::query_parameters::ListContainersOptionsBuilder`,
//!   `StatsOptionsBuilder`, `EventsOptionsBuilder` — builders for the three
//!   API calls.
//! - `bollard::models::{ContainerStatsResponse, EventMessage, EventMessageTypeEnum}`
//!   — re-exports of `bollard_stubs::models::*`; the only bollard-typed
//!   surface this module touches outside `connect`.

#![allow(dead_code)]

use std::collections::HashMap;

use bollard::models::{ContainerStatsResponse, EventMessage, EventMessageTypeEnum};
use bollard::query_parameters::{
    EventsOptionsBuilder, ListContainersOptionsBuilder, StatsOptionsBuilder,
};
use bollard::Docker;
use futures::StreamExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

use crate::docker::stats::{normalize, RawCpu, RawMem, StatSample};
use crate::docker::{from_bollard_summary, map_status};
use crate::theme::Status;
use crate::world::DockerMsg;

/// Spawn the Docker producer tasks.
///
/// Owns the bollard `Docker` handle from `connect_and_probe` (no reconnect)
/// and feeds typed [`DockerMsg`]s into `tx`. The orchestrator task runs for
/// the lifetime of the app — it returns a [`JoinHandle<()>`] so the caller
/// can abort it on shutdown.
///
/// Per-container stats tasks are owned internally by the orchestrator (no
/// handle escapes) and are aborted as soon as the orchestrator observes the
/// container's `die`/`destroy` event — no orphan tasks (PITFALLS Pitfall 7).
///
/// On any stream end/error (events stream tears down, daemon disconnects,
/// stats stream ends): the affected task ends cleanly. The orchestrator
/// does NOT panic, NOT `.unwrap()`, NOT block on a retry — it just stops
/// emitting. 03-04 owns the UX response to a flatlined channel.
pub fn spawn_docker_tasks(docker: Docker, tx: UnboundedSender<DockerMsg>) -> JoinHandle<()> {
    tokio::spawn(async move {
        // Seed the live set with everything visible right now. `all=true`
        // catches stopped/exited containers so the renderer can show the
        // full picture (the empty/dim states are useful info).
        let seed_opts = ListContainersOptionsBuilder::new().all(true).build();
        match docker.list_containers(Some(seed_opts)).await {
            Ok(summaries) => {
                let mut stats_tasks: HashMap<String, JoinHandle<()>> = HashMap::new();
                for s in &summaries {
                    let snap = from_bollard_summary(s);
                    // Skip empty-id entries — defensive against partial data.
                    if snap.id.is_empty() {
                        continue;
                    }
                    let id = snap.id.clone();
                    let is_running = snap.status == Status::Running;
                    if tx.send(DockerMsg::Added(snap)).is_err() {
                        return; // receiver gone, nothing more to do.
                    }
                    if is_running {
                        let handle = spawn_stats_task(docker.clone(), id.clone(), tx.clone());
                        stats_tasks.insert(id.clone(), handle);
                    }
                    // Phase 3 carryover (1): list_containers `exited`
                    // summaries land as Stopped even when the container OOM'd
                    // or exited non-zero (the summary doesn't carry exit_code
                    // / oom_killed). Fire an off-thread inspect to upgrade
                    // Stopped -> Crashed when warranted. We spawn
                    // unconditionally for every seeded container because:
                    //   (a) the seed pass runs ONCE at startup; the inspect
                    //       count is bounded by the container count
                    //       (typically ~30) — negligible Docker traffic.
                    //   (b) avoids a fragile match on bollard's state-string
                    //       enum, which has shifted across versions.
                    //   (c) enrich_snapshot_on_seed returns status_override
                    //       =None for Running containers, so the path is a
                    //       noop for healthy seeds.
                    // Best-effort; failure is silent (enrich returns None on
                    // bollard error).
                    let docker_seed = docker.clone();
                    let tx_seed = tx.clone();
                    let id_seed = id.clone();
                    tokio::spawn(async move {
                        if let Some(enr) =
                            crate::docker::enrich_snapshot_on_seed(&docker_seed, &id_seed).await
                        {
                            let _ = tx_seed.send(DockerMsg::Enriched(enr));
                        }
                    });
                }

                // Hand off to the events loop, passing the live stats-task
                // table so die/destroy can abort the right one.
                run_events_loop(docker, tx, stats_tasks).await;
            }
            Err(_) => {
                // list_containers failed — daemon is unreachable mid-startup.
                // Don't panic; just end the producer. 03-04 will detect the
                // flatlined channel and surface the failure to the user via
                // the same ProbeError-style banner.
            }
        }
    })
}

/// Subscribe to `docker.events()` and reconcile the live container set.
///
/// Container actions of interest:
///
/// | Docker action                       | DockerMsg            | Side effect          |
/// |-------------------------------------|----------------------|----------------------|
/// | `create`                            | Added (Stopped)      | -                    |
/// | `start`                             | StatusChanged(Running) + spawn stats task | -- |
/// | `pause`                             | StatusChanged(Paused)| -                    |
/// | `unpause`                           | StatusChanged(Running)| respawn stats task if missing |
/// | `restart`                           | StatusChanged(Restarting) | -               |
/// | `die`                               | StatusChanged(Stopped/Crashed) | abort stats task |
/// | `destroy`                           | Removed              | abort stats task     |
/// | `oom`                               | StatusChanged(Crashed)| -                   |
///
/// Anything we don't recognize is ignored — the next reconciling event will
/// catch up. This loop ends if the events stream ends or the receiver is
/// dropped.
async fn run_events_loop(
    docker: Docker,
    tx: UnboundedSender<DockerMsg>,
    mut stats_tasks: HashMap<String, JoinHandle<()>>,
) {
    let opts = EventsOptionsBuilder::new().build();
    let mut events = docker.events(Some(opts));

    while let Some(item) = events.next().await {
        let evt: EventMessage = match item {
            Ok(e) => e,
            Err(_) => {
                // Stream error: the daemon went away or hiccuped. End the
                // loop cleanly; the renderer sees no more updates. We never
                // panic on a stream item.
                break;
            }
        };

        // Filter to container-typed events; ignore networks/images/volumes
        // here (Phase 4 will reconcile networks separately).
        if evt.typ != Some(EventMessageTypeEnum::CONTAINER) {
            continue;
        }
        let action = evt.action.as_deref().unwrap_or("");
        let id = evt
            .actor
            .as_ref()
            .and_then(|a| a.id.as_deref())
            .unwrap_or("");
        if id.is_empty() {
            continue;
        }

        // Returns true if the channel is closed (receiver gone) — caller
        // should end the loop. Centralizing this keeps every arm a single
        // `send` + early-exit-on-closed.
        let send = |msg: DockerMsg| -> bool { tx.send(msg).is_err() };

        let stop = match action {
            "create" => {
                // The daemon emits create BEFORE the container has any state
                // other than "created". We seed it as Stopped (the safe
                // default; `map_status("created", ..) == Stopped`) and let a
                // subsequent `start` flip it to Running.
                let snap = crate::docker::ContainerSnapshot {
                    id: id.to_string(),
                    name: container_name_from_event(&evt, id),
                    status: map_status("created", None, None),
                    group_key: "none".to_string(),
                    ..crate::docker::ContainerSnapshot::default()
                };
                send(DockerMsg::Added(snap))
            }
            "start" => {
                let closed = send(DockerMsg::StatusChanged(id.to_string(), Status::Running));
                if !closed {
                    spawn_stats_for_id_if_absent(&docker, &tx, &mut stats_tasks, id);
                    // Phase 3 carryovers (2)/(3): backfill group_key + ports +
                    // mount_count from an off-thread inspect. If the
                    // group_key differs from what `create` seeded ("none"),
                    // LiveWorld will migrate the slot to the real network's
                    // Z-band. Producer never blocks on inspect — this is
                    // tokio::spawn fire-and-forget (Pitfall 8).
                    let docker_start = docker.clone();
                    let tx_start = tx.clone();
                    let id_owned = id.to_string();
                    tokio::spawn(async move {
                        if let Some(enr) =
                            crate::docker::enrich_snapshot_on_start(&docker_start, &id_owned).await
                        {
                            let _ = tx_start.send(DockerMsg::Enriched(enr));
                        }
                    });
                }
                closed
            }
            "unpause" => {
                let closed = send(DockerMsg::StatusChanged(id.to_string(), Status::Running));
                if !closed {
                    spawn_stats_for_id_if_absent(&docker, &tx, &mut stats_tasks, id);
                }
                closed
            }
            "pause" => send(DockerMsg::StatusChanged(id.to_string(), Status::Paused)),
            "restart" => send(DockerMsg::StatusChanged(id.to_string(), Status::Restarting)),
            "die" => {
                // Use exit code from event attributes if present; OOM is
                // signalled by a separate `oom` event.
                let exit_code = evt
                    .actor
                    .as_ref()
                    .and_then(|a| a.attributes.as_ref())
                    .and_then(|attrs| attrs.get("exitCode"))
                    .and_then(|s| s.parse::<i64>().ok());
                // Map an "exited" state with the exit code we have. OOM
                // upgrades happen via the `oom` event (below).
                let status = map_status("exited", None, exit_code);
                let closed = send(DockerMsg::StatusChanged(id.to_string(), status));
                abort_stats_task(&mut stats_tasks, id);
                closed
            }
            "oom" => {
                // OOMKilled overrides — even a clean exit becomes Crashed.
                let status = map_status("exited", Some(true), Some(137));
                let closed = send(DockerMsg::StatusChanged(id.to_string(), status));
                abort_stats_task(&mut stats_tasks, id);
                closed
            }
            "destroy" => {
                abort_stats_task(&mut stats_tasks, id);
                send(DockerMsg::Removed(id.to_string()))
            }
            _ => {
                // Anything else (kill/exec_*/health_status/...) — not
                // visualized by this phase. Phase 4 may add health states.
                false
            }
        };
        if stop {
            break;
        }
    }

    // Loop ended: abort any straggler stats tasks so they don't outlive the
    // events loop (Pitfall 7 — no orphan tasks).
    for (_id, handle) in stats_tasks.drain() {
        handle.abort();
    }
}

/// Recover a display name from an event's actor attributes ("name" key Docker
/// sets on container events). Falls back to a short id slice.
fn container_name_from_event(evt: &EventMessage, id: &str) -> String {
    let raw = evt
        .actor
        .as_ref()
        .and_then(|a| a.attributes.as_ref())
        .and_then(|attrs| attrs.get("name"))
        .cloned();
    if let Some(n) = raw {
        // Docker doesn't prefix the name on event attrs the way it does on
        // list_containers, but trim a leading slash just in case to keep
        // consistency with `from_bollard_summary`.
        n.trim_start_matches('/').to_string()
    } else {
        id.chars().take(12).collect()
    }
}

/// Spawn a stats task for `id` unless one is already alive in the map.
/// Idempotent — repeated start/unpause events don't pile up duplicate streams.
fn spawn_stats_for_id_if_absent(
    docker: &Docker,
    tx: &UnboundedSender<DockerMsg>,
    stats_tasks: &mut HashMap<String, JoinHandle<()>>,
    id: &str,
) {
    if let Some(handle) = stats_tasks.get(id) {
        if !handle.is_finished() {
            return;
        }
    }
    let handle = spawn_stats_task(docker.clone(), id.to_string(), tx.clone());
    stats_tasks.insert(id.to_string(), handle);
}

/// Abort a per-container stats task and remove it from the map. Safe to call
/// for ids that were never tracked.
fn abort_stats_task(stats_tasks: &mut HashMap<String, JoinHandle<()>>, id: &str) {
    if let Some(handle) = stats_tasks.remove(id) {
        handle.abort();
        // We don't await — `abort()` is fire-and-forget; the task ends at
        // its next .await point (the next stream item or yield).
    }
}

/// Spawn one per-container `stats(id, stream:true)` task. Each sample is
/// mapped onto 03-01's `RawCpu`/`RawMem` and normalized into a `StatSample`,
/// then shipped as a `DockerMsg::Stat`.
///
/// On stream end / error / channel close: end the task cleanly. The events
/// loop's `die`/`destroy` handler removes the entity; we never panic.
fn spawn_stats_task(
    docker: Docker,
    id: String,
    tx: UnboundedSender<DockerMsg>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let opts = StatsOptionsBuilder::new()
            .stream(true)
            .one_shot(false)
            .build();
        let mut stream = docker.stats(&id, Some(opts));
        // bollard's stats stream already carries `precpu_stats` per sample
        // (the prior frame). We feed BOTH cpu_stats and precpu_stats into
        // `normalize` each iteration — no per-task "previous sample" state
        // is needed on our side.
        while let Some(item) = stream.next().await {
            let resp = match item {
                Ok(r) => r,
                Err(_) => {
                    // Stream error (typically: container died, daemon
                    // restart). End cleanly; events::die/destroy handles
                    // entity removal. NEVER panic.
                    return;
                }
            };
            let sample = sample_from_response(&resp);
            if tx.send(DockerMsg::Stat(id.clone(), sample)).is_err() {
                // Channel closed — receiver gone, we're done.
                return;
            }
        }
        // Stream ended without error: container stopped streaming, we're done.
    })
}

/// Pure helper: turn a bollard [`ContainerStatsResponse`] into a normalized
/// [`StatSample`] by mapping its fields onto 03-01's `RawCpu`/`RawMem` input
/// structs and calling [`normalize`].
///
/// Memory cache picker: cgroup v2 emits `inactive_file` while cgroup v1 emits
/// `cache`. We try v2 first (the modern default), fall back to v1, and 0
/// when neither is present. Picking on every sample is fine — the daemon
/// type doesn't change at runtime, but it does vary by host (CI, dev box,
/// prod) and we'd rather be robust than configurable here.
///
/// Factored out so unit tests can hand-build a `ContainerStatsResponse`
/// fixture (no daemon needed) and assert the mapping is finite.
pub fn sample_from_response(resp: &ContainerStatsResponse) -> StatSample {
    let cur = raw_cpu_from(resp.cpu_stats.as_ref());
    let prev_storage = raw_cpu_from(resp.precpu_stats.as_ref());
    let mem = raw_mem_from(resp.memory_stats.as_ref());

    // Treat a zero `prev` (the very-first-frame `precpu_stats` shape Docker
    // emits) as warming-up by passing it through `normalize` which detects
    // both-zero counters as warming-up. We always pass Some(&prev) here so
    // every NON-first-frame sample gets a real delta computation; the
    // warming-up sentinel is the prev counters themselves, NOT a `None`.
    normalize(&cur, Some(&prev_storage), &mem)
}

/// Map bollard's `ContainerCpuStats` slot onto our `RawCpu` input.
/// Missing fields default to 0 / `None` / empty — `normalize` then guards
/// every downstream path.
fn raw_cpu_from(cpu: Option<&bollard::models::ContainerCpuStats>) -> RawCpu {
    let Some(cpu) = cpu else {
        return RawCpu::default();
    };
    let usage = cpu.cpu_usage.as_ref();
    let total_usage = usage.and_then(|u| u.total_usage).unwrap_or(0);
    let percpu_len = usage
        .and_then(|u| u.percpu_usage.as_ref())
        .map(|v| v.len())
        .unwrap_or(0);
    RawCpu {
        total_usage,
        system_usage: cpu.system_cpu_usage,
        // bollard reports `online_cpus: Option<u32>` — widen to u64 (our
        // RawCpu uses u64 to keep arithmetic uniform with `total_usage`).
        online_cpus: cpu.online_cpus.map(u64::from),
        percpu_len,
    }
}

/// Map bollard's `ContainerMemoryStats` slot onto our `RawMem` input.
///
/// Cache picker: prefer cgroup v2's `inactive_file`, fall back to cgroup
/// v1's `cache`, then 0. The picker is per-sample because the daemon type is
/// stable per host but unknown a priori (and Phase 3 ships without an env
/// flag for it). 03-04 may surface the detected cgroup version in the
/// status bar later if we ever need it.
fn raw_mem_from(mem: Option<&bollard::models::ContainerMemoryStats>) -> RawMem {
    let Some(mem) = mem else {
        return RawMem::default();
    };
    let usage = mem.usage.unwrap_or(0);
    let limit = mem.limit.unwrap_or(0);
    let cache = mem
        .stats
        .as_ref()
        .and_then(|s| s.get("inactive_file").copied().or_else(|| s.get("cache").copied()))
        .unwrap_or(0);
    RawMem {
        usage,
        cache,
        limit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bollard::models::{
        ContainerCpuStats, ContainerCpuUsage, ContainerMemoryStats, ContainerStatsResponse,
    };
    use std::collections::HashMap;

    /// Build a `ContainerStatsResponse` fixture for a busy container so the
    /// mapping+normalize path can be exercised without a daemon. cpu_delta =
    /// 1000, system_delta = 10_000, online_cpus = 4 -> cpu_pct = 40.0.
    fn busy_response_fixture() -> ContainerStatsResponse {
        let mut stats = HashMap::new();
        stats.insert("inactive_file".to_string(), 100u64);
        let mem = ContainerMemoryStats {
            usage: Some(500),
            stats: Some(stats),
            limit: Some(1000),
            ..Default::default()
        };
        let cpu = ContainerCpuStats {
            cpu_usage: Some(ContainerCpuUsage {
                total_usage: Some(2000),
                percpu_usage: Some(vec![500, 500, 500, 500]),
                ..Default::default()
            }),
            system_cpu_usage: Some(20_000),
            online_cpus: Some(4),
            ..Default::default()
        };
        let precpu = ContainerCpuStats {
            cpu_usage: Some(ContainerCpuUsage {
                total_usage: Some(1000),
                ..Default::default()
            }),
            system_cpu_usage: Some(10_000),
            online_cpus: Some(4),
            ..Default::default()
        };
        ContainerStatsResponse {
            cpu_stats: Some(cpu),
            precpu_stats: Some(precpu),
            memory_stats: Some(mem),
            ..Default::default()
        }
    }

    /// Done criterion: the bollard-response -> StatSample mapping yields a
    /// finite, in-range sample that the existing `load_to_half_extent` can
    /// consume — proves the 03-01 normalizer is wired correctly.
    #[test]
    fn sample_from_response_maps_busy_fixture_to_finite_sample() {
        let resp = busy_response_fixture();
        let s = sample_from_response(&resp);
        assert!(s.cpu_pct.is_finite(), "cpu_pct = {}", s.cpu_pct);
        assert!(s.mem_fraction.is_finite(), "mem_fraction = {}", s.mem_fraction);
        assert!(s.load.is_finite(), "load = {}", s.load);
        assert!(!s.warming_up, "non-zero prev counters => not warming up");
        // (1000/10_000) * 4 * 100 = 40.0
        assert!((s.cpu_pct - 40.0).abs() < 1e-3, "cpu_pct = {}", s.cpu_pct);
        // mem_used = usage - inactive_file = 500 - 100 = 400; fraction = 0.4
        assert!((s.mem_fraction - 0.4).abs() < 1e-3, "mem_fraction = {}", s.mem_fraction);
        // load = max(cpu_norm=40/400=0.1, mem_fraction=0.4) = 0.4
        assert!((s.load - 0.4).abs() < 1e-3, "load = {}", s.load);
        // Finally: feeds load_to_half_extent unchanged.
        let h = crate::world::entity::load_to_half_extent(s.load);
        assert!(h.is_finite() && h > 0.0, "h = {h}");
    }

    /// The very first frame of a stream (zero `precpu_stats` counters) maps
    /// to warming-up: load forced to 0.0 so the box doesn't snap. This is
    /// the Pitfall 1 invariant that flows through the producer side.
    #[test]
    fn sample_from_response_treats_zero_precpu_as_warming_up() {
        let mut resp = busy_response_fixture();
        // Wipe precpu (zero counters — the docker daemon's first-frame shape).
        resp.precpu_stats = Some(ContainerCpuStats {
            cpu_usage: Some(ContainerCpuUsage::default()),
            system_cpu_usage: Some(0),
            online_cpus: Some(4),
            ..Default::default()
        });
        let s = sample_from_response(&resp);
        assert!(s.warming_up, "zero precpu counters must flag warming-up");
        assert_eq!(s.load, 0.0, "warming-up samples force load to 0.0");
    }

    /// Missing memory stats (Windows daemon, partial sample) must NOT panic
    /// — `RawMem::default()` flows through normalize cleanly.
    #[test]
    fn sample_from_response_handles_missing_memory_stats() {
        let mut resp = busy_response_fixture();
        resp.memory_stats = None;
        let s = sample_from_response(&resp);
        assert!(s.mem_fraction.is_finite());
        assert_eq!(s.mem_used, 0);
        assert_eq!(s.mem_limit, 0);
    }

    /// cgroup v1 hosts emit `cache` instead of `inactive_file`. The picker
    /// must fall back to `cache` when v2's key is absent.
    #[test]
    fn raw_mem_picker_falls_back_to_cgroup_v1_cache() {
        let mut stats = HashMap::new();
        stats.insert("cache".to_string(), 200u64);
        let mem = ContainerMemoryStats {
            usage: Some(1000),
            stats: Some(stats),
            limit: Some(2000),
            ..Default::default()
        };
        let r = raw_mem_from(Some(&mem));
        assert_eq!(r.usage, 1000);
        assert_eq!(r.cache, 200, "cgroup v1 cache key must be picked up");
        assert_eq!(r.limit, 2000);
    }

    /// `online_cpus` widens from u32 to u64 cleanly.
    #[test]
    fn raw_cpu_widens_online_cpus_to_u64() {
        let cpu = ContainerCpuStats {
            cpu_usage: Some(ContainerCpuUsage {
                total_usage: Some(123),
                percpu_usage: Some(vec![10, 20, 30]),
                ..Default::default()
            }),
            online_cpus: Some(7),
            system_cpu_usage: Some(456),
            ..Default::default()
        };
        let r = raw_cpu_from(Some(&cpu));
        assert_eq!(r.total_usage, 123);
        assert_eq!(r.system_usage, Some(456));
        assert_eq!(r.online_cpus, Some(7u64));
        assert_eq!(r.percpu_len, 3);
    }

}
