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

pub mod entity;

pub use entity::{load_to_half_extent, Entity, MAX_HALF, MIN_HALF};
