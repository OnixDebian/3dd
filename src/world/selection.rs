//! Selection state — Tab cycle + brightness pulse + detail-open flag (CAM-04 / 04-03).
//!
//! Cycle order = `world.entities[].id` ascending. Deterministic because the
//! `LiveWorld::build_world` rebuild walks groups in registration order and
//! slots in index order (see `world::live::build_world`), and the id encoding
//! is `group << 16 | index_in_group` (lower bits = within-group slot).
//!
//! ## Brightness pulse
//!
//! `pulse_phase` is a normalized `[0, 1)` cycle advanced by [`Selection::tick`]
//! every render frame. The selected entity's RGB is multiplied by
//! `1.0 + AMPLITUDE * sin(pulse_phase * TAU)` so a Tab-cycled box throbs
//! slightly brighter than its neighbours without flickering — RESEARCH
//! "Selection (CAM-04)" recommended this brightness pulse over a full outline
//! (cheaper, and the brightness boost reads at any zoom).
//!
//! ## Reconcile-on-Removed
//!
//! When the selected container disappears (Removed message, count change),
//! [`Selection::reconcile`] moves the selection to the next live entity
//! (or `None` if the world is empty). Prevents Tab+Inspect on a now-dead
//! container from racing.

#![allow(dead_code)]

use crate::world::entity::Entity;
use crate::world::World;

/// Full pulse cycle in seconds. 1.2s is slow enough to read as a calm throb
/// rather than a strobe, and well outside the easing half-life (0.15s) so
/// the box's breathing isn't fighting the selection animation visually.
const PULSE_PERIOD: f32 = 1.2;

/// Brightness multiplier amplitude. 1.0 ± 0.18 = the selected box's RGB
/// swings between ~82% and ~118% of its unselected brightness. Visible at
/// any zoom; never close enough to white that the per-status color is lost.
const AMPLITUDE: f32 = 0.18;

/// Per-app selection state — owned by `App` (braille) and the kitty backend.
///
/// `selected_id` is the entity id (the `group << 16 | index` flat encoding
/// from `LiveWorld::build_world`) of the Tab-cycled container, or `None`
/// when nothing is selected. `last_label_cell` is hysteresis state for the
/// 04-04 label render (this plan ships the slot; the label lands next).
/// `pulse_phase` is the `[0, 1)` brightness-pulse phase; `detail_open` is
/// the CAM-05 popup toggle.
#[derive(Debug, Default, Clone)]
pub struct Selection {
    /// Selected entity id (`group << 16 | index_in_group`), or None.
    pub selected_id: Option<u32>,
    /// Last drawn label anchor in BRAILLE CELL coordinates (04-04 hysteresis).
    /// Braille uses `(dot_x / 2, dot_y / 4)` quantization.
    pub last_label_cell: Option<(i16, i16)>,
    /// Last drawn label anchor in KITTY CELL coordinates (04-04 hysteresis).
    /// Kitty cells are in PIXEL units (cell_w × cell_h from terminal metrics);
    /// the two cell systems disagree (RESEARCH Open Question #4: per-backend
    /// state in v1), so each backend keeps its own hysteresis cache.
    pub last_label_cell_kitty: Option<(i16, i16)>,
    /// Brightness-pulse phase in `[0, 1)`, advanced per-frame by [`Selection::tick`].
    pub pulse_phase: f32,
    /// Detail panel open flag (CAM-05). Toggled by Enter (open) / Esc (close).
    pub detail_open: bool,
}

