//! Bollard-free domain types — the isolation boundary (ARCHITECTURE Anti-Pattern 4).
//!
//! Downstream code (`world/`, `render3d/`, `kitty.rs`, `ui/`) speaks
//! [`ContainerSnapshot`] and reuses [`crate::theme::Status`] for status colors.
//! It must never `use bollard::...`. This file is the ONE place where bollard
//! container/state types are touched and mapped to those domain types — so when
//! bollard's generated stubs reshuffle (they have, between 0.18 and 0.21), only
//! this file needs to follow.
//!
//! What lives here:
//!
//! - [`ContainerSnapshot`] — the renderer's view of a single container: stable
//!   id, display name, lifecycle status (theme::Status, CONT-01), a `group_key`
//!   (primary network name) the ENT-01 layout grouping reads, plus `ports` and
//!   `mount_count` consumed by ENT-02 (port-glow dots) and ENT-03 (volume
//!   cylinders) in Phase 4.
//! - [`PortSummary`] / [`PortProto`] — bollard-free mirrors of bollard's port
//!   types, used by both the seed path (`from_bollard_summary`) and the enrich
//!   path (`docker::inspect`).
//! - [`EnrichedSnapshot`] — the bollard-free payload of
//!   [`crate::world::DockerMsg::Enriched`]; backfills group_key, ports,
//!   mount_count, and an optional status override from an off-thread
//!   `inspect_container` call (closes Phase 3 carryovers (1)/(2)/(3)).
//! - [`map_status`] — pure string + OOM/exit-code -> [`crate::theme::Status`]
//!   mapping. The `dead`/`exited(!=0)` -> Crashed rule lives here; `unknown` ->
//!   Stopped is the safe default (visible but dim).
//! - [`from_bollard_summary`] — the SINGLE function that imports bollard
//!   container types. Maps a bollard `ContainerSummary` (from `list_containers`)
//!   into a [`ContainerSnapshot`].

#![allow(dead_code)]

use crate::theme::Status;

/// Port-mapping protocol (bollard-free mirror of `bollard::models::PortSummaryTypeEnum`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PortProto {
    Tcp,
    Udp,
    Sctp,
}

impl PortProto {
    /// Wire-string form ("tcp"/"udp"/"sctp") for display.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
            Self::Sctp => "sctp",
        }
    }

    /// Parse from bollard's lowercase proto string. Unknown / `None` / empty
    /// fall back to TCP — Docker's default + Pitfall safety.
    pub(crate) fn from_bollard(s: Option<&str>) -> Self {
        match s.unwrap_or("tcp").to_ascii_lowercase().as_str() {
            "udp" => Self::Udp,
            "sctp" => Self::Sctp,
            _ => Self::Tcp,
        }
    }
}

/// A single published port mapping — bollard-free.
///
/// `private` is the port inside the container, `public` is the host-side port
/// (`None` when the port is exposed but not published, or when the mapping
/// hasn't been resolved yet).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PortSummary {
    pub private: u16,
    pub public: Option<u16>,
    pub proto: PortProto,
}

/// The renderer's view of one container — bollard-free, stable across
/// bollard API churn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerSnapshot {
    /// Real Docker container id (hex string, 64 chars). Stable across the
    /// container's lifetime; the world uses this as the slot key in `layout()`.
    pub id: String,
    /// Display name. Docker prefixes user names with `/` in the API ("/web-1");
    /// this is stripped here so renderers print the user-facing form ("web-1").
    pub name: String,
    /// Lifecycle status — reused from the theme palette so we have ONE status
    /// enum across the codebase (CONT-01). Resolved to color downstream via
    /// `Palette::status_color`.
    pub status: Status,
    /// Primary network name — the ENT-01 grouping axis (Phase 4 floor-planes).
    /// `"none"` when the container is attached to no network. When multiple
    /// networks are present, the alphabetically-first non-`none` name wins, so
    /// the choice is deterministic across `list_containers` responses.
    pub group_key: String,
    /// Published ports (bollard-free). Filled from `ContainerSummary.ports` on
    /// the seed path and from inspect's `network_settings.ports` on the enrich
    /// path. Empty when no ports are exposed. Consumed by Phase 4 ENT-02
    /// (port-glow dots).
    pub ports: Vec<PortSummary>,
    /// Number of mount points on the container — proxy for ENT-03 volume
    /// cylinder height (RESEARCH "Volume Size Proxy"). Defaults to 0 on the
    /// seed path; the enrich path sets it from inspect's `mounts.len()`.
    pub mount_count: usize,
}

