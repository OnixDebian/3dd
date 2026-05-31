//! Solid-cube rasterizer: painter's-sorted, depth-shaded, fogged face fill.
//!
//! `render()` turns a [`Cube`] + [`ViewParams`] into a [`Framebuffer`] of a cube
//! that reads as a SOLID 3D form (REND-01/02/03):
//!   1. Build a [`Projector`] (plan 01-03) from the raw view params.
//!   2. Back-face cull (drop faces whose outward normal points away from the eye).
//!   3. PAINTER'S SORT: draw faces farthest-first so nearer faces overwrite (the
//!      single strongest depth cue — PITFALLS.md Pitfall 4 says solid + occlusion
//!      beats wireframe). No per-pixel z-buffer: a convex cube needs none.
//!   4. Fill each face (two triangles, barycentric) shaded by orientation
//!      (Lambert-ish) and distance FOG via `theme::dim` — front/near bright,
//!      side/far dimmer.
//!
//! ARCH: PURE — no terminal, no ratatui Terminal, no I/O. Returns a Framebuffer;
//! the braille blit happens in plan 05. All color flows through the [`Palette`]
//! (no inline RGB here).

// `render` is consumed by plan 05; helpers exercised by tests today.
#![allow(dead_code)]

use glam::Vec3;
use ratatui::style::Color;

use crate::config::RenderConfig;
use crate::render3d::cube::{unit_cube, Cube, CUBE_EDGES};
use crate::render3d::framebuffer::Framebuffer;
use crate::render3d::plane::rasterize_floor_plane;
use crate::render3d::project::Projector;
use crate::render3d::scene_extras::SceneExtras;
use crate::render3d::{rotate_y_about, ViewParams};
use crate::theme::{self, Palette, Status};
use crate::world::entity::Entity;

/// Lowest brightness any visible face keeps after Lambert shading, so a face
/// turned fully edge-on stays clearly colored (never near-black). Raised so even
/// the most oblique visible face reads as vividly colored, while the lit-vs-unlit
/// gap (this floor → 1.0) still carries enough contrast to read as 3D.
const MIN_LAMBERT: f32 = 0.62;

/// Anti-aliasing supersample factor per braille dot, per axis. The cube is
/// rasterized into a framebuffer `SS`× larger on each axis, then downsampled back
/// to the braille resolution (see [`resolve_supersampled`]). Supersampling
/// positions silhouette and face-boundary edges to sub-dot accuracy, removing the
/// long "wavy wall" stairsteps that near-horizontal edges otherwise produce at the
/// 2×4-dot braille resolution. Cost is ~SS² more fill work; the frame is small so
/// it stays well under the FPS budget.
const SS: usize = 3;

