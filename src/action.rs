//! The [`Action`] enum, scheduling (pure), and firing (mutating) logic.

use rand::Rng;
use rand::distributions::OpenClosed01;
use smallvec::SmallVec;

use crate::engine::SimRng;
use crate::spawn::SpawnRule;
use crate::world::{Dir, LefKey, LegKey, Mobility, SiteKind, World};

pub type Time = f64;

/// A schedulable event.
///
/// `Action` is both the action *value* and the priority-queue *key*. Two
/// actions are equal iff they refer to the same entity-specific event.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum Action {
    /// A leg attempts to step one cell in its `facing` direction.
    Step { leg: LegKey },
    /// A LEF unbinds and respawns at a new site sampled from the [`SpawnRule`].
    Respawn { lef: LefKey },
}

/// Result of scheduling: either the next event time, or "never" (do not enqueue).
#[derive(Copy, Clone, Debug)]
pub enum Schedule {
    At(Time),
    Never,
}

#[derive(Copy, Clone, Debug)]
pub struct Rates {
    /// Step rate of a moving, unblocked leg.
    pub step: f64,
    /// Respawn rate when both legs are simply moving.
    pub respawn_default: f64,
    /// Respawn rate when at least one leg is captured (longer lifetime).
    pub respawn_captured: f64,
    // Eviction is treated as an immediate respawn (dt = 0); no rate field.
}

impl Rates {
    /// Reasonable demo rates: stepping is the fast process; respawns take
    /// thousands of steps; captured LEFs hang around an order of magnitude longer.
    pub fn default_demo() -> Self {
        Rates {
            step: 1.0,
            respawn_default: 1e-3,
            respawn_captured: 1e-4,
        }
    }
}

/// Set of entities mutated by a fired action. The engine consults this to
/// decide which actions need rescheduling. The model author is responsible
/// for pushing every leg/LEF whose state changed (or whose neighborhood
/// changed in a way that affects scheduling).
#[derive(Default, Debug)]
pub struct Touched {
    pub legs: SmallVec<[LegKey; 8]>,
    pub lefs: SmallVec<[LefKey; 4]>,
}

impl Touched {
    pub fn clear(&mut self) {
        self.legs.clear();
        self.lefs.clear();
    }
}

// ----- scheduling (pure, no mutation) -----

impl Action {
    pub fn schedule(
        &self,
        world: &World,
        rates: &Rates,
        now: Time,
        rng: &mut SimRng,
    ) -> Schedule {
        match *self {
            Action::Step { leg } => schedule_step(leg, world, rates, now, rng),
            Action::Respawn { lef } => schedule_respawn(lef, world, rates, now, rng),
        }
    }
}

fn schedule_step(
    leg_k: LegKey,
    world: &World,
    rates: &Rates,
    now: Time,
    rng: &mut SimRng,
) -> Schedule {
    let leg = &world.legs[leg_k];
    if leg.mobility != Mobility::Moving {
        return Schedule::Never;
    }
    let Some(target) = leg.facing.shift(leg.pos) else {
        return Schedule::Never;
    };
    if (target as usize) >= world.lattice.len() {
        return Schedule::Never;
    }
    if world.occupied(target) {
        return Schedule::Never;
    }
    Schedule::At(now + sample_exponential(rng, rates.step))
}

fn schedule_respawn(
    lef_k: LefKey,
    world: &World,
    rates: &Rates,
    now: Time,
    rng: &mut SimRng,
) -> Schedule {
    let [a, b] = world.lefs[lef_k].legs;
    let m1 = world.legs[a].mobility;
    let m2 = world.legs[b].mobility;
    use Mobility::*;
    let rate = match (m1, m2) {
        // Either leg evicted -> respawn immediately.
        (Evicted, _) | (_, Evicted) => return Schedule::At(now),
        // Either captured -> longer lifetime.
        (Captured, _) | (_, Captured) => rates.respawn_captured,
        _ => rates.respawn_default,
    };
    Schedule::At(now + sample_exponential(rng, rate))
}

// ----- firing (mutating) -----

pub fn fire<S: SpawnRule>(
    action: Action,
    world: &mut World,
    spawn: &S,
    _now: Time,
    rng: &mut SimRng,
    touched: &mut Touched,
) {
    match action {
        Action::Step { leg } => fire_step(leg, world, touched),
        Action::Respawn { lef } => fire_respawn(lef, world, spawn, rng, touched),
    }
}

