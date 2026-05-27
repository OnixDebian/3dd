//! PROTOTYPE: real-pixel cube renderer via the kitty graphics protocol.
//!
//! This is a side path to compare against the braille renderer. It rasterizes the
//! cube into a true RGBA image (per-pixel color, z-buffer, 2× supersampled AA) and
//! ships it to a kitty-compatible terminal as actual pixels — no glyph packing, so
//! no braille staircase. Reuses the existing [`Projector`], cube geometry, camera
//! and palette; only the output target differs.
//!
//! Entered via `dd3 --kitty` (live render) or `dd3 --dump-rgba <path>` (one frame
//! to a raw RGBA file, for offline inspection). The camera holds a fixed 3/4
//! framing angle; the motion is each box spinning in place about its own +Y axis.

use std::io::{self, Write};
use std::time::{Duration, Instant};

use color_eyre::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use crossterm::{cursor, execute};
use glam::Vec3;
use ratatui::style::Color;

use crate::camera::{Camera, DEFAULT_FOV, SPIN_RATE};
use crate::config::RenderConfig;
use crate::render3d::cube::unit_cube;
use crate::render3d::project::Projector;
use crate::render3d::{rotate_y_about, ViewParams};
use crate::theme::Palette;
use crate::world::entity::Entity;
use crate::world::scene::{synthetic_scene, SceneBounds};

/// Internal supersampling factor per axis for anti-aliasing. Real pixels, so this
/// is plain box-filtered MSAA (no braille constraints).
const SS: usize = 2;

