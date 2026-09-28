//! The [`Action`] enum, scheduling (pure), and firing (mutating) logic.

use rand::Rng;
use rand::distributions::OpenClosed01;
use smallvec::SmallVec;

use super::entity::{LefKey, LegKey, Mobility};
use super::lattice::{Direction, SiteKind};
use super::spawn::Spawn;
use super::world::World;
use super::{Schedule, SimRng, Time};

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Rates {
    /// Step rate of a moving, unblocked leg.
    pub step: f64,
    /// Respawn rate when both legs are simply moving.
    pub respawn: f64,
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
            respawn: 1e-3,
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
    /// A LEF unbinds and respawns at a new site sampled from the spawn rule.
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
    pub(crate) fn fire(
        &self,
        world: &mut World,
        spawn: &dyn Spawn,
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
/// it which actions to reschedule via [`Touched::dependent_actions`].
#[derive(Default, Debug)]
pub(crate) struct Touched {
    pub(crate) legs: SmallVec<[LegKey; 8]>,
    pub(crate) lefs: SmallVec<[LefKey; 4]>,
}

impl Touched {
    pub(crate) fn clear(&mut self) {
        self.legs.clear();
        self.lefs.clear();
    }

    /// The actions to reschedule after `fired` fired: `fired` itself first
    /// (its schedule depends on the state it just changed), then every
    /// action that depends on an entity in this set, each exactly once.
    ///
    /// This is the model's entity → action dependency map. Each reschedule
    /// draws a fresh random wait, so both the order and the absence of
    /// duplicates are part of a seeded trajectory.
    pub(crate) fn dependent_actions(&self, world: &World, fired: Action) -> SmallVec<[Action; 16]> {
        let from_legs = self.legs.iter().flat_map(|&leg_k| {
            let lef_k = world.legs[leg_k].lef;
            [Action::Step { leg: leg_k }, Action::Respawn { lef: lef_k }]
        });
        let from_lefs = self.lefs.iter().map(|&lef_k| Action::Respawn { lef: lef_k });

        let mut actions: SmallVec<[Action; 16]> = SmallVec::new();
        actions.push(fired);
        // A touched set is small (a handful of entities), so a linear scan
        // beats hashing.
        for action in from_legs.chain(from_lefs) {
            if !actions.contains(&action) {
                actions.push(action);
            }
        }
        actions
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
        _ => rates.respawn,
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
    spawn: &dyn Spawn,
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

    // Place both legs at the spawn site, landing as initial placement does.
    let kind = world.lattice.sites[new_pos as usize].kind;
    let mobility = land_lef(kind, rng);
    for (l, m) in [l1, l2].into_iter().zip(mobility) {
        let leg = &mut world.legs[l];
        leg.pos = new_pos;
        leg.mobility = m;
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
    // `libm::log` rather than `f64::ln`: `ln` calls the platform's libm,
    // which can differ in the last ulp between targets (e.g. macOS vs
    // wasm32). Event times feed heap ordering, so that would let the same
    // seed produce different trajectories on different targets. This is
    // the only transcendental in the simulation; everything else is exact
    // IEEE arithmetic or integer bit manipulation.
    -libm::log(u) / rate
}

// ----- landing on a site -----

/// The mobilities of a LEF's left and right legs as it lands, both at once,
/// on a site of `kind`: at initial placement ([`World::spawn_lef`]) and at
/// respawn (`fire_respawn`). Sharing it means the two can't diverge.
///
/// Landing isn't a step, so each leg is treated as if it arrived from the
/// side opposite its facing, as under forward-only stepping. (Open modeling
/// question: landing could equally well leave both legs Moving on the
/// grounds that "binding != stepping". See AGENTS.md for the expected
/// semantics.) Rolls the left leg, then the right.
pub(crate) fn land_lef(kind: SiteKind, rng: &mut SimRng) -> [Mobility; 2] {
    [
        resolve_mobility(kind, Direction::Left.opposite(), rng),
        resolve_mobility(kind, Direction::Right.opposite(), rng),
    ]
}

/// Stochastic resolution of a leg's mobility *at the moment it lands at a site*.
///
/// Takes `arrived_from` — the side of the destination cell from which the
/// leg arrived — rather than the leg's facing. The caller decides how to
/// derive that from its step model; in the current forward-only stepping
/// model, `arrived_from == leg.facing.opposite()`, but a model with
/// back-stepping or independent step direction would derive it differently.
///
/// # Invariant
///
/// **Only call this once per actual landing.** Capture and eviction are
/// modeled as events that fire *on the act of stepping onto a site* — not
/// as facts derivable from position. Calling this function on a leg that
/// hasn't moved would give it repeated, independent chances of being
/// captured/evicted, which is physically wrong: a leg standing still on a
/// CTCF that already failed to capture it must not get fresh rolls each
/// time the scheduler revisits it.
///
/// Concretely: never call from any `schedule_*` function or any other
/// non-mutating context. The correct call sites are exactly:
///
/// - `fire_step` — leg stepped to a new cell
/// - [`land_lef`] — a LEF placed its legs at a spawn cell, initially or
///   on respawn
///
/// # Semantics
///
/// - `Plain` sites: always returns `Moving`.
/// - `Ctcf { captures_from, strength }`:
///   - With probability `strength`: capture if `arrived_from == captures_from`,
///     otherwise evict.
///   - With probability `1 - strength`: pass through (`Moving`).
pub(crate) fn resolve_mobility(
    site: SiteKind,
    arrived_from: Direction,
    rng: &mut SimRng,
) -> Mobility {
    match site {
        SiteKind::Plain => Mobility::Moving,
        SiteKind::Ctcf { captures_from, strength } => {
            if !rng.gen_bool(strength) {
                Mobility::Moving
            } else if arrived_from == captures_from {
                Mobility::Captured
            } else {
                Mobility::Evicted
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Lattice;
    use rand::SeedableRng;

    #[test]
    fn dependent_actions_start_with_the_fired_action_and_have_no_duplicates() {
        let mut rng = SimRng::seed_from_u64(0);
        let mut world = World::new(Lattice::plain(20).unwrap());
        let (lef, [left, right]) = world.spawn_lef(10, &mut rng);
        let mut touched = Touched::default();
        touched.legs.extend([left, right, left]); // a leg touched twice
        touched.lefs.push(lef); // and the LEF both legs belong to

        let fired = Action::Respawn { lef };
        let deps = touched.dependent_actions(&world, fired);
        assert_eq!(
            deps.as_slice(),
            &[fired, Action::Step { leg: left }, Action::Step { leg: right }]
        );
    }

    #[test]
    fn plain_site_always_moving() {
        let mut rng = SimRng::seed_from_u64(0);
        assert_eq!(
            resolve_mobility(SiteKind::Plain, Direction::Right, &mut rng),
            Mobility::Moving
        );
        assert_eq!(
            resolve_mobility(SiteKind::Plain, Direction::Left, &mut rng),
            Mobility::Moving
        );
    }

    #[test]
    fn strength_1_captures_leg_arriving_on_capture_side() {
        let mut rng = SimRng::seed_from_u64(0);
        // CTCF's capture side is its right side. A leg that arrived from
        // the right should be captured.
        let site = SiteKind::Ctcf { captures_from: Direction::Right, strength: 1.0 };
        assert_eq!(
            resolve_mobility(site, Direction::Right, &mut rng),
            Mobility::Captured
        );
    }

    #[test]
    fn strength_1_evicts_leg_arriving_on_other_side() {
        let mut rng = SimRng::seed_from_u64(0);
        // CTCF's capture side is its right side; the left side evicts.
        let site = SiteKind::Ctcf { captures_from: Direction::Right, strength: 1.0 };
        assert_eq!(
            resolve_mobility(site, Direction::Left, &mut rng),
            Mobility::Evicted
        );
    }

    #[test]
    fn strength_0_always_passes_through() {
        let mut rng = SimRng::seed_from_u64(0);
        let site = SiteKind::Ctcf { captures_from: Direction::Right, strength: 0.0 };
        for _ in 0..50 {
            assert_eq!(resolve_mobility(site, Direction::Right, &mut rng), Mobility::Moving);
            assert_eq!(resolve_mobility(site, Direction::Left, &mut rng), Mobility::Moving);
        }
    }

    #[test]
    fn capture_fraction_tracks_strength() {
        let mut rng = SimRng::seed_from_u64(42);
        let p = 0.3;
        let site = SiteKind::Ctcf { captures_from: Direction::Right, strength: p };
        // Arrive from the capture side so the outcome on success is Captured.
        let n = 50_000;
        let mut captured = 0usize;
        for _ in 0..n {
            if resolve_mobility(site, Direction::Right, &mut rng) == Mobility::Captured {
                captured += 1;
            }
        }
        let frac = captured as f64 / n as f64;
        // ~4 sigma for a Bernoulli(p) sample mean.
        let sigma = (p * (1.0 - p) / n as f64).sqrt();
        assert!(
            (frac - p).abs() < 4.0 * sigma,
            "capture fraction {frac} differs from p={p} by more than 4σ ({sigma:.4})"
        );
    }
}
