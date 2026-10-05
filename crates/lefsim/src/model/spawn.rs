//! Spawn rules: where a LEF lands when it (re)spawns.
//!
//! [`Spawn`] is what a rule can do; [`SpawnRule`] lists the built-in rules as
//! plain data; [`populate`] uses a rule to build a starting world.

use rand::{Rng, SeedableRng};

use super::SimRng;
use super::lattice::{Lattice, Position, SiteKind};
use super::world::World;

/// A spawn rule: pick sites, and say whether it always can.
///
/// Must stay object-safe (dyn-compatible): the engine takes `&dyn Spawn` so
/// that FFI wrappers can hold a concrete, non-generic simulation type. Don't
/// add generic methods or `Self`-returning methods.
pub trait Spawn {
    /// Pick a site for a (re)spawning LEF. The LEF itself is not in
    /// `world`'s occupancy while this runs.
    fn sample(&self, world: &World, rng: &mut SimRng) -> Position;

    /// Whether this rule can always place a LEF when `n_lefs` LEFs share
    /// `lattice`, whatever their positions. `sample` may loop forever or
    /// panic if this fails.
    fn check(&self, lattice: &Lattice, n_lefs: usize) -> Result<(), SpawnError>;
}

/// The built-in spawn rules.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "rule", rename_all = "snake_case"))]
pub enum SpawnRule {
    /// Uniform among `Plain` sites that no leg occupies.
    #[default]
    UniformNonCtcf,
}

impl Spawn for SpawnRule {
    fn sample(&self, world: &World, rng: &mut SimRng) -> Position {
        match self {
            // By rejection. Fine when occupancy << lattice length; precompute
            // a free-site list otherwise.
            SpawnRule::UniformNonCtcf => {
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
    }

    fn check(&self, lattice: &Lattice, n_lefs: usize) -> Result<(), SpawnError> {
        match self {
            // The other n - 1 LEFs cover at most 2n - 2 sites, so 2n - 1
            // plain sites always leave one free.
            SpawnRule::UniformNonCtcf => {
                let needed = (2 * n_lefs).saturating_sub(1);
                let n_plain = lattice.n_plain();
                if n_plain < needed {
                    return Err(SpawnError::NotEnoughSites {
                        n_lefs,
                        needed,
                        n_plain,
                    });
                }
                Ok(())
            }
        }
    }
}

/// Why a spawn rule can't place the LEFs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpawnError {
    /// Not enough plain sites for `n_lefs` LEFs: the rule needs `needed`.
    NotEnoughSites {
        n_lefs: usize,
        needed: usize,
        n_plain: usize,
    },
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpawnError::NotEnoughSites {
                n_lefs,
                needed,
                n_plain,
            } => write!(
                f,
                "too many LEFs: {n_lefs} LEFs need at least {needed} non-CTCF sites, \
                 the lattice has {n_plain}"
            ),
        }
    }
}

impl std::error::Error for SpawnError {}

/// A world with `n` LEFs placed one at a time by `rule`, exactly as respawns
/// place them: each LEF lands at a sampled site (rolling for CTCF capture)
/// before the next site is sampled. `seed` drives only this placement.
pub fn populate(
    lattice: Lattice,
    n: usize,
    rule: &dyn Spawn,
    seed: u64,
) -> Result<World, SpawnError> {
    rule.check(&lattice, n)?;
    let mut rng = SimRng::seed_from_u64(seed);
    let mut world = World::new(lattice);
    for _ in 0..n {
        place_lef(&mut world, rule, &mut rng);
    }
    Ok(world)
}

/// Sample a site with `rule` and land a new LEF there.
pub(crate) fn place_lef(world: &mut World, rule: &dyn Spawn, rng: &mut SimRng) {
    let pos = rule.sample(world, rng);
    world.spawn_lef(pos, rng);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Ctcf, Direction, Mobility};

    #[test]
    fn uniform_check_needs_2n_minus_1_plain_sites() {
        let rule = SpawnRule::UniformNonCtcf;
        let ctcf = Ctcf {
            site: 0,
            captures_from: Direction::Left,
            strength: 1.0,
        };
        let lattice = Lattice::with_ctcfs(6, &[ctcf]).unwrap(); // 5 plain sites
        assert_eq!(rule.check(&lattice, 0), Ok(()));
        assert_eq!(rule.check(&lattice, 3), Ok(()));
        assert_eq!(
            rule.check(&lattice, 4),
            Err(SpawnError::NotEnoughSites {
                n_lefs: 4,
                needed: 7,
                n_plain: 5
            })
        );
    }

    #[test]
    fn populate_places_valid_lefs_reproducibly() {
        let ctcfs: Vec<_> = (0..10)
            .map(|i| Ctcf {
                site: 10 * i + 5,
                captures_from: Direction::Left,
                strength: 1.0,
            })
            .collect();
        let lattice = Lattice::with_ctcfs(100, &ctcfs).unwrap();
        let world = populate(lattice.clone(), 20, &SpawnRule::UniformNonCtcf, 3).unwrap();

        assert_eq!(world.n_lefs(), 20);
        let states = world.lef_states();
        for i in 0..20 {
            let ((l, lm), (r, rm)) = states.get(i).unwrap();
            assert_eq!(l, r, "both legs land on one site");
            assert!(matches!(lattice.sites()[l as usize].kind, SiteKind::Plain));
            assert_eq!((lm, rm), (Mobility::Moving, Mobility::Moving));
        }
        // A rebuild from the table passes validation, so no site is shared.
        assert_eq!(
            World::from_lef_states(lattice.clone(), &states).as_ref(),
            Ok(&world)
        );

        let again = populate(lattice.clone(), 20, &SpawnRule::UniformNonCtcf, 3).unwrap();
        assert_eq!(again, world);
        let other = populate(lattice, 20, &SpawnRule::UniformNonCtcf, 4).unwrap();
        assert_ne!(other, world);
    }

    #[test]
    fn populate_checks_the_rule_first() {
        let err = populate(Lattice::plain(3).unwrap(), 3, &SpawnRule::UniformNonCtcf, 0);
        assert_eq!(
            err,
            Err(SpawnError::NotEnoughSites {
                n_lefs: 3,
                needed: 5,
                n_plain: 3
            })
        );
    }

    #[test]
    fn populate_fills_a_tight_lattice() {
        // Exactly 2n - 1 plain sites is enough.
        let world = populate(Lattice::plain(7).unwrap(), 4, &SpawnRule::UniformNonCtcf, 9).unwrap();
        assert_eq!(world.n_lefs(), 4);
    }
}
