//! Live data layer — the consumer side of the Docker stream (03-03 + 04-01).
//!
//! [`LiveWorld`] is the reconciliation engine: it consumes [`DockerMsg`]
//! values produced by `docker::streams::spawn_docker_tasks` and rebuilds a
//! [`World`] of live containers in the EXISTING [`Entity`] shape both
//! renderers already consume. It does NOT touch bollard or the renderers —
//! that wiring happens in 03-04.
//!
//! ## Per-tick easing pass (04-01)
//!
//! Two cadences live here, deliberately separated:
//!
//! - **[`LiveWorld::apply`]** is the TOPOLOGY path. `Added` / `Removed` /
//!   `StatusChanged` change the entity set (or an entity's color), so they
//!   rebuild the World; `Stat` just records a NEW TARGET on the entry and
//!   returns `None` — no World rebuild on Stat.
//! - **[`LiveWorld::dress`]** is the per-FRAME size easing pass. It walks the
//!   live entries, eases each `displayed_half` toward its `target_half` via
//!   [`critically_damped`], and MUTATES `entity.half_extents` in place on the
//!   passed slice. No allocation, no `Vec` rebuild.
//!
//! This split closes RESEARCH **Pitfall A** ("Easing the size means EVERY
//! tick rebuilds the World"). The slot/group/status fields stay topology;
//! the size source decouples from the rebuild path. Both renderers
//! (braille `App::on_tick`, kitty `run_kitty` loop) call `dress(dt)` once
//! per frame with REAL elapsed dt, so the box's size sweeps smoothly between
//! ~1Hz stat samples — boxes BREATHE rather than snap.
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
use crate::docker::{ContainerSnapshot, EnrichedSnapshot};
use crate::theme::Status;
// Per-frame easing pass (CONT-03 / 04-01) — `dress(dt)` is the per-tick
// size easing path defined below; both renderers call it once per frame.
use crate::world::easing::{critically_damped, BREATHING_HALF_LIFE};
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
    /// Backfilled fields from an off-thread `inspect_container` call (Phase 4
    /// 04-02). Produced by `docker::inspect::enrich_snapshot_{on_start,on_seed}`.
    /// May MIGRATE the container's slot when `group_key` differs from what the
    /// seed/create path assigned (Phase 3 carryover (3)); updates ports +
    /// mount_count in place; optionally upgrades the entry's status via
    /// `status_override` (Phase 3 carryover (1)).
    Enriched(EnrichedSnapshot),
}

/// Per-container live state — what we know about one container right now.
///
/// CONT-03 split (04-01): three size fields live here.
///
/// - [`target_load`][LiveEntry::target_load] is the latest STAT value. Set by
///   [`DockerMsg::Stat`] (topology cadence ~1Hz per container). The size map
///   converts it to a target half-extent.
/// - [`displayed_half`][LiveEntry::displayed_half] is the current EASED size,
///   advanced by [`LiveWorld::dress`] every render tick (~30Hz). It's what
///   ends up in `Entity.half_extents` — the renderer reads only this.
/// - [`vel_half`][LiveEntry::vel_half] is the spring velocity. Kept across
///   `dress()` calls so a mid-flight target reversal doesn't lurch.
#[derive(Debug, Clone)]
struct LiveEntry {
    /// Snapshot at last add/event. `name` and `group_key` come from here;
    /// `status` is updated by [`DockerMsg::StatusChanged`] in place.
    snap: ContainerSnapshot,
    /// Latest normalized load TARGET (was `load` pre-04-01). `None` until the
    /// first non-warming-up [`DockerMsg::Stat`] arrives — `dress()` then
    /// eases toward `MIN_HALF` (Running) or `BASELINE_HALF_NO_LOAD` (non-Running).
    target_load: Option<f32>,
    /// Current EASED half-extent. Mutated by `dress(dt)`; seeded at insert
    /// time so the FIRST frame post-Added shows the floor-sized box and the
    /// very next tick begins easing toward the target.
    displayed_half: f32,
    /// Spring velocity for the eased half-extent. Crosses target reversals
    /// smoothly so a CPU spike-then-drop doesn't strobe the box size.
    vel_half: f32,
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

