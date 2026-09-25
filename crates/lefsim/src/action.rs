//! The [`Action`] enum, scheduling (pure), and firing (mutating) logic.

use rand::Rng;
use rand::distributions::OpenClosed01;
use smallvec::SmallVec;

use crate::engine::{Schedule, SimRng, Time};
use crate::spawn::SpawnRule;
use crate::world::{Direction, LefKey, LegKey, Mobility, World, resolve_mobility};

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Rates {
    /// Step rate of a moving, unblocked leg.
    pub step: f64,
    /// Respawn rate when both legs are simply moving.
    pub respawn_default: f64,
    /// Respawn rate when at least one leg is captured (longer lifetime).
    pub respawn_captured: f64,
    // Eviction is treated as an immediate respawn (dt = 0); no rate field.
}

impl Default for Rates {
    /// Reasonable demo rates: stepping is the fast process; respawns take
    /// thousands of steps; captured LEFs hang around an order of magnitude longer.
    fn default() -> Self {
        Rates {
            step: 1.0,
            respawn_default: 1e-3,
            respawn_captured: 1e-4,
        }
    }
}

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

impl Action {
    /// Names indexed by [`Action::code`].
    pub const NAMES: [&str; 2] = ["Step", "Respawn"];

    /// Stable `u8` encoding of the variant, used in recorded event frames.
    pub fn code(self) -> u8 {
        match self {
            Action::Step { .. } => 0,
            Action::Respawn { .. } => 1,
        }
    }

    /// Pure: compute when this action would next fire given the current world
    /// state and clock. Does not mutate.
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

    /// Mutating: apply this action's effects to the world. The action pushes
    /// every affected leg/LEF key into `touched` so the engine can reschedule
    /// dependents.
    pub fn fire(
        &self,
        world: &mut World,
        spawn: &dyn SpawnRule,
        _now: Time,
        rng: &mut SimRng,
        touched: &mut Touched,
    ) {
        match *self {
            Action::Step { leg } => fire_step(leg, world, rng, touched),
            Action::Respawn { lef } => fire_respawn(lef, world, spawn, rng, touched),
        }
    }
}

/// Set of entities mutated by a fired action. The model author pushes
/// every leg/LEF whose state changed (or whose neighborhood changed in a
/// way that affects scheduling) into this buffer. The engine then asks
/// it which actions need rescheduling via [`Touched::dependent_actions`].
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

    /// Iterator over actions whose schedules may have been invalidated
    /// because some entity in this set was mutated.
    /// 
    /// May yield the same action more than once (e.g. when both a leg
    /// and its LEF are in the set); the caller is responsible for dedup.
    pub fn dependent_actions<'a>(
        &'a self,
        world: &'a World,
    ) -> impl Iterator<Item = Action> + 'a {
        let from_legs = self.legs.iter().copied().flat_map(move |leg_k| {
            let lef_k = world.legs[leg_k].lef;
            [Action::Step { leg: leg_k }, Action::Respawn { lef: lef_k }]
        });
        let from_lefs = self
            .lefs
            .iter()
            .copied()
            .map(|lef_k| Action::Respawn { lef: lef_k });
        from_legs.chain(from_lefs)
    }
}

// ----- per-variant schedule implementations -----

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

// ----- per-variant fire implementations -----

fn fire_step(
    leg_k: LegKey,
    world: &mut World,
    rng: &mut SimRng,
    touched: &mut Touched,
) {
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

    // Mutate occupancy and the leg. The landing_mobility roll happens here,
    // exactly once per actual move — never in scheduling.
    //
    // Under the current forward-only step model, the step direction equals
    // `facing`, so the leg arrived from the side opposite its facing. A
    // future model with back-stepping or independent step direction would
    // derive `arrived_from` from the actual step direction instead.
    world.remove_leg(leg_k, old_pos);
    world.insert_leg(leg_k, new_pos);
    let kind = world.lattice.sites[new_pos as usize].kind;
    let arrived_from = facing.opposite();
    let leg = &mut world.legs[leg_k];
    leg.pos = new_pos;
    leg.mobility = resolve_mobility(kind, arrived_from, rng);

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

fn fire_respawn(
    lef_k: LefKey,
    world: &mut World,
    spawn: &dyn SpawnRule,
    rng: &mut SimRng,
    touched: &mut Touched,
) {
    let [l1, l2] = world.lefs[lef_k].legs;
    let p1 = world.legs[l1].pos;
    let p2 = world.legs[l2].pos;

    // Touch neighbors of both old positions; then vacate.
    for p in [p1, p2] {
        for d in [Direction::Left, Direction::Right] {
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
    world.remove_leg(l1, p1);
    world.remove_leg(l2, p2);

    let new_pos = spawn.sample(world, rng);

    // Touch neighbors of the new spawn site.
    for d in [Direction::Left, Direction::Right] {
        if let Some(np) = d.shift(new_pos)
            && (np as usize) < world.lattice.len()
        {
            for &n in world.legs_at(np) {
                touched.legs.push(n);
            }
        }
    }

    // Place both legs at the spawn site. This is a landing for each leg, so
    // each rolls landing_mobility against the new site. Respawn isn't a
    // step, so we adopt the same convention as spawn_lef: treat each leg
    // as if it arrived from the side opposite its facing. (Open modeling
    // question: respawn could equally well init both legs as Moving on
    // the grounds that "binding != stepping". See AGENTS.md for the
    // expected semantics.)
    let kind = world.lattice.sites[new_pos as usize].kind;
    for &l in &[l1, l2] {
        let facing = world.legs[l].facing;
        let arrived_from = facing.opposite();
        let leg = &mut world.legs[l];
        leg.pos = new_pos;
        leg.mobility = resolve_mobility(kind, arrived_from, rng);
    }
    world.insert_leg(l1, new_pos);
    world.insert_leg(l2, new_pos);

    touched.legs.extend([l1, l2]);
    touched.lefs.push(lef_k);
}

/// Sample an exponential dt with the given rate (events per unit time).
fn sample_exponential(rng: &mut SimRng, rate: f64) -> Time {
    // u ~ (0, 1] avoids ln(0).
    let u: f64 = rng.sample(OpenClosed01);
    -u.ln() / rate
}
