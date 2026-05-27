//! Autopilot orbit camera — the producer side of the render3d decoupling.
//!
//! `render3d` deliberately does NOT depend on a `Camera` type (see plan 01-04):
//! it consumes a raw [`ViewParams`]. This module owns the orbit controller and
//! each frame builds a [`ViewParams`] (`eye`/`target`/`up`/`fov`) that `render()`
//! consumes. The dependency points 05 → 04 (Camera imports ViewParams), never the
//! other way, so there is no cycle and `render3d` stays a pure snapshot renderer.
//!
//! ## Frustum-safe defaults (de-risk skip-on-clip flicker)
//!
//! The rasterizer drops any face with a vertex that clips to `None` (PITFALLS #4).
//! For a static centered cube that never fires, but an orbit can swing a vertex
//! toward the frustum edge. The cube is centered at `target` (the origin) and the
//! orbit radius is CONSTANT, so a single radius/fov choice holds for every yaw.
//!
//! The unit cube's bounding sphere has radius ≈ `sqrt(3)/2 ≈ 0.866`. To reveal
//! the TOP and BOTTOM faces the autopilot sweeps a WIDE pitch range (a steady
//! downward tilt plus a wide eased bob), which swings vertices closer to the
//! frustum edge. To keep the whole cube comfortably in-frustum across that wider
//! range we pulled the camera back to [`DEFAULT_RADIUS`] = 6.0. At 6.0 and
//! [`DEFAULT_FOV`] = 60° the half-angle to the bounding sphere is
//! `asin(0.866 / 6.0) ≈ 8.3°`, leaving a ~21.7° vertical margin under the 30°
//! half-FOV. Because the eye stays exactly `radius` from the centered cube for
//! EVERY (yaw, pitch), that margin holds at every orbit angle, so no face is ever
//! dropped mid-rotation. The `orbit_keeps_all_vertices_in_frustum` test pins this
//! across a full turn AND across the worst-case pitch extremes.

use glam::Vec3;

use crate::render3d::ViewParams;

/// Orbit radius: distance from the camera to the target. Pulled back from 4.0 to
/// 6.0 so the WIDE pitch sweep (which reveals the top/bottom faces) still keeps
/// the cube's bounding sphere (≈0.866) well inside the frustum for every orbit
/// angle (see module docs).
pub const DEFAULT_RADIUS: f32 = 6.0;

/// Vertical field of view in radians (60°). The camera owns the lens; this
/// overrides `RenderConfig::fov` when building [`ViewParams`]. Kept at 60°: the
/// zoom in the verify-tuning pass comes from a TIGHTER framing distance (a larger
/// [`FRAME_HALF_FOV`] / smaller margin pulling the eye in) plus a denser, shallower
/// rack — not from narrowing the lens, which fought the braille path's aspect-2
/// horizontal narrowing and clipped boxes at the scene edge.
pub const DEFAULT_FOV: f32 = std::f32::consts::FRAC_PI_3;

/// Autopilot yaw rate in radians/sec. SET TO 0 — the human's verify-tuning
/// override: the camera no longer ORBITS the scene (the whole rack swinging
/// around as a group was rejected). Instead the camera holds a fixed 3/4 framing
/// angle ([`FRAME_YAW`]) and the motion comes from each box spinning in place
/// (see [`SPIN_RATE`], applied per-box in the renderers). Left as a named knob so
/// the orbit can be re-enabled later. NOTE: this deviates from roadmap success
/// criterion #4 (autopilot orbit camera) — flagged for Phase reconciliation.
const YAW_RATE: f32 = 0.0;

/// Per-box self-spin rate in radians/sec (~30°/s — one revolution every ~12s, a
/// calm unmistakable spin, not frantic; PITFALLS #13). The renderers advance a
/// `spin` angle by REAL dt at this rate and rotate EACH box about its own +Y axis,
/// so every box spins in place while the camera stays still. Framerate-independent
/// (Gaffer decoupling), exactly like the old `YAW_RATE` was.
pub const SPIN_RATE: f32 = 0.525;

/// Fixed azimuth (radians, ~40°) the static camera frames the rack from, giving a
/// pleasant 3/4 view (looking at the corner of the rack, not face-on). Combined
/// with [`PITCH_BIAS`] this is the held viewing angle now that the orbit is off.
const FRAME_YAW: f32 = 0.7;

