//! Cylinder (N-gon prism) rasterizer for ENT-03 volume disks.
//!
//! A cylinder is modelled as 2N corners (top + bottom octagon) + 2 polygonal
//! caps + N side quads. The side quads share an axis and centroid-Z sort
//! breaks down (RESEARCH Pitfall H) — we instead sort sides by **yaw-to-camera
//! dot** (each side's outward XZ normal dotted with the eye-to-centroid
//! direction). Front-facing sides paint LAST so they overwrite the back-facing
//! ones that painted first; seams stay stable as the camera rotates.
//!
//! Caps are sorted top-vs-bottom by camera distance: the farther cap paints
//! first (then the sides; then the nearer cap paints on top). For an above-
//! the-rack camera that means: top cap paints last (visible), bottom cap is
//! occluded by the sides (correct).
//!
//! ARCH: pure (no I/O); takes a projector + framebuffer reference. Caller
//! decides ordering — the braille `render_scene` draws cubes FIRST, then
//! port glow, then cylinders, then image stacks. Color is flat (no Lambert,
//! no fog) so a volume "disk" reads as a passive resource indicator rather
//! than a light source — same emissive contract as ports.

#![allow(dead_code)]

use glam::Vec3;
use ratatui::style::Color;

use crate::render3d::framebuffer::Framebuffer;
use crate::render3d::project::Projector;
use crate::render3d::raster::{fill_face, fill_triangle};

/// Default octagon (8 sides). Cheap to project + sort, reads as a smooth
/// disk at the braille resolution. Higher counts add cost without visible
/// gain at terminal-cell resolution.
pub const DEFAULT_SIDES: usize = 8;

/// Rasterize a cylinder at `center` with `radius` (XZ extent) and `height`
/// (Y extent above `center.y`). Uses the default octagonal section.
///
/// `eye` is the camera position in world space — needed for the per-face
/// yaw-to-camera dot sort that avoids seam flicker on rotation (Pitfall H).
pub fn rasterize_cylinder(
    fb: &mut Framebuffer,
    projector: &Projector,
    eye: Vec3,
    center: Vec3,
    radius: f32,
    height: f32,
    color: Color,
) {
    rasterize_cylinder_n(
        fb,
        projector,
        eye,
        center,
        radius,
        height,
        DEFAULT_SIDES,
        color,
    );
}

/// Custom-side-count variant. `sides >= 3` (lower values degenerate); for
/// terminal rendering `sides == 8` (DEFAULT_SIDES) is the sweet spot.
#[allow(clippy::too_many_arguments)]
pub fn rasterize_cylinder_n(
    fb: &mut Framebuffer,
    projector: &Projector,
    eye: Vec3,
    center: Vec3,
    radius: f32,
    height: f32,
    sides: usize,
    color: Color,
) {
    if sides < 3 || radius <= 0.0 || height <= 0.0 {
        return;
    }
    use std::f32::consts::TAU;
    // `center` is the BOTTOM of the cylinder; height extends upward. This
    // matches the call site: `center = entity.position + Y * entity.half_extents.y`
    // (the top face of the cube), so the cylinder sits ON the cube.
    let bot_y = center.y;
    let top_y = center.y + height;

    let mut top: Vec<Vec3> = Vec::with_capacity(sides);
    let mut bot: Vec<Vec3> = Vec::with_capacity(sides);
    for i in 0..sides {
        let ang = TAU * i as f32 / sides as f32;
        let (s, c) = ang.sin_cos();
        let x = center.x + radius * c;
        let z = center.z + radius * s;
        top.push(Vec3::new(x, top_y, z));
        bot.push(Vec3::new(x, bot_y, z));
    }

    // PITFALL H: per-face yaw-to-camera dot sort. Each side quad's outward
    // XZ normal points from the cylinder axis to the quad centroid (pure
    // XZ vector, ignoring Y). Dot with (eye - centroid) — higher = more
    // front-facing. Sort ASCENDING so back-facing sides paint FIRST and
    // front-facing sides paint LAST (overwriting back-facing on overlap).
    let mut side_order: Vec<(usize, f32)> = (0..sides)
        .map(|i| {
            let centroid = (top[i] + top[(i + 1) % sides] + bot[(i + 1) % sides] + bot[i]) * 0.25;
            // Pure XZ normal (the cylinder's axis is +Y; sides face outward
            // in the XZ plane).
            let normal_xz = Vec3::new(centroid.x - center.x, 0.0, centroid.z - center.z)
                .normalize_or_zero();
            let to_cam = (eye - centroid).normalize_or_zero();
            let dot = normal_xz.dot(to_cam);
            (i, dot)
        })
        .collect();
    side_order.sort_by(|a, b| {
        a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)
    });

    // Cap depth sort: whichever cap is farther from the camera paints
    // FIRST (so the nearer cap can overwrite it after the sides paint).
    let top_centroid = Vec3::new(center.x, top_y, center.z);
    let bot_centroid = Vec3::new(center.x, bot_y, center.z);
    let top_d = (eye - top_centroid).length_squared();
    let bot_d = (eye - bot_centroid).length_squared();
    let (back_cap, front_cap, back_is_top): (&Vec<Vec3>, &Vec<Vec3>, bool) = if top_d > bot_d {
        (&top, &bot, true)
    } else {
        (&bot, &top, false)
    };
    let _ = back_is_top; // explicit `_` so future visibility / culling reads can grow here.

    // 1. Back cap (farther from camera) — paint first so sides + front cap
    //    can overwrite it where they project on top.
    paint_polygon(fb, projector, back_cap, color);
    // 2. Sides — back-to-front by yaw-to-camera dot (Pitfall H).
    for &(i, _) in &side_order {
        let a = top[i];
        let b = top[(i + 1) % sides];
        let c = bot[(i + 1) % sides];
        let d = bot[i];
        fill_face(fb, projector, &[a, b, c, d], color);
    }
    // 3. Front cap (closer to camera) — last so it overwrites whatever side
    //    paint landed where it projects.
    paint_polygon(fb, projector, front_cap, color);
}

