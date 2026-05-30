//! The view layer: a pure `view(frame, &app)` that lays out the screen.
//!
//! Layout is always derived from the live `frame.area()` so a resize never
//! reads a stale cached size (Pitfall #14). The scene area hosts the braille
//! Canvas that renders the orbiting cube — OR, when the live World is empty,
//! a centered plain-text "no containers" banner (so an idle Docker host doesn't
//! render a blank void, per Phase 3 criterion #5).

pub mod detail_panel;
pub mod labels;
pub mod legend;
pub mod scene;
pub mod status_bar;

use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::App;

/// The empty-state banner text. Centralized so future tests can grep for it and
/// the kitty backend can mirror the exact same string (criterion #5).
pub const EMPTY_BANNER: &str = "No containers running — start one and it'll appear here.";

/// Render the whole UI for one frame. Synchronous; takes `&mut App` so the
/// label hysteresis state (`Selection::last_label_cell`) can be mutated AT
/// the snap-and-decide step BEFORE the `Canvas::paint` closure is built (the
/// closure itself is `Fn`-bound and only sees Copy primitives).
pub fn view(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    // Vertical split: scene fills the screen above a 1-row status bar.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    let scene_area = chunks[0];
    let status_area = chunks[1];

    // Hot-swappable palette read from `app.palette` (05-04 THEME-04). The
    // field is `Copy` (see src/theme/mod.rs Palette derive) so the byte
    // copy here is trivial. Cycled at runtime by Effect::CyclePalette.
    let palette = app.palette;

    // 05-04-RV4: the view picks between the LIVE empty world and the
    // CACHED last-non-empty world based on the debounce-window state. We
    // borrow-split here so `scene::render_scene` can take `&mut app.selection`
    // later while the chosen world borrow stays alive — calling
    // `app.effective_world_for_view()` would lock the entire `app` immutably.
    // The decision below mirrors `App::effective_world_for_view` exactly.
    //
    // During the empty-banner debounce window we render the cached
    // snapshot so the scene chrome stays visually continuous through
    // transient empties (e.g. `docker rm -f` + `docker run` churn). RV2's
    // earlier fix returned only an empty bordered block during the
    // window — the user still saw a blank flash because the 3D content
    // vanished. RV4 paints the cached scene instead, so the boxes hold
    // position until the debounce expires or new containers arrive.
    let live_empty = app.world.entities.is_empty();
    let in_debounce = live_empty && !app.should_show_empty_banner();
    let effective_world: &crate::world::World = if in_debounce {
        app.last_non_empty_world.as_ref().unwrap_or(&app.world)
    } else {
        &app.world
    };
    if effective_world.entities.is_empty() {
        // No content to retain (first launch with no cache, OR debounce
        // expired on a stable-empty daemon). The DEBOUNCED decision lives
        // on App: `should_show_empty_banner` returns true at first launch
        // (no `non_empty_seen_once`) and when the debounce window has
        // elapsed. Phase 3 criterion #5 preserved.
        if app.should_show_empty_banner() {
            render_empty_banner(frame, scene_area, &palette);
        } else {
            // Empty world + cache also empty (shouldn't happen in
            // practice: should_show_empty_banner would have returned true
            // at first-launch and the cache is populated whenever
            // non_empty_seen_once flips). Defensive fallback to the
            // bordered block — still no flash because the world has been
            // empty since startup.
            let block = ratatui::widgets::Block::default()
                .title("scene")
                .borders(ratatui::widgets::Borders::ALL);
            frame.render_widget(block, scene_area);
        }
    } else {
        // The orbiting World of boxes renders into the braille Canvas in the
        // scene area. Palette is the single color source; the camera (framed
        // to the scene on entity-count changes by `App::run`) feeds
        // render_scene a ViewParams. The app owns the World — the single
        // source of truth.
        //
        // 04-04: render_scene also reads `live` + `&mut selection` for the
        // floor-plane palette decision, the per-entity port lookup, and the
        // selected-only label's hysteresis update.
        //
        // Note: during the RV4 debounce-with-cache path, `effective_world`
        // is the cached snapshot — boxes "freeze" at the last seen position
        // for up to 1 second while the daemon settles. live + selection are
        // still read from `app` (they reflect the LIVE state); a selected
        // id that no longer exists in the cached world is harmless (the
        // pulse path just skips it).
        scene::render_scene(
            frame,
            scene_area,
            &app.camera,
            effective_world,
            &app.live,
            &mut app.selection,
            &palette,
            &app.render_config,
            app.spin,
        );
    }

    status_bar::render(frame, status_area, app);

    // Legend HUD (THEME-05). Rendered AFTER the scene + status bar BUT
    // BEFORE the detail-panel popup so a (centered, large) popup overlays
    // a (top-right, small) legend when both are open. `legend_rect`
    // auto-skips on tiny terminals (returns None), so a 40-col window
    // simply omits the overlay rather than panicking.
    //
    // The legend reads `app.palette` + `app.palette_name` directly, the
    // same fields THEME-04's `Effect::CyclePalette` swaps — so cycling
    // the palette with P updates the legend's swatches AND title on the
    // next frame (no separate notification path).
    //
    // Visible across ALL view branches (empty + cached + populated) by
    // virtue of running here, AFTER the body of view picks its branch:
    // there's no fourth bypass path that skips the overlay.
    if app.hud_visible {
        if let Some(legend_area) = legend::legend_rect(scene_area) {
            legend::render_legend(frame, legend_area, &app.palette, &app.palette_name);
        }
    }

    // CAM-05 / 04-06b: popup overlay. Drawn LAST so the Clear + Block
    // hides the scene underneath in the popup's footprint. Block I/O
    // (`blkio_r` / `blkio_w`) is read from `LiveWorld::last_sample` EVERY
    // frame — the popup reflects the freshest counters even while open
    // (StatSample lands at ~1Hz per running container; DetailSnapshot is
    // cached on Enter and stays static until the user closes/reopens).
    if app.selection.detail_open {
        let blkio = app
            .selection
            .selected_id
            .and_then(|eid| app.live.id_string_for_entity(eid).map(|s| s.to_string()))
            .and_then(|cid| app.live.last_sample(&cid).map(|s| (s.blkio_r_bytes, s.blkio_w_bytes)))
            .unwrap_or((0, 0));
        if let Some(snap) = app.selection.pending_detail.as_ref() {
            detail_panel::render_detail_panel(frame, snap, blkio.0, blkio.1);
        } else {
            detail_panel::render_loading(frame);
        }
    }
}

/// Centered plain-text banner shown when the live World holds no entities.
///
/// Uses a bordered "scene" block (same chrome as the live render) so the
/// banner doesn't look like a different screen — the user sees the same frame,
/// just empty until containers appear. Vertical centering is approximated by a
/// `Layout` split (top padding + content + bottom padding).
fn render_empty_banner(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    palette: &crate::theme::Palette,
) {
    let block = Block::default().title("scene").borders(Borders::ALL);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Vertical centering: split the inner area so the banner lives on the
    // middle row regardless of terminal height.
    let rows = inner.height as usize;
    let middle = if rows >= 1 { (rows / 2) as u16 } else { 0 };
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(middle),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);

    // `edge` is the muted gray we use for the cube wireframe — the same dim
    // tone reads as "not the data, just chrome" for the empty banner.
    let banner_line = Line::from(Span::styled(
        EMPTY_BANNER,
        Style::default()
            .fg(palette.edge)
            .add_modifier(Modifier::ITALIC),
    ));
    let banner = Paragraph::new(banner_line)
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true })
        .style(Style::default().bg(Color::Reset));
    frame.render_widget(banner, v[1]);
}