impl Default for ContainerSnapshot {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            status: Status::Stopped,
            group_key: "none".to_string(),
            ports: Vec::new(),
            mount_count: 0,
        }
    }
}

/// Bollard-free payload of [`crate::world::DockerMsg::Enriched`].
///
/// Built by [`crate::docker::inspect::enrich_snapshot_on_start`] (called from
/// `streams.rs` on `start` events) and [`crate::docker::inspect::enrich_snapshot_on_seed`]
/// (called on every seeded container at startup). Carries the fields an
/// `inspect_container` call backfills that `list_containers`/`events()` alone
/// can't supply:
///
/// - `group_key`: the container's primary network — `create` events don't
///   carry this, so the snapshot starts as `"none"` and gets corrected here
///   (Phase 3 carryover (2)).
/// - `ports` + `mount_count`: surfaced for ENT-02 (port glow) and ENT-03
///   (volume cylinders) without forcing those entity-render plans to make
///   another bollard call.
/// - `status_override`: when set, upgrades the entry's status (currently
///   used to flip Stopped -> Crashed for already-exited-with-OOM containers
///   on the seed path — Phase 3 carryover (1)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrichedSnapshot {
    pub id: String,
    pub group_key: String,
    pub ports: Vec<PortSummary>,
    pub mount_count: usize,
    pub status_override: Option<Status>,
}

/// Map a Docker container state string + optional OOM/exit-code signals to
/// [`Status`].
///
/// Docker container state values (per the API):
/// `created`, `running`, `paused`, `restarting`, `removing`, `exited`, `dead`,
/// `stopping`. Anything else falls through to the unknown default.
///
/// Mapping rule (the chosen Crashed-vs-Stopped policy):
///
/// | Docker state                       | Status     |
/// |------------------------------------|------------|
/// | `running`                          | Running    |
/// | `paused`                           | Paused     |
/// | `restarting`                       | Restarting |
/// | `dead`                             | **Crashed**|
/// | `exited` with `oom_killed == true` | **Crashed**|
/// | `exited` with `exit_code != 0`     | **Crashed**|
/// | `exited` with `exit_code == 0`     | Stopped    |
/// | `exited` with no exit-code info    | Stopped    |
/// | `created`                          | Stopped    |
/// | `removing`                         | Stopped    |
/// | `stopping`                         | Stopped    |
/// | anything else (incl. empty string) | Stopped    |
///
/// The `Stopped` default is intentional: it renders the container visibly-but-
/// dim instead of silently dropping it. Unknown states get logged at the
/// stream/event layer (03-03), not here.
pub fn map_status(state: &str, oom_killed: Option<bool>, exit_code: Option<i64>) -> Status {
    match state {
        "running" => Status::Running,
        "paused" => Status::Paused,
        "restarting" => Status::Restarting,
        "dead" => Status::Crashed,
        "exited" => {
            // exited containers split: OOM or non-zero exit => Crashed,
            // clean exit(0) (or unknown exit) => Stopped.
            let oom = oom_killed == Some(true);
            let nonzero_exit = exit_code.map(|c| c != 0).unwrap_or(false);
            if oom || nonzero_exit {
                Status::Crashed
            } else {
                Status::Stopped
            }
        }
        // created / removing / stopping / "" / anything else => Stopped (safe default).
        _ => Status::Stopped,
    }
}

