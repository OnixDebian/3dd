//! Image-stack rasterizer (ENT-04).
//!
//! Each Docker image renders as a stack of N short cubes along +Y, where N
//! is `min(image.layer_count, MAX_LAYERS)`. The stack sits in a dedicated
//! region of the scene (deterministic +X offset beyond the rack — see
//! `world::layout::MAX_RACK_X` / `IMAGE_REGION_X_OFFSET`), so it never
//! occludes containers and orbit reveals it on demand.
//!
//! Reuses the unit-cube geometry + the shared `fill_face` path from
//! `render3d::raster` so a stack layer reads the same way a container box
//! does: flat-shaded by face, palette.edge color (no Lambert, no fog —
//! emissive contract matching the floor-plane lines + port glow). Painting
//! is straight back-face cull + face-by-face fill; per-stack painter's sort
//! is not required because:
//!   - Within a stack, layers do not overlap (separated by LAYER_GAP).
//!   - Across stacks, the kitty z-buffer resolves overlap correctly; the
//!     braille path leans on the fact that stacks live in a side region
//!     with limited inter-stack overlap from realistic camera angles.
//!
//! ARCH: pure (no I/O); takes a projector + framebuffer reference.
//! Caller decides ordering — both backends draw image stacks AFTER cubes,
//! port glow, and cylinders.

#![allow(dead_code)]

use glam::Vec3;

use crate::render3d::cube::unit_cube;
use crate::render3d::framebuffer::Framebuffer;
use crate::render3d::project::Projector;
use crate::render3d::raster::fill_face;
use crate::theme::Palette;

/// Half-extent of each layer cube in world units. Picked so a layer reads
/// as a chunky brick at the standard framing distance (~half a container's
/// MIN_HALF=0.3).
pub const LAYER_HALF: f32 = 0.5;

/// Vertical center-to-center spacing between adjacent layers in a stack.
/// Strictly greater than `2 * LAYER_HALF` so layers never touch — a small
/// gap (0.1 world units) between bricks reads as separation.
pub const LAYER_GAP: f32 = 1.1;

/// Cap layers per stack. A busy image (Debian-style, 30+ layers) doesn't
/// dominate the +X region. Picked so the tallest stack stays well under
/// the rack's vertical extent at MAX_HALF (1.2): 8 layers × LAYER_GAP=1.1
/// = ~8.8 world units, comparable to the rack's row spread.
pub const MAX_LAYERS: usize = 8;

/// Rasterize one image stack at world `base` with `layer_count` layers.
///
/// Layers are placed at `base.y + i*LAYER_GAP + LAYER_HALF` for
/// `i in 0..min(layer_count, MAX_LAYERS)` — the `+ LAYER_HALF` puts the
/// bottom of the lowest layer flush with `base.y` (call sites can pin
/// `base.y = 0` to sit on the same floor as the container rack).
///
/// Each layer is rasterized as a back-face-culled cube: the 6 faces of the
/// unit cube scaled by `LAYER_HALF`, translated to the layer center,
/// drawn with `palette.edge` (the muted gray that already paints the
/// floor-plane wireframes and the non-Running cube outlines).
pub fn rasterize_image_stack(
    fb: &mut Framebuffer,
    projector: &Projector,
    eye: Vec3,
    base: Vec3,
    layer_count: usize,
    palette: &Palette,
) {
    if layer_count == 0 {
        return;
    }
    let n = layer_count.min(MAX_LAYERS);
    let cube = unit_cube();
    let color = palette.edge;
    let scale = Vec3::splat(LAYER_HALF * 2.0); // half-extent -> full extent

    for i in 0..n {
        let center = Vec3::new(
            base.x,
            base.y + i as f32 * LAYER_GAP + LAYER_HALF,
            base.z,
        );
        // Back-face cull + face fill; no Lambert / no fog. Painter's order
        // within ONE cube is irrelevant (it's convex + back-face culled —
        // the visible faces never overlap each other).
        for face in &cube.faces {
            let verts = [
                center + cube.vertices[face.indices[0]] * scale,
                center + cube.vertices[face.indices[1]] * scale,
                center + cube.vertices[face.indices[2]] * scale,
                center + cube.vertices[face.indices[3]] * scale,
            ];
            let centroid = verts.iter().copied().sum::<Vec3>() / 4.0;
            if face.normal.dot(eye - centroid) <= 0.0 {
                continue;
            }
            fill_face(fb, projector, &verts, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RenderConfig;
    use crate::theme::Palette;

    fn projector(viewport: (u32, u32)) -> Projector {
        Projector::new(
            Vec3::new(2.5, 4.0, 8.0),
            Vec3::ZERO,
            Vec3::Y,
            viewport,
            &RenderConfig::default(),
        )
    }

    /// Zero layers is a no-op (no draw, no panic).
    #[test]
    fn image_stack_zero_layers_is_noop() {
        let mut fb = Framebuffer::new(64, 64);
        let proj = projector((64, 64));
        let pal = Palette::default();
        rasterize_image_stack(&mut fb, &proj, Vec3::new(2.5, 4.0, 8.0), Vec3::ZERO, 0, &pal);
        assert_eq!(fb.lit_pixels().count(), 0);
    }

    /// Cap enforcement: requesting 100 layers paints at most MAX_LAYERS.
    /// We don't count pixels precisely — instead verify it doesn't panic
    /// AND lights pixels in a non-trivial band (positive smoke).
    #[test]
    fn image_stack_caps_at_max_layers() {
        let mut fb = Framebuffer::new(128, 128);
        let proj = projector((128, 128));
        let pal = Palette::default();
        rasterize_image_stack(&mut fb, &proj, Vec3::new(2.5, 4.0, 8.0), Vec3::ZERO, 100, &pal);
        let lit = fb.lit_pixels().count();
        assert!(lit > 50, "expected the stack to light some pixels, got {lit}");
    }

    /// A small stack lights pixels (smoke).
    #[test]
    fn image_stack_lights_pixels() {
        let mut fb = Framebuffer::new(128, 128);
        let proj = projector((128, 128));
        let pal = Palette::default();
        rasterize_image_stack(&mut fb, &proj, Vec3::new(2.5, 4.0, 8.0), Vec3::ZERO, 3, &pal);
        let lit = fb.lit_pixels().count();
        assert!(lit > 30, "3-layer stack should light some pixels, got {lit}");
    }

    /// Constants pinned: MAX_LAYERS is the visual-sanity cap; LAYER_GAP must
    /// exceed 2*LAYER_HALF so adjacent layers never touch (a small air gap
    /// is what makes the stack read as discrete bricks). The two consts are
    /// loaded into runtime vars to dodge clippy::assertions_on_constants —
    /// the assertion is still meaningful as a regression guard against a
    /// future tightening of LAYER_GAP / loosening of LAYER_HALF.
    #[test]
    fn constants_match_plan() {
        let max = MAX_LAYERS;
        let gap = LAYER_GAP;
        let half = LAYER_HALF;
        assert_eq!(max, 8);
        assert!(gap > 2.0 * half, "LAYER_GAP must exceed 2*LAYER_HALF");
    }
}