impl Selection {
    /// Fresh empty selection.
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up the currently-selected entity in `world`. Returns `None` when
    /// nothing is selected, or when the selected id is no longer present
    /// (between an out-of-date selection and the next [`Selection::reconcile`]
    /// call).
    pub fn selected_entity<'a>(&self, world: &'a World) -> Option<&'a Entity> {
        let id = self.selected_id?;
        world.entities.iter().find(|e| e.id == id)
    }

    /// Cycle the selection forward through ids-ascending. From None lands on
    /// the lowest id; from the highest wraps to the lowest. Resets the label
    /// hysteresis so the new label snaps to its anchor on the next frame.
    pub fn next(&mut self, world: &World) {
        if world.entities.is_empty() {
            self.selected_id = None;
            self.detail_open = false;
            self.last_label_cell = None;
            self.last_label_cell_kitty = None;
            return;
        }
        let mut ids: Vec<u32> = world.entities.iter().map(|e| e.id).collect();
        ids.sort_unstable();
        self.selected_id = Some(match self.selected_id {
            None => ids[0],
            Some(cur) => match ids.iter().position(|&id| id > cur) {
                Some(p) => ids[p],
                None => ids[0], // wrap past the top
            },
        });
        self.last_label_cell = None;
        self.last_label_cell_kitty = None;
    }

    /// Cycle the selection backward through ids-ascending. From None lands on
    /// the highest id; from the lowest wraps to the highest.
    pub fn prev(&mut self, world: &World) {
        if world.entities.is_empty() {
            self.selected_id = None;
            self.detail_open = false;
            self.last_label_cell = None;
            self.last_label_cell_kitty = None;
            return;
        }
        let mut ids: Vec<u32> = world.entities.iter().map(|e| e.id).collect();
        ids.sort_unstable();
        self.selected_id = Some(match self.selected_id {
            None => *ids.last().expect("non-empty checked above"),
            Some(cur) => match ids.iter().rposition(|&id| id < cur) {
                Some(p) => ids[p],
                None => *ids.last().expect("non-empty"), // wrap past the bottom
            },
        });
        self.last_label_cell = None;
        self.last_label_cell_kitty = None;
    }

    /// Drop a stale selection if the world no longer contains it.
    ///
    /// Called after `drain_docker` returns count-changed. When the selected
    /// container disappeared, the selection moves to the first remaining
    /// entity (or `None` when the world is empty), and the detail panel
    /// closes — preventing a stale popup on a now-dead container.
    pub fn reconcile(&mut self, world: &World) {
        let Some(id) = self.selected_id else { return };
        if !world.entities.iter().any(|e| e.id == id) {
            self.selected_id = world.entities.first().map(|e| e.id);
            self.detail_open = false;
            self.last_label_cell = None;
            self.last_label_cell_kitty = None;
        }
    }

    /// Advance the pulse phase by real elapsed time `dt`. Wraps to `[0, 1)`.
    /// Non-finite or non-positive `dt` is treated as 0 (matches the existing
    /// `on_tick` / `Camera::step` dt guards).
    pub fn tick(&mut self, dt: f32) {
        let dt = if dt.is_finite() && dt > 0.0 { dt } else { 0.0 };
        self.pulse_phase = (self.pulse_phase + dt / PULSE_PERIOD).rem_euclid(1.0);
    }

    /// Brightness multiplier for an entity at the current pulse phase. Returns
    /// `1.0` for unselected entities (no boost) and `1.0 + AMPLITUDE * sin(phase * TAU)`
    /// for the selected one. The renderers multiply this into each face's
    /// RGB and clamp to `[0, 255]`.
    pub fn pulse_multiplier(&self, entity_id: u32) -> f32 {
        if Some(entity_id) != self.selected_id {
            return 1.0;
        }
        1.0 + AMPLITUDE * (self.pulse_phase * std::f32::consts::TAU).sin()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::entity::Entity;
    use crate::world::scene::SceneBounds;
    use glam::Vec3;

    /// Build a tiny World containing one Running entity per id. Ids may be
    /// in any order; the selection cycle is by ASCENDING id regardless.
    fn world_with_ids(ids: &[u32]) -> World {
        let entities: Vec<Entity> = ids
            .iter()
            .map(|&id| Entity {
                id,
                position: Vec3::ZERO,
                half_extents: Vec3::splat(0.5),
                status: crate::theme::Status::Running,
                group: 0,
            })
            .collect();
        let bounds = SceneBounds::from_entities(&entities);
        World { entities, bounds }
    }

    /// Next from None must pick the lowest id (deterministic Tab entry).
    #[test]
    fn next_from_none_picks_lowest_id() {
        let world = world_with_ids(&[3, 1, 2]);
        let mut sel = Selection::new();
        sel.next(&world);
        assert_eq!(sel.selected_id, Some(1));
    }

    /// Next from the highest id wraps to the lowest.
    #[test]
    fn next_wraps_at_highest() {
        let world = world_with_ids(&[1, 2, 3]);
        let mut sel = Selection::new();
        sel.selected_id = Some(3);
        sel.next(&world);
        assert_eq!(sel.selected_id, Some(1));
    }

    /// Prev from the lowest id wraps to the highest.
    #[test]
    fn prev_wraps_at_lowest() {
        let world = world_with_ids(&[1, 2, 3]);
        let mut sel = Selection::new();
        sel.selected_id = Some(1);
        sel.prev(&world);
        assert_eq!(sel.selected_id, Some(3));
    }

    /// Prev from None picks the highest id.
    #[test]
    fn prev_from_none_picks_highest_id() {
        let world = world_with_ids(&[1, 2, 3]);
        let mut sel = Selection::new();
        sel.prev(&world);
        assert_eq!(sel.selected_id, Some(3));
    }

    /// Reconcile drops a stale id and falls back to the first surviving
    /// entity. Also clears detail_open + label hysteresis.
    #[test]
    fn reconcile_drops_disappeared_selection() {
        let world = world_with_ids(&[5, 7, 9]);
        let mut sel = Selection::new();
        sel.selected_id = Some(99); // not in world
        sel.detail_open = true;
        sel.last_label_cell = Some((10, 20));
        sel.reconcile(&world);
        // Falls back to the first present id (5, the lowest by id order — the
        // `entities.first()` happens to also be the lowest because world_with_ids
        // builds them in input order; the contract is "first present", which is
        // deterministic).
        assert_eq!(
            sel.selected_id,
            Some(world.entities.first().unwrap().id)
        );
        assert!(!sel.detail_open, "detail_open must close on stale selection");
        assert!(sel.last_label_cell.is_none(), "label hysteresis must reset");
    }

    /// Reconcile on an empty world drops the selection entirely.
    #[test]
    fn reconcile_on_empty_world_drops_to_none() {
        let world = world_with_ids(&[]);
        let mut sel = Selection::new();
        sel.selected_id = Some(42);
        sel.detail_open = true;
        sel.reconcile(&world);
        assert_eq!(sel.selected_id, None);
        assert!(!sel.detail_open);
    }

    /// Reconcile on a still-present selection is a no-op.
    #[test]
    fn reconcile_keeps_present_selection() {
        let world = world_with_ids(&[1, 2, 3]);
        let mut sel = Selection::new();
        sel.selected_id = Some(2);
        sel.detail_open = true;
        sel.reconcile(&world);
        assert_eq!(sel.selected_id, Some(2));
        assert!(sel.detail_open, "detail_open must stay if selection is alive");
    }

    /// tick advances the pulse_phase and wraps in `[0, 1)`.
    #[test]
    fn tick_wraps_to_zero_after_period() {
        let mut sel = Selection::new();
        sel.tick(PULSE_PERIOD);
        assert!(
            sel.pulse_phase.abs() < 1e-5,
            "phase should wrap to ~0 after one full period, got {}",
            sel.pulse_phase
        );
        // Subsequent half-period leaves phase ~0.5.
        sel.tick(PULSE_PERIOD * 0.5);
        assert!(
            (sel.pulse_phase - 0.5).abs() < 1e-5,
            "phase should be ~0.5 after half period, got {}",
            sel.pulse_phase
        );
    }

    /// Non-finite / non-positive dt is a no-op.
    #[test]
    fn tick_with_zero_or_negative_dt_is_noop() {
        let mut sel = Selection::new();
        sel.pulse_phase = 0.25;
        sel.tick(0.0);
        sel.tick(-1.0);
        sel.tick(f32::NAN);
        sel.tick(f32::INFINITY);
        assert!((sel.pulse_phase - 0.25).abs() < 1e-6);
    }

    /// pulse_multiplier only boosts the selected entity.
    #[test]
    fn pulse_multiplier_only_boosts_selected() {
        let mut sel = Selection::new();
        sel.selected_id = Some(1);
        sel.pulse_phase = 0.25; // sin(0.25 * TAU) == 1.0
        let m_selected = sel.pulse_multiplier(1);
        let m_other = sel.pulse_multiplier(2);
        assert!(
            (m_selected - (1.0 + AMPLITUDE)).abs() < 1e-5,
            "selected mult should be 1+AMPLITUDE at phase 0.25, got {m_selected}"
        );
        assert!(
            (m_other - 1.0).abs() < 1e-6,
            "unselected mult must be 1.0, got {m_other}"
        );
    }

    /// pulse_multiplier returns 1.0 when nothing is selected.
    #[test]
    fn pulse_multiplier_returns_one_when_no_selection() {
        let sel = Selection::new();
        assert_eq!(sel.pulse_multiplier(42), 1.0);
    }

    /// Next on an empty world clears the selection cleanly.
    #[test]
    fn next_on_empty_world_clears_selection() {
        let world = world_with_ids(&[]);
        let mut sel = Selection::new();
        sel.selected_id = Some(7);
        sel.detail_open = true;
        sel.next(&world);
        assert_eq!(sel.selected_id, None);
        assert!(!sel.detail_open);
    }
}
