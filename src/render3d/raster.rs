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
use crate::render3d::cube::Cube;
use crate::render3d::framebuffer::Framebuffer;
use crate::render3d::project::Projector;
use crate::render3d::ViewParams;
use crate::theme::{self, Palette, Status};

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

    // World-space centroid + distance for cull, sort, and fog.
    let mut visible: Vec<RenderFace> = cube
        .faces
        .iter()
        .filter_map(|face| {
            let centroid = face_centroid(cube, &face.indices);
            let to_eye = view.eye - centroid;
            // Back-face cull: keep only faces whose outward normal faces the eye.
            if face.normal.dot(to_eye) <= 0.0 {
                return None;
            }
            Some(RenderFace {
                indices: face.indices,
                normal: face.normal,
                distance: to_eye.length(),
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

    // Synthetic cube: base color is the "running/alive" status color (a palette
    // lookup, never an inline RGB).
    let base = palette.status_color(Status::Running);

    let to_eye_dir = (view.eye - view.target).normalize_or_zero();

    for rf in &visible {
        // SHADING (REND-03):
        //   (a) orientation — brighter when the normal faces the camera.
        //   (b) distance fog — farther faces blended toward the background.
        let lambert = rf.normal.dot(to_eye_dir).max(0.0);
        let orient = MIN_LAMBERT + (1.0 - MIN_LAMBERT) * lambert;
        let fog = fog_factor(rf.distance, near_d, far_d);
        // Orientation dims toward black (shading); fog blends toward background.
        let shaded = theme::dim(base, orient);
        let color = palette.fog(shaded, fog);

        fill_face(&mut hi, &projector, cube, &rf.indices, color);
    }

    resolve_supersampled(&hi, w, h)
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
struct RenderFace {
    indices: [usize; 4],
    normal: Vec3,
    distance: f32,
}

/// World-space centroid of a quad face.
fn face_centroid(cube: &Cube, indices: &[usize; 4]) -> Vec3 {
    indices.iter().map(|&i| cube.vertices[i]).sum::<Vec3>() / 4.0
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

/// Project a quad's 4 corners and fill it (as two triangles) into the
/// framebuffer with `color`. If any corner clips off-screen the face is skipped
/// (acceptable for a single centered cube — PLAN Task 2 step 4).
fn fill_face(
    fb: &mut Framebuffer,
    projector: &Projector,
    cube: &Cube,
    indices: &[usize; 4],
    color: Color,
) {
    let mut pts = [(0.0f32, 0.0f32); 4];
    for (slot, &i) in pts.iter_mut().zip(indices.iter()) {
        match projector.project(cube.vertices[i]) {
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
        let mut faces = [
            RenderFace { indices: [0, 1, 2, 3], normal: Vec3::Z, distance: 1.0 },
            RenderFace { indices: [0, 1, 2, 3], normal: Vec3::Z, distance: 5.0 },
            RenderFace { indices: [0, 1, 2, 3], normal: Vec3::Z, distance: 3.0 },
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
}
