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
//! - **Firing** mutates the world and pushes affected entity keys into a
//!   [`Touched`](action::Touched) set, which the [`Engine`] uses to decide
//!   which actions need rescheduling.
//! - The dependency map from entities to actions collapses to constants for
//!   this model: each leg has one step action, and each LEF has one respawn
//!   action — no dependency index needs to be maintained.
//!
//! ## Module layout
//!
//! - [`lattice`]: immutable substrate (sites, directions, CTCFs).
//! - [`entity`]: typed keys plus the [`entity::Leg`] and [`entity::Lef`] structs.
//! - [`world`]: mutable world state — entity arenas plus the occupancy index.
//! - [`action`]: action enum, scheduling, and firing logic.
//! - [`spawn`]: spawn-site selection rules used by `Action::Respawn`.
//! - [`engine`]: priority-queue-driven simulation loop.

pub mod action;
pub mod engine;
pub mod entity;
pub mod lattice;
pub mod spawn;
pub mod world;

pub use action::{Action, Rates, Schedule, Time, Touched};
pub use engine::{Engine, SimRng};
pub use entity::{Leg, LefKey, LegKey, Lef, Mobility};
pub use lattice::{Dir, Lattice, Site, SiteIdx, SiteKind};
pub use spawn::{SpawnRule, UniformNonCtcfSpawn};
pub use world::World;