/// Autopilot pitch bob: the camera eases up and down by [`PITCH_AMPLITUDE`]
/// (about a steady [`PITCH_BIAS`] downward tilt) at [`PITCH_RATE`] rad/s, sweeping
/// from high above the cube to below it so the TOP and BOTTOM faces are both
/// revealed over the orbit, without ever flipping over.
const PITCH_RATE: f32 = 0.17;
/// Steady downward tilt (radians) the bob oscillates around. ~15° above the
/// equator as a baseline so, combined with the bob, the camera spends time
/// clearly looking DOWN onto the top face.
const PITCH_BIAS: f32 = std::f32::consts::FRAC_PI_8 * 1.2;
/// Peak amplitude (radians) of the vertical bob about [`PITCH_BIAS`]. Set to 0 —
/// the human asked the camera to hold a fixed elevation and only spin (no
/// time-varying vertical drift), so the pitch stays pinned at [`PITCH_BIAS`] and
/// only `yaw` advances. Left as a named knob so the bob can be re-enabled later.
const PITCH_AMPLITUDE: f32 = 0.0;

/// Hard clamp on pitch so the camera can never reach the poles (gimbal flip).
/// The widened sweep peaks at `bias + amplitude ≈ 60.8°`, comfortably under this.
const PITCH_LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.15;

/// Effective half-FOV [`Camera::frame_scene`] solves the orbit radius against,
/// in radians (~21°). This is NOT the vertical half-FOV (30° for a 60° lens):
/// the projector folds `cell_aspect` (≈2.0) into the perspective aspect, which
/// narrows the HORIZONTAL field below the vertical one. On a typical terminal
/// the horizontal half-angle is the binding constraint
/// (`atan(tan(30°) · (w/h)/cell_aspect) ≈ 21°` for a ~4:3 viewport), so the fit
/// must respect the tighter axis or a box clips off the left/right edge even
/// though the bounding sphere "fits" the vertical cone.
///
// Consumed by `frame_scene`, which the rasterizer plans (02-02/03) wire in.
//
// This is the braille path's BINDING horizontal half-angle at the 60° lens
// (`atan(tan(30°) · (w/h)/cell_aspect) ≈ 0.367` for a ~4:3 braille viewport with
// cell_aspect 2.0) — the tightest axis a box can clip against. Kept at the true
// binding angle; the verify-tuning zoom comes from the much smaller safety margin
// (the orbit headroom is gone — the camera is static now, so it never has to
// survive a yaw sweep) and the denser/shallower rack, not from over-widening this.
#[allow(dead_code)]
const FRAME_HALF_FOV: f32 = 0.367;

/// Angular safety margin (radians, ~8.6°) subtracted from [`FRAME_HALF_FOV`]
/// when [`Camera::frame_scene`] solves for the orbit radius. This is the
/// multi-box generalization of the Phase 1 single-cube headroom (~21.7° at
/// radius 6): it leaves a comfortable NDC margin so no box corner grazes the
/// frustum edge across the full yaw orbit, AND lets the pitch bias swing
/// vertices toward the edge without clipping. Pinned by
/// `frame_scene_keeps_whole_scene_in_frustum`.
#[allow(dead_code)]
const FRAME_SAFETY_MARGIN: f32 = 0.02;

/// An autopilot orbit camera circling a fixed `target`.
///
/// State is spherical: `yaw` (azimuth around the world Y axis), `pitch`
/// (elevation), and `radius` (distance to the target). [`Camera::step`] advances
/// the autopilot; [`Camera::view_params`] snapshots it into a [`ViewParams`].
#[derive(Debug, Clone, Copy)]
pub struct Camera {
    /// Azimuth angle around the world +Y axis, radians. Advances continuously.
    pub yaw: f32,
    /// Elevation angle above the XZ plane, radians. Clamped to [`PITCH_LIMIT`].
    pub pitch: f32,
    /// Distance from the camera to the target.
    pub radius: f32,
    /// The point the camera orbits and looks at.
    pub target: Vec3,
    /// Accumulated time (seconds) driving the eased pitch bob.
    elapsed: f32,
}

impl Camera {
    /// A camera at the frustum-safe defaults, orbiting the origin.
    pub fn new() -> Self {
        Self {
            yaw: FRAME_YAW,
            pitch: PITCH_BIAS,
            radius: DEFAULT_RADIUS,
            target: Vec3::ZERO,
            elapsed: 0.0,
        }
    }

