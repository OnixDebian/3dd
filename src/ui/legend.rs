//! Legend HUD (THEME-05): persistent overlay showing active palette name
//! and one labeled color swatch per Status variant.
//!
//! Rendered by BOTH backends so the user always knows which color means
//! which state — and so 05-04's palette cycling has a visible label that
//! follows the swatches.
//!
//! The legend is anchored TOP-LEFT (05-05-RV2 — moved from top-right
//! per user feedback at the human-verify checkpoint; chosen to avoid
//! colliding with the detail-panel popup, which is centered, and the
//! status bar, which is the bottom row). Width: 22 cells (longest
//! status label "restarting" + ■ + padding). Height: 7 rows (border +
//! 5 statuses + border).
//!
//! Toggled by `L`. Initial visibility comes from `AppConfig.hud_visible`
//! (default true).
//!
//! ## 05-05-RV3 (Bug C — "screen constantly flickers")
//!
//! Each kitty frame issues `delete_all` (removes the prior graphics
//! image) immediately followed by `emit_kitty` (places the new one).
//! Terminal cells (the legend's `│ ■ running │` text) are rendered ON
//! TOP of kitty images, but with a DEFAULT (reset) background — which
//! in kitty is transparent over images. So the spinning cube pixels
//! showed through every gap between the legend's text characters,
//! producing visible flicker at the render cadence (~30 Hz).
//!
//! Pre-05-05 the only cell text inside the image area was the small
//! per-frame label (single line, infrequent updates) and the optional
//! popup (opt-in, rare). The legend introduced a permanent 22×7 cell
//! region of text directly on top of the most-pixel-changing area of
//! the image — making the previously-tolerable transparency flicker
//! constant and obvious.
//!
//! RV3 fix: emit an EXPLICIT solid background SGR
//! (`\x1b[48;2;R;G;Bm` using `palette.background`) for every legend
//! cell. The legend now renders as a SOLID PANEL that fully masks the
//! image underneath. Braille mirror: the block style + per-span styles
//! switch from `bg(Color::Reset)` (transparent) to `bg(palette
//! .background)` (solid) — same intent, same fix.

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

/// Anchor the legend to the TOP-LEFT corner of `scene_area`, with a
/// 1-cell inset from each edge. Returns the legend's Rect or None when
/// the scene area is too small to host it (degrades gracefully).
///
/// 05-05-RV2 (Bug B): moved from top-right to top-left per user
/// feedback. The popup (centered) and status bar (bottom row) are
/// unaffected by the anchor flip — both already coexisted with the
/// previous top-right anchor.
pub fn legend_rect(scene_area: Rect) -> Option<Rect> {
    if scene_area.width < LEGEND_W + 2 || scene_area.height < LEGEND_H + 2 {
        return None;
    }
    Some(Rect {
        x: scene_area.x + 1,
        y: scene_area.y + 1,
        width: LEGEND_W,
        height: LEGEND_H,
    })
}

/// Render the legend into `area` of the braille `frame`. `palette_name`
/// is shown in the block title (e.g. "notion-soft"). Clears the cells
/// underneath first so the cube can't bleed through.
///
/// 05-05-RV3 (Bug C — flicker fix): every span and the block style
/// now carry an explicit `bg(palette.background)` so the legend cells
/// are SOLID, not transparent. Pre-RV3 the style used
/// `bg(Color::Reset)`, which in kitty terminals is transparent over
/// the graphics image — the constantly-changing pixels behind
/// produced visible flicker at the legend's footprint. Solid bg fixes
/// it on the braille side too: ratatui paints cells with
/// `bg(palette.background)` as a solid fill that masks any underlying
/// canvas content.
pub fn render_legend(frame: &mut Frame, area: Rect, palette: &Palette, palette_name: &str) {
    // Clear the cells we're about to paint so the scene under us doesn't
    // bleed through the borders / gaps. Clear writes default-style cells;
    // the subsequent Block paints the solid bg over them.
    frame.render_widget(Clear, area);

    let bg = palette.background;
    let mut lines: Vec<Line> = Vec::with_capacity(5);
    for (status, label) in LEGEND_STATUSES.iter() {
        let color = palette.status_color(*status);
        lines.push(Line::from(vec![
            Span::styled(" ", Style::default().bg(bg)),
            Span::styled("■", Style::default().fg(color).bg(bg)),
            Span::styled(" ", Style::default().bg(bg)),
            Span::styled(label.to_string(), Style::default().fg(palette.edge).bg(bg)),
        ]));
    }

    let title = format!(" {palette_name} ");
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().fg(palette.edge).bg(bg));

    let para = Paragraph::new(lines)
        .block(block)
        .style(Style::default().bg(bg));
    frame.render_widget(para, area);
}

// -------- kitty cell-grid emit ------------------------------------------

