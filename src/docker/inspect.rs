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

use crate::docker::domain::{EnrichedSnapshot, PortProto, PortSummary};
use crate::docker::map_status;
use crate::theme::Status;

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
}