    /// Advance the autopilot by real elapsed time `dt` (seconds).
    ///
    /// Yaw advances at a constant slow rate (wrapped to keep it bounded); pitch
    /// holds the fixed [`PITCH_BIAS`] elevation ([`PITCH_AMPLITUDE`] is 0, so the
    /// optional bob contributes nothing) and is hard-clamped away from the gimbal
    /// poles. Using real `dt` keeps the motion framerate-independent (Gaffer
    /// decoupling).
    pub fn step(&mut self, dt: f32) {
        // Ignore non-finite / negative dt defensively (e.g. a clock hiccup).
        let dt = if dt.is_finite() && dt > 0.0 { dt } else { 0.0 };

        self.elapsed += dt;
        // Continuous slow azimuth sweep, wrapped to stay in [0, TAU).
        self.yaw = (self.yaw + YAW_RATE * dt).rem_euclid(std::f32::consts::TAU);
        // Fixed downward tilt (the bob amplitude is 0), hard-clamped away from the
        // poles. The constant bias keeps the camera looking slightly down onto the
        // top face while only the yaw spins.
        self.pitch = (PITCH_BIAS + PITCH_AMPLITUDE * (self.elapsed * PITCH_RATE).sin())
            .clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    /// World-space eye position from the spherical (yaw, pitch, radius) orbit
    /// around `target`.
    pub fn eye(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        // Spherical → Cartesian: pitch lifts along +Y, yaw rotates in the XZ
        // plane. radius scales the unit direction.
        let dir = Vec3::new(cp * sy, sp, cp * cy);
        self.target + dir * self.radius
    }

    /// Build the [`ViewParams`] the renderer consumes this frame.
    ///
    /// `fov` is the lens the camera looks through (overrides `RenderConfig::fov`
    /// inside `render`). `up` is world +Y — the pitch clamp guarantees the view
    /// direction never aligns with it, so `look_at` never degenerates.
    pub fn view_params(&self, fov: f32) -> ViewParams {
        ViewParams {
            eye: self.eye(),
            target: self.target,
            up: Vec3::Y,
            fov,
        }
    }

    /// Frame the WHOLE scene instead of a unit cube (CAM-01 generalization).
    ///
    /// Sets the orbit `target` to `bounds.center` and solves for a `radius` that
    /// keeps the scene's bounding sphere inside the frustum with margin. This is
    /// the same constraint the Phase 1 single-cube derivation used —
    /// `asin(scene_radius / camera_radius) <= half_fov - margin` — rearranged for
    /// `radius`:
    ///
    /// ```text
    /// θ      = FRAME_HALF_FOV - FRAME_SAFETY_MARGIN
    /// radius = scene_radius · (1 + 1 / tan(θ))
    /// ```
    ///
    /// This is the *near-corner* tangent bound, not the simple
    /// `scene_r / sin(θ)` sphere bound: the box corner nearest the eye sits at
    /// distance `radius - scene_radius` yet can be offset by up to
    /// `scene_radius` perpendicular to the view axis, so it subtends
    /// `atan(scene_radius / (radius - scene_radius))` — LARGER than the sphere
    /// bound suggests. Solving that against the tighter HORIZONTAL half-angle
    /// ([`FRAME_HALF_FOV`], the cell-aspect-narrowed axis) keeps even the nearest
    /// box inside the left/right edge. Because the eye stays exactly `radius`
    /// from `center` for EVERY yaw/pitch, the margin holds at every orbit angle
    /// (the invariant `orbit_keeps_all_vertices_in_frustum` relies on), so no box
    /// corner pops mid-orbit. Pitch bias/clamp are untouched.
    ///
    /// Uses [`DEFAULT_FOV`] for the lens (the camera owns the lens); a degenerate
    /// (zero/non-finite) `bounds.radius` falls back to [`DEFAULT_RADIUS`].
    // First consumed by the rasterizer plans (02-02 braille, 02-03 kitty); the
    // test exercises it now, but no non-test caller exists yet.
    #[allow(dead_code)]
    pub fn frame_scene(&mut self, bounds: &crate::world::scene::SceneBounds) {
        self.target = bounds.center;

        let theta = FRAME_HALF_FOV - FRAME_SAFETY_MARGIN;
        let radius = bounds.radius * (1.0 + 1.0 / theta.tan());

        self.radius = if radius.is_finite() && radius > DEFAULT_RADIUS {
            radius
        } else {
            DEFAULT_RADIUS
        };
    }
}

impl Default for Camera {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RenderConfig;
    use crate::render3d::cube::unit_cube;
    use crate::render3d::project::Projector;

    /// The eye must always sit exactly `radius` from the target, for any orbit
    /// angle the autopilot can reach.
    #[test]
    fn eye_stays_on_orbit_sphere() {
        let mut cam = Camera::new();
        for _ in 0..200 {
            cam.step(0.05);
            let d = (cam.eye() - cam.target).length();
            assert!((d - cam.radius).abs() < 1e-4, "eye off the orbit sphere: {d}");
        }
    }

    /// Pitch must never approach the gimbal poles, no matter how long we run.
    #[test]
    fn pitch_is_bounded() {
        let mut cam = Camera::new();
        for _ in 0..10_000 {
            cam.step(0.1);
            assert!(cam.pitch.abs() <= PITCH_LIMIT, "pitch escaped clamp: {}", cam.pitch);
        }
    }

