//! 3D projection pipeline: world point → view → clip → NDC → screen pixel.
//!
//! Built on glam. This is the single authoritative place two things happen:
//!   1. The terminal cell-aspect correction (via `RenderConfig::cell_aspect`,
//!      folded into the perspective `aspect` term exactly once).
//!   2. The NDC→screen Y-flip (math-coords → top-left framebuffer grid), pinned
//!      by `up_world_point_maps_to_upper_half`.
//!
//! ARCH: pure module — no I/O, no terminal, no globals.

// Public projection API; consumed by plans 04/05, exercised by tests today.
#![allow(dead_code)]

use glam::{Mat4, Vec3};

use crate::config::RenderConfig;

/// Projects world points to screen pixels (braille sub-pixel space).
///
/// Holds the precomputed `view` (for clipping in view space) and the combined
/// `view_proj` (for the perspective divide), plus the pixel viewport.
#[derive(Debug, Clone)]
pub struct Projector {
    view: Mat4,
    view_proj: Mat4,
    px_w: f32,
    px_h: f32,
    near: f32,
}

impl Projector {
    /// Build a projector from a camera (`eye`/`target`/`up`), a pixel viewport
    /// `(px_w, px_h)` in braille sub-pixels, and a [`RenderConfig`].
    ///
    /// The perspective `aspect` argument incorporates `cell_aspect` as a SINGLE
    /// named factor so the same world-space unit length maps to equal on-screen
    /// extents horizontally and vertically (a unit cube reads as cubic, not a
    /// squashed brick — PITFALLS.md Pitfall 2).
    pub fn new(
        eye: Vec3,
        target: Vec3,
        up: Vec3,
        viewport_px: (u32, u32),
        config: &RenderConfig,
    ) -> Self {
        let px_w = viewport_px.0 as f32;
        let px_h = viewport_px.1 as f32;

        let view = Mat4::look_at_rh(eye, target, up);

        // The ONE place cell_aspect is applied: divide the pixel aspect by the
        // cell-aspect correction so vertical squash is undone in projection.
        let aspect = (px_w / px_h) / config.cell_aspect;
        let proj = Mat4::perspective_rh(config.fov, aspect, config.near, config.far);

        Self {
            view,
            view_proj: proj * view,
            px_w,
            px_h,
            near: config.near,
        }
    }

    /// Project a world point to `(screen_x, screen_y, depth)`, or `None` if the
    /// point is behind the camera / outside the view frustum.
    ///
    /// `depth` is NDC z in `[-1, 1]` (smaller = nearer), suitable for painter's
    /// sorting. Clipping happens BEFORE the perspective divide so behind-camera
    /// points never wrap to bogus on-screen coordinates.
    pub fn project(&self, world: Vec3) -> Option<(f32, f32, f32)> {
        // Clip in view space first: RH view space looks down -Z, so visible
        // points have z <= -near. Anything at or behind the near plane is out.
        let view_pos = self.view.transform_point3(world);
        if view_pos.z > -self.near {
            return None;
        }

        // Perspective divide → NDC (≈ [-1, 1] on each axis inside the frustum).
        let ndc = self.view_proj.project_point3(world);
        if !in_frustum(ndc) {
            return None;
        }

        Some(ndc_to_screen(ndc, self.px_w, self.px_h))
    }
}

/// True if an NDC point lies inside the canonical view volume.
/// glam uses a `[-1, 1]` z range (OpenGL-style) for `perspective_rh`.
fn in_frustum(ndc: Vec3) -> bool {
    (-1.0..=1.0).contains(&ndc.x)
        && (-1.0..=1.0).contains(&ndc.y)
        && (-1.0..=1.0).contains(&ndc.z)
}