    /// Apply one [`DockerMsg`] and rebuild the [`World`] if the TOPOLOGY changed.
    ///
    /// Returns `Some(World)` for entity-set / status changes (Added / Removed
    /// / StatusChanged — color is topology too because the renderer keys
    /// solid-vs-wireframe off `Entity.status`). Returns `None` for:
    /// - `Stat` messages (post-04-01: Stat sets a TARGET only; the per-frame
    ///   [`dress`][LiveWorld::dress] pass eases the box size without
    ///   reallocating the World — RESEARCH Pitfall A);
    /// - same-state no-ops (StatusChanged to the same status, warming-up
    ///   Stat samples, Stat with an unchanged load);
    /// - unknown ids (race-window between events and stats).
    pub fn apply(&mut self, msg: DockerMsg) -> Option<World> {
        let changed = match msg {
            DockerMsg::Added(snap) => self.handle_added(snap),
            DockerMsg::Removed(id) => self.handle_removed(&id),
            DockerMsg::StatusChanged(id, status) => self.handle_status(&id, status),
            DockerMsg::Stat(id, sample) => self.handle_stat(&id, sample),
            DockerMsg::Enriched(enr) => self.handle_enriched(enr),
        };

        if changed {
            Some(self.build_world())
        } else {
            None
        }
    }

    /// Read-only access to a live entry's [`ContainerSnapshot`] by container id.
    ///
    /// Used by Phase 4 ENT-02 (port glow) and ENT-03 (volume cylinders) to
    /// read `ports` / `mount_count` directly from the live entry without
    /// re-fetching from Docker — both fields are seeded by `from_bollard_summary`
    /// and refreshed by [`DockerMsg::Enriched`].
    pub fn snapshot(&self, id: &str) -> Option<&ContainerSnapshot> {
        self.entries.get(id).map(|e| &e.snap)
    }

    /// Reverse lookup: given an [`Entity::id`](crate::world::entity::Entity::id)
    /// (`group << 16 | index_in_group`), return the live container id string,
    /// or `None` if no entity has that slot right now.
    ///
    /// Used by 04-03's `apply_input_action` to turn a `Selection::selected_id`
    /// (an entity id) into the docker container id that 04-06's
    /// `Effect::SpawnInspect` carries to `docker::inspect::fetch_detail`.
    /// O(n) walk over `id_to_addr` — bounded by the live container count,
    /// microseconds at the scales we render.
    pub fn id_string_for_entity(&self, entity_id: u32) -> Option<&str> {
        let group_id = (entity_id / GROUP_STRIDE) as u16;
        let index_in_group = (entity_id % GROUP_STRIDE) as usize;
        for (id, &(g, i)) in self.id_to_addr.iter() {
            if g == group_id && i == index_in_group {
                return Some(id.as_str());
            }
        }
        None
    }

    // --- Message handlers (all return `true` iff the scene actually changed) -

    fn handle_added(&mut self, snap: ContainerSnapshot) -> bool {
        // Idempotent re-add: a re-emitted seed shouldn't allocate a fresh
        // slot. Refresh the snapshot (name/status may have changed since) but
        // keep the existing (group, index), target, displayed and velocity —
        // the box stays put and mid-ease.
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

        // Seed the displayed size: floor (MIN_HALF) for a Running container so
        // the first stat post-warmup eases UP from the floor (visible breath),
        // BASELINE_HALF_NO_LOAD for non-Running (which never gets a stats
        // stream). vel_half starts at 0 (rest).
        let displayed_half = if matches!(snap.status, Status::Running) {
            load_to_half_extent(0.0)
        } else {
            BASELINE_HALF_NO_LOAD
        };

        self.id_to_addr.insert(snap.id.clone(), (group_id, index_in_group));
        self.entries.insert(
            snap.id.clone(),
            LiveEntry {
                snap,
                target_load: None,
                displayed_half,
                vel_half: 0.0,
            },
        );
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
        let was_running = matches!(entry.snap.status, Status::Running);
        let is_running = matches!(status, Status::Running);
        entry.snap.status = status;

        // Seed the target on the Running <-> non-Running edge so the next
        // dress() pass eases toward the right size bucket immediately —
        // otherwise a freshly-Stopped Running box would keep its stale
        // target_load and dress() would still ease toward MAX_HALF until a
        // (never-arriving) Stat reset it. On non-Running -> Running, clear
        // target_load so dress() snaps to the MIN_HALF floor until the first
        // stat sample lands; that mirrors the Added seeding rule.
        if was_running != is_running {
            entry.target_load = None;
        }
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
        if entry.target_load == Some(new) {
            // No change to the target — dress() keeps easing toward the same
            // value naturally. No World rebuild.
            return false;
        }
        entry.target_load = Some(new);
        // 04-01: Stat sets a TARGET only — the per-frame dress() pass eases
        // the box size in place. Never rebuild the World on Stat (RESEARCH
        // Pitfall A: per-tick rebuild on continuous easing).
        false
    }

