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
    // 05-06 ROB-02: surface the resolved render capability + FPS cap
    // when degraded. Only the Ascii tier is "interesting" to the user
    // (Truecolor/Kitty look the same as the pre-05-06 baseline). The
    // empty default keeps the steady-state bar identical to pre-05-06.
    let degraded_str = match app.render_mode {
        crate::term::capability::TerminalCapability::Ascii => {
            format!(" | degraded: ascii @{}fps", app.config.degraded_fps_cap)
        }
        _ => String::new(),
    };
    let text = format!(
        "3dd | fps: {:.0} | size: {}x{} | boxes: {} | mode: {mode} | palette: {pname}{deg} | P palette, L legend, Tab select, q quit",
        app.fps,
        w,
        h,
        app.world.entities.len(),
        pname = app.palette_name,
        deg = degraded_str,
    );

    let bar = Paragraph::new(Line::from(text))
        .style(Style::default().fg(Color::White).bg(Color::DarkGray));

    frame.render_widget(bar, area);
}
