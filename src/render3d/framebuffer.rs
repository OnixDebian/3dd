//! Braille-resolution color framebuffer.
//!
//! A flat grid of `Option<Color>` sub-pixels. `(w, h)` is the braille SUB-PIXEL
//! resolution — `2 * cells_w` wide, `4 * cells_h` tall (matching the screen-pixel
//! convention plan 01-03's projector targets). The rasterizer (plan 04) writes lit
//! pixels here; plan 05 walks the lit-pixel iterator to blit braille glyphs.
//!
//! No depth buffer: painter's sort handles occlusion for convex boxes (ARCH /
//! PITFALLS.md Pitfall 4 — avoid a per-pixel z-buffer unless overlap demands it).
//!
//! ARCH: pure module — no I/O, no terminal. `set` is bounds-checked and silently
//! ignores out-of-range writes so resize/edge geometry can never panic
//! (PITFALLS.md Pitfall 14).

// Consumed by the rasterizer here and the blit in plan 05.
#![allow(dead_code)]

use ratatui::style::Color;

/// A color framebuffer at braille sub-pixel resolution.
///
/// Pixels are stored row-major: index `y * w + x`. A `None` cell is unlit
/// (transparent / background); `Some(color)` is a lit sub-pixel.
#[derive(Debug, Clone)]
pub struct Framebuffer {
    w: usize,
    h: usize,
    color: Vec<Option<Color>>,
}

impl Framebuffer {
    /// Allocate a `w × h` framebuffer with every pixel unlit (`None`).
    pub fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            color: vec![None; w * h],
        }
    }

    /// Sub-pixel width (braille columns = `2 * cells_w`).
    pub fn width(&self) -> usize {
        self.w
    }

    /// Sub-pixel height (braille rows = `4 * cells_h`).
    pub fn height(&self) -> usize {
        self.h
    }

    /// Reset every pixel to unlit (`None`). Reuses the allocation.
    pub fn clear(&mut self) {
        for px in &mut self.color {
            *px = None;
        }
    }

    /// Light the pixel at `(x, y)`. Out-of-range coordinates are silently
    /// ignored so the rasterizer never panics on clipped / resized geometry
    /// (PITFALLS.md Pitfall 14).
    pub fn set(&mut self, x: usize, y: usize, color: Color) {
        if let Some(idx) = self.index(x, y) {
            self.color[idx] = Some(color);
        }
    }

    /// The color at `(x, y)`, or `None` if unlit or out of range.
    pub fn get(&self, x: usize, y: usize) -> Option<Color> {
        self.index(x, y).and_then(|idx| self.color[idx])
    }

    /// Iterate over every LIT pixel as `(x, y, color)`. This is the blit feed
    /// for plan 05 (group lit pixels into 2×4 braille cells).
    pub fn lit_pixels(&self) -> impl Iterator<Item = (usize, usize, Color)> + '_ {
        let w = self.w;
        self.color.iter().enumerate().filter_map(move |(idx, cell)| {
            cell.map(|c| (idx % w, idx / w, c))
        })
    }

    /// Row-major index for `(x, y)`, or `None` if out of bounds.
    fn index(&self, x: usize, y: usize) -> Option<usize> {
        if x < self.w && y < self.h {
            Some(y * self.w + x)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test fixtures only — production draw color flows through the Palette.
    // Use named colors so the "no inline RGB in render3d" grep stays clean.
    const RED: Color = Color::Red;
    const BLUE: Color = Color::Blue;

    #[test]
    fn fresh_framebuffer_is_all_none() {
        let fb = Framebuffer::new(4, 4);
        assert_eq!(fb.width(), 4);
        assert_eq!(fb.height(), 4);
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(fb.get(x, y), None, "pixel ({x},{y}) should start unlit");
            }
        }
        assert_eq!(fb.lit_pixels().count(), 0);
    }

    #[test]
    fn set_get_roundtrips() {
        let mut fb = Framebuffer::new(8, 6);
        fb.set(3, 2, RED);
        assert_eq!(fb.get(3, 2), Some(RED));
        // Neighbors remain unlit.
        assert_eq!(fb.get(2, 2), None);
        assert_eq!(fb.get(3, 1), None);
    }

    #[test]
    fn set_overwrites_existing_pixel() {
        // Painter's algorithm relies on later writes overwriting earlier ones.
        let mut fb = Framebuffer::new(4, 4);
        fb.set(1, 1, RED);
        fb.set(1, 1, BLUE);
        assert_eq!(fb.get(1, 1), Some(BLUE));
    }

    #[test]
    fn out_of_bounds_set_does_not_panic() {
        let mut fb = Framebuffer::new(4, 4);
        fb.set(100, 100, RED); // way out of range
        fb.set(4, 0, RED); // exactly one past the right edge
        fb.set(0, 4, RED); // exactly one past the bottom edge
        assert_eq!(fb.lit_pixels().count(), 0, "no pixel should have been lit");
        assert_eq!(fb.get(100, 100), None);
    }

    #[test]
    fn clear_resets_all_pixels() {
        let mut fb = Framebuffer::new(4, 4);
        fb.set(0, 0, RED);
        fb.set(3, 3, BLUE);
        assert_eq!(fb.lit_pixels().count(), 2);
        fb.clear();
        assert_eq!(fb.lit_pixels().count(), 0);
        assert_eq!(fb.get(0, 0), None);
    }

    #[test]
    fn lit_pixels_reports_coords_and_color() {
        let mut fb = Framebuffer::new(5, 5);
        fb.set(2, 1, RED);
        fb.set(4, 3, BLUE);
        let mut lit: Vec<_> = fb.lit_pixels().collect();
        lit.sort_by_key(|&(x, y, _)| (y, x));
        assert_eq!(lit, vec![(2, 1, RED), (4, 3, BLUE)]);
    }
}