/// Emit the legend as raw ANSI cell writes to `out` at the TOP-LEFT of
/// the kitty pixel surface. Mirrors `render_legend` content via plain
/// terminal escapes (truecolor SGR) so the legend is visible on the
/// real-pixel backend too.
///
/// `cols`, `rows` are the terminal cell dimensions; the legend anchors
/// to (1, 1) (1-cell inset from left + top, 0-indexed) so it sits
/// inside the visible area without touching the left edge or the
/// status bar (rows - 1).
///
/// 05-05-RV2 (Bug B): anchor moved from top-right to top-left per user
/// feedback at the checkpoint. 05-05-RV3 (Bug C — flicker): every
/// cell now carries an explicit `palette.background` bg SGR
/// (`\x1b[48;2;R;G;Bm`) combined with the fg SGR so the kitty
/// graphics image cannot show through the legend's cell gaps; the
/// legend renders as a solid panel that fully masks the spinning
/// cubes underneath.
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
    let x = 1u16; // 0-indexed column (top-left of legend, 1-cell inset from left)
    let y = 1u16; // 0-indexed row
    let (er, eg, eb) = rgb_of(palette.edge);
    let (br, bg_, bb) = rgb_of(palette.background);

    // Helper: position cursor (terminal cells are 1-indexed).
    let pos = |col: u16, row: u16| format!("\x1b[{};{}H", row + 1, col + 1);
    // RV3 anti-flicker: the bg SGR is load-bearing. Every legend cell
    // emits truecolor fg+bg in one SGR so kitty's image surface is
    // fully masked under the legend's footprint.
    let edge_sgr = format!("\x1b[38;2;{er};{eg};{eb};48;2;{br};{bg_};{bb}m");
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

    // Status rows. The swatch color is the per-status fg; the bg stays
    // `palette.background` end-to-end so the row is one solid panel.
    for (i, (status, label)) in LEGEND_STATUSES.iter().enumerate() {
        let (sr, sg, sb) = rgb_of(palette.status_color(*status));
        // Row interior layout: "│ ■ {label}{pad}│"
        // Cells consumed by " ■ " is 3, label is label.len(), pad fills to interior - 3.
        let label_len = label.chars().count();
        let label_pad = interior.saturating_sub(3 + label_len);
        let row = y + 1 + i as u16;
        write!(
            out,
            "{}{edge_sgr}│ \x1b[38;2;{sr};{sg};{sb};48;2;{br};{bg_};{bb}m■{edge_sgr} {label}{}│{reset}",
            pos(x, row),
            " ".repeat(label_pad),
        )?;
    }

    // Bottom border.
    let bot = format!("└{}┘", "─".repeat(interior));
    write!(out, "{}{edge_sgr}{bot}{reset}", pos(x, y + LEGEND_H - 1))?;

    Ok(())
}

