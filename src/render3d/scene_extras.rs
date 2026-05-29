//! Optional-primitives container shared across both backends (W7 closure).
//!
//! [`SceneExtras`] is the SOLE place per-frame primitive references travel into
//! the renderers. 04-04 ships floors (network floor-planes for ENT-01) + ports
//! (per-entity port glow lookup for ENT-02). 04-05 extends the same struct with
//! `cylinders` (ENT-03 volume cylinders) + `image_stacks` (ENT-04 image stack
//! columns) WITHOUT growing the renderer entry-point signatures — the struct
//! grows fields; both renderers update inner consumption only.
//!
//! Both `render3d::render_scene` (braille) and `kitty::render_rgba` accept
//! `extras: &SceneExtras<'_>` as a single parameter — selected_id and
//! pulse_phase remain 04-03's own parameters (this plan does not touch the
//! Tab-selection plumbing). Callers build a fresh `SceneExtras` per frame from
//! the live world (no clones; the struct holds borrows).
//!
//! ARCH: pure data container — no I/O, no projection. Construction and
//! consumption sit at the call boundary.

#![allow(dead_code)]

use std::collections::HashMap;

use glam::{Vec2, Vec3};
use ratatui::style::Color;

use crate::docker::domain::PortSummary;

/// One network floor-plane: a wireframe quad at fixed world `y` per group,
/// sized to bound the group's slot footprint plus the layout's padding.
///
/// `center.y` is the FLOOR Y (≈ -0.1 below the box floor); `half_size_xz`
/// gives the quad's half-extent on each XZ axis (the quad spans
/// `center ± half_size_xz` in world space). `color` is the palette-resolved
/// uniform tint applied to all 4 edges (no per-edge shading).
#[derive(Debug, Clone, Copy)]
pub struct FloorPlane {
    pub center: Vec3,
    pub half_size_xz: Vec2,
    pub color: Color,
}

/// Per-entity port lookup: borrows the live [`PortSummary`] slices owned by
/// `LiveWorld::snapshot(id)` (which surfaces the bollard-free port list
/// `ContainerSnapshot.ports`). Keys are flat `Entity.id` values, so the
/// renderer can do a direct `extras.ports.get(&entity.id)` per entity.
pub type PortLookup<'a> = HashMap<u32, &'a [PortSummary]>;

/// One volume cylinder (ENT-03 / 04-05): a passive teal disk that sits on
/// top of a container with at least one mount. `center` is the BOTTOM of
/// the cylinder (the cube's top face); `radius` is the XZ extent;
/// `height` extends UPWARD from `center.y` by the
/// `proxy_volume_height(mount_count)` mapping (RESEARCH Code Example —
/// sqrt-compressive, MIN_H..MAX_H over mount_count 0..REF).
///
/// `color` is the palette-resolved teal (`palette.volume`) so the renderer
/// reads it from the palette, not from an inline RGB. Containers with
/// zero mounts are FILTERED at build time so this list never includes
/// "phantom" cylinders.
#[derive(Debug, Clone, Copy)]
pub struct Cylinder {
    pub center: Vec3,
    pub radius: f32,
    pub height: f32,
    pub color: Color,
}

/// One image stack (ENT-04 / 04-05): a column of short cubes in the +X
/// side region beyond the rack. `base` is the world position of the bottom
/// of the lowest layer (`x` = `MAX_RACK_X + IMAGE_REGION_X_OFFSET`, `z` =
/// `i * IMAGE_STACK_SPACING_Z` where `i` is the image's BTreeMap-id index
/// — deterministic / stable across runs, W9 closure); `layer_count` is
/// the image's `root_fs.layers.len()` value (the renderer caps it at
/// `stack::MAX_LAYERS` so a busy image doesn't tower).
#[derive(Debug, Clone, Copy)]
pub struct ImageStack {
    pub base: Vec3,
    pub layer_count: usize,
}

/// Optional-primitives container — the W7 closure: a single struct that
/// carries every Phase-4-and-later non-cube primitive into both backends.
///
/// Fields (post-04-05):
/// - `floors`: one [`FloorPlane`] per non-empty network group (04-04).
/// - `ports`: per-entity port-summary borrow lookup (04-04, driven by
///   `LiveWorld::snapshot`).
/// - `cylinders`: per-frame volume cylinders for containers with
///   `mount_count >= 1` (04-05, ENT-03).
/// - `image_stacks`: per-frame image stacks, one per [`ImageSnapshot`] in
///   the LiveWorld's `images` BTreeMap (04-05, ENT-04).
///
/// Both renderers take `&SceneExtras` (one parameter) and pattern-match
/// internally on which extras to draw, so adding a field never changes the
/// entry-point signatures (W7 closure: the struct grows in place; no
/// signature growth). The lifetime parameter ties the borrows to the
/// per-frame slices the caller built — no clones inside the renderer.
pub struct SceneExtras<'a> {
    pub floors: &'a [FloorPlane],
    pub ports: &'a PortLookup<'a>,
    pub cylinders: &'a [Cylinder],
    pub image_stacks: &'a [ImageStack],
}

impl<'a> SceneExtras<'a> {
    /// Construct extras from explicit borrows. The `floors` / `ports` /
    /// `cylinders` / `image_stacks` slices must outlive the renderer call
    /// but never the next frame — both backends build fresh per frame from
    /// the live world.
    pub fn new(
        floors: &'a [FloorPlane],
        ports: &'a PortLookup<'a>,
        cylinders: &'a [Cylinder],
        image_stacks: &'a [ImageStack],
    ) -> Self {
        Self {
            floors,
            ports,
            cylinders,
            image_stacks,
        }
    }
}
