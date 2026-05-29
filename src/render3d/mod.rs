//! Pure 3D rendering math.
//!
//! ARCH: `render3d` is a pure, unit-testable module — no I/O, no terminal, no
//! globals. It owns the world→screen projection pipeline that everything the
//! rasterizer draws sits on top of.

pub mod cube;
pub mod framebuffer;
pub mod plane;
pub mod project;
pub mod raster;
pub mod scene_extras;

// Public API surface consumed by plan 05 (blit + orbiting camera).
#[allow(unused_imports)]
pub use cube::{unit_cube, Cube};
#[allow(unused_imports)]
pub use framebuffer::Framebuffer;
#[allow(unused_imports)]
pub use project::Projector;
#[allow(unused_imports)]
pub use raster::{render, render_scene};
#[allow(unused_imports)]
pub use scene_extras::{FloorPlane, PortLookup, SceneExtras};

#[allow(unused_imports)]
pub use self::rotate_y_about as rotate_y;

use glam::Vec3;

/// Raw camera input for [`render`]. This is the SOLE way `render3d` learns where
/// the camera is — it deliberately does NOT depend on a `Camera` type.
///
/// Plan 05 introduces `Camera` (the orbit controller); that type constructs a
/// `ViewParams` and feeds it in here. Keeping `render3d` camera-agnostic preserves
/// the plan 05 → 04 dependency direction (no forward/circular type reference) and
/// lets the headless raster tests build a view from literal `Vec3`s.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewParams {
    /// Camera position in world space.
    pub eye: Vec3,
    /// Point the camera looks at.
    pub target: Vec3,
    /// World up direction.
    pub up: Vec3,
    /// Vertical field of view, in radians (overrides `RenderConfig::fov` so the
    /// camera owns its lens; config keeps near/far/cell_aspect).
    pub fov: f32,
}

/// Rotate `point` about `center` around the world +Y (vertical) axis by `angle`
/// radians.
///
/// This is the per-box self-spin transform (the human's verify-tuning request:
/// each box spins IN PLACE around its own vertical axis, instead of the whole
/// scene orbiting under a moving camera). Rotating about +Y leaves the box's Y
/// coordinate untouched and rotates its X/Z about the box center. Applied to a
/// box's 8 world vertices (and, separately, to each axis-aligned face normal)
/// before projection, it keeps the box a rigid, still-convex solid — so the
/// braille painter's sort and the kitty z-buffer both remain correct.
pub fn rotate_y_about(point: Vec3, center: Vec3, angle: f32) -> Vec3 {
    let (s, c) = angle.sin_cos();
    let d = point - center;
    // Standard Y-axis rotation of the (x, z) plane; y is invariant.
    let x = d.x * c + d.z * s;
    let z = -d.x * s + d.z * c;
    center + Vec3::new(x, d.y, z)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_2;

    #[test]
    fn rotate_y_keeps_y_and_radius() {
        let center = Vec3::new(2.0, 1.0, -3.0);
        let p = center + Vec3::new(1.0, 0.5, 0.0);
        let r = rotate_y_about(p, center, 0.7);
        // Y is invariant under a Y-axis spin.
        assert!((r.y - p.y).abs() < 1e-6, "y changed under Y rotation");
        // Distance from the center in the XZ plane is preserved (rigid spin).
        let d0 = ((p.x - center.x).powi(2) + (p.z - center.z).powi(2)).sqrt();
        let d1 = ((r.x - center.x).powi(2) + (r.z - center.z).powi(2)).sqrt();
        assert!((d0 - d1).abs() < 1e-5, "XZ radius not preserved: {d0} vs {d1}");
    }

    #[test]
    fn rotate_y_quarter_turn_maps_x_to_neg_z() {
        // A quarter turn about +Y sends +X toward -Z (right-handed, our winding).
        let center = Vec3::ZERO;
        let p = Vec3::new(1.0, 0.0, 0.0);
        let r = rotate_y_about(p, center, FRAC_PI_2);
        assert!(r.x.abs() < 1e-6, "x should be ~0 after quarter turn: {}", r.x);
        assert!((r.z + 1.0).abs() < 1e-6, "z should be ~-1 after quarter turn: {}", r.z);
    }

    #[test]
    fn rotate_y_zero_angle_is_identity() {
        let center = Vec3::new(1.0, 2.0, 3.0);
        let p = Vec3::new(4.0, 5.0, 6.0);
        let r = rotate_y_about(p, center, 0.0);
        assert!((r - p).length() < 1e-6, "zero angle must be identity");
    }
}
