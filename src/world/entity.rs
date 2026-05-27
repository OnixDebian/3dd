//! Domain entity + the clamped, compressive load->size mapping.
//!
//! An [`Entity`] is one box in the synthetic scene: a stable identity, a
//! world-space slot (assigned by [`crate::world::layout`]), per-axis
//! half-extents (which scale the unit cube the rasterizers draw), a reused
//! [`crate::theme::Status`] (CONT-01 — color is resolved later by the palette,
//! never here), and a synthetic group id (the network/grouping key, CONT-05).
//!
//! [`load_to_half_extent`] is the CONT-02 size proxy: a normalized usage value
//! in `[0, 1]` maps onto `[MIN_HALF, MAX_HALF]` through a compressive `sqrt`
//! curve so idle is never sub-pixel and a hog never fills the frame.
//!
//! ARCH: pure data — no I/O, no rasterizer deps, no inline color.

// Consumed by the layout/scene generators and the rasterizer plans (02-02/03).
#![allow(dead_code)]

use glam::Vec3;

use crate::theme::Status;

/// Minimum half-extent: an idle box is always at least this big, so it never
/// collapses to a sub-pixel speck (CONT-02 floor).
pub const MIN_HALF: f32 = 0.3;

/// Maximum half-extent: a maxed-out box is clamped to this ceiling, so a hog
/// never dominates / fills the frame (CONT-02 ceiling).
pub const MAX_HALF: f32 = 1.2;

/// One box in the scene.
///
/// `id` is the stable identity (the layout key; later a container id). All
/// placement and sizing is a pure function of `id`/`group`, so the same entity
/// always lands in the same slot (the anti-jitter guarantee).
#[derive(Debug, Clone, Copy)]
pub struct Entity {
    /// Stable identity — the layout key (later maps to a container id).
    pub id: u32,
    /// World-space slot center, assigned by [`crate::world::layout`].
    pub position: Vec3,
    /// Per-axis half-size; the rasterizers scale the unit cube by this.
    pub half_extents: Vec3,
    /// Lifecycle status. REUSES [`crate::theme::Status`] — color flows through
    /// `Palette::status_color`; no parallel color/status type is introduced.
    pub status: Status,
    /// Synthetic network/group id — the layout grouping key. Phase 4 ENT-01
    /// turns a group into a network floor-plane.
    pub group: u16,
}

/// Map a normalized load proxy in `[0, 1]` onto a half-extent in
/// `[MIN_HALF, MAX_HALF]` via a compressive `sqrt` curve (CONT-02).
///
/// - Out-of-range / non-finite inputs are clamped (never NaN, never negative,
///   never zero): non-finite -> `MIN_HALF`, then `load` is clamped to `[0, 1]`.
/// - The `sqrt` curve front-loads growth, so the size gained per unit of load
///   is larger at the low end and compresses in the heavy tail — a busy host
///   reads big without a single hog swallowing the frame.
pub fn load_to_half_extent(_load: f32) -> f32 {
    // RED stub — replaced by the real clamped/compressive mapping in GREEN.
    0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_clamps_to_floor() {
        // Idle load maps to the floor, never zero/sub-pixel.
        assert_eq!(load_to_half_extent(0.0), MIN_HALF);
    }

    #[test]
    fn hog_clamps_to_ceiling() {
        // Full load maps to the ceiling, never beyond.
        assert_eq!(load_to_half_extent(1.0), MAX_HALF);
    }

    #[test]
    fn out_of_range_inputs_clamp() {
        // Negative and >1 inputs clamp to the floor/ceiling — no overshoot,
        // no negative half-extent.
        assert_eq!(load_to_half_extent(-5.0), MIN_HALF);
        assert_eq!(load_to_half_extent(99.0), MAX_HALF);
    }

    #[test]
    fn is_monotonic() {
        // More load -> bigger box.
        assert!(load_to_half_extent(0.25) < load_to_half_extent(0.75));
    }

    #[test]
    fn is_compressive_not_linear() {
        // sqrt front-loads: the size gained over the first quarter of load
        // exceeds the size gained over the last quarter. A linear map would
        // make these equal.
        let low_gain = load_to_half_extent(0.25) - load_to_half_extent(0.0);
        let high_gain = load_to_half_extent(1.0) - load_to_half_extent(0.75);
        assert!(
            low_gain > high_gain,
            "expected compressive curve: low_gain={low_gain}, high_gain={high_gain}"
        );
    }

    #[test]
    fn always_finite_and_positive() {
        // Any f32 input — including the non-finite ones — yields a finite,
        // strictly positive half-extent.
        for &load in &[f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 0.0, 0.5, 1.0, 1e30] {
            let h = load_to_half_extent(load);
            assert!(h.is_finite() && h > 0.0, "bad half-extent {h} for load {load}");
        }
    }
}
