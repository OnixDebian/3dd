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

/// Reference braille viewport [`Camera::frame_scene`] solves the framing distance
/// against. The live braille/kitty viewports vary with terminal size, but the
/// camera is framed ONCE (at app construction, before any viewport is known), so
/// the fit must be backend-independent. We frame against this canonical ~4:3
/// braille grid at [`DEFAULT_FOV`] with `cell_aspect` 2.0 — the BINDING case: the
/// cell-aspect-2 horizontal axis is narrower than every other backend/aspect the
/// rack is shown in, so framing tight here is automatically frustum-safe on the
/// wider kitty path (square pixels, cell_aspect 1.0) and on taller terminals.
const FRAME_REF_VIEWPORT: (u32, u32) = (160, 120);

/// Cell-aspect of the reference braille framing viewport (the braille default).
/// The projector folds this into the perspective aspect, narrowing the HORIZONTAL
/// field below the vertical one — which is exactly why the horizontal NDC extent
/// is the binding axis the [`Camera::frame_scene`] fit drives to [`FRAME_TARGET_FILL`].
const FRAME_REF_CELL_ASPECT: f32 = 2.0;

/// Target fraction of the binding (horizontal) NDC half-axis the scene's projected
/// bounding box should fill (~0.92 → the rack spans ~92% of the half-width, i.e.
/// most of the frame, leaving a thin safety gutter to the edge). This replaces the
/// old bounding-SPHERE fit: that fit the circumscribing sphere of a diagonal-ribbon
/// rack, which left the rack under-filling its own sphere (~40-50% of frame). We
/// now solve the framing distance so the rack's ACTUAL projected AABB reaches this
/// fill on the binding axis — roughly 2x larger on screen (the human's "в два раза
/// крупнее"). The remaining 8% gutter is the safety margin so no spun corner grazes
/// the edge; pinned by `frame_scene_keeps_spinning_scene_in_frustum`.
const FRAME_TARGET_FILL: f32 = 0.92;

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

    /// Frame the WHOLE scene by fitting its PROJECTED bounding box (CAM-01,
    /// verify-tuning rev).
    ///
    /// Sets the orbit `target` to `bounds.center` and solves for the framing
    /// `radius` so the rack's ACTUAL screen-space projected extent fills
    /// [`FRAME_TARGET_FILL`] of the binding NDC half-axis — NOT the old
    /// circumscribing-SPHERE fit. The rack is a diagonal ribbon that under-fills
    /// its bounding sphere, so the sphere fit left it at ~40-50% of the frame; the
    /// human asked for ~2x larger ("в два раза крупнее"). Fitting the real
    /// projected AABB instead lets the rack fill most of the frame.
    ///
    /// ## Why a binary search, and why it stays frustum-safe on BOTH backends
    ///
    /// The view direction is fixed (`FRAME_YAW`/`PITCH_BIAS`); only `radius`
    /// moves the eye along it, and the projected NDC extent shrinks monotonically
    /// as `radius` grows. So we binary-search `radius`: at each candidate we build
    /// a reference projector ([`FRAME_REF_VIEWPORT`] / [`FRAME_REF_CELL_ASPECT`] /
    /// [`DEFAULT_FOV`] — the canonical BINDING braille frustum), project EVERY
    /// entity's 8 AABB corners SWEPT through a turn of per-box Y-spin (the corners
    /// poke widest mid-rotation, exactly as the renderers spin them), and measure
    /// the max horizontal NDC half-extent. We tighten `radius` until that extent
    /// reaches [`FRAME_TARGET_FILL`]. The horizontal axis is the binding one
    /// (cell_aspect 2 narrows it below vertical), so driving X to <1 keeps Y
    /// comfortably inside too; and the kitty path (square pixels, wider horizontal
    /// field) is automatically safer than this reference braille fit. The eye
    /// stays exactly `radius` from `center`, so the framing holds for the single
    /// static pose the human sees (the orbit is disabled).
    ///
    /// Uses [`DEFAULT_FOV`] for the lens (the camera owns the lens); an empty
    /// scene or a degenerate projected extent falls back to [`DEFAULT_RADIUS`].
    pub fn frame_scene(&mut self, world: &crate::world::World) {
        use crate::config::RenderConfig;
        use crate::render3d::project::Projector;
        use crate::render3d::rotate_y_about;

        self.target = world.bounds.center;

        if world.entities.is_empty() {
            self.radius = DEFAULT_RADIUS;
            return;
        }

        let proj_cfg = RenderConfig { fov: DEFAULT_FOV, cell_aspect: FRAME_REF_CELL_ASPECT, ..RenderConfig::default() };

        // Max horizontal NDC half-extent of the whole rack at a candidate radius,
        // swept across per-box spin so a mid-rotation corner can't be missed.
        // Returns None if any corner clips (radius too small — box off-screen).
        let max_ndc_x = |radius: f32| -> Option<f32> {
            let (sy, cy) = self.yaw.sin_cos();
            let (sp, cp) = self.pitch.sin_cos();
            let dir = Vec3::new(cp * sy, sp, cp * cy);
            let eye = self.target + dir * radius;
            let proj = Projector::new(eye, self.target, Vec3::Y, FRAME_REF_VIEWPORT, &proj_cfg);

            const SPIN_STEPS: u32 = 24;
            let mut worst: f32 = 0.0;
            for si in 0..SPIN_STEPS {
                let spin = std::f32::consts::TAU * si as f32 / SPIN_STEPS as f32;
                for e in &world.entities {
                    for &sx in &[-1.0f32, 1.0] {
                        for &sgy in &[-1.0f32, 1.0] {
                            for &sz in &[-1.0f32, 1.0] {
                                let corner = e.position
                                    + Vec3::new(
                                        sx * e.half_extents.x,
                                        sgy * e.half_extents.y,
                                        sz * e.half_extents.z,
                                    );
                                let spun = rotate_y_about(corner, e.position, spin);
                                match proj.ndc(spun) {
                                    Some(ndc) => worst = worst.max(ndc.x.abs()),
                                    None => return None, // clips — radius too tight
                                }
                            }
                        }
                    }
                }
            }
            Some(worst)
        };

        // Binary-search radius in [lo, hi]: lo too tight (clips or over-fills),
        // hi safely loose (extent < target). Invariant: at hi the rack fits.
        let mut hi = DEFAULT_RADIUS.max(4.0 * world.bounds.radius + 4.0);
        // Grow hi until the rack provably fits (extent below target) — defensive.
        for _ in 0..40 {
            match max_ndc_x(hi) {
                Some(x) if x <= FRAME_TARGET_FILL => break,
                _ => hi *= 1.5,
            }
        }
        let mut lo = 0.0_f32;
        // 40 iterations → sub-millimetre precision on the radius.
        for _ in 0..40 {
            let mid = 0.5 * (lo + hi);
            match max_ndc_x(mid) {
                // Fits and still under target → can pull closer (smaller radius).
                Some(x) if x <= FRAME_TARGET_FILL => hi = mid,
                // Over target or clipping → must back off (larger radius).
                _ => lo = mid,
            }
        }

        self.radius = if hi.is_finite() && hi > 0.0 { hi } else { DEFAULT_RADIUS };
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
        cam.frame_scene(&world);
        // frame_scene must target the scene center, not the origin.
        assert!(
            (cam.target - world.bounds.center).length() < 1e-4,
            "frame_scene did not target the scene center"
        );

        // Project against the SAME binding reference frustum frame_scene fits to
        // (the cell_aspect-2 braille viewport — the tightest axis). If no corner
        // clips here, none clips on the wider kitty path / taller terminals.
        let proj_cfg =
            RenderConfig { fov: DEFAULT_FOV, cell_aspect: FRAME_REF_CELL_ASPECT, ..RenderConfig::default() };

        let vp = cam.view_params(DEFAULT_FOV);
        let proj = Projector::new(vp.eye, vp.target, vp.up, FRAME_REF_VIEWPORT, &proj_cfg);

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

    /// PROJECTED-AABB FILL pin: `frame_scene` must pull the eye in close enough
    /// that the rack's projected bounding box FILLS the frame — the binding
    /// (horizontal) NDC half-extent must reach `FRAME_TARGET_FILL` (within a small
    /// tolerance) WITHOUT exceeding 1.0 on any axis at any spin. This guards the
    /// "~2x larger" tuning: it would fail (extent far below target) if the fit
    /// regressed to the old under-filling bounding-sphere model.
    #[test]
    fn frame_scene_fills_frame_on_binding_axis() {
        use std::f32::consts::TAU;

        use crate::render3d::rotate_y_about;

        let world = crate::world::scene::synthetic_scene();
        let mut cam = Camera::new();
        cam.frame_scene(&world);

        let proj_cfg =
            RenderConfig { fov: DEFAULT_FOV, cell_aspect: FRAME_REF_CELL_ASPECT, ..RenderConfig::default() };
        let vp = cam.view_params(DEFAULT_FOV);
        let proj = Projector::new(vp.eye, vp.target, vp.up, FRAME_REF_VIEWPORT, &proj_cfg);

        let mut max_x: f32 = 0.0;
        let mut max_y: f32 = 0.0;
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
                            let spun = rotate_y_about(corner, e.position, spin);
                            let ndc = proj.ndc(spun).expect("corner behind near plane");
                            max_x = max_x.max(ndc.x.abs());
                            max_y = max_y.max(ndc.y.abs());
                        }
                    }
                }
            }
        }

        // Binding axis reaches the target fill (the rack is "zoomed in"), and
        // nothing clips on EITHER axis.
        assert!(
            (max_x - FRAME_TARGET_FILL).abs() < 0.02,
            "binding-axis fill {max_x} should be ~{FRAME_TARGET_FILL} (rack not framed ~2x larger)"
        );
        assert!(max_x <= 1.0, "binding axis clips: max |ndc.x| = {max_x}");
        assert!(max_y <= 1.0, "vertical axis clips: max |ndc.y| = {max_y}");
    }
}
