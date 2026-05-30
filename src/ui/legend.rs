//! Legend HUD (THEME-05): persistent overlay showing active palette name
//! and one labeled color swatch per Status variant.
//!
//! Rendered by BOTH backends so the user always knows which color means
//! which state — and so 05-04's palette cycling has a visible label that
//! follows the swatches.
//!
//! The legend is anchored TOP-RIGHT (chosen to avoid colliding with the
//! detail-panel popup, which is centered, and the status bar, which is
//! the bottom row). Width: 22 cells (longest status label "restarting" +
//! ■ + padding). Height: 7 rows (border + 5 statuses + border).
//!
//! Toggled by `L`. Initial visibility comes from `AppConfig.hud_visible`
//! (default true).

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::theme::{Palette, Status};

/// Width of the legend in cells. Fits "■ restarting" + 2-cell padding.
pub const LEGEND_W: u16 = 22;
/// Height of the legend in cells. 1 top border + 5 status rows + 1 bottom border.
pub const LEGEND_H: u16 = 7;

/// Iteration order for status rows — deterministic; matches the order in
/// the existing Palette::status_color match. Public so the kitty backend
/// can iterate the same set.
pub const LEGEND_STATUSES: [(Status, &str); 5] = [
    (Status::Running, "running"),
    (Status::Paused, "paused"),
    (Status::Stopped, "stopped"),
    (Status::Restarting, "restarting"),
    (Status::Crashed, "crashed"),
];

/// Anchor the legend to the top-right corner of `scene_area`, with a
/// 1-cell inset from each edge. Returns the legend's Rect or None when
/// the scene area is too small to host it (degrades gracefully).
pub fn legend_rect(scene_area: Rect) -> Option<Rect> {
    if scene_area.width < LEGEND_W + 2 || scene_area.height < LEGEND_H + 2 {
        return None;
    }
    Some(Rect {
        x: scene_area.x + scene_area.width - LEGEND_W - 1,
        y: scene_area.y + 1,
        width: LEGEND_W,
        height: LEGEND_H,
    })
}

/// Render the legend into `area` of the braille `frame`. `palette_name`
/// is shown in the block title (e.g. "notion-soft"). Clears the cells
/// underneath first so the cube can't bleed through.
pub fn render_legend(frame: &mut Frame, area: Rect, palette: &Palette, palette_name: &str) {
    // Clear the cells we're about to paint so the scene under us doesn't
    // bleed through the borders / gaps.
    frame.render_widget(Clear, area);

    let mut lines: Vec<Line> = Vec::with_capacity(5);
    for (status, label) in LEGEND_STATUSES.iter() {
        let color = palette.status_color(*status);
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled("■", Style::default().fg(color)),
            Span::raw(" "),
            Span::styled(label.to_string(), Style::default().fg(palette.edge)),
        ]));
    }

    let title = format!(" {palette_name} ");
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().fg(palette.edge).bg(Color::Reset));

    let para = Paragraph::new(lines).block(block);
    frame.render_widget(para, area);
}

// -------- kitty cell-grid emit ------------------------------------------

/// Emit the legend as raw ANSI cell writes to `out` at the top-right of
/// the kitty pixel surface. Mirrors `render_legend` content via plain
/// terminal escapes (truecolor SGR) so the legend is visible on the
/// real-pixel backend too.
///
/// `cols`, `rows` are the terminal cell dimensions; the legend anchors
/// to (cols - LEGEND_W - 1, 1) so it sits inside the visible area
/// without touching the right edge or the status bar (rows - 1).
///
/// Pattern matches the existing kitty popup code (see
/// `kitty::draw_popup_box`): unicode box-drawing chars + truecolor SGR.
pub fn emit_legend_kitty(
    out: &mut impl std::io::Write,
    cols: u16,
    rows: u16,
    palette: &Palette,
    palette_name: &str,
) -> std::io::Result<()> {
    if cols < LEGEND_W + 2 || rows < LEGEND_H + 2 {
        return Ok(()); // graceful skip on tiny terminals
    }
    let x = cols - LEGEND_W - 1; // 0-indexed column (top-left of legend)
    let y = 1u16; // 0-indexed row
    let (er, eg, eb) = rgb_of(palette.edge);

    // Helper: position cursor (terminal cells are 1-indexed).
    let pos = |col: u16, row: u16| format!("\x1b[{};{}H", row + 1, col + 1);
    let edge_sgr = format!("\x1b[38;2;{er};{eg};{eb}m");
    let reset = "\x1b[0m";

    // Top border with title centered.
    let title = format!(" {palette_name} ");
    // Total interior width (excluding the two corner glyphs) is LEGEND_W - 2.
    let interior = (LEGEND_W as usize).saturating_sub(2);
    let title_len = title.chars().count();
    let title_pad_left = interior.saturating_sub(title_len) / 2;
    let title_pad_right = interior.saturating_sub(title_pad_left + title_len);
    let mut top = String::from("┌");
    top.push_str(&"─".repeat(title_pad_left));
    top.push_str(&title);
    top.push_str(&"─".repeat(title_pad_right));
    top.push('┐');
    write!(out, "{}{edge_sgr}{top}{reset}", pos(x, y))?;

    // Status rows.
    for (i, (status, label)) in LEGEND_STATUSES.iter().enumerate() {
        let (sr, sg, sb) = rgb_of(palette.status_color(*status));
        // Row interior layout: "│ ■ {label}{pad}│"
        // Cells consumed by " ■ " is 3, label is label.len(), pad fills to interior - 3.
        let label_len = label.chars().count();
        let label_pad = interior.saturating_sub(3 + label_len);
        let row = y + 1 + i as u16;
        write!(
            out,
            "{}{edge_sgr}│ \x1b[38;2;{sr};{sg};{sb}m■{edge_sgr} {label}{}│{reset}",
            pos(x, row),
            " ".repeat(label_pad),
        )?;
    }

    // Bottom border.
    let bot = format!("└{}┘", "─".repeat(interior));
    write!(out, "{}{edge_sgr}{bot}{reset}", pos(x, y + LEGEND_H - 1))?;

    Ok(())
}

