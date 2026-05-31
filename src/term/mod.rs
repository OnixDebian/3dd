//! Terminal-environment concerns: capability detection, env-var sniffing,
//! any future TTY-adapter glue. Distinct from `src/render3d/` (the 3D
//! rasterizer) — this module deals with the host TERMINAL, not the
//! rendered scene.

pub mod capability;
pub mod cell;
