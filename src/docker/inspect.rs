//! Bollard isolation for on-demand `inspect_container` calls (Phase 4).
//!
//! Two entry points feed [`crate::world::DockerMsg::Enriched`] from off-thread
//! `tokio::spawn`s in `docker::streams`:
//!
//! - [`enrich_snapshot_on_start`] backfills `group_key` + `ports` + `mount_count`
//!   when a `start` event arrives (Phase 3 carryover (2)). `create` events
//!   don't carry network attachments in bollard's `EventMessage.actor.attributes`,
//!   so the snapshot is seeded with `group_key: "none"` and corrected here once
//!   the daemon's inspect view is available.
//! - [`enrich_snapshot_on_seed`] does the same fields PLUS a `status_override`
//!   when the container is exited-with-OOM or exited-nonzero — `list_containers`
//!   doesn't carry OOMKilled / ExitCode, so an already-crashed container at
//!   startup is mis-mapped to `Stopped` and stays gray instead of red. This
//!   closes Phase 3 carryover (1).
//!
//! Both pass `size(false)` explicitly (Pitfall G from RESEARCH) — `size(true)`
//! would walk the overlay filesystem on every call, blocking the daemon. The
//! detail panel (CAM-05) will reuse this surface in 04-06.
//!
//! Bollard isolation: this file imports `bollard::Docker` +
//! `bollard::query_parameters::InspectContainerOptionsBuilder` +
//! `bollard::models::ContainerInspectResponse`. The output is the bollard-free
//! [`EnrichedSnapshot`]; downstream consumers (`world/live.rs`, both renderers)
//! never see bollard.
//!
//! Best-effort: on any bollard error we return `None`. The snapshot that was
//! already seeded by `from_bollard_summary` stays as-is — the renderer just
//! doesn't get the carryover-closure for that one container.

#![allow(dead_code)]

use bollard::query_parameters::InspectContainerOptionsBuilder;
use bollard::Docker;

use crate::docker::connect::{classify_default_path, ProbeError};
use crate::docker::domain::{EnrichedSnapshot, PortProto, PortSummary};
use crate::docker::map_status;
use crate::theme::Status;

// =============================================================================
// Detail panel data (CAM-05 / 04-06a)
// =============================================================================
//
// The bollard-free shape consumed by the popup in 04-06b. Lives in this module
// because `fetch_detail` is the only producer; the renderer reads it through
// `LiveWorld::last_inspect(id)` (cached on `DockerMsg::Inspected` arrival).
//
// Bollard isolation invariant: `DetailSnapshot` / `HealthSummary` /
// `MountSummary` contain ZERO bollard types — the panel renderer in 04-06b
// (`src/ui/detail.rs`) imports these from `crate::docker` only.

/// Per-container health summary — bollard-free mirror of bollard's `Health`.
///
/// `None` when the image has no HEALTHCHECK. `Starting` / `Healthy` /
/// `Unhealthy` mirror Docker's lifecycle. `Unhealthy` carries the failing
/// streak (number of consecutive failed checks) and the LAST probe's output
/// for the popup to display — older Moby builds may not populate
/// `failing_streak` (defaults to 0) and the `log` may be absent (output
/// defaults to empty string).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum HealthSummary {
    /// No HEALTHCHECK declared in the image.
    #[default]
    None,
    /// HEALTHCHECK declared but still inside its `start_period`.
    Starting,
    /// Last probe succeeded.
    Healthy,
    /// Last probe failed; `failing_streak` is the consecutive failure count
    /// and `last_output` is the most recent probe's `Output` (often empty
    /// if the probe just timed out).
    Unhealthy {
        failing_streak: i64,
        last_output: String,
    },
}

