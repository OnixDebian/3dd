//! The view layer: a pure `view(frame, &app)` that lays out the screen.
//!
//! Layout is always derived from the live `frame.area()` so a resize never
//! reads a stale cached size (Pitfall #14). The scene area hosts the braille
//! Canvas that renders the orbiting cube — OR, when the live World is empty,
//! a centered plain-text "no containers" banner (so an idle Docker host doesn't
//! render a blank void, per Phase 3 criterion #5).

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

/// Render the whole UI for one frame. Synchronous, read-only on `App`.
pub fn view(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Vertical split: scene fills the screen above a 1-row status bar.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    let scene_area = chunks[0];
    let status_area = chunks[1];

    let palette = crate::theme::Palette::default();

    if app.world.entities.is_empty() {
        // Zero containers: render a centered banner instead of an empty scene.
        // The status bar still draws below so fps/size/help stay visible.
        render_empty_banner(frame, scene_area, &palette);
    } else {
        // The orbiting World of boxes renders into the braille Canvas in the
        // scene area. Palette is the single color source; the camera (framed
        // to the scene on entity-count changes by `App::run`) feeds
        // render_scene a ViewParams. The app owns the World — the single
        // source of truth.
        scene::render_scene(
            frame,
            scene_area,
            &app.camera,
            &app.world,
            &palette,
            &app.render_config,
            app.spin,
        );
    }

    status_bar::render(frame, status_area, app);
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