/// One-shot clearing pass: write spaces over the cells the legend last
/// occupied so the kitty image (or terminal background) shows through
/// cleanly.
///
/// 05-05-RV1 (Bug A — "L doesn't work in kitty"): the original
/// Option (c) cleanup strategy ("next `emit_kitty` re-blits over the
/// stale legend cells") was wrong. The kitty graphics protocol places
/// an IMAGE; terminal cell text is rendered ON TOP of images and
/// persists until explicitly written-over with spaces. Pressing L to
/// hide the HUD therefore left a stuck "phantom" legend on screen
/// (cells unchanged, image redrawn behind them). RV1 wires this
/// function to fire ONCE when `hud_visible` transitions true → false
/// (tracked via `last_hud_visible` in `run_kitty`), writing solid-bg
/// spaces over every cell the legend last occupied — so the image
/// surface is the only thing visible there on the next frame.
///
/// Uses `palette.background` for the cleared cells so they blend with
/// the kitty image edge tone (this matches the bg the legend itself
/// uses post-RV3). On tiny terminals (below the legend's minimum) this
/// is a noop — mirrors `emit_legend_kitty`'s graceful skip.
pub fn emit_legend_clear_kitty(
    out: &mut impl std::io::Write,
    cols: u16,
    rows: u16,
    palette: &Palette,
) -> std::io::Result<()> {
    if cols < LEGEND_W + 2 || rows < LEGEND_H + 2 {
        return Ok(());
    }
    let x = 1u16; // 0-indexed column (matches emit_legend_kitty's RV2 top-left anchor)
    let y = 1u16;
    let (br, bg_, bb) = rgb_of(palette.background);
    let pos = |col: u16, row: u16| format!("\x1b[{};{}H", row + 1, col + 1);
    let bg_sgr = format!("\x1b[48;2;{br};{bg_};{bb}m");
    let reset = "\x1b[0m";
    let blanks = " ".repeat(LEGEND_W as usize);
    for r in 0..LEGEND_H {
        let row = y + r;
        write!(out, "{}{bg_sgr}{blanks}{reset}", pos(x, row))?;
    }
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
    fn legend_rect_anchors_top_left_on_normal_area() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 40,
        };
        let r = legend_rect(area).unwrap();
        assert_eq!(r.width, LEGEND_W);
        assert_eq!(r.height, LEGEND_H);
        // RV2 (Bug B): legend now anchors TOP-LEFT, not top-right.
        // 1-cell inset from the left edge.
        assert_eq!(r.x, area.x + 1);
        assert_eq!(r.y, area.y + 1);
    }

    /// RV2 regression pin: the rect must be on the LEFT side. If anyone
    /// ever flips this back to right-anchored the test breaks immediately.
    #[test]
    fn legend_rect_x_is_one_cell_inset_from_left() {
        let area = Rect {
            x: 5,
            y: 0,
            width: 100,
            height: 40,
        };
        let r = legend_rect(area).unwrap();
        assert_eq!(
            r.x,
            area.x + 1,
            "legend must anchor TOP-LEFT (1-cell inset from area.x), got x={}",
            r.x
        );
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

    /// RV3 (Bug C — flicker): every emit_legend_kitty cell must carry a
    /// truecolor background SGR (`\x1b[...48;2;R;G;Bm`) so the kitty
    /// image underneath cannot show through. Pre-RV3 cells used only fg
    /// SGR and a `\x1b[0m` reset; the cell's default bg in kitty is
    /// transparent over images, causing per-frame flicker as the
    /// spinning cube pixels showed through the text gaps. This test
    /// pins the load-bearing presence of the bg SGR in the emit output.
    #[test]
    fn emit_legend_kitty_uses_solid_truecolor_bg() {
        let palette = Palette::notion_soft();
        let (br, bg_, bb) = match palette.background {
            Color::Rgb(r, g, b) => (r, g, b),
            _ => panic!("notion-soft background must be Color::Rgb"),
        };
        let mut buf: Vec<u8> = Vec::new();
        emit_legend_kitty(&mut buf, 100, 40, &palette, "notion-soft").unwrap();
        let s = String::from_utf8_lossy(&buf);
        let needle = format!("48;2;{br};{bg_};{bb}");
        assert!(
            s.contains(&needle),
            "every legend cell must carry the solid-bg SGR ({needle}) to mask the kitty image — RV3 anti-flicker fix",
        );
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

    /// RV1 (Bug A — "L doesn't work in kitty"): emit_legend_clear_kitty
    /// writes exactly LEGEND_H rows of LEGEND_W spaces with the
    /// palette.background bg SGR — wiping the cell text the previous
    /// frame's emit_legend_kitty left behind. Without this, pressing
    /// L to hide the HUD leaves a phantom legend stuck on screen
    /// because the kitty graphics image redraw does NOT clear cell
    /// text (cells render ON TOP of images and persist).
    #[test]
    fn emit_legend_clear_kitty_writes_blank_rows_with_solid_bg() {
        let palette = Palette::notion_soft();
        let mut buf: Vec<u8> = Vec::new();
        emit_legend_clear_kitty(&mut buf, 100, 40, &palette).unwrap();
        let s = String::from_utf8_lossy(&buf);
        // The clear pass writes LEGEND_H cursor-positioning escapes.
        let cursor_moves = s.matches("\x1b[").count();
        assert!(
            cursor_moves >= LEGEND_H as usize,
            "expected >= {} cursor moves, got {cursor_moves}",
            LEGEND_H
        );
        // The bg SGR is present so the cleared cells are SOLID — using
        // default-reset would leave the cells transparent over the
        // kitty image and re-introduce the ghost-legend look.
        let (br, bg_, bb) = match palette.background {
            Color::Rgb(r, g, b) => (r, g, b),
            _ => panic!("notion-soft background must be Color::Rgb"),
        };
        let needle = format!("48;2;{br};{bg_};{bb}");
        assert!(s.contains(&needle), "clear pass must use solid bg SGR");
        // The buffer contains a run of LEGEND_W spaces (the blanking
        // content for each row).
        let space_run = " ".repeat(LEGEND_W as usize);
        assert!(s.contains(&space_run), "must write space runs of LEGEND_W width");
    }

    /// emit_legend_clear_kitty is a noop on tiny terminals (matches
    /// emit_legend_kitty's gate so the two functions degrade together).
    #[test]
    fn emit_legend_clear_kitty_noop_on_tiny_terminal() {
        let palette = Palette::notion_soft();
        let mut buf: Vec<u8> = Vec::new();
        emit_legend_clear_kitty(&mut buf, 10, 5, &palette).unwrap();
        assert!(buf.is_empty(), "clear must be a noop on tiny terminals");
    }
}
