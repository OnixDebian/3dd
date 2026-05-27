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

/// Center-to-center spacing between adjacent slots within a group's rack grid.
/// Strictly greater than `2 * MAX_HALF` (= 2.4) so even two maxed-out boxes in
/// neighbouring slots keep a clear gap (the no-overlap guarantee). Tightened from
/// 3.0 to 2.6 during the human verify-tuning pass: a denser rack has a smaller
/// bounding sphere, so the framing camera fills the frame far better (still
/// clears 2.4, so no AABB overlap — `neighbours_clear_max_boxes` pins it).
pub const SLOT_SPACING: f32 = 2.6;

/// Z distance between consecutive group bands. Large enough that the gap between
/// two groups' regions exceeds the spread within any single group's grid, so
/// groups read as distinct clusters rather than interleaving.
///
/// Reduced from 18.0 to 10.0 during the human verify-tuning pass: the deep scene
/// gave a huge bounding-sphere radius, forcing the camera far back so the rack
/// filled only ~10% of the frame. A shallower scene keeps the rack compact so the
/// tightened framing fills most of the frame. It MUST still exceed the max
/// intra-group corner-to-corner spread (~9.37 for the 4-col × 3-row grid at the
/// tightened SLOT_SPACING) so two boxes in the same group are never farther apart
/// than two in adjacent groups — i.e. groups stay distinct clusters
/// (`groups_form_distinct_clusters` pins it).
pub const GROUP_DEPTH: f32 = 10.0;

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
pub fn layout(group: u16, index_in_group: u32) -> Vec3 {
    let col = index_in_group % GRID_COLS;
    let row = index_in_group / GRID_COLS;

    // Center the columns about X=0 so the rack straddles the origin.
    let x_center = (GRID_COLS as f32 - 1.0) * SLOT_SPACING * 0.5;
    let x = col as f32 * SLOT_SPACING - x_center;

    // Shelves stack upward from the floor.
    let y = row as f32 * SLOT_SPACING;

    // Each group is a Z-band; bands are not centered here (the scene generator
    // knows the group count and centers the whole World via SceneBounds.center,
    // which the camera targets). Using group directly keeps layout a pure
    // function of its two args alone.
    let z = group as f32 * GROUP_DEPTH;

    Vec3::new(x, y, z)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::entity::MAX_HALF;

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