/// Render one frame of the whole World to a raw RGBA buffer (`w*h*4` bytes).
///
/// Every `entity` is drawn as the unit cube translated to `entity.position` and
/// scaled by `entity.half_extents * 2`, colored by `palette.status_color`. All
/// boxes share ONE `color`/`depth` buffer, so the per-pixel z-buffer in
/// [`fill_tri`] resolves inter-box occlusion for free (near hides far) — the
/// kitty path's built-in advantage over the painter-sorted braille path.
///
/// Fog uses the passed `bounds` for a SCENE-WIDE ABSOLUTE range
/// (camera-to-center distance ± `bounds.radius`), so a face's brightness depends
/// only on the static scene + camera, never on which faces happen to be visible
/// this frame — that is the flicker-safe property carried over from Phase 1.
///
/// Square pixels, so the projector's `cell_aspect` is 1.0 (the braille 2.0 value
/// corrects for tall braille dots, which do not apply here).
///
/// `spin` is the per-box self-rotation angle (radians) about each box's OWN +Y
/// axis. The camera is static (the human's verify override of the scene orbit);
/// the motion is each box spinning in place. Each box's corners and face normals
/// are rotated about that box's center before projection — the box stays a rigid
/// convex solid so the shared z-buffer still resolves inter-box occlusion.
pub fn render_rgba(
    entities: &[Entity],
    view: ViewParams,
    bounds: &SceneBounds,
    palette: &Palette,
    w: usize,
    h: usize,
    spin: f32,
) -> Vec<u8> {
    let (sw, sh) = (w * SS, h * SS);
    let bg = to_rgb(palette.background);
    // Supersampled accumulation buffers.
    let mut color = vec![bg; sw * sh];
    let mut depth = vec![f32::INFINITY; sw * sh];

    let config = RenderConfig {
        fov: view.fov,
        cell_aspect: 1.0,
        ..RenderConfig::default()
    };
    let projector = Projector::new(view.eye, view.target, view.up, (sw as u32, sh as u32), &config);

    // Scene-wide ABSOLUTE fog range: camera-to-scene-center distance ± the scene
    // bounding-sphere radius. A function of the static scene + camera only (never
    // per-frame visible faces), which is what keeps the shade flicker-free.
    let cam_to_center = (view.eye - bounds.center).length();
    let near = cam_to_center - bounds.radius;
    let far = cam_to_center + bounds.radius;

    // The unit cube is the per-box geometry template; faces/normals are reused for
    // every entity (transformed below), so build it once.
    let cube = unit_cube();

    for entity in entities {
        let base = palette.status_color(entity.status);
        // Per-axis scale = full extent (half_extents * 2); translate to position,
        // then spin the box about its OWN center around +Y by `spin`.
        let scale = entity.half_extents * 2.0;
        let world = |v: Vec3| rotate_y_about(entity.position + v * scale, entity.position, spin);

        for face in &cube.faces {
            // World-space corners + centroid for this entity's face.
            let corners: [Vec3; 4] = [
                world(cube.vertices[face.indices[0]]),
                world(cube.vertices[face.indices[1]]),
                world(cube.vertices[face.indices[2]]),
                world(cube.vertices[face.indices[3]]),
            ];
            let centroid = corners.iter().copied().sum::<Vec3>() / 4.0;
            // The box is spun, so rotate the axis-aligned normal by the same spin
            // to recover the true world normal for cull + Lambert shading.
            let normal = rotate_y_about(face.normal, Vec3::ZERO, spin);
            if normal.dot(view.eye - centroid) <= 0.0 {
                continue; // back-face cull against the eye, in world space
            }
            let shaded = face_shade(normal, centroid, &view, near, far, base, bg);

            // Project the 4 corners; skip face if any clips.
            let mut pts = [(0.0f32, 0.0f32, 0.0f32); 4];
            let mut clipped = false;
            for (slot, corner) in pts.iter_mut().zip(corners.iter()) {
                match projector.project(*corner) {
                    Some((x, y, z)) => *slot = (x, y, z),
                    None => {
                        clipped = true;
                        break;
                    }
                }
            }
            if clipped {
                continue;
            }
            fill_tri(&mut color, &mut depth, sw, sh, pts[0], pts[1], pts[2], shaded);
            fill_tri(&mut color, &mut depth, sw, sh, pts[0], pts[2], pts[3], shaded);
        }
    }

    // Box-downsample SS×SS -> final RGBA.
    let mut out = vec![0u8; w * h * 4];
    let n = (SS * SS) as u32;
    for y in 0..h {
        for x in 0..w {
            let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let (cr, cg, cb) = color[(y * SS + sy) * sw + (x * SS + sx)];
                    r += cr as u32;
                    g += cg as u32;
                    b += cb as u32;
                }
            }
            let idx = (y * w + x) * 4;
            out[idx] = (r / n) as u8;
            out[idx + 1] = (g / n) as u8;
            out[idx + 2] = (b / n) as u8;
            out[idx + 3] = 255;
        }
    }
    out
}

/// Flat-shade one face: orientation (Lambert toward the eye) plus ABSOLUTE
/// distance fog. The fog `near`/`far` bounds are the SCENE-WIDE absolute range
/// (camera-to-scene-center distance ± the scene bounding radius, computed once
/// per frame by the caller from the passed `SceneBounds`), NOT a per-frame
/// min/max of visible faces — that relative range made a face's brightness
/// depend on which OTHER faces were visible, so the top face flickered as the
/// sides rotated through. With fixed scene bounds each face's shade depends only
/// on the static scene + camera, so it is stable frame-to-frame.
fn face_shade(
    normal: Vec3,
    centroid: Vec3,
    view: &ViewParams,
    near: f32,
    far: f32,
    base: Color,
    bg: (u8, u8, u8),
) -> (u8, u8, u8) {
    let to_eye_dir = (view.eye - view.target).normalize_or_zero();
    let lambert = normal.dot(to_eye_dir).max(0.0);
    let orient = 0.62 + (1.0 - 0.62) * lambert;
    let fog = fog_factor((view.eye - centroid).length(), near, far);
    shade(base, orient, bg, fog)
}

