//! 3D projection pipeline — RED stub.
//!
//! Intentionally unimplemented so the tests fail before the real math lands.

use glam::Vec3;

use crate::config::RenderConfig;

/// Projects world points to screen pixels (braille sub-pixel space).
#[derive(Debug, Clone)]
pub struct Projector {
    _eye: Vec3,
}

impl Projector {
    /// Build a projector from a camera and a pixel viewport.
    pub fn new(
        eye: Vec3,
        _target: Vec3,
        _up: Vec3,
        _viewport_px: (u32, u32),
        _config: &RenderConfig,
    ) -> Self {
        Self { _eye: eye }
    }

    /// Project a world point to `(screen_x, screen_y, depth)` or `None` if clipped.
    pub fn project(&self, _world: Vec3) -> Option<(f32, f32, f32)> {
        // RED: not implemented yet.
        None
    }
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
    fn unit_cube_footprint_is_square() {
        // With a SQUARE pixel viewport, a unit cube facing the camera must
        // project to a footprint as wide as it is tall (cubic, not a brick).
        let cfg = RenderConfig::default();
        let p = projector(&cfg);

        let corners = unit_cube_corners(Vec3::ZERO);
        let (w, h) = projected_bbox(&p, &corners);

        let rel_err = (w - h).abs() / w;
        assert!(
            rel_err < 0.1,
            "cube footprint not square: w={w}, h={h}, rel_err={rel_err}"
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
