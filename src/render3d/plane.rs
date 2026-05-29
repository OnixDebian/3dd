//! Wireframe quad rasterizer for ENT-01 (network floor-planes).
//!
//! One thin gray rectangle per network group sits just below the rack so the
//! viewer reads "this Z-band is one network". The rasterizer reuses the same
//! [`draw_thick_line`][super::raster::draw_thick_line] band-rasterizer the
//! wireframe-cube path uses, with the same [`EDGE_HALF_PX_SS`][super::raster::EDGE_HALF_PX_SS]
//! thickness, so the floor's lines read at the same visual weight as cube
//! wireframe edges.
//!
//! ARCH: pure (no I/O); takes a projector + framebuffer reference and writes
//! 4 line segments. Caller decides ordering — for the braille painter's-sort
//! path this is drawn FIRST (so cube faces overwrite it where they intersect
//! the floor's edges), and for the kitty z-buffer path the per-pixel z compare
//! handles ordering automatically.
//!
//! Color is uniform: the plan picks `palette.edge` for non-selected groups
//! and `palette.status_color(Running)` for the selected entity's group
//! (RESEARCH "ENT-01 Network Floor-Planes" — selected-group highlight). No
//! shading, no fog — the floor is a passive frame, not a light source.

#![allow(dead_code)]

use glam::{Vec2, Vec3};
use ratatui::style::Color;

use crate::render3d::framebuffer::Framebuffer;
use crate::render3d::project::Projector;
use crate::render3d::raster::{draw_thick_line, EDGE_HALF_PX_SS};

/// Rasterize one wireframe floor-plane: project the 4 corners through the
/// (supersampled) projector and draw the 4 edges as a band of half-thickness
/// [`EDGE_HALF_PX_SS`].
///
/// Skips the entire floor if ANY corner clips off-screen — partial floors
/// flickering at the frustum edge are worse than no floor that frame. The
/// floor sits at fixed world Y (carried inside `center.y`); `half_size_xz`
/// gives the quad's XZ extent so corners are `center ± (±hx, 0, ±hz)`.
pub fn rasterize_floor_plane(
    fb: &mut Framebuffer,
    projector: &Projector,
    center: Vec3,
    half_size_xz: Vec2,
    color: Color,
) {
    let y = center.y;
    // CCW from above (looking down -Y): SW, SE, NE, NW. Order doesn't matter
    // visually (the band-rasterizer is undirected), but pin it for clarity.
    let corners = [
        Vec3::new(center.x - half_size_xz.x, y, center.z - half_size_xz.y),
        Vec3::new(center.x + half_size_xz.x, y, center.z - half_size_xz.y),
        Vec3::new(center.x + half_size_xz.x, y, center.z + half_size_xz.y),
        Vec3::new(center.x - half_size_xz.x, y, center.z + half_size_xz.y),
    ];
    let mut projected = [(0.0f32, 0.0f32); 4];
    for (slot, c) in projected.iter_mut().zip(corners.iter()) {
        match projector.project(*c) {
            Some((x, y, _z)) => *slot = (x, y),
            None => return, // any corner clips -> skip the whole floor
        }
    }
    for i in 0..4 {
        let a = projected[i];
        let b = projected[(i + 1) % 4];
        draw_thick_line(fb, a, b, EDGE_HALF_PX_SS, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RenderConfig;
    use crate::render3d::framebuffer::Framebuffer;

    /// A camera looking from above so a floor at the origin projects as a
    /// visible quad inside the framebuffer.
    fn top_down_projector(viewport: (u32, u32)) -> Projector {
        Projector::new(
            Vec3::new(0.0, 6.0, 6.0),
            Vec3::ZERO,
            Vec3::Y,
            viewport,
            &RenderConfig::default(),
        )
    }

    #[test]
    fn rasterize_lights_some_pixels() {
        // A floor at y=0 centered on the origin with ample XZ extent must
        // light a non-trivial number of framebuffer pixels (the 4 projected
        // edges).
        let mut fb = Framebuffer::new(128, 128);
        let proj = top_down_projector((128, 128));
        rasterize_floor_plane(
            &mut fb,
            &proj,
            Vec3::ZERO,
            Vec2::new(2.0, 2.0),
            Color::Rgb(0x9A, 0x9A, 0xA8),
        );
        let lit = fb.lit_pixels().count();
        assert!(lit > 10, "expected the floor edges to light some pixels, got {lit}");
    }

    #[test]
    fn rasterize_skips_when_offscreen() {
        // A floor placed BEHIND the camera must not light any pixels (any
        // corner that clips -> skip the entire floor).
        let mut fb = Framebuffer::new(64, 64);
        let proj = top_down_projector((64, 64));
        // Behind the camera (eye is at +Z=6, so +Z=20 is behind in look_at_rh).
        rasterize_floor_plane(
            &mut fb,
            &proj,
            Vec3::new(0.0, 0.0, 30.0),
            Vec2::new(1.0, 1.0),
            Color::Rgb(0x9A, 0x9A, 0xA8),
        );
        assert_eq!(fb.lit_pixels().count(), 0);
    }
}