/// Render `cube` into a fresh braille-resolution framebuffer.
///
/// `viewport` is the braille SUB-PIXEL size `(w, h)` (`2*cells_w`, `4*cells_h`).
/// The camera comes in as [`ViewParams`] — NOT a `Camera` type — so render3d stays
/// decoupled (plan 05's `Camera` builds a `ViewParams` and calls this).
///
/// `view.fov` overrides `config.fov` (the camera owns its lens); near/far and the
/// load-bearing `cell_aspect` come from `config`. Faces are drawn into an
/// `SS`×-supersampled buffer and resolved with coverage blending (see [`SS`]) for
/// anti-aliased edges.
pub fn render(
    cube: &Cube,
    view: ViewParams,
    viewport: (usize, usize),
    palette: &Palette,
    config: &RenderConfig,
) -> Framebuffer {
    let (w, h) = viewport;
    // Supersampled buffer: SS× the braille resolution on each axis. Scaling both
    // axes by the same factor preserves the px_w/px_h ratio the projector uses for
    // cell-aspect correction, so projection is identical — just higher-res.
    let (hw, hh) = (w * SS, h * SS);
    let mut hi = Framebuffer::new(hw, hh);

    // Camera owns the lens: fold view.fov into the projector's config.
    let proj_config = RenderConfig {
        fov: view.fov,
        ..*config
    };
    let projector = Projector::new(
        view.eye,
        view.target,
        view.up,
        (hw as u32, hh as u32),
        &proj_config,
    );

    // Synthetic cube: base color is the "running/alive" status color (a palette
    // lookup, never an inline RGB).
    let base = palette.status_color(Status::Running);

    // World-space verts + centroid + distance for cull, sort, and fog. Each face
    // carries its OWN 4 world-space vertices (here, the un-transformed cube verts)
    // so the shared `fill_face` path is identical to the multi-box `render_scene`.
    let mut visible: Vec<RenderFace> = cube
        .faces
        .iter()
        .filter_map(|face| {
            let verts = [
                cube.vertices[face.indices[0]],
                cube.vertices[face.indices[1]],
                cube.vertices[face.indices[2]],
                cube.vertices[face.indices[3]],
            ];
            let centroid = verts.iter().copied().sum::<Vec3>() / 4.0;
            let to_eye = view.eye - centroid;
            // Back-face cull: keep only faces whose outward normal faces the eye.
            if face.normal.dot(to_eye) <= 0.0 {
                return None;
            }
            Some(RenderFace {
                verts,
                normal: face.normal,
                distance: to_eye.length(),
                base,
                pulse_mult: 1.0,
            })
        })
        .collect();

    if visible.is_empty() {
        return Framebuffer::new(w, h);
    }

    // PAINTER'S SORT (REND-02): farthest first, so nearer faces are drawn last
    // and overwrite the far ones in the framebuffer.
    visible.sort_unstable_by(|a, b| {
        b.distance
            .partial_cmp(&a.distance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Fog range: map face distances across the visible spread so the nearest face
    // gets the brightest fog factor and the farthest the dimmest.
    let (near_d, far_d) = distance_range(&visible);

    let to_eye_dir = (view.eye - view.target).normalize_or_zero();

    shade_and_fill(&mut hi, &projector, &visible, to_eye_dir, near_d, far_d, palette);

    resolve_supersampled(&hi, w, h)
}

/// Render an arbitrary [`World`](crate::world::World) of boxes into a fresh
/// braille-resolution framebuffer (the multi-box generalization of [`render`]).
///
/// Each [`Entity`] is the unit cube scaled by `2 * half_extents` (the unit cube
/// spans ±0.5, so a side of `2 * half_extent`) and translated to its world
/// `position`. Every visible face of EVERY box is gathered into ONE list,
/// painter's-sorted farthest-first as a SINGLE pool (so a near box correctly
/// occludes a far one — inter-box ordering, not per-box-then-concatenated), then
/// fog is applied across the scene-wide distance range so far boxes dim and the
/// rack reads at depth. No per-pixel z-buffer — painter's sort over convex
/// axis-aligned boxes is sufficient (PITFALLS #4).
///
/// `viewport`, `view`, and `config` behave exactly as in [`render`]. Per-box
/// color is `palette.status_color(entity.status)` (CONT-01) — no inline RGB.
///
/// `spin` is the current per-box self-rotation angle (radians) about each box's
/// OWN vertical (+Y) axis. The camera is now static (no scene orbit — the human's
/// verify-tuning override of the autopilot orbit), and the motion comes from each
/// box spinning in place: every box's 8 world vertices AND its face normals are
/// rotated about that box's center by `spin` before culling/projecting. Rotating
/// a rigid box keeps it convex, so the cross-box painter's sort is still correct.
///
/// `force_solid` (05-06 RV5): when `true`, EVERY entity is rendered via the
/// SOLID back-face-culled face path regardless of `Status::is_solid()`. The
/// wireframe path on non-Running statuses pushes ALL 12 edges (no cull), so
/// back-of-cube edges from far boxes bleed through near-box faces and the
/// scene reads as "see-through" mush on the coarse ASCII marker (RV5 user
/// feedback "убрать прозрачность блоков"). Kitty + Truecolor pass `false`
/// here and preserve the wireframe-on-Paused/Stopped/Crashed visual.
#[allow(clippy::too_many_arguments)]
pub fn render_scene(
    entities: &[Entity],
    view: ViewParams,
    viewport: (usize, usize),
    palette: &Palette,
    config: &RenderConfig,
    spin: f32,
    selected_id: Option<u32>,
    selection_pulse_phase: f32,
    extras: &SceneExtras<'_>,
    force_solid: bool,
) -> Framebuffer {
    let (w, h) = viewport;
    let (hw, hh) = (w * SS, h * SS);
    let mut hi = Framebuffer::new(hw, hh);

    let proj_config = RenderConfig {
        fov: view.fov,
        ..*config
    };
    let projector = Projector::new(
        view.eye,
        view.target,
        view.up,
        (hw as u32, hh as u32),
        &proj_config,
    );

    // ENT-01 (04-04): network floor-planes go DOWN FIRST. They sit at fixed
    // y below the box floor, so cube fragments coming via the painter's sort
    // overwrite them where their projected footprints overlap on screen (a
    // floor edge under a near box is correctly hidden). Drawing into the
    // same SS framebuffer means the resolve_supersampled pass downsamples
    // floor + cubes together, keeping the visual weight consistent.
    for floor in extras.floors {
        rasterize_floor_plane(&mut hi, &projector, floor.center, floor.half_size_xz, floor.color);
    }

    // The shared unit-cube topology (indices + outward normals); each entity
    // materializes its OWN 8 world verts from these.
    let cube = unit_cube();

    // ONE combined fragment list spanning EVERY box (cross-box painter's pool).
    // A SOLID entity (Status::Running) contributes up to 6 Face fragments
    // (back-face culled); a wireframe entity contributes 12 Edge fragments
    // (all 12 cube edges, no culling — even back edges show, since the cube
    // is "transparent"). Sort by distance and draw back-to-front: a near
    // SOLID face overwrites the wireframe edges behind it (occluded), and a
    // near wireframe's edges overwrite faces behind it (drawn on top).
    // Per-entity brightness pulse for the Tab-selected box (04-03 / CAM-04).
    // Computed once per entity (constant across its faces); the renderer
    // multiplies it into the resolved color and clamps to [0, 255].
    let pulse_mult_for = |entity_id: u32| -> f32 {
        if Some(entity_id) == selected_id {
            const AMPLITUDE: f32 = 0.18;
            1.0 + AMPLITUDE * (selection_pulse_phase * std::f32::consts::TAU).sin()
        } else {
            1.0
        }
    };

    let mut fragments: Vec<Fragment> = Vec::new();
    for entity in entities {
        let scale = entity.half_extents * 2.0;
        let mut world_verts = [Vec3::ZERO; 8];
        for (slot, &v) in world_verts.iter_mut().zip(cube.vertices.iter()) {
            let placed = entity.position + v * scale;
            *slot = rotate_y_about(placed, entity.position, spin);
        }
        let pulse_mult = pulse_mult_for(entity.id);

        // 05-06 RV5: ASCII tier forces every box through the SOLID path so
        // back-of-cube wireframe edges don't bleed through near-box faces
        // (user feedback "убрать прозрачность блоков"). Kitty + Truecolor
        // pass `force_solid=false` and keep `status.is_solid()` — wireframes
        // on non-Running statuses remain the truecolor / kitty visual.
        let render_as_solid = entity.status.is_solid() || force_solid;
        if render_as_solid {
            // SOLID PATH: cull back-faces, push the visible faces as fragments.
            // Color: the entity's own status color (Running -> palette.running;
            // Paused/Stopped/Restarting/Crashed -> their respective status
            // colors). The face shader still applies fog + Lambert, so the
            // resulting silhouette reads as a colored opaque cube.
            let base = palette.status_color(entity.status);
            for face in &cube.faces {
                let verts = [
                    world_verts[face.indices[0]],
                    world_verts[face.indices[1]],
                    world_verts[face.indices[2]],
                    world_verts[face.indices[3]],
                ];
                let normal = rotate_y_about(face.normal, Vec3::ZERO, spin);
                let centroid = verts.iter().copied().sum::<Vec3>() / 4.0;
                let to_eye = view.eye - centroid;
                if normal.dot(to_eye) <= 0.0 {
                    continue;
                }
                fragments.push(Fragment::Face(RenderFace {
                    verts,
                    normal,
                    distance: to_eye.length(),
                    base,
                    pulse_mult,
                }));
            }
        } else {
            // WIREFRAME PATH: every edge contributes regardless of facing — a
            // transparent cube shows all 12 edges (the silhouette + the
            // interior edges that would be "hidden" on a solid).
            for &(ia, ib) in &CUBE_EDGES {
                let a = world_verts[ia];
                let b = world_verts[ib];
                let midpoint = (a + b) * 0.5;
                fragments.push(Fragment::Edge(RenderEdge {
                    a,
                    b,
                    distance: (view.eye - midpoint).length(),
                    pulse_mult,
                }));
            }
        }
    }

    // CROSS-BOX PAINTER'S SORT: sort the combined pool farthest-first, so near
    // fragments draw LAST and overwrite far ones (correct depth ordering for
    // mixed solid/wireframe — both contribute to the same sort).
    fragments.sort_unstable_by(|a, b| {
        b.distance()
            .partial_cmp(&a.distance())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    if !fragments.is_empty() {
        // Scene-wide fog range across ALL fragments (faces AND edges) so the
        // depth cue is consistent: a far wireframe fogs out the same way a
        // far face does.
        let (near_d, far_d) = fragment_distance_range(&fragments);
        let to_eye_dir = (view.eye - view.target).normalize_or_zero();
        shade_and_fill_fragments(
            &mut hi,
            &projector,
            &fragments,
            to_eye_dir,
            near_d,
            far_d,
            palette,
        );
    }

    // ENT-02 (04-04): port glow pass. Goes AFTER cubes so the bright dots sit
    // ON TOP of the front face (overwriting the cube fill where the dot lands).
    // Emissive: no fog, no Lambert — the dot's color is the palette indigo
    // glow at full brightness regardless of distance or orientation.
    //
    // Color = palette.glow (bright indigo) — deliberately DIFFERENT from the
    // Running face color (light green) so the port dot reads as a port and
    // doesn't blend invisibly into the cube it sits on.
    let glow_color = palette.glow;
    let port_face_axes = [
        (Vec3::Z, Vec3::X, Vec3::Y),     // +Z face (front), tie-break #1
        (Vec3::X, Vec3::NEG_Z, Vec3::Y), // +X face (right), tie-break #2
        (Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y), // -Z face (back), tie-break #3
        (Vec3::NEG_X, Vec3::Z, Vec3::Y), // -X face (left), tie-break #4
    ];
    for entity in entities {
        let Some(ports) = extras.ports.get(&entity.id) else {
            continue;
        };
        if ports.is_empty() {
            continue;
        }
        // Pick camera-facing vertical face. Walk in tie-break order and keep
        // the one whose SPUN normal has the largest positive dot with
        // (eye - entity.center). Deterministic — equal-dot ties resolve to
        // the earlier axis (+Z first).
        let to_eye = view.eye - entity.position;
        let mut best: Option<(usize, f32)> = None;
        for (i, (n, _, _)) in port_face_axes.iter().enumerate() {
            let spun_n = rotate_y_about(*n, Vec3::ZERO, spin);
            let d = spun_n.dot(to_eye);
            if d <= 0.0 {
                continue; // back-facing
            }
            best = match best {
                None => Some((i, d)),
                Some((_, prev_d)) if d > prev_d => Some((i, d)),
                _ => best,
            };
        }
        let Some((face_idx, _)) = best else { continue };
        let (normal, u_axis_local, v_axis_local) = port_face_axes[face_idx];
        // Spin the face's local axes into world.
        let spun_n = rotate_y_about(normal, Vec3::ZERO, spin);
        let spun_u = rotate_y_about(u_axis_local, Vec3::ZERO, spin);
        let spun_v = v_axis_local; // +Y is invariant under Y rotation

        let h_u = match normal {
            v if v.x.abs() > 0.5 => entity.half_extents.z, // ±X face: u runs along Z
            _ => entity.half_extents.x,                    // ±Z face: u runs along X
        };
        let h_v = entity.half_extents.y;
        // Slight outward push so the dot sits ON the face, not embedded in it.
        const FACE_EPS: f32 = 1e-3;
        let h_n = match normal {
            v if v.x.abs() > 0.5 => entity.half_extents.x,
            _ => entity.half_extents.z,
        };
        let face_center = entity.position + spun_n * (h_n + FACE_EPS);

        // Emissive quad half-size. Scaled with the box face so big boxes get
        // proportionally bigger dots, with a generous minimum so the glow is
        // still readable on idle MIN_HALF-sized containers (a small fraction
        // of MIN_HALF=0.3 collapses below the braille majority-coverage
        // threshold). 0.10 world units is ~2 braille dots at the standard
        // framing distance — visible at MIN_HALF without dominating the face,
        // and at MAX_HALF=1.2 the 0.18*face_min term wins so the dot scales
        // proportionally upward.
        let face_min = h_u.min(h_v);
        let dot_half = (0.18 * face_min).max(0.10);

        for (i, _port) in ports.iter().take(9).enumerate() {
            let col = (i % 3) as f32;
            let row = (i / 3) as f32;
            // UV in {0.25, 0.5, 0.75} -> face-local offset in {-0.5, 0, +0.5}.
            let u_off = ((col + 1.0) / 4.0 - 0.5) * 2.0;
            let v_off = ((row + 1.0) / 4.0 - 0.5) * 2.0;
            let port_pos = face_center + spun_u * (u_off * h_u) + spun_v * (v_off * h_v);
            // Build a small quad in the face plane (u/v axes) at the port.
            let verts = [
                port_pos - spun_u * dot_half - spun_v * dot_half,
                port_pos + spun_u * dot_half - spun_v * dot_half,
                port_pos + spun_u * dot_half + spun_v * dot_half,
                port_pos - spun_u * dot_half + spun_v * dot_half,
            ];
            // Emissive: write the bright glow color directly, no shade/fog.
            fill_face(&mut hi, &projector, &verts, glow_color);
        }
    }

    // ENT-03 (04-05): volume cylinders. Drawn AFTER cubes + ports so a
    // cylinder sitting on top of a cube paints over the cube's top face
    // where they overlap. Per-cylinder yaw-to-camera dot sort (Pitfall H)
    // lives inside `rasterize_cylinder` — keeps seams stable as the
    // cylinder rotates.
    for cyl in extras.cylinders {
        crate::render3d::cylinder::rasterize_cylinder(
            &mut hi,
            &projector,
            view.eye,
            cyl.center,
            cyl.radius,
            cyl.height,
            cyl.color,
        );
    }

    // ENT-04 (04-05): image stacks. Drawn LAST so a stack in the side
    // region overwrites any rack fragment that happened to project on
    // top of it (the painter's-sort order is "scene first, side region
    // on top" — the side region is conceptually "in front of" the rack
    // from any orbit angle the user is likely to choose). The per-cube
    // back-face cull inside `rasterize_image_stack` keeps each layer
    // crisp without a z-buffer.
    for stack in extras.image_stacks {
        crate::render3d::stack::rasterize_image_stack(
            &mut hi,
            &projector,
            view.eye,
            stack.base,
            stack.layer_count,
            palette,
        );
    }

    resolve_supersampled(&hi, w, h)
}

/// A culled wireframe edge: two WORLD-SPACE endpoints + camera distance to
/// the edge midpoint (the painter-sort key). Edges have no normal — they only
/// fog by midpoint distance, never Lambert-shaded.
struct RenderEdge {
    a: Vec3,
    b: Vec3,
    distance: f32,
    /// Brightness multiplier for the Tab-selection pulse (1.0 for unselected,
    /// `1 + 0.18 * sin(pulse_phase * TAU)` for the selected entity).
    pulse_mult: f32,
}

/// One drawable fragment in the cross-box painter's pool. A solid box pushes
/// up to 6 `Face` fragments (back-face culled); a wireframe box pushes 12
/// `Edge` fragments. Sorted together by `.distance()` so a near edge correctly
/// overwrites a far face and vice versa.
enum Fragment {
    Face(RenderFace),
    Edge(RenderEdge),
}

impl Fragment {
    fn distance(&self) -> f32 {
        match self {
            Fragment::Face(f) => f.distance,
            Fragment::Edge(e) => e.distance,
        }
    }
}

/// Half-thickness (in SUPERSAMPLED pixels) of wireframe edges in the braille
/// path. SS=3 means the supersample grid is 3× the dot resolution per axis;
/// a `1.6` half-thickness gives ~3 SS-pixels of total line width, enough that
/// a 45° edge passes the majority-coverage threshold in
/// [`resolve_supersampled`] and lights one column/row of dots along the edge.
/// Picked empirically to read as a clean ~1-dot-wide outline without ballooning
/// into a thick blob at scale.
///
/// Crate-visible so [`crate::render3d::plane`] can reuse the same edge
/// thickness for floor-plane wireframes (ENT-01) — keeps the floor edges
/// visually consistent with cube wireframe edges at the same resolution.
pub(crate) const EDGE_HALF_PX_SS: f32 = 1.6;

/// Scene-wide near/far across both face and edge fragments — the fog range.
fn fragment_distance_range(frags: &[Fragment]) -> (f32, f32) {
    let mut near = f32::INFINITY;
    let mut far = f32::NEG_INFINITY;
    for f in frags {
        let d = f.distance();
        near = near.min(d);
        far = far.max(d);
    }
    (near, far)
}

/// Shade and fill each fragment back-to-front. Faces use Lambert×fog; edges
/// use the palette edge color fogged by midpoint distance. Drawing into the
/// SS framebuffer in this order means a near fragment's pixels overwrite a
/// far fragment's pixels (painter occlusion, the existing scheme generalized
/// from "faces only" to "faces + edges").
fn shade_and_fill_fragments(
    hi: &mut Framebuffer,
    projector: &Projector,
    fragments: &[Fragment],
    to_eye_dir: Vec3,
    near_d: f32,
    far_d: f32,
    palette: &Palette,
) {
    for frag in fragments {
        match frag {
            Fragment::Face(rf) => {
                let lambert = rf.normal.dot(to_eye_dir).max(0.0);
                let orient = MIN_LAMBERT + (1.0 - MIN_LAMBERT) * lambert;
                let fog = fog_factor(rf.distance, near_d, far_d);
                let shaded = theme::dim(rf.base, orient);
                let color = palette.fog(shaded, fog);
                // Apply the selection brightness pulse (04-03 / CAM-04): a
                // small ±18% RGB swing on the selected entity, no-op on
                // others. Clamped to [0,255].
                let color = scale_brightness(color, rf.pulse_mult);
                fill_face(hi, projector, &rf.verts, color);
            }
            Fragment::Edge(re) => {
                let fog = fog_factor(re.distance, near_d, far_d);
                let color = palette.fog(palette.edge, fog);
                let color = scale_brightness(color, re.pulse_mult);
                fill_edge(hi, projector, re.a, re.b, color);
            }
        }
    }
}

/// Scale an RGB color's brightness by `mult`, clamped per-channel to `[0, 255]`.
/// Non-RGB colors pass through unchanged (the palette resolves everything to
/// `Color::Rgb` in this crate). `mult` near 1.0 leaves the color visually
/// untouched; the selection-pulse amplitude (0.18) keeps the swing readable
/// without ever saturating to white.
fn scale_brightness(color: Color, mult: f32) -> Color {
    if (mult - 1.0).abs() < 1e-6 {
        return color;
    }
    if let Color::Rgb(r, g, b) = color {
        let m = mult.max(0.0);
        let r = ((r as f32 * m).round()).clamp(0.0, 255.0) as u8;
        let g = ((g as f32 * m).round()).clamp(0.0, 255.0) as u8;
        let b = ((b as f32 * m).round()).clamp(0.0, 255.0) as u8;
        Color::Rgb(r, g, b)
    } else {
        color
    }
}

/// Project both endpoints of a 3D edge and stripe a thick line into the
/// framebuffer. Skips the edge if either endpoint clips off-screen (rare:
/// scene-framed cameras keep the whole rack in the frustum).
fn fill_edge(fb: &mut Framebuffer, projector: &Projector, a: Vec3, b: Vec3, color: Color) {
    let pa = projector.project(a);
    let pb = projector.project(b);
    let (Some((ax, ay, _)), Some((bx, by, _))) = (pa, pb) else {
        return;
    };
    draw_thick_line(fb, (ax, ay), (bx, by), EDGE_HALF_PX_SS, color);
}

/// Rasterize a screen-space line `a` → `b` as a rectangular band of half-width
/// `half_px`. Iterates the segment's AABB inflated by thickness; lights every
/// framebuffer pixel whose center is inside the band (perpendicular distance
/// to the segment ≤ `half_px`, with `t` clamped to `[0,1]` so caps are flat
/// rather than rounded). Out-of-range writes pass silently through
/// [`Framebuffer::set`] — the resize/clip safety carries through.
///
/// Crate-visible so [`crate::render3d::plane::rasterize_floor_plane`] can
/// share the same band-rasterizer for its 4 wireframe edges.
pub(crate) fn draw_thick_line(fb: &mut Framebuffer, a: (f32, f32), b: (f32, f32), half_px: f32, color: Color) {
    let (ax, ay) = a;
    let (bx, by) = b;
    let dx = bx - ax;
    let dy = by - ay;
    let len2 = dx * dx + dy * dy;
    if len2 <= f32::EPSILON {
        // Degenerate (start == end on screen): plant a single dot.
        fb.set(ax.round() as usize, ay.round() as usize, color);
        return;
    }
    let inv_len2 = 1.0 / len2;
    let (w, h) = (fb.width() as f32, fb.height() as f32);
    let pad = half_px + 1.0;
    let min_x = (ax.min(bx) - pad).floor().max(0.0) as i32;
    let max_x = (ax.max(bx) + pad).ceil().min(w - 1.0) as i32;
    let min_y = (ay.min(by) - pad).floor().max(0.0) as i32;
    let max_y = (ay.max(by) + pad).ceil().min(h - 1.0) as i32;
    if min_x > max_x || min_y > max_y {
        return;
    }
    let half_sq = half_px * half_px;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let t = ((px - ax) * dx + (py - ay) * dy) * inv_len2;
            if !(0.0..=1.0).contains(&t) {
                continue;
            }
            let cx = ax + t * dx;
            let cy = ay + t * dy;
            let ddx = px - cx;
            let ddy = py - cy;
            if ddx * ddx + ddy * ddy <= half_sq {
                fb.set(x as usize, y as usize, color);
            }
        }
    }
}

/// Shade each visible face (orientation Lambert × distance fog, per-face `base`
/// color) and fill it into the supersampled buffer. Shared by [`render`] and
/// [`render_scene`] so both paths shade identically.
fn shade_and_fill(
    hi: &mut Framebuffer,
    projector: &Projector,
    visible: &[RenderFace],
    to_eye_dir: Vec3,
    near_d: f32,
    far_d: f32,
    palette: &Palette,
) {
    for rf in visible {
        // SHADING (REND-03):
        //   (a) orientation — brighter when the normal faces the camera.
        //   (b) distance fog — farther faces blended toward the background.
        let lambert = rf.normal.dot(to_eye_dir).max(0.0);
        let orient = MIN_LAMBERT + (1.0 - MIN_LAMBERT) * lambert;
        let fog = fog_factor(rf.distance, near_d, far_d);
        // Orientation dims toward black (shading); fog blends toward background.
        let shaded = theme::dim(rf.base, orient);
        let color = palette.fog(shaded, fog);

        fill_face(hi, projector, &rf.verts, color);
    }
}

/// Downsample the `SS`×-supersampled buffer `hi` into the final braille-res
/// framebuffer.
///
/// A dot is lit only when the shape covers the MAJORITY of its area
/// (`2 * covered >= SS*SS`). This drops faint, low-coverage edge sub-samples that
/// would otherwise render as detached dim "dribble" specks below/right of the
/// body, and keeps a sub-dot-accurate, stable silhouette.
///
/// The lit dot takes the color of the DOMINANT face among its covered sub-samples
/// (the most frequent color — each face is one flat color), NOT a blend. This
/// keeps every wall a single uniform color with a crisp 1-dot boundary against the
/// next face — no darker band along a face's leading edge where it meets a darker
/// neighbour.
fn resolve_supersampled(hi: &Framebuffer, w: usize, h: usize) -> Framebuffer {
    let mut fb = Framebuffer::new(w, h);
    let n = (SS * SS) as u32;

    for y in 0..h {
        for x in 0..w {
            // Tally covered sub-sample colors (at most SS*SS distinct).
            let mut tally: Vec<(Color, u32)> = Vec::new();
            let mut covered = 0u32;
            for sy in 0..SS {
                for sx in 0..SS {
                    if let Some(c) = hi.get(x * SS + sx, y * SS + sy) {
                        covered += 1;
                        match tally.iter_mut().find(|(col, _)| *col == c) {
                            Some(entry) => entry.1 += 1,
                            None => tally.push((c, 1)),
                        }
                    }
                }
            }
            // Majority-coverage threshold: skip dots the shape barely touches.
            if 2 * covered >= n {
                // Dominant face color (mode) — flat walls, crisp face boundaries.
                let color = tally
                    .iter()
                    .max_by_key(|(_, count)| *count)
                    .map(|(c, _)| *c)
                    .expect("covered > 0 implies a tallied color");
                fb.set(x, y, color);
            }
        }
    }
    fb
}

/// A culled face carrying the precomputed data the fill loop needs.
///
/// Carries the face's OWN 4 WORLD-SPACE vertices (not indices into a shared
/// cube): with many boxes there is no single shared cube to index, and the
/// cross-box painter's sort must carry each face's own world geometry. `base` is
/// the per-box status color resolved by the palette.
struct RenderFace {
    verts: [Vec3; 4],
    normal: Vec3,
    distance: f32,
    base: Color,
    /// Brightness multiplier for the Tab-selection pulse (1.0 for unselected,
    /// `1 + 0.18 * sin(pulse_phase * TAU)` for the selected entity). Defaults
    /// to 1.0 in the legacy single-cube `render` path (no selection there).
    pulse_mult: f32,
}

/// Min/max camera distance across the visible faces (the fog range).
fn distance_range(faces: &[RenderFace]) -> (f32, f32) {
    let mut near = f32::INFINITY;
    let mut far = f32::NEG_INFINITY;
    for f in faces {
        near = near.min(f.distance);
        far = far.max(f.distance);
    }
    (near, far)
}

/// Map a face distance to a fog brightness factor in `[FOG_MIN, 1.0]`: the
/// nearest face keeps `1.0`, the farthest dims to `FOG_MIN`. Degenerate ranges
/// (all faces equidistant) return `1.0`.
fn fog_factor(distance: f32, near: f32, far: f32) -> f32 {
    // Weaker fog: the farthest visible face only dims to 0.7 of full brightness
    // (was 0.45) so far faces stay clearly colored. Enough near/far gap remains
    // to keep depth readable without driving distant faces toward background.
    const FOG_MIN: f32 = 0.7;
    let span = far - near;
    if span <= f32::EPSILON {
        return 1.0;
    }
    let t = ((distance - near) / span).clamp(0.0, 1.0);
    1.0 - t * (1.0 - FOG_MIN)
}

/// Project a quad's 4 WORLD-SPACE corners and fill it (as two triangles) into the
/// framebuffer with `color`. Takes the world verts directly (not a shared cube +
/// indices) so per-box geometry reaches the fill in the multi-box path. If any
/// corner clips off-screen the face is skipped (the scene-framing camera keeps
/// boxes in-frustum, so this is rare).
///
/// `pub(crate)` so the cylinder primitive (`render3d::cylinder`) and the stack
/// primitive (`render3d::stack`, via cube reuse) can share the same fill path
/// — keeps every braille surface going through the SAME triangle fill +
/// fb.set so coverage / majority-color resolve stays unified.
pub(crate) fn fill_face(fb: &mut Framebuffer, projector: &Projector, verts: &[Vec3; 4], color: Color) {
    let mut pts = [(0.0f32, 0.0f32); 4];
    for (slot, &v) in pts.iter_mut().zip(verts.iter()) {
        match projector.project(v) {
            Some((x, y, _depth)) => *slot = (x, y),
            None => return, // any vertex clips -> skip face
        }
    }
    // Quad [0,1,2,3] -> triangles (0,1,2) and (0,2,3).
    fill_triangle(fb, pts[0], pts[1], pts[2], color);
    fill_triangle(fb, pts[0], pts[2], pts[3], color);
}

/// Barycentric triangle fill into the framebuffer. Iterates the integer pixel
/// bounding box (clamped to the framebuffer) and lights pixels whose center lies
/// inside the triangle. Degenerate (zero-area) triangles light nothing.
///
/// `pub(crate)` so `render3d::cylinder` can use the same fill for its octagon
/// cap fan-triangulation — keeps every braille surface going through the
/// SAME edge-function rasterizer and EDGE_EPS seam tolerance.
pub(crate) fn fill_triangle(
    fb: &mut Framebuffer,
    a: (f32, f32),
    b: (f32, f32),
    c: (f32, f32),
    color: Color,
) {
    let (w, h) = (fb.width() as f32, fb.height() as f32);

    // Bounding box, clamped to the framebuffer.
    let min_x = a.0.min(b.0).min(c.0).floor().max(0.0);
    let max_x = a.0.max(b.0).max(c.0).ceil().min(w - 1.0);
    let min_y = a.1.min(b.1).min(c.1).floor().max(0.0);
    let max_y = a.1.max(b.1).max(c.1).ceil().min(h - 1.0);
    if min_x > max_x || min_y > max_y {
        return;
    }

    // Signed area * 2 (edge function denominator). Guard degeneracy / NaN.
    let area2 = edge(a, b, c);
    if area2.abs() < f32::EPSILON || !area2.is_finite() {
        return;
    }
    let inv = 1.0 / area2;

    let mut y = min_y as i32;
    let y_end = max_y as i32;
    while y <= y_end {
        let mut x = min_x as i32;
        let x_end = max_x as i32;
        while x <= x_end {
            let p = (x as f32 + 0.5, y as f32 + 0.5);
            // Barycentric weights via edge functions.
            let w0 = edge(b, c, p) * inv;
            let w1 = edge(c, a, p) * inv;
            let w2 = edge(a, b, p) * inv;
            // Inside if all weights have the same sign as the triangle area
            // (allow tiny negative epsilon to avoid seams between the two tris).
            if w0 >= -EDGE_EPS && w1 >= -EDGE_EPS && w2 >= -EDGE_EPS {
                fb.set(x as usize, y as usize, color);
            }
            x += 1;
        }
        y += 1;
    }
}

/// Edge-function epsilon: tolerate sub-pixel negatives so the shared diagonal of
/// the two triangles fills without a 1px seam.
const EDGE_EPS: f32 = 1e-3;

/// 2D edge function: twice the signed area of triangle (a, b, p). Positive when
/// `p` is left of the directed edge a→b (for our screen winding).
fn edge(a: (f32, f32), b: (f32, f32), p: (f32, f32)) -> f32 {
    (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render3d::cube::unit_cube;

    const VIEWPORT: (usize, usize) = (64, 64);

    /// A camera off the +X/+Y/+Z octant looking at the origin, so THREE faces
    /// (front, right, top) are visible — exercises cull, sort, and shading.
    fn corner_view() -> ViewParams {
        ViewParams {
            eye: Vec3::new(3.0, 2.5, 4.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov: std::f32::consts::FRAC_PI_3,
        }
    }

    fn lit(fb: &Framebuffer) -> Vec<(usize, usize, Color)> {
        fb.lit_pixels().collect()
    }

    /// Empty SceneExtras factory: borrows of empty slices/maps. The cube tests
    /// don't exercise floor-planes, port glow, volume cylinders, or image
    /// stacks — those have their own focused suites in `render3d::plane`,
    /// `render3d::cylinder`, `render3d::stack`, and the per-backend visual
    /// gates.
    fn empty_extras<'a>(
        floors: &'a [crate::render3d::scene_extras::FloorPlane],
        ports: &'a crate::render3d::scene_extras::PortLookup<'a>,
    ) -> SceneExtras<'a> {
        SceneExtras::new(floors, ports, &[], &[])
    }

    #[test]
    fn renders_nontrivial_pixels_near_center() {
        let cube = unit_cube();
        let pal = Palette::default();
        let cfg = RenderConfig::default();
        let fb = render(&cube, corner_view(), VIEWPORT, &pal, &cfg);

        let pixels = lit(&fb);
        assert!(
            pixels.len() > 100,
            "expected a solid filled cube, got only {} lit pixels",
            pixels.len()
        );

        // Footprint centroid should sit near the viewport center.
        let (sx, sy) = pixels.iter().fold((0usize, 0usize), |(ax, ay), &(x, y, _)| {
            (ax + x, ay + y)
        });
        let cx = sx as f32 / pixels.len() as f32;
        let cy = sy as f32 / pixels.len() as f32;
        assert!((cx - 32.0).abs() < 12.0, "footprint x off-center: {cx}");
        assert!((cy - 32.0).abs() < 12.0, "footprint y off-center: {cy}");
    }

    #[test]
    fn shading_varies_color_across_faces() {
        // (b) NOT all lit pixels share one color — proves depth/orientation
        // shading actually modulates the base color.
        let cube = unit_cube();
        let pal = Palette::default();
        let cfg = RenderConfig::default();
        let fb = render(&cube, corner_view(), VIEWPORT, &pal, &cfg);

        let distinct: std::collections::HashSet<_> =
            fb.lit_pixels().map(|(_, _, c)| c).collect();
        assert!(
            distinct.len() >= 2,
            "expected ≥2 distinct shaded colors, got {}",
            distinct.len()
        );
    }

    #[test]
    fn footprint_reads_cubic_at_subpixel_level() {
        // (c) Aspect carried through from plan 03. With a SQUARE viewport,
        // the only source of footprint asymmetry is the projector's
        // `cell_aspect` term, so the rasterized cube's bounding box must
        // satisfy `bbox_w ≈ cell_aspect * bbox_h`. RV4 (05-06): the default
        // `cell_aspect` is 1.0 (typical-cell parity) so this collapses to
        // `bbox_w ≈ bbox_h`. Pre-RV4 the default was 2.0 (asserted a 2:1
        // horizontal stretch from a braille-dot 1:2 cell-aspect bias).
        let cube = unit_cube();
        let pal = Palette::default();
        let cfg = RenderConfig::default();
        let fb = render(&cube, corner_view(), VIEWPORT, &pal, &cfg);

        let (mut min_x, mut max_x, mut min_y, mut max_y) =
            (usize::MAX, 0usize, usize::MAX, 0usize);
        for (x, y, _) in fb.lit_pixels() {
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
        }
        let bw = (max_x - min_x) as f32;
        let bh = (max_y - min_y) as f32;
        let expected_w = bh * cfg.cell_aspect;
        let rel_err = (bw - expected_w).abs() / expected_w;
        assert!(
            rel_err < 0.2,
            "rasterized cube not cubic: w={bw}, h={bh}, cell_aspect={}, \
             expected_w≈{expected_w}, rel_err={rel_err}",
            cfg.cell_aspect
        );
    }

    #[test]
    fn occlusion_center_pixel_is_near_face() {
        // (d) REND-02 occlusion proof. Look straight down +Z at the front face.
        // The +Z (front) face is nearest; the -Z (back) face is occluded. With a
        // head-on view only the front face survives the cull, and its Lambert is
        // ~1.0 (normal == to-eye dir) while every other face is culled — so the
        // center pixel must resolve to the brightest shade of the running color.
        let cube = unit_cube();
        let pal = Palette::default();
        let cfg = RenderConfig::default();
        let view = ViewParams {
            eye: Vec3::new(0.0, 0.0, 4.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov: std::f32::consts::FRAC_PI_3,
        };
        let fb = render(&cube, view, VIEWPORT, &pal, &cfg);

        let center = fb
            .get(VIEWPORT.0 / 2, VIEWPORT.1 / 2)
            .expect("center pixel must be lit by the front face");

        // The front face faces the camera dead-on: lambert ~1, single visible
        // face so fog factor == 1.0 -> the color is the (near-) full base color.
        let base = pal.status_color(Status::Running);
        assert_eq!(
            center, base,
            "center pixel should be the near (front) face base color, got {center:?}"
        );
        // And it must NOT be the background (a missed/occluded far face).
        assert_ne!(center, pal.background);
    }

    #[test]
    fn painters_sort_orders_farthest_first() {
        // Unit-test the sort step in isolation: feed faces with known distances
        // and assert strictly farthest-first ordering after the sort.
        let v = [Vec3::ZERO; 4];
        let base = Palette::default().status_color(Status::Running);
        let mut faces = [
            RenderFace { verts: v, normal: Vec3::Z, distance: 1.0, base, pulse_mult: 1.0 },
            RenderFace { verts: v, normal: Vec3::Z, distance: 5.0, base, pulse_mult: 1.0 },
            RenderFace { verts: v, normal: Vec3::Z, distance: 3.0, base, pulse_mult: 1.0 },
        ];
        faces.sort_unstable_by(|a, b| {
            b.distance
                .partial_cmp(&a.distance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let order: Vec<f32> = faces.iter().map(|f| f.distance).collect();
        assert_eq!(order, vec![5.0, 3.0, 1.0], "must be farthest-first");
    }

    #[test]
    fn back_faces_are_culled() {
        // Head-on view of the front face: of 6 faces, only the +Z face survives
        // the cull (the other 5 point away or edge-on). We verify indirectly:
        // the lit footprint is a single filled square (one face), so its area is
        // close to the bounding-box area (no multi-face silhouette overhang).
        let cube = unit_cube();
        let pal = Palette::default();
        let cfg = RenderConfig::default();
        let view = ViewParams {
            eye: Vec3::new(0.0, 0.0, 4.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov: std::f32::consts::FRAC_PI_3,
        };
        let fb = render(&cube, view, VIEWPORT, &pal, &cfg);

        let (mut min_x, mut max_x, mut min_y, mut max_y) =
            (usize::MAX, 0usize, usize::MAX, 0usize);
        let mut count = 0usize;
        for (x, y, _) in fb.lit_pixels() {
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
            count += 1;
        }
        let bbox_area = (max_x - min_x + 1) * (max_y - min_y + 1);
        let fill_ratio = count as f32 / bbox_area as f32;
        assert!(
            fill_ratio > 0.9,
            "single front face should fill its bbox densely; ratio={fill_ratio}"
        );
    }

    #[test]
    fn faces_stay_flat_no_seam_blend() {
        // Each wall must be ONE uniform color — no darker band along a face's
        // leading edge where it meets a darker neighbour. The AA resolve picks the
        // DOMINANT face color per dot (mode), never a blend, so the 3/4
        // `corner_view`, which shows exactly three faces, must yield at most 3
        // distinct colors. (≥2 confirms the faces are still differently shaded.)
        let cube = unit_cube();
        let pal = Palette::default();
        let cfg = RenderConfig::default();
        let fb = render(&cube, corner_view(), VIEWPORT, &pal, &cfg);

        let distinct: std::collections::HashSet<_> =
            fb.lit_pixels().map(|(_, _, c)| c).collect();
        assert!(
            (2..=3).contains(&distinct.len()),
            "faces must stay flat (one color each, 3 visible faces), got {}",
            distinct.len()
        );
    }

    #[test]
    fn does_not_panic_on_degenerate_view() {
        // Eye == target (zero view direction) must not panic — guard NaN paths.
        let cube = unit_cube();
        let pal = Palette::default();
        let cfg = RenderConfig::default();
        let view = ViewParams {
            eye: Vec3::ZERO,
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov: std::f32::consts::FRAC_PI_3,
        };
        let _fb = render(&cube, view, VIEWPORT, &pal, &cfg);
    }

    // ---- render_scene (multi-box) tests ----------------------------------

    /// Build one entity with a given status at `position`, sized by `half`.
    fn entity_at(id: u32, position: Vec3, half: f32, status: Status) -> Entity {
        Entity {
            id,
            position,
            half_extents: Vec3::splat(half),
            status,
            group: 0,
        }
    }

    /// A head-on view down +Z at the origin, so the front (+Z) face of a box at
    /// the origin fills the viewport center.
    fn head_on_view() -> ViewParams {
        ViewParams {
            eye: Vec3::new(0.0, 0.0, 12.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov: std::f32::consts::FRAC_PI_3,
        }
    }

    #[test]
    fn render_scene_two_box_occlusion_near_hides_far() {
        // Box A (near, in front along +Z) must occlude box B (far, behind) at the
        // overlapping center — proving the cross-box sort draws the near box last.
        // Give them DIFFERENT statuses so the resolved color identifies which box
        // won the center pixel.
        let pal = Palette::default();
        let cfg = RenderConfig::default();
        // A near the camera (+Z), B further back (-Z), both centered on the view
        // axis so their footprints overlap at the viewport center.
        let near = entity_at(0, Vec3::new(0.0, 0.0, 2.0), 0.6, Status::Running);
        let far = entity_at(1, Vec3::new(0.0, 0.0, -2.0), 0.6, Status::Crashed);
        let entities = [near, far];

        let floors: Vec<crate::render3d::scene_extras::FloorPlane> = Vec::new();
        let ports: crate::render3d::scene_extras::PortLookup = Default::default();
        let extras = empty_extras(&floors, &ports);
        let fb = render_scene(&entities, head_on_view(), VIEWPORT, &pal, &cfg, 0.0, None, 0.0, &extras, false);

        let center = fb
            .get(VIEWPORT.0 / 2, VIEWPORT.1 / 2)
            .expect("center pixel must be lit by the near box front face");
        // Near box's front face is head-on (lambert ~1) and nearest (fog ~1), so
        // the center resolves to the running base color, NOT the crashed color.
        let near_base = pal.status_color(Status::Running);
        let far_base = pal.status_color(Status::Crashed);
        assert_eq!(center, near_base, "center should be the NEAR box color");
        assert_ne!(center, far_base, "far box must not bleed through the near box");
    }

    #[test]
    fn render_scene_at_scale_grid_near_hides_far() {
        // AT-SCALE ordering guard: a hand-built grid where MANY faces from MANY
        // boxes compete, with a KNOWN near box overlapping a KNOWN far box at the
        // viewport center along the view axis. The center pixel must resolve to
        // the NEAR box's color — proving the SINGLE combined cross-box sort orders
        // correctly when the pool is large, not just in the trivial 2-box case.
        let pal = Palette::default();
        let cfg = RenderConfig::default();

        // A 3x3 grid in the X/Y plane at z = -3 (far), plus one near box at the
        // center on the view axis at z = +2. The grid gives a crowded face pool;
        // the near box must win the center despite all those competing faces.
        let mut entities = Vec::new();
        let mut id = 0u32;
        for gy in -1..=1 {
            for gx in -1..=1 {
                entities.push(entity_at(
                    id,
                    Vec3::new(gx as f32 * 2.5, gy as f32 * 2.5, -3.0),
                    0.6,
                    Status::Crashed, // far grid = crashed (red)
                ));
                id += 1;
            }
        }
        // The known near box on the view axis, a distinct status.
        let near = entity_at(id, Vec3::new(0.0, 0.0, 2.0), 0.6, Status::Running);
        entities.push(near);

        let floors: Vec<crate::render3d::scene_extras::FloorPlane> = Vec::new();
        let ports: crate::render3d::scene_extras::PortLookup = Default::default();
        let extras = empty_extras(&floors, &ports);
        let fb = render_scene(&entities, head_on_view(), VIEWPORT, &pal, &cfg, 0.0, None, 0.0, &extras, false);

        let center = fb
            .get(VIEWPORT.0 / 2, VIEWPORT.1 / 2)
            .expect("center pixel must be lit by the near box");
        let near_base = pal.status_color(Status::Running);
        assert_eq!(
            center, near_base,
            "near box must occlude the far grid box at center (got {center:?})"
        );
    }

    #[test]
    fn render_scene_applies_per_box_status_color() {
        // Two boxes with different statuses, placed side by side (no overlap), must
        // yield >= 2 distinct colors — proving per-box status color is applied.
        let pal = Palette::default();
        let cfg = RenderConfig::default();
        let a = entity_at(0, Vec3::new(-2.0, 0.0, 0.0), 0.6, Status::Running);
        let b = entity_at(1, Vec3::new(2.0, 0.0, 0.0), 0.6, Status::Crashed);
        let entities = [a, b];

        let floors: Vec<crate::render3d::scene_extras::FloorPlane> = Vec::new();
        let ports: crate::render3d::scene_extras::PortLookup = Default::default();
        let extras = empty_extras(&floors, &ports);
        let fb = render_scene(&entities, head_on_view(), VIEWPORT, &pal, &cfg, 0.0, None, 0.0, &extras, false);

        let distinct: std::collections::HashSet<_> =
            fb.lit_pixels().map(|(_, _, c)| c).collect();
        assert!(
            distinct.len() >= 2,
            "expected >= 2 distinct per-box status colors, got {}",
            distinct.len()
        );
    }

    #[test]
    fn render_scene_empty_is_all_unlit_and_does_not_panic() {
        let pal = Palette::default();
        let cfg = RenderConfig::default();
        let floors: Vec<crate::render3d::scene_extras::FloorPlane> = Vec::new();
        let ports: crate::render3d::scene_extras::PortLookup = Default::default();
        let extras = empty_extras(&floors, &ports);
        let fb = render_scene(&[], head_on_view(), VIEWPORT, &pal, &cfg, 0.0, None, 0.0, &extras, false);
        assert_eq!(fb.lit_pixels().count(), 0, "empty scene must be all unlit");
    }

    #[test]
    fn render_scene_off_center_box_lands_in_framebuffer() {
        // A box well off the scene center still lands inside the framebuffer when
        // framed by a scene-radius view (built from a literal pulled-back eye).
        let pal = Palette::default();
        let cfg = RenderConfig::default();
        let off = entity_at(0, Vec3::new(4.0, 0.0, 0.0), 0.6, Status::Running);
        let view = ViewParams {
            eye: Vec3::new(4.0, 2.5, 6.0),
            target: Vec3::new(4.0, 0.0, 0.0),
            up: Vec3::Y,
            fov: std::f32::consts::FRAC_PI_3,
        };
        let floors: Vec<crate::render3d::scene_extras::FloorPlane> = Vec::new();
        let ports: crate::render3d::scene_extras::PortLookup = Default::default();
        let extras = empty_extras(&floors, &ports);
        let fb = render_scene(&[off], view, VIEWPORT, &pal, &cfg, 0.0, None, 0.0, &extras, false);
        assert!(
            fb.lit_pixels().count() > 50,
            "off-center box should rasterize a solid footprint when framed, got {}",
            fb.lit_pixels().count()
        );
    }

    // ---- 05-06 RV5: force_solid pin -------------------------------------

    /// RV5 contract: with `force_solid=true`, a non-Running (Crashed) box
    /// renders as a SOLID face footprint instead of the 12-edge wireframe.
    /// Verify by comparing pixel counts: the solid path rasterizes a
    /// contiguous front face (hundreds of dots), the wireframe path
    /// rasterizes only thin lines along the 12 edges (tens of dots, but
    /// always strictly fewer than the filled face). User feedback "убрать
    /// прозрачность блоков" — this is the pin that catches a future revert
    /// of the `force_solid` plumbing.
    #[test]
    fn render_scene_force_solid_renders_non_running_as_face() {
        let pal = Palette::default();
        let cfg = RenderConfig::default();
        // A Crashed box (non-Running -> wireframe by default) centered on
        // the view axis with the head-on camera.
        let crashed = entity_at(0, Vec3::ZERO, 0.6, Status::Crashed);
        let floors: Vec<crate::render3d::scene_extras::FloorPlane> = Vec::new();
        let ports: crate::render3d::scene_extras::PortLookup = Default::default();
        let extras = empty_extras(&floors, &ports);

        // Wireframe path (force_solid=false): only edges contribute.
        let fb_wire = render_scene(
            &[crashed],
            head_on_view(),
            VIEWPORT,
            &pal,
            &cfg,
            0.0,
            None,
            0.0,
            &extras,
            false,
        );
        let wire_count = fb_wire.lit_pixels().count();

        // Solid path (force_solid=true): the front face fills a quad.
        let fb_solid = render_scene(
            &[crashed],
            head_on_view(),
            VIEWPORT,
            &pal,
            &cfg,
            0.0,
            None,
            0.0,
            &extras,
            true,
        );
        let solid_count = fb_solid.lit_pixels().count();

        // Solid path fills a contiguous face footprint while the wireframe
        // path lights only the box's projected edge pixels. The solid count
        // MUST exceed the wireframe count by a meaningful margin (head-on
        // view at 0 spin: wireframe ~20 edge dots vs solid ~36 face dots
        // at VIEWPORT=64×64). Pin the > inequality + an absolute margin
        // (+8) so a future "force_solid renders fewer fragments" regression
        // can't sneak through under round-off.
        assert!(
            solid_count > wire_count + 8,
            "force_solid must render a filled face footprint with strictly more pixels than the wireframe edges; wireframe={wire_count}, solid={solid_count}",
        );
        // And: the solid render uses the Crashed status color (the entity's
        // own status; force_solid does NOT reskin to Running).
        let crashed_base = pal.status_color(Status::Crashed);
        let has_crashed_pixel = fb_solid
            .lit_pixels()
            .any(|(_, _, c)| c == crashed_base);
        assert!(
            has_crashed_pixel,
            "force_solid must preserve the entity's own status color (Crashed) — not reskin to Running",
        );
    }

    /// RV5 contract: `force_solid=true` does NOT change the rendering of a
    /// Running box (already solid). Pin so a future refactor doesn't
    /// accidentally double-process Running entities.
    #[test]
    fn render_scene_force_solid_running_unchanged() {
        let pal = Palette::default();
        let cfg = RenderConfig::default();
        let running = entity_at(0, Vec3::ZERO, 0.6, Status::Running);
        let floors: Vec<crate::render3d::scene_extras::FloorPlane> = Vec::new();
        let ports: crate::render3d::scene_extras::PortLookup = Default::default();
        let extras = empty_extras(&floors, &ports);

        let fb_normal = render_scene(
            &[running],
            head_on_view(),
            VIEWPORT,
            &pal,
            &cfg,
            0.0,
            None,
            0.0,
            &extras,
            false,
        );
        let fb_forced = render_scene(
            &[running],
            head_on_view(),
            VIEWPORT,
            &pal,
            &cfg,
            0.0,
            None,
            0.0,
            &extras,
            true,
        );
        // Pixel-identical: a Running box is solid on both paths.
        let normal_pixels: Vec<_> = fb_normal.lit_pixels().collect();
        let forced_pixels: Vec<_> = fb_forced.lit_pixels().collect();
        assert_eq!(
            normal_pixels, forced_pixels,
            "force_solid must be a no-op for an already-Running entity (got differing pixel sets)",
        );
    }
}