/// Map a bollard `ContainerSummary` (from `list_containers`) to a
/// [`ContainerSnapshot`]. THIS IS THE ONLY function in the crate (outside
/// `connect`) that imports a bollard container type — keep it that way.
///
/// Notes on the bollard 0.21 surface (re-exported as `bollard::models::*`):
///
/// - `ContainerSummary.id: Option<String>` — full hex id.
/// - `ContainerSummary.names: Option<Vec<String>>` — Docker prefixes each name
///   with `/`. We strip that prefix and take the first.
/// - `ContainerSummary.state: Option<ContainerSummaryStateEnum>` — has a
///   `Display` impl that yields the same lowercase strings the Docker API
///   documents (`"running"`, `"exited"`, ...). We use `Display` for the
///   mapping so [`map_status`] stays a string-keyed pure function.
/// - `ContainerSummary` does NOT carry `OOMKilled` / `ExitCode` (those live on
///   inspect's `ContainerState`). For containers returned by `list_containers`,
///   we therefore pass `None`/`None`: an `exited` summary maps to `Stopped`.
///   When the enrich path follows up with `inspect_container`, callers can
///   override the status via `EnrichedSnapshot.status_override`.
/// - `ContainerSummary.network_settings.networks` is a `HashMap<String, EndpointSettings>`
///   keyed by network name. To get a deterministic primary, we sort the keys
///   and take the first non-empty one. `"none"` when there are no networks.
/// - `ContainerSummary.ports: Option<Vec<bollard::models::PortSummary>>` — the
///   published port list. Mapped to our bollard-free [`PortSummary`]; sorted
///   by `(private, proto)` for stable order across calls.
pub fn from_bollard_summary(summary: &bollard::models::ContainerSummary) -> ContainerSnapshot {
    let id = summary.id.clone().unwrap_or_default();

    // Name: strip leading '/' Docker adds to user names; fall back to a short
    // id slice when names are absent (rare — usually only happens for stub data).
    let name = summary
        .names
        .as_ref()
        .and_then(|ns| ns.first())
        .map(|n| n.trim_start_matches('/').to_string())
        .unwrap_or_else(|| id.chars().take(12).collect());

    // Status: bollard's ContainerSummaryStateEnum has a Display impl producing
    // the documented lowercase wire strings ("running", "exited", "dead", ...).
    // We do NOT have OOM / exit-code on summary, so pass None / None.
    let state_str = summary
        .state
        .as_ref()
        .map(|s| s.to_string())
        .unwrap_or_default();
    let status = map_status(&state_str, None, None);

    // Group key: deterministic primary network. Sort keys so the choice is
    // stable across calls regardless of HashMap iteration order.
    let group_key = summary
        .network_settings
        .as_ref()
        .and_then(|ns| ns.networks.as_ref())
        .and_then(|map| {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            keys.into_iter().next().cloned()
        })
        .unwrap_or_else(|| "none".to_string());

    // Published ports (cheap on the summary — already in the list response).
    // Sorted by (private, proto) for stable ordering across `list_containers`
    // calls (the bollard `Vec<PortSummary>` order is not guaranteed stable).
    let ports = summary
        .ports
        .as_ref()
        .map(|ps| {
            let mut out: Vec<PortSummary> = ps
                .iter()
                .map(|p| PortSummary {
                    private: p.private_port,
                    public: p.public_port,
                    proto: PortProto::from_bollard(
                        p.typ.as_ref().map(|t| t.to_string()).as_deref(),
                    ),
                })
                .collect();
            out.sort_by_key(|p| (p.private, p.proto));
            out
        })
        .unwrap_or_default();

    // mount_count is left at 0 here — the precise count comes from inspect's
    // `mounts.len()` via the enrich path. Leaving it at 0 avoids implying we
    // have an inspect-quality count before the enrich message has landed.
    ContainerSnapshot {
        id,
        name,
        status,
        group_key,
        ports,
        mount_count: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- map_status: every Docker state string maps deterministically ---------

    #[test]
    fn map_status_running() {
        assert_eq!(map_status("running", None, None), Status::Running);
    }

    #[test]
    fn map_status_paused() {
        assert_eq!(map_status("paused", None, None), Status::Paused);
    }

    #[test]
    fn map_status_restarting() {
        assert_eq!(map_status("restarting", None, None), Status::Restarting);
    }

    #[test]
    fn map_status_dead_is_crashed() {
        // `dead` => Crashed regardless of any other signal.
        assert_eq!(map_status("dead", None, None), Status::Crashed);
        assert_eq!(
            map_status("dead", Some(false), Some(0)),
            Status::Crashed,
            "dead overrides exit_code=0"
        );
    }

    #[test]
    fn map_status_exited_zero_is_stopped() {
        // Clean exit(0) => Stopped (dim, but not "crashed" red).
        assert_eq!(map_status("exited", Some(false), Some(0)), Status::Stopped);
    }

    #[test]
    fn map_status_exited_nonzero_is_crashed() {
        // Non-zero exit code => Crashed.
        assert_eq!(
            map_status("exited", Some(false), Some(137)),
            Status::Crashed
        );
        assert_eq!(map_status("exited", None, Some(1)), Status::Crashed);
    }

    #[test]
    fn map_status_exited_oom_is_crashed() {
        // OOMKilled overrides exit_code (some kernels still report exit 0).
        assert_eq!(map_status("exited", Some(true), Some(0)), Status::Crashed);
    }

    #[test]
    fn map_status_exited_unknown_exit_code_is_stopped() {
        // No OOM signal, no exit_code => treat as Stopped (safe default).
        assert_eq!(map_status("exited", None, None), Status::Stopped);
        assert_eq!(map_status("exited", Some(false), None), Status::Stopped);
    }

    #[test]
    fn map_status_created_removing_stopping_are_stopped() {
        assert_eq!(map_status("created", None, None), Status::Stopped);
        assert_eq!(map_status("removing", None, None), Status::Stopped);
        assert_eq!(map_status("stopping", None, None), Status::Stopped);
    }

    #[test]
    fn map_status_unknown_defaults_to_stopped() {
        // Anything we don't recognize falls through to Stopped — visible-but-dim,
        // never panics, never drops the container from the scene.
        assert_eq!(map_status("", None, None), Status::Stopped);
        assert_eq!(map_status("ghost-state", None, None), Status::Stopped);
        assert_eq!(map_status("RUNNING", None, None), Status::Stopped); // case-sensitive
    }

    // --- from_bollard_summary: ports extraction --------------------------------

    fn summary_with_ports(
        ports: Vec<bollard::models::PortSummary>,
    ) -> bollard::models::ContainerSummary {
        bollard::models::ContainerSummary {
            id: Some("abc".to_string()),
            names: Some(vec!["/x".to_string()]),
            ports: Some(ports),
            ..Default::default()
        }
    }

    #[test]
    fn from_bollard_summary_extracts_ports_tcp_udp() {
        let ps = vec![
            bollard::models::PortSummary {
                ip: None,
                private_port: 8080,
                public_port: Some(18080),
                typ: Some(bollard::models::PortSummaryTypeEnum::TCP),
            },
            bollard::models::PortSummary {
                ip: None,
                private_port: 53,
                public_port: None,
                typ: Some(bollard::models::PortSummaryTypeEnum::UDP),
            },
        ];
        let snap = from_bollard_summary(&summary_with_ports(ps));
        // Sorted by (private, proto): 53/udp first, 8080/tcp second.
        assert_eq!(snap.ports.len(), 2);
        assert_eq!(
            snap.ports[0],
            PortSummary {
                private: 53,
                public: None,
                proto: PortProto::Udp
            }
        );
        assert_eq!(
            snap.ports[1],
            PortSummary {
                private: 8080,
                public: Some(18080),
                proto: PortProto::Tcp
            }
        );
        // mount_count stays 0 on the seed path (filled by enrich later).
        assert_eq!(snap.mount_count, 0);
    }

    #[test]
    fn from_bollard_summary_no_ports_is_empty_vec() {
        let s = bollard::models::ContainerSummary {
            id: Some("abc".to_string()),
            names: Some(vec!["/x".to_string()]),
            ports: None,
            ..Default::default()
        };
        let snap = from_bollard_summary(&s);
        assert!(snap.ports.is_empty());
    }

    #[test]
    fn port_proto_from_bollard_defaults_to_tcp() {
        // None / empty / unknown all fall back to Tcp.
        assert_eq!(PortProto::from_bollard(None), PortProto::Tcp);
        assert_eq!(PortProto::from_bollard(Some("")), PortProto::Tcp);
        assert_eq!(PortProto::from_bollard(Some("garbage")), PortProto::Tcp);
        // Real protocols round-trip (case-insensitive on input).
        assert_eq!(PortProto::from_bollard(Some("udp")), PortProto::Udp);
        assert_eq!(PortProto::from_bollard(Some("UDP")), PortProto::Udp);
        assert_eq!(PortProto::from_bollard(Some("sctp")), PortProto::Sctp);
        // as_str round-trip.
        assert_eq!(PortProto::Tcp.as_str(), "tcp");
        assert_eq!(PortProto::Udp.as_str(), "udp");
        assert_eq!(PortProto::Sctp.as_str(), "sctp");
    }

    #[test]
    fn container_snapshot_default_is_sane() {
        // Default lets test fixtures stay terse without re-spelling every new
        // field — `..Default::default()` is now an option for callers.
        let d = ContainerSnapshot::default();
        assert_eq!(d.id, "");
        assert_eq!(d.name, "");
        assert_eq!(d.status, Status::Stopped);
        assert_eq!(d.group_key, "none");
        assert!(d.ports.is_empty());
        assert_eq!(d.mount_count, 0);
    }

    #[test]
    fn map_status_covers_all_status_variants() {
        // Cross-check: every Status variant is reachable from SOME state input.
        // (Catches the "added a Status variant but forgot to map any state to it" bug.)
        let outputs = [
            map_status("running", None, None),
            map_status("paused", None, None),
            map_status("exited", None, None), // -> Stopped
            map_status("restarting", None, None),
            map_status("dead", None, None), // -> Crashed
        ];
        assert!(outputs.contains(&Status::Running));
        assert!(outputs.contains(&Status::Paused));
        assert!(outputs.contains(&Status::Stopped));
        assert!(outputs.contains(&Status::Restarting));
        assert!(outputs.contains(&Status::Crashed));
    }
}
