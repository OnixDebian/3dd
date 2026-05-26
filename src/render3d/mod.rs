//! Pure 3D rendering math.
//!
//! ARCH: `render3d` is a pure, unit-testable module — no I/O, no terminal, no
//! globals. It owns the world→screen projection pipeline that everything the
//! rasterizer draws sits on top of.

pub mod project;

pub use project::Projector;
