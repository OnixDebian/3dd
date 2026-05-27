//! Deterministic, network-aware layout (CONT-05 — the resolved research
//! decision: **network-grouped rack grid**).
//!
//! [`layout`] maps a stable `(group, index_in_group)` pair to a fixed world
//! slot. It is PURE: no clock, no `rand`, no global mutable state — so the same
//! entity id always lands in the same place (the anti-jitter / anti-teleport
//! guarantee). Groups are the first-class placement axis: each group occupies a
//! contiguous Z-band (a future network floor-plane, Phase 4 ENT-01); within a
//! group, boxes fill a 2D rack grid (columns along X, shelves stacked along Y).
//!
//! Slot spacing comfortably exceeds `2 * MAX_HALF`, so even max-size boxes never
//! overlap their grid neighbours.

#![allow(dead_code)]

use glam::Vec3;

use crate::world::entity::MAX_HALF;

/// Center-to-center spacing between adjacent slots within a group's rack grid.
/// Strictly greater than `2 * MAX_HALF` (= 2.4) so even two maxed-out boxes in
/// neighbouring slots keep a clear gap (the no-overlap guarantee).
pub const SLOT_SPACING: f32 = 3.0;

/// Z distance between consecutive group bands. Large enough that the gap between
/// two groups' regions exceeds the spread within any single group's grid, so
/// groups read as distinct clusters rather than interleaving.
pub const GROUP_DEPTH: f32 = 18.0;

/// Number of columns (along X) in each group's rack grid. Rows stack along Y;
/// `index_in_group` fills column-major within a group.
pub const GRID_COLS: u32 = 4;

/// World slot center for the entity at `index_in_group` within `group`.
///
/// Deterministic and pure. Columns run along +X, rows (shelves) stack along +Y,
/// and each group sits on its own Z-band. The whole rack is centered near the
/// origin so the existing frustum-safe camera math stays sane: X is centered on
/// the grid columns, Z is centered on the group bands. Y starts at the floor
/// (row 0 at y=0) and stacks upward.
pub fn layout(_group: u16, _index_in_group: u32) -> Vec3 {
    // RED stub — every entity collapses to the origin (slot collision).
    Vec3::ZERO
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_deterministic() {
        // Same (group, index) always yields the same slot.
        assert_eq!(layout(2, 7), layout(2, 7));
        assert_eq!(layout(0, 0), layout(0, 0));
    }

    #[test]
    fn distinct_slots_dont_collide() {
        // Different (group, index) pairs map to different positions.
        let a = layout(0, 0);
        let b = layout(0, 1);
        let c = layout(1, 0);
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(b, c);
    }

    #[test]
    fn neighbours_clear_max_boxes() {
        // Adjacent slots are at least 2*MAX_HALF apart (no AABB overlap even at
        // max size).
        let a = layout(0, 0);
        let b = layout(0, 1);
        assert!(
            (a - b).length() >= 2.0 * MAX_HALF,
            "adjacent slots too close: {}",
            (a - b).length()
        );
    }
}