fn fire_step(leg_k: LegKey, world: &mut World, touched: &mut Touched) {
    let (old_pos, lef_k, facing) = {
        let leg = &world.legs[leg_k];
        (leg.pos, leg.lef, leg.facing)
    };
    let new_pos = facing
        .shift(old_pos)
        .expect("step scheduled at lattice boundary");

    // Identify rear and post neighbors for rescheduling.
    let rear_neighbor = facing
        .opposite()
        .shift(old_pos)
        .filter(|p| (*p as usize) < world.lattice.len())
        .and_then(|p| world.legs_at(p).first().copied());
    let post_neighbor = facing
        .shift(new_pos)
        .filter(|p| (*p as usize) < world.lattice.len())
        .and_then(|p| world.legs_at(p).first().copied());

    // Mutate occupancy and the leg.
    world.unplace(leg_k, old_pos);
    world.place(leg_k, new_pos);
    let kind = world.lattice.sites[new_pos as usize].kind;
    let leg = &mut world.legs[leg_k];
    leg.pos = new_pos;
    leg.mobility = derive_mobility(kind, facing);

    // Tell the engine what to reschedule.
    touched.legs.push(leg_k);
    touched.lefs.push(lef_k);
    if let Some(n) = rear_neighbor
        && n != leg_k
    {
        touched.legs.push(n);
    }
    if let Some(n) = post_neighbor
        && n != leg_k
    {
        touched.legs.push(n);
    }
}

fn fire_respawn<S: SpawnRule>(
    lef_k: LefKey,
    world: &mut World,
    spawn: &S,
    rng: &mut SimRng,
    touched: &mut Touched,
) {
    let [l1, l2] = world.lefs[lef_k].legs;
    let p1 = world.legs[l1].pos;
    let p2 = world.legs[l2].pos;

    // Touch neighbors of both old positions; then vacate.
    for p in [p1, p2] {
        for d in [Dir::Left, Dir::Right] {
            if let Some(np) = d.shift(p)
                && (np as usize) < world.lattice.len()
            {
                for &n in world.legs_at(np) {
                    if n != l1 && n != l2 {
                        touched.legs.push(n);
                    }
                }
            }
        }
    }
    world.unplace(l1, p1);
    world.unplace(l2, p2);

    let new_pos = spawn.sample(world, rng);

    // Touch neighbors of the new spawn site.
    for d in [Dir::Left, Dir::Right] {
        if let Some(np) = d.shift(new_pos)
            && (np as usize) < world.lattice.len()
        {
            for &n in world.legs_at(np) {
                touched.legs.push(n);
            }
        }
    }

    // Place both legs at the spawn site, deriving mobility from it.
    let kind = world.lattice.sites[new_pos as usize].kind;
    for &l in &[l1, l2] {
        let facing = world.legs[l].facing;
        let leg = &mut world.legs[l];
        leg.pos = new_pos;
        leg.mobility = derive_mobility(kind, facing);
    }
    world.place(l1, new_pos);
    world.place(l2, new_pos);

    touched.legs.extend([l1, l2]);
    touched.lefs.push(lef_k);
}

/// Map (site kind, leg facing) -> mobility after stepping onto the site.
///
/// Conventions:
/// - `Plain` sites leave the leg `Moving`.
/// - `Ctcf { stops }` captures legs whose `facing == stops` (the leg was
///   on the CTCF's "back" side and arrived at its "front"), and evicts
///   legs whose `facing == stops.opposite()`.
pub fn derive_mobility(site: SiteKind, facing: Dir) -> Mobility {
    match site {
        SiteKind::Plain => Mobility::Moving,
        SiteKind::Ctcf { stops } => {
            if facing == stops {  // TODO: gate by capture probability
                Mobility::Captured
            } else {
                Mobility::Evicted
            }
        }
    }
}

/// Sample an exponential dt with the given rate (events per unit time).
fn sample_exponential(rng: &mut SimRng, rate: f64) -> Time {
    // u ~ (0, 1] avoids ln(0).
    let u: f64 = rng.sample(OpenClosed01);
    -u.ln() / rate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_when_facing_matches_stops() {
        assert_eq!(
            derive_mobility(SiteKind::Ctcf { stops: Dir::Right }, Dir::Right),
            Mobility::Captured
        );
        assert_eq!(
            derive_mobility(SiteKind::Ctcf { stops: Dir::Left }, Dir::Left),
            Mobility::Captured
        );
    }

    #[test]
    fn evict_when_facing_opposes_stops() {
        assert_eq!(
            derive_mobility(SiteKind::Ctcf { stops: Dir::Right }, Dir::Left),
            Mobility::Evicted
        );
        assert_eq!(
            derive_mobility(SiteKind::Ctcf { stops: Dir::Left }, Dir::Right),
            Mobility::Evicted
        );
    }

    #[test]
    fn plain_keeps_moving() {
        assert_eq!(derive_mobility(SiteKind::Plain, Dir::Right), Mobility::Moving);
        assert_eq!(derive_mobility(SiteKind::Plain, Dir::Left), Mobility::Moving);
    }
}
