//! Live data layer — the consumer side of the Docker stream (03-03).
//!
//! [`LiveWorld`] is the reconciliation engine: it consumes [`DockerMsg`]
//! values produced by `docker::streams::spawn_docker_tasks` and rebuilds a
//! [`World`] of live containers in the EXISTING [`Entity`] shape both
//! renderers already consume. It does NOT touch bollard or the renderers —
//! that wiring happens in 03-04.
//!
//! The two correctness pins this module owns:
//!
//! 1. **Stable per-container slot (CONT-05).** A given container id keeps the
//!    SAME slot — and therefore the SAME world-space position — for its whole
//!    lifetime, regardless of how many other containers come and go. The
//!    anti-teleport rule: when a container is removed, the rest of the rack
//!    does NOT shift around to fill the gap. Holes get reused by future
//!    arrivals (lowest-free slot), so the rack stays compact without churning.
//!
//! 2. **Same shape as `synthetic_scene()`.** We reuse [`layout()`],
//!    [`load_to_half_extent()`], and [`SceneBounds::from_entities`] verbatim
//!    — never duplicating those formulas. The renderers consume a `World` of
//!    `Entity` exactly as before; only the source of those entities changes.
//!
//! Slot scheme (the chosen design — see also 03-03-PLAN's design_decisions):
//!
//! - `groups: HashMap<String, u16>` — insertion-ordered group registry: the
//!   first network name seen becomes group 0, the next becomes 1, etc.
//! - `group_slots: Vec<Vec<Option<String>>>` — PER-GROUP slot vectors,
//!   indexed by group id. `group_slots[g][i] = Some(id)` means slot `i`
//!   within group `g` holds container `id`. A `None` is a freed hole that
//!   future SAME-group arrivals will reuse (lowest-free) — DIFFERENT-group
//!   containers cannot consume it (that would teleport a same-group neighbor).
//! - `id_to_addr: HashMap<String, (u16, usize)>` — reverse index for O(1)
//!   lookup of `(group_id, index_in_group)`.
//! - `Entity.id (u32)` = a flat slot index synthesized as `group * GROUP_STRIDE
//!   + index_in_group`. Stable per container id, unique while alive, fits u32.
//!
//! A container's position is `layout(group, index_in_group)` — exactly the
//! call `synthetic_scene()` makes. Because per-group slot indices are
//! insertion-ordered and never re-shuffled on removal, the position is stable
//! across churn (anti-teleport).

#![allow(dead_code)]

use std::collections::HashMap;

use glam::Vec3;

use crate::docker::stats::StatSample;
use crate::docker::ContainerSnapshot;
use crate::theme::Status;
use crate::world::entity::{load_to_half_extent, Entity};
use crate::world::layout::layout;
use crate::world::scene::SceneBounds;
use crate::world::World;

/// One typed message from the Docker data layer (the producer side, 03-03).
///
/// This is a bollard-free enum: all bollard types are mapped at the producer
/// (via `crate::docker::domain` and `crate::docker::stats`) before a
/// `DockerMsg` is sent. Downstream consumers (this module, the renderer wiring
/// in 03-04) never see bollard.
#[derive(Debug, Clone)]
pub enum DockerMsg {
    /// A container appeared — from the initial `list_containers` seed or from
    /// a subsequent `events()` create/start.
    Added(ContainerSnapshot),
    /// A container was destroyed / died — remove its entity and free its slot.
    Removed(String),
    /// A status transition (pause/unpause/restart/die) on an existing container.
    /// `Removed` is sent separately for full lifecycle ends.
    StatusChanged(String, Status),
    /// A normalized stats sample (the box's size source).
    Stat(String, StatSample),
}

/// Per-container live state — what we know about one container right now.
#[derive(Debug, Clone)]
struct LiveEntry {
    /// Snapshot at last add/event. `name` and `group_key` come from here;
    /// `status` is updated by [`DockerMsg::StatusChanged`] in place.
    snap: ContainerSnapshot,
    /// Latest normalized load. `None` until the first non-warming-up
    /// [`DockerMsg::Stat`] arrives — the entity is sized to `MIN_HALF`
    /// until then so it doesn't snap when the first real sample lands.
    load: Option<f32>,
}

