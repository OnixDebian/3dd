//! Volume size proxy (ENT-03 / 04-05).
//!
//! Maps a container's `mount_count` onto a cylinder height in world units.
//! Bytes-on-disk is NOT cheaply available from the Docker API (RESEARCH
//! Pitfall F: `VolumeUsageData.size = -1` for non-`local` drivers, and
//! `/system/df` walks the filesystem — minutes-scale on large hosts —
//! moby#31951). v1 uses a mount-count proxy: a container with more
//! mounts visibly has more disk surface than a container with none.
//!
//! Worst-case correctness loss: two containers with the same mount count
//! but vastly different on-disk volume sizes (one mounts /tmp empty,
//! the other mounts /data 50GB) render identical cylinders. Acceptable
//! for a passive visualizer; v2 DATA-01 closes it via on-demand
//! `system df -v` behind an explicit user key.
//!
//! Pure (no I/O); shared by both backends.

#![allow(dead_code)]

/// Minimum cylinder height (one mount). Picked so the cylinder reads as
/// a visible disk on top of even the smallest container box (MIN_HALF =
/// 0.3 cube; a 0.2-tall cylinder is two thirds the cube height — clearly
/// visible).
pub const MIN_VOL_H: f32 = 0.2;

/// Maximum cylinder height (saturating at MOUNT_REF mounts). Picked so
/// a max-out cylinder is about as tall as a MAX_HALF=1.2 cube — same
/// visual weight as the container it sits on.
pub const MAX_VOL_H: f32 = 1.2;

/// Mount count at which the cylinder saturates to MAX_VOL_H. Most
/// production containers have 1–3 mounts; 4 is a comfortable saturation
/// point that lets a busy data container max out visually without a
/// hog (a 30-mount oddball) dominating the rack.
pub const MOUNT_REF: f32 = 4.0;

/// Map `mount_count` to a cylinder height in world units (ENT-03).
///
/// Returns `MIN_VOL_H` at `mount_count == 0` (degenerate; call sites
/// should filter out zero-mount containers BEFORE building a cylinder,
/// so this branch is defensive only). The mapping is sqrt-compressive
/// (same family as `load_to_half_extent`): going from 0 → 1 mount adds
/// a lot of height; going from 3 → 4 mounts adds a little. A busy
/// volume host's rack reads as "lots of cylinders mid-height" rather
/// than "one tower dominating".
pub fn proxy_volume_height(mount_count: usize) -> f32 {
    let load = (mount_count as f32 / MOUNT_REF).clamp(0.0, 1.0);
    MIN_VOL_H + (MAX_VOL_H - MIN_VOL_H) * load.sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Zero mounts maps to the floor — call sites filter zero-mount entries
    /// BEFORE building a cylinder, but the defensive floor keeps the curve
    /// well-defined.
    #[test]
    fn proxy_volume_height_zero_is_min_h() {
        let h = proxy_volume_height(0);
        assert!(
            (h - MIN_VOL_H).abs() < 1e-6,
            "expected MIN_VOL_H={MIN_VOL_H}, got {h}"
        );
    }

    /// Mount count = MOUNT_REF (4) saturates to MAX_VOL_H.
    #[test]
    fn proxy_volume_height_saturates_at_ref() {
        let h = proxy_volume_height(4);
        assert!(
            (h - MAX_VOL_H).abs() < 1e-6,
            "expected MAX_VOL_H={MAX_VOL_H}, got {h}"
        );
    }

    /// Beyond MOUNT_REF the curve clamps; a 30-mount oddball does NOT tower.
    #[test]
    fn proxy_volume_height_clamps_above_ref() {
        let h_ref = proxy_volume_height(4);
        let h_huge = proxy_volume_height(30);
        assert!((h_ref - h_huge).abs() < 1e-6, "saturation must clamp; got ref={h_ref}, huge={h_huge}");
    }

    /// sqrt-compressive: gain from 0->1 mount EXCEEDS gain from 3->4 mounts.
    /// Same shape as `load_to_half_extent`'s sqrt curve — a 1-mount container
    /// gains a lot of visible height; a 4th-mount container barely adds any.
    #[test]
    fn proxy_volume_height_compressive() {
        let g_01 = proxy_volume_height(1) - proxy_volume_height(0);
        let g_34 = proxy_volume_height(4) - proxy_volume_height(3);
        assert!(
            g_01 > g_34,
            "sqrt-compressive: expected gain 0->1 ({g_01}) > gain 3->4 ({g_34})"
        );
    }

    /// Monotonic: more mounts -> bigger cylinder (within saturation).
    #[test]
    fn proxy_volume_height_monotonic() {
        assert!(proxy_volume_height(0) < proxy_volume_height(1));
        assert!(proxy_volume_height(1) < proxy_volume_height(2));
        assert!(proxy_volume_height(2) < proxy_volume_height(3));
        // 4 onwards saturates — equal, not strictly less.
    }
}
