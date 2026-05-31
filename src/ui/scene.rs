//! Framebuffer → braille bridge: the SINGLE adapter that owns the
//! coordinate-system mapping (ARCH Pattern 3).
//!
//! [`SceneShape`] is a ratatui [`Shape`] painted inside a `Marker::Braille`
//! [`Canvas`]. Each frame it:
//!   1. renders the cube to a [`Framebuffer`] sized to the canvas braille
//!      resolution (`2*cols × 4*rows`) via [`render3d::render`], and
//!   2. blits every lit sub-pixel with [`Painter::paint`].
//!
//! ## The braille coordinate convention (the resolved research spike)
//!
//! A ratatui `Canvas` uses MATH coordinates (bottom-left origin, configurable
//! x/y bounds) for its high-level `draw` helpers — but [`Painter::paint`] takes
//! GRID DOT indices directly, with a TOP-LEFT origin (`(0,0)` is the upper-left
//! dot, y increasing downward). The framebuffer is ALSO a top-left grid, and the
//! projector (plan 01-03) ALREADY applied the single NDC→screen Y-flip (pinned by
//! `up_world_point_maps_to_upper_half`). So the mapping here is the IDENTITY:
//!
//!   framebuffer (x, y)  →  painter.paint(x, y, color)
//!
//! No second inversion. Flipping again would put the cube upside-down; the human
//! gate just sanity-checks it isn't. The framebuffer is sized to exactly match
//! the canvas grid resolution so `(x, y)` indices line up 1:1 with braille dots.
//!
//! All color comes from the framebuffer (already palette-derived in `render`);
//! no RGB is named here.
//!
//! ## 04-04 additions
//!
//! - Floor-planes (ENT-01): one wireframe quad per non-empty network group,
//!   pre-built from `LiveWorld::group_bounds_xz()` and threaded through
//!   `SceneExtras` so the renderer draws them BEFORE cube fragments. The
//!   selected entity's group gets the indigo highlight color; unselected
//!   groups use `palette.edge`.
//! - Port-glow dots (ENT-02): per-entity port summaries (read via
//!   `LiveWorld::snapshot`) are passed by reference in the SceneExtras
//!   `PortLookup`. The renderer pass projects them on the camera-facing face.
//! - Selected-only label (CONT-04): the projected anchor + cell hysteresis
//!   are computed BEFORE entering the `Canvas::paint` closure (a `Fn`-bound
//!   closure cannot borrow `&mut Selection`), and only Copy-friendly
//!   primitives travel into the closure for `ctx.print`.

use std::collections::HashMap;

use ratatui::symbols::Marker;
use ratatui::widgets::canvas::{Canvas, Painter, Shape};
use ratatui::widgets::{Block, Borders};
use ratatui::Frame;

use crate::camera::{Camera, DEFAULT_FOV};
use crate::config::RenderConfig;
use crate::render3d::render_scene as raster_render_scene;
use crate::render3d::scene_extras::{Cylinder, FloorPlane, ImageStack, PortLookup, SceneExtras};
use crate::theme::{Palette, Status};
use crate::ui::labels::{project_label_anchor, snap_anchor_with_hysteresis, truncate_label};
use crate::world::live::LiveWorld;
use crate::world::selection::Selection;
use crate::world::{Entity, World};

/// A ratatui [`Shape`] that blits a rendered multi-box framebuffer into a
/// braille canvas. Owns a snapshot of the camera/config/entities it renders with.
struct SceneShape<'a> {
    /// Braille sub-pixel viewport `(w, h) = (2*cols, 4*rows)` — must match the
    /// canvas grid resolution so dot indices line up 1:1.
    viewport: (usize, usize),
    /// The World's boxes to draw, framed by [`Camera::frame_scene`].
    entities: &'a [Entity],
    /// Already framed by [`Camera::frame_scene`] before draw (target/radius set).
    camera: Camera,
    palette: Palette,
    config: RenderConfig,
    /// Current per-box self-spin angle (radians), owned by the app and advanced
    /// on the logic tick. The camera is static; this is the scene's motion.
    spin: f32,
    /// Entity id of the Tab-cycled selection (04-03 / CAM-04), or `None`.
    /// Copy primitive so the `Fn` closure on `Canvas::paint` stays compatible
    /// with the shape's `Sync` bound (no `&Selection` lifetime inside).
    selected_id: Option<u32>,
    /// `[0, 1)` brightness-pulse phase; the rasterizer applies
    /// `1 + 0.18 * sin(phase * TAU)` to the selected entity's RGB.
    selection_pulse_phase: f32,
    /// 04-04 optional primitives (floor-planes + per-entity port lookup) —
    /// borrowed; built fresh per frame by `render_scene` from the live world.
    extras: SceneExtras<'a>,
}