/// Flat-shade the base color by orientation (toward black) then distance fog
/// (toward background), returning packed RGB.
fn shade(base: Color, orient: f32, bg: (u8, u8, u8), fog: f32) -> (u8, u8, u8) {
    let (br, bg_, bb) = to_rgb(base);
    // orientation: lerp black->base by orient
    let o = orient.clamp(0.0, 1.0);
    let (mut r, mut g, mut b) = (
        (br as f32 * o) as u8,
        (bg_ as f32 * o) as u8,
        (bb as f32 * o) as u8,
    );
    // fog: lerp bg->color by fog
    let f = fog.clamp(0.0, 1.0);
    r = lerp(bg.0, r, f);
    g = lerp(bg.1, g, f);
    b = lerp(bg.2, b, f);
    (r, g, b)
}

fn lerp(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round().clamp(0.0, 255.0) as u8
}

fn fog_factor(distance: f32, near: f32, far: f32) -> f32 {
    const FOG_MIN: f32 = 0.7;
    let span = far - near;
    if span <= f32::EPSILON {
        return 1.0;
    }
    let t = ((distance - near) / span).clamp(0.0, 1.0);
    1.0 - t * (1.0 - FOG_MIN)
}

/// Z-buffered barycentric triangle fill into the supersampled RGB buffer. Depth is
/// the projector's NDC z (smaller = nearer).
#[allow(clippy::too_many_arguments)]
fn fill_tri(
    color: &mut [(u8, u8, u8)],
    depth: &mut [f32],
    w: usize,
    h: usize,
    a: (f32, f32, f32),
    b: (f32, f32, f32),
    c: (f32, f32, f32),
    rgb: (u8, u8, u8),
) {
    let min_x = a.0.min(b.0).min(c.0).floor().max(0.0) as i32;
    let max_x = a.0.max(b.0).max(c.0).ceil().min(w as f32 - 1.0) as i32;
    let min_y = a.1.min(b.1).min(c.1).floor().max(0.0) as i32;
    let max_y = a.1.max(b.1).max(c.1).ceil().min(h as f32 - 1.0) as i32;
    if min_x > max_x || min_y > max_y {
        return;
    }
    let area = edge(a, b, c);
    if area.abs() < f32::EPSILON {
        return;
    }
    let inv = 1.0 / area;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let p = (x as f32 + 0.5, y as f32 + 0.5);
            let w0 = edge2(b, c, p) * inv;
            let w1 = edge2(c, a, p) * inv;
            let w2 = edge2(a, b, p) * inv;
            if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                let z = w0 * a.2 + w1 * b.2 + w2 * c.2;
                let idx = y as usize * w + x as usize;
                if z < depth[idx] {
                    depth[idx] = z;
                    color[idx] = rgb;
                }
            }
        }
    }
}

fn edge(a: (f32, f32, f32), b: (f32, f32, f32), c: (f32, f32, f32)) -> f32 {
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
}
fn edge2(a: (f32, f32, f32), b: (f32, f32, f32), p: (f32, f32)) -> f32 {
    (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0)
}

fn to_rgb(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (0, 0, 0),
    }
}

