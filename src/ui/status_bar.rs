//! The bottom status bar: mode / fps / size / quit hint. Pure read of `App`.

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::App;

/// Render the status bar into `area`.
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let (w, h) = app.size;
    let mode = if app.camera.autopilot_active { "auto" } else { "manual" };
    let text = format!(
        "3dd | fps: {:.0} | size: {}x{} | boxes: {} | mode: {mode} | palette: {pname} | P palette, L legend, Tab select, q quit",
        app.fps,
        w,
        h,
        app.world.entities.len(),
        pname = app.palette_name,
    );

    let bar = Paragraph::new(Line::from(text))
        .style(Style::default().fg(Color::White).bg(Color::DarkGray));

    frame.render_widget(bar, area);
}
