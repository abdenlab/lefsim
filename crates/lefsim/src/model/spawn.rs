//! Spawn-site selection rules used by [`crate::Action::Respawn`].

use rand::Rng;

use super::lattice::{Position, SiteKind};
use super::world::World;
use super::SimRng;

/// Strategy for picking the new spawn site when a LEF respawns.
///
/// Must stay object-safe (dyn-compatible): the engine takes
/// `&dyn SpawnRule` so that FFI wrappers can hold a concrete, non-generic
/// simulation type. Don't add generic methods or `Self`-returning methods.
pub trait SpawnRule {
    fn sample(&self, world: &World, rng: &mut SimRng) -> Position;
}

/// Uniformly samples among `Plain`, currently-unoccupied sites by rejection.
/// Fine when occupancy << lattice length; precompute a free-site list otherwise.
#[derive(Default, Copy, Clone, Debug)]
pub struct UniformNonCtcfSpawn;

impl SpawnRule for UniformNonCtcfSpawn {
    fn sample(&self, world: &World, rng: &mut SimRng) -> Position {
        let n = world.lattice.len() as u32;
        loop {
            let i = rng.gen_range(0..n);
            let kind = world.lattice.sites[i as usize].kind;
            if matches!(kind, SiteKind::Plain) && !world.occupied(i) {
                return i;
            }
        }
    }
}
