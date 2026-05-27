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
/// overrides `RenderConfig::fov` when building [`ViewParams`].
pub const DEFAULT_FOV: f32 = std::f32::consts::FRAC_PI_3;

/// Autopilot yaw rate in radians/sec. ~30°/s — a calm but unmistakable sweep (one
/// full revolution every ~12s), still smooth and NOT frantic (PITFALLS #13). Bumped
/// 1.5x from the original ~20°/s after the human re-verify wanted slightly faster
/// rotation; the orbit visits the same camera positions (same pitch range, same
/// radius), so the frustum-safe guarantee is unchanged.
const YAW_RATE: f32 = 0.525;

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
    /// projector built from the camera's ViewParams and assert NONE clip to None,
    /// across the FULL yaw turn crossed with the FULL widened pitch range —
    /// including the worst-case extremes `bias ± amplitude`. The autopilot's bob
    /// period is long relative to one yaw revolution, so stepping `step()` alone
    /// would NOT visit the pitch extremes during a single turn; we therefore drive
    /// yaw and pitch independently here so the test actually covers the worst case.
    /// We also assert a comfortable NDC margin (not just `is_some()`), proving the
    /// cube clears the frustum edge rather than grazing it. If this ever fails, a
    /// face would be dropped mid-orbit and the human would see it pop.
    #[test]
    fn orbit_keeps_all_vertices_in_frustum() {
        use std::f32::consts::TAU;

        let cube = unit_cube();
        let cfg = RenderConfig::default();
        // A representative braille viewport (non-square, like a real terminal).
        let viewport = (160u32, 120u32);
        let proj_cfg = RenderConfig { fov: DEFAULT_FOV, ..cfg };

        // The exact pitch extremes the autopilot can reach (bias ± amplitude).
        let pitch_lo = PITCH_BIAS - PITCH_AMPLITUDE;
        let pitch_hi = PITCH_BIAS + PITCH_AMPLITUDE;
        assert!(
            pitch_hi < PITCH_LIMIT && pitch_lo > -PITCH_LIMIT,
            "sweep must stay inside the gimbal clamp: lo={pitch_lo}, hi={pitch_hi}"
        );

        // Sweep yaw across a full turn × pitch across its full range, hitting the
        // extremes. NDC margin: every vertex must sit within ±MARGIN of the cube
        // edges, i.e. its |ndc| stays under (1 - MARGIN) on every axis.
        const MARGIN: f32 = 0.12;
        let yaw_steps = 72;
        let pitch_steps = 24;
        for yi in 0..yaw_steps {
            let yaw = TAU * yi as f32 / yaw_steps as f32;
            for pi in 0..=pitch_steps {
                let pitch = pitch_lo + (pitch_hi - pitch_lo) * pi as f32 / pitch_steps as f32;
                let cam = Camera {
                    yaw,
                    pitch,
                    radius: DEFAULT_RADIUS,
                    target: Vec3::ZERO,
                    elapsed: 0.0,
                };
                let vp = cam.view_params(DEFAULT_FOV);
                let proj = Projector::new(vp.eye, vp.target, vp.up, viewport, &proj_cfg);
                for (i, &v) in cube.vertices.iter().enumerate() {
                    let p = proj.project(v);
                    assert!(
                        p.is_some(),
                        "vertex {i} ({v:?}) clipped at yaw={yaw}, pitch={pitch}",
                    );
                    // The projector returns (screen_x, screen_y, ndc_z). Re-derive
                    // the on-screen position relative to the viewport center to
                    // confirm a real margin, not just an in-frustum boolean.
                    let (sx, sy, _z) = p.unwrap();
                    let nx = (sx / viewport.0 as f32) * 2.0 - 1.0;
                    let ny = (sy / viewport.1 as f32) * 2.0 - 1.0;
                    assert!(
                        nx.abs() <= 1.0 - MARGIN && ny.abs() <= 1.0 - MARGIN,
                        "vertex {i} too close to frustum edge at yaw={yaw}, \
                         pitch={pitch}: nx={nx}, ny={ny}",
                    );
                }
            }
        }
    }
}
