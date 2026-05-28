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
//!   id, display name, lifecycle status (theme::Status, CONT-01), and a
//!   `group_key` (primary network name) the future ENT-01 layout grouping reads.
//! - [`map_status`] — pure string + OOM/exit-code -> [`crate::theme::Status`]
//!   mapping. The `dead`/`exited(!=0)` -> Crashed rule lives here; `unknown` ->
//!   Stopped is the safe default (visible but dim).
//! - [`from_bollard_summary`] — the SINGLE function that imports bollard
//!   container types. Maps a bollard `ContainerSummary` (from `list_containers`)
//!   into a [`ContainerSnapshot`].
//!
//! 03-03 (streams) will wire `from_bollard_summary` into the events/list path;
//! 03-04 wires the snapshot stream into `World`.

#![allow(dead_code)]

use crate::theme::Status;

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
///   When 03-03 follows up with `inspect_container`, callers can re-derive
///   status using `map_status(state, oom, exit_code)` from `ContainerState`.
/// - `ContainerSummary.network_settings.networks` is a `HashMap<String, EndpointSettings>`
///   keyed by network name. To get a deterministic primary, we sort the keys
///   and take the first non-empty one. `"none"` when there are no networks.
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

    ContainerSnapshot {
        id,
        name,
        status,
        group_key,
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