impl Shape for SceneShape<'_> {
    fn draw(&self, painter: &mut Painter) {
        // Degenerate areas: nothing to paint.
        if self.viewport.0 == 0 || self.viewport.1 == 0 {
            return;
        }

        // Camera owns the lens: feed render_scene a ViewParams built from the
        // (already scene-framed) orbit.
        let view = self.camera.view_params(DEFAULT_FOV);
        let fb = raster_render_scene(
            self.entities,
            view,
            self.viewport,
            &self.palette,
            &self.config,
            self.spin,
            self.selected_id,
            self.selection_pulse_phase,
            &self.extras,
        );

        // Blit: framebuffer top-left (x, y) maps to braille dot (x, y) with NO
        // second flip — the projector already flipped Y once (see module docs).
        for (x, y, color) in fb.lit_pixels() {
            painter.paint(x, y, color);
        }
    }
}

/// Build the per-frame [`FloorPlane`] vector for braille from the live world.
///
/// One entry per non-empty network group; color is `palette.edge` for
/// unselected groups, and `palette.status_color(Running)` for the network the
/// SELECTED container belongs to (RESEARCH "ENT-01 Network Floor-Planes" —
/// selected-group highlight).
///
/// Pure (no I/O); reuses [`LiveWorld::group_bounds_xz`] for the geometry.
fn build_floor_planes(
    live: &LiveWorld,
    selection: &Selection,
    palette: &Palette,
) -> Vec<FloorPlane> {
    // Which network does the selected container belong to? `None` when nothing
    // is selected or the id isn't live (race window — drained on count change).
    let selected_group_key: Option<String> = selection
        .selected_id
        .and_then(|id| live.id_string_for_entity(id).map(|s| s.to_string()))
        .and_then(|cid| live.snapshot(&cid).map(|s| s.group_key.clone()));

    live.group_bounds_xz()
        .into_iter()
        .map(|(key, center, half_size_xz)| {
            let color = if selected_group_key.as_deref() == Some(key.as_str()) {
                palette.status_color(Status::Running)
            } else {
                palette.edge
            };
            FloorPlane {
                center,
                half_size_xz,
                color,
            }
        })
        .collect()
}

/// Build the per-frame port lookup borrowing `ContainerSnapshot.ports` from
/// `LiveWorld`. Empty when the live world is empty or no entity has ports.
///
/// The returned `HashMap` borrows directly from the live entries — no clones.
/// The borrow lasts only the current frame (the renderer call), matching the
/// SceneExtras lifetime contract.
fn build_port_lookup<'a>(world: &World, live: &'a LiveWorld) -> PortLookup<'a> {
    let mut map: PortLookup<'a> = HashMap::new();
    for entity in &world.entities {
        // Reverse entity.id -> container id; then read the snapshot's ports.
        if let Some(cid) = live.id_string_for_entity(entity.id) {
            if let Some(snap) = live.snapshot(cid) {
                if !snap.ports.is_empty() {
                    map.insert(entity.id, snap.ports.as_slice());
                }
            }
        }
    }
    map
}

/// Build per-frame volume cylinders for the braille path (ENT-03 / 04-05).
///
/// One [`Cylinder`] per entity with `mount_count >= 1`. The cylinder sits
/// on TOP of the cube (`center.y = entity.position.y + half_extents.y`),
/// with radius narrower than the cube so it reads as a separate object
/// (`0.4 * half_extents.x`). Height comes from the mount-count proxy
/// (`world::volume::proxy_volume_height`). Color is `palette.volume`.
///
/// I12 closure: uses the existing `LiveWorld::snapshot(id).mount_count`
/// accessor (added in 04-02) — NO `LiveWorld::mount_count(id)` helper.
fn build_volume_cylinders(world: &World, live: &LiveWorld, palette: &Palette) -> Vec<Cylinder> {
    let mut out: Vec<Cylinder> = Vec::new();
    for entity in &world.entities {
        let mc = live
            .id_string_for_entity(entity.id)
            .and_then(|cid| live.snapshot(cid))
            .map(|s| s.mount_count)
            .unwrap_or(0);
        if mc == 0 {
            continue;
        }
        let height = crate::world::volume::proxy_volume_height(mc);
        let center = glam::Vec3::new(
            entity.position.x,
            entity.position.y + entity.half_extents.y,
            entity.position.z,
        );
        let radius = entity.half_extents.x * 0.4;
        out.push(Cylinder {
            center,
            radius,
            height,
            color: palette.volume,
        });
    }
    out
}