/// Fan-triangulate and fill a convex N-gon (the top / bottom cap) into the
/// framebuffer. Skips the entire polygon if any vertex clips off-screen —
/// scene-framing keeps the rack in-frustum so this is rare.
fn paint_polygon(fb: &mut Framebuffer, projector: &Projector, ring: &[Vec3], color: Color) {
    if ring.len() < 3 {
        return;
    }
    let mut pts: Vec<(f32, f32)> = Vec::with_capacity(ring.len());
    for &p in ring {
        match projector.project(p) {
            Some((x, y, _z)) => pts.push((x, y)),
            None => return, // any vertex clips -> skip the cap
        }
    }
    // Fan triangulation from vertex 0.
    for i in 1..pts.len() - 1 {
        fill_triangle(fb, pts[0], pts[i], pts[i + 1], color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RenderConfig;
    use crate::render3d::framebuffer::Framebuffer;

    /// A camera looking toward the origin from a height + offset, so a
    /// cylinder at the origin projects as a visible disk.
    fn projector(viewport: (u32, u32)) -> Projector {
        Projector::new(
            Vec3::new(2.5, 3.0, 5.0),
            Vec3::ZERO,
            Vec3::Y,
            viewport,
            &RenderConfig::default(),
        )
    }

    /// A cylinder behind the camera must NOT panic; should just light no
    /// pixels (the caps clip; the sides project but with corners off-screen
    /// get skipped at projector.project=None).
    #[test]
    fn rasterize_cylinder_does_not_panic_on_offscreen() {
        let mut fb = Framebuffer::new(64, 64);
        let proj = projector((64, 64));
        // Place cylinder well behind the camera (eye is at +Z=5 looking
        // toward -Z; +Z=30 is behind near plane).
        rasterize_cylinder(
            &mut fb,
            &proj,
            Vec3::new(2.5, 3.0, 5.0),
            Vec3::new(0.0, 0.0, 30.0),
            0.3,
            0.5,
            Color::Rgb(80, 140, 140),
        );
        // Nothing lit because every cap vertex clips.
        assert_eq!(fb.lit_pixels().count(), 0);
    }

    /// A normal-position cylinder lights a meaningful number of pixels.
    #[test]
    fn rasterize_cylinder_lights_pixels() {
        let mut fb = Framebuffer::new(128, 128);
        let proj = projector((128, 128));
        rasterize_cylinder(
            &mut fb,
            &proj,
            Vec3::new(2.5, 3.0, 5.0),
            Vec3::ZERO,
            0.5,
            1.0,
            Color::Rgb(80, 140, 140),
        );
        let lit = fb.lit_pixels().count();
        assert!(lit > 50, "expected the cylinder to fill some pixels, got {lit}");
    }

    /// Degenerate inputs (zero radius / zero height / sides < 3) are no-ops,
    /// never panic.
    #[test]
    fn rasterize_cylinder_degenerate_inputs_are_noop() {
        let mut fb = Framebuffer::new(32, 32);
        let proj = projector((32, 32));
        let teal = Color::Rgb(80, 140, 140);

        rasterize_cylinder(&mut fb, &proj, Vec3::new(2.5, 3.0, 5.0), Vec3::ZERO, 0.0, 1.0, teal);
        rasterize_cylinder(&mut fb, &proj, Vec3::new(2.5, 3.0, 5.0), Vec3::ZERO, 0.5, 0.0, teal);
        rasterize_cylinder_n(
            &mut fb,
            &proj,
            Vec3::new(2.5, 3.0, 5.0),
            Vec3::ZERO,
            0.5,
            1.0,
            2,
            teal,
        );
        assert_eq!(fb.lit_pixels().count(), 0);
    }

    /// DEFAULT_SIDES is 8 — pin the octagon assumption so a downstream
    /// refactor doesn't quietly bump it (8 reads as a smooth disk at
    /// terminal resolution; higher counts add cost without visible gain).
    #[test]
    fn default_sides_is_octagon() {
        assert_eq!(DEFAULT_SIDES, 8);
    }
}