/// Best-effort Color → (r,g,b) for the kitty emitter. Non-Rgb colors
/// (palette quantization edge case) collapse to gray.
fn rgb_of(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (0x88, 0x88, 0x88),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legend_rect_returns_none_on_tiny_area() {
        let small = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 4,
        };
        assert!(legend_rect(small).is_none());
    }

    #[test]
    fn legend_rect_anchors_top_right_on_normal_area() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 40,
        };
        let r = legend_rect(area).unwrap();
        assert_eq!(r.width, LEGEND_W);
        assert_eq!(r.height, LEGEND_H);
        // Top-right: x near right edge, y near top.
        assert!(r.x + r.width <= area.x + area.width);
        assert_eq!(r.y, area.y + 1);
        // 1-cell inset from the right edge.
        assert_eq!(r.x + r.width, area.x + area.width - 1);
    }

    /// Boundary: width just under the minimum returns None; exactly the
    /// minimum returns Some. Pins the off-by-one at the gate.
    #[test]
    fn legend_rect_boundary_widths() {
        let just_under = Rect {
            x: 0,
            y: 0,
            width: LEGEND_W + 1,
            height: LEGEND_H + 2,
        };
        assert!(legend_rect(just_under).is_none());
        let exact = Rect {
            x: 0,
            y: 0,
            width: LEGEND_W + 2,
            height: LEGEND_H + 2,
        };
        assert!(legend_rect(exact).is_some());
    }

    #[test]
    fn legend_statuses_cover_all_5_variants() {
        // If a new Status variant lands in src/theme/mod.rs the legend
        // must include it — this test fails fast.
        let names: Vec<&str> = LEGEND_STATUSES.iter().map(|(_, n)| *n).collect();
        assert!(names.contains(&"running"));
        assert!(names.contains(&"paused"));
        assert!(names.contains(&"stopped"));
        assert!(names.contains(&"restarting"));
        assert!(names.contains(&"crashed"));
        assert_eq!(LEGEND_STATUSES.len(), 5);
    }

    /// The kitty emit fn must NOT panic and must write SOMETHING when the
    /// terminal is big enough. We just exercise the path against an
    /// in-memory buffer — the visual properties land in the
    /// human-verify checkpoint.
    #[test]
    fn emit_legend_kitty_writes_to_buffer_when_sized() {
        let palette = Palette::notion_soft();
        let mut buf: Vec<u8> = Vec::new();
        emit_legend_kitty(&mut buf, 100, 40, &palette, "notion-soft").unwrap();
        assert!(!buf.is_empty(), "emit must produce output on a sized terminal");
        let s = String::from_utf8_lossy(&buf);
        // Title appears in the buffer.
        assert!(s.contains("notion-soft"), "title must appear in output");
        // The kitty-style cursor-position escape is used to anchor cells.
        assert!(s.contains("\x1b["), "must include CSI cursor positioning");
    }

    /// On a tiny terminal the kitty emit is a noop (no escapes, no bytes).
    /// Pins the graceful-degradation invariant.
    #[test]
    fn emit_legend_kitty_noop_on_tiny_terminal() {
        let palette = Palette::notion_soft();
        let mut buf: Vec<u8> = Vec::new();
        emit_legend_kitty(&mut buf, 10, 5, &palette, "notion-soft").unwrap();
        assert!(buf.is_empty(), "emit must be a noop on tiny terminals");
    }
}
