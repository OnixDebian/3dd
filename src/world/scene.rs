//! Synthetic scene generator + scene bounds.
//!
//! [`synthetic_scene`] builds a [`World`] of dozens of entities across a handful
//! of groups, each placed in its stable [`crate::world::layout`] slot, sized by
//! the clamped/compressive [`crate::world::load_to_half_extent`], and tagged
//! with a varied [`crate::theme::Status`]. Load and status are derived
//! deterministically from the entity id (a tiny hash — no `rand`, reproducible),
//! so the whole scene is stable across rebuilds (the anti-teleport pin).
//!
//! [`SceneBounds`] is the axis-aligned extent + center + bounding-sphere radius
//! of every entity AABB; [`crate::camera::Camera::frame_scene`] uses it to frame
//! the whole rack instead of a unit cube.

#![allow(dead_code)]

use glam::Vec3;

use crate::theme::Status;
use crate::world::entity::{load_to_half_extent, Entity};
use crate::world::layout::layout;
use crate::world::World;

/// Number of synthetic groups (future network floor-planes). Reduced from 5 to 3
/// during the human verify-tuning pass: 5 groups stacked along Z made the scene
/// far deeper than wide, so the bounding sphere the camera frames was dominated by
/// depth and the rack rendered as a tiny diagonal ribbon (~10% of the frame). A
/// shallower scene fills the frame far better while still showing distinct groups.
/// Still yields 30 boxes (`scene_has_dozens_of_entities` needs >= 30).
const GROUP_COUNT: u16 = 3;
/// Entities per group; `GROUP_COUNT * PER_GROUP` is the total box count.
const PER_GROUP: u32 = 10;

/// Axis-aligned bounds of the whole scene plus a bounding sphere.
///
/// Computed from every entity AABB (`position ± half_extents`). `center` is the
/// AABB midpoint; `radius` is the distance from `center` to the farthest entity
/// corner — the camera frames the scene by fitting this sphere in the frustum.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneBounds {
    /// Minimum corner of the enclosing AABB.
    pub min: Vec3,
    /// Maximum corner of the enclosing AABB.
    pub max: Vec3,
    /// Midpoint of the AABB — the camera's orbit target.
    pub center: Vec3,
    /// Bounding-sphere radius about `center` (to the farthest corner).
    pub radius: f32,
}

impl SceneBounds {
    /// Fold the entity AABBs into an enclosing AABB, then derive center +
    /// bounding-sphere radius. Empty input yields a degenerate zero box.
    pub fn from_entities(entities: &[Entity]) -> Self {
        if entities.is_empty() {
            return Self {
                min: Vec3::ZERO,
                max: Vec3::ZERO,
                center: Vec3::ZERO,
                radius: 0.0,
            };
        }

        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for e in entities {
            min = min.min(e.position - e.half_extents);
            max = max.max(e.position + e.half_extents);
        }

        let center = (min + max) * 0.5;
        // Farthest entity corner from center => bounding sphere radius.
        let mut radius: f32 = 0.0;
        for e in entities {
            for corner in aabb_corners(e.position, e.half_extents) {
                radius = radius.max((corner - center).length());
            }
        }

        Self { min, max, center, radius }
    }
}

/// Build the synthetic scene: `GROUP_COUNT * PER_GROUP` entities in stable
/// slots, with deterministic per-id load and status variety.
pub fn synthetic_scene() -> World {
    let mut entities = Vec::with_capacity((GROUP_COUNT as u32 * PER_GROUP) as usize);

    let mut id: u32 = 0;
    for group in 0..GROUP_COUNT {
        for index_in_group in 0..PER_GROUP {
            let position = layout(group, index_in_group);
            let load = synthetic_load(id);
            let half = load_to_half_extent(load);
            entities.push(Entity {
                id,
                position,
                half_extents: Vec3::splat(half),
                status: synthetic_status(id),
                group,
            });
            id += 1;
        }
    }

    let bounds = SceneBounds::from_entities(&entities);
    World { entities, bounds }
}

/// Deterministic pseudo-random load in `[0, 1]` from an entity id. A tiny LCG
/// hash — reproducible, no `rand` dependency.
fn synthetic_load(id: u32) -> f32 {
    // Knuth multiplicative hash, take the top bits as a fraction.
    let h = id.wrapping_mul(2_654_435_761);
    (h >> 8) as f32 / (1u32 << 24) as f32
}