/// Minimal standard base64 encoder (avoids a dependency).
fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18 & 63) as usize] as char);
        out.push(T[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6 & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}

/// Transmit + display an RGBA image at the cursor via the kitty graphics protocol,
/// chunked into ≤4096-byte base64 payloads. `q=2` suppresses terminal replies so
/// they don't pollute our input stream.
fn emit_kitty(out: &mut impl Write, rgba: &[u8], w: usize, h: usize) -> io::Result<()> {
    // zlib-compress the pixels (o=z): a flat-shaded cube on a solid background
    // compresses ~20-50×, cutting the per-frame payload from MBs to tens of KB —
    // the dominant cost of animating real pixels over the terminal.
    let compressed = miniz_oxide::deflate::compress_to_vec_zlib(rgba, 3);
    let payload = base64(&compressed);
    let bytes = payload.as_bytes();
    let mut chunks = bytes.chunks(4096).peekable();
    let mut first = true;
    while let Some(chunk) = chunks.next() {
        let more = if chunks.peek().is_some() { 1 } else { 0 };
        if first {
            write!(
                out,
                "\x1b_Gf=32,s={w},v={h},a=T,t=d,o=z,q=2,m={more};"
            )?;
            first = false;
        } else {
            write!(out, "\x1b_Gm={more};")?;
        }
        out.write_all(chunk)?;
        write!(out, "\x1b\\")?;
    }
    Ok(())
}

/// Delete all displayed images (used to clear the previous frame).
fn delete_all(out: &mut impl Write) -> io::Result<()> {
    write!(out, "\x1b_Ga=d,q=2\x1b\\")
}

/// Best-effort detection of kitty graphics protocol support from the environment.
///
/// Real-pixel rendering only works in terminals that implement the protocol
/// (kitty, ghostty, WezTerm). Alacritty and most SSH/dumb terminals support NO
/// inline graphics at all, so the app must fall back to the braille renderer there.
/// This uses env hints (reliable for the common cases); a runtime query handshake
/// would be the fully robust upgrade.
pub fn supports_kitty_graphics() -> bool {
    if std::env::var_os("KITTY_WINDOW_ID").is_some()
        || std::env::var_os("GHOSTTY_RESOURCES_DIR").is_some()
        || std::env::var_os("WEZTERM_PANE").is_some()
    {
        return true;
    }
    matches!(std::env::var("TERM"), Ok(t) if t.contains("kitty") || t.contains("ghostty"))
}

/// Live loop rendering real pixels via kitty graphics: a static-camera 3/4 view of
/// the rack with each box spinning in place. Quits on q/Esc/Ctrl-C.
pub fn run_kitty() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, cursor::Hide)?;
    write!(stdout, "\x1b[2J")?; // clear screen
    stdout.flush()?;

    let world = synthetic_scene();
    let palette = Palette::default();
    let mut camera = Camera::new();
    camera.frame_scene(&world.bounds); // frame the whole rack (frustum-safe radius)
    let mut last = Instant::now();
    let mut fps = 0.0f32;
    // Per-box self-spin angle, advanced by REAL dt (framerate-independent), since
    // the camera is now static and the motion is each box spinning in place.
    let mut spin = 0.0f32;

    let result = (|| -> Result<()> {
        loop {
            // Input (non-blocking).
            if event::poll(Duration::from_millis(0))? {
                if let Event::Key(k) = event::read()? {
                    if k.kind == KeyEventKind::Press {
                        let quit = matches!(k.code, KeyCode::Char('q') | KeyCode::Esc)
                            || (k.code == KeyCode::Char('c')
                                && k.modifiers.contains(KeyModifiers::CONTROL));
                        if quit {
                            break;
                        }
                    }
                }
            }

            // Terminal geometry: cells (cols/rows) for the status line + pixels for
            // the image. Reserve the BOTTOM cell row for the status bar so the image
            // never covers it. Fall back to sane defaults if the terminal doesn't
            // report a pixel size.
            let (cols, rows, px_w, px_h) = match crossterm::terminal::window_size() {
                Ok(ws) if ws.width > 0 && ws.height > 0 && ws.rows > 0 => {
                    (ws.columns, ws.rows, ws.width as usize, ws.height as usize)
                }
                _ => (90, 30, 720, 560),
            };
            let cell_h = (px_h / rows as usize).max(1);
            let w = px_w.min(1400);
            let h = px_h.saturating_sub(cell_h).clamp(1, 1080); // leave the last row

            let now = Instant::now();
            let dt = now.duration_since(last).as_secs_f32();
            last = now;
            if dt > 0.0 {
                let inst = 1.0 / dt;
                fps = if fps == 0.0 { inst } else { fps * 0.9 + inst * 0.1 };
            }
            camera.step(dt); // holds a fixed framing angle now (YAW_RATE == 0)
            spin = (spin + SPIN_RATE * dt).rem_euclid(std::f32::consts::TAU);
            let view = camera.view_params(DEFAULT_FOV);

            let rgba = render_rgba(&world.entities, view, &world.bounds, &palette, w, h, spin);

            delete_all(&mut stdout)?;
            write!(stdout, "\x1b[H")?; // cursor home — image anchored top-left
            emit_kitty(&mut stdout, &rgba, w, h)?;
            // Status bar on the reserved bottom row (mirrors the braille HUD).
            write!(
                stdout,
                "\x1b[{rows};1H\x1b[2K3dd | fps: {fps:.0} | size: {cols}x{rows} | kitty | q to quit"
            )?;
            stdout.flush()?;

            std::thread::sleep(Duration::from_millis(33));
        }
        Ok(())
    })();

    // Restore.
    let _ = delete_all(&mut stdout);
    let _ = execute!(stdout, cursor::Show);
    let _ = write!(stdout, "\x1b[2J\x1b[H");
    let _ = stdout.flush();
    let _ = disable_raw_mode();
    result
}

