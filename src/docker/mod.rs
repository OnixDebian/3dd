//! Docker data layer — the bridge between the Docker daemon and the renderer.
//!
//! This module is the home of Phase 3: turning a live container set into the
//! same `World { entities, bounds }` shape the renderer already consumes (the
//! [`crate::world`] domain). It is split into four submodules with deliberate
//! responsibilities — each one is the unit of one Phase-3 plan:
//!
//! - [`stats`] (03-01) — PURE normalizer: raw Docker counters in, a normalized
//!   [`stats::StatSample`] (CPU%, mem fraction, `[0,1]` load) out. No bollard
//!   dependency. The CPU%-delta gotcha (PITFALLS Pitfall 1) is contained and
//!   pinned by unit tests here.
//! - `domain` (03-02, NOT YET CREATED) — the daemon-connection state machine
//!   and the empty/permission/down domain states.
//! - `connect` (03-02, NOT YET CREATED) — bollard client construction +
//!   capability probe.
//! - `streams` (03-03, NOT YET CREATED) — the live event + per-container
//!   stats streams that feed the renderer.
//!
//! The forward-facing API surface is currently `stats` only; the other slots
//! are reserved here so that 03-02 and 03-03 only CREATE their own files (the
//! commented `pub mod` lines below tell them which line to uncomment, never
//! editing the structure of this file).

// Phase-3 surface is consumed by the renderer wiring (03-04) and Phase 4 — the
// items are intentionally public before their first call site lands. The
// re-exports below are part of that forward-facing API and will be wired in
// by 03-04 / Phase 4; silence the warning until then.
#![allow(dead_code)]
#![allow(unused_imports)]

pub mod stats;

// pub mod domain;   // created by 03-02 — uncomment when src/docker/domain.rs lands
// pub mod connect;  // created by 03-02 — uncomment when src/docker/connect.rs lands
// pub mod streams;  // created by 03-03 — uncomment when src/docker/streams.rs lands

// Re-export the normalizer surface so call sites can write `crate::docker::normalize`
// without reaching into the submodule path. Keep this list in sync with the
// items the rest of the app speaks.
pub use stats::{normalize, RawCpu, RawMem, StatSample};
