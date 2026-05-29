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
    /// pulls the camera closer, positive pushes it farther. Clamped to
    /// `[MIN_RADIUS, MAX_RADIUS]`.
    pub fn nudge_zoom(&mut self, delta: f32) {
        self.radius = (self.radius + delta).clamp(MIN_RADIUS, MAX_RADIUS);
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

    /// Radius must stay within `[MIN_RADIUS, MAX_RADIUS]` so the user can't
    /// fly inside the rack or to infinity.
    #[test]
    fn nudge_zoom_clamps_to_radius_range() {
        let mut cam = Camera::new();
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
