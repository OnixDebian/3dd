//! Pure helpers for CONT-04 selected-only labels.
//!
//! Two responsibilities:
//!
//! 1. **[`project_label_anchor`]** — given a camera, the selected entity, a
//!    viewport size, and the renderer's cell-aspect, project the entity's
//!    top-center into screen-resolution coordinates (DOT units for braille,
//!    PIXEL units for kitty). Returns `None` when the anchor projects outside
//!    the frustum so callers can drop the label that frame (no edge-clamp
//!    that would stick the name to a viewport corner).
//!
//! 2. **[`snap_anchor_with_hysteresis`]** — quantize the float anchor to a
//!    cell-grid integer coordinate, but only UPDATE the cached value when the
//!    new cell differs from the stored one by ≥1 on either axis. The static
//!    stored cell is what the caller prints to, so a sub-cell wobble during
//!    spin / breathing leaves the label still (RESEARCH "Hysteresis rule").
//!
//! Both helpers are pure (no closures, no I/O); the hysteresis state lives on
//! `Selection` (`last_label_cell` for braille; `last_label_cell_kitty` for
//! kitty — different cell coordinate systems, per RESEARCH Open Question #4).
//!
//! Long names get truncated with a UTF-8-safe ellipsis at [`MAX_LABEL_LEN`]
//! so a 60-char name doesn't punch through the next entity's column.

#![allow(dead_code)]

use glam::Vec3;

use crate::camera::{Camera, DEFAULT_FOV};
use crate::config::RenderConfig;
use crate::render3d::project::Projector;
use crate::world::entity::Entity;

/// Maximum label length in characters before truncation. 24 fits comfortably
/// inside a single rack column even on a narrow terminal; the renderer
/// truncates and appends `…` so the visible label never overflows.
pub const MAX_LABEL_LEN: usize = 24;

/// Project the entity's top-center (`position + Y * half_extents.y`) into
/// renderer-native screen coordinates. Returns `None` if the projected point
/// falls outside the canonical view frustum (behind the near plane, or off
/// any axis) — callers drop the label for that frame rather than clamping to
/// the viewport edge (RESEARCH "What 'occlusion-aware' means here").
///
/// `viewport` is the screen-resolution viewport — `(dot_w, dot_h)` for the
/// braille path (= `2 * cells_w`, `4 * cells_h`) and `(px_w, px_h)` for the
/// kitty path. `cell_aspect` is the projector's correction factor — for
/// the kitty path it is 1.0 (square pixels), for the braille path it is
/// derived from the LIVE terminal cell pixel size via
/// [`crate::config::RenderConfig::braille_cell_aspect_for_cell`] (05-06
/// RV4: at typical 2:1 cells this is also 1.0 so a unit cube reads as
/// cubic at the SAME shape on both tiers). The caller must use the value
/// that matches the renderer's projector exactly so the label tracks the
/// rasterized box.
pub fn project_label_anchor(
    camera: &Camera,
    entity: &Entity,
    viewport: (u32, u32),
    cell_aspect: f32,
) -> Option<(f32, f32)> {
    let view = camera.view_params(DEFAULT_FOV);
    let proj_config = RenderConfig {
        fov: view.fov,
        cell_aspect,
        ..RenderConfig::default()
    };
    let proj = Projector::new(view.eye, view.target, view.up, viewport, &proj_config);
    let top = entity.position + Vec3::Y * entity.half_extents.y;
    proj.project(top).map(|(x, y, _z)| (x, y))
}

/// Snap a float anchor to a cell-grid integer coordinate with HYSTERESIS:
/// only update the cached cell when the new candidate differs from the
/// stored cell by ≥1 cell on either axis. Returns the cell to PRINT at
/// (the stored value AFTER the update, which is unchanged when the
/// candidate falls inside the current cell).
///
/// `cells_per_unit` says how many SCREEN-RESOLUTION units fit in one cell:
/// `(2, 4)` for the braille path (2 dots/col × 4 dots/row), `(cell_w, cell_h)`
/// in PIXELS for the kitty path (resolved at the call site from
/// `crossterm::terminal::window_size`).
///
/// The first call (when `last_cell` is `None`) stores and returns the fresh
/// quantized cell — there's nothing to hysteresize against. Subsequent calls
/// jump only when the candidate has crossed a cell boundary on at least one
/// axis; this kills the per-frame jitter the box's breathing / spin would
/// otherwise inject into the projected anchor.
pub fn snap_anchor_with_hysteresis(
    anchor: (f32, f32),
    last_cell: &mut Option<(i16, i16)>,
    cells_per_unit: (u32, u32),
) -> (i16, i16) {
    let (cu_x, cu_y) = (cells_per_unit.0.max(1) as f32, cells_per_unit.1.max(1) as f32);
    let candidate = (
        (anchor.0 / cu_x).floor() as i16,
        (anchor.1 / cu_y).floor() as i16,
    );
    match *last_cell {
        None => {
            *last_cell = Some(candidate);
            candidate
        }
        Some(prev) => {
            if (candidate.0 - prev.0).abs() >= 1 || (candidate.1 - prev.1).abs() >= 1 {
                *last_cell = Some(candidate);
                candidate
            } else {
                prev
            }
        }
    }
}