/// Stride used to synthesize a stable `Entity.id: u32` from a
/// `(group_id, index_in_group)` pair. With `GROUP_STRIDE = 1<<16`, the low 16
/// bits are the per-group index and the high 16 bits are the group id —
/// matching the `u16` group id naturally and fitting `u32` without overflow
/// for any plausible container count.
const GROUP_STRIDE: u32 = 1 << 16;

/// Half-extent for non-Running containers that have no live stats stream
/// (Stopped/Paused/Restarting/Crashed never emit a Stat sample, so their
/// `load` is permanently `None`). Picked big enough to read clearly as a
/// wireframe in a deep multi-group scene where `frame_scene` pulls the
/// camera back to fit several network bands. With `MIN_HALF=0.3` and
/// `MAX_HALF=1.2`, `0.85` is comfortably mid-range — still visibly smaller
/// than a max-loaded running box, so live load is the dominant size signal.
const BASELINE_HALF_NO_LOAD: f32 = 0.85;

/// Live reconciliation state.
///
/// Apply [`DockerMsg`]s with [`LiveWorld::apply`] to keep the entity set in
/// sync with the live container population, rebuilding a [`World`] on each
/// change so the renderer can swap it in.
pub struct LiveWorld {
    /// Group registry: first network name seen -> group 0, next -> 1, etc.
    /// Stable insertion order (never re-shuffled) so groups stay put when a
    /// container in another group disappears.
    groups: HashMap<String, u16>,
    /// Per-group slot vectors, indexed by group id. `group_slots[g][i]` is
    /// `Some(id)` when slot `i` of group `g` is occupied, `None` when it's a
    /// freed hole that a future SAME-group arrival will reuse.
    group_slots: Vec<Vec<Option<String>>>,
    /// Reverse index: container id -> (group_id, index_in_group). O(1)
    /// lookup on `Removed`, `StatusChanged`, and `Stat`.
    id_to_addr: HashMap<String, (u16, usize)>,
    /// Per-id live state (status + latest load).
    entries: HashMap<String, LiveEntry>,
}

impl Default for LiveWorld {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveWorld {
    /// Empty live world — no entities, no slots, no groups.
    pub fn new() -> Self {
        Self {
            groups: HashMap::new(),
            group_slots: Vec::new(),
            id_to_addr: HashMap::new(),
            entries: HashMap::new(),
        }
    }

    /// Number of currently-live containers (for tests / introspection).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` when there are no live containers.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Apply one [`DockerMsg`] and rebuild the [`World`] if the scene changed.
    ///
    /// Returns `Some(World)` whenever the entity set or any entity's
    /// status/size changed (so the renderer can swap it in). Returns `None`
    /// only for messages that are pure no-ops (e.g. `StatusChanged` to the
    /// same status, or `Stat`/`StatusChanged` for an unknown container —
    /// which can happen on race-window ordering between events and stats).
    pub fn apply(&mut self, msg: DockerMsg) -> Option<World> {
        let changed = match msg {
            DockerMsg::Added(snap) => self.handle_added(snap),
            DockerMsg::Removed(id) => self.handle_removed(&id),
            DockerMsg::StatusChanged(id, status) => self.handle_status(&id, status),
            DockerMsg::Stat(id, sample) => self.handle_stat(&id, sample),
        };

        if changed {
            Some(self.build_world())
        } else {
            None
        }
    }

    // --- Message handlers (all return `true` iff the scene actually changed) -

    fn handle_added(&mut self, snap: ContainerSnapshot) -> bool {
        // Idempotent re-add: a re-emitted seed shouldn't allocate a fresh
        // slot. Refresh the snapshot (name/status may have changed since) but
        // keep the existing (group, index) and load — the box stays put.
        if self.id_to_addr.contains_key(&snap.id) {
            let entry = self
                .entries
                .get_mut(&snap.id)
                .expect("id_to_addr and entries kept in lockstep");
            entry.snap = snap;
            return true;
        }

        // Fresh container: register the group (insertion-ordered) and find
        // the lowest-free slot WITHIN THAT GROUP. Per-group slot space is the
        // anti-teleport invariant: a container's index_in_group never shifts
        // because some other group's container left.
        let group_id = self.ensure_group(&snap.group_key);
        let g = group_id as usize;
        let index_in_group = match self.group_slots[g].iter().position(|s| s.is_none()) {
            Some(idx) => {
                self.group_slots[g][idx] = Some(snap.id.clone());
                idx
            }
            None => {
                self.group_slots[g].push(Some(snap.id.clone()));
                self.group_slots[g].len() - 1
            }
        };

        self.id_to_addr.insert(snap.id.clone(), (group_id, index_in_group));
        self.entries
            .insert(snap.id.clone(), LiveEntry { snap, load: None });
        true
    }