    /// Handle an [`EnrichedSnapshot`] from an off-thread `inspect_container`
    /// call (04-02 / Phase 3 carryovers).
    ///
    /// Three effects, in order:
    ///
    /// 1. **In-place field updates.** `ports` and `mount_count` always update
    ///    on the live entry's `ContainerSnapshot` — no slot migration, no
    ///    position change. These feed Phase 4 ENT-02 / ENT-03.
    /// 2. **status_override.** When set (currently only `Some(Crashed)` from
    ///    `enrich_snapshot_on_seed` for exited-OOM / exited-nonzero
    ///    containers), promote the entry's status — Phase 3 carryover (1).
    /// 3. **Slot migration.** When `group_key` differs from what the entry
    ///    currently has, MIGRATE the slot to the new group: free the old
    ///    slot (same as Removed), allocate the lowest-free in the new group,
    ///    update `id_to_addr`, refresh `snap.group_key`. Other containers in
    ///    the OLD or NEW group keep their `(group, index)` — anti-teleport
    ///    invariant preserved (CONT-05 still holds across network changes).
    ///
    /// Returns `true` whenever any topology-visible change happened (status
    /// upgrade, slot migration, or ports/mounts refresh) — the renderer needs
    /// the World rebuilt to see the new fields. Unknown ids (race window
    /// where the container was removed between the inspect dispatch and the
    /// inspect result) are silent no-ops, never panics.
    fn handle_enriched(&mut self, enr: EnrichedSnapshot) -> bool {
        // Lookup current address — `None` means the container was removed
        // before the off-thread inspect could complete. Best-effort: drop it.
        let Some(&(old_group, old_idx)) = self.id_to_addr.get(&enr.id) else {
            return false;
        };

        // 1. In-place updates to fields that don't require slot migration.
        if let Some(entry) = self.entries.get_mut(&enr.id) {
            entry.snap.ports = enr.ports.clone();
            entry.snap.mount_count = enr.mount_count;
            // 2. status_override — currently only Stopped -> Crashed.
            if let Some(s) = enr.status_override {
                if entry.snap.status != s {
                    entry.snap.status = s;
                }
            }
        }

        // 3. Slot migration when the group_key changed (Phase 3 carryover (3)).
        let current_group_key = self.entries[&enr.id].snap.group_key.clone();
        if current_group_key == enr.group_key {
            // Same group — only ports/mounts/status may have changed, but
            // those ARE topology-visible (the renderer needs the rebuild so
            // ENT-02/ENT-03 see the new ports / mount_count, and any status
            // override changes the wireframe/solid choice).
            return true;
        }

        // Free the old slot — like handle_removed, only null this group's
        // slot. Same-group neighbors keep their positions.
        self.group_slots[old_group as usize][old_idx] = None;

        // Allocate the new slot in the (possibly new) group.
        let new_group = self.ensure_group(&enr.group_key);
        let g = new_group as usize;
        let new_idx = match self.group_slots[g].iter().position(|s| s.is_none()) {
            Some(i) => {
                self.group_slots[g][i] = Some(enr.id.clone());
                i
            }
            None => {
                self.group_slots[g].push(Some(enr.id.clone()));
                self.group_slots[g].len() - 1
            }
        };
        self.id_to_addr.insert(enr.id.clone(), (new_group, new_idx));
        if let Some(entry) = self.entries.get_mut(&enr.id) {
            entry.snap.group_key = enr.group_key.clone();
        }
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

                // Size: read the CURRENT eased value from the entry. apply()
                // is the topology cadence (Added/Removed/StatusChanged); the
                // per-frame easing lives in `dress(dt, &mut entities)`. On the
                // very first frame after an Added, displayed_half is the
                // seeded floor (Running) or baseline (non-Running), so the
                // FIRST stat eases UP from the floor — that's the breath.
                let half = entry.displayed_half;

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

    /// Per-tick easing pass (CONT-03 / 04-01).
    ///
    /// Walks the live entries, eases each entry's `displayed_half` toward its
    /// target half-extent via the critically-damped spring, and MUTATES the
    /// matching `Entity.half_extents` in `entities` IN PLACE — no allocation,
    /// no `Vec` rebuild. Call this from the render-tick path
    /// (`App::on_tick` for braille, `run_kitty`'s main loop for kitty) with
    /// REAL elapsed dt.
    ///
    /// The target half is computed from `(target_load, status)` exactly as
    /// the old `build_world` did:
    /// - `(Some(load), _)`     -> `load_to_half_extent(load)` (sqrt-compressive map)
    /// - `(None, Running)`     -> `load_to_half_extent(0.0)` (== MIN_HALF floor)
    /// - `(None, non-Running)` -> `BASELINE_HALF_NO_LOAD`
    ///
    /// dt non-finite or `<= 0` is treated as 0 (matches `App::on_tick`'s
    /// existing dt guard; the underlying spring step also no-ops then).
    ///
    /// An entity with no matching entry (impossible after `build_world`, but
    /// defensive against any caller passing a stale slice) is left alone.
    /// Entries whose entity is missing from the slice (e.g. a renderer that
    /// hasn't seen the rebuild yet) are also left alone — they'll be picked
    /// up on the next `dress` after the slice catches up.
    pub fn dress(&mut self, dt: f32, entities: &mut [Entity]) {
        let dt = if dt.is_finite() && dt > 0.0 { dt } else { 0.0 };
        // Build a small O(n) `entity.id -> slice index` map so we can locate
        // each entity from an entry without quadratic scans. n is bounded by
        // the live container count (well under 1000), so this is microseconds
        // per frame.
        let mut id_to_idx: HashMap<u32, usize> = HashMap::with_capacity(entities.len());
        for (idx, e) in entities.iter().enumerate() {
            id_to_idx.insert(e.id, idx);
        }

        for (id, (group_id, index_in_group)) in self.id_to_addr.iter() {
            let entity_id = (*group_id as u32) * GROUP_STRIDE + *index_in_group as u32;
            let Some(&ei) = id_to_idx.get(&entity_id) else {
                continue;
            };
            let Some(entry) = self.entries.get_mut(id) else {
                continue;
            };

            let target_half = match (entry.target_load, entry.snap.status) {
                (Some(load), _) => load_to_half_extent(load),
                (None, Status::Running) => load_to_half_extent(0.0),
                (None, _) => BASELINE_HALF_NO_LOAD,
            };

            critically_damped(
                &mut entry.displayed_half,
                &mut entry.vel_half,
                target_half,
                BREATHING_HALF_LIFE,
                dt,
            );
            entities[ei].half_extents = Vec3::splat(entry.displayed_half);
        }
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
            ..ContainerSnapshot::default()
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

    /// Done criterion (04-01 contract): Stat sets a TARGET only — apply()
    /// returns None. The per-frame dress(dt) pass eases the box size in
    /// place. With a big-step dt the spring snaps essentially fully to the
    /// target.
    ///
    /// Renamed from `add_then_stat_sizes_box`: pre-04-01 Stat triggered a
    /// World rebuild and immediately produced MAX_HALF. Post-04-01 the
    /// rebuild is gone (RESEARCH Pitfall A); the size sweep lives in dress.
    #[test]
    fn add_then_dress_sizes_box() {
        let mut lw = LiveWorld::new();
        let mut w = lw
            .apply(DockerMsg::Added(snap("a", "net0", Status::Running)))
            .expect("Added must produce a World");
        assert_eq!(w.entities.len(), 1);
        // Pre-stat: at the floor size (MIN_HALF), not zero, not NaN.
        assert!((w.entities[0].half_extents.x - MIN_HALF).abs() < 1e-6);
        // a took group 0 / slot 0.
        assert_eq!(w.entities[0].id, eid(0, 0));

        // Stat sets a target only — NO World rebuild on Stat (Pitfall A).
        assert!(
            lw.apply(DockerMsg::Stat("a".to_string(), running_load(1.0)))
                .is_none(),
            "Stat must NOT trigger a World rebuild (size easing lives in dress)"
        );

        // Fast-forward the spring with a big dt — the size snaps essentially
        // to the target half. 10s is well past 4·BREATHING_HALF_LIFE, so the
        // exponential decay drives the remainder essentially to zero.
        lw.dress(10.0, &mut w.entities);
        let h = w.entities[0].half_extents.x;
        assert!(
            (h - MAX_HALF).abs() < 1e-3,
            "load 1.0 + fast-forwarded dress should reach MAX_HALF, got {h}"
        );
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
        let w_init = lw
            .apply(DockerMsg::Added(snap("anchor", "net0", Status::Running)))
            .expect("Added must produce a World");
        // 04-01: a Stat is no longer a rebuild source; use the World produced
        // by Added as the reference for the anchor's initial position. The
        // Stat below is still applied so the anchor has a known target_load
        // across the churn (for completeness — only its position is asserted).
        let pos_initial = position_of(&w_init, eid(0, 0));
        assert!(lw
            .apply(DockerMsg::Stat("anchor".to_string(), running_load(0.4)))
            .is_none());

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
    /// the floor size; the box does NOT snap to zero or NaN. Post-04-01 the
    /// box's displayed size is read via dress() rather than apply().
    #[test]
    fn warming_up_does_not_snap() {
        let mut lw = LiveWorld::new();
        let mut w = lw
            .apply(DockerMsg::Added(snap("a", "net0", Status::Running)))
            .expect("Added must produce a World");
        let warming = StatSample {
            cpu_pct: 0.0,
            mem_used: 0,
            mem_limit: 0,
            mem_fraction: 0.0,
            load: 0.0,
            warming_up: true,
        };
        // Warming-up sample is a no-op everywhere.
        assert!(lw.apply(DockerMsg::Stat("a".to_string(), warming)).is_none());

        // A real non-warming Stat with load=0.0 sets target=0.0; dress()
        // eases toward MIN_HALF. Either way (already at MIN_HALF, or eased
        // there) the size stays at the floor — finite, positive, MIN_HALF.
        assert!(lw
            .apply(DockerMsg::Stat("a".to_string(), running_load(0.0)))
            .is_none());
        lw.dress(10.0, &mut w.entities);
        let h = w.entities[0].half_extents.x;
        assert!(h.is_finite() && h > 0.0, "h must be finite/positive, got {h}");
        assert!((h - MIN_HALF).abs() < 1e-3, "load 0.0 -> MIN_HALF, got {h}");
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

    // ---- 04-01 dress() / easing contract pins ----

    /// Pitfall A pin: a Stat with a fresh load returns None from apply (no
    /// World rebuild). Before 04-01 this returned Some(World) — change is
    /// deliberate. The size easing now lives in dress(), called per-frame.
    #[test]
    fn stat_does_not_rebuild() {
        let mut lw = LiveWorld::new();
        let _ = lw
            .apply(DockerMsg::Added(snap("a", "net0", Status::Running)))
            .expect("Added must produce a World");
        // Non-warming Stat -> None.
        assert!(
            lw.apply(DockerMsg::Stat("a".to_string(), running_load(0.5))).is_none(),
            "Stat must NOT trigger a World rebuild (size easing lives in dress)"
        );
        // A second different-load Stat is also None (no rebuild churn).
        assert!(
            lw.apply(DockerMsg::Stat("a".to_string(), running_load(0.8))).is_none(),
            "subsequent Stat must also not rebuild — target is set in place"
        );
    }

    /// dress() eases the displayed size toward the latest target_load. Over
    /// ~2s of 30Hz ticks the spring settles essentially at MAX_HALF, and the
    /// envelope NEVER overshoots (critically-damped pin).
    #[test]
    fn dress_eases_size_toward_target() {
        let mut lw = LiveWorld::new();
        let mut w = lw
            .apply(DockerMsg::Added(snap("a", "net0", Status::Running)))
            .expect("Added must produce a World");
        // Sanity: pre-easing the box sits at MIN_HALF (the seeded floor).
        assert!((w.entities[0].half_extents.x - MIN_HALF).abs() < 1e-6);

        let _ = lw.apply(DockerMsg::Stat("a".to_string(), running_load(1.0)));
        // ~2s at 30fps — 60 steps. Track the peak to assert no overshoot.
        let mut peak = w.entities[0].half_extents.x;
        for _ in 0..60 {
            lw.dress(0.033, &mut w.entities);
            let h = w.entities[0].half_extents.x;
            if h > peak {
                peak = h;
            }
        }
        let h = w.entities[0].half_extents.x;
        assert!(
            (h - MAX_HALF).abs() < 0.01,
            "dress should settle near MAX_HALF after 2s, got {h}"
        );
        assert!(
            peak <= MAX_HALF + 1e-3,
            "spring overshot MAX_HALF: peak={peak}"
        );
    }

    /// dt = 0.0 must be a no-op for dress() — entity sizes unchanged from
    /// their pre-dress values. The underlying spring step has the same guard.
    #[test]
    fn dress_with_zero_dt_does_not_move() {
        let mut lw = LiveWorld::new();
        let mut w = lw
            .apply(DockerMsg::Added(snap("a", "net0", Status::Running)))
            .expect("Added must produce a World");
        let _ = lw.apply(DockerMsg::Stat("a".to_string(), running_load(0.8)));
        let before = w.entities[0].half_extents;
        lw.dress(0.0, &mut w.entities);
        let after = w.entities[0].half_extents;
        assert_eq!(before, after, "zero-dt dress must not change sizes");
    }

    /// dress() on an empty entity slice is a no-op (no panic). Useful both
    /// for the empty-state banner path AND for defensive callers.
    #[test]
    fn dress_on_empty_entities_is_noop() {
        let mut lw = LiveWorld::new();
        // No entries; no entities. Just don't panic.
        let mut entities: Vec<Entity> = Vec::new();
        lw.dress(0.033, &mut entities);
        assert!(entities.is_empty());

        // And on an entity-less but populated entries: also fine.
        let mut lw = LiveWorld::new();
        let _ = lw.apply(DockerMsg::Added(snap("a", "net0", Status::Running)));
        // Caller passed in a stale empty slice — dress just skips, no panic.
        let mut entities: Vec<Entity> = Vec::new();
        lw.dress(0.033, &mut entities);
    }

    /// After a Removed, the surviving entity continues to ease — the dropped
    /// entry is gone from `entries`/`id_to_addr` so dress() naturally skips
    /// it; the surviving entry's spring still steps.
    #[test]
    fn removed_entry_no_longer_dressed() {
        let mut lw = LiveWorld::new();
        lw.apply(DockerMsg::Added(snap("a", "net0", Status::Running)));
        // Discard the 2-entity world; we only care about the post-remove one.
        let _ = lw
            .apply(DockerMsg::Added(snap("b", "net0", Status::Running)))
            .expect("Added must produce a World");
        // Both targets set high; nothing eased yet.
        let _ = lw.apply(DockerMsg::Stat("a".to_string(), running_load(1.0)));
        let _ = lw.apply(DockerMsg::Stat("b".to_string(), running_load(1.0)));

        // Remove a. World rebuilds without a's entity; b's entity remains.
        let mut w = lw
            .apply(DockerMsg::Removed("a".to_string()))
            .expect("Removed must produce a World");
        assert_eq!(w.entities.len(), 1, "only b should remain");
        assert_eq!(w.entities[0].id, eid(0, 1), "b kept its slot");

        // dress fast-forward — b's surviving entry must ease to MAX_HALF.
        lw.dress(10.0, &mut w.entities);
        let h = w.entities[0].half_extents.x;
        assert!(
            (h - MAX_HALF).abs() < 1e-3,
            "surviving entity b must still ease, got h={h}"
        );
    }

    /// A non-Running entity with no Stat ever arriving holds at the baseline
    /// half — dress repeatedly is idempotent at the target. (Status::Stopped
    /// never gets a stats stream from Docker, so target_load stays None.)
    #[test]
    fn dress_with_no_target_holds_baseline_for_non_running() {
        let mut lw = LiveWorld::new();
        let mut w = lw
            .apply(DockerMsg::Added(snap("a", "net0", Status::Stopped)))
            .expect("Added must produce a World");
        // Pre-dress: seeded at BASELINE_HALF_NO_LOAD.
        assert!((w.entities[0].half_extents.x - BASELINE_HALF_NO_LOAD).abs() < 1e-6);

        // Many ticks — the target is BASELINE_HALF_NO_LOAD, the displayed
        // value is already there, the spring should not drift.
        for _ in 0..100 {
            lw.dress(0.033, &mut w.entities);
        }
        let h = w.entities[0].half_extents.x;
        assert!(
            (h - BASELINE_HALF_NO_LOAD).abs() < 1e-3,
            "non-Running with no stat must hold the baseline, got {h}"
        );
    }

    // --- 04-02: Enriched / slot migration / Phase 3 carryovers ----------------

    use crate::docker::{EnrichedSnapshot, PortProto, PortSummary as DomainPortSummary};

    fn enriched(
        id: &str,
        group_key: &str,
        ports: Vec<DomainPortSummary>,
        mount_count: usize,
        status_override: Option<Status>,
    ) -> EnrichedSnapshot {
        EnrichedSnapshot {
            id: id.to_string(),
            group_key: group_key.to_string(),
            ports,
            mount_count,
            status_override,
        }
    }

    /// Done criterion: Enriched for a never-Added id is a silent no-op
    /// (race-window where the container was removed before the off-thread
    /// inspect returned).
    #[test]
    fn enriched_unknown_id_is_noop() {
        let mut lw = LiveWorld::new();
        assert!(lw
            .apply(DockerMsg::Enriched(enriched("ghost", "net0", vec![], 0, None)))
            .is_none());
        assert_eq!(lw.len(), 0);
    }

    /// Done criterion: Enriched with the SAME group_key updates ports +
    /// mount_count in place and rebuilds the World. Slot/position unchanged.
    #[test]
    fn enriched_same_group_updates_in_place() {
        let mut lw = LiveWorld::new();
        let w0 = lw
            .apply(DockerMsg::Added(snap("a", "net0", Status::Running)))
            .expect("Added must produce a World");
        let pos_before = position_of(&w0, eid(0, 0));

        let ports = vec![
            DomainPortSummary {
                private: 80,
                public: Some(8080),
                proto: PortProto::Tcp,
            },
            DomainPortSummary {
                private: 443,
                public: None,
                proto: PortProto::Tcp,
            },
        ];
        let w1 = lw
            .apply(DockerMsg::Enriched(enriched(
                "a",
                "net0",
                ports.clone(),
                3,
                None,
            )))
            .expect("Enriched on same-group still triggers rebuild");

        // Position unchanged.
        assert_eq!(
            position_of(&w1, eid(0, 0)),
            pos_before,
            "container moved despite same group_key"
        );
        // snapshot() surfaces ports + mount_count for ENT-02 / ENT-03.
        let snap_now = lw.snapshot("a").expect("a is alive");
        assert_eq!(snap_now.ports, ports);
        assert_eq!(snap_now.mount_count, 3);
    }

    /// Done criterion: Enriched with a NEW group_key migrates the slot —
    /// the migrating container moves to the new group's Z-band, OTHER
    /// containers in the OLD group keep their positions (anti-teleport for
    /// non-migrating neighbors, Phase 3 carryover (3)).
    #[test]
    fn enriched_new_group_migrates_slot() {
        let mut lw = LiveWorld::new();
        // Seed: a and b both in group "old". a took slot 0, b took slot 1.
        lw.apply(DockerMsg::Added(snap("a", "old", Status::Running)));
        let w0 = lw
            .apply(DockerMsg::Added(snap("b", "old", Status::Running)))
            .expect("Added must produce a World");
        let pos_a_before = position_of(&w0, eid(0, 0));
        let pos_b_before = position_of(&w0, eid(0, 1));

        // Migrate "a" to group "new". b stays in "old".
        let w1 = lw
            .apply(DockerMsg::Enriched(enriched("a", "new", vec![], 0, None)))
            .expect("group_key change must rebuild");

        // a is now at (group=1, slot=0) — first slot in the new group.
        let pos_a_after = w1
            .entities
            .iter()
            .find(|e| e.id == eid(1, 0))
            .map(|e| e.position)
            .expect("a should now occupy group 1 / slot 0");
        assert_ne!(
            pos_a_after, pos_a_before,
            "a's position didn't change despite group migration"
        );
        // b stayed at group-0/slot-1 (anti-teleport for neighbors).
        assert_eq!(
            position_of(&w1, eid(0, 1)),
            pos_b_before,
            "b teleported on a's migration"
        );
        // a's old slot (group-0/slot-0) is no longer in the world.
        assert!(
            w1.entities.iter().all(|e| e.id != eid(0, 0)),
            "a's old slot (group-0/slot-0) leaked into world"
        );
        // The migrated container has the new group_key.
        assert_eq!(lw.snapshot("a").unwrap().group_key, "new");
    }

    /// Done criterion: Enriched with `status_override: Some(Crashed)`
    /// upgrades a Stopped entry to Crashed (Phase 3 carryover (1) — exited-
    /// with-OOM containers that seeded as Stopped via list_containers).
    #[test]
    fn enriched_status_override_upgrades_stopped_to_crashed() {
        let mut lw = LiveWorld::new();
        lw.apply(DockerMsg::Added(snap("a", "net0", Status::Stopped)));
        assert_eq!(lw.snapshot("a").unwrap().status, Status::Stopped);

        let w = lw
            .apply(DockerMsg::Enriched(enriched(
                "a",
                "net0",
                vec![],
                0,
                Some(Status::Crashed),
            )))
            .expect("status_override must trigger rebuild");
        let entity = w
            .entities
            .iter()
            .find(|e| e.id == eid(0, 0))
            .expect("a is in world");
        assert_eq!(entity.status, Status::Crashed);
        assert_eq!(lw.snapshot("a").unwrap().status, Status::Crashed);
    }

    /// Done criterion: Enriched with `status_override: None` does NOT change
    /// the entry's status — the override is opt-in. ports/mounts still update.
    #[test]
    fn enriched_status_override_none_does_not_change_status() {
        let mut lw = LiveWorld::new();
        lw.apply(DockerMsg::Added(snap("a", "net0", Status::Running)));
        let w = lw
            .apply(DockerMsg::Enriched(enriched(
                "a", "net0", vec![], 2, None,
            )))
            .expect("Enriched must rebuild");
        let entity = w
            .entities
            .iter()
            .find(|e| e.id == eid(0, 0))
            .expect("a is in world");
        assert_eq!(
            entity.status,
            Status::Running,
            "status_override=None must NOT flip Running"
        );
        assert_eq!(lw.snapshot("a").unwrap().mount_count, 2);
    }

    /// Sanity: snapshot() surfaces the live ContainerSnapshot for reading;
    /// unknown ids return None (used by ENT-02/ENT-03 to read ports /
    /// mount_count without re-fetching).
    #[test]
    fn snapshot_returns_live_snapshot_or_none() {
        let mut lw = LiveWorld::new();
        assert!(lw.snapshot("nope").is_none());
        lw.apply(DockerMsg::Added(snap("a", "net0", Status::Running)));
        let s = lw.snapshot("a").expect("a is alive");
        assert_eq!(s.id, "a");
        assert_eq!(s.group_key, "net0");
        assert_eq!(s.status, Status::Running);
    }
}
