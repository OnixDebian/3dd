//! The `world` data layer: synthetic scene of dozens of boxes.
//!
//! Turns "one cube" into "a rack of boxes" as pure, deterministic data. An
//! [`Entity`] carries a stable identity, a layout-assigned world slot, a
//! clamped/compressive size, and a reused [`crate::theme::Status`]. The
//! [`layout`] algorithm gives every entity a stable network-grouped slot
//! (CONT-05), and [`synthetic_scene`] produces a full [`World`] plus
//! [`SceneBounds`] the camera frames.
//!
//! ARCH: pure module — no I/O, no rasterizer deps, no inline RGB. Color is
//! resolved downstream by the palette via [`crate::theme::Status`].
//!
//! NOTE: forward-facing API — the rasterizer plans (02-02 braille, 02-03 kitty)
//! are the first consumers of `World`/`Entity`/`SceneBounds`, so until then they
//! report as dead code. The module-level `allow` keeps the surface in place
//! without masking dead code elsewhere (mirrors `theme`/`render3d`).
#![allow(dead_code)]

pub mod entity;
pub mod layout;
pub mod live;
pub mod scene;

// Re-exported for the rasterizer plans (02-02 braille, 02-03 kitty); not all
// consumed within the crate yet, so silence the until-then unused warning
// (mirrors `render3d`'s public-API re-exports).
#[allow(unused_imports)]
pub use entity::{load_to_half_extent, Entity, MAX_HALF, MIN_HALF};
#[allow(unused_imports)]
pub use layout::layout;
#[allow(unused_imports)]
pub use live::{DockerMsg, LiveWorld};
#[allow(unused_imports)]
pub use scene::{synthetic_scene, SceneBounds};

/// A fully-populated synthetic scene: every box as data plus the enclosing
/// [`SceneBounds`] the camera frames.
#[derive(Debug, Clone)]
pub struct World {
    /// Every box in the scene, in stable slots.
    pub entities: Vec<Entity>,
    /// Axis-aligned bounds + center + bounding-sphere radius of the whole scene.
    pub bounds: SceneBounds,
}
