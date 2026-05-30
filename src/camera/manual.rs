//! Manual camera input — spherical-delta updates with clamps (CAM-02 / 04-03).
//!
//! Each handler nudges ONE axis of the spherical `(yaw, pitch, radius)` state
//! by a fixed step. Yaw wraps modulo `TAU`; pitch hard-clamps to
//! `[-PITCH_LIMIT, +PITCH_LIMIT]` (no gimbal flip); radius clamps to
//! `[MIN_RADIUS, MAX_RADIUS]` (can't fly inside the rack or to infinity).
//!
//! See RESEARCH "Manual Camera Input" + Pitfalls D (layout-independent keys)
//! and E (Press AND Repeat treated identically). Step sizes match RESEARCH's
//! "Step sizes" table — comfortable hold-to-rotate at typical key-repeat rate,
//! one full turn in ~63 presses for yaw.

use crate::camera::{Camera, DEFAULT_RADIUS, PITCH_LIMIT};

/// Yaw nudge per key press/repeat (radians, ~5.7°). One full turn in ~63
/// presses; smooth glide when the key is held (KeyEventKind::Repeat path).
pub const YAW_STEP: f32 = 0.10;

/// Pitch nudge per key press/repeat (radians, ~4°). Slightly slower than yaw
/// because the available pitch range (`±PITCH_LIMIT`) is narrower than yaw's
/// full `TAU`.
pub const PITCH_STEP: f32 = 0.07;

/// Zoom nudge per key press/repeat (world units). At the default scene radius
/// of ~6, ~10 presses move end-to-end without feeling chunky.
pub const ZOOM_STEP: f32 = 0.4;

/// Closest the camera can fly toward the scene center. Picked just outside
/// the typical rack's bounding sphere so the user can't accidentally end up
/// inside a box (which would look like a solid wall of color and confuse
/// the input).
pub const MIN_RADIUS: f32 = 1.2;

/// Farthest the camera can pull away. 4× the default framing radius gives a
/// generous overview without letting the user disappear into the fog plane.
pub const MAX_RADIUS: f32 = DEFAULT_RADIUS * 4.0;

impl Camera {
    /// Nudge yaw by `delta` radians (sign chooses direction). Wraps via
    /// `rem_euclid(TAU)` so yaw stays bounded across any number of nudges.
    pub fn nudge_yaw(&mut self, delta: f32) {
        self.yaw = (self.yaw + delta).rem_euclid(std::f32::consts::TAU);
    }