/// One container mount — bollard-free mirror of bollard's `MountPoint`.
///
/// Every field is owned (no borrows) so the panel renderer can hold the
/// snapshot across frames without a lifetime gymnastics. Truncation /
/// formatting (long paths, anonymous-volume names) is the renderer's job
/// in 04-06b — this layer just relays.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MountSummary {
    /// Mount type: "bind" / "volume" / "tmpfs" / "npipe" / "cluster" / "image"
    /// per Docker's MountPoint type. Empty string when bollard reported None.
    pub kind: String,
    /// Source location — for bind mounts the host path, for volumes the
    /// volume name (or storage location on the host for inspect's view).
    /// Empty for tmpfs.
    pub source: String,
    /// Destination — the path inside the container.
    pub destination: String,
    /// Mount is read-write (`true`) or read-only (`false`). Defaults to
    /// `true` when bollard reported `None` (Docker's default mount mode is RW).
    pub rw: bool,
}

/// The full detail-panel payload for one container (CAM-05 / 04-06a).
///
/// Built by [`fetch_detail`] from a single `inspect_container` call (with
/// `size = false` — Pitfall G locked: never walk the overlay filesystem). The
/// fields are populated per RESEARCH "Detail Panel Data: Field-by-Field
/// Source Map", EXCEPT `SizeRw` / `SizeRootFs` which are deliberately deferred
/// to v2 (DATA-01) because their cost is too high for a live panel toggle.
///
/// Block I/O bytes (read/write) do NOT live here — they live on
/// [`crate::docker::StatSample`] which is sampled at the stats stream
/// cadence. 04-06b reads them via `LiveWorld::last_sample(id)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailSnapshot {
    /// Full container id (hex; same id as `ContainerSnapshot.id`).
    pub id: String,
    /// Display name — leading '/' stripped to match `from_bollard_summary`.
    pub name: String,
    /// Lifecycle status — re-derived from `state.status` + `oom_killed` +
    /// `exit_code` via [`crate::docker::map_status`] so it agrees with the
    /// seed/event path (no "inspect says X, list says Y" drift).
    pub status: Status,
    /// HEALTHCHECK summary.
    pub health: HealthSummary,
    /// ISO timestamp of the last start (`state.started_at`). Empty when the
    /// container has never started. The popup parses / formats this client-
    /// side (uptime, "since 12 min ago", etc.); this layer just relays.
    pub started_at_iso: String,
    /// Total restart count since the container was created.
    pub restart_count: i64,
    /// Restart policy name ("no" / "always" / "unless-stopped" /
    /// "on-failure"). Defaults to "no" when bollard reported `None`.
    pub restart_policy: String,
    /// Human image name (`config.image`) — typically "alpine:3.20" or a
    /// `sha256:...` digest reference. Empty when bollard reported `None`.
    pub image_human: String,
    /// Image digest as recorded on the container (`inspect.image`).
    pub image_digest: String,
    /// Network mode from host_config — "default" / "host" / "bridge" /
    /// "container:<id>" / "<custom-network>". Empty when bollard reported `None`.
    pub network_mode: String,
    /// Per-network attachments: `(network_name, ipv4_address)`. Sorted by
    /// network name for stable display. IP is the empty string when none is
    /// assigned (host-network or pre-start). Same source as
    /// `pick_primary_network`'s key set so the panel and the floor-plane
    /// group never disagree.
    pub networks: Vec<(String, String)>,
    /// Published ports — reuses `extract_ports` (same logic as
    /// [`enrich_snapshot_on_start`]) so the panel and the port-glow dots
    /// always show the same list, in the same `(private, proto)` order.
    pub ports: Vec<PortSummary>,
    /// Mount points — bollard-free, sorted by `destination` for stable
    /// display. 04-06b will truncate / scroll at render time; this layer
    /// stores them all (containers with 30+ binds are legal).
    pub mounts: Vec<MountSummary>,
}

/// Enrich a snapshot from a `start` event — backfills group_key + ports +
/// mount_count from an off-thread inspect_container call. Best-effort; returns
/// `None` on any bollard error (the events loop carries on with the seeded
/// snapshot's "none" group_key etc).
pub async fn enrich_snapshot_on_start(docker: &Docker, id: &str) -> Option<EnrichedSnapshot> {
    let opts = InspectContainerOptionsBuilder::new().size(false).build();
    let resp = docker.inspect_container(id, Some(opts)).await.ok()?;
    Some(EnrichedSnapshot {
        id: id.to_string(),
        group_key: pick_primary_network(&resp),
        ports: extract_ports(&resp),
        mount_count: resp.mounts.as_ref().map(|m| m.len()).unwrap_or(0),
        status_override: None,
    })
}