/// Deterministic status from an entity id: mostly Running, a sprinkle of the
/// other states so the palette's color variety is visible.
fn synthetic_status(id: u32) -> Status {
    match id % 10 {
        0 => Status::Paused,
        3 => Status::Restarting,
        6 => Status::Stopped,
        9 => Status::Crashed,
        _ => Status::Running,
    }
}

/// The 8 corners of an AABB given its center and half-extents.
fn aabb_corners(center: Vec3, half: Vec3) -> [Vec3; 8] {
    let mut out = [Vec3::ZERO; 8];
    let mut i = 0;
    for &sx in &[-1.0f32, 1.0] {
        for &sy in &[-1.0f32, 1.0] {
            for &sz in &[-1.0f32, 1.0] {
                out[i] = center + Vec3::new(sx * half.x, sy * half.y, sz * half.z);
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::entity::MAX_HALF;

    #[test]
    fn scene_has_dozens_of_entities() {
        let world = synthetic_scene();
        assert!(
            world.entities.len() >= 30,
            "expected dozens, got {}",
            world.entities.len()
        );
    }

    #[test]
    fn no_two_aabbs_overlap() {
        // Min pairwise center distance must clear 2*MAX_HALF, so no two entity
        // AABBs intersect regardless of their individual sizes.
        let world = synthetic_scene();
        let es = &world.entities;
        let mut min_d = f32::INFINITY;
        for i in 0..es.len() {
            for j in (i + 1)..es.len() {
                min_d = min_d.min((es[i].position - es[j].position).length());
            }
        }
        assert!(
            min_d >= 2.0 * MAX_HALF,
            "two AABBs can overlap: min center distance {min_d} < {}",
            2.0 * MAX_HALF
        );
    }

    #[test]
    fn groups_form_distinct_clusters() {
        // Max intra-group center spread < min inter-group region gap: proves
        // groups are contiguous regions, not interleaved.
        let world = synthetic_scene();
        let es = &world.entities;

        let mut max_intra: f32 = 0.0;
        for i in 0..es.len() {
            for j in (i + 1)..es.len() {
                if es[i].group == es[j].group {
                    max_intra = max_intra.max((es[i].position - es[j].position).length());
                }
            }
        }

        let mut min_inter = f32::INFINITY;
        for i in 0..es.len() {
            for j in (i + 1)..es.len() {
                if es[i].group != es[j].group {
                    min_inter = min_inter.min((es[i].position - es[j].position).length());
                }
            }
        }

        assert!(
            max_intra < min_inter,
            "groups interleave: max intra-group spread {max_intra} >= min inter-group gap {min_inter}"
        );
    }

    #[test]
    fn bounds_enclose_every_corner() {
        let world = synthetic_scene();
        let b = world.bounds;
        for e in &world.entities {
            for c in aabb_corners(e.position, e.half_extents) {
                assert!(
                    c.x >= b.min.x - 1e-4 && c.x <= b.max.x + 1e-4
                        && c.y >= b.min.y - 1e-4 && c.y <= b.max.y + 1e-4
                        && c.z >= b.min.z - 1e-4 && c.z <= b.max.z + 1e-4,
                    "corner {c:?} outside bounds {:?}..{:?}",
                    b.min,
                    b.max
                );
                // radius must reach the farthest corner.
                assert!(
                    (c - b.center).length() <= b.radius + 1e-4,
                    "corner {c:?} outside bounding sphere radius {}",
                    b.radius
                );
            }
        }
    }

    #[test]
    fn center_is_aabb_midpoint() {
        let b = synthetic_scene().bounds;
        let mid = (b.min + b.max) * 0.5;
        assert!((b.center - mid).length() < 1e-4, "center {:?} != midpoint {mid:?}", b.center);
    }

    #[test]
    fn scene_is_stable_across_rebuilds() {
        // The anti-teleport pin: same id -> same slot on every rebuild.
        let a = synthetic_scene();
        let c = synthetic_scene();
        assert_eq!(a.entities.len(), c.entities.len());
        for (ea, ec) in a.entities.iter().zip(c.entities.iter()) {
            assert_eq!(ea.id, ec.id);
            assert_eq!(ea.position, ec.position);
            assert_eq!(ea.half_extents, ec.half_extents);
            assert_eq!(ea.status, ec.status);
        }
    }
}