    /// Nudge pitch by `delta` radians (positive = look up). Hard-clamped to
    /// `[-PITCH_LIMIT, +PITCH_LIMIT]` — the gimbal-safe range the autopilot
    /// already uses.
    pub fn nudge_pitch(&mut self, delta: f32) {
        self.pitch = (self.pitch + delta).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    /// Nudge radius (distance from target) by `delta` world units. Negative
    /// pulls the camera closer, positive pushes it farther.
    ///
    /// **Per-camera clamp (RV3 fix).** Reads `self.radius_min` /
    /// `self.radius_max` instead of the static [`MIN_RADIUS`] / [`MAX_RADIUS`]
    /// constants so [`Camera::frame_scene`] can lift the upper bound to
    /// match a scene whose framed radius exceeds the static ceiling. Without
    /// this, a scene framed at radius > MAX_RADIUS gets its FIRST zoom-in
    /// click snapped down to MAX_RADIUS, and zoom-out can never recover the
    /// original framing distance — the asymmetry the user reported as
    /// "zoom-in then zoom-out doesn't return to the same place".
    ///
    /// The per-camera bounds default to MIN_RADIUS / MAX_RADIUS at
    /// construction; `frame_scene` lifts `radius_max` upward only (never
    /// shrinks it), so a subsequent re-frame on a smaller scene doesn't
    /// narrow the user's zoom window.
    pub fn nudge_zoom(&mut self, delta: f32) {
        self.radius = (self.radius + delta).clamp(self.radius_min, self.radius_max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    /// Yaw must wrap around `TAU` so prolonged orbiting can't overflow into
    /// non-finite territory.
    #[test]
    fn nudge_yaw_wraps_modulo_tau() {
        let mut cam = Camera::new();
        cam.yaw = TAU - 0.05;
        cam.nudge_yaw(0.1);
        // After wrap, yaw should be near +0.05 (a small positive value).
        assert!(
            cam.yaw < 0.1,
            "yaw should wrap into [0, 0.1), got {}",
            cam.yaw
        );
        assert!(cam.yaw >= 0.0, "yaw must stay non-negative, got {}", cam.yaw);
    }

    /// Pitch must never exceed `+PITCH_LIMIT`, no matter how hard the caller
    /// nudges it.
    #[test]
    fn nudge_pitch_clamps_to_limit() {
        let mut cam = Camera::new();
        cam.pitch = 0.0;
        cam.nudge_pitch(100.0);
        assert!(
            (cam.pitch - PITCH_LIMIT).abs() < 1e-6,
            "pitch should clamp to +PITCH_LIMIT, got {}",
            cam.pitch
        );
        cam.nudge_pitch(-100.0);
        assert!(
            (cam.pitch + PITCH_LIMIT).abs() < 1e-6,
            "pitch should clamp to -PITCH_LIMIT, got {}",
            cam.pitch
        );
    }

    /// Radius must stay within `[radius_min, radius_max]` so the user can't
    /// fly inside the rack or to infinity. Defaults match the static
    /// `MIN_RADIUS` / `MAX_RADIUS` constants when no `frame_scene` lift has
    /// happened yet.
    #[test]
    fn nudge_zoom_clamps_to_radius_range() {
        let mut cam = Camera::new();
        // Confirm the per-camera defaults match the static constants.
        assert!((cam.radius_min - MIN_RADIUS).abs() < 1e-6);
        assert!((cam.radius_max - MAX_RADIUS).abs() < 1e-6);
        cam.nudge_zoom(-1000.0);
        assert!(
            (cam.radius - MIN_RADIUS).abs() < 1e-6,
            "radius should clamp to MIN_RADIUS, got {}",
            cam.radius
        );
        cam.nudge_zoom(10_000.0);
        assert!(
            (cam.radius - MAX_RADIUS).abs() < 1e-6,
            "radius should clamp to MAX_RADIUS, got {}",
            cam.radius
        );
    }

    // ---- 05-04-RV3: zoom symmetry — frame_scene lifts radius_max ----------

    /// **THE LOAD-BEARING REGRESSION PIN for RV3.** Walk N zoom-in steps then
    /// N zoom-out steps from a `frame_scene`-framed starting radius (the
    /// synthetic 30-box rack frames at ~36, well above the static MAX=24).
    /// After the round-trip the radius MUST return to within one ZOOM_STEP of
    /// the original framing distance.
    ///
    /// Pre-RV3: the first -ZOOM_STEP click snapped from 36→24 (the static
    /// MAX_RADIUS ceiling), and zoom-out clamped at 24 forever — round-trip
    /// would end at 24, not 36, breaking symmetry. With the per-camera lift,
    /// `radius_max = max(24, 36*1.25) = 45`, so the round-trip stays
    /// symmetric across the full [1.2, 36*1.25] range.
    #[test]
    fn zoom_round_trip_returns_to_framed_radius() {
        let world = crate::world::scene::synthetic_scene();
        let mut cam = Camera::new();
        cam.frame_scene(&world);
        let framed = cam.radius;
        assert!(
            framed > MAX_RADIUS,
            "test precondition: synthetic scene must frame at radius > MAX_RADIUS \
             so we exercise the lift (got framed={framed}, MAX_RADIUS={MAX_RADIUS})"
        );

        // 10 zoom-in clicks then 10 zoom-out clicks — should net to zero
        // because nudge_zoom is additive and the lifted ceiling is high
        // enough to absorb the round-trip without clamping.
        for _ in 0..10 {
            cam.nudge_zoom(-ZOOM_STEP);
        }
        for _ in 0..10 {
            cam.nudge_zoom(ZOOM_STEP);
        }
        assert!(
            (cam.radius - framed).abs() < 1e-4,
            "10-in/10-out round-trip should return to framed radius {framed}, got {} (delta {})",
            cam.radius,
            cam.radius - framed
        );
    }

    /// After `frame_scene` on a large scene, the per-camera `radius_max`
    /// MUST be lifted to at least 125% of the framed radius. This is the
    /// invariant `nudge_zoom` relies on to deliver the round-trip property
    /// pinned above.
    #[test]
    fn frame_scene_lifts_radius_max_above_framed_distance() {
        let world = crate::world::scene::synthetic_scene();
        let mut cam = Camera::new();
        cam.frame_scene(&world);
        let framed = cam.radius;
        assert!(
            cam.radius_max >= framed * 1.25 - 1e-4,
            "radius_max ({}) must be lifted to >= framed*1.25 ({}); RV3 invariant",
            cam.radius_max,
            framed * 1.25
        );
        // ALSO must be at least the static MAX_RADIUS — never below the
        // small-scene safe ceiling.
        assert!(
            cam.radius_max >= MAX_RADIUS,
            "radius_max ({}) must never drop below the static MAX_RADIUS ({MAX_RADIUS})",
            cam.radius_max
        );
    }

    /// User can now zoom OUT past the static MAX_RADIUS after framing a
    /// large scene — the original failure mode. This is the user-facing
    /// observable: "I can zoom back to the original distance and then some
    /// for an overview view".
    #[test]
    fn zoom_out_reaches_lifted_ceiling_on_large_scene() {
        let world = crate::world::scene::synthetic_scene();
        let mut cam = Camera::new();
        cam.frame_scene(&world);
        let lifted_max = cam.radius_max;
        assert!(
            lifted_max > MAX_RADIUS,
            "test precondition: synthetic scene must lift radius_max above static MAX"
        );
        // Pull the user all the way in then nudge-out 10000 steps — clamp
        // must land at lifted_max, NOT static MAX_RADIUS.
        cam.radius = MIN_RADIUS;
        for _ in 0..10_000 {
            cam.nudge_zoom(ZOOM_STEP);
        }
        assert!(
            (cam.radius - lifted_max).abs() < 1e-4,
            "after a full pull-out, radius should clamp at lifted_max ({lifted_max}), got {}",
            cam.radius
        );
    }

    /// Verify-frame helper: write a 10-step zoom-walk trace (radius before
    /// each step) to /tmp/v504-rv3-zoom-walk.txt so the human-verify
    /// checkpoint has a numeric artifact proving symmetry. Run this test
    /// with `--ignored` (off by default to keep the test suite hermetic).
    #[test]
    #[ignore = "writes a file under /tmp; run on demand via --ignored for human verify"]
    fn rv3_writes_zoom_walk_trace() {
        use std::io::Write;
        let world = crate::world::scene::synthetic_scene();
        let mut cam = Camera::new();
        cam.frame_scene(&world);
        let framed = cam.radius;
        let max = cam.radius_max;
        let min = cam.radius_min;
        let mut f = std::fs::File::create("/tmp/v504-rv3-zoom-walk.txt").unwrap();
        writeln!(
            f,
            "RV3 zoom symmetry verify\n\
             ============================\n\
             initial framed radius (braille cell_aspect=2.0): {framed:.6}\n\
             radius_max (lifted)                            : {max:.6}\n\
             radius_min                                     : {min:.6}\n\
             static MAX_RADIUS                              : {MAX_RADIUS}\n\
             static MIN_RADIUS                              : {MIN_RADIUS}\n\
             ZOOM_STEP                                      : {ZOOM_STEP}\n",
        )
        .unwrap();
        writeln!(f, "step  direction  radius_before    radius_after").unwrap();
        for i in 0..10 {
            let before = cam.radius;
            cam.nudge_zoom(-ZOOM_STEP);
            writeln!(f, "{:>4}   IN         {:>12.6}    {:>12.6}", i, before, cam.radius).unwrap();
        }
        for i in 0..10 {
            let before = cam.radius;
            cam.nudge_zoom(ZOOM_STEP);
            writeln!(f, "{:>4}   OUT        {:>12.6}    {:>12.6}", i, before, cam.radius).unwrap();
        }
        writeln!(
            f,
            "\nfinal radius after 10-in/10-out: {:.6}\n\
             delta from initial framing      : {:.6e}\n\
             PASS (within 1e-4): {}",
            cam.radius,
            cam.radius - framed,
            (cam.radius - framed).abs() < 1e-4
        )
        .unwrap();
    }

    /// `frame_scene` lifts the ceiling UP-ONLY: re-framing a smaller scene
    /// later must NOT shrink the user's zoom window. Once the user has been
    /// allowed to zoom out to X, they keep that range even if a container
    /// disappears and the rack shrinks.
    #[test]
    fn frame_scene_never_shrinks_radius_max() {
        let world = crate::world::scene::synthetic_scene();
        let mut cam = Camera::new();
        cam.frame_scene(&world);
        let after_big = cam.radius_max;

        // Now frame a tiny scene (single entity). The static fallback for
        // empty entities would set radius = DEFAULT_RADIUS and the lift
        // would compute DEFAULT_RADIUS*1.25 = 7.5 — well below after_big.
        // The ceiling must stay at after_big.
        use crate::world::scene::SceneBounds;
        use crate::world::{Entity, World};
        use crate::theme::Status;
        let tiny = World {
            entities: vec![Entity {
                id: 1,
                position: glam::Vec3::ZERO,
                half_extents: glam::Vec3::splat(0.1),
                status: Status::Running,
                group: 0,
            }],
            bounds: SceneBounds::from_entities(&[]),
        };
        cam.frame_scene(&tiny);
        assert!(
            cam.radius_max >= after_big - 1e-4,
            "re-framing a smaller scene must NOT shrink radius_max ({}) below the previous lift ({after_big})",
            cam.radius_max
        );
    }

    /// `on_user_input` is one-way: it flips autopilot off and there's no
    /// auto-revert (RESEARCH Open Question #1).
    #[test]
    fn on_user_input_flips_autopilot_off() {
        let mut cam = Camera::new();
        assert!(cam.autopilot_active, "fresh camera must start in autopilot");
        cam.on_user_input();
        assert!(
            !cam.autopilot_active,
            "on_user_input must flip autopilot off"
        );
        // Idempotent: a second call doesn't flip it back on.
        cam.on_user_input();
        assert!(!cam.autopilot_active);
    }

    /// `step` must be a no-op when autopilot is inactive — so a future
    /// re-enable of `YAW_RATE`/`PITCH_AMPLITUDE` can't leak into manual mode
    /// and fight the user's orbit.
    #[test]
    fn step_no_op_when_autopilot_inactive() {
        let mut cam = Camera::new();
        cam.on_user_input();
        let yaw_before = cam.yaw;
        let pitch_before = cam.pitch;
        let radius_before = cam.radius;
        // Even a massive dt must not change state when autopilot is off.
        cam.step(100.0);
        assert_eq!(cam.yaw, yaw_before, "yaw drifted under no-autopilot step");
        assert_eq!(
            cam.pitch, pitch_before,
            "pitch drifted under no-autopilot step"
        );
        assert_eq!(
            cam.radius, radius_before,
            "radius drifted under no-autopilot step"
        );
    }
}