/// Render a single frame to a raw RGBA file (`w h` printed to stdout) for offline
/// inspection without a kitty terminal.
pub fn dump_rgba(path: &str, w: usize, h: usize) -> Result<()> {
    let world = synthetic_scene();
    let palette = Palette::default();
    let mut camera = Camera::new();
    camera.frame_scene(&world.bounds); // frame the whole rack
    // Static camera now; advance the per-box spin to an informative 3/4 pose so
    // the dump shows boxes mid-rotation (not all axis-aligned/edge-on).
    let spin = SPIN_RATE * 2.0;
    let view = camera.view_params(DEFAULT_FOV);
    let rgba = render_rgba(&world.entities, view, &world.bounds, &palette, w, h, spin);
    std::fs::write(path, &rgba)?;
    println!("{w} {h} {}", rgba.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The top face's shade must NOT change as the camera orbits in yaw (fixed
    /// pitch/radius). This pins the fix for the "top flickers brighter/darker"
    /// bug — absolute SCENE-WIDE fog bounds make a face's brightness independent
    /// of which other faces are currently visible. The bounds are now derived
    /// from the framed synthetic scene (Task 1's scene-fog), so the property is
    /// exercised under the real multi-box fog range, not a single-cube one.
    #[test]
    fn top_face_shade_is_yaw_invariant() {
        let palette = Palette::default();
        let base = palette.status_color(crate::theme::Status::Running);
        let bg = to_rgb(palette.background);
        let world = synthetic_scene();
        let top_normal = Vec3::Y;
        // Place the probe face AT the scene center: the eye stays exactly `radius`
        // from the center for every yaw (orbit invariant), so the absolute
        // scene-fog distance is constant and ONLY a flicker bug (a fog range that
        // depended on the visible-face set) could vary the shade. Off-center
        // points legitimately change distance under orbit, which is correct fog,
        // not flicker — so the center is the right place to pin the property.
        let top_centroid = world.bounds.center;

        let mut camera = Camera::new();
        camera.frame_scene(&world.bounds);
        let mut shades = Vec::new();
        for _ in 0..12 {
            camera.step(0.5); // advance yaw, pitch stays fixed
            let view = camera.view_params(DEFAULT_FOV);
            // Scene-wide absolute fog range, exactly as render_rgba computes it.
            let cam_to_center = (view.eye - world.bounds.center).length();
            let near = cam_to_center - world.bounds.radius;
            let far = cam_to_center + world.bounds.radius;
            shades.push(face_shade(top_normal, top_centroid, &view, near, far, base, bg));
        }
        // Every sampled yaw must yield the identical top-face color.
        assert!(
            shades.windows(2).all(|w| w[0] == w[1]),
            "top face shade varied across yaw (flicker): {shades:?}"
        );
    }

    /// A camera looking straight down -Z at two same-status boxes stacked along
    /// the view axis (A near the eye, B far). Because all boxes share one
    /// z-buffer, the overlapping output pixels must carry box A's color — the
    /// nearer box — proving inter-box occlusion (near hides far, no blend).
    #[test]
    fn z_buffer_occludes_far_box_behind_near_box() {
        use crate::theme::Status;
        let palette = Palette::default();
        let (w, h) = (40usize, 40usize);

        // A in front (z = +2), B behind (z = -2); both centered on the view axis.
        let near_box = Entity {
            id: 0,
            position: Vec3::new(0.0, 0.0, 2.0),
            half_extents: Vec3::splat(0.6),
            status: Status::Crashed, // red — distinct from the running color
            group: 0,
        };
        let far_box = Entity {
            id: 1,
            position: Vec3::new(0.0, 0.0, -2.0),
            half_extents: Vec3::splat(0.6),
            status: Status::Crashed,
            group: 0,
        };
        let entities = [near_box, far_box];
        let bounds = SceneBounds::from_entities(&entities);

        // Eye on +Z looking toward -Z so A is strictly nearer than B.
        let view = ViewParams {
            eye: Vec3::new(0.0, 0.0, 10.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov: DEFAULT_FOV,
        };

        // Color A alone (drop B) to learn its exact rendered center pixel.
        let only_a = render_rgba(&[near_box], view, &bounds, &palette, w, h, 0.0);
        let both = render_rgba(&entities, view, &bounds, &palette, w, h, 0.0);

        let center = ((h / 2) * w + (w / 2)) * 4;
        let a_px = &only_a[center..center + 3];
        let both_px = &both[center..center + 3];
        let bg = to_rgb(palette.background);
        // The overlap must show A's front-face color (non-background) and be
        // identical to the A-only render — B did not bleed through, no blend.
        assert_ne!(
            (both_px[0], both_px[1], both_px[2]),
            bg,
            "center pixel was background — boxes did not render"
        );
        assert_eq!(
            both_px, a_px,
            "near box A did not fully occlude far box B (z-buffer/blend bug)"
        );
    }

    /// Two boxes of DIFFERENT status side by side must produce at least two
    /// distinct non-background colors — per-box status coloring is in effect.
    #[test]
    fn distinct_status_boxes_yield_distinct_colors() {
        use std::collections::HashSet;

        use crate::theme::Status;
        let palette = Palette::default();
        let (w, h) = (60usize, 40usize);
        let bg = to_rgb(palette.background);

        let entities = [
            Entity {
                id: 0,
                position: Vec3::new(-1.5, 0.0, 0.0),
                half_extents: Vec3::splat(0.5),
                status: Status::Running,
                group: 0,
            },
            Entity {
                id: 1,
                position: Vec3::new(1.5, 0.0, 0.0),
                half_extents: Vec3::splat(0.5),
                status: Status::Crashed,
                group: 0,
            },
        ];
        let bounds = SceneBounds::from_entities(&entities);
        let view = ViewParams {
            eye: Vec3::new(0.0, 0.0, 10.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov: DEFAULT_FOV,
        };
        let rgba = render_rgba(&entities, view, &bounds, &palette, w, h, 0.0);

        let mut colors: HashSet<(u8, u8, u8)> = HashSet::new();
        for px in rgba.chunks_exact(4) {
            let c = (px[0], px[1], px[2]);
            if c != bg {
                colors.insert(c);
            }
        }
        assert!(
            colors.len() >= 2,
            "expected >= 2 distinct non-background colors, got {}",
            colors.len()
        );
    }

    /// An empty scene renders all-background, no panic (degenerate bounds safe).
    #[test]
    fn empty_scene_is_all_background() {
        let palette = Palette::default();
        let (w, h) = (16usize, 16usize);
        let bounds = SceneBounds::from_entities(&[]);
        let view = ViewParams {
            eye: Vec3::new(0.0, 0.0, 10.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov: DEFAULT_FOV,
        };
        let rgba = render_rgba(&[], view, &bounds, &palette, w, h, 0.0);
        let bg = to_rgb(palette.background);
        assert_eq!(rgba.len(), w * h * 4);
        for px in rgba.chunks_exact(4) {
            assert_eq!((px[0], px[1], px[2]), bg, "non-background pixel in empty scene");
        }
    }
}