    fn handle_removed(&mut self, id: &str) -> bool {
        let Some((group_id, index_in_group)) = self.id_to_addr.remove(id) else {
            // Unknown id — race-window between events and stats; nothing to do.
            return false;
        };
        // CRITICAL: only NULL OUT this group's slot — leave the rest of the
        // group's slot vector untouched. Same-group neighbors keep their
        // index_in_group, so their world-space position is unchanged.
        self.group_slots[group_id as usize][index_in_group] = None;
        self.entries.remove(id);
        true
    }

    fn handle_status(&mut self, id: &str, status: Status) -> bool {
        let Some(entry) = self.entries.get_mut(id) else {
            // Unknown id — see handle_removed. Skip.
            return false;
        };
        if entry.snap.status == status {
            return false;
        }
        entry.snap.status = status;
        true
    }

    fn handle_stat(&mut self, id: &str, sample: StatSample) -> bool {
        let Some(entry) = self.entries.get_mut(id) else {
            return false;
        };
        // Warming-up samples: do NOT overwrite a real previous load with the
        // zero from a fresh warming-up frame, and do NOT snap the box from
        // its floor size to anything at all. Per PITFALLS Pitfall 1, the
        // first sample is garbage — but `StatSample::warming_up` already
        // forces `load = 0.0` on the producer side, so we just skip it.
        if sample.warming_up {
            return false;
        }
        // The load field is already in [0, 1], non-finite-scrubbed by
        // normalize(). Hand it to the existing size map unchanged.
        let new = sample.load.clamp(0.0, 1.0);
        if entry.load == Some(new) {
            // No visible change — skip the World rebuild.
            return false;
        }
        entry.load = Some(new);
        true
    }

    // --- Helpers --------------------------------------------------------------

    /// Register a network name in the insertion-ordered group registry and
    /// return its group id. Stable: a given key always maps to the same id.
    /// Also makes sure `group_slots` has a slot vector for the new group.
    fn ensure_group(&mut self, key: &str) -> u16 {
        if let Some(&g) = self.groups.get(key) {
            return g;
        }
        // Group ids are assigned in insertion order — `groups.len()` gives the
        // next free id, fitting in u16 (we will never have >65535 networks in
        // a single Docker host; layout would frame poorly long before that).
        let next: u16 = self.groups.len() as u16;
        self.groups.insert(key.to_string(), next);
        // Keep `group_slots` in lockstep with `groups`: the new group starts
        // with an empty slot vector. Adds within this group will fill it.
        self.group_slots.push(Vec::new());
        next
    }

