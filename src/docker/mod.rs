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

// Phase-3 surface — most items are now wired through main.rs (03-04). A few
// helpers (RawCpu/RawMem/map_status/from_bollard_summary) are still only used
// inside the docker layer; keep dead_code/unused_imports allowed at module
// scope rather than scattered attributes on each re-export.
#![allow(dead_code)]
#![allow(unused_imports)]

pub mod stats;

pub mod domain;
pub mod connect;
pub mod inspect;
pub mod streams;

// Re-export the normalizer surface so call sites can write `crate::docker::normalize`
// without reaching into the submodule path. Keep this list in sync with the
// items the rest of the app speaks.
pub use stats::{normalize, RawCpu, RawMem, StatSample};
pub use domain::{
    from_bollard_summary, map_status, ContainerSnapshot, EnrichedSnapshot, PortProto, PortSummary,
};
pub use connect::{connect_and_probe, ProbeError};
pub use inspect::{enrich_snapshot_on_seed, enrich_snapshot_on_start};
pub use streams::spawn_docker_tasks;
