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
        // In-frustum NDC, then map to screen. `ndc` already rejects behind-near
        // points; here we additionally require the point be inside the canonical
        // view volume on every axis (the rasterizer's drop-on-clip rule).
        let ndc = self.ndc(world)?;
        if !in_frustum(ndc) {
            return None;
        }
        Some(ndc_to_screen(ndc, self.px_w, self.px_h))
    }

    /// Project a world point to raw NDC (`Vec3`, ≈ `[-1, 1]` per axis inside the
    /// frustum), or `None` if it is at/behind the near plane.
    ///
    /// Unlike [`Projector::project`] this does NOT reject points outside the
    /// `[-1, 1]` box — it returns the NDC even when a coordinate exceeds 1, so
    /// callers (the camera's projected-AABB framing) can MEASURE how far past the
    /// edge a corner reaches and solve a fit. Clipping in view space still happens
    /// first so behind-camera points never wrap to bogus coordinates.
    pub fn ndc(&self, world: Vec3) -> Option<Vec3> {
        // Clip in view space first: RH view space looks down -Z, so visible
        // points have z <= -near. Anything at or behind the near plane is out.
        let view_pos = self.view.transform_point3(world);
        if view_pos.z > -self.near {
            return None;
        }
        Some(self.view_proj.project_point3(world))
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
        // Cubic, not squashed. With a SQUARE pixel viewport the only source
        // of horizontal-vs-vertical asymmetry is the `cell_aspect` term in
        // the projector, so the unit cube's screen-space bounding box must
        // satisfy `bbox_w ≈ cell_aspect * bbox_h`. RV4 (05-06): default
        // `cell_aspect = 1.0` (typical-2:1-cell parity), so this collapses
        // to the literal `bbox_w ≈ bbox_h` case. Pre-RV4 the default was
        // 2.0 (the assertion expected a 2:1 horizontal stretch).
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

    /// RV4 (05-06) PARITY PIN — replaces the RV3 "asymmetric by design"
    /// tripwire test. The braille tier's `cell_aspect` is now derived from
    /// the LIVE terminal cell pixel size via
    /// [`RenderConfig::braille_cell_aspect_for_cell`]; the central
    /// invariant is that, at typical monospace cells (~2:1 height:width),
    /// the braille projector's perspective aspect equals the kitty
    /// projector's. Pre-RV4 they differed by 2x (root cause of the user's
    /// "braille is flattened compared to kitty" feedback).
    ///
    /// We exercise BOTH tier formulas on the same logical viewport and
    /// assert their resulting `Mat4::perspective_rh` aspect values agree
    /// within a tight tolerance. If a future refactor accidentally
    /// reintroduces the asymmetry (e.g. re-pinning `cell_aspect` to a
    /// static constant), this test FAILS and the per-cell rustdoc in
    /// `config::RenderConfig` points to the math.
    #[test]
    fn rv4_braille_and_kitty_projector_aspects_match_at_typical_cell() {
        use glam::Mat4;
        // Same logical viewport for both tiers: 80 cells wide × 30 rows.
        let cells = (80u32, 30u32);
        // Typical monospace cell: 10×20 (height:width = 2:1).
        let cell_px = (10u16, 20u16);

        // Braille tier projector aspect — derives cell_aspect from the
        // LIVE cell size (RV4 helper). At 2:1 cells this yields 1.0.
        let dyn_aspect = RenderConfig::braille_cell_aspect_for_cell(cell_px);
        let braille_cfg = RenderConfig { cell_aspect: dyn_aspect, ..RenderConfig::default() };
        let braille_px = (cells.0 * 2, cells.1 * 4);
        let braille_aspect =
            (braille_px.0 as f32 / braille_px.1 as f32) / braille_cfg.cell_aspect;

        // Kitty tier projector aspect — real square pixels, cell_aspect=1.0.
        let kitty_cfg = RenderConfig { cell_aspect: 1.0, ..RenderConfig::default() };
        let kitty_px = (cells.0 * cell_px.0 as u32, cells.1 * cell_px.1 as u32);
        let kitty_aspect =
            (kitty_px.0 as f32 / kitty_px.1 as f32) / kitty_cfg.cell_aspect;

        // RV4: braille MUST match kitty within float epsilon at typical
        // cells. Both reduce to W/(2H) by the math in the rustdoc.
        let delta = (braille_aspect - kitty_aspect).abs();
        assert!(
            delta < 1e-5,
            "RV4 parity broken: braille_aspect={braille_aspect}, kitty_aspect={kitty_aspect}, \
             delta={delta}. See `config::RenderConfig` rustdoc 'Braille vs kitty perspective \
             parity (05-06 RV4)' for the derivation; if you intentionally re-introduced the \
             asymmetry, also update `Camera::frame_scene_with_aspect` callers."
        );

        // Sanity: building both projection matrices succeeds (no NaN/Inf
        // in either aspect computation) and they agree element-wise within
        // a tight tolerance (downstream consumers may rely on this).
        let braille_proj =
            Mat4::perspective_rh(braille_cfg.fov, braille_aspect, braille_cfg.near, braille_cfg.far);
        let kitty_proj =
            Mat4::perspective_rh(kitty_cfg.fov, kitty_aspect, kitty_cfg.near, kitty_cfg.far);
        for r in 0..4 {
            for c in 0..4 {
                let bp = braille_proj.col(c)[r];
                let kp = kitty_proj.col(c)[r];
                assert!(
                    (bp - kp).abs() < 1e-4,
                    "projection matrices diverge at ({r}, {c}): braille={bp}, kitty={kp}"
                );
            }
        }
    }

    /// RV4 follow-up: a unit cube at the origin projects to the SAME
    /// screen bounding-box width/height ratio under both tiers when their
    /// pixel viewports have the SAME physical aspect ratio (i.e. the same
    /// `(cells, cell_px)` product). This is the user-facing observable —
    /// boxes "look the same shape" across kitty + braille.
    #[test]
    fn rv4_unit_cube_projects_to_same_bbox_ratio_across_tiers() {
        // Same logical 80×30 cell viewport, typical 2:1 cells.
        let cells = (80u32, 30u32);
        let cell_px = (10u16, 20u16);
        let (eye, target, up) = camera();

        // Braille tier projector: (2W, 4H) dot viewport, dynamic cell_aspect.
        let braille_cfg = RenderConfig {
            cell_aspect: RenderConfig::braille_cell_aspect_for_cell(cell_px),
            ..RenderConfig::default()
        };
        let braille_vp = (cells.0 * 2, cells.1 * 4);
        let braille_p = Projector::new(eye, target, up, braille_vp, &braille_cfg);

        // Kitty tier projector: real (W*cw, H*ch) pixel viewport, cell_aspect=1.0.
        let kitty_cfg = RenderConfig { cell_aspect: 1.0, ..RenderConfig::default() };
        let kitty_vp = (cells.0 * cell_px.0 as u32, cells.1 * cell_px.1 as u32);
        let kitty_p = Projector::new(eye, target, up, kitty_vp, &kitty_cfg);

        let corners = unit_cube_corners(Vec3::ZERO);
        let (bw, bh) = projected_bbox(&braille_p, &corners);
        let (kw, kh) = projected_bbox(&kitty_p, &corners);

        // The bounding-box ratio is the user-facing "shape" of the box.
        // RV4 requires the SAME ratio on both tiers.
        let br = bw / bh;
        let kr = kw / kh;
        let rel = (br - kr).abs() / kr;
        assert!(
            rel < 1e-3,
            "RV4 visual parity: unit cube bbox ratio must match across tiers. \
             braille w/h = {br}, kitty w/h = {kr}, rel_err = {rel}"
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