/// Enrich a snapshot on the seed pass (one per `list_containers` entry at
/// startup). Same fields as on_start, plus a `status_override` that upgrades
/// `Stopped -> Crashed` for already-exited-with-OOM / exited-nonzero
/// containers (Phase 3 carryover (1)).
///
/// We spawn this UNCONDITIONALLY for every seeded container — the inspect
/// count is bounded by the container count exactly once at startup (typically
/// ~30, negligible Docker traffic). This avoids fragile matching on bollard's
/// state-string enum across versions. For Running containers the override is
/// `None` (`map_status` returns Running for state="running", and the gate below
/// only emits Crashed), so the path is a noop for healthy seeds.
pub async fn enrich_snapshot_on_seed(docker: &Docker, id: &str) -> Option<EnrichedSnapshot> {
    let opts = InspectContainerOptionsBuilder::new().size(false).build();
    let resp = docker.inspect_container(id, Some(opts)).await.ok()?;

    // Pull oom_killed + exit_code through the existing map_status policy so the
    // upgrade rule (exited+oom OR exited+nonzero -> Crashed) stays in one place.
    let status_override = resp.state.as_ref().and_then(|state| {
        let state_str = state
            .status
            .as_ref()
            .map(|s| s.to_string())
            .unwrap_or_default();
        let new = map_status(&state_str, state.oom_killed, state.exit_code);
        // Only emit an override when the inspect view PROMOTES the status to
        // Crashed — Stopped/Running/Paused/Restarting all stay as whatever
        // the seed mapped (or as whatever StatusChanged later sets). Currently
        // the only legitimate seed-time upgrade is Stopped -> Crashed.
        (new == Status::Crashed).then_some(new)
    });

    Some(EnrichedSnapshot {
        id: id.to_string(),
        group_key: pick_primary_network(&resp),
        ports: extract_ports(&resp),
        mount_count: resp.mounts.as_ref().map(|m| m.len()).unwrap_or(0),
        status_override,
    })
}