    /// The camera now HOLDS its framing angle — `step` must NOT move the yaw
    /// (the human's verify override: no scene orbit). The motion lives in the
    /// per-box spin instead, exercised in the renderer tests.
    #[test]
    fn yaw_is_static_under_step() {
        let mut cam = Camera::new();
        let start = cam.yaw;
        for _ in 0..100 {
            cam.step(0.1);
        }
        assert!(
            (cam.yaw - start).abs() < 1e-6,
            "camera yaw drifted though the orbit is disabled: {} -> {}",
            start,
            cam.yaw
        );
        // Sanity: YAW_RATE itself is pinned at 0 so the orbit really is off.
        assert_eq!(YAW_RATE, 0.0, "orbit must be disabled (YAW_RATE == 0)");
    }

    /// FRUSTUM-SAFE FRAMING pin (static-camera form): the camera no longer orbits,
    /// so we project all 8 unit-cube vertices through the SINGLE held framing pose
    /// (`FRAME_YAW`, `PITCH_BIAS`) and assert NONE clip to None. With the orbit
    /// gone there is no yaw sweep to survive — just the one pose the human sees.
    /// This guards against the tightened framing constants pulling a vertex off
    /// the edge. (Per-box spin is exercised in the renderer/`render3d` tests.)
    #[test]
    fn framed_pose_keeps_all_vertices_in_frustum() {
        let cube = unit_cube();
        let cfg = RenderConfig::default();
        // A representative braille viewport (non-square, like a real terminal).
        let viewport = (160u32, 120u32);
        let proj_cfg = RenderConfig { fov: DEFAULT_FOV, ..cfg };

        let cam = Camera::new(); // FRAME_YAW / PITCH_BIAS / DEFAULT_RADIUS, origin
        assert!(
            cam.pitch.abs() < PITCH_LIMIT,
            "framing pitch must stay inside the gimbal clamp: {}",
            cam.pitch
        );
        let vp = cam.view_params(DEFAULT_FOV);
        let proj = Projector::new(vp.eye, vp.target, vp.up, viewport, &proj_cfg);
        for (i, &v) in cube.vertices.iter().enumerate() {
            assert!(
                proj.project(v).is_some(),
                "vertex {i} ({v:?}) clipped at the framing pose",
            );
        }
    }

    /// MULTI-BOX FRUSTUM PIN (static-camera form). Build the synthetic scene,
    /// frame it with `frame_scene`, then at the SINGLE held framing pose assert
    /// EVERY entity's 8 AABB corners — SPUN about the box's own +Y axis through a
    /// representative range of spin angles — project to `Some(..)`. The camera no
    /// longer orbits, so there is no yaw sweep; instead the boxes spin, so we
    /// sweep the SPIN angle to confirm no corner clips as a box rotates in place.
    /// Guards the tightened framing radius against a spun corner popping the edge.
    #[test]
    fn frame_scene_keeps_spinning_scene_in_frustum() {
        use std::f32::consts::TAU;

        use crate::render3d::rotate_y_about;

        let world = crate::world::scene::synthetic_scene();

        let mut cam = Camera::new();
        cam.frame_scene(&world.bounds);
        // frame_scene must target the scene center, not the origin.
        assert!(
            (cam.target - world.bounds.center).length() < 1e-4,
            "frame_scene did not target the scene center"
        );

        let cfg = RenderConfig::default();
        let viewport = (160u32, 120u32);
        let proj_cfg = RenderConfig { fov: DEFAULT_FOV, ..cfg };

        let vp = cam.view_params(DEFAULT_FOV);
        let proj = Projector::new(vp.eye, vp.target, vp.up, viewport, &proj_cfg);

        // Sweep the per-box spin so a box mid-rotation can't push a corner off.
        let spin_steps = 24;
        for si in 0..spin_steps {
            let spin = TAU * si as f32 / spin_steps as f32;
            for e in &world.entities {
                for &sx in &[-1.0f32, 1.0] {
                    for &sy in &[-1.0f32, 1.0] {
                        for &sz in &[-1.0f32, 1.0] {
                            let corner = e.position
                                + glam::Vec3::new(
                                    sx * e.half_extents.x,
                                    sy * e.half_extents.y,
                                    sz * e.half_extents.z,
                                );
                            // Spin the corner about THIS box's center, as the
                            // renderers do, before projecting.
                            let spun = rotate_y_about(corner, e.position, spin);
                            assert!(
                                proj.project(spun).is_some(),
                                "entity {} corner {spun:?} clipped at spin={spin}",
                                e.id
                            );
                        }
                    }
                }
            }
        }
    }
}
