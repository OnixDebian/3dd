//! PROTOTYPE: real-pixel cube renderer via the kitty graphics protocol.
//!
//! This is a side path to compare against the braille renderer. It rasterizes the
//! cube into a true RGBA image (per-pixel color, z-buffer, 2× supersampled AA) and
//! ships it to a kitty-compatible terminal as actual pixels — no glyph packing, so
//! no braille staircase. Reuses the existing [`Projector`], cube geometry, camera
//! and palette; only the output target differs.
//!
//! Entered via `dd3 --kitty` (live orbit) or `dd3 --dump-rgba <path>` (one frame to
//! a raw RGBA file, for offline inspection). Not wired into the ratatui app.

use std::io::{self, Write};
use std::time::{Duration, Instant};

use color_eyre::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use crossterm::{cursor, execute};
use glam::Vec3;
use ratatui::style::Color;

use crate::camera::{Camera, DEFAULT_FOV};
use crate::config::RenderConfig;
use crate::render3d::cube::{unit_cube, Cube};
use crate::render3d::project::Projector;
use crate::render3d::ViewParams;
use crate::theme::Palette;

/// Internal supersampling factor per axis for anti-aliasing. Real pixels, so this
/// is plain box-filtered MSAA (no braille constraints).
const SS: usize = 2;

/// Render one frame of the orbiting cube to a raw RGBA buffer (`w*h*4` bytes).
///
/// Square pixels, so the projector's `cell_aspect` is 1.0 (the braille 2.0 value
/// corrects for tall braille dots, which do not apply here).
pub fn render_rgba(
    cube: &Cube,
    view: ViewParams,
    palette: &Palette,
    w: usize,
    h: usize,
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

    let base = palette.status_color(crate::theme::Status::Running);

    for face in &cube.faces {
        let centroid = face.indices.iter().map(|&i| cube.vertices[i]).sum::<Vec3>() / 4.0;
        if face.normal.dot(view.eye - centroid) <= 0.0 {
            continue; // back-face cull
        }
        let shaded = face_shade(face.normal, centroid, &view, base, bg);

        // Project the 4 corners; skip face if any clips.
        let mut pts = [(0.0f32, 0.0f32, 0.0f32); 4];
        let mut clipped = false;
        for (slot, &i) in pts.iter_mut().zip(face.indices.iter()) {
            match projector.project(cube.vertices[i]) {
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
/// distance fog. The fog bounds are the camera-to-center distance ± the cube's
/// bounding radius, NOT a per-frame min/max of visible faces — that relative range
/// made a face's brightness depend on which OTHER faces were visible, so the top
/// face flickered as the sides rotated through. With fixed bounds each face's shade
/// depends only on its own (here constant) geometry, so it is stable frame-to-frame.
fn face_shade(
    normal: Vec3,
    centroid: Vec3,
    view: &ViewParams,
    base: Color,
    bg: (u8, u8, u8),
) -> (u8, u8, u8) {
    const CUBE_BOUND: f32 = 0.8660254; // unit-cube bounding sphere radius = sqrt(3)/2
    let cam_dist = (view.eye - view.target).length();
    let near = cam_dist - CUBE_BOUND;
    let far = cam_dist + CUBE_BOUND;
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

/// Live orbit loop rendering real pixels via kitty graphics. Quits on q/Esc/Ctrl-C.
pub fn run_kitty() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, cursor::Hide)?;
    write!(stdout, "\x1b[2J")?; // clear screen
    stdout.flush()?;

    let cube = unit_cube();
    let palette = Palette::default();
    let mut camera = Camera::new();
    camera.radius = 3.2; // closer than the braille default (6.0) — fills the image
    let mut last = Instant::now();

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

            // Size the image to the window pixel size (kitty reports it), leaving a
            // small margin; fall back to a square if unavailable.
            let (w, h) = match crossterm::terminal::window_size() {
                Ok(ws) if ws.width > 0 && ws.height > 0 => {
                    ((ws.width as usize).min(1000), (ws.height as usize).min(800))
                }
                _ => (720, 560),
            };

            let now = Instant::now();
            let dt = now.duration_since(last).as_secs_f32();
            last = now;
            camera.step(dt);
            let view = camera.view_params(DEFAULT_FOV);

            let rgba = render_rgba(&cube, view, &palette, w, h);

            delete_all(&mut stdout)?;
            write!(stdout, "\x1b[H")?; // cursor home
            emit_kitty(&mut stdout, &rgba, w, h)?;
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
    let cube = unit_cube();
    let palette = Palette::default();
    let mut camera = Camera::new();
    camera.radius = 3.2;
    camera.step(2.0); // advance to a 3/4 pose
    let view = camera.view_params(DEFAULT_FOV);
    let rgba = render_rgba(&cube, view, &palette, w, h);
    std::fs::write(path, &rgba)?;
    println!("{w} {h} {}", rgba.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The top face's shade must NOT change as the camera orbits in yaw (fixed
    /// pitch/radius). This pins the fix for the "top flickers brighter/darker"
    /// bug — absolute fog bounds make a face's brightness independent of which
    /// other faces are currently visible.
    #[test]
    fn top_face_shade_is_yaw_invariant() {
        let palette = Palette::default();
        let base = palette.status_color(crate::theme::Status::Running);
        let bg = to_rgb(palette.background);
        let top_normal = Vec3::Y;
        let top_centroid = Vec3::new(0.0, 0.5, 0.0);

        let mut camera = Camera::new();
        camera.radius = 3.2;
        let mut shades = Vec::new();
        for _ in 0..12 {
            camera.step(0.5); // advance yaw, pitch stays fixed
            let view = camera.view_params(DEFAULT_FOV);
            shades.push(face_shade(top_normal, top_centroid, &view, base, bg));
        }
        // Every sampled yaw must yield the identical top-face color.
        assert!(
            shades.windows(2).all(|w| w[0] == w[1]),
            "top face shade varied across yaw (flicker): {shades:?}"
        );
    }
}
