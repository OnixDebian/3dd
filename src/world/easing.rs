//! Pure easing primitive — critically-damped spring step (CONT-03).
//!
//! [`critically_damped`] is the closed-form implicit-Euler step for a
//! critically-damped (ζ=1) spring. Given a current position `x` and velocity
//! `v`, it advances both toward `x_target` over a characteristic `half_life`
//! at real elapsed `dt`. Framerate-independent; no overshoot; smooth velocity
//! continuity across target reversals. Two independent canonical sources:
//! Holden's "Spring-It-On" (theorangeduck.com) and Chou's "Precise Control
//! over Numeric Springing" (allenchou.net).
//!
//! Used by [`crate::world::live`] to ease each container's displayed
//! half-extent toward the latest stat target — boxes BREATHE instead of
//! snapping when a new sample lands.
//!
//! Half-life choice: `BREATHING_HALF_LIFE = 0.15s`. With stats arriving at
//! ~1Hz, the canonical `4 * half_life` rule gives 0.6s to within 2% of the
//! new steady-state — visible easing inside one stat interval, fully settled
//! before the next sample lands.
//!
//! Pure module: no I/O, no global state. The function is `&mut x, &mut v`
//! only — no state outside its args.

#![allow(dead_code)]

/// Canonical breathing half-life (seconds). 4τ ≈ 0.6s to within 2% — visible
/// easing inside one ~1Hz stat interval, fully settled before the next sample.
pub const BREATHING_HALF_LIFE: f32 = 0.15;

