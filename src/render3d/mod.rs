//! Pure 3D rendering math.
//!
//! ARCH: `render3d` is a pure, unit-testable module — no I/O, no terminal, no
//! globals. It owns the world→screen projection pipeline that everything the
//! rasterizer draws sits on top of.

pub mod cube;
pub mod framebuffer;
pub mod project;

// Public API surface consumed by plan 05 (blit + orbiting camera).
#[allow(unused_imports)]
pub use cube::{unit_cube, Cube};
#[allow(unused_imports)]
pub use framebuffer::Framebuffer;
#[allow(unused_imports)]
pub use project::Projector;

use glam::Vec3;

/// Raw camera input for [`render`]. This is the SOLE way `render3d` learns where
/// the camera is — it deliberately does NOT depend on a `Camera` type.
///
/// Plan 05 introduces `Camera` (the orbit controller); that type constructs a
/// `ViewParams` and feeds it in here. Keeping `render3d` camera-agnostic preserves
/// the plan 05 → 04 dependency direction (no forward/circular type reference) and
/// lets the headless raster tests build a view from literal `Vec3`s.
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)] // constructed by `render` (this plan) and plan 05's Camera
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