/// Trim a label to at most [`MAX_LABEL_LEN`] characters, replacing the tail
/// with a single `…` when truncated. UTF-8-safe (operates on chars). Returns
/// a borrowed `&str` when no truncation is needed and an owned `String`
/// otherwise — the `Cow`-free caller pattern is just
/// `let label = truncate_label(name);` then `print(label.as_ref())`.
pub fn truncate_label(name: &str) -> std::borrow::Cow<'_, str> {
    let count = name.chars().count();
    if count <= MAX_LABEL_LEN {
        return std::borrow::Cow::Borrowed(name);
    }
    // Reserve one char for the ellipsis.
    let mut out: String = name.chars().take(MAX_LABEL_LEN - 1).collect();
    out.push('…');
    std::borrow::Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- snap_anchor_with_hysteresis ----------------------------------------

    /// First call seeds the cell and returns it directly.
    #[test]
    fn first_call_stores_and_returns_new_cell() {
        let mut last = None;
        let cell = snap_anchor_with_hysteresis((10.0, 16.0), &mut last, (2, 4));
        assert_eq!(cell, (5, 4));
        assert_eq!(last, Some((5, 4)));
    }

    /// Sub-cell drift (anchor still falls inside the cached cell) returns
    /// the cached cell unchanged — the static "no-shiver" property.
    #[test]
    fn sub_cell_drift_returns_previous_cell() {
        let mut last = Some((5, 5));
        // (10.1, 21.0) / (2, 4) = (5.05, 5.25) -> candidate (5, 5) == prev.
        let cell = snap_anchor_with_hysteresis((10.1, 21.0), &mut last, (2, 4));
        assert_eq!(cell, (5, 5));
        assert_eq!(last, Some((5, 5)));
    }

    /// Crossing a cell on the horizontal axis updates and returns the new
    /// cell.
    #[test]
    fn horizontal_drift_updates_and_returns_new() {
        let mut last = Some((5, 5));
        // (13.0, 21.0) / (2, 4) = (6.5, 5.25) -> candidate (6, 5).
        let cell = snap_anchor_with_hysteresis((13.0, 21.0), &mut last, (2, 4));
        assert_eq!(cell, (6, 5));
        assert_eq!(last, Some((6, 5)));
    }

    /// Crossing a cell on the vertical axis also updates.
    #[test]
    fn vertical_drift_updates() {
        let mut last = Some((5, 5));
        // (10.0, 25.0) / (2, 4) = (5.0, 6.25) -> candidate (5, 6).
        let cell = snap_anchor_with_hysteresis((10.0, 25.0), &mut last, (2, 4));
        assert_eq!(cell, (5, 6));
        assert_eq!(last, Some((5, 6)));
    }

    /// Zero cells_per_unit doesn't panic — clamps to 1.
    #[test]
    fn zero_cells_per_unit_does_not_panic() {
        let mut last = None;
        let cell = snap_anchor_with_hysteresis((10.0, 20.0), &mut last, (0, 0));
        // With cu (1, 1) candidate is (10, 20).
        assert_eq!(cell, (10, 20));
    }

    // -- truncate_label -----------------------------------------------------

    #[test]
    fn truncate_short_label_is_borrowed() {
        let cow = truncate_label("abc");
        assert_eq!(cow.as_ref(), "abc");
        assert!(matches!(cow, std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn truncate_long_label_appends_ellipsis() {
        let long = "a".repeat(MAX_LABEL_LEN + 10);
        let cow = truncate_label(&long);
        let s = cow.as_ref();
        assert_eq!(s.chars().count(), MAX_LABEL_LEN);
        assert!(s.ends_with('…'));
    }

    #[test]
    fn truncate_at_exact_length_does_not_truncate() {
        let exact = "a".repeat(MAX_LABEL_LEN);
        let cow = truncate_label(&exact);
        assert_eq!(cow.as_ref(), exact);
    }

    // -- project_label_anchor -----------------------------------------------

    /// A box at the camera target projects close to the viewport center.
    #[test]
    fn project_anchor_at_target_is_near_center() {
        let cam = Camera::new();
        // Place the entity at the camera's target (origin by default).
        let entity = Entity {
            id: 0,
            position: cam.target,
            half_extents: Vec3::splat(0.3),
            status: crate::theme::Status::Running,
            group: 0,
        };
        let viewport = (160u32, 120u32);
        // RV4: typical-cell braille `cell_aspect = 1.0` (parity with kitty).
        let anchor = project_label_anchor(&cam, &entity, viewport, 1.0)
            .expect("entity at target must project");
        let (cx, cy) = (viewport.0 as f32 / 2.0, viewport.1 as f32 / 2.0);
        // Top of a box at target is above center; X stays near center.
        assert!((anchor.0 - cx).abs() < 16.0, "anchor.x = {}, cx = {cx}", anchor.0);
        assert!(anchor.1 < cy, "anchor must be above center (smaller y)");
    }

    /// A box well behind the camera projects to None (frustum-cull).
    #[test]
    fn project_anchor_behind_camera_is_none() {
        let cam = Camera::new();
        // Behind the camera: along the eye->target direction past the camera.
        let eye = cam.eye();
        let behind = eye + (eye - cam.target).normalize() * 100.0;
        let entity = Entity {
            id: 0,
            position: behind,
            half_extents: Vec3::splat(0.3),
            status: crate::theme::Status::Running,
            group: 0,
        };
        // RV4: typical-cell braille `cell_aspect = 1.0` (parity with kitty).
        assert!(project_label_anchor(&cam, &entity, (160, 120), 1.0).is_none());
    }
}
