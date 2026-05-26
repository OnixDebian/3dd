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
//! The unit cube's bounding sphere has radius ≈ `sqrt(3)/2 ≈ 0.866`. At
//! [`DEFAULT_RADIUS`] = 4.0 and [`DEFAULT_FOV`] = 60° the cube subtends a small
//! fraction of the view: the half-angle to the bounding sphere is
//! `asin(0.866 / 4.0) ≈ 12.5°`, well inside the 30° vertical half-FOV (and the
//! horizontal half-FOV is wider still after aspect correction). That ~17.5°
//! vertical margin means all 8 vertices stay comfortably in-frustum across the
//! ENTIRE yaw/pitch orbit, so no face is ever dropped mid-rotation. The
//! `orbit_keeps_all_vertices_in_frustum` test pins this across a full turn.

use glam::Vec3;

use crate::render3d::ViewParams;

/// Orbit radius: distance from the camera to the target. Chosen with the cube's
/// bounding sphere (≈0.866) so the whole cube stays well inside the frustum for
/// every orbit angle (see module docs).
pub const DEFAULT_RADIUS: f32 = 4.0;

/// Vertical field of view in radians (60°). The camera owns the lens; this
/// overrides `RenderConfig::fov` when building [`ViewParams`].
pub const DEFAULT_FOV: f32 = std::f32::consts::FRAC_PI_3;

/// Autopilot yaw rate in radians/sec. ~20°/s — a slow, calm sweep (one full
/// revolution every ~18s), NOT frantic (PITFALLS #13: slow, smooth motion).
const YAW_RATE: f32 = 0.35;

/// Autopilot pitch bob: the camera eases up and down by [`PITCH_AMPLITUDE`] at
/// [`PITCH_RATE`] rad/s, giving a gentle parallax without ever flipping over.
const PITCH_RATE: f32 = 0.17;
/// Peak pitch (radians) of the vertical bob. ~17° — stays far from the ±90°
/// gimbal poles so `up` never degenerates.
const PITCH_AMPLITUDE: f32 = std::f32::consts::FRAC_PI_8 * 0.8;

/// Hard clamp on pitch so the camera can never reach the poles (gimbal flip).
const PITCH_LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.15;

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
            yaw: 0.0,
            pitch: PITCH_AMPLITUDE,
            radius: DEFAULT_RADIUS,
            target: Vec3::ZERO,
            elapsed: 0.0,
        }
    }

    /// Advance the autopilot by real elapsed time `dt` (seconds).
    ///
    /// Yaw advances at a constant slow rate (wrapped to keep it bounded); pitch
    /// eases through a gentle sine bob and is hard-clamped so it can never reach
    /// the gimbal poles. Using real `dt` keeps the motion framerate-independent
    /// (Gaffer decoupling).
    pub fn step(&mut self, dt: f32) {
        // Ignore non-finite / negative dt defensively (e.g. a clock hiccup).
        let dt = if dt.is_finite() && dt > 0.0 { dt } else { 0.0 };

        self.elapsed += dt;
        // Continuous slow azimuth sweep, wrapped to stay in [0, TAU).
        self.yaw = (self.yaw + YAW_RATE * dt).rem_euclid(std::f32::consts::TAU);
        // Eased vertical bob (sine), then hard-clamped away from the poles.
        self.pitch = (PITCH_AMPLITUDE * (self.elapsed * PITCH_RATE).sin())
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

    /// `step` advances yaw at the expected slow rate using real dt.
    #[test]
    fn yaw_advances_with_dt() {
        let mut cam = Camera::new();
        let start = cam.yaw;
        cam.step(1.0);
        // After 1s, yaw advanced by ~YAW_RATE radians (modulo wrap, which can't
        // fire here since YAW_RATE < TAU).
        let advanced = (cam.yaw - start).rem_euclid(std::f32::consts::TAU);
        assert!((advanced - YAW_RATE).abs() < 1e-4, "yaw rate wrong: {advanced}");
    }

    /// FRUSTUM-SAFE DEFAULTS pin: project all 8 unit-cube vertices through a
    /// projector built from the camera's ViewParams at many yaw/pitch angles
    /// across a FULL turn and assert NONE clip to None. If this ever fails, a
    /// face would be dropped mid-orbit and the human would see a flicker.
    #[test]
    fn orbit_keeps_all_vertices_in_frustum() {
        let cube = unit_cube();
        let cfg = RenderConfig::default();
        // A representative braille viewport (non-square, like a real terminal).
        let viewport = (160u32, 120u32);

        let mut cam = Camera::new();
        // Walk a full revolution in small steps; the eased pitch sweeps too.
        let steps = 360;
        for _ in 0..steps {
            cam.step(std::f32::consts::TAU / (YAW_RATE * steps as f32));
            let vp = cam.view_params(DEFAULT_FOV);
            // Camera owns the lens: fold its fov into the projector config.
            let proj_cfg = RenderConfig { fov: vp.fov, ..cfg };
            let proj = Projector::new(vp.eye, vp.target, vp.up, viewport, &proj_cfg);
            for (i, &v) in cube.vertices.iter().enumerate() {
                assert!(
                    proj.project(v).is_some(),
                    "vertex {i} ({v:?}) clipped at yaw={}, pitch={}",
                    cam.yaw,
                    cam.pitch
                );
            }
        }
    }
}
