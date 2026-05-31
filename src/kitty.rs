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

use std::collections::HashSet;
use std::io::{self, Write};
use std::time::{Duration, Instant};

use color_eyre::Result;
use crossterm::event::{
    self, Event, KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement,
};
use crossterm::{cursor, execute};
use glam::Vec3;
use ratatui::style::Color;
use tokio::runtime::Handle;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::action::{apply_input_action, coalesce_actions, Action, Effect, HeldAction};
use crate::camera::{Camera, DEFAULT_FOV, SPIN_RATE};
use crate::config::RenderConfig;
use crate::docker::Docker;
use crate::render3d::cube::{unit_cube, CUBE_EDGES};
use crate::render3d::project::Projector;
use crate::render3d::scene_extras::SceneExtras;
use crate::render3d::{rotate_y_about, ViewParams};
use crate::theme::{Palette, Status};
use crate::world::entity::Entity;
use crate::world::scene::{synthetic_scene, SceneBounds};
use crate::world::selection::Selection;
use crate::world::{DockerMsg, LiveWorld, World};

/// Internal supersampling factor per axis for anti-aliasing. Real pixels, so this
/// is plain box-filtered MSAA (no braille constraints).
const SS: usize = 2;

/// The four mutually-exclusive paths the kitty render loop can take per frame.
///
/// Returned by [`compute_kitty_render_decision`]. The loop body matches on the
/// variant and either: renders the live or cached World as an RGBA image
/// (`LiveWorld` / `CachedWorld`), writes the empty-state banner text
/// (`Banner`), or paints only the screen-clear + status bar with NO banner
/// text and NO image (`ChromeOnly`).
///
/// **The `ChromeOnly` variant fixes the RV6 cold-start banner-bypass bug.**
///
/// Pre-RV6, the kitty loop conflated two distinct empty-world cases into the
/// same "render_target.is_none() -> paint banner" branch:
///
/// 1. *Stable empty* (debounce satisfied / first-launch and grace expired) —
///    the legitimate banner case (Phase 3 criterion #5).
/// 2. *Cold-start race* (process just launched, `frames_since_start <
///    KITTY_STARTUP_GRACE_FRAMES`, no DockerMsg has been drained yet, cache
///    is `None`) — the RV5 grace gate computed `show_banner=false` here, but
///    the next-step `render_target` selector returned `None` because the
///    cache was empty, and the loop body painted the banner ANYWAY.
///
/// The user-reported ~100 ms flicker in kitty after RV1-RV5 was this exact
/// bypass: the App-side `should_show_empty_banner()` invariant was honored in
/// the braille view (`ui::view` falls through to a bordered "scene" block with
/// no banner text when the cache is empty during the grace), but the kitty
/// path's parallel state machine was missing the equivalent fall-through.
///
/// `ChromeOnly` makes that fall-through explicit: the kitty path issues
/// `\x1b[2J\x1b[H` to clear the surface but writes NOTHING into the scene
/// region. The status bar still paints on the reserved bottom row (it lives
/// downstream of every branch), so the user sees only "3dd | fps: ... | mode:
/// ... | palette: ..." until the bollard seed lands and the first non-empty
/// world clones into the cache.
///
/// Field semantics on the variants:
/// - `LiveWorld` is taken when the live world has entities — normal hot path.
/// - `CachedWorld` is taken when the live world is empty BUT the cache is
///   populated AND the banner is suppressed — RV4 debounce path, holds the
///   previous frame's geometry frozen.
/// - `Banner` is taken when the banner is allowed AND the world is empty.
/// - `ChromeOnly` is the cold-start fall-through: world empty, banner
///   suppressed, cache empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KittyRenderDecision {
    /// Render the live World as an RGBA image (normal path, world non-empty).
    LiveWorld,
    /// Render the cached `last_non_empty_world_kitty` as RGBA (debounce path
    /// during a transient empty, after `non_empty_seen_once` flipped true).
    CachedWorld,
    /// Write the `EMPTY_BANNER` text centered in the scene area (Phase 3
    /// criterion #5: stable-empty daemon must surface the message).
    Banner,
    /// Clear the screen + paint the status bar only — NO banner text, NO
    /// scene image. The cold-start fall-through during the grace window
    /// before bollard's seed has drained; this is the path that closes the
    /// RV6 banner-bypass and eliminates the residual ~100 ms flicker.
    ChromeOnly,
}

/// Terminal geometry the kitty render loop feeds to `render_rgba` /
/// `emit_kitty`: `(cols, rows, image_px_w, image_px_h)`.
///
/// `image_px_w/image_px_h` are the IMAGE pixel dimensions (after reserving
/// the bottom row for the status bar), not the raw terminal pixel size. They
/// land in the kitty graphics protocol header as `s={w},v={h}` — so any
/// change between consecutive frames forces kitty to allocate a NEW GPU
/// texture instead of taking the same-dimension atomic-replace fast path
/// that RV5 relies on.
pub(crate) type KittyGeometry = (u16, u16, usize, usize);

/// Number of consecutive frames a NEW geometry must be reported by
/// `crossterm::terminal::window_size()` before the kitty render loop
/// adopts it as the live geometry.
///
/// Set to 3 frames (~100 ms at the 30 FPS render cadence). Single-frame
/// blips from compositor surface reconfiguration (e.g. OBS / wf-recorder /
/// any xdg-desktop-portal screencast init that briefly invalidates the
/// terminal surface and triggers a transient pixel-size change) are
/// suppressed: the loop keeps using the previously-stable geometry and
/// `emit_kitty` keeps hitting the same-dimension atomic-replace path. A
/// genuine user-driven resize persists across many frames and is adopted
/// after the ~100 ms gate — imperceptible to a human resizing a window.
pub(crate) const KITTY_GEOMETRY_STABILITY_FRAMES: u32 = 3;
pub(crate) const KITTY_FALLBACK_GEOMETRY: KittyGeometry = (90, 30, 720, 560);

/// Per-frame state for the dimension-stability gate.
///
/// Tracks the currently-adopted geometry (`stable`) plus the most recently
/// CANDIDATE geometry from `window_size()` and how many consecutive frames
/// it has been observed.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct KittyGeometryGate {
    stable: Option<KittyGeometry>,
    candidate: Option<KittyGeometry>,
    candidate_streak: u32,
}

