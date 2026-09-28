//! Action-based simulation primitives for loop-extrusion-factor (LEF) models
//! on a 1D lattice.
//!
//! See `AGENTS.md` for the design rationale. The short version:
//!
//! - **Entities** live in typed slot-map arenas in [`World`].
//! - **Actions** are a sum-type enum ([`Action`]) addressed by entity key,
//!   so the action value *is* its priority-queue key.
//! - **Scheduling** is a pure function of world state and time
//!   ([`Action::schedule`]).
//! - **Firing** mutates the world and records which entities it touched,
//!   which tells the engine which actions to reschedule.
//! - The dependency map from entities to actions collapses to constants for
//!   this model: each leg has one step action, and each LEF has one respawn
//!   action — no dependency index needs to be maintained.
//!
//! ## Running and observing
//!
//! [`Sim`] is the entry point: build one with [`SimBuilder`], drive it with
//! [`Sim::step`], [`Sim::run_until`] or [`Sim::run_events`], and observe it
//! with a [`Recorder`] (grid snapshots) and/or an [`EventRecorder`] (event
//! deltas) via the `*_with` drivers.
//!
//! ```
//! use lefsim::*;
//!
//! let mut sites = vec![Site::plain(); 100];
//! sites[30] = Site::ctcf(Direction::Right, 1.0);
//! sites[70] = Site::ctcf(Direction::Left, 1.0);
//!
//! let mut sim = Sim::builder(Lattice::new(sites)?).seed(42).lefs(3).build()?;
//! let (frames, report) = sim.collect_until(100.0, SampleMode::every_dt(1.0));
//! assert_eq!(frames.n_snapshots(), 101);
//! assert_eq!(report.stopped, StopReason::ReachedTime);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! ## Module layout
//!
//! The crate has two halves, and [`model`] never depends on [`sim`]:
//!
//! - [`model`]: the loop-extrusion model.
//!   - [`model::world`]: the state: lattice plus entities.
//!   - [`model::lattice`]: the immutable lattice (sites, directions, CTCFs).
//!   - [`model::entity`]: typed keys plus the [`Leg`] and [`Lef`] structs.
//!   - [`model::action`]: the dynamics: actions, their [`Rates`], scheduling
//!     and firing, and how landing on a CTCF resolves.
//!   - [`model::spawn`]: spawn-site rules used by `Action::Respawn`.
//! - [`sim`]: running and observing a model.
//!   - [`Sim`], its builder, and batch drivers.
//!   - [`sim::record`]: recorders, frame buffers, and static run metadata.
//!   - The event loop and its indexed heap (internal).

pub mod model;
pub mod sim;

pub use model::{
    Action, Ctcf, Direction, Lattice, LatticeError, Leg, Lef, LefKey, LefStates, LegKey, LegState,
    Mobility, Position, Rates, Schedule, SimRng, Site, SiteKind, SpawnRule, Time,
    UniformNonCtcfSpawn, World, WorldError,
};
pub use sim::record::{
    EventColumns, EventFrames, EventRecorder, LefColumns, LefFrames, LefSnapshot, Observer,
    Recorder, SampleMode, SimMeta,
};
pub use sim::{BuildError, RunReport, Sim, SimBuilder, StopReason};
