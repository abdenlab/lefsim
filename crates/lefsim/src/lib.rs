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
//! let mut sim = Sim::builder(Lattice::new(sites)).seed(42).lefs(3).build()?;
//! let (frames, report) = sim.collect_until(100.0, SampleMode::every_dt(1.0));
//! assert_eq!(frames.n_snapshots(), 101);
//! assert_eq!(report.stopped, StopReason::ReachedTime);
//! # Ok::<(), BuildError>(())
//! ```
//!
//! ## Module layout
//!
//! - [`sim`]: the [`Sim`] facade, its builder, and batch drivers.
//! - [`record`]: recorders, frame buffers, and static run metadata.
//! - [`world`]: world state and the static substrate it sits on.
//!   - [`world::lattice`]: immutable lattice (sites, directions, CTCFs).
//!   - [`world::entity`]: typed keys plus the [`world::Leg`] and
//!     [`world::Lef`] structs.
//! - [`action`]: action enum, scheduling, and firing logic.
//! - [`spawn`]: spawn-site selection rules used by `Action::Respawn`.
//! - [`engine`]: the priority-queue event loop (internal) and time types.

pub mod action;
pub mod engine;
pub mod record;
pub mod sim;
pub mod spawn;
pub mod world;

pub use action::{Action, Rates};
pub use engine::{Schedule, SimRng, Time};
pub use record::{
    EventColumns, EventFrames, EventRecorder, LefColumns, LefFrames, LefSnapshot, Observer,
    Recorder, SampleMode, SimMeta,
};
pub use sim::{BuildError, RunReport, Sim, SimBuilder, StopReason};
pub use spawn::{SpawnRule, UniformNonCtcfSpawn};
pub use world::{
    Direction, Lattice, Leg, Lef, LefKey, LegKey, Mobility, Site, Position, SiteKind, World,
};