/// Fetch a full [`DetailSnapshot`] for the popup (CAM-05 / 04-06a).
///
/// Off-thread entry point: 04-06b will `tokio::spawn` this on Enter and feed
/// the result back through the mpsc as [`crate::world::DockerMsg::Inspected`].
/// The function holds no shared mutable state — `&Docker` is cheap-clone
/// (Arc inside), and the returned [`DetailSnapshot`] is owned, so it crosses
/// the channel boundary without lifetime gymnastics.
///
/// Calls `inspect_container` with `size = false` (Pitfall G — `size = true`
/// walks the overlay filesystem per call, blocking the daemon on the order of
/// seconds for big images). `SizeRw` / `SizeRootFs` are intentionally NOT on
/// [`DetailSnapshot`]; v2 (DATA-01) may add them behind a user-driven gate.
///
/// On any bollard error, classifies via [`crate::docker::connect::classify_default_path`]
/// so the popup's error message reuses 03-02's 4-variant policy (SocketMissing
/// / PermissionDenied / DaemonDown / Other). The caller (04-06b) decides
/// whether to display the error inline in the popup or close the panel.
pub async fn fetch_detail(docker: &Docker, id: &str) -> Result<DetailSnapshot, ProbeError> {
    let opts = InspectContainerOptionsBuilder::new().size(false).build();
    let resp = docker
        .inspect_container(id, Some(opts))
        .await
        .map_err(|e| classify_default_path(&e))?;

    // Status: re-derive from state.status + oom_killed + exit_code through
    // map_status so the popup agrees with the seed/event path on Crashed-vs-
    // Stopped (Phase 3 carryover (1)).
    let (status, started_at_iso, restart_count_state, health) = match resp.state.as_ref() {
        Some(state) => {
            let status_str = state.status.as_ref().map(|s| s.to_string()).unwrap_or_default();
            let status = map_status(&status_str, state.oom_killed, state.exit_code);
            let started = state.started_at.clone().unwrap_or_default();
            let health = state
                .health
                .as_ref()
                .map(health_from_bollard)
                .unwrap_or(HealthSummary::None);
            // restart_count is read off the inspect response itself, NOT off
            // state — but we collect both reads in one match to keep the
            // unwrap-or-default branching tidy.
            (status, started, resp.restart_count.unwrap_or(0), health)
        }
        None => (Status::Stopped, String::new(), 0, HealthSummary::None),
    };

    let restart_policy = resp
        .host_config
        .as_ref()
        .and_then(|hc| hc.restart_policy.as_ref())
        .and_then(|rp| rp.name.as_ref())
        .map(|n| n.to_string())
        // Docker's documented default when no policy is set: "no".
        .unwrap_or_else(|| "no".to_string());

    let network_mode = resp
        .host_config
        .as_ref()
        .and_then(|hc| hc.network_mode.clone())
        .unwrap_or_default();

    let image_human = resp
        .config
        .as_ref()
        .and_then(|c| c.image.clone())
        .unwrap_or_default();
    let image_digest = resp.image.clone().unwrap_or_default();

    let name = resp
        .name
        .as_deref()
        .map(|n| n.trim_start_matches('/').to_string())
        .unwrap_or_default();

    let networks = extract_networks(&resp);
    let ports = extract_ports(&resp);
    let mounts = extract_mounts(&resp);

    Ok(DetailSnapshot {
        id: id.to_string(),
        name,
        status,
        health,
        started_at_iso,
        restart_count: restart_count_state,
        restart_policy,
        image_human,
        image_digest,
        network_mode,
        networks,
        ports,
        mounts,
    })
}

/// Map bollard's `Health` -> bollard-free [`HealthSummary`].
///
/// The status string is canonical Docker lifecycle: "none" / "starting" /
/// "healthy" / "unhealthy". Anything else (empty string, future Moby
/// variants) falls back to [`HealthSummary::None`] — defensive but stable.
fn health_from_bollard(h: &bollard::models::Health) -> HealthSummary {
    let s = h.status.as_ref().map(|s| s.to_string()).unwrap_or_default();
    match s.as_str() {
        "starting" => HealthSummary::Starting,
        "healthy" => HealthSummary::Healthy,
        "unhealthy" => {
            // failing_streak missing on older Moby -> 0; log missing or empty
            // -> empty output string. Take the LAST log entry's `output` so
            // the popup shows the freshest probe message.
            let failing_streak = h.failing_streak.unwrap_or(0);
            let last_output = h
                .log
                .as_ref()
                .and_then(|v| v.last())
                .and_then(|r| r.output.clone())
                .unwrap_or_default();
            HealthSummary::Unhealthy {
                failing_streak,
                last_output,
            }
        }
        // "" / "none" / unknown -> no healthcheck (or unrecognised — pin to None).
        _ => HealthSummary::None,
    }
}