/// Build per-frame image stacks for the braille path (ENT-04 / 04-05).
///
/// One [`ImageStack`] per image in `LiveWorld::image_stack_positions()`.
/// Positions are deterministic (BTreeMap-by-id order + MAX_RACK_X
/// constant — W9 closure). Drops the repo_tag tuple element (labels not
/// part of the v1 stack visual).
fn build_image_stacks(live: &LiveWorld) -> Vec<ImageStack> {
    live.image_stack_positions()
        .into_iter()
        .map(|(base, layer_count, _tag)| ImageStack {
            base,
            layer_count,
        })
        .collect()
}

/// Render the orbiting World of boxes into the scene `area`.
///
/// Builds a `Marker::Braille` canvas inside a bordered "scene" block and paints
/// a [`SceneShape`] sized to the canvas's braille grid resolution. The viewport
/// is derived from the LIVE inner area every frame, so a resize re-sizes the
/// framebuffer with no stale cache (PITFALLS #14).
///
/// `selection` is mutably borrowed for the duration of the label hysteresis
/// PRE-COMPUTATION only — by the time we construct the `Canvas::paint`
/// closure, the `&mut` borrow has been dropped and only Copy primitives
/// travel inside (the closure is `Fn`, not `FnMut`).
/// `marker` (05-06 ROB-02): the Canvas cell marker. `Marker::Braille` is
/// the truecolor tier (1×2 dot × 1×4 sub-pixel per cell). The Ascii tier
/// passes `Marker::Block` (one solid '█' per cell — coarser but SSH /
/// dumb-terminal friendly with no Unicode-braille dependency).
#[allow(clippy::too_many_arguments)]
pub fn render_scene(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    camera: &Camera,
    world: &World,
    live: &LiveWorld,
    selection: &mut Selection,
    palette: &Palette,
    config: &RenderConfig,
    spin: f32,
    marker: Marker,
) {
    let block = Block::default().title("scene").borders(Borders::ALL);

    // Inner area = canvas grid in CELLS (block borders removed). Braille grid
    // resolution is 2 dots wide × 4 dots tall per cell.
    let inner = block.inner(area);
    let cells = (inner.width as usize, inner.height as usize);
    let viewport = (cells.0 * 2, cells.1 * 4);

    // 04-04 optional primitives — built per-frame from the live world.
    let floors = build_floor_planes(live, selection, palette);
    let ports = build_port_lookup(world, live);
    // 04-05 optional primitives: volume cylinders + image stacks. Both are
    // owned `Vec`s borrowed by the SceneExtras for the canvas closure's
    // lifetime (the shape captures the extras via `move`, so the binding
    // here must stay alive until the closure runs).
    let cylinders = build_volume_cylinders(world, live, palette);
    let image_stacks = build_image_stacks(live);

    // -- LABEL PRE-COMPUTATION (option (a)) --
    // The `Canvas::paint` closure is `Fn` (not `FnMut`), so &mut Selection cannot
    // cross the closure boundary. Snap the anchor + mutate hysteresis state
    // HERE, then move the owned `String` into the closure with `move`. The
    // closure also moves the SceneShape (which borrows from outer locals), so
    // we MUST keep the floors / ports / label_print bindings alive until the
    // closure runs (they're moved into it). `frame.render_widget(canvas, area)`
    // consumes the canvas and runs the closure synchronously, so the
    // pre-closure bindings can be dropped right after.
    let label_print: Option<(i16, i16, String)> = selection
        .selected_entity(world)
        .and_then(|sel_entity| {
            let dot_anchor = project_label_anchor(
                camera,
                sel_entity,
                (viewport.0 as u32, viewport.1 as u32),
                config.cell_aspect,
            )?;
            // Braille cell = 2 dots wide × 4 dots tall.
            let (col, row) =
                snap_anchor_with_hysteresis(dot_anchor, &mut selection.last_label_cell, (2, 4));
            // Container name via the live snapshot. Truncated for safety.
            let name_owned = live
                .id_string_for_entity(sel_entity.id)
                .and_then(|cid| live.snapshot(cid))
                .map(|s| truncate_label(s.name.as_str()).into_owned())
                .unwrap_or_default();
            if name_owned.is_empty() {
                return None;
            }
            // Print one CELL above the box top. Convert dot coords to cell coords
            // for the print position. snap returned (col, row) already in cells.
            let print_row = row.saturating_sub(1);
            Some((col, print_row, name_owned))
        });

    // Build the SceneShape AFTER the &mut selection borrow is released — clone
    // the world entities slice into the shape's owned reference container via
    // a borrow that lives only inside the canvas closure.
    let shape = SceneShape {
        viewport,
        entities: world.entities.as_slice(),
        camera: *camera,
        palette: *palette,
        config: *config,
        spin,
        selected_id: selection.selected_id,
        selection_pulse_phase: selection.pulse_phase,
        extras: SceneExtras::new(
            floors.as_slice(),
            &ports,
            cylinders.as_slice(),
            image_stacks.as_slice(),
        ),
    };

    // Canvas X bounds are in DOT coordinates (top-left origin); ctx.print
    // takes the same. Cell coords from snap are converted to dot coords by
    // multiplying by (2, 4) — the same conversion the canvas uses internally
    // for its braille grid. Pin print position in DOT coords so ratatui's
    // cell quantization lands the text where we computed.
    let canvas = Canvas::default()
        .block(block)
        .marker(marker)
        .background_color(palette.background)
        // Bounds match the braille dot grid (top-left math via Painter::paint,
        // not these high-level bounds — kept 1:1 with the grid for clarity).
        .x_bounds([0.0, viewport.0.max(1) as f64 - 1.0])
        .y_bounds([0.0, viewport.1.max(1) as f64 - 1.0])
        .paint(move |ctx| {
            ctx.draw(&shape);
            if let Some((col, row, name)) = label_print.as_ref() {
                // Cells -> dot coords (canvas bounds are in dots, but ratatui
                // print is cell-quantized internally and we already snapped to
                // cell granularity). Map by (col*2, row*4) so the text lands
                // on a cell boundary, then flip Y back to math coords
                // (Canvas.print uses math coords: bottom-left origin).
                let cx_dot = (*col as f64) * 2.0;
                // Canvas y_bounds are bottom-left math: y=0 at the bottom,
                // y=viewport.1-1 at the top. Our (row) is in TOP-LEFT cell
                // coords; convert to math y at the cell top.
                let cy_dot_top_left = (*row as f64) * 4.0;
                let cy_math = (viewport.1 as f64 - 1.0) - cy_dot_top_left;
                // Owned ratatui Line so the lifetime is 'static and doesn't
                // tangle with the ctx borrow. Clone is cheap (a small String).
                let line = ratatui::text::Line::from(name.clone());
                ctx.print(cx_dot, cy_math, line);
            }
        });

    frame.render_widget(canvas, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Status;
    use crate::world::live::DockerMsg;
    use crate::world::scene::SceneBounds;
    use crate::world::World;
    use crate::docker::ContainerSnapshot;

    fn snap(id: &str) -> ContainerSnapshot {
        ContainerSnapshot {
            id: id.to_string(),
            name: id.to_string(),
            status: Status::Running,
            group_key: "net0".to_string(),
            ..ContainerSnapshot::default()
        }
    }

    /// build_floor_planes returns one entry per non-empty group; the selected
    /// container's group gets the indigo highlight color, others get edge gray.
    #[test]
    fn build_floor_planes_highlights_selected_group() {
        let palette = Palette::default();
        let mut live = LiveWorld::new();
        // net0 holds "a"; net1 holds "b" (a different group_key).
        live.apply(DockerMsg::Added(snap("a")));
        live.apply(DockerMsg::Added(ContainerSnapshot {
            id: "b".to_string(),
            name: "b".to_string(),
            status: Status::Running,
            group_key: "net1".to_string(),
            ..ContainerSnapshot::default()
        }));
        let mut selection = Selection::new();
        // Select entity id 0 (first slot in net0).
        selection.selected_id = Some(0);

        let floors = build_floor_planes(&live, &selection, &palette);
        assert_eq!(floors.len(), 2, "one floor per non-empty group");
        // net0 (selected) -> running color; net1 (unselected) -> edge color.
        assert_eq!(floors[0].color, palette.status_color(Status::Running));
        assert_eq!(floors[1].color, palette.edge);
    }

    /// build_floor_planes with no selection paints every floor in the muted
    /// edge color.
    #[test]
    fn build_floor_planes_no_selection_is_all_edge() {
        let palette = Palette::default();
        let mut live = LiveWorld::new();
        live.apply(DockerMsg::Added(snap("a")));
        let selection = Selection::new();
        let floors = build_floor_planes(&live, &selection, &palette);
        assert_eq!(floors.len(), 1);
        assert_eq!(floors[0].color, palette.edge);
    }

    /// build_port_lookup borrows ports from the live entries — empty when no
    /// container has any.
    #[test]
    fn build_port_lookup_empty_when_no_ports() {
        let mut live = LiveWorld::new();
        live.apply(DockerMsg::Added(snap("a")));
        let entities = vec![Entity {
            id: 0,
            position: glam::Vec3::ZERO,
            half_extents: glam::Vec3::splat(0.5),
            status: Status::Running,
            group: 0,
        }];
        let world = World {
            entities,
            bounds: SceneBounds::from_entities(&[]),
        };
        let ports = build_port_lookup(&world, &live);
        assert!(ports.is_empty());
    }
}