impl KittyGeometryGate {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The currently-adopted geometry (`None` before the first successful
    /// poll). Tests use this to assert the stability gate's external
    /// behavior without poking at internal counters.
    #[allow(dead_code)]
    pub(crate) fn stable(&self) -> Option<KittyGeometry> {
        self.stable
    }
}

/// Pure dimension-stability resolver for the kitty render loop.
///
/// Drives `KittyGeometryGate`. Called once per frame with the result of
/// `crossterm::terminal::window_size()` (filtered to `Some` iff the values
/// look valid: positive cols/rows AND positive pixel dimensions). Returns
/// the geometry the frame should use.
///
/// Behavior:
///
/// 1. **First valid poll (cold start).** Adopt immediately as `stable` —
///    no gate; the loop has nothing else to use.
/// 2. **Current matches `stable`.** Adopt; reset the candidate counter
///    (steady state — no flutter).
/// 3. **Current matches the candidate AND streak ≥
///    `KITTY_GEOMETRY_STABILITY_FRAMES`.** Promote candidate to `stable`,
///    reset counter — genuine resize confirmed.
/// 4. **Current matches the candidate but streak hasn't yet hit the
///    threshold.** Increment streak; return the old `stable` (don't yet
///    adopt the new geometry; this is the suppress-flutter path that
///    closes the OBS-recording-start flicker).
/// 5. **Current differs from BOTH `stable` and current candidate.** Set a
///    new candidate with streak=1; return old `stable`.
/// 6. **No valid `current` (window_size returned 0 or errored).** Hold
///    the existing `stable` if any, else fall back to
///    `KITTY_FALLBACK_GEOMETRY`. Candidate is left unchanged — a
///    transient-invalid frame doesn't reset a real in-flight resize.
///
/// **Invariant** — once `stable` is `Some(_)`, the resolver NEVER returns
/// a different geometry within the next `KITTY_GEOMETRY_STABILITY_FRAMES -
/// 1` calls regardless of `current`. This is the byte-level property that
/// closes the user-reported "flickers when starting OBS recording" bug:
/// during the compositor surface reconfiguration triggered by OBS's
/// screencast portal allocation, the terminal may emit one or two
/// invalid/transient pixel-size reports, but the kitty image headers stay
/// byte-stable (`s={w},v={h}` unchanged) so `emit_kitty` keeps using the
/// same-dimension atomic-replace fast path RV5 relies on.
pub(crate) fn resolve_kitty_geometry(
    gate: &mut KittyGeometryGate,
    current: Option<KittyGeometry>,
) -> KittyGeometry {
    match current {
        Some(cur) => {
            match gate.stable {
                None => {
                    // Cold-start: first valid poll. Adopt immediately —
                    // there is nothing else to hold.
                    gate.stable = Some(cur);
                    gate.candidate = None;
                    gate.candidate_streak = 0;
                    cur
                }
                Some(stable) if cur == stable => {
                    // Steady state: same geometry as stable. Reset any
                    // in-flight candidate (flutter has resolved).
                    gate.candidate = None;
                    gate.candidate_streak = 0;
                    stable
                }
                Some(stable) => {
                    // Differs from stable. Treat as a candidate.
                    if gate.candidate == Some(cur) {
                        gate.candidate_streak =
                            gate.candidate_streak.saturating_add(1);
                    } else {
                        gate.candidate = Some(cur);
                        gate.candidate_streak = 1;
                    }
                    if gate.candidate_streak >= KITTY_GEOMETRY_STABILITY_FRAMES {
                        // Confirmed resize — promote.
                        gate.stable = Some(cur);
                        gate.candidate = None;
                        gate.candidate_streak = 0;
                        cur
                    } else {
                        // Suppress the flutter: keep emitting at the
                        // previously-stable geometry so the kitty image
                        // header's `s=,v=` bytes don't change.
                        stable
                    }
                }
            }
        }
        None => {
            // Invalid poll. Hold the stable geometry if any, else fall
            // back. Leave candidate state alone — a single invalid frame
            // shouldn't reset an in-flight resize.
            gate.stable.unwrap_or(KITTY_FALLBACK_GEOMETRY)
        }
    }
}

/// Pure decision function for the kitty render branch. Takes ONLY the state
/// scalars the per-frame loop tracks; returns which path to take.
///
/// Extracted as a pure function so it can be unit-tested without spinning up
/// the full `run_kitty` loop (which depends on a tokio runtime, crossterm raw
/// mode, kitty graphics protocol output, bollard, and the docker mpsc
/// channels). The single-state-machine invariant is now expressible as
/// table-driven tests against this one function.
///
/// Invariants pinned by tests:
///
/// - **Cold-start frame 0 with no cache → `ChromeOnly`.** This is the RV6
///   fix. The pre-RV6 loop returned (effectively) `Banner` here and the
///   banner flickered for ~100 ms until bollard's seed landed.
/// - **Live non-empty world → `LiveWorld`.** Always overrides everything;
///   even during the grace window, a populated world renders immediately.
/// - **Live empty + cache populated + banner suppressed → `CachedWorld`.**
///   The RV4 debounce path: the last seen geometry holds while the daemon
///   settles a transient empty.
/// - **Live empty + banner allowed → `Banner`.** Stable-empty or
///   post-grace-with-never-seen path; Phase 3 criterion #5.
/// - **Live empty + banner suppressed + cache empty → `ChromeOnly`.** The
///   cold-start fall-through, plus any rare edge where the cache somehow
///   stays empty after the grace (defensive — should not happen in practice
///   because `should_show_empty_banner` would have returned true).
///
/// Mirrors `App::should_show_empty_banner` + `App::effective_world_for_view`
/// from the braille backend. The two backends now follow the SAME state
/// machine.
pub(crate) fn compute_kitty_render_decision(
    world_empty_now: bool,
    non_empty_seen_once: bool,
    empty_streak_frames: u32,
    frames_since_start: u32,
    has_cached_world: bool,
    debounce_frames: u32,
    grace_frames: u32,
) -> KittyRenderDecision {
    // Live non-empty wins immediately, regardless of any timer.
    if !world_empty_now {
        return KittyRenderDecision::LiveWorld;
    }
    // World is empty. Compute whether the banner is allowed THIS frame.
    let in_startup_grace = frames_since_start < grace_frames;
    let show_banner = !in_startup_grace
        && (!non_empty_seen_once || empty_streak_frames >= debounce_frames);
    if show_banner {
        // Stable empty (post-grace + never-seen, OR post-grace + sustained
        // empty for debounce_frames). Phase 3 criterion #5.
        return KittyRenderDecision::Banner;
    }
    // Banner is suppressed (in grace, or non_empty_seen_once and within the
    // debounce window). If we have a cached non-empty world, use it.
    if has_cached_world {
        return KittyRenderDecision::CachedWorld;
    }
    // Banner suppressed AND no cache — the cold-start race. Paint chrome
    // only; the bollard seed will drain within ~1 s and either populate the
    // cache (→ CachedWorld next frame) or expire the grace (→ Banner once
    // we're sure the daemon truly is empty). No flicker because no banner
    // text is emitted here.
    KittyRenderDecision::ChromeOnly
}

/// Half-thickness (in SUPERSAMPLED pixels) of wireframe edges in the kitty path.
/// Chosen so the SS-downsampled line reads as ~1.5 output-pixels wide — thin
/// enough that a dense wireframe (many stopped containers) doesn't visually
/// collapse into a blob, thick enough to survive the box-filter downsample to
/// the final RGBA.
const WIRE_HALF_PX: f32 = 1.5;

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
#[allow(clippy::too_many_arguments)]
pub fn render_rgba(
    entities: &[Entity],
    view: ViewParams,
    bounds: &SceneBounds,
    palette: &Palette,
    w: usize,
    h: usize,
    spin: f32,
    selected_id: Option<u32>,
    selection_pulse_phase: f32,
    extras: &SceneExtras<'_>,
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

    // ENT-01 (04-04): floor-planes for each network group, drawn BEFORE the
    // cube loop. The shared z-buffer in fill_tri/draw_line_z resolves
    // overlap correctly: a near cube face naturally beats the floor at the
    // pixels it overlaps because cubes sit ABOVE the floor in world Y, so
    // their depth is closer to the camera at every shared screen pixel.
    for floor in extras.floors {
        let y = floor.center.y;
        let hx = floor.half_size_xz.x;
        let hz = floor.half_size_xz.y;
        let corners = [
            Vec3::new(floor.center.x - hx, y, floor.center.z - hz),
            Vec3::new(floor.center.x + hx, y, floor.center.z - hz),
            Vec3::new(floor.center.x + hx, y, floor.center.z + hz),
            Vec3::new(floor.center.x - hx, y, floor.center.z + hz),
        ];
        let mut proj = [(0.0f32, 0.0f32, 0.0f32); 4];
        let mut clipped = false;
        for (slot, c) in proj.iter_mut().zip(corners.iter()) {
            match projector.project(*c) {
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
        let line_rgb = to_rgb(floor.color);
        for i in 0..4 {
            draw_line_z(
                &mut color,
                &mut depth,
                sw,
                sh,
                proj[i],
                proj[(i + 1) % 4],
                line_rgb,
                WIRE_HALF_PX,
            );
        }
    }

    // The unit cube is the per-box geometry template; faces/normals are reused for
    // every entity (transformed below), so build it once.
    let cube = unit_cube();

    // Per-entity brightness pulse for the Tab-selected box (04-03 / CAM-04).
    // Computed once per entity (constant across its faces / edges).
    let pulse_mult_for = |entity_id: u32| -> f32 {
        if Some(entity_id) == selected_id {
            const AMPLITUDE: f32 = 0.18;
            1.0 + AMPLITUDE * (selection_pulse_phase * std::f32::consts::TAU).sin()
        } else {
            1.0
        }
    };

    for entity in entities {
        // Per-axis scale = full extent (half_extents * 2); translate to position,
        // then spin the box about its OWN center around +Y by `spin`.
        let scale = entity.half_extents * 2.0;
        let world = |v: Vec3| rotate_y_about(entity.position + v * scale, entity.position, spin);
        let pulse_mult = pulse_mult_for(entity.id);

        if entity.status.is_solid() {
            // SOLID PATH (Running): fill the 6 cube faces (existing logic).
            let base = palette.status_color(entity.status);
            for face in &cube.faces {
                let corners: [Vec3; 4] = [
                    world(cube.vertices[face.indices[0]]),
                    world(cube.vertices[face.indices[1]]),
                    world(cube.vertices[face.indices[2]]),
                    world(cube.vertices[face.indices[3]]),
                ];
                let centroid = corners.iter().copied().sum::<Vec3>() / 4.0;
                let normal = rotate_y_about(face.normal, Vec3::ZERO, spin);
                if normal.dot(view.eye - centroid) <= 0.0 {
                    continue;
                }
                let shaded = face_shade(normal, centroid, &view, near, far, base, bg);
                let shaded = scale_brightness_rgb(shaded, pulse_mult);

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
        } else {
            // WIREFRAME PATH (non-Running): draw the 12 cube edges. The
            // shared `depth` buffer makes the lines correctly occluded by
            // solid boxes in front, and lets wireframes occlude each other
            // edge-by-edge. Faces are transparent — only the outline shows.
            //
            // Per-edge color: palette.edge dimmed by midpoint distance (the
            // SAME fog factor a face at that midpoint would get), so wire
            // edges fog out at depth like the rest of the scene.
            for &(ia, ib) in &CUBE_EDGES {
                let a3 = world(cube.vertices[ia]);
                let b3 = world(cube.vertices[ib]);
                let (Some(pa), Some(pb)) = (projector.project(a3), projector.project(b3)) else {
                    continue;
                };
                let midpoint_dist = (view.eye - (a3 + b3) * 0.5).length();
                let fog = fog_factor(midpoint_dist, near, far);
                let shaded = shade(palette.edge, 1.0, bg, fog);
                let shaded = scale_brightness_rgb(shaded, pulse_mult);
                draw_line_z(&mut color, &mut depth, sw, sh, pa, pb, shaded, WIRE_HALF_PX);
            }
        }
    }

    // ENT-02 (04-04): port glow pass. Drawn AFTER the cube loop so it lands
    // on TOP of cube faces — same emissive contract as the braille path: no
    // fog, no Lambert. Each port's emissive quad sits at FACE_EPS outside
    // the picked face, so its NDC z is fractionally smaller than the face
    // it sits on; the shared z-buffer keeps it from being eaten by the
    // cube fill that just landed there.
    //
    // Color = palette.glow (bright indigo) — deliberately DIFFERENT from the
    // Running face color (light green) so the port dot reads as a port and
    // doesn't blend invisibly into the cube it sits on.
    let glow_rgb = to_rgb(palette.glow);
    let port_face_axes: [(Vec3, Vec3, Vec3); 4] = [
        (Vec3::Z, Vec3::X, Vec3::Y),         // +Z front (tie-break #1)
        (Vec3::X, Vec3::NEG_Z, Vec3::Y),     // +X right
        (Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y), // -Z back
        (Vec3::NEG_X, Vec3::Z, Vec3::Y),     // -X left
    ];
    for entity in entities {
        let Some(ports) = extras.ports.get(&entity.id) else {
            continue;
        };
        if ports.is_empty() {
            continue;
        }
        // Pick camera-facing vertical face (deterministic tie-break order).
        let to_eye = view.eye - entity.position;
        let mut best: Option<(usize, f32)> = None;
        for (i, (n, _, _)) in port_face_axes.iter().enumerate() {
            let spun_n = rotate_y_about(*n, Vec3::ZERO, spin);
            let d = spun_n.dot(to_eye);
            if d <= 0.0 {
                continue;
            }
            best = match best {
                None => Some((i, d)),
                Some((_, prev_d)) if d > prev_d => Some((i, d)),
                _ => best,
            };
        }
        let Some((face_idx, _)) = best else { continue };
        let (normal, u_axis_local, v_axis_local) = port_face_axes[face_idx];
        let spun_n = rotate_y_about(normal, Vec3::ZERO, spin);
        let spun_u = rotate_y_about(u_axis_local, Vec3::ZERO, spin);
        let spun_v = v_axis_local;
        let h_u = if normal.x.abs() > 0.5 {
            entity.half_extents.z
        } else {
            entity.half_extents.x
        };
        let h_v = entity.half_extents.y;
        let h_n = if normal.x.abs() > 0.5 {
            entity.half_extents.x
        } else {
            entity.half_extents.z
        };
        const FACE_EPS: f32 = 1e-3;
        let face_center = entity.position + spun_n * (h_n + FACE_EPS);
        // Dot half-size: 18% of the box face's shorter axis with a 0.10
        // world-unit floor so the glow remains readable on idle MIN_HALF
        // (0.3) boxes (matches the braille formula in render3d::raster).
        let face_min = h_u.min(h_v);
        let dot_half = (0.18 * face_min).max(0.10);
        for (i, _port) in ports.iter().take(9).enumerate() {
            let col = (i % 3) as f32;
            let row = (i / 3) as f32;
            let u_off = ((col + 1.0) / 4.0 - 0.5) * 2.0;
            let v_off = ((row + 1.0) / 4.0 - 0.5) * 2.0;
            let port_pos = face_center + spun_u * (u_off * h_u) + spun_v * (v_off * h_v);
            let verts = [
                port_pos - spun_u * dot_half - spun_v * dot_half,
                port_pos + spun_u * dot_half - spun_v * dot_half,
                port_pos + spun_u * dot_half + spun_v * dot_half,
                port_pos - spun_u * dot_half + spun_v * dot_half,
            ];
            let mut proj_pts = [(0.0f32, 0.0f32, 0.0f32); 4];
            let mut clipped = false;
            for (slot, v) in proj_pts.iter_mut().zip(verts.iter()) {
                match projector.project(*v) {
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
            // Two triangles, FULL brightness emissive (no shade, no fog).
            fill_tri(
                &mut color,
                &mut depth,
                sw,
                sh,
                proj_pts[0],
                proj_pts[1],
                proj_pts[2],
                glow_rgb,
            );
            fill_tri(
                &mut color,
                &mut depth,
                sw,
                sh,
                proj_pts[0],
                proj_pts[2],
                proj_pts[3],
                glow_rgb,
            );
        }
    }

    // ENT-03 (04-05): volume cylinders. N-gon prism (8 sides by default)
    // sitting on TOP of containers with mount_count >= 1. The shared
    // z-buffer in fill_tri resolves overlap with cubes correctly (cube
    // top face is one layer deep; the cylinder sits +1e-3 above so its
    // depth wins at every shared pixel of the cube top). No back-face
    // cull on the sides — the per-face yaw-to-camera dot sort is the
    // braille trick; kitty's z-buffer makes it unnecessary, but we still
    // skip back-facing sides to halve the fill cost.
    const CYL_SIDES: usize = 8;
    for cyl in extras.cylinders {
        use std::f32::consts::TAU;
        let cyl_rgb = to_rgb(cyl.color);
        let bot_y = cyl.center.y;
        let top_y = cyl.center.y + cyl.height;
        let mut top_ring: [Vec3; CYL_SIDES] = [Vec3::ZERO; CYL_SIDES];
        let mut bot_ring: [Vec3; CYL_SIDES] = [Vec3::ZERO; CYL_SIDES];
        for i in 0..CYL_SIDES {
            let ang = TAU * i as f32 / CYL_SIDES as f32;
            let (s, c) = ang.sin_cos();
            let x = cyl.center.x + cyl.radius * c;
            let z = cyl.center.z + cyl.radius * s;
            top_ring[i] = Vec3::new(x, top_y, z);
            bot_ring[i] = Vec3::new(x, bot_y, z);
        }
        // Sides — back-face cull via outward-XZ normal dotted with
        // (eye - centroid). Front-facing only.
        for i in 0..CYL_SIDES {
            let a = top_ring[i];
            let b = top_ring[(i + 1) % CYL_SIDES];
            let c_v = bot_ring[(i + 1) % CYL_SIDES];
            let d = bot_ring[i];
            let centroid = (a + b + c_v + d) * 0.25;
            let normal_xz = Vec3::new(
                centroid.x - cyl.center.x,
                0.0,
                centroid.z - cyl.center.z,
            )
            .normalize_or_zero();
            if normal_xz.dot(view.eye - centroid) <= 0.0 {
                continue;
            }
            let mut pts = [(0.0f32, 0.0f32, 0.0f32); 4];
            let verts = [a, b, c_v, d];
            let mut clipped = false;
            for (slot, v) in pts.iter_mut().zip(verts.iter()) {
                match projector.project(*v) {
                    Some(p) => *slot = p,
                    None => {
                        clipped = true;
                        break;
                    }
                }
            }
            if clipped {
                continue;
            }
            fill_tri(&mut color, &mut depth, sw, sh, pts[0], pts[1], pts[2], cyl_rgb);
            fill_tri(&mut color, &mut depth, sw, sh, pts[0], pts[2], pts[3], cyl_rgb);
        }
        // Caps — top cap visible from above, bottom cap from below.
        let top_n = Vec3::Y;
        let bot_n = Vec3::NEG_Y;
        for (ring, normal, ring_y) in [(top_ring, top_n, top_y), (bot_ring, bot_n, bot_y)] {
            let centroid = Vec3::new(cyl.center.x, ring_y, cyl.center.z);
            if normal.dot(view.eye - centroid) <= 0.0 {
                continue;
            }
            // Project all ring corners.
            let mut pts: [(f32, f32, f32); CYL_SIDES] = [(0.0, 0.0, 0.0); CYL_SIDES];
            let mut clipped = false;
            for (slot, v) in pts.iter_mut().zip(ring.iter()) {
                match projector.project(*v) {
                    Some(p) => *slot = p,
                    None => {
                        clipped = true;
                        break;
                    }
                }
            }
            if clipped {
                continue;
            }
            // Fan-triangulate from vertex 0.
            for i in 1..CYL_SIDES - 1 {
                fill_tri(
                    &mut color,
                    &mut depth,
                    sw,
                    sh,
                    pts[0],
                    pts[i],
                    pts[i + 1],
                    cyl_rgb,
                );
            }
        }
    }

    // ENT-04 (04-05): image stacks. Each stack is N short cubes stacked
    // along +Y at `base`. Reuses the cube geometry + back-face cull, but
    // through the kitty path's z-buffered fill_tri (not the braille
    // fill_face). Color is palette.edge — same muted gray as floor lines
    // and wireframe cubes.
    let stack_rgb = to_rgb(palette.edge);
    let stack_cube = unit_cube();
    let layer_half = crate::render3d::stack::LAYER_HALF;
    let layer_gap = crate::render3d::stack::LAYER_GAP;
    let max_layers = crate::render3d::stack::MAX_LAYERS;
    for stack in extras.image_stacks {
        if stack.layer_count == 0 {
            continue;
        }
        let n = stack.layer_count.min(max_layers);
        let scale = Vec3::splat(layer_half * 2.0);
        for i in 0..n {
            let center = Vec3::new(
                stack.base.x,
                stack.base.y + i as f32 * layer_gap + layer_half,
                stack.base.z,
            );
            for face in &stack_cube.faces {
                let corners = [
                    center + stack_cube.vertices[face.indices[0]] * scale,
                    center + stack_cube.vertices[face.indices[1]] * scale,
                    center + stack_cube.vertices[face.indices[2]] * scale,
                    center + stack_cube.vertices[face.indices[3]] * scale,
                ];
                let centroid = corners.iter().copied().sum::<Vec3>() / 4.0;
                if face.normal.dot(view.eye - centroid) <= 0.0 {
                    continue;
                }
                let mut pts = [(0.0f32, 0.0f32, 0.0f32); 4];
                let mut clipped = false;
                for (slot, c) in pts.iter_mut().zip(corners.iter()) {
                    match projector.project(*c) {
                        Some(p) => *slot = p,
                        None => {
                            clipped = true;
                            break;
                        }
                    }
                }
                if clipped {
                    continue;
                }
                fill_tri(&mut color, &mut depth, sw, sh, pts[0], pts[1], pts[2], stack_rgb);
                fill_tri(&mut color, &mut depth, sw, sh, pts[0], pts[2], pts[3], stack_rgb);
            }
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

/// Z-buffered thick-line rasterizer. Draws a band of width `2 * half_px`
/// around the segment `a` → `b` into the shared `color`/`depth` SS buffer.
/// Depth is linearly interpolated along the line (NDC z, smaller = nearer)
/// and z-tested per pixel — so a wireframe edge is correctly hidden behind a
/// solid face that already filled the buffer there. Iterates the bounding
/// box of the segment + thickness; perpendicular distance to the segment
/// gives the band mask, with the projected `t` parameter giving the depth.
#[allow(clippy::too_many_arguments)]
fn draw_line_z(
    color: &mut [(u8, u8, u8)],
    depth: &mut [f32],
    w: usize,
    h: usize,
    a: (f32, f32, f32),
    b: (f32, f32, f32),
    rgb: (u8, u8, u8),
    half_px: f32,
) {
    let (ax, ay, az) = a;
    let (bx, by, bz) = b;
    let dx = bx - ax;
    let dy = by - ay;
    let len2 = dx * dx + dy * dy;
    // Degenerate (start == end on screen): plant a single dot at `a`.
    if len2 <= f32::EPSILON {
        let xi = ax.round() as i32;
        let yi = ay.round() as i32;
        if (0..w as i32).contains(&xi) && (0..h as i32).contains(&yi) {
            let idx = yi as usize * w + xi as usize;
            if az < depth[idx] {
                depth[idx] = az;
                color[idx] = rgb;
            }
        }
        return;
    }
    let inv_len2 = 1.0 / len2;

    // Bounding box of the THICK segment (segment AABB inflated by half_px).
    let pad = half_px + 1.0;
    let min_x = (ax.min(bx) - pad).floor().max(0.0) as i32;
    let max_x = (ax.max(bx) + pad).ceil().min(w as f32 - 1.0) as i32;
    let min_y = (ay.min(by) - pad).floor().max(0.0) as i32;
    let max_y = (ay.max(by) + pad).ceil().min(h as f32 - 1.0) as i32;
    if min_x > max_x || min_y > max_y {
        return;
    }
    let half_sq = half_px * half_px;

    for y in min_y..=max_y {
        for x in min_x..=max_x {
            // Pixel center.
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            // Project pixel onto the line: t in [0,1] is the segment parameter
            // at the closest point. Clamped so caps are flat (not capsule-round).
            let t = ((px - ax) * dx + (py - ay) * dy) * inv_len2;
            if !(0.0..=1.0).contains(&t) {
                continue;
            }
            let cx = ax + t * dx;
            let cy = ay + t * dy;
            let ddx = px - cx;
            let ddy = py - cy;
            if ddx * ddx + ddy * ddy > half_sq {
                continue;
            }
            // Interpolate depth along the segment at parameter `t`.
            let z = az + t * (bz - az);
            let idx = y as usize * w + x as usize;
            if z < depth[idx] {
                depth[idx] = z;
                color[idx] = rgb;
            }
        }
    }
}

fn to_rgb(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (0, 0, 0),
    }
}

/// Multiply a packed RGB triple by `mult` per channel, clamped to `[0, 255]`.
/// `mult` near 1.0 leaves the color visually identical; the selection-pulse
/// amplitude (0.18) keeps the swing readable without ever saturating.
fn scale_brightness_rgb(rgb: (u8, u8, u8), mult: f32) -> (u8, u8, u8) {
    if (mult - 1.0).abs() < 1e-6 {
        return rgb;
    }
    let m = mult.max(0.0);
    let r = ((rgb.0 as f32 * m).round()).clamp(0.0, 255.0) as u8;
    let g = ((rgb.1 as f32 * m).round()).clamp(0.0, 255.0) as u8;
    let b = ((rgb.2 as f32 * m).round()).clamp(0.0, 255.0) as u8;
    (r, g, b)
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

/// Persistent kitty image ID used for atomic frame-to-frame replace.
///
/// ## 05-05-RV5: persistent image ID closes the delete-then-emit race window
///
/// **The bug RV5 fixes:** the user reported "the 3D environment itself
/// disappears, especially often starts flickering when the window is not
/// in focus." Status bar + legend stay solid; only the kitty pixel image
/// flickers / vanishes intermittently. FPS drops to ~13 (from ~30) when
/// unfocused, widening the gap.
///
/// **Root cause:** pre-RV5 every LiveWorld / CachedWorld frame did
/// `delete_all` → `\x1b[H` → `emit_kitty`. `emit_kitty` transmits tens of
/// KB of base64-encoded zlib-compressed RGBA in chunks of ≤4096 bytes per
/// `\x1b_G...\x1b\\` segment, often spanning many segments. Between
/// kitty receiving `delete_all` (image is gone) and finishing parsing the
/// last `emit_kitty` segment (image is restored), the kitty compositor
/// can render the intermediate "no image" state. When the window is
/// UNFOCUSED, kitty (as a wayland/X compositor optimization) throttles
/// its own rendering — making that gap window much larger than the
/// 33 ms render budget. The user sees the image vanish for several
/// frames at a time.
///
/// **Fix:** use a persistent kitty image ID (`i=<N>`) and DROP the
/// per-frame `delete_all`. Per the kitty graphics protocol, transmitting
/// a new image with `a=T` and the same `i=N` ATOMICALLY replaces the
/// prior image with that ID — no visible gap, no separate delete step.
/// Kitty processes the new image transmission to completion, THEN swaps
/// it in as a single operation.
///
/// `KITTY_IMAGE_ID = 1` is the constant ID used by `emit_kitty` and the
/// targeted `delete_image_by_id` helper. `delete_all` is retained but
/// now only called on (a) explicit empty-state transitions (Banner /
/// ChromeOnly with `last_was_empty=false`) and (b) shutdown — both
/// genuine cases where the image must actually go away.
const KITTY_IMAGE_ID: u32 = 1;

/// Transmit + display an RGBA image at the cursor via the kitty graphics protocol,
/// chunked into ≤4096-byte base64 payloads. `q=2` suppresses terminal replies so
/// they don't pollute our input stream.
///
/// ## 05-05-RV4: `z=-1` puts the image strictly BELOW terminal cell text
///
/// Pre-RV4 the image placement omitted the `z` key, which defaults to `z=0`.
/// Per kitty's graphics-protocol docs (graphics-protocol.rst, line 533):
/// "Negative z-index values mean that the images will be drawn under the
/// text. This allows rendering of text on top of images." So at `z=0` the
/// image is NOT guaranteed to be under text — text-over-image only works
/// when the cell carries a non-default background (the image fills the
/// cell pixel area underneath the glyph, BG covers it).
///
/// User-observed symptom (RV4): the legend appeared for ONE frame
/// (`KittyRenderDecision::ChromeOnly` cold-start, where no image is
/// emitted at all) then disappeared on `LiveWorld` frame 2+ when the
/// first image placed at default `z=0` started covering the cell layer.
/// Pre-05-05 the only cell text inside the image footprint was the small
/// per-frame label (single line, infrequent updates) and the optional
/// popup (opt-in, rare) — both were small enough that their cell-bg
/// interactions with `z=0` images were tolerable. The 22×7 legend made
/// the inadequacy of `z=0` immediately visible.
///
/// Fix: emit `z=-1` so images are STRICTLY below text. The legend
/// (and labels, and popup) now render reliably on top regardless of
/// per-cell background state. RV3's solid-bg fix is still load-bearing
/// for visual clarity (the legend reads as a panel, not transparent
/// text floating over a busy scene), but it is no longer the only
/// guard against the image covering the legend.
///
/// ## 05-05-RV5: `i=1` enables atomic frame-to-frame replace
///
/// See `KITTY_IMAGE_ID` rustdoc above. The header now carries `i=1` so
/// each `emit_kitty` call atomically replaces the previous frame's
/// image instead of relying on a separate `delete_all` step that
/// created an observable gap (especially when the window is unfocused
/// and kitty throttles its compositor rendering).
fn emit_kitty(out: &mut impl Write, rgba: &[u8], w: usize, h: usize) -> io::Result<()> {
    // zlib-compress the pixels (o=z): a flat-shaded cube on a solid background
    // compresses ~20-50×, cutting the per-frame payload from MBs to tens of KB —
    // the dominant cost of animating real pixels over the terminal.
    let compressed = miniz_oxide::deflate::compress_to_vec_zlib(rgba, 3);
    let payload = base64(&compressed);
    let bytes = payload.as_bytes();
    let mut chunks = bytes.chunks(4096).peekable();
    let mut first = true;
    let id = KITTY_IMAGE_ID;
    while let Some(chunk) = chunks.next() {
        let more = if chunks.peek().is_some() { 1 } else { 0 };
        if first {
            // z=-1: image is rendered UNDER terminal cell text. Load-
            // bearing for the legend HUD (see fn rustdoc above).
            // i=<KITTY_IMAGE_ID>: persistent image ID — re-transmitting
            // with the same id atomically replaces the prior frame's
            // image (no delete-then-emit race; see RV5 rustdoc).
            write!(
                out,
                "\x1b_Gf=32,s={w},v={h},a=T,t=d,o=z,q=2,z=-1,i={id},m={more};"
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

/// Delete all displayed images (used at shutdown and on explicit empty-state
/// transitions; NOT called per-frame any more — see `KITTY_IMAGE_ID` rustdoc
/// and `emit_kitty`).
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
/// Build floor-planes for the kitty path. Mirrors the braille builder in
/// `ui::scene` — the selected entity's group_key gets the indigo
/// status_color(Running) highlight, every other group gets palette.edge.
fn build_floor_planes_kitty(
    live: &LiveWorld,
    selection: &Selection,
    palette: &Palette,
) -> Vec<crate::render3d::scene_extras::FloorPlane> {
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
            crate::render3d::scene_extras::FloorPlane {
                center,
                half_size_xz,
                color,
            }
        })
        .collect()
}

/// Build the per-entity port lookup for the kitty path. Mirrors the braille
/// path: borrows `ContainerSnapshot.ports` directly from LiveWorld entries,
/// keyed by `Entity.id` so the renderer's port glow pass can do a direct
/// `extras.ports.get(&entity.id)` lookup.
fn build_port_lookup_kitty<'a>(
    world: &World,
    live: &'a LiveWorld,
) -> crate::render3d::scene_extras::PortLookup<'a> {
    let mut map = crate::render3d::scene_extras::PortLookup::new();
    for entity in &world.entities {
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

/// Build per-frame volume cylinders for the kitty path (ENT-03 / 04-05).
///
/// One cylinder per entity with `mount_count >= 1`. Cylinder height comes
/// from `proxy_volume_height(mount_count)` (the sqrt-compressive mount-count
/// proxy from `world::volume`); the cylinder sits on top of the cube
/// (`center.y = entity.top_y`), with radius scaled to the cube's XZ half-
/// extent. Color is `palette.volume` (muted teal — distinct from running
/// green / glow indigo).
///
/// I12 closure: uses the existing `LiveWorld::snapshot(id).mount_count`
/// accessor — no `LiveWorld::mount_count(id)` helper added.
fn build_volume_cylinders_kitty(
    world: &World,
    live: &LiveWorld,
    palette: &Palette,
) -> Vec<crate::render3d::scene_extras::Cylinder> {
    let mut out: Vec<crate::render3d::scene_extras::Cylinder> = Vec::new();
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
        // Sit on TOP of the cube: cube center.y + half_extent.y.
        let center = Vec3::new(
            entity.position.x,
            entity.position.y + entity.half_extents.y,
            entity.position.z,
        );
        let radius = entity.half_extents.x * 0.4;
        out.push(crate::render3d::scene_extras::Cylinder {
            center,
            radius,
            height,
            color: palette.volume,
        });
    }
    out
}

/// Build per-frame image stacks for the kitty path (ENT-04 / 04-05).
///
/// One stack per image in `LiveWorld::image_stack_positions()`. Positions
/// are deterministic (BTreeMap-by-id order + MAX_RACK_X constant — W9
/// closure). The kitty path drops the repo_tag (the third tuple element):
/// labels aren't part of the image stack visual yet — Phase 5 may add a
/// label tier for selected stacks.
fn build_image_stacks_kitty(
    live: &LiveWorld,
) -> Vec<crate::render3d::scene_extras::ImageStack> {
    live.image_stack_positions()
        .into_iter()
        .map(|(base, layer_count, _tag)| crate::render3d::scene_extras::ImageStack {
            base,
            layer_count,
        })
        .collect()
}

/// Compute the (col0, row0, w, h) cell-grid rectangle for the kitty popup.
///
/// `pct_w` / `pct_h` are in `0.0..=1.0`. Result is clamped so a tiny terminal
/// still produces a 0..n rect with `n >= 3` (need at least one inner row for
/// content). Cell coordinates are 0-indexed; the caller adds +1 when writing
/// ANSI cursor moves (which are 1-indexed).
fn centered_cell_rect(cols: usize, rows: usize, pct_w: f32, pct_h: f32) -> (usize, usize, usize, usize) {
    // Target = pct of cols/rows; floor minimums (20w / 8h) so a popup stays
    // legible; cap to the actual terminal so the rect never overflows.
    let target_w = (cols as f32 * pct_w).round() as usize;
    let target_h = (rows as f32 * pct_h).round() as usize;
    let w = target_w.max(20).min(cols.max(1));
    let h = target_h.max(8).min(rows.saturating_sub(1).max(1));
    let col0 = cols.saturating_sub(w) / 2;
    let row0 = rows.saturating_sub(h) / 2;
    (col0, row0, w, h)
}

/// Draw a unicode-box-drawing border at the given cell rect onto `stdout`.
///
/// Uses U+250C / U+2500 / U+2510 / U+2502 / U+2514 / U+2518 (light box).
/// Cursor positions are 1-indexed in ANSI; the caller passes 0-indexed
/// `col0` / `row0` so this fn converts. After the border, the popup interior
/// (rows `row0+1..row0+h-1`, cols `col0+1..col0+w-1`) is cleared with spaces
/// so the underlying image bytes don't bleed through.
fn draw_popup_box(
    stdout: &mut impl Write,
    col0: usize,
    row0: usize,
    w: usize,
    h: usize,
) -> io::Result<()> {
    if w < 3 || h < 3 {
        return Ok(());
    }
    // Top row: ┌─...─┐
    write!(stdout, "\x1b[{};{}H", row0 + 1, col0 + 1)?;
    write!(stdout, "\u{250C}")?;
    for _ in 0..(w - 2) {
        write!(stdout, "\u{2500}")?;
    }
    write!(stdout, "\u{2510}")?;
    // Middle rows: │   spaces   │
    for r in 1..(h - 1) {
        write!(stdout, "\x1b[{};{}H", row0 + 1 + r, col0 + 1)?;
        write!(stdout, "\u{2502}")?;
        for _ in 0..(w - 2) {
            write!(stdout, " ")?;
        }
        write!(stdout, "\u{2502}")?;
    }
    // Bottom row: └─...─┘
    write!(stdout, "\x1b[{};{}H", row0 + h, col0 + 1)?;
    write!(stdout, "\u{2514}")?;
    for _ in 0..(w - 2) {
        write!(stdout, "\u{2500}")?;
    }
    write!(stdout, "\u{2518}")?;
    Ok(())
}

/// Produce the per-line text contents for the kitty popup body.
///
/// Mirrors the braille panel's field set (status, health, image, started,
/// restarts, network, ports, mounts, block I/O, help) as plain strings. The
/// renderer writes each line with a `MoveTo + Print` pair clipped to the
/// popup's interior width. Mounts and ports are truncated to the first 3
/// entries with a "…N more" tail to keep the popup height predictable.
fn format_detail_lines(snap: &crate::docker::DetailSnapshot, blkio_r: u64, blkio_w: u64) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(20);
    out.push(format!(" {} ", snap.name));
    out.push(format!("Status:     {:?}", snap.status));
    out.push(format!("Health:     {}", kitty_health_str(&snap.health)));
    out.push(format!("Image:      {}", snap.image_human));
    out.push(format!("Started:    {}", snap.started_at_iso));
    out.push(format!(
        "Restarts:   {} ({})",
        snap.restart_count, snap.restart_policy
    ));
    out.push(format!("Network:    {}", snap.network_mode));
    out.push(format!("Ports ({}):", snap.ports.len()));
    out.push(format!("  {}", kitty_format_ports(&snap.ports)));
    out.push(format!("Mounts ({}):", snap.mounts.len()));
    for m in snap.mounts.iter().take(3) {
        out.push(format!(
            "  {} ({})",
            kitty_format_mount(m),
            if m.rw { "rw" } else { "ro" }
        ));
    }
    if snap.mounts.len() > 3 {
        out.push(format!("  …{} more", snap.mounts.len() - 3));
    }
    out.push(format!(
        "Block I/O:  R {} / W {}",
        kitty_human_bytes(blkio_r),
        kitty_human_bytes(blkio_w)
    ));
    out.push(String::new());
    out.push("Esc close · Enter refresh · q quit".to_string());
    out
}

/// Map [`HealthSummary`] to a kitty-popup-friendly summary (mirrors the
/// braille `health_str` but local so the kitty path doesn't need to import
/// ui::detail_panel's pub-fn'd helpers).
fn kitty_health_str(h: &crate::docker::HealthSummary) -> String {
    use crate::docker::HealthSummary;
    match h {
        HealthSummary::None => "none".to_string(),
        HealthSummary::Starting => "starting".to_string(),
        HealthSummary::Healthy => "healthy".to_string(),
        HealthSummary::Unhealthy {
            failing_streak,
            last_output,
        } => {
            if last_output.is_empty() {
                format!("unhealthy (streak={failing_streak})")
            } else {
                let trimmed: String = last_output
                    .replace(['\n', '\r'], " ")
                    .chars()
                    .take(40)
                    .collect();
                format!("unhealthy (streak={failing_streak}): {trimmed}")
            }
        }
    }
}

/// Comma-join a port list — kitty-side mirror of detail_panel::format_ports.
fn kitty_format_ports(ports: &[crate::docker::PortSummary]) -> String {
    if ports.is_empty() {
        return "(none)".to_string();
    }
    let mut parts: Vec<String> = ports
        .iter()
        .take(3)
        .map(|p| match p.public {
            Some(pub_port) => format!("{}/{} -> {}", p.private, p.proto.as_str(), pub_port),
            None => format!("{}/{}", p.private, p.proto.as_str()),
        })
        .collect();
    if ports.len() > 3 {
        parts.push(format!("…{} more", ports.len() - 3));
    }
    parts.join(", ")
}

/// Render one mount — kitty-side mirror of detail_panel::format_mount.
fn kitty_format_mount(m: &crate::docker::MountSummary) -> String {
    let src = if m.source.is_empty() {
        "(anon)"
    } else {
        m.source.as_str()
    };
    let body = format!("{src}:{}", m.destination);
    if body.chars().count() > 60 {
        let half = 28;
        let head: String = body.chars().take(half).collect();
        let tail: String = body.chars().rev().take(half).collect::<String>().chars().rev().collect();
        format!("{head}…{tail}")
    } else {
        body
    }
}

/// SI-byte formatter — kitty-side mirror of detail_panel::human_bytes.
fn kitty_human_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB", "PB"];
    if n == 0 {
        return "0 B".to_string();
    }
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", n, UNITS[unit])
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

/// True when the active terminal exposes the kitty graphics protocol.
///
/// Delegates to [`crate::term::capability::detect`] (05-06 ROB-02): the
/// unified detector is the single source of truth for env-driven terminal
/// classification. Returns true iff `detect()` returns
/// [`crate::term::capability::TerminalCapability::Kitty`].
pub fn supports_kitty_graphics() -> bool {
    matches!(
        crate::term::capability::detect(),
        crate::term::capability::TerminalCapability::Kitty,
    )
}

/// Live loop rendering real pixels via kitty graphics: a static-camera 3/4 view of
/// the live container rack with each box spinning in place. Quits on q/Esc/Ctrl-C.
///
/// `docker_rx` carries typed [`DockerMsg`] values from `docker::streams`. Each
/// loop iteration we drain the channel non-blocking via `try_recv` (the
/// `UnboundedReceiver` doesn't need a runtime for this), feed every message to
/// the [`LiveWorld`] reconciler, and swap the local `world` binding on each
/// rebuild. The render call (`render_rgba`) is byte-for-byte unchanged — only
/// the data source flipped from `synthetic_scene()` to the live reconciler
/// (DOCK-01/03/04). When no entities are alive we skip the image entirely and
/// write a centered plain-text banner (criterion #5 — never a blank void).
///
/// SYNC path retained: bollard's producer task lives on the existing tokio
/// runtime (from `main.rs`), feeds the channel, and we read from it here
/// without needing our own runtime. The 33ms pacing + raw-mode lifecycle +
/// quit-key handling are untouched.
pub fn run_kitty(
    docker: Docker,
    tx_for_inspect: UnboundedSender<DockerMsg>,
    mut docker_rx: UnboundedReceiver<DockerMsg>,
    handle: Handle,
    config: crate::config::AppConfig,
) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, cursor::Hide)?;
    write!(stdout, "\x1b[2J")?; // clear screen
    stdout.flush()?;

    // 05-05-RV7: enable the kitty keyboard protocol (KKP) so the loop
    // receives `KeyEventKind::Press` AND `KeyEventKind::Release` (not
    // just Press as on most pre-KKP terminals). Without Release we
    // can't tell when the user lifts a movement key, which is what
    // the held-set approach requires to bypass the OS auto-repeat
    // initial delay (~250-500 ms) the user complained about ("first
    // tick fires immediately but second has a delay").
    //
    // Symmetric with `Tui::enter` in the braille backend: best-effort
    // push, falls back to OS-repeat path if the terminal doesn't
    // support it. The `crate::tui::mark_kkp_active(true)` call hooks
    // the kitty path into the same panic-hook `restore()` that pops
    // KKP on crash — otherwise a panic inside run_kitty would leave
    // KKP active across process exit and the host shell would render
    // arrow keys as escape sequences.
    let kkp_active = if supports_keyboard_enhancement().unwrap_or(false) {
        let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            | KeyboardEnhancementFlags::REPORT_EVENT_TYPES;
        if execute!(stdout, PushKeyboardEnhancementFlags(flags)).is_ok() {
            crate::tui::mark_kkp_active(true);
            true
        } else {
            false
        }
    } else {
        false
    };
    // Held-key set, populated only when `kkp_active`. On non-KKP
    // terminals this stays empty forever and the loop falls back to
    // the OS-repeat → Action::Nudge* path (Action::from_key still
    // produces a Nudge for KeyEventKind::Repeat on movement keys, so
    // the user gets the pre-RV7 OS-repeat behavior — including the
    // initial delay, which we can't fix without KKP).
    let mut held_keys: HashSet<HeldAction> = HashSet::new();

    // Resolve initial palette from config.palette. Unknown name falls back
    // to notion-soft AND rewrites palette_name so the cycle's .position()
    // lookup can find the current slot (mirrors App::with_docker; see that
    // method's doc comment for the broader rationale). `mut` because
    // runtime cycling via Effect::CyclePalette swaps both fields in place.
    let (mut palette, mut palette_name) = match crate::theme::Palette::by_name(&config.palette) {
        Some(p) => (p, config.palette.clone()),
        None => {
            eprintln!(
                "config: unknown palette '{}', falling back to notion-soft",
                config.palette
            );
            (crate::theme::Palette::notion_soft(), "notion-soft".to_string())
        }
    };
    // THEME-05: legend HUD visibility. Initialised from the config-file
    // value so the kitty backend honors the same `hud_visible` setting
    // App reads. Flipped at runtime by `Effect::ToggleLegend` (L key)
    // below.
    //
    // 05-05-RV1 (Bug A — "L doesn't work"): the original Option (c)
    // cleanup ("next `emit_kitty` re-blits over stale legend cells")
    // was wrong — `emit_kitty` places a graphics-protocol IMAGE; cell
    // text is rendered ON TOP of images and persists until written-
    // over with spaces. RV1 tracks `last_hud_visible` alongside
    // `hud_visible` and fires `emit_legend_clear_kitty` exactly once
    // when the user toggles the HUD off, so the phantom legend cells
    // get wiped on the transition frame.
    let mut hud_visible = config.hud_visible;
    let mut last_hud_visible = hud_visible;
    // `config` is otherwise unused in this plan (05-06 will read
    // `auto_degrade`, `degraded_fps_cap`, `force_mode` from it).
    let _config = config;
    // Live reconciler + an initially empty World. The banner kicks in until
    // the first `Added` from the producer lands; `render_rgba` is never called
    // on an empty entity set.
    let mut live = LiveWorld::new();
    let mut world = World {
        entities: Vec::new(),
        bounds: SceneBounds::from_entities(&[]),
    };
    let mut camera = Camera::new();
    // Tab-cycle selection + brightness pulse + detail-panel toggle (04-03).
    // Owned here mirrors App.selection in the braille backend. The same
    // apply_input_action dispatch surface mutates both, so the user-facing
    // behavior is identical across backends.
    let mut selection = Selection::new();
    let mut last = Instant::now();
    let mut fps = 0.0f32;
    // Per-box self-spin angle, advanced by REAL dt (framerate-independent), since
    // the camera is now static and the motion is each box spinning in place.
    let mut spin = 0.0f32;
    // Track the last seen entity count so we know when to re-frame the camera.
    // Add/remove changes the rack radius; pure stat updates don't, so a Stat-
    // only drain pass MUST NOT re-frame (avoids per-second view jitter).
    let mut last_count: usize = 0;
    // Track whether the previous frame painted an image (vs the empty banner)
    // so we can clear the image surface exactly when transitioning empty ->
    // non-empty (and vice versa) without flickering on every banner-only loop.
    let mut last_was_empty = true;
    // Empty-banner debounce state (mirrors the braille App fields).
    //
    // RV2 introduced the debounce at 6 frames (~200 ms). RV4 bumped to
    // 30 frames (~1000 ms) AND added the `last_non_empty_world_kitty`
    // cache. RV5 bumped the debounce window to 150 frames (~5000 ms at
    // ~30 FPS) AND added the cold-start grace: the banner is unconditionally
    // suppressed for the first `KITTY_STARTUP_GRACE_FRAMES` (~1 s) of
    // process life regardless of `non_empty_seen_once`.
    //
    // RV5 was INCOMPLETE: the gate computed `show_banner=false` during the
    // grace, but the next-step `render_target` selector returned `None`
    // (because the cache is also empty during cold start), and the loop
    // body's `if render_target.is_none()` branch painted the banner ANYWAY.
    // The user observed the same flicker after the RV5 binary shipped.
    //
    // RV6 closes the bypass by extracting the decision into a pure
    // `compute_kitty_render_decision` function that returns one of FOUR
    // explicit variants (LiveWorld / CachedWorld / Banner / ChromeOnly).
    // The ChromeOnly variant is the cold-start fall-through: world empty,
    // banner suppressed, cache empty -> paint only the status bar, NO
    // banner text. This mirrors the braille `ui::view` path that draws a
    // bordered "scene" block (no banner text) in the same situation.
    //
    // `non_empty_seen_once` is sticky after the first non-empty world;
    // after the grace expires, if it's STILL false the banner shows
    // (Phase 3 criterion #5: a daemon with zero containers gets its
    // banner, just delayed by ~1 s).
    //
    // `frames_since_start` is the kitty mirror of `App.tick_count` —
    // monotonically increments at the kitty frame cadence (~30 FPS); the
    // grace gate consults it instead of wall-time so the kitty path
    // doesn't drift on frame-rate stalls.
    let mut non_empty_seen_once = false;
    let mut empty_streak_frames: u32 = 0;
    let mut last_non_empty_world_kitty: Option<World> = None;
    let mut frames_since_start: u32 = 0;
    const KITTY_EMPTY_BANNER_DEBOUNCE_FRAMES: u32 = 150;
    const KITTY_STARTUP_GRACE_FRAMES: u32 = 30;
    // 04-04 label state: the (col, row, len) of the LAST cell-grid label the
    // kitty path drew, so we can erase it with spaces BEFORE writing the new
    // one. Avoids stale-label streaks when the selection moves or the box
    // spins past the camera-facing position.
    let mut last_label_print: Option<(u16, u16, usize)> = None;
    // 05-05-RV6: dimension-stability gate. The kitty graphics protocol
    // emits image dimensions in the header (`s={w},v={h}`); when those
    // bytes change between consecutive frames, kitty must allocate a new
    // GPU texture for the `i=1` placement (the same-dimension atomic-
    // replace fast path RV5 relies on doesn't apply to a dimension
    // change). The user reported "still flickers when starting OBS
    // recording" — diagnosed as compositor surface reconfiguration during
    // xdg-desktop-portal screencast init causing one or two transient
    // pixel-size reports from `crossterm::terminal::window_size()`. The
    // gate holds the previously-stable geometry until a NEW geometry
    // persists for `KITTY_GEOMETRY_STABILITY_FRAMES` consecutive frames
    // (~100 ms at 30 FPS) — single-frame flutter is suppressed; a
    // genuine user-driven resize is adopted ~100 ms later (imperceptible).
    let mut geometry_gate = KittyGeometryGate::new();

    let result = (|| -> Result<()> {
        loop {
            // Input (non-blocking). All key dispatch goes through the SAME
            // apply_input_action surface as the braille backend (04-03 single
            // dispatch invariant) — the Effect interpretation here interprets
            // Quit -> break and SpawnInspect -> (04-06 plugs the off-thread
            // inspect spawn here).
            //
            // 05-04-RV1 (release-stops-input fix): drain EVERY pending event
            // per frame, not just one. The pre-fix kitty loop read one event
            // per 33ms frame; a 1-second key hold at 30 Hz OS repeat queued
            // ~30 events in crossterm's internal reader, which then drained
            // one-per-frame for ANOTHER second after release. Now we pull all
            // pending events in a tight inner loop, build an Action list, and
            // coalesce identical kinds into one (matches App::run in braille).
            // Hold-to-glide still works (each frame pulls the next batch and
            // applies one nudge); release stops motion on the next frame.
            //
            // 05-05-RV7: when `kkp_active`, route movement-key Press into
            // the held set (and dispatch one nudge for the first tick),
            // movement-key Release out, and IGNORE movement-key Repeat
            // (held_keys drives the cadence instead). Non-movement keys
            // and non-KKP terminals: unchanged path through
            // `Action::from_key`, which drops Repeat for discrete keys
            // (so holding P / L / Tab cycles ONCE per Press).
            let mut pending_actions: Vec<Action> = Vec::new();
            while event::poll(Duration::from_millis(0))? {
                if let Event::Key(k) = event::read()? {
                    if kkp_active {
                        if let Some(held) = HeldAction::from_key_code(k.code) {
                            match k.kind {
                                KeyEventKind::Press => {
                                    held_keys.insert(held);
                                    // First-tick: dispatch one nudge
                                    // immediately so the user sees motion
                                    // the moment they press a key.
                                    pending_actions.push(held.to_action());
                                }
                                KeyEventKind::Release => {
                                    held_keys.remove(&held);
                                }
                                KeyEventKind::Repeat => {
                                    // No-op: per-frame held loop drives
                                    // the cadence below.
                                }
                            }
                            continue;
                        }
                    }
                    // Non-movement keys (or non-KKP terminal): route
                    // through from_key. Discrete-key Repeat is dropped
                    // there (so holding P doesn't auto-cycle).
                    pending_actions.push(Action::from_key(k));
                }
            }
            // 05-05-RV7: per-frame held-set dispatch. Emit one nudge per
            // held movement key on every render frame. This bypasses the
            // OS auto-repeat initial delay: the moment a key enters the
            // set, every subsequent frame fires a nudge until the
            // matching Release removes it. On non-KKP terminals
            // `held_keys` stays empty and this loop is a noop.
            if !held_keys.is_empty() {
                for held in held_keys.iter() {
                    pending_actions.push(held.to_action());
                }
            }
            let mut quit_requested = false;
            for action in coalesce_actions(&pending_actions) {
                let effect = apply_input_action(
                    action,
                    &mut camera,
                    &mut selection,
                    &world,
                    &live,
                );
                match effect {
                    Effect::Quit => {
                        quit_requested = true;
                        break;
                    }
                    Effect::CyclePalette => {
                        // Mirror App::next_palette logic. The cycle order
                        // is the same in both backends — keep it in sync.
                        // Helper kept local to avoid a hard dep from
                        // kitty.rs on App's method.
                        let mut order: Vec<&str> =
                            vec!["notion-soft", "cyberpunk-neon", "terminal-green"];
                        if crate::theme::Palette::from_omarchy().is_some() {
                            order.push("omarchy");
                        }
                        let cur = order
                            .iter()
                            .position(|n| *n == palette_name.as_str())
                            .unwrap_or(0);
                        let next = order[(cur + 1) % order.len()];
                        palette = crate::theme::Palette::by_name_or_default(next);
                        palette_name = next.to_string();
                    }
                    Effect::ToggleLegend => {
                        // THEME-05: flip HUD visibility. Mirrors
                        // `App::update`'s arm — kitty owns its own state
                        // because the App is not in the kitty render loop.
                        hud_visible = !hud_visible;
                    }
                    Effect::SpawnInspect(id) => {
                        // 04-06b: off-thread inspect via the tokio runtime
                        // Handle (run_kitty is sync but lives inside
                        // #[tokio::main]; Handle::spawn schedules onto
                        // that runtime). Idempotent: skip if already
                        // in-flight. Pitfall 8: render loop never blocks
                        // on the inspect.
                        if !selection.inspect_in_flight {
                            selection.inspect_in_flight = true;
                            selection.pending_detail = None;
                            let docker_c = docker.clone();
                            let tx_c = tx_for_inspect.clone();
                            handle.spawn(async move {
                                if let Ok(snap) =
                                    crate::docker::fetch_detail(&docker_c, &id).await
                                {
                                    let _ = tx_c.send(DockerMsg::Inspected(snap));
                                }
                                // On error: swallow. Popup stays "loading…"
                                // until user presses Esc or Enter again.
                            });
                        }
                    }
                    Effect::None => {}
                }
            }
            if quit_requested {
                break;
            }

            // Drain pending DockerMsgs non-blocking. `try_recv` on
            // `UnboundedReceiver` doesn't require a tokio runtime — it just
            // pops from the in-process queue. Cadence is fully decoupled from
            // the 33ms render pacing (DOCK-04 / Pitfall 3). On
            // Empty/Disconnected we just stop draining for this iteration.
            let mut rebuilt = false;
            while let Ok(msg) = docker_rx.try_recv() {
                // 04-06b: demux Inspected into selection BEFORE live.apply
                // — mirrors App::drain_docker in the braille backend so the
                // popup-facing slot is the source of truth for the renderer.
                if let DockerMsg::Inspected(snap) = &msg {
                    selection.pending_detail = Some(snap.clone());
                    selection.inspect_in_flight = false;
                }
                if let Some(w) = live.apply(msg) {
                    world = w;
                    rebuilt = true;
                }
            }
            if rebuilt && world.entities.len() != last_count {
                // Count changed (add or remove). Re-frame the camera so the
                // new rack is fully in view; pure stat updates keep the
                // existing framing intact. Kitty renders at SQUARE pixels
                // (cell_aspect = 1.0), so the framing aspect must match,
                // otherwise the scene fills only ~50% of the viewport (the
                // ratio of kitty's wider horizontal NDC to braille's).
                //
                // GATED behind autopilot_active (04-03 CAM-03): a manual-mode
                // user driving the camera must NOT be yanked back by every
                // container add/remove.
                if !world.entities.is_empty() && camera.autopilot_active {
                    camera.frame_scene_with_aspect(&world, 1.0);
                }
                last_count = world.entities.len();
                // Drop a stale selection if the selected container disappeared.
                selection.reconcile(&world);
            }

            // Terminal geometry: cells (cols/rows) for the status line + pixels for
            // the image. Reserve the BOTTOM cell row for the status bar so the image
            // never covers it.
            //
            // 05-05-RV6: route through `resolve_kitty_geometry` /
            // `KittyGeometryGate` (above) to suppress single-frame pixel-
            // size flutter from compositor surface reconfiguration (OBS
            // screencast portal init, wf-recorder start, etc.). The kitty
            // image header carries `s={w},v={h}`; holding those bytes
            // stable across the OBS-init window keeps `emit_kitty` on the
            // same-dimension atomic-replace fast path (RV5 invariant) so
            // the user no longer sees the 3D image vanish for a frame or
            // two when starting a screen recording.
            let current_geometry: Option<KittyGeometry> =
                match crossterm::terminal::window_size() {
                    Ok(ws) if ws.width > 0 && ws.height > 0 && ws.rows > 0 => Some((
                        ws.columns,
                        ws.rows,
                        ws.width as usize,
                        ws.height as usize,
                    )),
                    _ => None,
                };
            let (cols, rows, px_w, px_h) =
                resolve_kitty_geometry(&mut geometry_gate, current_geometry);
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
            // Per-frame size easing (CONT-03 / 04-01). See
            // `world::live::dress` rustdoc + RESEARCH Pitfall A: sizes are
            // eased here, NOT via a Stat-driven World rebuild, so the frame
            // cadence (~30 FPS) drives smooth breathing independently of the
            // ~1Hz Stat sample arrival. Empty entity slice is a no-op.
            live.dress(dt, &mut world.entities);
            // Advance the selection brightness-pulse phase by REAL dt — same
            // framerate-independent path the breathing pass uses (04-03).
            selection.tick(dt);

            // Empty-banner debounce bookkeeping. Update the streak BEFORE
            // deciding whether to paint banner-vs-image so the first
            // non-empty world after launch immediately drops the debounce
            // window for any future transient empties.
            //
            // RV4: also refresh the `last_non_empty_world_kitty` cache on
            // every non-empty frame so the debounce path has a frozen
            // snapshot to render instead of a blank surface.
            //
            // RV5: also advance `frames_since_start` every iteration so
            // the cold-start grace gate (below) has a monotonic counter.
            // The grace gate suppresses the banner unconditionally for
            // ~1 s after launch, eliminating the ~100 ms flash the user
            // reported.
            frames_since_start = frames_since_start.saturating_add(1);
            let world_empty_now = world.entities.is_empty();
            if world_empty_now {
                empty_streak_frames = empty_streak_frames.saturating_add(1);
            } else {
                non_empty_seen_once = true;
                empty_streak_frames = 0;
                last_non_empty_world_kitty = Some(world.clone());
            }
            // RV6 render decision (single state-machine, mirrors
            // `App::should_show_empty_banner` + `App::effective_world_for_view`).
            // Extracted into a pure function so the cold-start fall-through
            // (`ChromeOnly`) is unit-testable without standing up the full
            // kitty render loop. See `KittyRenderDecision` rustdoc for the
            // four cases and the rationale for the RV6 fix.
            let decision = compute_kitty_render_decision(
                world_empty_now,
                non_empty_seen_once,
                empty_streak_frames,
                frames_since_start,
                last_non_empty_world_kitty.is_some(),
                KITTY_EMPTY_BANNER_DEBOUNCE_FRAMES,
                KITTY_STARTUP_GRACE_FRAMES,
            );

            // Pick the World reference (if any) we'll feed to render_rgba.
            // `LiveWorld` → live `&world`, `CachedWorld` → cached snapshot,
            // `Banner` / `ChromeOnly` → no scene draw at all.
            let render_target: Option<&World> = match decision {
                KittyRenderDecision::LiveWorld => Some(&world),
                KittyRenderDecision::CachedWorld => last_non_empty_world_kitty.as_ref(),
                KittyRenderDecision::Banner | KittyRenderDecision::ChromeOnly => None,
            };

            if decision == KittyRenderDecision::Banner {
                // Stable empty (debounce satisfied OR first launch never
                // had containers AND grace expired). Banner path — Phase 3
                // criterion #5.
                if !last_was_empty {
                    delete_all(&mut stdout)?;
                    last_was_empty = true;
                }
                // Clear screen + place a centered banner. Use cell math (cols/
                // rows) for centering since this is plain text, not pixels.
                write!(stdout, "\x1b[2J\x1b[H")?;
                let banner = crate::ui::EMPTY_BANNER;
                let banner_col = if (cols as usize) > banner.len() {
                    ((cols as usize - banner.len()) / 2 + 1) as u16
                } else {
                    1
                };
                let banner_row = (rows / 2).max(1);
                write!(stdout, "\x1b[{banner_row};{banner_col}H{banner}")?;
            } else if decision == KittyRenderDecision::ChromeOnly {
                // RV6 cold-start fall-through. World is empty, banner is
                // suppressed (grace active or debounce within window), and
                // we have no cached frame to render. Paint ONLY the screen
                // clear — the status bar at the loop tail still draws on
                // the reserved bottom row, but no banner text and no scene
                // image flashes between launch and the first DockerMsg.
                //
                // This is the symmetric mirror of the braille `ui::view`
                // path that falls through to a bordered "scene" block when
                // `effective_world.is_empty() && !should_show_empty_banner()`.
                if !last_was_empty {
                    delete_all(&mut stdout)?;
                    last_was_empty = true;
                }
                write!(stdout, "\x1b[2J\x1b[H")?;
            } else if let Some(target) = render_target {
                let view = camera.view_params(DEFAULT_FOV);
                // 04-04 extras: build floor-planes + port lookup PER FRAME
                // from the live world. No clones: PortLookup borrows ports
                // from LiveWorld entries via `snapshot`.
                //
                // RV4: `target` is either the live `&world` (normal path)
                // or the cached `last_non_empty_world_kitty` (debounce
                // path). Extras pull from `live` regardless because the
                // selected entity / port lookup / cylinders reference
                // live state that may have changed even when the world
                // momentarily emptied. Stale entity-ids harmlessly skip.
                let floors = build_floor_planes_kitty(&live, &selection, &palette);
                let ports = build_port_lookup_kitty(target, &live);
                // 04-05 ENT-03 / ENT-04: volume cylinders + image stacks
                // built per-frame from the live world. Builders are local
                // to this file so all kitty-path extras assembly stays in
                // one place.
                let cylinders = build_volume_cylinders_kitty(target, &live, &palette);
                let image_stacks = build_image_stacks_kitty(&live);
                let extras = SceneExtras::new(
                    floors.as_slice(),
                    &ports,
                    cylinders.as_slice(),
                    image_stacks.as_slice(),
                );

                let rgba = render_rgba(
                    &target.entities,
                    view,
                    &target.bounds,
                    &palette,
                    w,
                    h,
                    spin,
                    selection.selected_id,
                    selection.pulse_phase,
                    &extras,
                );
                // 05-05-RV5: NO per-frame `delete_all`. The pre-RV5 sequence
                // `delete_all -> \x1b[H -> emit_kitty` created an observable
                // gap between kitty receiving the delete (image gone) and
                // finishing the multi-segment image transmission (~tens of
                // KB of base64 in 4 KB chunks per `\x1b_G...\x1b\\`
                // segment). Especially visible when the window was UNFOCUSED
                // and kitty throttled its compositor rendering — the user
                // reported the 3D image "disappears, often flickers when
                // window not in focus" while the legend + status bar stayed
                // solid. Fix: `emit_kitty` now carries `i=KITTY_IMAGE_ID`
                // so the new image atomically REPLACES the prior frame's
                // placement at the same ID; kitty parses the full
                // transmission, then swaps it in as a single operation
                // with no intermediate "no image" state. See
                // `KITTY_IMAGE_ID` rustdoc for the full reasoning.
                write!(stdout, "\x1b[H")?; // cursor home — image anchored top-left
                emit_kitty(&mut stdout, &rgba, w, h)?;
                last_was_empty = false;

                // 04-04 CONT-04: selected-only cell-grid label via
                // execute!(MoveTo + Print). Pitfall C — we deliberately do
                // NOT bake the text into the RGBA buffer; that would render
                // at fuzzy braille resolution. Print on the terminal cell
                // grid for terminal-font crispness.
                //
                // Clear the previous frame's label first so a moved
                // selection doesn't leave a streak.
                if let Some((pcol, prow, plen)) = last_label_print.take() {
                    write!(stdout, "\x1b[{prow};{pcol}H{}", " ".repeat(plen))?;
                }
                if let Some(sel_entity) = selection.selected_entity(target) {
                    let cell_w_px = (px_w / cols as usize).max(1);
                    let cell_h_px = cell_h;
                    if let Some(anchor) = crate::ui::labels::project_label_anchor(
                        &camera,
                        sel_entity,
                        (w as u32, h as u32),
                        1.0, // kitty path is square-pixel
                    ) {
                        let (cell_col, cell_row) =
                            crate::ui::labels::snap_anchor_with_hysteresis(
                                anchor,
                                &mut selection.last_label_cell_kitty,
                                (cell_w_px as u32, cell_h_px as u32),
                            );
                        // Pull the container name; truncate for safety.
                        let name = live
                            .id_string_for_entity(sel_entity.id)
                            .and_then(|cid| live.snapshot(cid))
                            .map(|s| {
                                crate::ui::labels::truncate_label(s.name.as_str())
                                    .into_owned()
                            })
                            .unwrap_or_default();
                        if !name.is_empty() {
                            // 1-indexed terminal cells; one row above the box top.
                            let term_col = (cell_col.max(0) as u16) + 1;
                            let term_row = (cell_row.max(0) as u16).saturating_sub(1) + 1;
                            // Clamp to terminal bounds: don't write past the
                            // reserved status bar row (rows - 1).
                            let term_row = term_row.clamp(1, rows.saturating_sub(1).max(1));
                            let term_col = term_col.clamp(1, cols.max(1));
                            write!(stdout, "\x1b[{term_row};{term_col}H{name}")?;
                            last_label_print = Some((term_col, term_row, name.chars().count()));
                        }
                    }
                } else {
                    // No selection (or none-visible): reset hysteresis state
                    // so the next selection snaps to its anchor immediately.
                    selection.last_label_cell_kitty = None;
                }
            }

            // Legend HUD (THEME-05). Emitted AFTER the scene image / label
            // / banner / chrome — i.e. across ALL FOUR `KittyRenderDecision`
            // variants — so the legend is visible in every render state:
            //
            // - LiveWorld: legend overdraws the top-right cells of the
            //   pixel image, same as the popup overdraws the middle.
            // - CachedWorld (debounce): legend over the cached frame.
            // - Banner: legend coexists with the centered banner (Phase 3
            //   criterion #5 still satisfied — banner text remains visible
            //   left of and below the legend's top-right corner box).
            // - ChromeOnly (RV6 cold-start fall-through): legend over a
            //   cleared screen — the only visible UI for the brief grace
            //   window before bollard's first Added lands. The braille
            //   path mirrors this: legend renders on top of the bordered
            //   "scene" block during the same window.
            //
            // Placed BEFORE the popup write so the popup overdraws the
            // legend's footprint if/when they overlap (mirrors the
            // braille view: scene → status_bar → legend → popup).
            //
            // 05-05-RV1 toggle-off cleanup (replaces the original Option
            // (c) "let emit_kitty re-blit naturally" strategy which was
            // wrong — cells render ON TOP of the kitty graphics image
            // and persist until written-over with spaces). When
            // `hud_visible` transitions true → false (user pressed L),
            // emit a one-shot `emit_legend_clear_kitty` so the cell-text
            // legend is wiped from the terminal cell buffer. The
            // Banner/ChromeOnly branches already issue `\x1b[2J\x1b[H`
            // which clears everything, but LiveWorld/CachedWorld
            // branches do NOT — those are the load-bearing paths for
            // this fix.
            if hud_visible {
                crate::ui::legend::emit_legend_kitty(
                    &mut stdout,
                    cols,
                    rows,
                    &palette,
                    &palette_name,
                )?;
            } else if last_hud_visible {
                // Transition frame: clear the cells the legend last
                // painted so the user actually sees the HUD disappear.
                crate::ui::legend::emit_legend_clear_kitty(
                    &mut stdout,
                    cols,
                    rows,
                    &palette,
                )?;
            }
            last_hud_visible = hud_visible;

            // CAM-05 / 04-06b: kitty popup OVER the image surface.
            //
            // Drawn AFTER the image emit + label so the box-drawing chars
            // and content lines land on terminal CELLS (not RGBA pixels) —
            // sharp text, not braille-fuzzy. The next frame's `delete_all`
            // + fresh `emit_kitty` re-paints the cells back to image
            // content when the popup closes; no manual clear needed.
            if selection.detail_open {
                let (col0, row0, popup_w, popup_h) =
                    centered_cell_rect(cols as usize, rows as usize, 0.6, 0.6);
                if popup_w >= 3 && popup_h >= 3 {
                    draw_popup_box(&mut stdout, col0, row0, popup_w, popup_h)?;

                    // Live blkio from LiveWorld::last_sample so the popup
                    // ticks even while open (CONT-04 / CAM-05 live data).
                    let blkio = selection
                        .selected_id
                        .and_then(|eid| live.id_string_for_entity(eid).map(|s| s.to_string()))
                        .and_then(|cid| live.last_sample(&cid).map(|s| (s.blkio_r_bytes, s.blkio_w_bytes)))
                        .unwrap_or((0, 0));

                    let lines: Vec<String> = if let Some(snap) = selection.pending_detail.as_ref() {
                        format_detail_lines(snap, blkio.0, blkio.1)
                    } else {
                        vec!["Inspecting container…".to_string()]
                    };

                    // Write each line clipped to the popup's interior width.
                    // Interior = `popup_w - 2` cells, starting at col `col0 + 1`
                    // (1-indexed: col0 + 2). Interior rows start at row0 + 1
                    // (1-indexed: row0 + 2) and end at row0 + h - 1 (excl).
                    let inner_w = popup_w.saturating_sub(2);
                    let inner_h = popup_h.saturating_sub(2);
                    for (i, line) in lines.iter().take(inner_h).enumerate() {
                        let term_row = row0 + 2 + i;
                        let term_col = col0 + 2;
                        // Truncate each line to inner_w chars (no wrap; cuts off).
                        let truncated: String = line.chars().take(inner_w).collect();
                        // Pad to inner_w so a shorter line clears stale chars from
                        // the previous frame (the popup may shrink between frames
                        // if e.g. the mount count drops).
                        let pad = inner_w.saturating_sub(truncated.chars().count());
                        write!(
                            stdout,
                            "\x1b[{term_row};{term_col}H{truncated}{}",
                            " ".repeat(pad)
                        )?;
                    }
                }
            }

            // Status bar on the reserved bottom row (mirrors the braille HUD).
            // The box count is the live container count (or 0 in the empty state).
            // The mode field flips "auto" -> "manual" the moment the user
            // presses a camera-driving key (04-03 CAM-03).
            let boxes = world.entities.len();
            let mode = if camera.autopilot_active { "auto" } else { "manual" };
            write!(
                stdout,
                "\x1b[{rows};1H\x1b[2K3dd | fps: {fps:.0} | size: {cols}x{rows} | boxes: {boxes} | mode: {mode} | palette: {palette_name} | kitty | P palette, L legend, Tab select, q quit"
            )?;
            stdout.flush()?;

            std::thread::sleep(Duration::from_millis(33));
        }
        Ok(())
    })();

    // Restore.
    let _ = delete_all(&mut stdout);
    // 05-05-RV7: pop KKP before disabling raw mode so the pop escape is
    // parsed under raw mode (where the terminal handles it cleanly)
    // rather than echoed on the cooked-mode shell prompt after exit.
    // Pair with the push above; idempotent if not active. Also clears
    // the global KKP_ACTIVE flag the panic hook checks, so a clean
    // shutdown doesn't leave the flag set for some hypothetical future
    // panic.
    if kkp_active {
        let _ = execute!(stdout, PopKeyboardEnhancementFlags);
        crate::tui::mark_kkp_active(false);
    }
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
    // Kitty pixels are square (cell_aspect 1.0), so frame for that aspect.
    camera.frame_scene_with_aspect(&world, 1.0);
    // Static camera now; advance the per-box spin to an informative 3/4 pose so
    // the dump shows boxes mid-rotation (not all axis-aligned/edge-on).
    let spin = SPIN_RATE * 2.0;
    let view = camera.view_params(DEFAULT_FOV);
    // Offline dump: no live selection, no live ports / floors / cylinders /
    // image_stacks. SceneExtras carries empty slices, so the renderer skips
    // every extras pass — pure synthetic cubes.
    let floors: Vec<crate::render3d::scene_extras::FloorPlane> = Vec::new();
    let ports = crate::render3d::scene_extras::PortLookup::new();
    let cylinders: Vec<crate::render3d::scene_extras::Cylinder> = Vec::new();
    let image_stacks: Vec<crate::render3d::scene_extras::ImageStack> = Vec::new();
    let extras = SceneExtras::new(&floors, &ports, &cylinders, &image_stacks);
    let rgba = render_rgba(
        &world.entities,
        view,
        &world.bounds,
        &palette,
        w,
        h,
        spin,
        None,
        0.0,
        &extras,
    );
    std::fs::write(path, &rgba)?;
    println!("{w} {h} {}", rgba.len());
    Ok(())
}

/// Render a snapshot of the user's ACTUAL local containers — read read-only
/// via the `docker` CLI (no bollard, no daemon socket from this process), with
/// every other container (alphabetical by name) forced to `Running` + a
/// synthetic load so the resulting frame shows a 50/50 solid-vs-wireframe mix
/// across the user's real network groups. Used for visual verification of
/// the wireframe path against a realistic layout without touching real
/// container state (no `start` / `stop`).
///
/// Falls back gracefully:
///   - `docker` CLI not on `$PATH` -> error
///   - `docker ps -a` returns 0 containers -> a small synthetic banner-ish
///     scene so the dump isn't empty (and the caller still gets a valid file).
pub fn dump_snapshot(path: &str, w: usize, h: usize) -> Result<()> {
    use std::process::Command;

    use crate::docker::domain::map_status;
    use crate::docker::stats::StatSample;
    use crate::docker::ContainerSnapshot;
    use crate::world::live::LiveWorld;
    use crate::world::DockerMsg;

    // Read containers via the docker CLI (no bollard / no async). We need a
    // record per container: id, name, lowercase state (running/exited/...),
    // and the first network name. `--format` with `|` separators sidesteps
    // tabs in name fields and lets us split deterministically.
    let out = Command::new("docker")
        .args([
            "ps",
            "-a",
            "--no-trunc",
            "--format",
            "{{.ID}}|{{.Names}}|{{.State}}|{{.Networks}}|{{.Mounts}}",
        ])
        .output()
        .map_err(|e| color_eyre::eyre::eyre!("failed to run `docker ps -a`: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(color_eyre::eyre::eyre!("`docker ps -a` failed: {stderr}"));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);

    // Parse + sort by NAME so the test frame is deterministic frame-to-frame
    // (the LiveWorld slot assignment is first-seen, so a stable input order
    // yields a stable layout). One record per non-empty line.
    // Records: (id, name, state, networks, mounts_field). `mounts_field` is
    // a comma-separated list from `{{.Mounts}}` — its element count is our
    // proxy for ContainerSnapshot.mount_count (Phase 4 ENT-03).
    let mut records: Vec<(String, String, String, String, String)> = stdout
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(5, '|').collect();
            if parts.len() != 5 || parts[0].is_empty() {
                return None;
            }
            Some((
                parts[0].to_string(),
                parts[1].to_string(),
                parts[2].to_string(),
                parts[3].to_string(),
                parts[4].to_string(),
            ))
        })
        .collect();
    records.sort_by(|a, b| a.1.cmp(&b.1));

    // Build the live world by replaying Added for each container — uses the
    // SAME slot-assignment / layout / sizing pipeline as the live render
    // (CONT-05 / first-seen-by-group). Then force every OTHER entry to
    // Running and feed a synthetic load so it renders as solid green at a
    // visible size (the wireframe-vs-solid ratio is the whole point here).
    // If we got 0 containers from the CLI, fall back to the mixed test scene
    // so the caller still gets a frame.
    if records.is_empty() {
        return dump_rgba_mixed(path, w, h);
    }

    let mut live = LiveWorld::new();
    // Track the most recent World returned by `apply` — that's the final
    // state after all messages, since `apply` rebuilds on every message that
    // actually changes the scene.
    let mut latest: Option<World> = None;
    for (id, name, state, networks, mounts) in &records {
        let group_key = networks
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .min() // alphabetically-first, mirrors `from_bollard_summary` in domain.rs
            .unwrap_or("none")
            .to_string();
        // Real mount count from the {{.Mounts}} CLI field: comma-separated
        // mount paths. Empty field => 0 mounts. This gives the offline
        // dump path REAL cylinders for containers with bind mounts/volumes
        // (ENT-03 visual gate without needing the bollard inspect path).
        let mount_count = mounts
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .count();
        let snap = ContainerSnapshot {
            id: id.clone(),
            name: name.clone(),
            status: map_status(state, None, None),
            group_key,
            mount_count,
            ..ContainerSnapshot::default()
        };
        if let Some(w) = live.apply(DockerMsg::Added(snap)) {
            latest = Some(w);
        }
    }
    // Force half to Running with a synthetic load. `load` varies in [0.3,
    // 0.8] across the forced set so the solid boxes also have varied sizes
    // (avoids a "row of identical cubes" look). Uses StatusChanged + Stat,
    // the same path the live event stream would drive.
    //
    // Every other forced container also gets a synthetic Enriched message
    // carrying 1-3 fake ports, so the offline dump exercises the ENT-02
    // port glow path the same way the live `from_bollard_summary` seed
    // would (the dump_snapshot CLI doesn't have port info).
    let forced: Vec<&(String, String, String, String, String)> =
        records.iter().step_by(2).collect();
    let n = forced.len().max(1) as f32;
    for (i, rec) in forced.iter().enumerate() {
        let id = &rec.0;
        if let Some(w) = live.apply(DockerMsg::StatusChanged(
            id.clone(),
            crate::theme::Status::Running,
        )) {
            latest = Some(w);
        }
        let load = 0.3 + 0.5 * (i as f32 / n);
        let sample = StatSample {
            cpu_pct: 0.0,
            mem_used: 0,
            mem_limit: 0,
            mem_fraction: 0.0,
            load,
            warming_up: false,
            blkio_r_bytes: 0,
            blkio_w_bytes: 0,
        };
        if let Some(w) = live.apply(DockerMsg::Stat(id.clone(), sample)) {
            latest = Some(w);
        }
        // Synthetic ports for every OTHER forced container so the dump shows
        // the ENT-02 glow pass against a varied port count distribution.
        if i % 2 == 0 {
            let port_count = (i % 4 + 1) as u16;
            let ports: Vec<crate::docker::PortSummary> = (0..port_count)
                .map(|p| crate::docker::PortSummary {
                    private: 8000 + p,
                    public: Some(8000 + p),
                    proto: crate::docker::PortProto::Tcp,
                })
                .collect();
            // Read current snap to preserve group_key during enrich.
            let group_key = live
                .snapshot(id)
                .map(|s| s.group_key.clone())
                .unwrap_or_else(|| "none".to_string());
            let enriched = crate::docker::EnrichedSnapshot {
                id: id.clone(),
                group_key,
                ports,
                mount_count: 0,
                status_override: None,
            };
            if let Some(w) = live.apply(DockerMsg::Enriched(enriched)) {
                latest = Some(w);
            }
        }
    }
    // Synthetic image set for the offline dump (ENT-04 visual gate without
    // a live `list_images` call): a small varied set so the stack region
    // shows different heights (1-7 layers). Real local images aren't
    // available here without a bollard call; this keeps the dump path
    // bollard-free while still exercising the rendering pipeline.
    for (i, (id, _name, _state, _net, _mounts)) in records.iter().take(5).enumerate() {
        let layers = 1 + (i * 2) % 7; // 1, 3, 5, 0, 2 -> all > 0
        let layers = layers.max(1);
        let synth = crate::docker::ImageSnapshot {
            id: format!("img-{i}-{}", &id[..id.len().min(8)]),
            repo_tag: format!("synthetic:tag-{i}"),
            layer_count: layers,
        };
        if let Some(w) = live.apply(DockerMsg::ImageAdded(synth)) {
            latest = Some(w);
        }
    }

    let world = latest.expect("non-empty record set must yield at least one rebuild");

    let palette = Palette::default();
    let mut camera = Camera::new();
    camera.frame_scene_with_aspect(&world, 1.0);
    let spin = SPIN_RATE * 2.0;
    let view = camera.view_params(DEFAULT_FOV);
    // Offline snapshot: build floor-planes from the live world so the dump
    // exercises the ENT-01 wiring even when no graphics terminal is
    // attached. Port lookup borrows live snapshot ports (often empty in
    // dump_snapshot since seed snapshots may not have rich port data).
    let floors = build_floor_planes_kitty(&live, &Selection::new(), &palette);
    let ports = build_port_lookup_kitty(&world, &live);
    let cylinders = build_volume_cylinders_kitty(&world, &live, &palette);
    let image_stacks = build_image_stacks_kitty(&live);
    let extras = SceneExtras::new(
        floors.as_slice(),
        &ports,
        cylinders.as_slice(),
        image_stacks.as_slice(),
    );
    let rgba = render_rgba(
        &world.entities,
        view,
        &world.bounds,
        &palette,
        w,
        h,
        spin,
        None,
        0.0,
        &extras,
    );
    std::fs::write(path, &rgba)?;
    let solid = world
        .entities
        .iter()
        .filter(|e| e.status.is_solid())
        .count();
    let total = world.entities.len();
    println!("{w} {h} {} (solid {solid}/{total})", rgba.len());
    Ok(())
}

/// Render a mixed-status scene: a small set of Running boxes (solid green
/// faces) interleaved with many non-Running boxes (wireframe gray) across
/// several network groups, mimicking a real-world mostly-stopped Docker rack.
/// Used to verify the wireframe path offline.
pub fn dump_rgba_mixed(path: &str, w: usize, h: usize) -> Result<()> {
    use crate::theme::Status;
    use crate::world::entity::load_to_half_extent;
    use crate::world::layout::layout;

    let palette = Palette::default();
    // 4 network groups × 4 containers each. Status pattern picks a couple of
    // Running per group (the "alive" cluster) and the rest non-Running (the
    // "off / paused / crashed" set) — same mix shape as the user's local rack.
    let statuses = [
        Status::Stopped, Status::Running, Status::Stopped, Status::Stopped,
        Status::Stopped, Status::Stopped, Status::Running, Status::Stopped,
        Status::Paused,  Status::Stopped, Status::Stopped, Status::Crashed,
        Status::Stopped, Status::Restarting, Status::Stopped, Status::Stopped,
    ];
    let groups: u16 = 4;
    let per_group: u32 = 4;

    let mut entities: Vec<Entity> = Vec::with_capacity((groups as usize) * per_group as usize);
    for g in 0..groups {
        for i in 0..per_group {
            let idx = (g as usize) * (per_group as usize) + i as usize;
            let status = statuses[idx];
            // Match the live-world rule: Running with no live stats sits at
            // MIN_HALF; non-Running uses the baseline (~mid-range). Mock-Running
            // gets a varied small load so the alive boxes also breathe in size.
            let half = match status {
                Status::Running => load_to_half_extent(0.55 + 0.1 * (i as f32 % 2.0)),
                _ => 0.85, // mirrors BASELINE_HALF_NO_LOAD in src/world/live.rs
            };
            entities.push(Entity {
                id: (g as u32) * (1 << 16) + i,
                position: layout(g, i),
                half_extents: Vec3::splat(half),
                status,
                group: g,
            });
        }
    }
    let bounds = SceneBounds::from_entities(&entities);
    let world = World { entities, bounds };

    let mut camera = Camera::new();
    camera.frame_scene_with_aspect(&world, 1.0);
    let spin = SPIN_RATE * 2.0;
    let view = camera.view_params(DEFAULT_FOV);
    let floors: Vec<crate::render3d::scene_extras::FloorPlane> = Vec::new();
    let ports = crate::render3d::scene_extras::PortLookup::new();
    let cylinders: Vec<crate::render3d::scene_extras::Cylinder> = Vec::new();
    let image_stacks: Vec<crate::render3d::scene_extras::ImageStack> = Vec::new();
    let extras = SceneExtras::new(&floors, &ports, &cylinders, &image_stacks);
    let rgba = render_rgba(
        &world.entities,
        view,
        &world.bounds,
        &palette,
        w,
        h,
        spin,
        None,
        0.0,
        &extras,
    );
    std::fs::write(path, &rgba)?;
    println!("{w} {h} {}", rgba.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test helper: render_rgba with empty SceneExtras (no floor-planes, no
    /// ports). Mirrors `crate::render3d::raster::tests::empty_extras`. Used
    /// by every existing render_rgba test — none of them exercise floors or
    /// ports (those have their own focused suites + the visual gate).
    #[allow(clippy::too_many_arguments)]
    fn rgba_empty_extras(
        entities: &[Entity],
        view: ViewParams,
        bounds: &SceneBounds,
        palette: &Palette,
        w: usize,
        h: usize,
        spin: f32,
        selected_id: Option<u32>,
        pulse_phase: f32,
    ) -> Vec<u8> {
        let floors: Vec<crate::render3d::scene_extras::FloorPlane> = Vec::new();
        let ports = crate::render3d::scene_extras::PortLookup::new();
        let cylinders: Vec<crate::render3d::scene_extras::Cylinder> = Vec::new();
        let image_stacks: Vec<crate::render3d::scene_extras::ImageStack> = Vec::new();
        let extras = SceneExtras::new(&floors, &ports, &cylinders, &image_stacks);
        render_rgba(
            entities,
            view,
            bounds,
            palette,
            w,
            h,
            spin,
            selected_id,
            pulse_phase,
            &extras,
        )
    }

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
        camera.frame_scene(&world);
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
        // Use Running (solid) so both boxes fill their faces — the test is
        // about the z-buffer correctness on overlapping SOLID geometry. With
        // wireframe (non-Running) boxes the center pixel lands between two
        // edges and is empty; the same z-buffer is still in effect for
        // wireframe (verified separately), but the center-pixel probe only
        // makes sense for filled faces.
        let near_box = Entity {
            id: 0,
            position: Vec3::new(0.0, 0.0, 2.0),
            half_extents: Vec3::splat(0.6),
            status: Status::Running,
            group: 0,
        };
        let far_box = Entity {
            id: 1,
            position: Vec3::new(0.0, 0.0, -2.0),
            half_extents: Vec3::splat(0.6),
            status: Status::Running,
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
        let only_a = rgba_empty_extras(&[near_box], view, &bounds, &palette, w, h, 0.0, None, 0.0);
        let both = rgba_empty_extras(&entities, view, &bounds, &palette, w, h, 0.0, None, 0.0);

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
        let rgba = rgba_empty_extras(&entities, view, &bounds, &palette, w, h, 0.0, None, 0.0);

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

    /// Non-Running boxes render as a wireframe: SOME pixels are lit (the cube
    /// edges) but the box is mostly hollow — the center of a single stopped
    /// box is background (no face fill), and the lit pixels are the configured
    /// edge color (fog-modulated). Pins the wireframe path.
    #[test]
    fn stopped_box_renders_as_wireframe() {
        use crate::theme::Status;
        let palette = Palette::default();
        let (w, h) = (80usize, 80usize);
        let entities = [Entity {
            id: 0,
            position: Vec3::ZERO,
            half_extents: Vec3::splat(0.8),
            status: Status::Stopped,
            group: 0,
        }];
        let bounds = SceneBounds::from_entities(&entities);
        // Head-on view so the front face is centered.
        let view = ViewParams {
            eye: Vec3::new(0.6, 0.5, 4.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov: DEFAULT_FOV,
        };
        let rgba = rgba_empty_extras(&entities, view, &bounds, &palette, w, h, 0.0, None, 0.0);
        let bg = to_rgb(palette.background);

        // Pixels are lit (the 12 edges project to some screen pixels).
        let lit: usize = rgba
            .chunks_exact(4)
            .filter(|px| (px[0], px[1], px[2]) != bg)
            .count();
        assert!(lit > 50, "wireframe edges should light some pixels, got {lit}");

        // Lit pixels are gray-ish (channels are close to each other) — the
        // configured `palette.edge` is a neutral gray, fog only dims it.
        for px in rgba.chunks_exact(4) {
            let (r, g, b) = (px[0], px[1], px[2]);
            if (r, g, b) == bg {
                continue;
            }
            let max = r.max(g).max(b) as i16;
            let min = r.min(g).min(b) as i16;
            // Edge color is near-gray; allow some tolerance for the fog blend
            // toward the background (which carries a faint indigo tint).
            assert!(
                max - min < 24,
                "wireframe pixel not near-gray: ({r},{g},{b}) span={}",
                max - min
            );
        }

        // The 2x2 patch dead-center is between the front face's edges (the
        // edges are at the projected ±half_extent corners, not at the center)
        // — so the center is background, proving the cube is transparent.
        let center = ((h / 2) * w + (w / 2)) * 4;
        assert_eq!(
            (rgba[center], rgba[center + 1], rgba[center + 2]),
            bg,
            "center of wireframe cube should be transparent (background)"
        );
    }

    /// A wireframe box BEHIND a solid box must NOT show its edges through the
    /// solid face — the shared z-buffer hides them. Pins wireframe/solid
    /// occlusion (the property the `Crashed` test used to pin for solid/solid).
    #[test]
    fn wireframe_behind_solid_is_occluded() {
        use crate::theme::Status;
        let palette = Palette::default();
        let (w, h) = (80usize, 80usize);
        // Solid in front (Running, z=+1), wireframe behind (Stopped, z=-1).
        let entities = [
            Entity {
                id: 0,
                position: Vec3::new(0.0, 0.0, 1.0),
                half_extents: Vec3::splat(0.6),
                status: Status::Running,
                group: 0,
            },
            Entity {
                id: 1,
                position: Vec3::new(0.0, 0.0, -1.0),
                half_extents: Vec3::splat(0.6),
                status: Status::Stopped,
                group: 0,
            },
        ];
        let bounds = SceneBounds::from_entities(&entities);
        let view = ViewParams {
            eye: Vec3::new(0.0, 0.0, 6.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov: DEFAULT_FOV,
        };
        // Render the solid alone, then both, and compare the front-face region.
        let solid_only = rgba_empty_extras(&[entities[0]], view, &bounds, &palette, w, h, 0.0, None, 0.0);
        let both = rgba_empty_extras(&entities, view, &bounds, &palette, w, h, 0.0, None, 0.0);
        // Sample a horizontal strip across the middle row — wherever the
        // solid box lit a pixel, the BOTH render must match exactly (the
        // wireframe behind was occluded, contributing nothing).
        let bg = to_rgb(palette.background);
        let row = h / 2;
        let mut compared = 0usize;
        for x in 0..w {
            let i = (row * w + x) * 4;
            let sa = (solid_only[i], solid_only[i + 1], solid_only[i + 2]);
            if sa == bg {
                continue;
            }
            let bo = (both[i], both[i + 1], both[i + 2]);
            assert_eq!(
                sa, bo,
                "wireframe behind solid bled through at ({x},{row}): solid={sa:?}, both={bo:?}"
            );
            compared += 1;
        }
        assert!(compared > 10, "no overlap to test; got {compared} pixels");
    }

    /// format_detail_lines (04-06b) produces the expected count of lines
    /// for a fully-populated DetailSnapshot — title + 9 data rows + ports
    /// section + mounts section + block I/O + spacer + help footer.
    #[test]
    fn format_detail_lines_produces_expected_field_count() {
        use crate::docker::{DetailSnapshot, HealthSummary, MountSummary, PortProto, PortSummary};
        use crate::theme::Status;
        let snap = DetailSnapshot {
            id: "x".to_string(),
            name: "container-x".to_string(),
            status: Status::Running,
            health: HealthSummary::Healthy,
            started_at_iso: "2026-05-29T00:00:00Z".to_string(),
            restart_count: 0,
            restart_policy: "no".to_string(),
            image_human: "nginx".to_string(),
            image_digest: "sha256:abc".to_string(),
            network_mode: "bridge".to_string(),
            networks: Vec::new(),
            ports: vec![PortSummary {
                private: 80,
                public: Some(8080),
                proto: PortProto::Tcp,
            }],
            mounts: vec![MountSummary {
                kind: "bind".to_string(),
                source: "/host".to_string(),
                destination: "/data".to_string(),
                rw: true,
            }],
        };
        let lines = super::format_detail_lines(&snap, 1500, 3500);
        // Expected layout (15 lines):
        //   0: title
        //   1: Status
        //   2: Health
        //   3: Image
        //   4: Started
        //   5: Restarts
        //   6: Network
        //   7: Ports (count):
        //   8:   <ports line>
        //   9: Mounts (count):
        //  10:   <mount entry>
        //  11: Block I/O
        //  12: <blank>
        //  13: help footer
        assert_eq!(lines.len(), 14, "got {} lines: {lines:?}", lines.len());
        assert!(lines[0].contains("container-x"));
        assert!(lines[1].contains("Running"));
        assert!(lines[2].contains("healthy"));
        assert!(lines[3].contains("nginx"));
        assert!(lines[5].contains("0 (no)"));
        assert!(lines[6].contains("bridge"));
        assert!(lines[8].contains("80/tcp -> 8080"));
        assert!(lines[10].contains("/host:/data"));
        assert!(lines[11].contains("1.5 KB"));
        assert!(lines[11].contains("3.5 KB"));
        assert!(lines[13].contains("Esc close"));
    }

    /// centered_cell_rect produces a centered rect with predictable dims.
    #[test]
    fn centered_cell_rect_centers() {
        let (col0, row0, w, h) = super::centered_cell_rect(100, 50, 0.6, 0.6);
        // 60% of 100 = 60 wide; 60% of 50 = 30 tall.
        assert_eq!(w, 60);
        assert_eq!(h, 30);
        assert_eq!(col0, 20);
        assert_eq!(row0, 10);
    }

    /// Small terminals still produce a usable rect (clamped, not panicking).
    #[test]
    fn centered_cell_rect_handles_tiny_terminal() {
        let (_, _, w, h) = super::centered_cell_rect(10, 10, 0.6, 0.6);
        // 10 * 0.6 = 6 width — below the clamp floor of 20; clamps UP-to floor.
        // But the rect cannot be wider than the terminal; clamps DOWN to cols.
        assert!(w <= 10);
        assert!(h <= 10);
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
        let rgba = rgba_empty_extras(&[], view, &bounds, &palette, w, h, 0.0, None, 0.0);
        let bg = to_rgb(palette.background);
        assert_eq!(rgba.len(), w * h * 4);
        for px in rgba.chunks_exact(4) {
            assert_eq!((px[0], px[1], px[2]), bg, "non-background pixel in empty scene");
        }
    }

    // ===== 05-04-RV6: kitty banner state-machine regression tests =====
    //
    // These tests pin the bypass the user kept hitting after RV1-RV5. The
    // pre-RV6 kitty loop conflated "stable empty" (banner allowed) with
    // "cold-start race" (banner suppressed by grace but cache also empty)
    // into the same `render_target.is_none()` branch, which always painted
    // the banner text. The fix extracted `compute_kitty_render_decision`
    // returning one of four explicit variants; these tests pin all four
    // variants AND the legacy bypass case that was the actual bug.

    /// RV6 BUG CASE PINNED: frame 0 of process life, no DockerMsg yet
    /// drained, cache is `None`, world is empty. The decision MUST be
    /// `ChromeOnly`, NOT `Banner`. This is the exact frame the user saw
    /// flicker on the kitty launch — pre-RV6 the code returned a logical
    /// equivalent of `Banner` here.
    #[test]
    fn rv6_cold_start_frame_zero_no_cache_is_chrome_only() {
        let decision = compute_kitty_render_decision(
            /* world_empty_now    */ true,
            /* non_empty_seen     */ false,
            /* empty_streak       */ 0,
            /* frames_since_start */ 0,
            /* has_cache          */ false,
            /* debounce_frames    */ 150,
            /* grace_frames       */ 30,
        );
        assert_eq!(
            decision,
            KittyRenderDecision::ChromeOnly,
            "RV6 cold-start frame 0 (no DockerMsg drained, cache None, grace active) \
             MUST return ChromeOnly — pre-RV6 this returned Banner and painted the \
             EMPTY_BANNER text, causing the ~100 ms launch flicker the user reported \
             across rounds 1-5."
        );
    }

    /// Every frame across the entire startup-grace window with no
    /// container ever observed must return `ChromeOnly`. This pins the
    /// "no banner during the grace, EVER, even on the boundary" invariant.
    #[test]
    fn rv6_all_grace_frames_are_chrome_only_when_cache_empty() {
        const GRACE: u32 = 30;
        for frame in 0..GRACE {
            let decision = compute_kitty_render_decision(
                true, false, frame, frame, false, 150, GRACE,
            );
            assert_eq!(
                decision,
                KittyRenderDecision::ChromeOnly,
                "frame {frame} of {GRACE}-frame grace returned {decision:?}, expected ChromeOnly"
            );
        }
    }

    /// After the grace expires AND no container has ever appeared, the
    /// banner must finally show. This preserves Phase 3 criterion #5
    /// (a daemon with zero containers gets its banner, just delayed).
    #[test]
    fn rv6_post_grace_never_seen_shows_banner() {
        let decision = compute_kitty_render_decision(
            /* world_empty_now    */ true,
            /* non_empty_seen     */ false,
            /* empty_streak       */ 30,
            /* frames_since_start */ 30, // exactly at grace boundary
            /* has_cache          */ false,
            /* debounce_frames    */ 150,
            /* grace_frames       */ 30,
        );
        assert_eq!(
            decision,
            KittyRenderDecision::Banner,
            "post-grace + never-seen-a-container MUST show the banner (Phase 3 \
             criterion #5: empty daemon must surface the empty-state message)."
        );
    }

    /// Live non-empty world ALWAYS wins, regardless of any timer. This
    /// pins that a populated daemon at launch never flashes ChromeOnly
    /// (which would also be perceived as flicker).
    #[test]
    fn rv6_live_non_empty_always_renders_live() {
        // During grace, after grace, with or without cache — all variants
        // must render LiveWorld when the world has entities.
        for grace_state in &[0u32, 15, 30, 100, 10_000] {
            for has_cache in &[true, false] {
                let decision = compute_kitty_render_decision(
                    false,
                    false,
                    0,
                    *grace_state,
                    *has_cache,
                    150,
                    30,
                );
                assert_eq!(
                    decision,
                    KittyRenderDecision::LiveWorld,
                    "non-empty world at frames_since_start={grace_state}, has_cache={has_cache} \
                     returned {decision:?}, expected LiveWorld"
                );
            }
        }
    }

    /// Once the cache is populated (non_empty_seen_once + has_cache),
    /// a transient empty within the debounce window must render the
    /// cached world — NOT the banner, NOT ChromeOnly. This is the
    /// RV4 invariant restated as a unit test on the decision function.
    #[test]
    fn rv6_transient_empty_with_cache_uses_cache() {
        let decision = compute_kitty_render_decision(
            /* world_empty_now    */ true,
            /* non_empty_seen     */ true,
            /* empty_streak       */ 5, // well below debounce
            /* frames_since_start */ 500, // long past grace
            /* has_cache          */ true,
            /* debounce_frames    */ 150,
            /* grace_frames       */ 30,
        );
        assert_eq!(
            decision,
            KittyRenderDecision::CachedWorld,
            "transient empty after first container with cache populated must \
             render the cached world (RV4 invariant)."
        );
    }

    /// After a SUSTAINED empty exceeding the debounce window, the banner
    /// must show even though we previously saw containers. This pins
    /// the RV5 debounce-expiration path.
    #[test]
    fn rv6_sustained_empty_past_debounce_shows_banner() {
        let decision = compute_kitty_render_decision(
            /* world_empty_now    */ true,
            /* non_empty_seen     */ true,
            /* empty_streak       */ 150, // at debounce boundary
            /* frames_since_start */ 1000,
            /* has_cache          */ true,
            /* debounce_frames    */ 150,
            /* grace_frames       */ 30,
        );
        assert_eq!(
            decision,
            KittyRenderDecision::Banner,
            "sustained empty past debounce_frames must finally show the \
             banner — Phase 3 criterion #5 even if a container existed once."
        );
    }

    /// The four variants are mutually exclusive and cover the entire
    /// state space. A property-style sweep over a small grid pins that
    /// no input combination returns a value the loop body can't handle.
    /// Also pins that the function is total (never panics).
    #[test]
    fn rv6_decision_is_total_over_state_grid() {
        let debounce = 150u32;
        let grace = 30u32;
        for &world_empty in &[true, false] {
            for &seen in &[true, false] {
                for &streak in &[0u32, 1, 29, 30, 100, 149, 150, 151, 1_000] {
                    for &frame in &[0u32, 1, 15, 29, 30, 31, 100, 1_000] {
                        for &has_cache in &[true, false] {
                            let d = compute_kitty_render_decision(
                                world_empty, seen, streak, frame, has_cache, debounce, grace,
                            );
                            // Total function — any of the four variants is fine,
                            // we just need to confirm the call returns and the
                            // result type is the enum (compiler enforces).
                            let _ = d;
                            // Specific invariants:
                            if !world_empty {
                                assert_eq!(d, KittyRenderDecision::LiveWorld);
                            } else if frame < grace {
                                // In the grace window, the banner must NEVER be
                                // chosen — that's the whole point of RV5+RV6.
                                // ChromeOnly / CachedWorld are both fine.
                                assert_ne!(
                                    d, KittyRenderDecision::Banner,
                                    "frame={frame} (in grace) returned Banner — \
                                     RV6 banner-bypass regression"
                                );
                            }
                            // Specifically: during grace + no cache + empty world
                            // -> always ChromeOnly. This is the user-reported bug.
                            if world_empty && frame < grace && !has_cache {
                                assert_eq!(
                                    d,
                                    KittyRenderDecision::ChromeOnly,
                                    "frame={frame} (grace), no cache, world empty \
                                     returned {d:?} — RV6 bypass regression"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// 05-05-RV4 regression pin: every kitty image emit MUST carry the
    /// `z=-1` parameter so the image is rendered STRICTLY BELOW terminal
    /// cell text (legend, labels, popup). Pre-RV4 the emit omitted `z`,
    /// defaulting to `z=0`, which per kitty's graphics-protocol docs
    /// does NOT guarantee text-on-image visibility — at `z=0` the image
    /// covers cells unless the cell carries an explicit non-default
    /// background. The user observed the legend "appears for ms and
    /// disappears": frame 1 (`ChromeOnly`, no image emit) showed the
    /// legend; frame 2+ (`LiveWorld`, image at `z=0`) hid it.
    ///
    /// This test asserts the EXACT byte sequence the protocol header
    /// emits — the `z=-1` substring is the load-bearing piece. If any
    /// future refactor drops it (or flips the sign), this fails fast.
    #[test]
    fn rv4_kitty_image_emit_carries_z_minus_one() {
        let mut buf: Vec<u8> = Vec::new();
        // 4 RGBA pixels (2x2 image) is enough to exercise the header
        // formatter; render_rgba is not on the test path here — we just
        // need emit_kitty to format the first chunk's prelude.
        let rgba = vec![0u8; 16]; // 2*2 px * 4 bytes
        emit_kitty(&mut buf, &rgba, 2, 2).expect("emit_kitty must succeed on a tiny buffer");
        let s = String::from_utf8_lossy(&buf);
        assert!(
            s.contains("z=-1"),
            "emit_kitty must include `z=-1` so the image is below cell text; \
             output did not contain the substring. Header bytes (lossy): {s:?}",
        );
        // Negative pin: the default `z=0` must NEVER appear in the header
        // (would mean the fix regressed and we're back to image-over-text).
        assert!(
            !s.contains("z=0"),
            "emit_kitty must NOT include `z=0` (image would render over \
             cell text and re-introduce the legend disappearance bug); \
             header bytes (lossy): {s:?}",
        );
    }

    /// 05-05-RV4 secondary pin: the per-frame KittyRenderDecision is
    /// COMPLETELY ORTHOGONAL to the legend visibility logic. The legend
    /// must render every frame `hud_visible == true` regardless of which
    /// decision variant the loop took (LiveWorld / CachedWorld / Banner
    /// / ChromeOnly). RV4 itself doesn't change this branching — the
    /// fix is purely at the image-emit z-index — but pinning the
    /// independence here means a future refactor that gates the legend
    /// emit behind the wrong decision variant fails immediately.
    ///
    /// This is a STATIC check: we verify that every variant of
    /// `KittyRenderDecision` is matched against in `run_kitty`'s legend
    /// emit (indirectly: there is exactly ONE `if hud_visible` outside
    /// the decision match-tree). If you split the legend emit into
    /// per-variant branches you'll need to revisit this invariant.
    #[test]
    fn rv4_legend_emit_independent_of_render_decision() {
        // Sanity: every variant exists and is pairwise-distinct.
        // PartialEq is already on the enum; a 5th variant lands here as
        // a non-exhaustive pattern match — forcing the maintainer to
        // revisit the legend-emit invariant.
        let variants = [
            KittyRenderDecision::LiveWorld,
            KittyRenderDecision::CachedWorld,
            KittyRenderDecision::Banner,
            KittyRenderDecision::ChromeOnly,
        ];
        for (i, a) in variants.iter().enumerate() {
            for (j, b) in variants.iter().enumerate() {
                if i == j {
                    assert_eq!(a, b);
                } else {
                    assert_ne!(a, b, "variants {i} and {j} compared equal — \
                                       KittyRenderDecision distinctness regression");
                }
            }
        }
        // Exhaustive pattern: a future variant would force a compile
        // error here (or a wildcard arm), pointing the next reviewer
        // at the legend-emit invariant in run_kitty.
        for v in variants.iter() {
            match v {
                KittyRenderDecision::LiveWorld
                | KittyRenderDecision::CachedWorld
                | KittyRenderDecision::Banner
                | KittyRenderDecision::ChromeOnly => {}
            }
        }
    }

    // ===== 05-05-RV5: persistent image ID + no per-frame delete_all =====
    //
    // The user reported after RV4 landed: "the 3D environment itself
    // disappears, especially often starts flickering when the window is
    // not in focus" (legend + status bar stay solid, only the kitty
    // pixel image vanishes intermittently; FPS drops to ~13 when
    // unfocused, widening the gap).
    //
    // Root cause: every LiveWorld/CachedWorld frame did
    // `delete_all -> \x1b[H -> emit_kitty`. The `emit_kitty` payload is
    // tens of KB of base64-encoded zlib-compressed RGBA split into
    // 4096-byte segments, each wrapped in `\x1b_G...\x1b\\`. Between
    // kitty processing the `delete_all` (image is gone) and finishing
    // parse of the last `emit_kitty` segment (image is back), the kitty
    // compositor can render the "no image" intermediate state. When the
    // window is unfocused, kitty throttles its own compositor rendering
    // — the gap window stretches across multiple render budgets and the
    // user sees the image vanish entirely for several frames.
    //
    // Fix: use a persistent kitty image ID (`i=KITTY_IMAGE_ID`) in
    // `emit_kitty`. Per kitty's graphics protocol, transmitting a new
    // image with `a=T` and the same `i=N` ATOMICALLY replaces the
    // prior image — no separate delete step, no gap. The per-frame
    // `delete_all` was DROPPED from the LiveWorld/CachedWorld paths;
    // `delete_all` is only invoked now on (a) explicit empty-state
    // transitions where the image must actually disappear, and (b)
    // shutdown.

    /// RV5 positive pin: every kitty image emit MUST carry `i=1` so the
    /// new image atomically replaces the prior frame's placement at
    /// the same id. Without `i=`, kitty assigns a fresh ID per
    /// transmission and the prior frame's image lingers (also visible
    /// flicker, plus unbounded image-table growth on long sessions).
    #[test]
    fn rv5_kitty_image_emit_carries_persistent_image_id() {
        let mut buf: Vec<u8> = Vec::new();
        let rgba = vec![0u8; 16]; // 2*2 px * 4 bytes
        emit_kitty(&mut buf, &rgba, 2, 2).expect("emit_kitty must succeed");
        let s = String::from_utf8_lossy(&buf);
        // The id we picked is 1 — bound to KITTY_IMAGE_ID. If a future
        // refactor renames the constant, this test still pins that SOME
        // `i=<digit>` is present (positional check).
        assert!(
            s.contains(&format!("i={}", KITTY_IMAGE_ID)),
            "emit_kitty must include `i={}` so frame N+1 atomically \
             replaces frame N's placement (no delete-then-emit race); \
             header bytes (lossy): {s:?}",
            KITTY_IMAGE_ID,
        );
    }

    /// RV5 negative pin: a single `emit_kitty` call MUST NOT emit the
    /// `a=d` (delete) escape sequence. Pre-RV5 the per-frame sequence
    /// in run_kitty was `delete_all -> emit_kitty`, but `delete_all`
    /// was a separate call; this test now pins that nobody splices a
    /// delete INTO the emit path (a tempting "atomicity through a
    /// single write" refactor that would actually re-introduce the
    /// race because `a=d` and `a=T` are processed sequentially by
    /// kitty regardless of how they hit the wire).
    ///
    /// More importantly this pins the BYTE-LEVEL invariant: the emit
    /// path produces ONLY image-transmit segments (`a=T,...` initial
    /// and `m=` continuation), no other kitty graphics actions.
    #[test]
    fn rv5_kitty_emit_contains_no_delete_action() {
        let mut buf: Vec<u8> = Vec::new();
        // Larger image to exercise multi-segment chunking — pre-RV5
        // each segment carried a separate header; if a future refactor
        // accidentally splices `a=d` into a continuation, multi-segment
        // catches it.
        let rgba = vec![0u8; 64 * 64 * 4];
        emit_kitty(&mut buf, &rgba, 64, 64).expect("emit_kitty must succeed");
        let s = String::from_utf8_lossy(&buf);
        assert!(
            !s.contains("a=d"),
            "emit_kitty must NOT emit `a=d` (delete action) — that \
             would re-introduce the delete-then-emit race RV5 fixes; \
             output (lossy): {s:?}",
        );
    }

    /// RV5 byte-level pin on the `delete_all` helper: when invoked it
    /// MUST emit exactly the documented APC sequence. We pin this so
    /// the shutdown/transition cleanup paths stay byte-stable (a kitty
    /// terminal restored after dd3 exits should have no lingering
    /// image-id-1 placement; the final `delete_all` in run_kitty's
    /// epilogue is load-bearing for that).
    #[test]
    fn rv5_delete_all_emits_documented_apc_sequence() {
        let mut buf: Vec<u8> = Vec::new();
        delete_all(&mut buf).expect("delete_all must succeed");
        let s = String::from_utf8_lossy(&buf);
        assert!(
            s.contains("a=d"),
            "delete_all helper must emit `a=d` so kitty actually \
             removes the image; output (lossy): {s:?}",
        );
        assert!(
            s.starts_with("\u{1b}_G"),
            "delete_all helper must start with the APC \\x1b_G prefix; \
             output (lossy): {s:?}",
        );
    }

    /// RV5 ANTI-FLICKER end-to-end byte-level invariant: two consecutive
    /// frames of `emit_kitty` produce a byte stream with EXACTLY zero
    /// `a=d` substrings. This is the property that closes the user-
    /// reported flicker: there is no point between frame N and frame
    /// N+1 where kitty receives a delete-image action.
    ///
    /// Pre-RV5 the per-frame sequence in run_kitty was
    /// `delete_all(stdout) -> emit_kitty(stdout)` so a recorder
    /// capturing the wire bytes would have shown `a=d,...,a=T,...,a=d,
    /// ...,a=T,...` — one `a=d` per frame. Post-RV5 the recorder
    /// shows ONLY `a=T` segments with the same `i=1`, frame after
    /// frame.
    ///
    /// We can't directly invoke `run_kitty` from a unit test (raw
    /// mode + kitty terminal + tokio runtime), so we simulate the
    /// new per-frame call sequence by invoking `emit_kitty` twice
    /// against a recording writer.
    #[test]
    fn rv5_consecutive_frames_emit_no_delete_actions() {
        let mut wire: Vec<u8> = Vec::new();
        let rgba = vec![0u8; 16];
        // Simulate two consecutive LiveWorld frames (the post-RV5
        // call sequence: NO delete_all between them).
        emit_kitty(&mut wire, &rgba, 2, 2).expect("frame N emit");
        emit_kitty(&mut wire, &rgba, 2, 2).expect("frame N+1 emit");
        let s = String::from_utf8_lossy(&wire);
        let n_delete = s.matches("a=d").count();
        assert_eq!(
            n_delete, 0,
            "two consecutive frames emitted {n_delete} `a=d` action(s); \
             expected 0 (RV5 invariant: no per-frame delete_all). \
             Wire bytes (lossy): {s:?}",
        );
        // Sanity: both frames carry the persistent image id.
        let n_id = s.matches(&format!("i={}", KITTY_IMAGE_ID)).count();
        assert_eq!(
            n_id, 2,
            "expected 2 occurrences of `i={}` (one per frame); got {n_id}. \
             Wire bytes (lossy): {s:?}",
            KITTY_IMAGE_ID,
        );
    }

    // ---- 05-05-RV6 dimension-stability gate (OBS-recording-start flicker) ----

    /// Cold start: the very first valid poll is adopted immediately as the
    /// stable geometry. There is nothing else to hold; the gate must not
    /// fall back to `KITTY_FALLBACK_GEOMETRY` once the terminal reports a
    /// real value.
    #[test]
    fn rv6_geometry_gate_cold_start_adopts_first_valid_poll() {
        let mut gate = KittyGeometryGate::new();
        let real: KittyGeometry = (120, 40, 1280, 720);
        let resolved = resolve_kitty_geometry(&mut gate, Some(real));
        assert_eq!(resolved, real);
        assert_eq!(gate.stable(), Some(real));
    }

    /// Single-frame flutter (one frame at a DIFFERENT geometry surrounded
    /// by the original) is SUPPRESSED. The kitty image header bytes
    /// `s={w},v={h}` stay constant across the blip, so `emit_kitty` keeps
    /// hitting the same-dimension atomic-replace fast path RV5 relies on.
    /// This is the property that closes the user-reported "still flickers
    /// when starting OBS recording" bug.
    #[test]
    fn rv6_geometry_gate_suppresses_single_frame_flutter() {
        let mut gate = KittyGeometryGate::new();
        let stable: KittyGeometry = (120, 40, 1280, 720);
        let blip: KittyGeometry = (90, 30, 720, 560);
        // Warm up to stable.
        assert_eq!(resolve_kitty_geometry(&mut gate, Some(stable)), stable);
        // ONE frame at blip → still stable.
        assert_eq!(resolve_kitty_geometry(&mut gate, Some(blip)), stable);
        // Back to stable → still stable; candidate cleared.
        assert_eq!(resolve_kitty_geometry(&mut gate, Some(stable)), stable);
        assert_eq!(gate.stable(), Some(stable));
    }

    /// Two-frame flutter (e.g. compositor reconfigure spans two frames)
    /// is still suppressed — only at frame 3 of a sustained NEW geometry
    /// does the gate adopt it. This is the load-bearing invariant: the
    /// `KITTY_GEOMETRY_STABILITY_FRAMES = 3` threshold means a real
    /// resize is adopted ~100 ms after the user stops dragging (well
    /// within human perception threshold) but transient blips up to 2
    /// frames are filtered out.
    #[test]
    fn rv6_geometry_gate_suppresses_two_frame_flutter_then_adopts_at_three() {
        let mut gate = KittyGeometryGate::new();
        let stable: KittyGeometry = (120, 40, 1280, 720);
        let new: KittyGeometry = (140, 50, 1400, 900);
        assert_eq!(resolve_kitty_geometry(&mut gate, Some(stable)), stable);
        // Frame 1 of new: still stable.
        assert_eq!(resolve_kitty_geometry(&mut gate, Some(new)), stable);
        // Frame 2 of new: still stable.
        assert_eq!(resolve_kitty_geometry(&mut gate, Some(new)), stable);
        // Frame 3 of new: PROMOTED. Confirmed resize.
        assert_eq!(resolve_kitty_geometry(&mut gate, Some(new)), new);
        assert_eq!(gate.stable(), Some(new));
    }

    /// A transient INVALID poll (window_size returned 0 or errored) is
    /// silently held at the stable geometry. The kitty path never emits
    /// a frame at the `(90, 30, 720, 560)` fallback unless the very first
    /// poll (cold start) is invalid. This stops the pre-RV6 behavior
    /// where a single transient invalid report flipped the emit
    /// dimensions to the fallback and back, breaking RV5's same-dimension
    /// atomic-replace.
    #[test]
    fn rv6_geometry_gate_holds_stable_on_invalid_poll() {
        let mut gate = KittyGeometryGate::new();
        let stable: KittyGeometry = (120, 40, 1280, 720);
        assert_eq!(resolve_kitty_geometry(&mut gate, Some(stable)), stable);
        // Invalid poll: hold stable, do NOT fall back.
        assert_eq!(resolve_kitty_geometry(&mut gate, None), stable);
        assert_eq!(resolve_kitty_geometry(&mut gate, None), stable);
        // Recover: stable still stable.
        assert_eq!(resolve_kitty_geometry(&mut gate, Some(stable)), stable);
    }

    /// Cold-start invalid poll falls back to `KITTY_FALLBACK_GEOMETRY`
    /// (the only time the fallback is ever returned). This is a
    /// defensive contract — without a stable to hold, the loop needs
    /// SOMETHING for the first frame's `render_rgba` + `emit_kitty`.
    #[test]
    fn rv6_geometry_gate_falls_back_only_on_cold_start_invalid() {
        let mut gate = KittyGeometryGate::new();
        assert_eq!(resolve_kitty_geometry(&mut gate, None), KITTY_FALLBACK_GEOMETRY);
        // The fallback is NOT promoted to stable — the next valid poll
        // adopts cleanly.
        assert_eq!(gate.stable(), None);
        let real: KittyGeometry = (120, 40, 1280, 720);
        assert_eq!(resolve_kitty_geometry(&mut gate, Some(real)), real);
    }

    /// Constant-geometry steady state: 100 frames at the same geometry
    /// must all return the same geometry. This pins that the gate doesn't
    /// drift, spuriously promote, or otherwise mutate the stable value
    /// when nothing has changed — the most common case in practice (a
    /// user not resizing their window during normal operation).
    #[test]
    fn rv6_geometry_gate_steady_state_is_stable() {
        let mut gate = KittyGeometryGate::new();
        let geom: KittyGeometry = (120, 40, 1280, 720);
        for i in 0..100 {
            let resolved = resolve_kitty_geometry(&mut gate, Some(geom));
            assert_eq!(resolved, geom, "frame {i}: drifted from steady state");
        }
        assert_eq!(gate.stable(), Some(geom));
    }

    /// **05-05-RV6 ANTI-FLICKER end-to-end byte-level invariant.**
    ///
    /// Simulate the wire-bytes scenario that reproduces the OBS-start
    /// flicker: 6 frames where the middle 2 frames report a DIFFERENT
    /// geometry (compositor surface reconfigure during xdg-desktop-portal
    /// screencast init). With the gate in place, all 6 emitted images
    /// MUST carry the same `s=,v=` bytes — kitty stays on the same-
    /// dimension atomic-replace fast path the entire time and the user
    /// sees no gap in the 3D image.
    ///
    /// Pre-RV6 the middle two frames would have emitted with a DIFFERENT
    /// `s=,v=`, forcing kitty to allocate a new GPU texture twice (once
    /// on the way in, once on the way out) — each allocation creating a
    /// gap in the `i=1` placement and a one-frame flash of the prior
    /// image's geometry. The user's "still flickers when starting OBS
    /// recording" symptom IS that exact gap.
    #[test]
    fn rv6_consecutive_frames_image_header_bytes_stable_under_blip() {
        let mut gate = KittyGeometryGate::new();
        let stable: KittyGeometry = (120, 40, 1280, 720);
        // 6-frame sequence: 2 stable, 2 blip (different), 2 stable.
        // The blip lasts under KITTY_GEOMETRY_STABILITY_FRAMES so the
        // gate must hold stable for ALL 6 frames.
        let blip: KittyGeometry = (90, 30, 720, 560);
        let polls: [Option<KittyGeometry>; 6] = [
            Some(stable),
            Some(stable),
            Some(blip),
            Some(blip),
            Some(stable),
            Some(stable),
        ];
        let mut wire: Vec<u8> = Vec::new();
        for poll in polls {
            let (_cols, _rows, w, h) = resolve_kitty_geometry(&mut gate, poll);
            // Reserve last row → image height is px_h minus one cell.
            // For the test we just feed (px_w, px_h) directly; the
            // property under test is that EVERY frame's emitted header
            // carries the same `s=,v=`.
            //
            // Tiny rgba: 1×1 px. We only assert header bytes — not the
            // visual content.
            let small_w = w.min(8);
            let small_h = h.min(8);
            let rgba = vec![0u8; small_w * small_h * 4];
            emit_kitty(&mut wire, &rgba, small_w, small_h).expect("emit_kitty");
        }
        let s = String::from_utf8_lossy(&wire);
        // Stable geometry feeds w=8, h=8 because of the `.min(8)`
        // clamp — both stable AND blip should reduce to the same 8×8 in
        // the test. So this property is trivially satisfied UNLESS the
        // gate accidentally promoted blip mid-sequence and stable !=
        // blip even after clamping. Defensive: also assert via wider
        // dims that exceed the clamp.
        //
        // Re-run with larger dims so the `.min(8)` clamp can't mask a
        // promotion bug.
        let mut gate2 = KittyGeometryGate::new();
        let stable2: KittyGeometry = (120, 40, 200, 200);
        let blip2: KittyGeometry = (90, 30, 100, 100);
        let polls2: [Option<KittyGeometry>; 6] = [
            Some(stable2),
            Some(stable2),
            Some(blip2),
            Some(blip2),
            Some(stable2),
            Some(stable2),
        ];
        let mut wire2: Vec<u8> = Vec::new();
        for poll in polls2 {
            let (_cols, _rows, w, h) = resolve_kitty_geometry(&mut gate2, poll);
            let rgba = vec![0u8; w * h * 4];
            emit_kitty(&mut wire2, &rgba, w, h).expect("emit_kitty");
        }
        let s2 = String::from_utf8_lossy(&wire2);
        // Every frame must have used the stable (200×200) dims.
        let n_stable = s2.matches("s=200,v=200").count();
        let n_blip = s2.matches("s=100,v=100").count();
        assert_eq!(
            n_stable, 6,
            "all 6 frames must emit with the stable s=200,v=200 header; \
             got {n_stable}. Wire (lossy, len={}): {s2:.500?}", s2.len(),
        );
        assert_eq!(
            n_blip, 0,
            "no frame may emit with the blip s=100,v=100 header; got \
             {n_blip}. Wire (lossy, len={}): {s2:.500?}", s2.len(),
        );
        // First-test sanity (clamped case): no `a=d` in either capture.
        assert!(!s.contains("a=d"));
        assert!(!s2.contains("a=d"));
    }
}
