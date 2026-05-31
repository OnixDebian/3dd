//! Terminal cell pixel geometry: the size of one character cell in real
//! screen pixels, as reported by `crossterm::terminal::window_size()`.
//!
//! # Why this exists (05-06 RV4)
//!
//! Two backends draw the 3D scene:
//!
//! - **kitty** packs square pixels via the kitty graphics protocol; its
//!   projector consumes a real `(W*cw, H*ch)` pixel viewport at
//!   `cell_aspect=1.0`, so the perspective aspect matches the physical
//!   screen.
//! - **braille** packs 2×4 sub-cell dots into each terminal cell; its
//!   projector consumes a `(W*2, H*4)` *dot* viewport. Each braille dot
//!   occupies `cw/2 × ch/4` real px — at a typical 10×20 cell (cell ratio
//!   2:1), that is a SQUARE 5×5 pixel dot.
//!
//! For the braille tier's perspective to match the kitty tier visually, the
//! `cell_aspect` correction must be derived from the SAME physical cell
//! ratio the kitty path already uses:
//!
//! ```text
//! braille projector aspect = (2W / 4H) / cell_aspect
//! kitty projector aspect   = (W*cw) / (H*ch)
//!                          = W / (H * ch/cw)
//!
//! solve  (2W/4H) / cell_aspect  =  W / (H * ch/cw)
//!         1 / (2 * cell_aspect) =  cw / ch
//!         cell_aspect           =  ch / (2*cw)
//! ```
//!
//! At a typical 2:1 cell (`ch = 2*cw`), `cell_aspect = 1.0` — IDENTICAL to
//! the kitty path. At taller cells (e.g. 1:2.4 → `cell_aspect = 1.2`) the
//! braille path applies a small horizontal narrowing so boxes still read
//! as cubic. This is the "v2 fix sketch" from the 05-06 RV3 rustdoc in
//! `config::RenderConfig`.
//!
//! # Why a fallback exists
//!
//! `crossterm::terminal::window_size()` is supported on every desktop
//! terminal we ship for (kitty, ghostty, Alacritty, WezTerm, gnome-terminal,
//! foot, xterm) but can return zeros or fail under:
//!
//! - certain SSH multiplexers that don't proxy the TIOCGWINSZ ioctl,
//! - tests / pipes where there is no TTY,
//! - early-startup races where the kernel hasn't yet sized the pty.
//!
//! We treat any of those cases as "unknown" and return the
//! [`TYPICAL_CELL_PIXELS`] fallback — the historical 10×20 monospace cell
//! ratio. The fallback is also what the deterministic test path uses (the
//! `App::new` / dump-mode entry points never go through a live TTY).

/// Typical monospace cell pixel size — used as a fallback when crossterm
/// can't report a real one. 10×20 corresponds to the canonical 2:1
/// (height:width) cell aspect; at this ratio the dynamic correction
/// resolves to the historical `cell_aspect=1.0`.
///
/// Kept slightly conservative: a wider (taller-than-2:1) cell will pull
/// `cell_aspect` upward and narrow the braille horizontal extent; a
/// narrower (less-than-2:1) cell will pull it down. Both directions are
/// safe because the Phase-1 framing solver re-runs against the same
/// aspect (`Camera::frame_scene_with_aspect`), so the rack always fills
/// the same `FRAME_TARGET_FILL` fraction of the binding axis.
pub const TYPICAL_CELL_PIXELS: (u16, u16) = (10, 20);

/// Live cell pixel size from `crossterm::terminal::window_size()`, or
/// [`TYPICAL_CELL_PIXELS`] if the query fails / returns zeros.
///
/// **Returns `(cell_w_px, cell_h_px)`** — the size of ONE terminal cell in
/// real screen pixels, NOT the whole-window pixel size.
///
/// `crossterm` returns the WINDOW pixel size (`width`/`height`) plus the
/// grid size (`columns`/`rows`); we divide to recover the per-cell value.
/// Any zero division input collapses to the fallback. This function is
/// I/O (a single ioctl-equivalent syscall on Linux); callers should poll
/// at most once per frame and cache the result for the projector +
/// camera-framing call sites.
pub fn cell_pixel_size() -> (u16, u16) {
    match crossterm::terminal::window_size() {
        Ok(ws) if ws.width > 0 && ws.height > 0 && ws.columns > 0 && ws.rows > 0 => {
            let cw = (ws.width / ws.columns).max(1);
            let ch = (ws.height / ws.rows).max(1);
            (cw, ch)
        }
        _ => TYPICAL_CELL_PIXELS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fallback is the canonical 2:1 monospace ratio — under no
    /// circumstance should we land at a non-2-ish ratio when nothing is
    /// known about the terminal. This pin guards against an accidental
    /// constant change shifting every test's framing baseline.
    #[test]
    fn typical_cell_is_two_to_one_height_to_width() {
        let (cw, ch) = TYPICAL_CELL_PIXELS;
        let ratio = ch as f32 / cw as f32;
        assert!(
            (ratio - 2.0).abs() < 1e-3,
            "TYPICAL_CELL_PIXELS must be ~2:1 (got cw={cw}, ch={ch}, ratio={ratio})"
        );
    }

    /// `cell_pixel_size()` always returns non-zero values — both for
    /// the live-TTY path and for the fallback path. A zero would divide-
    /// by-zero in `RenderConfig::braille_cell_aspect_for_cell`.
    ///
    /// Under `cargo test` there is no TTY, so this exercises the fallback
    /// branch deterministically.
    #[test]
    fn cell_pixel_size_returns_non_zero() {
        let (cw, ch) = cell_pixel_size();
        assert!(cw > 0, "cell width must be non-zero, got {cw}");
        assert!(ch > 0, "cell height must be non-zero, got {ch}");
    }
}