/// Walk an inspect response's `network_settings.networks` map and return
/// `(name, ip)` pairs sorted by network name. IP is the empty string when
/// none has been assigned (host-network containers, pre-start).
fn extract_networks(resp: &bollard::models::ContainerInspectResponse) -> Vec<(String, String)> {
    let Some(ns) = resp.network_settings.as_ref() else {
        return Vec::new();
    };
    let Some(nets) = ns.networks.as_ref() else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = nets
        .iter()
        .map(|(name, ep)| {
            let ip = ep.ip_address.clone().unwrap_or_default();
            (name.clone(), ip)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Walk an inspect response's `mounts` Vec and map every `MountPoint` to a
/// [`MountSummary`]. Sorted by `destination` for stable display.
fn extract_mounts(resp: &bollard::models::ContainerInspectResponse) -> Vec<MountSummary> {
    let Some(mps) = resp.mounts.as_ref() else {
        return Vec::new();
    };
    let mut out: Vec<MountSummary> = mps
        .iter()
        .map(|mp| MountSummary {
            // MountPointType is a String alias in bollard 0.21 — no enum
            // resolution needed; "" when bollard reported None.
            kind: mp.typ.clone().unwrap_or_default(),
            source: mp.source.clone().unwrap_or_default(),
            destination: mp.destination.clone().unwrap_or_default(),
            // RW defaults to true: Docker's default mount mode is read-write,
            // and bollard reports None for the (uncommon) case where Docker
            // didn't include the field.
            rw: mp.rw.unwrap_or(true),
        })
        .collect();
    out.sort_by(|a, b| a.destination.cmp(&b.destination));
    out
}

/// Deterministic primary-network pick from an inspect response's
/// `network_settings.networks` HashMap. Sort the keys and take the first —
/// mirrors the policy in `from_bollard_summary` so both paths agree on the
/// group_key for a multi-network container.
fn pick_primary_network(resp: &bollard::models::ContainerInspectResponse) -> String {
    resp.network_settings
        .as_ref()
        .and_then(|ns| ns.networks.as_ref())
        .and_then(|map| {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            keys.into_iter().next().cloned()
        })
        .unwrap_or_else(|| "none".to_string())
}

/// Extract bollard-free [`PortSummary`] entries from an inspect response.
///
/// `inspect.network_settings.ports` is a `HashMap<String, Option<Vec<PortBinding>>>`
/// keyed by "PORT/PROTO" strings (e.g. `"8080/tcp"`, `"53/udp"`). The KEY is
/// the source of truth for the private port + proto; the value (when `Some`)
/// carries the host-side bindings. We take the first binding's `host_port` as
/// the published port — Docker can bind to multiple host IPs, but for the
/// visualization a single host-port number suffices.
///
/// Returns entries sorted by `(private, proto)` for stable order across calls
/// (HashMap iteration order is unspecified).
fn extract_ports(resp: &bollard::models::ContainerInspectResponse) -> Vec<PortSummary> {
    let Some(ns) = resp.network_settings.as_ref() else {
        return Vec::new();
    };
    let Some(ports) = ns.ports.as_ref() else {
        return Vec::new();
    };

    let mut out = Vec::with_capacity(ports.len());
    for (key, bindings) in ports.iter() {
        // Parse "PORT/PROTO" — fall back to "tcp" when the slash is missing.
        let (port_str, proto_str) = key.split_once('/').unwrap_or((key.as_str(), "tcp"));
        let Ok(private) = port_str.parse::<u16>() else {
            continue;
        };
        let proto = PortProto::from_bollard(Some(proto_str));

        // Public port: first binding's host_port (Docker may bind multiple
        // host IPs to the same container port). None means the port is
        // declared/exposed but not published to the host.
        let public = bindings
            .as_ref()
            .and_then(|v| v.first())
            .and_then(|b| b.host_port.as_deref())
            .and_then(|s| s.parse::<u16>().ok());

        out.push(PortSummary {
            private,
            public,
            proto,
        });
    }
    out.sort_by_key(|p| (p.private, p.proto));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn inspect_with_networks(network_names: &[&str]) -> bollard::models::ContainerInspectResponse {
        let mut nets = HashMap::new();
        for n in network_names {
            nets.insert(n.to_string(), Default::default());
        }
        let ns = bollard::models::NetworkSettings {
            networks: Some(nets),
            ..Default::default()
        };
        bollard::models::ContainerInspectResponse {
            network_settings: Some(ns),
            ..Default::default()
        }
    }

    #[test]
    fn pick_primary_network_sorts_keys() {
        let r = inspect_with_networks(&["zeta", "alpha", "mid"]);
        assert_eq!(pick_primary_network(&r), "alpha");
    }

    #[test]
    fn pick_primary_network_no_networks_is_none_string() {
        // No network_settings at all -> "none".
        let r = bollard::models::ContainerInspectResponse::default();
        assert_eq!(pick_primary_network(&r), "none");
        // network_settings present but networks map None -> "none".
        let r2 = bollard::models::ContainerInspectResponse {
            network_settings: Some(bollard::models::NetworkSettings::default()),
            ..Default::default()
        };
        assert_eq!(pick_primary_network(&r2), "none");
    }

    #[test]
    fn extract_ports_parses_tcp_udp_and_sorts() {
        // Build inspect with ports = {
        //   "8080/tcp": Some([{host_port:"18080"}]),
        //   "53/udp":   None,
        //   "443/tcp":  Some([{host_port:"8443"}, {host_port:"unused"}]),  // first wins
        // }
        let mut ports: HashMap<String, Option<Vec<bollard::models::PortBinding>>> = HashMap::new();
        ports.insert(
            "8080/tcp".to_string(),
            Some(vec![bollard::models::PortBinding {
                host_ip: None,
                host_port: Some("18080".to_string()),
            }]),
        );
        ports.insert("53/udp".to_string(), None);
        ports.insert(
            "443/tcp".to_string(),
            Some(vec![
                bollard::models::PortBinding {
                    host_ip: None,
                    host_port: Some("8443".to_string()),
                },
                bollard::models::PortBinding {
                    host_ip: None,
                    host_port: Some("9999".to_string()),
                },
            ]),
        );
        let ns = bollard::models::NetworkSettings {
            ports: Some(ports),
            ..Default::default()
        };
        let r = bollard::models::ContainerInspectResponse {
            network_settings: Some(ns),
            ..Default::default()
        };

        let extracted = extract_ports(&r);
        // Stable sorted by (private, proto): 53/udp, 443/tcp, 8080/tcp.
        assert_eq!(extracted.len(), 3);
        assert_eq!(
            extracted[0],
            PortSummary {
                private: 53,
                public: None,
                proto: PortProto::Udp
            }
        );
        assert_eq!(
            extracted[1],
            PortSummary {
                private: 443,
                public: Some(8443),
                proto: PortProto::Tcp
            }
        );
        assert_eq!(
            extracted[2],
            PortSummary {
                private: 8080,
                public: Some(18080),
                proto: PortProto::Tcp
            }
        );
    }

    #[test]
    fn extract_ports_no_ports_is_empty() {
        // No network_settings at all -> empty.
        let r = bollard::models::ContainerInspectResponse::default();
        assert!(extract_ports(&r).is_empty());
        // network_settings present but ports None -> empty.
        let r2 = bollard::models::ContainerInspectResponse {
            network_settings: Some(bollard::models::NetworkSettings::default()),
            ..Default::default()
        };
        assert!(extract_ports(&r2).is_empty());
    }

    #[test]
    fn extract_ports_malformed_key_is_skipped() {
        // A "not-a-port/tcp" key should be silently dropped (not parse to u16).
        let mut ports: HashMap<String, Option<Vec<bollard::models::PortBinding>>> = HashMap::new();
        ports.insert("garbage/tcp".to_string(), None);
        ports.insert(
            "80/tcp".to_string(),
            Some(vec![bollard::models::PortBinding {
                host_ip: None,
                host_port: Some("8080".to_string()),
            }]),
        );
        let ns = bollard::models::NetworkSettings {
            ports: Some(ports),
            ..Default::default()
        };
        let r = bollard::models::ContainerInspectResponse {
            network_settings: Some(ns),
            ..Default::default()
        };
        let out = extract_ports(&r);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].private, 80);
    }

    // ---- 04-06a: fetch_detail + DetailSnapshot mappings --------------------

    fn make_health(status: &str, streak: Option<i64>, last_out: Option<&str>) -> bollard::models::Health {
        let status_enum = match status {
            "starting" => Some(bollard::models::HealthStatusEnum::STARTING),
            "healthy" => Some(bollard::models::HealthStatusEnum::HEALTHY),
            "unhealthy" => Some(bollard::models::HealthStatusEnum::UNHEALTHY),
            "none" => Some(bollard::models::HealthStatusEnum::NONE),
            "" => Some(bollard::models::HealthStatusEnum::EMPTY),
            _ => None,
        };
        let log = last_out.map(|o| {
            vec![bollard::models::HealthcheckResult {
                output: Some(o.to_string()),
                ..Default::default()
            }]
        });
        bollard::models::Health {
            status: status_enum,
            failing_streak: streak,
            log,
        }
    }

    /// Done criterion: every Health status string maps to the right
    /// HealthSummary variant; unhealthy carries failing_streak + last_output.
    #[test]
    fn health_from_bollard_covers_all_variants() {
        // "none" and "" both map to HealthSummary::None.
        assert_eq!(
            health_from_bollard(&make_health("none", None, None)),
            HealthSummary::None
        );
        assert_eq!(
            health_from_bollard(&make_health("", None, None)),
            HealthSummary::None
        );
        assert_eq!(
            health_from_bollard(&make_health("starting", None, None)),
            HealthSummary::Starting
        );
        assert_eq!(
            health_from_bollard(&make_health("healthy", None, None)),
            HealthSummary::Healthy
        );
        // unhealthy with both fields present.
        let u = health_from_bollard(&make_health("unhealthy", Some(7), Some("probe timed out")));
        match u {
            HealthSummary::Unhealthy { failing_streak, last_output } => {
                assert_eq!(failing_streak, 7);
                assert_eq!(last_output, "probe timed out");
            }
            other => panic!("expected Unhealthy, got {other:?}"),
        }
        // unhealthy with missing failing_streak (older Moby) -> 0; missing log
        // -> empty output.
        let u2 = health_from_bollard(&make_health("unhealthy", None, None));
        assert_eq!(
            u2,
            HealthSummary::Unhealthy {
                failing_streak: 0,
                last_output: String::new(),
            }
        );
    }

    /// Health::log with multiple entries: last_output reads the LAST entry
    /// (freshest probe). Pinning this so the panel always shows the most
    /// recent message.
    #[test]
    fn health_unhealthy_picks_last_log_entry() {
        let h = bollard::models::Health {
            status: Some(bollard::models::HealthStatusEnum::UNHEALTHY),
            failing_streak: Some(3),
            log: Some(vec![
                bollard::models::HealthcheckResult {
                    output: Some("old".to_string()),
                    ..Default::default()
                },
                bollard::models::HealthcheckResult {
                    output: Some("newer".to_string()),
                    ..Default::default()
                },
                bollard::models::HealthcheckResult {
                    output: Some("newest".to_string()),
                    ..Default::default()
                },
            ]),
        };
        match health_from_bollard(&h) {
            HealthSummary::Unhealthy { last_output, .. } => {
                assert_eq!(last_output, "newest", "must pick the last log entry");
            }
            other => panic!("expected Unhealthy, got {other:?}"),
        }
    }

    /// Done criterion: MountPoint -> MountSummary maps kind/source/destination
    /// trivially; rw=None defaults to true (Docker's default mount mode).
    #[test]
    fn mount_summary_maps_kind_and_rw() {
        let resp = bollard::models::ContainerInspectResponse {
            mounts: Some(vec![
                bollard::models::MountPoint {
                    typ: Some("bind".to_string()),
                    source: Some("/host/data".to_string()),
                    destination: Some("/data".to_string()),
                    rw: Some(false),
                    ..Default::default()
                },
                bollard::models::MountPoint {
                    typ: Some("volume".to_string()),
                    source: Some("/var/lib/docker/volumes/db/_data".to_string()),
                    destination: Some("/var/lib/postgresql/data".to_string()),
                    rw: None, // None should default to true (RW)
                    ..Default::default()
                },
                bollard::models::MountPoint {
                    typ: None, // None typ -> empty string
                    source: Some("".to_string()),
                    destination: Some("/tmp".to_string()),
                    rw: Some(true),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        let mounts = extract_mounts(&resp);
        // Sorted by destination: "/data", "/tmp", "/var/lib/postgresql/data".
        assert_eq!(mounts.len(), 3);
        assert_eq!(mounts[0].destination, "/data");
        assert_eq!(mounts[0].kind, "bind");
        assert_eq!(mounts[0].source, "/host/data");
        assert!(!mounts[0].rw, "explicit RW=false must propagate");
        assert_eq!(mounts[1].destination, "/tmp");
        assert_eq!(mounts[1].kind, "");
        assert!(mounts[1].rw);
        assert_eq!(mounts[2].destination, "/var/lib/postgresql/data");
        assert_eq!(mounts[2].kind, "volume");
        assert!(mounts[2].rw, "rw=None must default to true (Docker default)");
    }

    /// Done criterion: extract_mounts on no mounts is empty (defensive — both
    /// `mounts: None` and `mounts: Some(empty)`).
    #[test]
    fn extract_mounts_no_mounts_is_empty() {
        let r1 = bollard::models::ContainerInspectResponse::default();
        assert!(extract_mounts(&r1).is_empty());
        let r2 = bollard::models::ContainerInspectResponse {
            mounts: Some(Vec::new()),
            ..Default::default()
        };
        assert!(extract_mounts(&r2).is_empty());
    }

    /// extract_networks walks network_settings.networks and pulls (name, ip)
    /// pairs sorted by name. Missing IP -> empty string.
    #[test]
    fn extract_networks_pulls_name_and_ip_sorted() {
        let mut nets: HashMap<String, bollard::models::EndpointSettings> = HashMap::new();
        nets.insert(
            "zeta".to_string(),
            bollard::models::EndpointSettings {
                ip_address: Some("172.20.0.4".to_string()),
                ..Default::default()
            },
        );
        nets.insert(
            "alpha".to_string(),
            bollard::models::EndpointSettings {
                ip_address: Some("172.18.0.2".to_string()),
                ..Default::default()
            },
        );
        nets.insert(
            "mid".to_string(),
            bollard::models::EndpointSettings {
                ip_address: None,
                ..Default::default()
            },
        );
        let ns = bollard::models::NetworkSettings {
            networks: Some(nets),
            ..Default::default()
        };
        let r = bollard::models::ContainerInspectResponse {
            network_settings: Some(ns),
            ..Default::default()
        };
        let out = extract_networks(&r);
        assert_eq!(out.len(), 3);
        // Sorted by network name.
        assert_eq!(out[0].0, "alpha");
        assert_eq!(out[0].1, "172.18.0.2");
        assert_eq!(out[1].0, "mid");
        assert_eq!(out[1].1, "", "missing IP must be empty string");
        assert_eq!(out[2].0, "zeta");
        assert_eq!(out[2].1, "172.20.0.4");
    }

    /// extract_networks on no networks is empty (defensive).
    #[test]
    fn extract_networks_no_networks_is_empty() {
        let r1 = bollard::models::ContainerInspectResponse::default();
        assert!(extract_networks(&r1).is_empty());
        let r2 = bollard::models::ContainerInspectResponse {
            network_settings: Some(bollard::models::NetworkSettings::default()),
            ..Default::default()
        };
        assert!(extract_networks(&r2).is_empty());
    }

    /// DetailSnapshot Default-able via Default fields on every sub-type; an
    /// empty inspect response yields finite, populated-with-defaults values
    /// — no panics on a daemon that omits everything. Smoke test via the
    /// helpers (we don't have a live `Docker` to call `fetch_detail` against
    /// here; the helpers cover the field-mapping logic).
    #[test]
    fn helpers_on_default_inspect_response_yield_empty_collections() {
        let resp = bollard::models::ContainerInspectResponse::default();
        assert!(extract_mounts(&resp).is_empty());
        assert!(extract_networks(&resp).is_empty());
        assert!(extract_ports(&resp).is_empty());
        assert_eq!(pick_primary_network(&resp), "none");
    }
}
