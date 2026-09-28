//! The loop-extrusion model: its state (the [`World`]: lattice and
//! entities), its parameters ([`Rates`]), and its dynamics (the [`Action`]s
//! and the [`SpawnRule`]).
//!
//! Nothing here depends on [`crate::sim`], which runs and observes a model.

pub mod action;
pub mod entity;
pub mod lattice;
pub mod spawn;
pub mod world;

pub use action::{Action, Rates};
pub use entity::{Leg, Lef, LefKey, LegKey, Mobility};
pub use lattice::{Ctcf, Direction, Lattice, LatticeError, Position, Site, SiteKind};
pub use spawn::{SpawnRule, UniformNonCtcfSpawn};
pub use world::{LefStates, LegState, World, WorldError};

use rand_pcg::Pcg64;

/// Continuous simulation clock. `f64` is plenty of precision for the
/// time scales we care about.
pub type Time = f64;

/// Result returned by [`Action::schedule`]. Tells the engine when (or
/// whether) to next fire the action.
#[derive(Copy, Clone, Debug)]
pub enum Schedule {
    At(Time),
    Never,
}

/// Concrete RNG type used throughout the simulation. Locked in so that
/// signatures stay simple and seeded runs are reproducible. Trade off:
/// callers wanting a different RNG must change this alias.
pub type SimRng = Pcg64;