/// Map NDC `[-1, 1]` → screen pixels. THE single Y-flip lives here.
///
/// `x = (ndc.x*0.5 + 0.5) * px_w` and `y = (1 - (ndc.y*0.5 + 0.5)) * px_h`.
/// The Y term is flipped because NDC is math-coords (+y up) while the
/// framebuffer is a top-left grid (+y down): world-up → smaller screen y.
/// Returns `(x, y, depth)` where depth = ndc.z passed through for sorting.
fn ndc_to_screen(ndc: Vec3, px_w: f32, px_h: f32) -> (f32, f32, f32) {
    let x = (ndc.x * 0.5 + 0.5) * px_w;
    let y = (1.0 - (ndc.y * 0.5 + 0.5)) * px_h;
    (x, y, ndc.z)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RenderConfig;
    use glam::Vec3;

    /// A camera looking down -Z at the origin, with world +Y as up.
    /// Square pixel viewport so any X/Y asymmetry is purely aspect correction.
    const VIEWPORT: (u32, u32) = (200, 200);

    fn camera() -> (Vec3, Vec3, Vec3) {
        let eye = Vec3::new(0.0, 0.0, 5.0);
        let target = Vec3::ZERO;
        let up = Vec3::Y;
        (eye, target, up)
    }

    fn projector(config: &RenderConfig) -> Projector {
        let (eye, target, up) = camera();
        Projector::new(eye, target, up, VIEWPORT, config)
    }

    #[test]
    fn target_projects_to_viewport_center() {
        let cfg = RenderConfig::default();
        let p = projector(&cfg);
        let (_eye, target, _up) = camera();

        let (sx, sy, _depth) = p.project(target).expect("target must be on-screen");

        let cx = VIEWPORT.0 as f32 / 2.0;
        let cy = VIEWPORT.1 as f32 / 2.0;
        assert!((sx - cx).abs() < 1.0, "center x: got {sx}, want ~{cx}");
        assert!((sy - cy).abs() < 1.0, "center y: got {sy}, want ~{cy}");
    }

    #[test]
    fn unit_cube_footprint_is_cubic() {
        // Cubic, not squashed. A braille dot is ~1:2 (twice as tall as wide), so
        // a cube that READS as cubic on screen must occupy `cell_aspect` times
        // as many dots horizontally as vertically. With a SQUARE pixel viewport
        // the only source of asymmetry is the aspect-correction term, so we
        // assert `bbox_w ≈ cell_aspect * bbox_h`. (At cell_aspect=1.0 — perfectly
        // square dots — this collapses to the literal width==height case.)
        let cfg = RenderConfig::default();
        let p = projector(&cfg);

        let corners = unit_cube_corners(Vec3::ZERO);
        let (w, h) = projected_bbox(&p, &corners);

        let expected_w = h * cfg.cell_aspect;
        let rel_err = (w - expected_w).abs() / expected_w;
        assert!(
            rel_err < 0.1,
            "cube not cubic on-screen: w={w}, h={h}, cell_aspect={}, \
             expected_w≈{expected_w}, rel_err={rel_err}",
            cfg.cell_aspect
        );
    }

    #[test]
    fn up_world_point_maps_to_upper_half() {
        // PIN the Y-flip convention: a point offset from the target along the
        // camera's +up direction must project to a STRICTLY SMALLER screen y
        // (top-left framebuffer convention). Not human-eyeballed — locked here.
        let cfg = RenderConfig::default();
        let p = projector(&cfg);
        let (_eye, target, up) = camera();

        let above = target + up * 0.5;

        let (_tx, ty, _td) = p.project(target).expect("target on-screen");
        let (_ux, uy, _ud) = p.project(above).expect("up point on-screen");

        assert!(
            uy < ty,
            "up world point must land higher (smaller y): up_y={uy}, target_y={ty}"
        );
    }

    #[test]
    fn behind_camera_point_is_clipped() {
        // A point well behind the camera (eye is at +5 Z looking toward -Z, so
        // anything at large +Z is behind the near plane) must return None, not
        // a wrapped/garbage on-screen coordinate.
        let cfg = RenderConfig::default();
        let p = projector(&cfg);

        let behind = Vec3::new(0.0, 0.0, 50.0);
        assert!(
            p.project(behind).is_none(),
            "behind-camera point must clip to None, got {:?}",
            p.project(behind)
        );
    }

    #[test]
    fn cell_aspect_is_load_bearing() {
        // Changing cell_aspect must change the cube's on-screen aspect ratio in
        // the expected direction, proving the factor is actually wired in (and
        // single-sourced). Larger cell_aspect stretches X relative to Y, so the
        // footprint width grows relative to its height.
        let corners = unit_cube_corners(Vec3::ZERO);

        let cfg_low = RenderConfig { cell_aspect: 1.0, ..RenderConfig::default() };
        let cfg_high = RenderConfig { cell_aspect: 3.0, ..RenderConfig::default() };

        let (w_low, h_low) = projected_bbox(&projector(&cfg_low), &corners);
        let (w_high, h_high) = projected_bbox(&projector(&cfg_high), &corners);

        let ratio_low = w_low / h_low;
        let ratio_high = w_high / h_high;

        assert!(
            ratio_high > ratio_low,
            "increasing cell_aspect must widen footprint: low={ratio_low}, high={ratio_high}"
        );
    }

    // --- helpers ---

    fn unit_cube_corners(center: Vec3) -> Vec<Vec3> {
        let h = 0.5;
        let mut v = Vec::with_capacity(8);
        for &dx in &[-h, h] {
            for &dy in &[-h, h] {
                for &dz in &[-h, h] {
                    v.push(center + Vec3::new(dx, dy, dz));
                }
            }
        }
        v
    }

    /// Returns (bbox_width, bbox_height) of the projected screen footprint.
    fn projected_bbox(p: &Projector, points: &[Vec3]) -> (f32, f32) {
        let mut min_x = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for &pt in points {
            let (sx, sy, _d) = p.project(pt).expect("cube corner must be on-screen");
            min_x = min_x.min(sx);
            max_x = max_x.max(sx);
            min_y = min_y.min(sy);
            max_y = max_y.max(sy);
        }
        (max_x - min_x, max_y - min_y)
    }
}
