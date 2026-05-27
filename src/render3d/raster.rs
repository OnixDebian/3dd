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
use crate::render3d::cube::{unit_cube, Cube};
use crate::render3d::framebuffer::Framebuffer;
use crate::render3d::project::Projector;
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
pub fn render_scene(
    entities: &[Entity],
    view: ViewParams,
    viewport: (usize, usize),
    palette: &Palette,
    config: &RenderConfig,
    spin: f32,
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

    // The shared unit-cube topology (indices + outward normals); each entity
    // materializes its OWN 8 world verts from these.
    let cube = unit_cube();

    // ONE combined face list spanning EVERY box (cross-box painter's pool).
    let mut visible: Vec<RenderFace> = Vec::new();
    for entity in entities {
        let base = palette.status_color(entity.status);
        // Transform the unit cube into world space ONCE per box: scale by the
        // side length (2 * half_extents), translate to the world position, then
        // spin about THIS box's center around +Y by `spin` (self-rotation).
        let scale = entity.half_extents * 2.0;
        let mut world_verts = [Vec3::ZERO; 8];
        for (slot, &v) in world_verts.iter_mut().zip(cube.vertices.iter()) {
            let placed = entity.position + v * scale;
            *slot = rotate_y_about(placed, entity.position, spin);
        }

        for face in &cube.faces {
            let verts = [
                world_verts[face.indices[0]],
                world_verts[face.indices[1]],
                world_verts[face.indices[2]],
                world_verts[face.indices[3]],
            ];
            // The box is now spun, so the unit-cube normal is NO LONGER the world
            // normal — rotate it about the origin (a pure direction) by the same
            // spin so cull and Lambert shading use the true world-space normal.
            let normal = rotate_y_about(face.normal, Vec3::ZERO, spin);
            let centroid = verts.iter().copied().sum::<Vec3>() / 4.0;
            let to_eye = view.eye - centroid;
            // Back-face cull per face against the spun normal.
            if normal.dot(to_eye) <= 0.0 {
                continue;
            }
            visible.push(RenderFace {
                verts,
                normal,
                distance: to_eye.length(),
                base,
            });
        }
    }

    if visible.is_empty() {
        return Framebuffer::new(w, h);
    }

    // CROSS-BOX PAINTER'S SORT (the critical correctness step): sort the SINGLE
    // combined face pool farthest-first, so a near box's faces are drawn LAST and
    // overwrite the far ones. Do NOT sort per-box then concatenate.
    visible.sort_unstable_by(|a, b| {
        b.distance
            .partial_cmp(&a.distance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Scene-wide fog range: near/far across ALL visible faces of ALL boxes, so
    // far boxes dim and near boxes stay bright (depth reads at scale).
    let (near_d, far_d) = distance_range(&visible);

    let to_eye_dir = (view.eye - view.target).normalize_or_zero();

    shade_and_fill(&mut hi, &projector, &visible, to_eye_dir, near_d, far_d, palette);

    resolve_supersampled(&hi, w, h)
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
fn fill_face(fb: &mut Framebuffer, projector: &Projector, verts: &[Vec3; 4], color: Color) {
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
fn fill_triangle(
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
        // (c) Aspect carried through from plan 03. A braille dot is ~1:2 (twice
        // as tall as wide), so a cube that READS as cubic on screen must occupy
        // `cell_aspect` times as many SUB-PIXEL columns as rows. We therefore
        // assert `bbox_w ≈ cell_aspect * bbox_h` (the same invariant plan 03's
        // `unit_cube_footprint_is_cubic` pins), confirming the rasterized
        // footprint isn't squashed/stretched.
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
            RenderFace { verts: v, normal: Vec3::Z, distance: 1.0, base },
            RenderFace { verts: v, normal: Vec3::Z, distance: 5.0, base },
            RenderFace { verts: v, normal: Vec3::Z, distance: 3.0, base },
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

        let fb = render_scene(&entities, head_on_view(), VIEWPORT, &pal, &cfg, 0.0);

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

        let fb = render_scene(&entities, head_on_view(), VIEWPORT, &pal, &cfg, 0.0);

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

        let fb = render_scene(&entities, head_on_view(), VIEWPORT, &pal, &cfg, 0.0);

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
        let fb = render_scene(&[], head_on_view(), VIEWPORT, &pal, &cfg, 0.0);
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
        let fb = render_scene(&[off], view, VIEWPORT, &pal, &cfg, 0.0);
        assert!(
            fb.lit_pixels().count() > 50,
            "off-center box should rasterize a solid footprint when framed, got {}",
            fb.lit_pixels().count()
        );
    }
}
