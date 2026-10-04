//! The loop-extrusion model: its state (the [`World`]: lattice and
//! entities), its parameters ([`Rates`]), and its dynamics (the [`Action`]s
//! and the [`Spawn`] rules). A [`Model`] bundles a world, rates and a
//! built-in spawn rule: everything a run needs except a seed.
//!
//! Nothing here depends on [`crate::sim`], which runs and observes a model.

pub mod action;
pub mod distance;
pub mod entity;
pub mod lattice;
pub mod spawn;
pub mod world;
#[cfg(feature = "serde")]
mod serial;

pub use action::{Action, Rates};
pub use distance::{DistanceError, LoopGraph};
pub use entity::{Leg, Lef, LefKey, LegKey, Mobility};
pub use lattice::{Ctcf, Direction, Lattice, LatticeError, Position, Site, SiteKind};
pub use spawn::{populate, Spawn, SpawnError, SpawnRule};
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

/// A complete, valid model: a world, its rates, and a built-in spawn rule.
///
/// Plain data apart from validation: `Clone`, and equal when its parts are.
/// With the `serde` feature it (de)serializes as a versioned document:
///
/// ```json
/// {
///   "format": "lefsim.model", "version": 2,
///   "lattice": { "n_sites": 100,
///                "ctcfs": [{ "site": 30, "captures_from": "right", "strength": 0.8 }] },
///   "lefs": [{ "left_pos": 10, "right_pos": 12,
///               "left_mobility": "moving", "right_mobility": "moving" }],
///   "rates": { "step": 1.0, "respawn": 0.001, "respawn_stalled": 0.01, "respawn_evicted": 0.1,
///              "respawn_captured": 0.001 },
///   "spawn": { "rule": "uniform_non_ctcf" }
/// }
/// ```
///
/// Loading goes through [`Lattice::with_ctcfs`], [`World::from_lef_states`]
/// and [`Model::new`], so a document can't describe an invalid model.
/// Run it with [`crate::Sim::new`]. For a custom spawn rule, see
/// [`crate::Sim::with_spawn`].
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "serde",
    serde(try_from = "serial::ModelData", into = "serial::ModelData")
)]
pub struct Model {
    pub(crate) world: World,
    pub(crate) rates: Rates,
    pub(crate) spawn: SpawnRule,
}

impl Model {
    /// Check that the rates are positive and finite, and that the spawn rule
    /// can always place the world's LEFs. The world validated itself.
    pub fn new(world: World, rates: Rates, spawn: SpawnRule) -> Result<Self, ModelError> {
        check(&world, &rates, &spawn)?;
        Ok(Model { world, rates, spawn })
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    pub fn rates(&self) -> Rates {
        self.rates
    }

    pub fn spawn(&self) -> SpawnRule {
        self.spawn
    }

    /// The parts, by value.
    pub fn into_parts(self) -> (World, Rates, SpawnRule) {
        (self.world, self.rates, self.spawn)
    }
}

/// The checks behind [`Model::new`], shared with [`crate::Sim::with_spawn`].
pub(crate) fn check(world: &World, rates: &Rates, spawn: &dyn Spawn) -> Result<(), ModelError> {
    for (name, value) in [
        ("step", rates.step),
        ("respawn", rates.respawn),
        ("respawn_stalled", rates.respawn_stalled),
        ("respawn_evicted", rates.respawn_evicted),
        ("respawn_captured", rates.respawn_captured),
    ] {
        if !(value.is_finite() && value > 0.0) {
            return Err(ModelError::BadRate { name, value });
        }
    }
    spawn.check(world.lattice(), world.n_lefs()).map_err(ModelError::Spawn)
}

/// Why a world, rates and spawn rule don't make a valid model.
#[derive(Clone, Debug, PartialEq)]
pub enum ModelError {
    /// A rate that isn't positive and finite. `name` is the [`Rates`] field.
    BadRate { name: &'static str, value: f64 },
    /// The spawn rule can't always place the world's LEFs.
    Spawn(SpawnError),
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelError::BadRate { name, value } => {
                write!(f, "the {name} rate must be positive and finite, got {value}")
            }
            ModelError::Spawn(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for ModelError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ModelError::Spawn(e) => Some(e),
            ModelError::BadRate { .. } => None,
        }
    }
}

/// Concrete RNG type used throughout the simulation. Locked in so that
/// signatures stay simple and seeded runs are reproducible. Trade off:
/// callers wanting a different RNG must change this alias.
pub type SimRng = Pcg64;

#[cfg(test)]
mod tests {
    use super::*;

    fn world(n_sites: usize, n_lefs: usize) -> World {
        populate(Lattice::plain(n_sites).unwrap(), n_lefs, &SpawnRule::UniformNonCtcf, 1).unwrap()
    }

    #[test]
    fn model_checks_rates_and_spawn_rule() {
        let rule = SpawnRule::UniformNonCtcf;
        assert!(Model::new(world(10, 3), Rates::default(), rule).is_ok());

        let bad = Rates { respawn: f64::NAN, ..Rates::default() };
        let err = Model::new(world(10, 3), bad, rule).unwrap_err();
        assert!(matches!(err, ModelError::BadRate { name: "respawn", .. }), "{err}");
        let bad = Rates { step: 0.0, ..Rates::default() };
        assert!(Model::new(world(10, 3), bad, rule).is_err());

        // Three LEFs on four sites: add_lef accepts them, the rule can't
        // guarantee a free site at respawn.
        let mut w = World::new(Lattice::plain(4).unwrap());
        for p in [0, 1, 2] {
            w.add_lef((p, Mobility::Moving), (p, Mobility::Moving)).unwrap();
        }
        let err = Model::new(w, Rates::default(), rule).unwrap_err();
        assert!(matches!(err, ModelError::Spawn(SpawnError::NotEnoughSites { .. })), "{err}");
    }

    #[test]
    fn model_is_send_sync_and_clone() {
        fn assert_traits<T: Send + Sync + Clone>() {}
        assert_traits::<Model>();
    }
}