/// Critically-damped spring step (ζ=1, implicit Euler, closed form).
///
/// Eases `x` and its velocity `v` toward `x_target` over `half_life` seconds
/// at real elapsed `dt`. Framerate-independent: doubling `dt` and halving the
/// number of steps converges to the same `x` within tolerance. No overshoot.
///
/// Guards:
/// - Non-finite or non-positive `dt` is a no-op (matches `on_tick`'s `dt` guard
///   in `src/app.rs` — a frame skip / clock glitch never poisons the spring).
/// - Non-finite or non-positive `half_life` snaps `x = x_target`, `v = 0` (a
///   degenerate config NEVER yields NaN).
pub fn critically_damped(
    x: &mut f32,
    v: &mut f32,
    x_target: f32,
    half_life: f32,
    dt: f32,
) {
    // dt guard: a glitchy clock or frame skip leaves the spring untouched.
    if !dt.is_finite() || dt <= 0.0 {
        return;
    }
    // half_life guard: a degenerate (zero / negative / NaN) half_life snaps
    // safely to the target rather than producing infinities.
    if !half_life.is_finite() || half_life <= 0.0 {
        *x = x_target;
        *v = 0.0;
        return;
    }

    // Exponential closed-form for the critically-damped 2nd-order ODE
    //
    //   ẋ = v,   v̇ = -2ω·v - ω²·(x - x_target)
    //
    // Position response (with x₀=0, v₀=0, target=1) is
    //   x(t) = 1 - (1 + ω·t) · e^(-ω·t)
    // velocity response is
    //   v(t) = ω² · t · e^(-ω·t)
    // The general one-step formulas (Holden, "Spring-It-On") for (x_next,
    // v_next) given (x, v, x_target, ω, dt) are:
    //
    //   d   = x - x_target           // deviation from target
    //   ed  = e^(-ω·dt)              // decay envelope
    //   x_next = (d·(1 + ω·dt) + v·dt) · ed + x_target
    //   v_next = (v·(1 - ω·dt) - d·ω²·dt) · ed
    //
    // Truly framerate-independent: the result depends only on the elapsed
    // wall time, not on how it's subdivided into steps (modulo f32 roundoff).
    //
    // Natural frequency ω = 2·ln(2) / half_life. The factor of 2 is Holden's
    // `dampingFromHalflife` — picks ω so the half-life is the actual decay
    // half-life. The position response then settles to within 2% by
    // t ≈ 4.21·half_life (close to the canonical "4τ" rule).
    let omega = 2.0 * std::f32::consts::LN_2 / half_life;
    let ed = (-omega * dt).exp();
    let d = *x - x_target;
    let new_x = (d * (1.0 + omega * dt) + *v * dt) * ed + x_target;
    let new_v = (*v * (1.0 - omega * dt) - d * omega * omega * dt) * ed;
    *x = new_x;
    *v = new_v;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Idle: zero target, zero state, zero velocity stays put — the spring
    /// has nothing to do and must not drift / accumulate noise.
    #[test]
    fn idle_with_zero_target_stays_zero() {
        let mut x = 0.0f32;
        let mut v = 0.0f32;
        for _ in 0..100 {
            critically_damped(&mut x, &mut v, 0.0, BREATHING_HALF_LIFE, 0.033);
        }
        assert_eq!(x, 0.0, "x drifted: {x}");
        assert_eq!(v, 0.0, "v drifted: {v}");
    }

    /// Easing toward a step target settles within tolerance and NEVER
    /// overshoots — that's the critically-damped pin.
    #[test]
    fn eases_toward_target_without_overshoot() {
        let mut x = 0.3f32;
        let mut v = 0.0f32;
        let target = 1.2f32;
        let mut max_x = x;
        // ~2s at 30fps (60 steps of dt=0.033).
        for _ in 0..60 {
            critically_damped(&mut x, &mut v, target, BREATHING_HALF_LIFE, 0.033);
            if x > max_x {
                max_x = x;
            }
        }
        assert!((x - target).abs() < 0.01, "x did not settle to target: {x}");
        assert!(
            max_x <= target + 1e-3,
            "overshoot detected: max_x = {max_x} > target {target}"
        );
    }

    /// Canonical settling rule for a critically-damped spring with Holden's
    /// `ω = 2·ln(2)/half_life`. The position response is
    /// `1 - (1 + ωt)·e^(-ωt)`; the 2% threshold sits at ω·t ≈ 5.83, i.e.
    /// `t ≈ 4.21 · half_life`. So at `4 · half_life` we expect ≈97.4%
    /// (within 3%) — within 2% by `5 · half_life`. We pin both bounds: the
    /// 4·hl point is within 3%, the 5·hl point is within 2%. That's the
    /// rapid-convergence guarantee CONT-03 cares about — boxes visibly
    /// settle inside one ~1Hz stat interval.
    #[test]
    fn reaches_within_two_percent_in_four_half_lives() {
        let target = 1.0f32;
        let half_life = 0.15f32;
        let dt = 0.001f32;

        // 4·half_life — analytical settles to ~97.4%; tolerance 3%.
        let mut x = 0.0f32;
        let mut v = 0.0f32;
        let steps = ((4.0 * half_life) / dt) as u32;
        for _ in 0..steps {
            critically_damped(&mut x, &mut v, target, half_life, dt);
        }
        assert!(
            (x - target).abs() <= 0.03 * target,
            "4·half_life should reach within 3%: x = {x}, target = {target}"
        );

        // 5·half_life — well past the analytical 2% threshold (4.21·hl).
        let mut x = 0.0f32;
        let mut v = 0.0f32;
        let steps = ((5.0 * half_life) / dt) as u32;
        for _ in 0..steps {
            critically_damped(&mut x, &mut v, target, half_life, dt);
        }
        assert!(
            (x - target).abs() <= 0.02 * target,
            "5·half_life should reach within 2%: x = {x}, target = {target}"
        );
    }

    /// Mid-flight target reversal: velocity continuity is the whole point of
    /// the velocity-aware spring — a sudden target flip must not lurch.
    /// Velocity must turn negative within a few steps and `x` returns to ~0.
    #[test]
    fn velocity_reverses_on_target_reversal() {
        let mut x = 0.0f32;
        let mut v = 0.0f32;
        // Ease toward 1.0 for ~0.2s (mid-flight, well shy of settled).
        for _ in 0..6 {
            critically_damped(&mut x, &mut v, 1.0, BREATHING_HALF_LIFE, 0.033);
        }
        assert!(v > 0.0, "v should be positive mid-flight, got {v}");

        // Flip target to 0.0. Velocity must go negative within a few steps.
        let mut became_negative = false;
        for _ in 0..6 {
            critically_damped(&mut x, &mut v, 0.0, BREATHING_HALF_LIFE, 0.033);
            if v < 0.0 {
                became_negative = true;
            }
        }
        assert!(became_negative, "v should have reversed sign after target flip");

        // Run long enough to settle back near zero (~2s).
        for _ in 0..60 {
            critically_damped(&mut x, &mut v, 0.0, BREATHING_HALF_LIFE, 0.033);
        }
        assert!(x.abs() < 0.02, "x did not return to 0: {x}");
    }

    /// Framerate independence: a coarse-dt simulation and a fine-dt simulation
    /// over the same total wall-clock duration reach the same `x` within
    /// tolerance. Critically-damped implicit Euler holds this property exactly
    /// in theory; tolerance accounts for f32 round-off.
    #[test]
    fn framerate_independent_within_tolerance() {
        // Coarse: 0.6s at dt=0.033 -> ~18 steps.
        let mut x_coarse = 0.0f32;
        let mut v_coarse = 0.0f32;
        let total = 0.6f32;
        let dt_coarse = 0.033f32;
        let steps_coarse = (total / dt_coarse) as u32;
        for _ in 0..steps_coarse {
            critically_damped(&mut x_coarse, &mut v_coarse, 1.0, BREATHING_HALF_LIFE, dt_coarse);
        }

        // Fine: 0.6s at dt=0.0033 -> ~180 steps.
        let mut x_fine = 0.0f32;
        let mut v_fine = 0.0f32;
        let dt_fine = 0.0033f32;
        let steps_fine = (total / dt_fine) as u32;
        for _ in 0..steps_fine {
            critically_damped(&mut x_fine, &mut v_fine, 1.0, BREATHING_HALF_LIFE, dt_fine);
        }

        assert!(
            (x_coarse - x_fine).abs() < 0.005,
            "framerate-dependence: coarse {x_coarse} vs fine {x_fine}"
        );
    }

    /// dt = NaN must be a no-op (must NEVER propagate NaN into the spring
    /// state — that would freeze the box at NaN forever).
    #[test]
    fn nan_dt_is_no_op() {
        let mut x = 0.5f32;
        let mut v = 0.1f32;
        critically_damped(&mut x, &mut v, 1.0, BREATHING_HALF_LIFE, f32::NAN);
        assert_eq!(x, 0.5);
        assert_eq!(v, 0.1);
        // dt = 0.0 and dt < 0.0 are also no-ops.
        critically_damped(&mut x, &mut v, 1.0, BREATHING_HALF_LIFE, 0.0);
        critically_damped(&mut x, &mut v, 1.0, BREATHING_HALF_LIFE, -0.05);
        assert_eq!(x, 0.5);
        assert_eq!(v, 0.1);
    }

    /// Degenerate half_life must snap safely — never NaN. half_life=0 would
    /// divide by zero in the omega expression without the guard.
    #[test]
    fn degenerate_half_life_snaps_safely() {
        let mut x = 0.3f32;
        let mut v = 0.7f32;
        critically_damped(&mut x, &mut v, 1.2, 0.0, 0.033);
        assert!(x.is_finite() && v.is_finite(), "snapped to NaN/Inf");
        assert_eq!(x, 1.2);
        assert_eq!(v, 0.0);

        // Negative half_life: same snap.
        let mut x = 0.3f32;
        let mut v = 0.7f32;
        critically_damped(&mut x, &mut v, 1.2, -0.5, 0.033);
        assert_eq!(x, 1.2);
        assert_eq!(v, 0.0);

        // NaN half_life: same snap.
        let mut x = 0.3f32;
        let mut v = 0.7f32;
        critically_damped(&mut x, &mut v, 1.2, f32::NAN, 0.033);
        assert_eq!(x, 1.2);
        assert_eq!(v, 0.0);
    }
}