    /// Build the [`World`] from the current live state. Pure assembly: every
    /// entity reuses [`layout()`] and [`load_to_half_extent()`] verbatim, and
    /// the bounds come from the existing [`SceneBounds::from_entities`].
    fn build_world(&self) -> World {
        let mut entities: Vec<Entity> = Vec::with_capacity(self.entries.len());

        // Walk groups in registration order, then slots in index order, so
        // the world list is deterministic across rebuilds.
        for (group_id, slots) in self.group_slots.iter().enumerate() {
            let group_id = group_id as u16;
            for (index_in_group, occupant) in slots.iter().enumerate() {
                let Some(id) = occupant else { continue };
                let Some(entry) = self.entries.get(id) else { continue };

                // Size:
                //   - With a live load -> existing sqrt-compressive load map.
                //   - No load yet AND status == Running (warming-up, no first
                //     non-warming sample) -> floor (MIN_HALF) so it stays small
                //     until a real sample arrives, then breathes up.
                //   - No load and status != Running (Stopped/Paused/etc — these
                //     statuses never get a stats stream, so `load` is always
                //     None) -> a fixed BASELINE_HALF chosen big enough that
                //     wireframe boxes stay legible even in a deep multi-group
                //     scene. Wireframe is rendered downstream by status, so the
                //     box still reads as "off" — it's just a visible outline.
                let half = match (entry.load, entry.snap.status) {
                    (Some(load), _) => load_to_half_extent(load),
                    (None, Status::Running) => load_to_half_extent(0.0),
                    (None, _) => BASELINE_HALF_NO_LOAD,
                };

                entities.push(Entity {
                    // Flat stable id encoding (group << 16 | index_in_group).
                    // Unique while the container is alive, fits u32, and
                    // doesn't shift on neighbor churn — anti-teleport even
                    // through Entity.id.
                    id: (group_id as u32) * GROUP_STRIDE + index_in_group as u32,
                    position: layout(group_id, index_in_group as u32),
                    half_extents: Vec3::splat(half),
                    status: entry.snap.status,
                    group: group_id,
                });
            }
        }

        let bounds = SceneBounds::from_entities(&entities);
        World { entities, bounds }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::entity::{MAX_HALF, MIN_HALF};

    fn snap(id: &str, group: &str, status: Status) -> ContainerSnapshot {
        ContainerSnapshot {
            id: id.to_string(),
            name: id.to_string(),
            status,
            group_key: group.to_string(),
        }
    }

    fn running_load(load: f32) -> StatSample {
        // Synthesize a non-warming-up sample with a precise `load` value.
        // Other fields are not consumed by LiveWorld for sizing.
        StatSample {
            cpu_pct: 0.0,
            mem_used: 0,
            mem_limit: 0,
            mem_fraction: 0.0,
            load,
            warming_up: false,
        }
    }

    fn position_of(world: &World, entity_id: u32) -> Vec3 {
        world
            .entities
            .iter()
            .find(|e| e.id == entity_id)
            .unwrap_or_else(|| panic!("entity id {entity_id} not in world"))
            .position
    }

    /// Synthesize the Entity.id for a given (group, index_in_group) pair —
    /// mirror of the formula in `LiveWorld::build_world`.
    fn eid(group: u16, idx: u32) -> u32 {
        (group as u32) * GROUP_STRIDE + idx
    }

    /// Done criterion: Added then Stat(load=1.0) -> entity exists, half_extent
    /// near MAX_HALF (the existing size map applied to load 1.0).
    #[test]
    fn add_then_stat_sizes_box() {
        let mut lw = LiveWorld::new();
        let w0 = lw.apply(DockerMsg::Added(snap("a", "net0", Status::Running)));
        let w0 = w0.expect("Added must produce a World");
        assert_eq!(w0.entities.len(), 1);
        // Pre-stat: at the floor size (MIN_HALF), not zero, not NaN.
        assert!((w0.entities[0].half_extents.x - MIN_HALF).abs() < 1e-6);
        // a took group 0 / slot 0.
        assert_eq!(w0.entities[0].id, eid(0, 0));

        let w1 = lw.apply(DockerMsg::Stat("a".to_string(), running_load(1.0)));
        let w1 = w1.expect("Stat must trigger a rebuild on first non-warming sample");
        let h = w1.entities[0].half_extents.x;
        assert!((h - MAX_HALF).abs() < 1e-6, "load 1.0 -> MAX_HALF, got {h}");
    }

    /// Done criterion: remove a middle container; the remaining containers
    /// keep their EXACT positions (the anti-teleport pin, CONT-05).
    #[test]
    fn removed_frees_slot_others_stable() {
        let mut lw = LiveWorld::new();
        lw.apply(DockerMsg::Added(snap("a", "net0", Status::Running)));
        lw.apply(DockerMsg::Added(snap("b", "net0", Status::Running)));
        let w_full = lw
            .apply(DockerMsg::Added(snap("c", "net0", Status::Running)))
            .expect("Added must produce a World");
        // a, b, c took group 0's slots 0, 1, 2 (lowest-free within group).
        let pos_a = position_of(&w_full, eid(0, 0));
        let pos_c = position_of(&w_full, eid(0, 2));

        let w_after = lw
            .apply(DockerMsg::Removed("b".to_string()))
            .expect("Removed must produce a World");
        assert_eq!(w_after.entities.len(), 2, "b is gone");
        // a kept group-0/slot-0; c kept group-0/slot-2 — NO teleport.
        assert_eq!(
            position_of(&w_after, eid(0, 0)),
            pos_a,
            "a teleported on b's removal"
        );
        assert_eq!(
            position_of(&w_after, eid(0, 2)),
            pos_c,
            "c teleported on b's removal"
        );
        // b's slot is truly gone from the world.
        assert!(
            w_after.entities.iter().all(|e| e.id != eid(0, 1)),
            "b's slot (group-0/slot-1) leaked into world"
        );
    }

    /// Done criterion: a container's id keeps the SAME position across a long
    /// churn of unrelated arrivals and removals.
    #[test]
    fn id_keeps_slot_across_churn() {
        let mut lw = LiveWorld::new();
        lw.apply(DockerMsg::Added(snap("anchor", "net0", Status::Running)));
        let pos_initial = position_of(
            &lw.apply(DockerMsg::Stat("anchor".to_string(), running_load(0.4)))
                .expect("Stat triggers rebuild"),
            eid(0, 0),
        );

        // Churn: add 10 containers, remove every other one, add 5 more.
        for i in 0..10 {
            lw.apply(DockerMsg::Added(snap(&format!("c{i}"), "net0", Status::Running)));
        }
        for i in (0..10).step_by(2) {
            lw.apply(DockerMsg::Removed(format!("c{i}")));
        }
        let mut w_final = None;
        for i in 0..5 {
            w_final = lw.apply(DockerMsg::Added(snap(
                &format!("late{i}"),
                "net0",
                Status::Running,
            )));
        }
        let w_final = w_final.expect("at least one Added produces a World");

        // Anchor still at its original slot/position.
        let pos_now = position_of(&w_final, eid(0, 0));
        assert_eq!(
            pos_now, pos_initial,
            "anchor teleported across churn: {pos_initial:?} -> {pos_now:?}"
        );
    }

    /// Done criterion: removing a middle container and adding a new one
    /// reuses the freed hole (lowest-free) — the rack stays compact and
    /// doesn't grow unboundedly.
    #[test]
    fn reused_hole_keeps_rack_compact() {
        let mut lw = LiveWorld::new();
        lw.apply(DockerMsg::Added(snap("a", "net0", Status::Running)));
        lw.apply(DockerMsg::Added(snap("b", "net0", Status::Running)));
        lw.apply(DockerMsg::Added(snap("c", "net0", Status::Running)));
        lw.apply(DockerMsg::Removed("b".to_string()));

        let w = lw
            .apply(DockerMsg::Added(snap("d", "net0", Status::Running)))
            .expect("Added must produce a World");

        // d took group-0 slot 1 (the freed hole). The world has exactly 3
        // entities and the slot indices within group 0 are {0, 1, 2} —
        // not {0, 2, 3}.
        let mut ids: Vec<u32> = w.entities.iter().map(|e| e.id).collect();
        ids.sort_unstable();
        assert_eq!(
            ids,
            vec![eid(0, 0), eid(0, 1), eid(0, 2)],
            "rack grew instead of reusing hole"
        );
    }

    /// Done criterion: a warming-up first sample (load 0.0) leaves the box at
    /// the floor size; the box does NOT snap to zero or NaN.
    #[test]
    fn warming_up_does_not_snap() {
        let mut lw = LiveWorld::new();
        lw.apply(DockerMsg::Added(snap("a", "net0", Status::Running)));
        let warming = StatSample {
            cpu_pct: 0.0,
            mem_used: 0,
            mem_limit: 0,
            mem_fraction: 0.0,
            load: 0.0,
            warming_up: true,
        };
        // Warming-up sample is a no-op; world is not rebuilt because nothing
        // visible changed. The (post-Added) state still has the floor box.
        assert!(lw.apply(DockerMsg::Stat("a".to_string(), warming)).is_none());

        // Re-read by issuing a status no-op? Easier: re-derive via a real
        // non-warming Stat with load=0.0 to ensure the floor is preserved.
        // (But that wouldn't be a no-op if load just transitioned None->0.0.)
        // Instead just sanity-check via the next observable: a real Stat with
        // 0.0 should still sit at MIN_HALF.
        let w = lw
            .apply(DockerMsg::Stat("a".to_string(), running_load(0.0)))
            .expect("Stat must trigger rebuild on first non-warming sample");
        let h = w.entities[0].half_extents.x;
        assert!(h.is_finite() && h > 0.0, "h must be finite/positive, got {h}");
        assert!((h - MIN_HALF).abs() < 1e-6, "load 0.0 -> MIN_HALF, got {h}");
    }

    /// Done criterion: removing the last container yields a `World` with 0
    /// entities and finite degenerate bounds — never panics.
    #[test]
    fn empty_set_is_safe() {
        let mut lw = LiveWorld::new();
        lw.apply(DockerMsg::Added(snap("only", "net0", Status::Running)));
        let w = lw
            .apply(DockerMsg::Removed("only".to_string()))
            .expect("Removed must produce a World even when emptying");
        assert!(w.entities.is_empty());
        // SceneBounds::from_entities returns zero/zero/zero degenerate bounds
        // for an empty input — assert finite, not panicking.
        assert!(w.bounds.center.x.is_finite() && w.bounds.radius.is_finite());
        assert_eq!(w.bounds.radius, 0.0);
    }

    /// Done criterion: `StatusChanged` updates an entity's `Status` IN PLACE
    /// without moving it (anti-teleport invariant also applies to color
    /// changes).
    #[test]
    fn status_change_recolors() {
        let mut lw = LiveWorld::new();
        lw.apply(DockerMsg::Added(snap("a", "net0", Status::Running)));
        let w0 = lw
            .apply(DockerMsg::Added(snap("b", "net0", Status::Running)))
            .expect("Added must produce a World");
        let pos_a_before = position_of(&w0, eid(0, 0));
        let pos_b_before = position_of(&w0, eid(0, 1));

        let w1 = lw
            .apply(DockerMsg::StatusChanged("a".to_string(), Status::Paused))
            .expect("StatusChanged must trigger rebuild on a real change");
        assert_eq!(
            position_of(&w1, eid(0, 0)),
            pos_a_before,
            "a moved on status change"
        );
        assert_eq!(
            position_of(&w1, eid(0, 1)),
            pos_b_before,
            "b moved on a's status change"
        );
        let a = w1.entities.iter().find(|e| e.id == eid(0, 0)).unwrap();
        assert_eq!(a.status, Status::Paused);

        // Idempotent: the same status -> None (no rebuild).
        assert!(lw
            .apply(DockerMsg::StatusChanged("a".to_string(), Status::Paused))
            .is_none());
    }

    /// Sanity: messages for an unknown container id are silent no-ops, not
    /// panics. This guards the events <-> stats race-window where a `Stat`
    /// for a container may arrive after its `Removed`.
    #[test]
    fn unknown_id_messages_are_noop() {
        let mut lw = LiveWorld::new();
        assert!(lw.apply(DockerMsg::Removed("ghost".to_string())).is_none());
        assert!(lw
            .apply(DockerMsg::StatusChanged("ghost".to_string(), Status::Crashed))
            .is_none());
        assert!(lw
            .apply(DockerMsg::Stat("ghost".to_string(), running_load(0.5)))
            .is_none());
        assert_eq!(lw.len(), 0);
    }

    /// Multi-group sanity: containers in different network groups sit on
    /// different Z-bands (the position derives via the network-grouped
    /// layout, same as `synthetic_scene`).
    #[test]
    fn distinct_groups_get_distinct_z_bands() {
        let mut lw = LiveWorld::new();
        let w0 = lw
            .apply(DockerMsg::Added(snap("a", "net0", Status::Running)))
            .unwrap();
        let pa = position_of(&w0, eid(0, 0));
        let w1 = lw
            .apply(DockerMsg::Added(snap("b", "net1", Status::Running)))
            .unwrap();
        // b is the first container in group 1, so it sits at group-1/slot-0.
        let pb = position_of(&w1, eid(1, 0));
        assert!(
            (pa.z - pb.z).abs() > 1e-3,
            "different groups should sit on different z bands: {pa:?} vs {pb:?}"
        );
    }
}
